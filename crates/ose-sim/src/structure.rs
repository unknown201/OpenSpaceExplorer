//! A powered vessel's structure: its parts as rigid bodies (see [`crate::rigid`]) loaded every step with the
//! thrust and drag the point-mass flight model delivers, so joints carry the original's loads and can break.
//!
//! Thrust goes to the current stage's attached engines in proportion to their rated thrust, along the vessel
//! axis. Drag is split over the attached parts by their frontal area on each vessel axis, so a vessel flying
//! sideways through thick air bends at its weak joints. Gravity is uniform across a vessel and loads nothing.
//! The structure's own motion is not used: the trajectory and attitude stay with the flight model.

use bevy_ecs::prelude::*;
use ose_data::craft::Craft;
use ose_data::{PartDef, ResourceDef};
use ose_math::DVec3;

use crate::aero::{self, DragCurves, Faces};
use crate::flight::{Powered, StepOutcome};
use crate::rigid::{PartForce, RigidVessel};

/// Loads below this (kN) leave the structure as it is rather than stepping it.
const QUIET_KN: f64 = 0.01;

#[derive(Component)]
pub struct Structure {
    pub rigid: RigidVessel,
    /// The part the vessel keeps when it breaks apart (the craft root).
    pub root: usize,
    /// Per flight stage (aligned with `FlightSpec::stages`): parts attached and engines running.
    pub stage_parts: Vec<Vec<usize>>,
    pub stage_engines: Vec<Vec<usize>>,
    /// Rated vacuum thrust of each part's engine, kN (0 for parts without one).
    pub engine_thrust_kn: Vec<f64>,
    /// Each part's drag faces after occlusion by stack neighbours (`None` without a drag cube), and its
    /// authored rotation (Unity quaternion x, y, z, w) to bring the airflow into part space.
    pub faces: Vec<Option<Faces>>,
    pub part_rot: Vec<[f64; 4]>,
    /// Each part's `Cd * A` at the last `aero` call, m^2.
    pub part_cda_m2: Vec<f64>,
    curves: DragCurves,
    /// Flight stage the structure was last trimmed to.
    staged_to: usize,
}

/// A piece that broke off this step.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub parts: Vec<usize>,
    pub mass_t: f64,
    /// Propellant of the current stage that went with it, t.
    pub propellant_t: f64,
    /// Position and velocity relative to the vessel, inertial frame.
    pub offset_m: DVec3,
    pub dv_mps: DVec3,
}

/// An orthonormal basis with `y` along the vessel axis.
fn basis(facing: DVec3) -> (DVec3, DVec3, DVec3) {
    let y = facing.normalized();
    let reference = if y.z.abs() < 0.9 {
        DVec3::new(0.0, 0.0, 1.0)
    } else {
        DVec3::new(1.0, 0.0, 0.0)
    };
    let x = y.cross(reference).normalized();
    let z = x.cross(y);
    (x, y, z)
}

impl Structure {
    pub fn build(craft: &Craft, defs: &[PartDef], resources: &[ResourceDef]) -> Structure {
        let rigid = RigidVessel::build(craft, defs, resources);
        let report = ose_data::staging::delta_v_by_stage(craft, defs, resources);
        let mut stage_parts = Vec::new();
        let mut stage_engines = Vec::new();
        // the same stage filter as `FlightSpec::from_report`
        for s in &report.stages {
            if s.thrust_kn <= 0.0 && s.start_mass_t <= 0.0 {
                continue;
            }
            stage_parts.push(s.alive.clone());
            stage_engines.push(s.engines.clone());
        }
        let engine_thrust_kn = craft
            .parts
            .iter()
            .map(|p| {
                defs.iter()
                    .find(|d| d.id == p.part_id)
                    .and_then(|d| d.engines.first())
                    .map(|e| e.max_thrust_kn)
                    .unwrap_or(0.0)
            })
            .collect();
        let root = craft
            .parts
            .iter()
            .position(|p| p.parent.is_none())
            .unwrap_or(0);
        let def_of = |i: usize| defs.iter().find(|d| d.id == craft.parts[i].part_id);
        let mut faces: Vec<Option<Faces>> = (0..craft.parts.len())
            .map(|i| {
                def_of(i)
                    .and_then(|d| d.drag_cubes.first())
                    .map(Faces::from_cube)
            })
            .collect();
        // a stack connection covers the faces on both sides of it
        for (i, p) in craft.parts.iter().enumerate() {
            let Some(parent) = p.parent else { continue };
            for (part, node) in [
                (i, p.own_node.as_deref()),
                (parent, p.parent_node.as_deref()),
            ] {
                let Some(n) = node.and_then(|n| {
                    def_of(part).and_then(|d| d.attach_nodes.iter().find(|a| a.id == n))
                }) else {
                    continue;
                };
                if !n.node_type.eq_ignore_ascii_case("stack") {
                    continue;
                }
                let dir = if n.orientation.length_squared() > 1e-9 {
                    n.orientation
                } else {
                    n.position
                };
                if let Some(f) = faces[part].as_mut() {
                    f.occlude(dir, n.size);
                }
            }
        }
        let part_rot = craft
            .parts
            .iter()
            .map(|p| {
                let q = p.local_rotation;
                if q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w < 1e-12 {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    [q.x, q.y, q.z, q.w]
                }
            })
            .collect();
        let n = craft.parts.len();
        let mut s = Structure {
            rigid,
            root,
            stage_parts,
            stage_engines,
            engine_thrust_kn,
            faces,
            part_rot,
            part_cda_m2: vec![0.0; n],
            curves: DragCurves::default(),
            staged_to: 0,
        };
        s.trim_to(0);
        s
    }

    /// Drop the parts that are not attached in flight stage `stage` (they were staged away).
    pub fn trim_to(&mut self, stage: usize) {
        self.staged_to = stage;
        let Some(alive) = self.stage_parts.get(stage) else {
            return;
        };
        let gone: Vec<usize> = (0..self.rigid.len())
            .filter(|i| self.rigid.present[*i] && !alive.contains(i))
            .collect();
        self.rigid.remove_parts(&gone);
    }

    /// Fraction of `stage`'s rated thrust whose engines are still attached.
    pub fn thrust_scale(&self, stage: usize) -> f64 {
        let Some(engines) = self.stage_engines.get(stage) else {
            return 1.0;
        };
        let total: f64 = engines.iter().map(|&i| self.engine_thrust_kn[i]).sum();
        if total <= 0.0 {
            return 1.0;
        }
        let attached: f64 = engines
            .iter()
            .filter(|&&i| self.rigid.present[i])
            .map(|&i| self.engine_thrust_kn[i])
            .sum();
        attached / total
    }

    /// Drag of the attached parts for travel through the air at `v_air` (inertial, m/s) with the vessel axis
    /// along `facing`: sets each part's `Cd * A` and returns the total, m^2. Parts without a cube add none.
    pub fn aero(&mut self, stage: usize, facing: DVec3, v_air: DVec3, rho: f64, mach: f64) -> f64 {
        if stage != self.staged_to {
            self.trim_to(stage);
        }
        let speed = v_air.length();
        self.part_cda_m2.iter_mut().for_each(|c| *c = 0.0);
        if speed <= 0.0 {
            return 0.0;
        }
        let (bx, by, bz) = basis(facing);
        let d = v_air * (1.0 / speed);
        // vessel frame, then the craft's Unity axes (Z negated)
        let du = DVec3::new(d.dot(bx), d.dot(by), -d.dot(bz));
        let mut total = 0.0;
        for i in 0..self.rigid.len() {
            if !self.rigid.present[i] {
                continue;
            }
            let area_drag = match &self.faces[i] {
                Some(f) => f.area_drag(
                    &self.curves,
                    aero::rotate_inverse(self.part_rot[i], du),
                    mach,
                ),
                None => 0.0,
            };
            let cda = aero::cda_m2(&self.curves, area_drag, rho * speed);
            self.part_cda_m2[i] = cda;
            total += cda;
        }
        total
    }

    /// Load the structure with one flight step's thrust and drag. Pieces that broke off are removed from the
    /// structure and returned; the caller takes their mass off the vessel and spawns them.
    pub fn apply(&mut self, p: &Powered, out: &StepOutcome, dt: f64) -> Vec<Piece> {
        if p.stage != self.staged_to {
            self.trim_to(p.stage);
        }
        if p.landed || p.crashed {
            return Vec::new();
        }
        if out.thrust_kn < QUIET_KN && out.drag_kn.length() < QUIET_KN {
            return Vec::new();
        }
        let (bx, by, bz) = basis(p.facing);
        let present: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .collect();
        let mut forces = Vec::new();
        // thrust over the attached engines of the burning stage
        if out.thrust_kn > 0.0 {
            if let Some(engines) = self.stage_engines.get(p.stage) {
                let live: Vec<usize> = engines
                    .iter()
                    .copied()
                    .filter(|&i| self.rigid.present[i] && self.engine_thrust_kn[i] > 0.0)
                    .collect();
                let rated: f64 = live.iter().map(|&i| self.engine_thrust_kn[i]).sum();
                for &i in &live {
                    forces.push(PartForce {
                        part: i,
                        force_kn: DVec3::new(
                            0.0,
                            out.thrust_kn * self.engine_thrust_kn[i] / rated,
                            0.0,
                        ),
                        offset_m: DVec3::ZERO,
                    });
                }
            }
        }
        // drag over the attached parts by frontal area on each axis
        let drag = DVec3::new(
            out.drag_kn.dot(bx),
            out.drag_kn.dot(by),
            out.drag_kn.dot(bz),
        );
        let cda_total: f64 = present.iter().map(|&i| self.part_cda_m2[i]).sum();
        if drag.length() >= QUIET_KN && cda_total > 0.0 {
            for &i in &present {
                forces.push(PartForce {
                    part: i,
                    force_kn: drag * (self.part_cda_m2[i] / cda_total),
                    offset_m: DVec3::ZERO,
                });
            }
        } else if drag.length() >= QUIET_KN && !present.is_empty() {
            let area = |i: usize| {
                let s = self.rigid.sizes_m[i];
                DVec3::new(s.y * s.z, s.x * s.z, s.x * s.y)
            };
            let total = present.iter().fold(DVec3::ZERO, |a, &i| a + area(i));
            for &i in &present {
                let a = area(i);
                let share = |n: f64, t: f64| if t > 0.0 { n / t } else { 0.0 };
                forces.push(PartForce {
                    part: i,
                    force_kn: DVec3::new(
                        drag.x * share(a.x, total.x),
                        drag.y * share(a.y, total.y),
                        drag.z * share(a.z, total.z),
                    ),
                    offset_m: DVec3::ZERO,
                });
            }
        }
        let broken = self.rigid.step(dt, DVec3::ZERO, &forces);
        if broken.is_empty() {
            return Vec::new();
        }
        let groups = self.rigid.groups();
        let Some(keep) = groups.iter().find(|g| g.contains(&self.root)).cloned() else {
            return Vec::new();
        };
        let (keep_c, keep_v) = self.rigid.centre_of(&keep);
        // propellant still in the stage's tanks, as a fraction of what they held at launch
        let fuel_frac = p
            .current_stage()
            .map(|s| {
                if s.propellant_t > 0.0 {
                    (p.propellant_t / s.propellant_t).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);
        let to_inertial = |v: DVec3| bx * v.x + by * v.y + bz * v.z;
        let mut pieces = Vec::new();
        for g in groups.into_iter().filter(|g| !g.contains(&self.root)) {
            let (c, v) = self.rigid.centre_of(&g);
            let dry: f64 = g.iter().map(|&i| self.rigid.dry_masses_t[i]).sum();
            let stored: f64 = g
                .iter()
                .map(|&i| self.rigid.masses_t[i] - self.rigid.dry_masses_t[i])
                .sum();
            let prop = stored * fuel_frac;
            pieces.push(Piece {
                mass_t: dry + prop,
                propellant_t: prop,
                offset_m: to_inertial(c - keep_c),
                dv_mps: to_inertial(v - keep_v),
                parts: g,
            });
        }
        for piece in &pieces {
            self.rigid.remove_parts(&piece.parts);
        }
        pieces
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::{FlightSpec, StageSpec};

    fn assets() -> (Vec<Craft>, Vec<PartDef>, Vec<ResourceDef>) {
        (
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
        )
    }

    fn flying(facing: DVec3) -> Powered {
        let spec = FlightSpec {
            stages: vec![StageSpec {
                thrust_vac_kn: 200.0,
                propellant_t: 1.0,
                start_mass_t: 5.0,
                ..Default::default()
            }],
            drag_area_m2: 1.0,
            drag_cd: 0.3,
        };
        let mut p = Powered::new(spec, DVec3::new(700_000.0, 0.0, 0.0), DVec3::ZERO, false);
        p.facing = facing;
        p
    }

    #[test]
    fn stages_line_up_with_the_flight_spec_and_staging_trims_parts() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
        let spec = FlightSpec::from_report(&report, &[], &[], 1.0);
        let mut s = Structure::build(k1, &defs, &res);
        assert_eq!(s.stage_parts.len(), spec.stages.len());
        let before = s.rigid.present.iter().filter(|x| **x).count();
        assert_eq!(before, s.stage_parts[0].len());
        s.trim_to(1);
        let after = s.rigid.present.iter().filter(|x| **x).count();
        assert!(after < before, "{before} -> {after}");
        assert_eq!(s.thrust_scale(1), 1.0);
    }

    #[test]
    fn full_thrust_with_drag_on_the_nose_breaks_nothing() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
        let mut s = Structure::build(k1, &defs, &res);
        let p = flying(DVec3::new(1.0, 0.0, 0.0));
        let out = StepOutcome {
            crashed: false,
            touched_down: false,
            thrust_kn: report.stages[0].thrust_kn,
            drag_kn: DVec3::new(-20.0, 0.0, 0.0),
        };
        for _ in 0..150 {
            let broken = s.apply(&p, &out, 0.02);
            assert!(broken.is_empty(), "{broken:?}");
        }
    }

    #[test]
    fn broadside_drag_tears_off_the_boosters_and_they_leave_with_their_mass() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(k1, &defs, &res);
        let p = flying(DVec3::new(1.0, 0.0, 0.0));
        // flying sideways through dense air: the drag is across the stack
        let out = StepOutcome {
            crashed: false,
            touched_down: false,
            thrust_kn: 0.0,
            drag_kn: DVec3::new(0.0, 0.0, -3_000.0),
        };
        let mut pieces = Vec::new();
        for _ in 0..100 {
            pieces.extend(s.apply(&p, &out, 0.02));
        }
        assert!(!pieces.is_empty(), "something should tear off");
        for piece in &pieces {
            assert!(!piece.parts.contains(&s.root));
            assert!(piece.mass_t > 0.0);
            for &i in &piece.parts {
                assert!(!s.rigid.present[i], "broken-off parts leave the structure");
            }
        }
    }
}

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
use crate::resources::{FlowNet, Instruction, Request};
use crate::rigid::{PartForce, RigidVessel, Turning};

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
    /// Tanks and the flow graph engines draw through.
    pub net: FlowNet,
    /// Each part's engine mixture (`None` for parts without an engine).
    pub engine_mixture: Vec<Option<String>>,
    /// Each part's electric charge use and production, and crew capacity.
    pub electric: Vec<ose_data::Electric>,
    pub crew: Vec<u32>,
    /// Each part's id and family (like "0010-Probe").
    pub part_ids: Vec<String>,
    pub families: Vec<String>,
    /// Experiments on the parts: (part, experiment).
    pub experiments: Vec<(usize, ose_data::science::PartExperiment)>,
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
        let engine_mixture = (0..n)
            .map(|i| {
                def_of(i)
                    .and_then(|d| d.engines.first())
                    .map(|e| e.propellant_mixture.clone())
            })
            .collect();
        let electric = (0..n)
            .map(|i| def_of(i).map(|d| d.electric).unwrap_or_default())
            .collect();
        let crew = (0..n)
            .map(|i| def_of(i).map(|d| d.crew_capacity).unwrap_or(0))
            .collect();
        let experiments = (0..n)
            .flat_map(|i| {
                def_of(i)
                    .map(|d| d.experiments.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |e| (i, e))
            })
            .collect();
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
            net: FlowNet::build(craft, defs, resources),
            engine_mixture,
            electric,
            crew,
            experiments,
            part_ids: craft.parts.iter().map(|p| p.part_id.clone()).collect(),
            families: (0..n)
                .map(|i| def_of(i).map(|d| d.family.clone()).unwrap_or_default())
                .collect(),
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
        self.net.remove_parts(&gone);
    }

    /// Engines of `stage` still attached, with their rated thrust.
    pub fn live_engines(&self, stage: usize) -> Vec<usize> {
        self.stage_engines
            .get(stage)
            .map(|e| {
                e.iter()
                    .copied()
                    .filter(|&i| self.rigid.present[i] && self.engine_thrust_kn[i] > 0.0)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Engines eligible to produce thrust in this stage (attached and supplied with all propellants).
    pub fn fed_engines(&self, stage: usize) -> Vec<usize> {
        self.live_engines(stage)
            .into_iter()
            .filter(|&i| self.fed(i))
            .collect()
    }

    /// Whether an engine can reach every ingredient of its mixture.
    fn fed(&self, engine: usize) -> bool {
        let Some(mix) = &self.engine_mixture[engine] else {
            return false;
        };
        let ing = self.net.ingredients(mix);
        !ing.is_empty()
            && ing
                .iter()
                .all(|&(r, _)| self.net.available(engine, r) > 1e-9)
    }

    /// Propellant the burning stage's engines can reach, t (each tank counted once).
    pub fn reachable_propellant_t(&self, stage: usize) -> f64 {
        let mut seen = vec![false; self.net.tanks.len()];
        let mut total = 0.0;
        for e in self.live_engines(stage) {
            let Some(mix) = &self.engine_mixture[e] else {
                continue;
            };
            for (r, _) in self.net.ingredients(mix) {
                for t in self.net.groups(e, r).into_iter().flatten() {
                    if !seen[t] {
                        seen[t] = true;
                        let tank = &self.net.tanks[t];
                        total += tank.stored * self.net.unit_mass_t[tank.resource];
                    }
                }
            }
        }
        total
    }

    /// Bring a flight state in line with the structure: trimmed to its stage, mass from what the tanks hold,
    /// propellant the engines can reach, thrust from the engines that are attached and fed.
    pub fn sync(&mut self, p: &mut Powered) {
        if p.stage != self.staged_to {
            self.trim_to(p.stage);
        }
        p.mass_t = self.net.mass_t().max(0.001);
        p.propellant_t = self.reachable_propellant_t(p.stage);
        p.thrust_scale = match self.stage_engines.get(p.stage) {
            Some(all) => {
                let rated: f64 = all.iter().map(|&i| self.engine_thrust_kn[i]).sum();
                let running: f64 = self
                    .live_engines(p.stage)
                    .into_iter()
                    .filter(|&i| self.fed(i))
                    .map(|i| self.engine_thrust_kn[i])
                    .sum();
                if rated > 0.0 {
                    running / rated
                } else {
                    1.0
                }
            }
            None => 1.0,
        };
    }

    /// One step of electric charge: solar panels (`sun` is the starlight relative to the home world, 0 in
    /// shadow; fixed panels are taken at half their rating since roll is not simulated), generators (engine
    /// alternators while their engine runs) and RTGs fill the batteries; probe cores and reaction wheels (at
    /// `wheel_input` 0..1 of full torque) draw from them. Sets the vessel's wheel power and, for an uncrewed
    /// vessel, whether its probe cores still have control.
    pub fn electric(&mut self, p: &mut Powered, sun: f64, wheel_input: f64, dt: f64) {
        let Some(ec) = self.net.resource("ElectricCharge") else {
            return;
        };
        let live = self.live_engines(p.stage);
        let running = p.throttle * p.thrust_scale;
        let mut requests = Vec::new();
        let mut wheels = Vec::new();
        let mut cores = Vec::new();
        let instruction = |units_per_s: f64| Instruction {
            resource: ec,
            units_per_s,
        };
        for i in 0..self.rigid.len() {
            if !self.rigid.present[i] {
                continue;
            }
            let e = self.electric[i];
            let mut made = 0.0;
            if e.solar_output > 0.0 {
                made += e.solar_output * sun * if e.solar_tracking { 1.0 } else { 0.5 };
            }
            if e.generator_output > 0.0 {
                if self.engine_thrust_kn[i] > 0.0 {
                    if live.contains(&i) && self.fed(i) {
                        made += e.generator_output * running;
                    }
                } else {
                    made += e.generator_output;
                }
            }
            if made > 0.0 {
                requests.push(Request {
                    part: i,
                    instructions: vec![instruction(-made)],
                    priority: 10,
                });
            }
            if e.command_use > 0.0 {
                cores.push(requests.len());
                requests.push(Request {
                    part: i,
                    instructions: vec![instruction(e.command_use)],
                    priority: 5,
                });
            }
            if e.wheel_use_per_knm > 0.0 && e.wheel_torque_vec_knm > 0.0 {
                wheels.push((requests.len(), e.wheel_torque_vec_knm, i));
                requests.push(Request {
                    part: i,
                    instructions: vec![instruction(
                        e.wheel_use_per_knm * e.wheel_torque_vec_knm * wheel_input.clamp(0.0, 1.0),
                    )],
                    priority: 4,
                });
            }
        }
        let fractions = self.net.process(&requests, dt);
        // a wheel idling draws nothing but still needs charge to turn
        let total: f64 = wheels.iter().map(|w| w.1).sum();
        p.wheel_scale = if total > 0.0 {
            wheels
                .iter()
                .map(|&(r, torque, part)| {
                    let powered = if requests[r].instructions[0].units_per_s > 0.0 {
                        fractions[r]
                    } else if self.net.available(part, ec) > 1e-9 {
                        1.0
                    } else {
                        0.0
                    };
                    torque * powered
                })
                .sum::<f64>()
                / total
        } else {
            1.0
        };
        let crewed = (0..self.rigid.len()).any(|i| self.rigid.present[i] && self.crew[i] > 0);
        p.controlled = crewed || cores.is_empty() || cores.iter().any(|&r| fractions[r] > 0.0);
        if !p.controlled {
            p.throttle = 0.0;
        }
    }

    /// Draw `burned_t` of propellant (what the flight step burned) through the flow graph, split over the
    /// running engines by rated thrust, then `sync`.
    pub fn burn(&mut self, p: &mut Powered, burned_t: f64, dt: f64) {
        if burned_t > 0.0 && dt > 0.0 {
            let engines: Vec<usize> = self
                .live_engines(p.stage)
                .into_iter()
                .filter(|&i| self.fed(i))
                .collect();
            let rated: f64 = engines.iter().map(|&i| self.engine_thrust_kn[i]).sum();
            let mut requests = Vec::new();
            for &e in &engines {
                let Some(mix) = &self.engine_mixture[e] else {
                    continue;
                };
                let unit = self.net.mixture_unit_mass_t(mix);
                if unit <= 0.0 || rated <= 0.0 {
                    continue;
                }
                let units_per_s = burned_t * self.engine_thrust_kn[e] / rated / dt / unit;
                requests.push(Request {
                    part: e,
                    instructions: self
                        .net
                        .ingredients(mix)
                        .into_iter()
                        .map(|(resource, u)| Instruction {
                            resource,
                            units_per_s: u * units_per_s,
                        })
                        .collect(),
                    priority: 0,
                });
            }
            self.net.process(&requests, dt);
        }
        self.sync(p);
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
        let (bx, by, bz) = basis(p.facing);
        let to_vessel = |v: DVec3| DVec3::new(v.dot(bx), v.dot(by), v.dot(bz));
        let alpha = to_vessel(out.angular_accel);
        let turning = alpha.length() > 1e-4 || out.angvel.length() > 1e-3;
        if p.landed
            || p.crashed
            || (out.thrust_kn < QUIET_KN && out.drag_kn.length() < QUIET_KN && !turning)
        {
            // nothing pushes the structure: let it ring down
            self.rigid.relax(dt);
            return Vec::new();
        }
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
        // turning the vessel: the wheels twist it at the root, the gimbals push sideways at the engines
        let turn = turning.then(|| {
            let need = self.rigid.torque_for(alpha);
            let share = out.wheel_share.clamp(0.0, 1.0);
            let mut t = Turning {
                alpha,
                omega: to_vessel(out.angvel),
                torques: vec![(self.root, need * share)],
                forces: Vec::new(),
            };
            let engines = self.live_engines(p.stage);
            let c = self.rigid.rest_centre();
            let gimbal = need * (1.0 - share);
            if gimbal.length() > 0.0 && !engines.is_empty() {
                let each = gimbal * (1.0 / engines.len() as f64);
                for &e in &engines {
                    let r = self.rigid.rest_position(e) - c;
                    if r.length_squared() > 1e-6 {
                        t.forces.push(PartForce {
                            part: e,
                            force_kn: each.cross(r) * (1.0 / r.length_squared()),
                            offset_m: DVec3::ZERO,
                        });
                    }
                }
            } else if gimbal.length() > 0.0 {
                t.torques[0].1 = need;
            }
            t
        });
        let broken = self
            .rigid
            .step_turning(dt, DVec3::ZERO, &forces, turn.as_ref());
        if broken.is_empty() {
            return Vec::new();
        }
        let groups = self.rigid.groups();
        let Some(keep) = groups.iter().find(|g| g.contains(&self.root)).cloned() else {
            return Vec::new();
        };
        let (keep_c, keep_v) = self.rigid.centre_of(&keep);
        let to_inertial = |v: DVec3| bx * v.x + by * v.y + bz * v.z;
        let mut pieces = Vec::new();
        for g in groups.into_iter().filter(|g| !g.contains(&self.root)) {
            let (c, v) = self.rigid.centre_of(&g);
            let mass = self.net.mass_of(&g);
            let dry: f64 = g.iter().map(|&i| self.net.dry_t[i]).sum();
            pieces.push(Piece {
                mass_t: mass,
                propellant_t: mass - dry,
                offset_m: to_inertial(c - keep_c),
                dv_mps: to_inertial(v - keep_v),
                parts: g,
            });
        }
        for piece in &pieces {
            self.rigid.remove_parts(&piece.parts);
            self.net.remove_parts(&piece.parts);
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

    fn ec_stored(s: &Structure) -> f64 {
        let ec = s.net.resource("ElectricCharge").unwrap();
        s.net
            .tanks
            .iter()
            .filter(|t| t.resource == ec && s.net.present[t.part])
            .map(|t| t.stored)
            .sum()
    }

    #[test]
    fn effect_engines_require_fuel_and_attachment() {
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(craft, &defs, &res);
        let engines = s.fed_engines(0);
        assert!(!engines.is_empty());
        s.rigid.remove_parts(&[engines[0]]);
        assert!(!s.fed_engines(0).contains(&engines[0]));
        for tank in &mut s.net.tanks {
            tank.stored = 0.0;
        }
        assert!(s.fed_engines(0).is_empty());
    }

    #[test]
    fn a_probe_without_charge_loses_control_and_the_sun_brings_it_back() {
        let (crafts, defs, res) = assets();
        // a probe core, a battery and a reaction wheel, no panels
        let drone = crafts.iter().find(|c| c.name == "Stock03_Drone").unwrap();
        let mut s = Structure::build(drone, &defs, &res);
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        let start = ec_stored(&s);
        assert!(start > 0.0);
        // turning hard in the dark
        let mut steps = 0;
        while p.controlled && steps < 1_000_000 {
            s.electric(&mut p, 0.0, 1.0, 1.0);
            steps += 1;
        }
        assert!(!p.controlled, "battery never ran out");
        assert_eq!(p.wheel_scale, 0.0);
        assert_eq!(p.throttle, 0.0);
        assert!(ec_stored(&s) < 1e-9);
        // the drone has no panels: sunlight changes nothing
        s.electric(&mut p, 1.0, 0.0, 10.0);
        assert!(!p.controlled);
    }

    #[test]
    fn kerbal_k1_panels_charge_its_batteries_in_sunlight_only() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(k1, &defs, &res);
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        let ec = s.net.resource("ElectricCharge").unwrap();
        for t in s.net.tanks.iter_mut().filter(|t| t.resource == ec) {
            t.stored *= 0.5;
        }
        let half = ec_stored(&s);
        s.electric(&mut p, 0.0, 0.0, 10.0);
        assert!(ec_stored(&s) <= half, "no sun, no charge");
        s.electric(&mut p, 1.0, 0.0, 10.0);
        let gained = ec_stored(&s) - half;
        // eight fixed 0.15/s panels at half rating: 0.6 per second
        assert!((gained - 6.0).abs() < 0.5, "gained {gained}");
        assert!(p.controlled, "a crewed pod always has control");
    }

    #[test]
    fn a_sudden_load_makes_the_boosters_wobble_and_ring_down() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
        let mut s = Structure::build(k1, &defs, &res);
        let p = flying(DVec3::new(1.0, 0.0, 0.0));
        let out = StepOutcome {
            thrust_kn: report.stages[0].thrust_kn,
            ..Default::default()
        };
        let booster = |s: &Structure| {
            s.rigid
                .links
                .iter()
                .find(|l| l.child == 21)
                .map(|l| l.bend.length())
                .unwrap()
        };
        let mut peak: f64 = 0.0;
        let mut settled = 0.0;
        for i in 0..100 {
            s.apply(&p, &out, 0.02);
            peak = peak.max(booster(&s));
            if i == 99 {
                settled = booster(&s);
            }
        }
        // the booster on its radial decoupler bends under the thrust, overshooting before it settles
        assert!(settled > 1e-3, "settled bend {settled}");
        assert!(peak > settled * 1.2, "peak {peak} settled {settled}");
        let flex = s.rigid.flex(1e-4, 1e-3);
        assert!(
            flex.iter().any(|f| f.part == 21),
            "the booster is reported as flexed"
        );
        assert!(s.rigid.max_stress() > 0.0 && s.rigid.max_stress() < 1.0);
        // thrust off: it rings down to nothing
        let quiet = StepOutcome::default();
        for _ in 0..500 {
            s.apply(&p, &quiet, 0.02);
        }
        assert!(booster(&s) < 0.1 * settled, "{}", booster(&s));
    }

    #[test]
    fn turning_hard_bends_the_stack() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(k1, &defs, &res);
        let p = flying(DVec3::new(1.0, 0.0, 0.0));
        // a hard pitch: 0.5 rad/s^2 across the vessel axis, from the wheels alone
        let out = StepOutcome {
            angular_accel: DVec3::new(0.0, 0.0, 0.5),
            angvel: DVec3::new(0.0, 0.0, 0.3),
            wheel_share: 1.0,
            ..Default::default()
        };
        for _ in 0..50 {
            s.apply(&p, &out, 0.02);
        }
        let worst = s
            .rigid
            .links
            .iter()
            .map(|l| l.torque_knm)
            .fold(0.0, f64::max);
        assert!(worst > 10.0, "turning loads the joints ({worst} kN*m)");
        assert!(
            !s.rigid.flex(1e-4, 1e-3).is_empty(),
            "something visibly bends"
        );
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
            thrust_kn: report.stages[0].thrust_kn,
            drag_kn: DVec3::new(-20.0, 0.0, 0.0),
            ..Default::default()
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
            thrust_kn: 0.0,
            drag_kn: DVec3::new(0.0, 0.0, -3_000.0),
            ..Default::default()
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

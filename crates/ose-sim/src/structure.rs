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
use crate::thermal;

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
    /// Each part's crash tolerance, m/s.
    pub crash_tolerance: Vec<f64>,
    /// Deployable solar panels folded out.
    pub deployed: Vec<bool>,
    /// Each part's id and family (like "0010-Probe").
    pub part_ids: Vec<String>,
    pub families: Vec<String>,
    /// Experiments on the parts: (part, experiment).
    pub experiments: Vec<(usize, ose_data::science::PartExperiment)>,
    /// Engine thrust on the structure, kN, lagging the flight model's thrust (see `apply`), and this step's
    /// target.
    engine_load_kn: Vec<f64>,
    engine_load_target_kn: Vec<f64>,
    /// Parts each flight stage activates (the craft's staging list for it).
    pub stage_activations: Vec<Vec<usize>>,
    /// Parachutes: definition, state and how far open (0..1).
    pub chute_defs: Vec<Option<ose_data::Parachute>>,
    pub chutes: Vec<Chute>,
    pub chute_open: Vec<f64>,
    /// Part temperatures and limits, K, thermal masses, and the re-entry heat each took last step, kW.
    pub temperature_k: Vec<f64>,
    pub max_temp_k: Vec<f64>,
    pub thermal_mass: Vec<f64>,
    pub reentry_flux_kw: Vec<f64>,
    /// Heat shields on the parts, and whether each is ablating.
    pub heatshields: Vec<Option<ose_data::Heatshield>>,
    pub ablating: Vec<bool>,
    /// Each part's bounds as a box (centre relative to the part origin, half size), vessel frame; landing
    /// gear also folded out.
    stowed_box: Vec<(DVec3, DVec3)>,
    extended_box: Vec<Option<(DVec3, DVec3)>>,
    /// Landing gear: definition and whether it is out.
    pub gear_defs: Vec<Option<ose_data::LandingGear>>,
    pub gear_out: Vec<bool>,
    /// Resource converters: definition, switched on, and the share of their rate they ran at last step.
    pub converter_defs: Vec<Option<ose_data::Converter>>,
    pub converter_on: Vec<bool>,
    pub converter_running: Vec<f64>,
    /// Each part's decoupler push (`ejectionForce`, 0 for other parts).
    pub ejection_force: Vec<f64>,
    /// RCS thrusters on the parts.
    pub rcs_defs: Vec<Option<ose_data::Rcs>>,
    /// Lifting surfaces (their curves resolved), intake areas, and each engine's jet curves.
    lift_defs: Vec<Option<(ose_data::LiftingSurface, aero::LiftCurves)>>,
    pub intake_area_m2: Vec<f64>,
    engine_jet: Vec<Option<crate::flight::Jet>>,
    /// Built nose-forward (a plane): the craft lies level on the ground, on its craft -Z... side (its top,
    /// craft -Z after `level_layout`, faces up) instead of standing on its tail.
    pub level: bool,
    /// Flight stage the structure was last trimmed to.
    staged_to: usize,
}

fn quat_mul(a: ose_data::craft::Quat, b: ose_data::craft::Quat) -> ose_data::craft::Quat {
    ose_data::craft::Quat {
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
    }
}

/// A craft built nose-forward (the root part pointing along the floor, as planes and rovers are built in
/// the hangar) turned so its pointing direction is craft +Y like a rocket's: the flight model thrusts and
/// steers along +Y. Its old top (+Y) becomes -Z. `None` for a craft that already points up.
pub fn level_layout(craft: &Craft) -> Option<Craft> {
    let root = craft.parts.iter().position(|p| p.parent.is_none())?;
    let q = craft.parts[root].local_rotation;
    let q = if q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w < 1e-12 {
        ose_data::craft::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    } else {
        q
    };
    let c = aero::rotate([q.x, q.y, q.z, q.w], DVec3::new(0.0, 1.0, 0.0));
    if c.y.abs() >= 0.5 {
        return None;
    }
    // the shortest turn from `c` to +Y
    let y = DVec3::new(0.0, 1.0, 0.0);
    let axis = c.cross(y);
    let angle = c.dot(y).clamp(-1.0, 1.0).acos();
    let axis = if axis.length() > 1e-9 {
        axis.normalized()
    } else {
        DVec3::new(1.0, 0.0, 0.0)
    };
    let (sh, ch) = (angle * 0.5).sin_cos();
    let r = ose_data::craft::Quat {
        x: axis.x * sh,
        y: axis.y * sh,
        z: axis.z * sh,
        w: ch,
    };
    let mut out = craft.clone();
    for part in &mut out.parts {
        part.local_position = aero::rotate([r.x, r.y, r.z, r.w], part.local_position);
        let pq = part.local_rotation;
        let pq = if pq.x * pq.x + pq.y * pq.y + pq.z * pq.z + pq.w * pq.w < 1e-12 {
            ose_data::craft::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            }
        } else {
            pq
        };
        part.local_rotation = quat_mul(r, pq);
    }
    Some(out)
}

/// A parachute's state (`DeploymentStates`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chute {
    #[default]
    Stowed,
    /// Waiting for the air to thicken.
    Armed,
    SemiDeployed,
    Deployed,
    /// Cut away (after landing) or torn off (opened too fast).
    Cut,
}

/// Time constant of engine thrust reaching the structure, s.
const THRUST_LAG_S: f64 = 0.4;

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

/// A part's bounds (part space, Unity axes) as a box in the vessel frame: centre relative to the part
/// origin and half size of the rotated box.
fn vessel_box(q: ose_data::craft::Quat, b: ose_data::PartBounds) -> (DVec3, DVec3) {
    let q = if q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w < 1e-12 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [q.x, q.y, q.z, q.w]
    };
    let to_vessel = |v: DVec3| {
        let u = aero::rotate(q, v);
        DVec3::new(u.x, u.y, -u.z)
    };
    let h = b.size_m * 0.5;
    let (ax, ay, az) = (
        to_vessel(DVec3::new(h.x, 0.0, 0.0)),
        to_vessel(DVec3::new(0.0, h.y, 0.0)),
        to_vessel(DVec3::new(0.0, 0.0, h.z)),
    );
    let half = DVec3::new(
        ax.x.abs() + ay.x.abs() + az.x.abs(),
        ax.y.abs() + ay.y.abs() + az.y.abs(),
        ax.z.abs() + ay.z.abs() + az.z.abs(),
    );
    (to_vessel(b.center_m), half)
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
        let mut stage_activations = Vec::new();
        for s in &report.stages {
            if s.thrust_kn <= 0.0 && s.start_mass_t <= 0.0 {
                continue;
            }
            stage_parts.push(s.alive.clone());
            stage_engines.push(s.engines.clone());
            stage_activations.push(craft.stages.get(s.stage_id).cloned().unwrap_or_default());
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
        let sizes = rigid.sizes_m.clone();
        let thermal_mass: Vec<f64> = (0..craft.parts.len())
            .map(|i| {
                let modifier = defs
                    .iter()
                    .find(|d| d.id == craft.parts[i].part_id)
                    .map(|d| d.thermal_mass_modifier)
                    .unwrap_or(1.0);
                (rigid.dry_masses_t[i] * thermal::SPECIFIC_HEAT * modifier)
                    .max(thermal::MIN_THERMAL_MASS)
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
            deployed: vec![false; n],
            crash_tolerance: (0..n)
                .map(|i| def_of(i).map(|d| d.crash_tolerance).unwrap_or(0.0))
                .collect(),
            electric,
            crew,
            experiments,
            part_ids: craft.parts.iter().map(|p| p.part_id.clone()).collect(),
            families: (0..n)
                .map(|i| def_of(i).map(|d| d.family.clone()).unwrap_or_default())
                .collect(),
            stage_activations,
            chute_defs: (0..n)
                .map(|i| def_of(i).and_then(|d| d.parachute.clone()))
                .collect(),
            chutes: vec![Chute::Stowed; n],
            chute_open: vec![0.0; n],
            temperature_k: vec![thermal::INITIAL_TEMPERATURE_K; n],
            max_temp_k: (0..n)
                .map(|i| def_of(i).map(|d| d.max_temp_k).unwrap_or(0.0))
                .collect(),
            thermal_mass,
            reentry_flux_kw: vec![0.0; n],
            heatshields: (0..n)
                .map(|i| def_of(i).and_then(|d| d.heatshield.clone()))
                .collect(),
            ablating: vec![false; n],
            level: false,
            stowed_box: (0..n)
                .map(|i| {
                    let q = craft.parts[i].local_rotation;
                    match def_of(i).and_then(|d| d.bounds) {
                        Some(b) => vessel_box(q, b),
                        None => (DVec3::ZERO, sizes[i] * 0.5),
                    }
                })
                .collect(),
            extended_box: (0..n)
                .map(|i| {
                    let q = craft.parts[i].local_rotation;
                    def_of(i)
                        .and_then(|d| d.landing_gear.as_ref())
                        .and_then(|g| g.extended)
                        .map(|b| vessel_box(q, b))
                })
                .collect(),
            converter_defs: (0..n)
                .map(|i| def_of(i).and_then(|d| d.converter.clone()))
                .collect(),
            converter_on: vec![false; n],
            lift_defs: (0..n)
                .map(|i| {
                    def_of(i).and_then(|d| d.lifting_surface.clone()).map(|l| {
                        let curves = if l.curve_set == "Custom" {
                            aero::LiftCurves {
                                lift: l.lift_curve.clone(),
                                lift_mach: l.lift_mach_curve.clone(),
                                drag: l.drag_curve.clone(),
                                drag_mach: l.drag_mach_curve.clone(),
                            }
                        } else {
                            aero::default_lift_curves()
                        };
                        (l, curves)
                    })
                })
                .collect(),
            intake_area_m2: (0..n)
                .map(|i| def_of(i).map(|d| d.intake_area_m2).unwrap_or(0.0))
                .collect(),
            engine_jet: (0..n)
                .map(|i| {
                    def_of(i)
                        .and_then(|d| d.engines.first())
                        .filter(|e| e.air_breathing)
                        .map(|e| crate::flight::Jet {
                            vel_curve: e.vel_curve.clone(),
                            atm_curve: e.atm_curve.clone(),
                        })
                })
                .collect(),
            rcs_defs: (0..n)
                .map(|i| def_of(i).and_then(|d| d.rcs.clone()))
                .collect(),
            ejection_force: (0..n)
                .map(|i| def_of(i).map(|d| d.ejection_force).unwrap_or(0.0))
                .collect(),
            converter_running: vec![0.0; n],
            gear_defs: (0..n)
                .map(|i| def_of(i).and_then(|d| d.landing_gear.clone()))
                .collect(),
            gear_out: (0..n)
                .map(|i| {
                    def_of(i)
                        .and_then(|d| d.landing_gear.as_ref())
                        .is_some_and(|g| g.starts_extended)
                })
                .collect(),
            engine_load_kn: vec![0.0; n],
            engine_load_target_kn: vec![0.0; n],
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
    /// What the burning stage's engines' tanks hold when full, t (each tank counted once).
    pub fn reachable_capacity_t(&self, stage: usize) -> f64 {
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
                        total += tank.capacity * self.net.unit_mass_t[tank.resource];
                    }
                }
            }
        }
        total
    }

    /// How the attached parts meet the ground standing upright: (centre of mass height above the lowest
    /// point, radius of the parts it stands on, lowest crash tolerance among them).
    pub fn ground_profile(&self) -> (f64, f64, f64) {
        let (clearance, footprint, tolerance, _) = self.feet();
        (clearance, footprint, tolerance)
    }

    /// A part's box at rest (centre, half size), vessel frame, with landing gear as it is now.
    pub fn part_box(&self, i: usize) -> (DVec3, DVec3) {
        let (offset, half) = match (&self.extended_box[i], self.gear_out[i]) {
            (Some(b), true) => *b,
            _ => self.stowed_box[i],
        };
        (self.rigid.rest_position(i) + offset, half)
    }

    /// The RCS thrusters still attached: thrust, turning torque (each thruster at its distance from the
    /// centre of mass) and Isp.
    fn sync_rcs(&self, p: &mut Powered) {
        let c = self.rigid.rest_centre();
        let (mut thrust, mut torque) = (0.0, 0.0);
        let mut isp = (0.0, 0.0);
        for i in (0..self.rigid.len()).filter(|&i| self.rigid.present[i]) {
            if let Some(r) = &self.rcs_defs[i] {
                thrust += r.thrust_kn;
                torque += r.thrust_kn * (self.rigid.rest_position(i) - c).length();
                if isp.0 == 0.0 {
                    isp = (r.isp_curve.evaluate(0.0), r.isp_curve.evaluate(1.0));
                }
            }
        }
        p.rcs.thrust_kn = thrust;
        p.rcs.torque_knm = torque;
        p.rcs.isp_vac_s = isp.0;
        p.rcs.isp_asl_s = isp.1;
    }

    /// Draw the propellant the RCS used this step from the tanks; sets the share delivered.
    pub fn rcs_burn(&mut self, p: &mut Powered, pressure_atm: f64, dt: f64) {
        if p.rcs.usage <= 0.0 || p.rcs.thrust_kn <= 0.0 {
            return;
        }
        let Some(def) = self.rcs_defs.iter().flatten().next().cloned() else {
            return;
        };
        let isp = def.isp_curve.evaluate(pressure_atm.max(0.0)).max(1e-3);
        // t/s of propellant at the thrust used
        let flow_t = p.rcs.thrust_kn * p.rcs.usage.min(3.0) / (isp * ose_data::G0);
        let ingredients = {
            let mix = self.net.ingredients(&def.propellant);
            if mix.is_empty() {
                self.net
                    .resources
                    .iter()
                    .position(|r| r.eq_ignore_ascii_case(&def.propellant))
                    .map(|r| vec![(r, 1.0)])
                    .unwrap_or_default()
            } else {
                mix
            }
        };
        let unit_t: f64 = ingredients
            .iter()
            .map(|&(r, u)| self.net.unit_mass_t[r] * u)
            .sum();
        if ingredients.is_empty() || unit_t <= 0.0 {
            p.rcs.powered = 0.0;
            return;
        }
        let units = flow_t / unit_t;
        let request = Request {
            part: self.root,
            instructions: ingredients
                .iter()
                .map(|&(r, u)| Instruction {
                    resource: r,
                    units_per_s: units * u,
                })
                .collect(),
            priority: 6,
        };
        p.rcs.powered = self.net.process(&[request], dt)[0];
    }

    /// The wheels out under the craft, its drive direction and wheelbase, into the flight state's `drive`.
    fn sync_drive(&self, p: &mut Powered) {
        let wheels: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| {
                self.rigid.present[i]
                    && self.gear_out[i]
                    && self.gear_defs[i]
                        .as_ref()
                        .is_some_and(|g| !g.leg && g.radius_m > 0.0)
            })
            .collect();
        p.drive.wheels = wheels
            .iter()
            .map(|&i| {
                let g = self.gear_defs[i].as_ref().expect("gear");
                crate::flight::DriveWheel {
                    radius_m: g.radius_m,
                    torque_knm: g.motor.as_ref().map(|m| m.torque_curve.clone()),
                    ec_per_s: g.motor.as_ref().map(|m| m.ec_per_s).unwrap_or(0.0),
                    brake_knm: g.brake_torque_knm,
                    steers: g.steerable,
                }
            })
            .collect();
        // the root part points the way it drives (a probe core's up), laid flat; craft +Z otherwise
        let up = aero::rotate(self.part_rot[self.root], DVec3::new(0.0, 1.0, 0.0));
        let flat = (up.x * up.x + up.z * up.z).sqrt();
        p.drive.forward = if flat > 0.3 {
            [up.x / flat, up.z / flat]
        } else {
            [0.0, 1.0]
        };
        // wheel spread along that direction (positions in craft axes: vessel z is craft -Z); a level craft
        // drives along its nose
        let along: Vec<f64> = wheels
            .iter()
            .map(|&i| {
                let r = self.rigid.rest_position(i);
                if self.level {
                    r.y
                } else {
                    r.x * p.drive.forward[0] - r.z * p.drive.forward[1]
                }
            })
            .collect();
        let spread = along.iter().cloned().fold(f64::MIN, f64::max)
            - along.iter().cloned().fold(f64::MAX, f64::min);
        p.drive.wheelbase_m = if spread.is_finite() {
            spread.max(0.5)
        } else {
            1.0
        };
    }

    /// Switch the resource converters (fuel cells, reactors) on or off; returns how many changed.
    pub fn set_converters(&mut self, on: bool) -> usize {
        let mut changed = 0;
        for i in 0..self.converter_on.len() {
            if self.converter_defs[i].is_some()
                && self.rigid.present[i]
                && self.converter_on[i] != on
            {
                self.converter_on[i] = on;
                changed += 1;
            }
        }
        changed
    }

    /// Fold the landing gear out (or in); returns how many changed.
    pub fn deploy_gear(&mut self, out: bool) -> usize {
        let mut changed = 0;
        for i in 0..self.gear_out.len() {
            let foldable = self.gear_defs[i].as_ref().is_some_and(|g| g.retractable);
            if foldable && self.rigid.present[i] && self.gear_out[i] != out {
                self.gear_out[i] = out;
                changed += 1;
            }
        }
        changed
    }

    /// `ground_profile` and the parts that touch the ground first.
    fn feet(&self) -> (f64, f64, f64, Vec<usize>) {
        let present: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .collect();
        if present.is_empty() {
            return (0.0, 0.0, 0.0, Vec::new());
        }
        let c = self.rigid.rest_centre();
        // up in the vessel frame: +Y standing, +Z lying level (craft -Z; the vessel frame negates Z)
        let u = self.up_axis();
        let bottom = |i: usize| {
            let (centre, half) = self.part_box(i);
            centre.dot(u) - (half.x * u.x.abs() + half.y * u.y.abs() + half.z * u.z.abs())
        };
        let lowest = present
            .iter()
            .map(|&i| bottom(i))
            .fold(f64::INFINITY, f64::min);
        // the parts within half a metre of the lowest point carry it
        let feet: Vec<usize> = present
            .iter()
            .copied()
            .filter(|&i| bottom(i) < lowest + 0.5)
            .collect();
        let footprint = feet
            .iter()
            .map(|&i| {
                let (centre, half) = self.part_box(i);
                let p = centre - c;
                let across = p - u * p.dot(u);
                let spread = if self.level {
                    half.x.max(half.y)
                } else {
                    half.x.max(half.z)
                };
                across.length() + spread
            })
            .fold(0.0, f64::max);
        // the impact lands on the parts that touch first (within 15 cm of the lowest point)
        let contact: Vec<usize> = feet
            .iter()
            .copied()
            .filter(|&i| bottom(i) < lowest + 0.15)
            .collect();
        let tolerance = contact
            .iter()
            .map(|&i| self.crash_tolerance[i])
            .filter(|t| *t > 0.0)
            .fold(f64::INFINITY, f64::min);
        (
            (c.dot(u) - lowest).max(0.0),
            footprint,
            if tolerance.is_finite() {
                tolerance
            } else {
                0.0
            },
            contact,
        )
    }

    /// Which way is up on the ground, vessel frame: the axis standing (+Y), or the craft's top lying level.
    fn up_axis(&self) -> DVec3 {
        if self.level {
            DVec3::new(0.0, 0.0, 1.0)
        } else {
            DVec3::new(0.0, 1.0, 0.0)
        }
    }

    /// Bring a flight state in line with the structure: trimmed to its stage, mass from what the tanks hold,
    /// propellant the engines can reach, thrust from the engines that are attached and fed.
    pub fn sync(&mut self, p: &mut Powered) {
        if p.stage != self.staged_to {
            self.trim_to(p.stage);
        }
        p.mass_t = self.net.mass_t().max(0.001);
        self.sync_drive(p);
        self.sync_rcs(p);
        p.jet = self
            .live_engines(p.stage)
            .into_iter()
            .find_map(|i| self.engine_jet[i].clone());
        p.intakes =
            (0..self.rigid.len()).any(|i| self.rigid.present[i] && self.intake_area_m2[i] > 0.0);
        let (clearance, footprint, tolerance) = self.ground_profile();
        p.ground_clearance_m = clearance;
        p.footprint_m = footprint;
        p.crash_tolerance_mps = tolerance;
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
            if e.solar_output > 0.0 && (!e.solar_deployable || self.deployed[i]) {
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
        // converters that are on take their inputs and make their outputs in one go (none when the outputs
        // have no room, so a fuel cell idles on full batteries)
        let mut converting = Vec::new();
        for i in 0..self.rigid.len() {
            let Some(c) = self.converter_defs[i].as_ref() else {
                continue;
            };
            self.converter_running[i] = 0.0;
            if !self.converter_on[i] || !self.rigid.present[i] {
                continue;
            }
            let mut instructions = Vec::new();
            let mut known = true;
            for (res, rate, sign) in c
                .inputs
                .iter()
                .map(|(r, u)| (r, u, 1.0))
                .chain(c.outputs.iter().map(|(r, u)| (r, u, -1.0)))
            {
                match self.net.resource(res) {
                    Some(id) => instructions.push(Instruction {
                        resource: id,
                        units_per_s: sign * rate * c.conversion_rate,
                    }),
                    None => known = false,
                }
            }
            if known && !instructions.is_empty() {
                converting.push((requests.len(), i));
                requests.push(Request {
                    part: i,
                    instructions,
                    priority: 9,
                });
            }
        }
        // wheel motors draw in proportion to the drive input
        let motor_use: f64 = p
            .drive
            .wheels
            .iter()
            .filter(|w| w.torque_knm.is_some())
            .map(|w| w.ec_per_s)
            .sum::<f64>()
            * p.drive.throttle.abs().clamp(0.0, 1.0);
        let motors = (motor_use > 0.0).then(|| {
            requests.push(Request {
                part: self.root,
                instructions: vec![instruction(motor_use)],
                priority: 4,
            });
            requests.len() - 1
        });
        let fractions = self.net.process(&requests, dt);
        for &(r, i) in &converting {
            self.converter_running[i] = fractions[r];
        }
        p.drive.powered = match motors {
            Some(r) => fractions[r],
            None => {
                if self.net.available(self.root, ec) > 1e-9 {
                    1.0
                } else {
                    0.0
                }
            }
        };
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

    /// Heat the parts for one step (see `thermal`): `v_air` is the velocity through the air (inertial), the
    /// air at the vessel has `density`, `mach`, `ambient_k` and `conductivity`. Returns the parts that went
    /// past their maximum temperature (when parts may burn up).
    #[allow(clippy::too_many_arguments)]
    pub fn update_heat(
        &mut self,
        facing: DVec3,
        v_air: DVec3,
        density: f64,
        mach: f64,
        ambient_k: f64,
        conductivity: f64,
        settings: &thermal::HeatSettings,
        dt: f64,
    ) -> Vec<usize> {
        let present: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .collect();
        let (bx, by, bz) = basis(facing);
        let speed = v_air.length();
        let d = if speed > 0.0 {
            DVec3::new(v_air.dot(bx), v_air.dot(by), v_air.dot(bz)) * (1.0 / speed)
        } else {
            DVec3::ZERO
        };
        let du = DVec3::new(d.x, d.y, -d.z);
        let exposed = if speed > 0.0 {
            let boxes: Vec<(DVec3, DVec3)> = present.iter().map(|&i| self.part_box(i)).collect();
            thermal::exposed_proportions(&boxes, d, mach)
        } else {
            vec![0.0; present.len()]
        };
        let mut burned = Vec::new();
        for (k, &i) in present.iter().enumerate() {
            let local = aero::rotate_inverse(self.part_rot[i], du);
            let (airstream, cd, wetted) = match &self.faces[i] {
                Some(f) => {
                    let (a, cd) = f.airstream(&self.curves, local, mach);
                    (a, cd, f.wetted())
                }
                None => {
                    let s = self.rigid.sizes_m[i];
                    (0.0, 0.0, 2.0 * (s.x * s.y + s.y * s.z + s.x * s.z))
                }
            };
            let mut flux = thermal::reentry_flux(mach, exposed[k], airstream, density, cd);
            // a heat shield above its ablation temperature turns the heat away while it faces the flow
            // (`Module_Heatshield.ThermalUpdate`) and burns ablator for the heat it takes
            // (`PartComponentModule_Heatshield`)
            self.ablating[i] = false;
            if let Some(h) = self.heatshields[i].clone() {
                let left = self.resource_fraction(i, &h.ablator);
                if self.temperature_k[i] > h.ablation_temp_k && left > 0.001 {
                    self.ablating[i] = true;
                    let over = ((self.temperature_k[i] - h.ablation_temp_k)
                        / h.ablation_max_over_k.max(1e-6))
                    .clamp(0.0, 1.0);
                    let dir = aero::rotate(self.part_rot[i], h.shielding_direction);
                    let dir = DVec3::new(dir.x, dir.y, -dir.z);
                    let facing_flow = (d * speed).dot(dir).clamp(0.0, 1.0);
                    flux -= flux * over * facing_flow * h.shielding_scale;
                    let removed = if facing_flow < 1.0 {
                        flux / (1.0 - facing_flow)
                    } else {
                        flux
                    };
                    let units = removed.max(0.0) * h.pyrolysis_loss_factor * h.ablator_rate * dt;
                    self.drain_part(i, &h.ablator, units);
                }
            }
            self.reentry_flux_kw[i] = flux;
            self.temperature_k[i] = thermal::step_temperature(
                self.temperature_k[i],
                self.thermal_mass[i],
                wetted,
                ambient_k,
                conductivity,
                flux,
                settings.scale,
                dt,
            );
            if settings.part_destruction
                && self.max_temp_k[i] > 0.0
                && self.temperature_k[i] > self.max_temp_k[i]
            {
                burned.push(i);
            }
        }
        burned
    }

    /// How full a part's own tanks of `resource` are, 0..1.
    fn resource_fraction(&self, part: usize, resource: &str) -> f64 {
        let Some(r) = self.net.resource(resource) else {
            return 0.0;
        };
        let (stored, capacity) = self
            .net
            .tanks
            .iter()
            .filter(|t| t.part == part && t.resource == r)
            .fold((0.0, 0.0), |(s, c), t| (s + t.stored, c + t.capacity));
        if capacity > 0.0 {
            stored / capacity
        } else {
            0.0
        }
    }

    fn drain_part(&mut self, part: usize, resource: &str, mut units: f64) {
        let Some(r) = self.net.resource(resource) else {
            return;
        };
        for t in self
            .net
            .tanks
            .iter_mut()
            .filter(|t| t.part == part && t.resource == r)
        {
            let take = units.min(t.stored);
            t.stored -= take;
            units -= take;
        }
    }

    /// A touchdown at `speed`, m/s, harder than the parts it lands on can take (`crashTolerance`): those parts
    /// break, taking up the impact their tolerance allows (speed² less tolerance²), and the next parts down
    /// meet what is left. Returns the broken parts and the pieces that came loose, or `None` when the root
    /// broke (the vessel is lost).
    pub fn impact(&mut self, mut speed: f64, facing: DVec3) -> Option<(Vec<usize>, Vec<Piece>)> {
        let (bx, by, bz) = basis(facing);
        let mut broken = Vec::new();
        let mut pieces = Vec::new();
        loop {
            let (_, _, tolerance, feet) = self.feet();
            let tolerance = if tolerance > 0.0 {
                tolerance
            } else {
                crate::flight::CRASH_SPEED_MPS
            };
            if feet.is_empty() || speed <= tolerance {
                break;
            }
            if feet.contains(&self.root) {
                return None;
            }
            self.rigid.remove_parts(&feet);
            self.net.remove_parts(&feet);
            broken.extend(feet);
            pieces.extend(self.split_off(bx, by, bz));
            speed = (speed * speed - tolerance * tolerance).max(0.0).sqrt();
        }
        Some((broken, pieces))
    }

    /// Destroy parts that burned up: they leave the structure, and whatever only they held on breaks off
    /// as pieces. `None` when the root burned (the vessel is lost).
    pub fn burn_up(&mut self, parts: &[usize], facing: DVec3) -> Option<Vec<Piece>> {
        if parts.contains(&self.root) {
            return None;
        }
        self.rigid.remove_parts(parts);
        self.net.remove_parts(parts);
        let (bx, by, bz) = basis(facing);
        Some(self.split_off(bx, by, bz))
    }

    /// The push the decouplers in flight stage `stage` give the parts they let go, kN·s
    /// (`Module_Decouple`: half the ejection force over one 50 Hz step each way).
    pub fn ejection_impulse(&self, stage: usize) -> f64 {
        self.stage_activations
            .get(stage)
            .map(|parts| {
                parts
                    .iter()
                    .map(|&i| self.ejection_force.get(i).copied().unwrap_or(0.0))
                    .sum::<f64>()
            })
            .unwrap_or(0.0)
            * 0.5
            * 0.02
    }

    /// Fire flight stage `stage`'s activations: parachutes in it arm.
    pub fn activate_stage(&mut self, stage: usize) {
        let parts = self
            .stage_activations
            .get(stage)
            .cloned()
            .unwrap_or_default();
        for i in parts {
            self.arm_chute(i);
        }
    }

    fn arm_chute(&mut self, i: usize) {
        if self.chute_defs.get(i).is_some_and(|c| c.is_some())
            && self.rigid.present[i]
            && self.chutes[i] == Chute::Stowed
        {
            self.chutes[i] = Chute::Armed;
        }
    }

    /// Arm every packed parachute (the original's deploy action); returns how many.
    pub fn arm_all_chutes(&mut self) -> usize {
        let before = self.chutes.iter().filter(|c| **c == Chute::Armed).count();
        for i in 0..self.chutes.len() {
            self.arm_chute(i);
        }
        self.chutes.iter().filter(|c| **c == Chute::Armed).count() - before
    }

    /// Advance the parachutes: an armed one opens partly once the air is thick enough, fully below its deploy
    /// altitude above the ground, opening at its animation rate; it is cut away once landed and nearly
    /// still, and torn off if it opens fully faster than about Mach 1 (`MachSafetyScalar` 0.95).
    pub fn update_chutes(
        &mut self,
        pressure_atm: f64,
        height_m: f64,
        mach: f64,
        landed: bool,
        speed: f64,
        dt: f64,
    ) {
        for i in 0..self.chutes.len() {
            let Some(c) = self.chute_defs[i].clone() else {
                continue;
            };
            if !self.rigid.present[i] {
                continue;
            }
            match self.chutes[i] {
                Chute::Armed if pressure_atm >= c.min_pressure_atm && !landed => {
                    self.chutes[i] = Chute::SemiDeployed;
                    self.chute_open[i] = 0.0;
                }
                Chute::SemiDeployed => {
                    self.chute_open[i] =
                        (self.chute_open[i] + c.semi_deploy_rate.max(0.05) * dt).min(1.0);
                    if height_m <= c.deploy_altitude_m && self.chute_open[i] >= 1.0 {
                        self.chutes[i] = Chute::Deployed;
                        self.chute_open[i] = 0.0;
                    }
                }
                Chute::Deployed => {
                    self.chute_open[i] =
                        (self.chute_open[i] + c.deploy_rate.max(0.05) * dt).min(1.0);
                    if mach > 0.95 {
                        self.chutes[i] = Chute::Cut;
                    }
                }
                _ => {}
            }
            if landed
                && speed < c.auto_cut_speed_mps.max(0.5)
                && matches!(self.chutes[i], Chute::SemiDeployed | Chute::Deployed)
            {
                self.chutes[i] = Chute::Cut;
            }
        }
    }

    /// Fold all deployable solar panels out (or back in); returns how many changed.
    pub fn deploy_panels(&mut self, out: bool) -> usize {
        let mut n = 0;
        for i in 0..self.electric.len() {
            if self.electric[i].solar_deployable
                && self.electric[i].solar_output > 0.0
                && self.rigid.present[i]
                && self.deployed[i] != out
            {
                self.deployed[i] = out;
                n += 1;
            }
        }
        n
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

    /// Lift and drag of the attached lifting surfaces (wings, fins, control surfaces) for travel through air
    /// of density `rho` at `v_air` (inertial) with the vessel axis along `facing`, kN, inertial.
    pub fn lift(&self, facing: DVec3, v_air: DVec3, rho: f64, mach: f64) -> DVec3 {
        let speed = v_air.length();
        if speed <= 0.0 || rho <= 0.0 {
            return DVec3::ZERO;
        }
        let (bx, by, bz) = basis(facing);
        let travel = v_air * (1.0 / speed);
        let q = 0.5 * rho * speed * speed;
        let mut total = DVec3::ZERO;
        for i in 0..self.rigid.len() {
            let Some((l, curves)) = &self.lift_defs[i] else {
                continue;
            };
            if !self.rigid.present[i] || l.deflection_lift_coeff <= 0.0 {
                continue;
            }
            // the surface's axis: part space (Unity) to the craft, to the vessel frame (Z negated), to inertial
            let u = aero::rotate(self.part_rot[i], l.axis);
            let axis = bx * u.x + by * u.y + bz * (-u.z);
            total = total
                + aero::surface_force(
                    curves,
                    l.deflection_lift_coeff,
                    l.omnidirectional,
                    axis,
                    travel,
                    q,
                    mach,
                );
        }
        total
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
            let local = aero::rotate_inverse(self.part_rot[i], du);
            let mut area_drag = match &self.faces[i] {
                Some(f) => f.area_drag(&self.curves, local, mach),
                None => 0.0,
            };
            // an opening parachute blends from its packed shape to its semi-deployed and deployed shapes
            if let Some(c) = &self.chute_defs[i] {
                let shape = |cube: &Option<ose_data::DragCube>| {
                    cube.as_ref()
                        .map(|q| Faces::from_cube(q).area_drag(&self.curves, local, mach))
                        .unwrap_or(area_drag)
                };
                let t = self.chute_open[i];
                area_drag = match self.chutes[i] {
                    Chute::SemiDeployed => area_drag + (shape(&c.semi_cube) - area_drag) * t,
                    Chute::Deployed => {
                        let semi = shape(&c.semi_cube);
                        semi + (shape(&c.deployed_cube) - semi) * t
                    }
                    _ => area_drag,
                };
            }
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
            self.engine_load_kn.iter_mut().for_each(|x| *x = 0.0);
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
                // only engines that still have propellant push
                let live: Vec<usize> = engines
                    .iter()
                    .copied()
                    .filter(|&i| {
                        self.rigid.present[i] && self.engine_thrust_kn[i] > 0.0 && self.fed(i)
                    })
                    .collect();
                let rated: f64 = live.iter().map(|&i| self.engine_thrust_kn[i]).sum();
                for &i in &live {
                    self.engine_load_target_kn[i] =
                        out.thrust_kn * self.engine_thrust_kn[i] / rated;
                }
            }
        }
        // Engine thrust reaches the structure through a short lag: solid boosters tail off and liquid engines
        // spool (the original's engine response and thrust curves) rather than stepping, which would ring the
        // joints far past their static load.
        let k = 1.0 - (-dt / THRUST_LAG_S).exp();
        for i in 0..self.engine_load_kn.len() {
            let target = self.engine_load_target_kn[i];
            self.engine_load_target_kn[i] = 0.0;
            self.engine_load_kn[i] += (target - self.engine_load_kn[i]) * k;
            if self.rigid.present[i] && self.engine_load_kn[i].abs() > 1e-6 {
                forces.push(PartForce {
                    part: i,
                    force_kn: DVec3::new(0.0, self.engine_load_kn[i], 0.0),
                    offset_m: DVec3::ZERO,
                });
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
        self.split_off(bx, by, bz)
    }

    /// Take the groups no longer joined to the root out of the structure as pieces (`bx`..`bz`: the vessel
    /// frame in inertial axes).
    fn split_off(&mut self, bx: DVec3, by: DVec3, bz: DVec3) -> Vec<Piece> {
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

/// A Mk1 pod with its parachute on top and, if `shield`, a 1.25 m heat shield under it, with the stock part
/// catalog (for heat tests).
#[cfg(test)]
pub(crate) fn test_capsule(shield: bool) -> (Craft, Vec<PartDef>, Vec<ResourceDef>) {
    let crafts: Vec<Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let c = crafts
        .iter()
        .find(|c| c.name == "Stock12_StartRocket")
        .unwrap();
    let mut parts = vec![c.parts[0].clone()];
    if shield {
        let mut h = c.parts[1].clone();
        h.part_id = "heatshield_1v".into();
        h.local_position.y = -0.392;
        h.resources = vec![ose_data::craft::StoredResource {
            resource: "Ablator".into(),
            stored_units: 0.2,
            capacity_units: 0.2,
        }];
        parts.push(h);
    }
    let mut chute = c.parts[4].clone();
    chute.parent = Some(0);
    parts.push(chute);
    let craft = Craft {
        name: "Capsule".into(),
        description: String::new(),
        stages: vec![vec![parts.len() - 1]],
        parts,
    };
    (craft, defs, res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::{FlightSpec, StageSpec};

    /// The starter rocket with three large landing legs on its tank.
    fn lander() -> (Craft, Vec<PartDef>, Vec<ResourceDef>) {
        let (crafts, defs, res) = assets();
        let mut c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap()
            .clone();
        let tank = 2;
        for k in 0..3 {
            let a = k as f64 * std::f64::consts::TAU / 3.0;
            let mut leg = c.parts[tank].clone();
            leg.guid = format!("leg{k}");
            leg.part_id = "landingleg_2v".into();
            leg.parent = Some(tank);
            leg.own_node = None;
            leg.parent_node = None;
            leg.resources = Vec::new();
            leg.local_position = DVec3::new(0.65 * a.cos(), -0.9, 0.65 * a.sin());
            let half = -a * 0.5 + std::f64::consts::FRAC_PI_4;
            leg.local_rotation = ose_data::craft::Quat {
                x: 0.0,
                y: half.sin(),
                z: 0.0,
                w: half.cos(),
            };
            c.parts.push(leg);
        }
        (c, defs, res)
    }

    #[test]
    fn a_planes_wings_lift_it_with_its_nose_up_and_its_jet_needs_air() {
        let (crafts, defs, res) = assets();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let s = Structure::build(c, &defs, &res);
        // vessel axis (craft +Y, the plane's top) along world +Y; the nose (craft +Z) is world -Z
        let facing = DVec3::new(0.0, 1.0, 0.0);
        let (_, by, bz) = basis(facing);
        let nose = -bz;
        let at = |aoa_deg: f64| {
            let a = aoa_deg.to_radians();
            // flying 80 m/s along the nose, the flight path below it by the angle of attack
            let travel = nose * a.cos() - by * a.sin();
            s.lift(facing, travel * 80.0, 1.2, 0.24)
        };
        let level = at(0.0);
        let up5 = at(5.0);
        let up10 = at(10.0);
        // lift grows with the angle of attack, pointing up through the wings
        assert!(up5.dot(by) > level.dot(by) + 1.0, "{level:?} {up5:?}");
        assert!(up10.dot(by) > up5.dot(by), "{up5:?} {up10:?}");
        // at 10 degrees the wings carry the plane (7.8 t) at 80 m/s
        let weight_kn = 7.78 * 9.81;
        assert!(
            up10.dot(by) > weight_kn,
            "lift {} kN vs weight {weight_kn}",
            up10.dot(by)
        );
        // and drag holds it back
        assert!(up10.dot(nose) < 0.0);
        // no air, no lift
        assert_eq!(s.lift(facing, nose * 80.0, 0.0, 0.0), DVec3::ZERO);
        // the Wheesley breathes: thrust follows the air, nothing in vacuum or without intakes
        let mut p = flying(facing);
        let mut s = s;
        s.sync(&mut p);
        let jet = p.jet.clone().expect("a jet stage");
        assert!(p.intakes);
        let sea = jet.factor(0.3, 1.2, true, true);
        assert!(sea > 0.8 && sea <= 1.0, "{sea}");
        assert!(jet.factor(0.3, 0.2, true, true) < 0.5 * sea, "thin air");
        assert_eq!(jet.factor(0.3, 1.2, false, true), 0.0, "no oxygen");
        assert_eq!(jet.factor(3.0, 1.2, true, true), 0.0, "past its top speed");
        assert!(
            p.propellant_t > 0.5,
            "methane reaches the jet: {} t",
            p.propellant_t
        );
    }

    #[test]
    fn a_fuel_cell_turns_methalox_into_charge_until_the_batteries_are_full() {
        // the starter rocket with a fuel cell on its tank
        let (crafts, defs, res) = assets();
        let mut c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap()
            .clone();
        let mut cell = c.parts[2].clone();
        cell.guid = "cell".into();
        cell.part_id = "fuelcell_0v".into();
        cell.parent = Some(2);
        cell.own_node = None;
        cell.parent_node = None;
        cell.resources = Vec::new();
        cell.local_position = DVec3::new(0.7, -0.9, 0.0);
        c.parts.push(cell);
        let mut s = Structure::build(&c, &defs, &res);
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        let ec = s.net.resource("ElectricCharge").unwrap();
        let methane = s.net.resource("Methane").unwrap();
        let stored = |s: &Structure, r: usize| -> f64 {
            s.net
                .tanks
                .iter()
                .filter(|t| t.resource == r)
                .map(|t| t.stored)
                .sum()
        };
        // flatten the batteries, in the dark
        for t in s.net.tanks.iter_mut().filter(|t| t.resource == ec) {
            t.stored = 0.0;
        }
        let fuel0 = stored(&s, methane);
        assert!(fuel0 > 0.0, "the tank carries methalox");
        // off: nothing happens
        s.electric(&mut p, 0.0, 0.0, 10.0);
        assert!(stored(&s, ec) < 1e-9);
        assert_eq!(s.set_converters(true), 1);
        s.electric(&mut p, 0.0, 0.0, 10.0);
        let made = stored(&s, ec);
        assert!(made > 1.0, "charge from the fuel cells: {made}");
        assert!(stored(&s, methane) < fuel0, "methane burned");
        assert!(s.converter_running.iter().any(|f| *f > 0.99));
        // run until full: then they idle and burn nothing
        for _ in 0..2_000 {
            s.electric(&mut p, 0.0, 0.0, 10.0);
        }
        let full = stored(&s, methane);
        s.electric(&mut p, 0.0, 0.0, 10.0);
        assert!(
            stored(&s, methane) >= full - 1e-12,
            "full batteries: the cells idle"
        );
    }

    #[test]
    fn landing_legs_fold_out_below_the_engine_widen_the_stance_and_take_a_hard_landing() {
        let (c, defs, res) = lander();
        let mut s = Structure::build(&c, &defs, &res);
        let (clear_in, foot_in, tol_in) = s.ground_profile();
        assert_eq!(tol_in, 10.0, "stowed, it stands on its engine");
        assert_eq!(s.deploy_gear(true), 3);
        assert_eq!(s.deploy_gear(true), 0, "already out");
        let (clear_out, foot_out, tol_out) = s.ground_profile();
        assert!(
            clear_out > clear_in + 0.3,
            "legs reach below the bell: {clear_in} -> {clear_out}"
        );
        assert!(
            foot_out > foot_in + 0.5,
            "a wider stance: {foot_in} -> {foot_out}"
        );
        assert_eq!(tol_out, 20.0, "it lands on the legs");
        // 15 m/s: the legs take it; folded in, the engine breaks
        let (broken, _) = s.impact(15.0, DVec3::new(1.0, 0.0, 0.0)).unwrap();
        assert!(broken.is_empty());
        s.deploy_gear(false);
        let (broken, _) = s.impact(15.0, DVec3::new(1.0, 0.0, 0.0)).unwrap();
        assert_eq!(broken, vec![3], "the engine breaks");
        // with the engine gone the vessel stands on its tank and legs
        assert!(
            s.impact(200.0, DVec3::new(1.0, 0.0, 0.0)).is_none(),
            "far too fast"
        );
    }

    #[test]
    fn re_entry_burns_a_bare_pod_and_a_heat_shield_ablates_to_save_it() {
        let facing = DVec3::new(1.0, 0.0, 0.0);
        // falling bottom first at Mach 7 through thin upper air
        let v_air = DVec3::new(-2_300.0, 0.0, 0.0);
        let (rho, mach, ambient, k) = (0.01, 7.0, 220.0, 0.026);
        let settings = thermal::HeatSettings::default();
        let fly = |s: &mut Structure, seconds: f64| -> Option<f64> {
            let steps = (seconds / 0.02) as usize;
            for n in 0..steps {
                let burned = s.update_heat(facing, v_air, rho, mach, ambient, k, &settings, 0.02);
                if burned.contains(&s.root) {
                    return Some(n as f64 * 0.02);
                }
            }
            None
        };
        let (bare, defs, res) = test_capsule(false);
        let mut s = Structure::build(&bare, &defs, &res);
        let burned_at = fly(&mut s, 60.0).expect("the bare pod burns up");
        assert!(
            burned_at > 2.0,
            "heats over a few seconds, not at once: {burned_at} s"
        );
        // the parachute on top sits in the pod's shadow
        assert_eq!(s.reentry_flux_kw[1], 0.0);

        let (shielded, defs, res) = test_capsule(true);
        let mut s = Structure::build(&shielded, &defs, &res);
        assert_eq!(fly(&mut s, 60.0), None, "the shield saves it");
        let (pod, shield) = (0, 1);
        assert!(s.ablating[shield], "{:?} K", s.temperature_k);
        let t = s.temperature_k[shield];
        assert!(
            (1_000.0..1_100.0).contains(&t),
            "the shield holds near its ablation point: {t} K"
        );
        assert!(
            s.temperature_k[pod] < 400.0,
            "the pod stays cool: {} K",
            s.temperature_k[pod]
        );
        let left = s.resource_fraction(shield, "Ablator");
        assert!(left < 1.0 && left > 0.0, "ablator used: {left}");
        // flying the other way round the shield is in the pod's shadow and the pod burns
        let mut s = Structure::build(&shielded, &defs, &res);
        let nose_first = -v_air;
        for _ in 0..3_000 {
            s.update_heat(facing, nose_first, rho, mach, ambient, k, &settings, 0.02);
        }
        assert!(
            s.temperature_k[pod] > 400.0 && !s.ablating[shield],
            "a capsule flying nose first is not protected: pod at {} K",
            s.temperature_k[pod]
        );
        // subsonic air only cools
        let mut s = Structure::build(&bare, &defs, &res);
        s.temperature_k[0] = 600.0;
        s.update_heat(facing, v_air * 0.1, 1.0, 0.7, 288.0, k, &settings, 1.0);
        assert!(s.temperature_k[0] < 600.0 && s.reentry_flux_kw[0] == 0.0);
    }

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
    fn a_gust_bends_the_boosters_and_they_ring_down() {
        let (crafts, defs, res) = assets();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(k1, &defs, &res);
        let p = flying(DVec3::new(1.0, 0.0, 0.0));
        // a sudden gust across the stack (engine thrust ramps in, so it would not ring the joints)
        let out = StepOutcome {
            drag_kn: DVec3::new(0.0, 0.0, -2_000.0),
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
        assert!(settled > 1e-4, "settled bend {settled}");
        assert!(peak >= settled);
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
    fn folded_panels_make_nothing_until_deployed() {
        let (crafts, defs, res) = assets();
        // the K2 carries folding 1x6 panels
        let k2 = crafts.iter().find(|c| c.name == "Kerbal K2").unwrap();
        let mut s = Structure::build(k2, &defs, &res);
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        let ec = s.net.resource("ElectricCharge").unwrap();
        for t in s.net.tanks.iter_mut().filter(|t| t.resource == ec) {
            t.stored *= 0.5;
        }
        let before = ec_stored(&s);
        s.electric(&mut p, 1.0, 0.0, 10.0);
        let folded = ec_stored(&s) - before;
        assert!(s.deploy_panels(true) > 0);
        s.electric(&mut p, 1.0, 0.0, 10.0);
        let open = ec_stored(&s) - before - folded;
        assert!(open > folded + 1.0, "folded {folded} open {open}");
        assert!(s.deploy_panels(true) == 0, "already out");
    }

    #[test]
    fn a_parachute_opens_in_thick_air_and_drags_hard_when_fully_open() {
        let (crafts, defs, res) = assets();
        let c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap();
        let mut s = Structure::build(c, &defs, &res);
        let chute = (0..c.parts.len())
            .find(|&i| s.chute_defs[i].is_some())
            .unwrap();
        // falling nose up at 100 m/s
        let fall = DVec3::new(-100.0, 0.0, 0.0);
        let facing = DVec3::new(1.0, 0.0, 0.0);
        let packed = s.aero(0, facing, fall, 1.0, 0.3);
        // its stage arms it; thin air keeps it packed
        let last = s.stage_activations.len() - 1;
        s.activate_stage(last);
        assert_eq!(s.chutes[chute], Chute::Armed);
        s.update_chutes(0.01, 5_000.0, 0.3, false, 100.0, 1.0);
        assert_eq!(s.chutes[chute], Chute::Armed);
        // thick air: opens partly, then fully below 1 km
        for _ in 0..20 {
            s.update_chutes(0.8, 3_000.0, 0.3, false, 100.0, 0.5);
        }
        assert_eq!(s.chutes[chute], Chute::SemiDeployed);
        let semi = s.aero(0, facing, fall, 1.0, 0.3);
        for _ in 0..40 {
            s.update_chutes(0.9, 800.0, 0.3, false, 100.0, 0.5);
        }
        assert_eq!(s.chutes[chute], Chute::Deployed);
        let open = s.aero(0, facing, fall, 1.0, 0.3);
        assert!(
            semi > packed && open > 5.0 * semi,
            "packed {packed} semi {semi} open {open}"
        );
        // landed and still: cut away
        s.update_chutes(1.0, 0.0, 0.0, true, 0.1, 0.1);
        assert_eq!(s.chutes[chute], Chute::Cut);
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
            drag_kn: DVec3::new(0.0, 0.0, -30_000.0),
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

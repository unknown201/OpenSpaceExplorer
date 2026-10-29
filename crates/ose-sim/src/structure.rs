//! A powered vessel's structure: its parts as rigid bodies (see [`crate::rigid`]) loaded every step with the
//! thrust and drag the point-mass flight model delivers, so joints carry the original's loads and can break.
//!
//! Each engine's fuel delivery and part orientation set its thrust and structural load target.
//! Resolved aerodynamic, actuator, water, suspension and impact loads retain their part/application point.
//! The frontal-area load split remains only as a fallback. Uniform gravity does not load joints in free fall.
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineDemand {
    pub part: usize,
    pub thrust_kn: f64,
    pub mass_flow_tps: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AeroForces {
    pub cda_m2: f64,
    /// Sum of drag-cube forces in inertial kN; need not oppose centre-of-mass travel.
    pub drag_kn: DVec3,
    /// Lifting-surface force, including its induced drag, inertial kN.
    pub lift_kn: DVec3,
    pub torque_knm: DVec3,
}

/// Atmospheric-relative motion at the mass centre. Angular velocity includes pitch, yaw and roll,
/// minus the atmosphere's angular velocity, all in inertial axes.
#[derive(Clone, Copy, Debug)]
pub struct Airflow {
    pub frame: (DVec3, DVec3, DVec3),
    pub velocity: DVec3,
    pub angular_velocity: DVec3,
    pub density: f64,
    pub speed_of_sound: f64,
}

impl Airflow {
    pub fn for_vessel(
        p: &Powered,
        atmosphere_spin: DVec3,
        density: f64,
        speed_of_sound: f64,
    ) -> Self {
        Self {
            frame: crate::flight::vessel_basis(p),
            velocity: p.v - atmosphere_spin.cross(p.r),
            // The parked/rolling path carries the supported hull with the body without
            // storing that inertial spin in p.angvel. It has no relative tumble.
            angular_velocity: if p.landed {
                DVec3::ZERO
            } else {
                p.angvel + p.facing * p.roll_rate - atmosphere_spin
            },
            density,
            speed_of_sound,
        }
    }
    pub fn bulk(frame: (DVec3, DVec3, DVec3), velocity: DVec3, density: f64, mach: f64) -> Self {
        Self {
            frame,
            velocity,
            angular_velocity: DVec3::ZERO,
            density,
            speed_of_sound: if mach > 0.0 {
                velocity.length() / mach
            } else {
                f64::INFINITY
            },
        }
    }
    fn velocity_at(&self, offset: DVec3) -> DVec3 {
        let (x, y, z) = self.frame;
        self.velocity
            + self
                .angular_velocity
                .cross(x * offset.x + y * offset.y + z * offset.z)
    }

    fn mach(&self, speed: f64) -> f64 {
        if self.speed_of_sound > 0.0 {
            speed / self.speed_of_sound
        } else {
            0.0
        }
    }
}

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
    wheel_axes_knm: Vec<DVec3>,
    gimbal_range_rad: Vec<f64>,
    resolved_actuators: Option<Vec<crate::actuators::Actuator>>,
    /// Last delivered gimbal direction in each engine's exported model frame.
    pub engine_gimbals: Vec<(u16, DVec3)>,
    wheel_actuation_knm: Vec<DVec3>,
    geometry: Vec<ose_data::flight_geometry::FlightGeometry>,
    controls: Vec<ose_data::flight_geometry::PartControl>,
    control_sources: Vec<bool>,
    gimbal_angles: Vec<Vec<[f64; 2]>>,
    surface_angles: Vec<f64>,
    resolved_nozzle_forces: Vec<PartForce>,
    pub gimbal_poses: Vec<(u16, DVec3, [f64; 4])>,
    pub rcs_nozzles: Vec<(u16, DVec3, f32)>,
    last_rcs_pressure_atm: f64,
    /// Inertial force with a craft-frame application offset; consumed once by `apply`.
    pub(crate) surface_loads: Vec<PartForce>,
    pub suspension_compression_m: Vec<f64>,
    engine_isp: Vec<Option<ose_data::Curve>>,
    engine_throttle_locked: Vec<bool>,
    /// Each part's drag faces after occlusion by stack neighbours (`None` without a drag cube), and its
    /// authored rotation (Unity quaternion x, y, z, w) to bring the airflow into part space.
    pub faces: Vec<Option<Faces>>,
    pub part_rot: Vec<[f64; 4]>,
    /// Each part's `Cd * A` at the last `aero` call, m^2.
    pub part_cda_m2: Vec<f64>,
    curves: DragCurves,
    last_air_speed_mps: f64,
    last_airflow: Option<Airflow>,
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
    /// Delivered forces for this step, retained even when its final fuel was just consumed.
    resolved_engine_thrust: Option<Vec<f64>>,
    /// Per-part airflow force in inertial axes, with a craft-frame application offset.
    resolved_aero_forces: Option<Vec<PartForce>>,
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
    /// Fairings: for each fairing base, the parts it covers (everything stacked on its top node) and
    /// whether it was jettisoned; cargo bays and whether they are open; and which parts the airflow cannot
    /// reach (no drag, no re-entry heat).
    pub fairing_payload: Vec<Option<Vec<usize>>>,
    pub fairing_deployed: Vec<bool>,
    pub cargo_bay: Vec<bool>,
    pub bay_open: Vec<bool>,
    pub shielded: Vec<bool>,
    /// RCS thrusters on the parts.
    pub rcs_defs: Vec<Option<ose_data::Rcs>>,
    /// Lifting surfaces (their curves resolved), intake areas, and each engine's jet curves.
    lift_defs: Vec<Option<(ose_data::LiftingSurface, aero::LiftCurves)>>,
    pub intake_area_m2: Vec<f64>,
    /// Control surfaces: deflection range (rad) and moving share of the surface.
    ctrl: Vec<(f64, f64)>,
    engine_jet: Vec<Option<crate::flight::Jet>>,
    /// Built nose-forward (a plane): craft +Z is the belly and -Z is up, in right-handed vessel axes.
    pub level: bool,
    /// Flight stage the structure was last trimmed to.
    staged_to: usize,
}

pub use ose_data::craft::level_layout;

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

/// Drag coefficients of a fairing shell end on (a nose cone) and side on (a cylinder): our own figures.
const FAIRING_CD_FRONT: f64 = 0.3;
const FAIRING_CD_SIDE: f64 = 0.6;

/// How hard a leg's or wheel's suspension stops the craft over its travel, m/s^2.
const SUSPENSION_DECEL: f64 = 30.0;

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
            wheel_axes_knm: (0..n)
                .map(|i| {
                    def_of(i)
                        .map(|d| {
                            d.reaction_wheel_axes_knm.unwrap_or(DVec3::new(
                                d.reaction_wheel_torque_knm,
                                d.reaction_wheel_torque_knm,
                                d.reaction_wheel_torque_knm,
                            ))
                        })
                        .unwrap_or(DVec3::ZERO)
                })
                .collect(),
            gimbal_range_rad: (0..n)
                .map(|i| {
                    def_of(i)
                        .and_then(|d| d.engines.first())
                        .and_then(|e| e.gimbal_range_deg)
                        .unwrap_or(0.0)
                        .clamp(0.0, 90.0)
                        .to_radians()
                })
                .collect(),
            controls: vec![Default::default(); n],
            control_sources: (0..n)
                .map(|i| {
                    def_of(i).is_some_and(|d| {
                        d.module_names.iter().any(|m| {
                            m == "PartComponentModule_Command"
                                || m == "PartComponentModule_DockingNode"
                        })
                    })
                })
                .collect(),
            resolved_actuators: None,
            engine_gimbals: Vec::new(),
            wheel_actuation_knm: vec![DVec3::ZERO; n],
            geometry: (0..n)
                .map(|i| {
                    def_of(i)
                        .map(|d| d.flight_geometry.clone())
                        .unwrap_or_default()
                })
                .collect(),
            gimbal_angles: (0..n)
                .map(|i| {
                    vec![
                        [0.0; 2];
                        def_of(i)
                            .map(|d| d.flight_geometry.gimbals.len())
                            .unwrap_or(0)
                    ]
                })
                .collect(),
            surface_angles: vec![0.0; n],
            resolved_nozzle_forces: Vec::new(),
            gimbal_poses: Vec::new(),
            rcs_nozzles: Vec::new(),
            last_rcs_pressure_atm: 0.0,
            surface_loads: Vec::new(),
            suspension_compression_m: vec![0.0; n],
            engine_isp: (0..n)
                .map(|i| {
                    def_of(i)
                        .and_then(|d| d.engines.first())
                        .map(|e| e.isp_curve.clone())
                })
                .collect(),
            engine_throttle_locked: (0..n)
                .map(|i| {
                    def_of(i)
                        .and_then(|d| d.engines.first())
                        .is_some_and(|e| e.throttle_locked)
                })
                .collect(),
            faces,
            part_rot,
            part_cda_m2: vec![0.0; n],
            curves: DragCurves::default(),
            last_air_speed_mps: 0.0,
            last_airflow: None,
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
            ctrl: (0..n)
                .map(|i| {
                    def_of(i)
                        .map(|d| (d.ctrl_range_deg.to_radians(), d.ctrl_area))
                        .unwrap_or((0.0, 0.0))
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
            fairing_payload: (0..n)
                .map(|i| {
                    def_of(i).filter(|d| d.fairing).map(|_| {
                        // the subtree on the fairing's top node
                        let mut out = Vec::new();
                        let mut stack: Vec<usize> = (0..n)
                            .filter(|&j| {
                                craft.parts[j].parent == Some(i)
                                    && craft.parts[j].parent_node.as_deref() == Some("top")
                            })
                            .collect();
                        while let Some(j) = stack.pop() {
                            out.push(j);
                            stack.extend((0..n).filter(|&k| craft.parts[k].parent == Some(j)));
                        }
                        out
                    })
                })
                .collect(),
            fairing_deployed: vec![false; n],
            cargo_bay: (0..n)
                .map(|i| def_of(i).is_some_and(|d| d.cargo_bay))
                .collect(),
            bay_open: vec![false; n],
            shielded: vec![false; n],
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
            resolved_engine_thrust: None,
            resolved_aero_forces: None,
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

    /// Supplied engines producing thrust at this throttle setting.
    pub fn firing_engines(&self, stage: usize, throttle: f64) -> Vec<usize> {
        self.fed_engines(stage)
            .into_iter()
            .filter(|&i| self.engine_throttle_locked[i] || throttle > 0.0)
            .collect()
    }

    fn engine_duty(&self, engine: usize, throttle: f64) -> f64 {
        if self.engine_throttle_locked[engine] {
            1.0
        } else {
            throttle.clamp(0.0, 1.0)
        }
    }

    /// Requested thrust and mass flow per engine, using the full authored pressure curve.
    /// Jet flow modifiers affect only that engine, never the other engines in its stage.
    pub fn engine_demands(
        &self,
        stage: usize,
        throttle: f64,
        env: crate::flight::EngineEnvironment,
    ) -> Vec<EngineDemand> {
        let intakes =
            (0..self.rigid.len()).any(|i| self.rigid.present[i] && self.intake_area_m2[i] > 0.0);
        self.fed_engines(stage)
            .into_iter()
            .filter_map(|part| {
                let curve = self.engine_isp[part].as_ref()?;
                let vacuum_isp = curve.evaluate(0.0);
                if vacuum_isp <= 0.0 {
                    return None;
                }
                let air = self.engine_jet[part]
                    .as_ref()
                    .map(|jet| jet.factor(env.mach, env.density, env.oxygen, intakes))
                    .unwrap_or(1.0);
                let mass_flow_tps = self.engine_thrust_kn[part] / (vacuum_isp * ose_data::G0)
                    * self.engine_duty(part, throttle)
                    * self.controls[part].thrust_limiter
                    * air;
                (mass_flow_tps > 0.0).then(|| EngineDemand {
                    part,
                    mass_flow_tps,
                    thrust_kn: mass_flow_tps
                        * curve.evaluate(env.pressure_atm.max(0.0)).max(0.0)
                        * ose_data::G0,
                })
            })
            .collect()
    }

    pub fn can_control_from(&self, i: usize) -> bool {
        self.control_sources.get(i) == Some(&true) && self.rigid.present.get(i) == Some(&true)
    }
    pub fn control_rotation(&self, i: usize) -> [f64; 4] {
        let mount = glam::DQuat::from_array(self.part_rot[i]);
        let local = glam::DQuat::from_array(
            self.geometry[i]
                .control_rotation
                .unwrap_or([0., 0., 0., 1.]),
        );
        let q = (mount * local).normalize().to_array();
        [-q[0], -q[1], q[2], q[3]]
    }
    pub fn control_capabilities(&self) -> Vec<(u16, u8)> {
        (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .map(|i| {
                (
                    i as u16,
                    u8::from(self.wheel_axes_knm[i].length_squared() > 0.0)
                        | (u8::from(self.gimbal_range_rad[i] > 0.0) << 1)
                        | (u8::from(self.rcs_defs[i].is_some()) << 2)
                        | (u8::from(self.ctrl[i].0 > 0.0) << 3)
                        | (u8::from(self.engine_thrust_kn[i] > 0.0) << 4)
                        | (u8::from(self.can_control_from(i)) << 5),
                )
            })
            .collect()
    }

    fn part_axes(&self, i: usize) -> [DVec3; 3] {
        [
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(0.0, 0.0, -1.0),
        ]
        .map(|v| crate::rigid::to_vessel(aero::rotate(self.part_rot[i], v)).normalized())
    }

    fn part_point(&self, i: usize, point: DVec3) -> DVec3 {
        self.rigid.rest_position(i) + crate::rigid::to_vessel(aero::rotate(self.part_rot[i], point))
    }

    fn engine_forces(&self, i: usize, thrust: f64) -> Vec<PartForce> {
        let Some(jets) = self.geometry[i].engines.first().filter(|j| !j.is_empty()) else {
            return vec![PartForce {
                part: i,
                force_kn: self.part_axes(i)[1] * thrust,
                offset_m: DVec3::ZERO,
            }];
        };
        jets.iter()
            .map(|jet| {
                let mut position = jet.position_m;
                let mut direction = jet.direction;
                if let Some(g) = jet
                    .gimbal
                    .and_then(|g| self.geometry[i].gimbals.get(g).map(|d| (g, d)))
                {
                    let angles = self.gimbal_angles[i][g.0];
                    let frame = glam::DQuat::from_array(g.1.rotation);
                    let turn = frame
                        * glam::DQuat::from_rotation_x(angles[0] * g.1.axis_mult[0])
                        * glam::DQuat::from_rotation_y(angles[1] * g.1.axis_mult[1])
                        * frame.inverse();
                    let v = position - g.1.position_m;
                    let p = turn * glam::DVec3::new(v.x, v.y, v.z);
                    position = g.1.position_m + DVec3::new(p.x, p.y, p.z);
                    let d = turn * glam::DVec3::new(direction.x, direction.y, direction.z);
                    direction = DVec3::new(d.x, d.y, d.z);
                }
                PartForce {
                    part: i,
                    offset_m: self.part_point(i, position) - self.rigid.rest_position(i),
                    force_kn: crate::rigid::to_vessel(aero::rotate(self.part_rot[i], direction))
                        * thrust
                        * jet.weight,
                }
            })
            .collect()
    }

    /// KSP PartComponent.CenterOfMassOffset applies to the part's total (dry + resource) mass.
    pub fn mass_properties(&self) -> crate::dynamics::MassProperties {
        let masses = self.part_masses();
        let mut out = crate::dynamics::MassProperties::default();
        for (i, &mass) in masses.iter().enumerate() {
            if self.rigid.present[i] && self.net.present[i] {
                out.mass_t += mass;
                out.centre_m =
                    out.centre_m + self.part_point(i, self.geometry[i].mass_offset_m) * mass;
            }
        }
        if out.mass_t > 0.0 {
            out.centre_m = out.centre_m * (1.0 / out.mass_t);
        }
        for (i, &mass) in masses.iter().enumerate() {
            if self.rigid.present[i] && self.net.present[i] {
                if let Some(tensor) = self.geometry[i].inertia_per_t {
                    let axes = self.part_axes(i);
                    out.inertia.add_tensor(
                        mass,
                        tensor,
                        [axes[0], axes[1], -axes[2]],
                        self.part_point(i, self.geometry[i].mass_offset_m) - out.centre_m,
                    );
                    continue;
                }
                out.inertia.add_box(
                    mass,
                    self.rigid.sizes_m[i],
                    self.part_axes(i),
                    self.part_point(i, self.geometry[i].mass_offset_m) - out.centre_m,
                );
            }
        }
        out
    }

    fn part_masses(&self) -> Vec<f64> {
        let mut masses = self.net.dry_t.clone();
        for tank in &self.net.tanks {
            masses[tank.part] += tank.stored * self.net.unit_mass_t[tank.resource];
        }
        masses
    }

    fn sync_structural_mass(&mut self) {
        self.rigid.set_part_masses(&self.part_masses());
    }

    /// Resolve fuel before integrating motion so a starved engine cannot provide a free step of thrust.
    pub fn propulsion(
        &mut self,
        p: &Powered,
        env: crate::flight::EngineEnvironment,
        dt: f64,
    ) -> crate::flight::Propulsion {
        self.resolved_nozzle_forces.clear();
        for i in 0..self.controls.len() {
            self.controls[i] = p.part_control(i);
        }
        let mut delivered = vec![0.0; self.rigid.len()];
        let mut out = crate::flight::Propulsion {
            mass: self.mass_properties(),
            ..Default::default()
        };
        if p.ignited && !p.crashed && dt > 0.0 {
            let demands = self.engine_demands(p.stage, p.throttle, env);
            let mut active = Vec::new();
            let mut requests = Vec::new();
            for demand in demands {
                let Some(mix) = &self.engine_mixture[demand.part] else {
                    continue;
                };
                let unit_mass = self.net.mixture_unit_mass_t(mix);
                if unit_mass <= 0.0 {
                    continue;
                }
                requests.push(Request {
                    part: demand.part,
                    instructions: self
                        .net
                        .ingredients(mix)
                        .into_iter()
                        .map(|(resource, units)| Instruction {
                            resource,
                            units_per_s: units * demand.mass_flow_tps / unit_mass,
                        })
                        .collect(),
                    priority: 0,
                });
                active.push(demand);
            }
            let before = self.net.mass_t();
            let fractions = self.net.process(&requests, dt);
            out.burned_t = (before - self.net.mass_t()).max(0.0);
            for (demand, fraction) in active.iter().zip(fractions) {
                let thrust = demand.thrust_kn * fraction;
                delivered[demand.part] = thrust;
                out.thrust_kn += thrust;
                for force in self.engine_forces(demand.part, thrust) {
                    out.force_kn = out.force_kn + force.force_kn;
                    out.torque_knm = out.torque_knm
                        + (self.rigid.rest_position(force.part) + force.offset_m
                            - out.mass.centre_m)
                            .cross(force.force_kn);
                    self.resolved_nozzle_forces.push(force);
                }
            }
        }
        self.resolved_engine_thrust = Some(delivered);
        out
    }

    /// Build bounded channels from attached, powered actuators and this step's delivered thrust.
    pub fn configure_actuators(
        &mut self,
        p: &Powered,
        env: crate::flight::EngineEnvironment,
        propulsion: &mut crate::flight::Propulsion,
    ) {
        self.last_rcs_pressure_atm = env.pressure_atm;
        self.rcs_nozzles.clear();
        use crate::actuators::Actuator;
        let mut channels = Vec::new();
        let centre = propulsion.mass.centre_m;
        for i in (0..self.rigid.len()).filter(|&i| self.rigid.present[i]) {
            let axes = self.part_axes(i);
            let cfg = p.part_control(i);
            let caps = self.wheel_axes_knm[i]
                * p.wheel_scale.clamp(0.0, 1.0)
                * cfg.wheel_authority
                * f64::from(cfg.wheel_enabled);
            for (j, cap) in [caps.x, caps.y, caps.z].into_iter().enumerate() {
                if cap > 0.0 {
                    channels.push(Actuator {
                        positive_only: false,
                        part: i,
                        // Module_ReactionWheel applies torque in the vessel control frame,
                        // regardless of the mounted part's orientation.
                        torque_knm: aero::rotate(
                            p.control_rotation,
                            [
                                DVec3::new(1., 0., 0.),
                                DVec3::new(0., 1., 0.),
                                DVec3::new(0., 0., 1.),
                            ][j],
                        ) * cap,
                        force_kn: DVec3::ZERO,
                        offset_m: DVec3::ZERO,
                        group: i * 8 + j,
                        axial: None,
                    });
                }
            }
            let r = self.rigid.rest_position(i) - centre;
            let thrust = self
                .resolved_engine_thrust
                .as_ref()
                .map(|t| t[i])
                .unwrap_or(0.0);
            let side = thrust
                * (self.gimbal_range_rad[i] * cfg.gimbal_limiter).sin()
                * f64::from(cfg.gimbal_enabled);
            for axis in [axes[0], axes[2]] {
                if side > 0.0 {
                    let force = axis * side;
                    channels.push(Actuator {
                        positive_only: false,
                        part: i,
                        torque_knm: r.cross(force),
                        force_kn: force,
                        offset_m: DVec3::ZERO,
                        group: i * 8 + 3,
                        axial: Some((axes[1] * thrust, r)),
                    });
                }
            }
            // Old catalogs without nozzle transforms retain the mounted force fallback.
            if p.groups.rcs && cfg.rcs_enabled {
                if let Some(rcs) = &self.rcs_defs[i] {
                    let isp_scale = if rcs.isp_curve.evaluate(0.0) > 0.0 {
                        rcs.isp_curve.evaluate(env.pressure_atm) / rcs.isp_curve.evaluate(0.0)
                    } else {
                        0.0
                    };
                    let ingredients = self.net.ingredients(&rcs.propellant);
                    let fed = !ingredients.is_empty()
                        && ingredients
                            .iter()
                            .all(|&(resource, _)| self.net.available(i, resource) > 1e-9);
                    if !self.geometry[i].rcs.is_empty() {
                        for (j, nozzle) in self.geometry[i].rcs.iter().enumerate() {
                            let force = crate::rigid::to_vessel(aero::rotate(
                                self.part_rot[i],
                                nozzle.direction,
                            )) * (rcs.thrust_kn
                                * isp_scale.max(0.0)
                                * f64::from(fed)
                                * nozzle.weight
                                * cfg.rcs_limiter);
                            let offset =
                                self.part_point(i, nozzle.position_m) - self.rigid.rest_position(i);
                            channels.push(Actuator {
                                positive_only: true,
                                part: i,
                                force_kn: force,
                                torque_knm: (r + offset).cross(force),
                                offset_m: offset,
                                group: self.rigid.len() * 8 + i * 32 + j,
                                axial: None,
                            });
                        }
                    } else {
                        for axis in axes {
                            let force = axis
                                * (rcs.thrust_kn
                                    * isp_scale.max(0.0)
                                    * f64::from(fed)
                                    * cfg.rcs_limiter);
                            channels.push(Actuator {
                                positive_only: false,
                                part: i,
                                torque_knm: r.cross(force),
                                force_kn: force,
                                offset_m: DVec3::ZERO,
                                group: i * 8 + 4,
                                axial: None,
                            });
                        }
                    }
                }
            }
            let (range, share) = self.ctrl[i];
            if let Some((l, curves)) = &self.lift_defs[i] {
                if range > 0.0 && share > 0.0 && !self.shielded[i] {
                    let (speed, mach) = self
                        .last_airflow
                        .map(|flow| {
                            let speed = flow.velocity_at(self.part_box(i).0 - centre).length();
                            (speed, flow.mach(speed))
                        })
                        .unwrap_or((self.last_air_speed_mps, env.mach));
                    let q = 0.5 * env.density * speed.powi(2);
                    let cap = (q
                        * aero::LIFT_MULTIPLIER
                        * l.deflection_lift_coeff
                        * share
                        * curves.lift.evaluate(range.sin())
                        * curves.lift_mach.evaluate(mach))
                    .max(0.0);
                    let axis = aero::rotate(self.part_rot[i], l.axis);
                    let force = DVec3::new(axis.x, axis.y, -axis.z) * cap;
                    let (at, _) = self.part_box(i);
                    channels.push(Actuator {
                        positive_only: false,
                        part: i,
                        torque_knm: (at - centre).cross(force),
                        force_kn: force,
                        offset_m: at - self.rigid.rest_position(i),
                        group: i * 8 + 5,
                        axial: None,
                    });
                }
            }
        }
        if !p.controlled {
            channels.clear();
        }
        self.resolved_actuators = Some(channels.clone());
        propulsion.actuators = Some(channels);
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

    /// Every resource aboard: (resource id, units stored, units of capacity), in the flow graph's order.
    pub fn resource_totals(&self) -> Vec<(String, f64, f64)> {
        self.totals(|t| self.net.present[self.net.tanks[t].part])
    }

    /// The same for the tanks the burning stage's engines draw from.
    pub fn stage_resource_totals(&self, stage: usize) -> Vec<(String, f64, f64)> {
        let mut reach = vec![false; self.net.tanks.len()];
        for e in self.live_engines(stage) {
            let Some(mix) = &self.engine_mixture[e] else {
                continue;
            };
            for (r, _) in self.net.ingredients(mix) {
                for t in self.net.groups(e, r).into_iter().flatten() {
                    reach[t] = true;
                }
            }
        }
        self.totals(|t| reach[t])
    }

    fn totals(&self, take: impl Fn(usize) -> bool) -> Vec<(String, f64, f64)> {
        let mut out: Vec<(String, f64, f64)> = Vec::new();
        for (t, tank) in self.net.tanks.iter().enumerate() {
            if !take(t) || tank.capacity <= 0.0 {
                continue;
            }
            let id = &self.net.resources[tank.resource];
            match out.iter_mut().find(|(n, _, _)| n == id) {
                Some(e) => {
                    e.1 += tank.stored;
                    e.2 += tank.capacity;
                }
                None => out.push((id.clone(), tank.stored, tank.capacity)),
            }
        }
        out
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

    /// The wheels out under the craft, its drive direction and wheelbase, into the flight state's `drive`.
    fn sync_drive(&self, p: &mut Powered) {
        // Old saves can retain an empty craft after its last part was detached/recovered.
        // Such a vessel has no root transform or wheel drive.
        let Some(root_rotation) = self.part_rot.get(self.root).copied() else {
            p.drive = Default::default();
            return;
        };
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
        let up = aero::rotate(root_rotation, DVec3::new(0.0, 1.0, 0.0));
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
        // Up in the right-handed vessel frame: +Y standing, -Z lying level.
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

    /// Up on the ground in vessel axes: +Y standing, -Z with the craft lying level.
    fn up_axis(&self) -> DVec3 {
        if self.level {
            DVec3::new(0.0, 0.0, -1.0)
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
        if p.control_point
            .is_some_and(|i| !self.can_control_from(i as usize))
        {
            p.control_point = None;
            p.control_rotation = [0.0, 0.0, 0.0, 1.0];
            p.sas.reset();
        }
        self.sync_structural_mass();
        p.mass_t = self.net.mass_t().max(0.001);
        self.update_shielding();
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
        let fed = self.fed_engines(p.stage);
        let supplied: f64 = fed.iter().map(|&i| self.engine_thrust_kn[i]).sum();
        let locked: f64 = fed
            .iter()
            .filter(|&&i| self.engine_throttle_locked[i])
            .map(|&i| self.engine_thrust_kn[i])
            .sum();
        p.locked_thrust_fraction = if supplied > 0.0 {
            locked / supplied
        } else {
            0.0
        };
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
    /// shadow; fixed panels are taken at half their rating), generators, RTGs and converters fill the
    /// batteries. Probe cores and drive motors draw before wheel actuation is planned. Updates control
    /// eligibility and drive power for this tick; reaction-wheel requests are resolved separately.
    pub fn electric_services(&mut self, p: &mut Powered, sun: f64, dt: f64) {
        let Some(ec) = self.net.resource("ElectricCharge") else {
            return;
        };
        let live = self.live_engines(p.stage);
        let mut requests = Vec::new();
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
                    if p.ignited && !p.crashed && live.contains(&i) && self.fed(i) {
                        made += e.generator_output * self.engine_duty(i, p.throttle);
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
        let crewed = (0..self.rigid.len()).any(|i| self.rigid.present[i] && self.crew[i] > 0);
        p.controlled = crewed || cores.is_empty() || cores.iter().any(|&r| fractions[r] > 0.0);
        if !p.controlled {
            p.throttle = 0.0;
        }
    }

    /// Legacy scalar-demand entry point for coast/debris services and callers without part control plans.
    pub fn electric(&mut self, p: &mut Powered, sun: f64, wheel_input: f64, dt: f64) {
        self.electric_services(p, sun, dt);
        let torques: Vec<_> = self
            .electric
            .iter()
            .map(|e| e.wheel_torque_vec_knm * wheel_input.clamp(0.0, 1.0))
            .collect();
        self.consume_wheel_charge(p, &torques, dt);
    }

    /// Consume charge once at the actual wheel mounts, before applying their torque.
    fn consume_wheel_charge(&mut self, p: &mut Powered, torques: &[f64], dt: f64) -> Vec<f64> {
        let ec = self.net.resource("ElectricCharge");
        let mut requests = Vec::new();
        let mut parts = Vec::new();
        let mut delivered = vec![0.0; self.rigid.len()];
        for (i, &torque) in torques.iter().enumerate() {
            if !self.rigid.present[i] {
                continue;
            }
            let cost = self.electric[i].wheel_use_per_knm;
            if cost <= 0.0 {
                delivered[i] = 1.0;
            } else if let Some(ec) = ec {
                if torque > 0.0 && p.controlled {
                    parts.push(i);
                    requests.push(Request {
                        part: i,
                        instructions: vec![Instruction {
                            resource: ec,
                            units_per_s: cost * torque,
                        }],
                        priority: 4,
                    });
                } else if self.net.available(i, ec) > 1e-9 {
                    delivered[i] = 1.0;
                }
            }
        }
        for (i, fraction) in parts.into_iter().zip(self.net.process(&requests, dt)) {
            delivered[i] = fraction;
        }
        let mut total = 0.0;
        let mut powered = 0.0;
        for (i, axes) in self.wheel_axes_knm.iter().enumerate() {
            if self.rigid.present[i] {
                total += axes.length();
                powered += axes.length() * delivered[i];
            }
        }
        p.wheel_scale = if total > 0.0 { powered / total } else { 1.0 };
        delivered
    }

    /// Freeze combined actuator inputs to this tick's delivered charge and RCS propellant.
    pub fn resolve_actuator_resources(
        &mut self,
        p: &mut Powered,
        body: &crate::bodies::Body,
        propulsion: &mut crate::flight::Propulsion,
        dt: f64,
    ) {
        // A dry wheel must not win the allocation ahead of a usable gimbal or surface.
        let ec = self.net.resource("ElectricCharge");
        for a in propulsion.actuators.as_mut().expect("configured actuators") {
            if a.force_kn.length_squared() == 0.0
                && a.axial.is_none()
                && self.electric[a.part].wheel_use_per_knm > 0.0
                && !ec.is_some_and(|ec| self.net.available(a.part, ec) > 1e-9)
            {
                a.torque_knm = DVec3::ZERO;
            }
        }
        let mut allocation = crate::flight::plan_actuators(p, body, propulsion, dt);
        self.resolve_authored_gimbals(p, propulsion, &mut allocation, dt);
        self.resolve_surface_geometry(p, propulsion, &mut allocation, dt);
        let channels = propulsion.actuators.as_ref().expect("configured actuators");
        // Reference wheels receive the same axis commands. They do not compete for a
        // least-squares allocation or let the first available wheel consume the request.
        for i in 0..self.rigid.len() {
            let cfg = p.part_control(i);
            let cap = self.wheel_axes_knm[i] * cfg.wheel_authority * f64::from(cfg.wheel_enabled);
            let manual = DVec3::new(
                p.rotation_input.x,
                p.rotation_input.z + p.roll_input,
                -p.rotation_input.y,
            );
            let command = aero::rotate_inverse(p.control_rotation, p.control_axes);
            let command = match cfg.wheel_mode {
                1 => manual,
                2 => {
                    if p.attitude == crate::flight::Attitude::Free {
                        DVec3::ZERO
                    } else {
                        command - manual
                    }
                }
                _ => command,
            };
            let desired = if self.rigid.present[i] && p.controlled {
                aero::rotate(
                    p.control_rotation,
                    DVec3::new(cap.x * command.x, cap.y * command.y, cap.z * command.z),
                )
            } else {
                DVec3::ZERO
            };
            self.wheel_actuation_knm[i] =
                crate::actuators::wheel_response(self.wheel_actuation_knm[i], desired, dt);
        }
        for (a, input) in channels.iter().zip(&mut allocation.inputs) {
            if a.force_kn.length_squared() == 0.0 && a.axial.is_none() {
                let old = *input;
                let norm = a.torque_knm.length_squared();
                *input = if norm > 0.0 {
                    self.wheel_actuation_knm[a.part].dot(a.torque_knm) / norm
                } else {
                    0.0
                };
                allocation.torque_knm = allocation.torque_knm + a.torque_knm * (*input - old);
            }
        }
        let mut magnitudes = vec![0.0; self.rigid.len()];
        for (a, input) in channels.iter().zip(&allocation.inputs) {
            if a.force_kn.length_squared() == 0.0 && a.axial.is_none() {
                let t = a.torque_knm * *input;
                magnitudes[a.part] += t.x.abs() + t.y.abs() + t.z.abs();
            }
        }
        let delivered = self.consume_wheel_charge(p, &magnitudes, dt);
        for (a, input) in channels.iter().zip(&mut allocation.inputs) {
            if a.force_kn.length_squared() == 0.0 && a.axial.is_none() {
                let old = *input;
                *input *= delivered[a.part];
                allocation.torque_knm = allocation.torque_knm + a.torque_knm * (*input - old);
            }
        }
        self.resolve_rcs(p, propulsion, &mut allocation, dt);
        propulsion.control_allocation = Some(allocation);
    }

    fn resolve_authored_gimbals(
        &mut self,
        p: &Powered,
        propulsion: &crate::flight::Propulsion,
        allocation: &mut crate::actuators::Allocation,
        dt: f64,
    ) {
        self.gimbal_poses.clear();
        let channels = propulsion.actuators.as_ref().unwrap();
        for i in 0..self.geometry.len() {
            if !self.rigid.present[i]
                || self.geometry[i]
                    .engines
                    .first()
                    .is_none_or(|j| j.is_empty())
            {
                continue;
            }
            for (a, input) in channels.iter().zip(&mut allocation.inputs) {
                if a.part == i && a.axial.is_some() {
                    allocation.force_kn = allocation.force_kn - a.force_kn * *input;
                    allocation.torque_knm = allocation.torque_knm - a.torque_knm * *input;
                    *input = 0.0;
                }
            }
            allocation.axial_corrections.retain(|f| {
                if f.part == i {
                    allocation.force_kn = allocation.force_kn - f.force_kn;
                    allocation.torque_knm = allocation.torque_knm
                        - (self.rigid.rest_position(i) + f.offset_m - propulsion.mass.centre_m)
                            .cross(f.force_kn);
                    false
                } else {
                    true
                }
            });
            for g in 0..self.geometry[i].gimbals.len() {
                let definition = &self.geometry[i].gimbals[g];
                let cfg = p.part_control(i);
                let at = self.part_point(i, definition.position_m);
                let lever = aero::rotate_inverse(p.control_rotation, at - propulsion.mass.centre_m);
                let command = aero::rotate_inverse(p.control_rotation, p.control_axes);
                let mut input = glam::DVec3::new(
                    if cfg.pitch { command.x } else { 0. },
                    if cfg.roll { command.y } else { 0. },
                    if cfg.yaw { -command.z } else { 0. },
                );
                if !cfg.gimbal_enabled {
                    input = glam::DVec3::ZERO;
                }
                let sign = if lever.y > 0.0 { -1.0 } else { 1.0 };
                input.x *= sign;
                input.z *= sign;
                if lever.x.abs() > 0.1 {
                    input.x += input.y * lever.x.signum();
                }
                if lever.z.abs() > 0.1 {
                    input.z -= input.y * lever.z.signum();
                }
                let body = aero::rotate(p.control_rotation, DVec3::new(input.x, input.y, -input.z));
                input = glam::DVec3::new(body.x, body.y, -body.z);
                let mount = glam::DQuat::from_array(self.part_rot[i]);
                let frame = glam::DQuat::from_array(definition.rotation);
                let local = (mount * frame).inverse() * input;
                let old = self.gimbal_angles[i][g];
                let target = [
                    local.x.clamp(-1.0, 1.0)
                        * definition.limits_rad[if local.x > 0.0 { 0 } else { 1 }]
                        * cfg.gimbal_limiter,
                    local.y.clamp(-1.0, 1.0)
                        * definition.limits_rad[if local.y > 0.0 { 2 } else { 3 }]
                        * cfg.gimbal_limiter,
                ];
                let t = definition
                    .response_speed
                    .map(|s| (s * dt).clamp(0.0, 1.0))
                    .unwrap_or(1.0);
                let next = [
                    old[0] + (target[0] - old[0]) * t,
                    old[1] + (target[1] - old[1]) * t,
                ];
                self.gimbal_angles[i][g] = next;
                let turn = frame
                    * glam::DQuat::from_rotation_x(next[0] * definition.axis_mult[0])
                    * glam::DQuat::from_rotation_y(next[1] * definition.axis_mult[1])
                    * frame.inverse();
                let q = turn.to_array();
                self.gimbal_poses.push((
                    i as u16,
                    DVec3::new(
                        -definition.position_m.x,
                        definition.position_m.y,
                        definition.position_m.z,
                    ),
                    [q[0], -q[1], -q[2], q[3]],
                ));
            }
            let thrust = self
                .resolved_engine_thrust
                .as_ref()
                .map(|t| t[i])
                .unwrap_or(0.0);
            let next = self.engine_forces(i, thrust);
            let old: Vec<_> = self
                .resolved_nozzle_forces
                .iter()
                .filter(|f| f.part == i)
                .copied()
                .collect();
            for (forces, sign) in [(&next, 1.0), (&old, -1.0)] {
                for f in forces {
                    allocation.force_kn = allocation.force_kn + f.force_kn * sign;
                    allocation.torque_knm = allocation.torque_knm
                        + (self.rigid.rest_position(i) + f.offset_m - propulsion.mass.centre_m)
                            .cross(f.force_kn)
                            * sign;
                }
            }
            self.resolved_nozzle_forces.retain(|f| f.part != i);
            self.resolved_nozzle_forces.extend(next);
        }
    }

    fn lift_point(&self, i: usize) -> DVec3 {
        if self.geometry[i].inertia_source.is_empty() {
            self.part_box(i).0
        } else {
            self.part_point(i, self.geometry[i].lift_offset_m)
        }
    }

    fn resolve_surface_geometry(
        &mut self,
        p: &Powered,
        propulsion: &crate::flight::Propulsion,
        allocation: &mut crate::actuators::Allocation,
        dt: f64,
    ) {
        let Some(flow) = self.last_airflow else {
            return;
        };
        let (x, y, z) = flow.frame;
        let centre = propulsion.mass.centre_m;
        let command = if p.controlled {
            p.control_axes
        } else {
            DVec3::ZERO
        };
        for i in 0..self.geometry.len() {
            let Some(surface) = self.geometry[i].surface.clone() else {
                continue;
            };
            if !self.rigid.present[i] || self.shielded[i] {
                continue;
            }
            let point = self.lift_point(i);
            let input = if surface.ignore_sas {
                aero::rotate(
                    p.control_rotation,
                    DVec3::new(
                        p.rotation_input.x,
                        p.roll_input + p.rotation_input.z,
                        -p.rotation_input.y,
                    ),
                )
            } else {
                command
            };
            let cfg = p.part_control(i);
            let input = aero::rotate_inverse(p.control_rotation, input);
            let input = DVec3::new(
                if cfg.surface_axes[0] { input.x } else { 0. },
                if cfg.surface_axes[1] { input.y } else { 0. },
                if cfg.surface_axes[2] { input.z } else { 0. },
            );
            let input =
                aero::rotate(p.control_rotation, input) * if cfg.surface_invert { -1. } else { 1. };
            let velocity = flow.velocity_at(point - centre);
            let speed = velocity.length();
            let travel = if speed > 0. {
                velocity * (1. / speed)
            } else {
                DVec3::ZERO
            };
            let q = 0.5 * flow.density * speed * speed;
            // Determine the mounted hinge's signed torque direction from a small physical
            // deflection. This includes mirrored surfaces and reversed airflow.
            let saved = self.surface_angles[i];
            self.surface_angles[i] = 0.001;
            let plus = self.lifting_force(i, flow.frame, travel, q, flow.mach(speed));
            self.surface_angles[i] = -0.001;
            let minus = self.lifting_force(i, flow.frame, travel, q, flow.mach(speed));
            self.surface_angles[i] = saved;
            let delta = plus - minus;
            let derivative =
                (point - centre).cross(DVec3::new(delta.dot(x), delta.dot(y), delta.dot(z)));
            let target = if derivative.length() > 1e-10 {
                input.dot(derivative.normalized()).clamp(-1., 1.)
                    * self.ctrl[i].0
                    * p.part_control(i).surface_authority
            } else {
                0.
            };
            let old = self.lifting_force(i, flow.frame, travel, q, flow.mach(speed));
            let previous = self.surface_angles[i];
            self.surface_angles[i] = if surface.exponential {
                previous + (target - previous) * (surface.speed * dt).clamp(0., 1.)
            } else {
                previous
                    + (target - previous).clamp(
                        -surface.speed.to_radians() * dt,
                        surface.speed.to_radians() * dt,
                    )
            };
            let new = self.lifting_force(i, flow.frame, travel, q, flow.mach(speed));
            let delta = new - old;
            let local = DVec3::new(delta.dot(x), delta.dot(y), delta.dot(z));
            for (a, u) in propulsion
                .actuators
                .as_ref()
                .unwrap()
                .iter()
                .zip(&mut allocation.inputs)
            {
                if a.part == i && a.group == i * 8 + 5 {
                    allocation.force_kn = allocation.force_kn - a.force_kn * *u;
                    allocation.torque_knm = allocation.torque_knm - a.torque_knm * *u;
                    *u = 0.;
                }
            }
            allocation.force_kn = allocation.force_kn + local;
            allocation.torque_knm = allocation.torque_knm + (point - centre).cross(local);
            if let Some(forces) = &mut self.resolved_aero_forces {
                forces.push(PartForce {
                    part: i,
                    force_kn: delta,
                    offset_m: point - self.rigid.rest_position(i),
                });
            }
        }
    }

    /// Combine rotation and translation at each RCS mount, then resolve its one fuel request.
    /// Until nozzle geometry is imported, each mount retains the bounded force-sphere proxy.
    fn resolve_rcs(
        &mut self,
        p: &mut Powered,
        propulsion: &mut crate::flight::Propulsion,
        allocation: &mut crate::actuators::Allocation,
        dt: f64,
    ) {
        if propulsion
            .actuators
            .as_ref()
            .is_some_and(|a| a.iter().any(|a| a.positive_only))
        {
            self.resolve_nozzle_rcs(p, propulsion, allocation, dt);
            return;
        }
        let channels = propulsion.actuators.as_ref().expect("configured actuators");
        let t = p.rcs.translate;
        let t = DVec3::new(
            t.x.clamp(-1.0, 1.0),
            t.y.clamp(-1.0, 1.0),
            t.z.clamp(-1.0, 1.0),
        );
        let translation = t * (1.0 / t.length().max(1.0));
        let mut requests = Vec::new();
        let mut mounts = Vec::new();
        p.rcs.usage = 0.0;
        for (part, def) in self.rcs_defs.iter().enumerate() {
            let Some(def) = def else { continue };
            let indices: Vec<_> = channels
                .iter()
                .enumerate()
                .filter(|(_, a)| a.part == part && a.group == part * 8 + 4)
                .map(|(i, _)| i)
                .collect();
            let mut inputs: Vec<_> = indices
                .iter()
                .map(|&i| {
                    let force = channels[i].force_kn;
                    allocation.inputs[i]
                        + if force.length() > 0.0 && !p.crashed && p.controlled {
                            force.normalized().dot(translation)
                        } else {
                            0.0
                        }
                })
                .collect();
            let norm = inputs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1.0);
            for input in &mut inputs {
                *input /= norm;
            }
            let effort = inputs.iter().map(|v| v * v).sum::<f64>().sqrt();
            let unit_mass = self.net.mixture_unit_mass_t(&def.propellant);
            let isp = def.isp_curve.evaluate(0.0);
            if effort <= 0.0 || unit_mass <= 0.0 || isp <= 0.0 {
                for i in indices {
                    let old = allocation.inputs[i];
                    allocation.inputs[i] = 0.0;
                    allocation.force_kn = allocation.force_kn - channels[i].force_kn * old;
                    allocation.torque_knm = allocation.torque_knm - channels[i].torque_knm * old;
                }
                continue;
            }
            let units_per_s = def.thrust_kn * effort / (isp * ose_data::G0 * unit_mass);
            requests.push(Request {
                part,
                instructions: self
                    .net
                    .ingredients(&def.propellant)
                    .into_iter()
                    .map(|(resource, units)| Instruction {
                        resource,
                        units_per_s: units * units_per_s,
                    })
                    .collect(),
                priority: 6,
            });
            mounts.push((indices, inputs, effort));
        }
        let before = self.net.mass_t();
        let delivered = self.net.process(&requests, dt);
        propulsion.burned_t += (before - self.net.mass_t()).max(0.0);
        let mut requested = 0.0;
        let mut supplied = 0.0;
        for ((indices, inputs, effort), fraction) in mounts.into_iter().zip(delivered) {
            requested += effort;
            supplied += effort * fraction;
            for (i, input) in indices.into_iter().zip(inputs) {
                let old = allocation.inputs[i];
                let new = input * fraction;
                allocation.inputs[i] = new;
                allocation.force_kn = allocation.force_kn + channels[i].force_kn * (new - old);
                allocation.torque_knm =
                    allocation.torque_knm + channels[i].torque_knm * (new - old);
            }
        }
        p.rcs.usage = supplied;
        p.rcs.powered = if requested > 0.0 {
            supplied / requested
        } else {
            0.0
        };
    }

    fn resolve_nozzle_rcs(
        &mut self,
        p: &mut Powered,
        propulsion: &mut crate::flight::Propulsion,
        allocation: &mut crate::actuators::Allocation,
        dt: f64,
    ) {
        let channels = propulsion.actuators.as_ref().unwrap();
        let indices: Vec<_> = channels
            .iter()
            .enumerate()
            .filter(|(_, a)| a.positive_only)
            .map(|(i, _)| i)
            .collect();
        let nozzles: Vec<_> = indices.iter().map(|&i| channels[i].clone()).collect();
        let translation = if p.controlled && !p.crashed {
            aero::rotate(p.control_rotation, p.rcs.translate)
        } else {
            DVec3::ZERO
        };
        let inputs: Vec<_> = nozzles
            .iter()
            .map(|a| {
                let cfg = p.part_control(a.part);
                let mask = |v: DVec3, offset: usize| {
                    let v = aero::rotate_inverse(p.control_rotation, v);
                    aero::rotate(
                        p.control_rotation,
                        DVec3::new(
                            if cfg.rcs_axes[offset] { v.x } else { 0. },
                            if cfg.rcs_axes[offset + 1] { v.y } else { 0. },
                            if cfg.rcs_axes[offset + 2] { v.z } else { 0. },
                        ),
                    )
                };
                if a.force_kn.length_squared() < 1e-16 {
                    0.
                } else {
                    crate::actuators::rcs_input(
                        a.force_kn.normalized(),
                        self.rigid.rest_position(a.part) + a.offset_m - propulsion.mass.centre_m,
                        mask(p.control_axes, 0),
                        mask(translation, 3),
                    )
                }
            })
            .collect();
        let request = nozzles
            .iter()
            .zip(&inputs)
            .fold(DVec3::ZERO, |s, (a, u)| s + a.torque_knm * *u);
        let force = nozzles
            .iter()
            .zip(&inputs)
            .fold(DVec3::ZERO, |s, (a, u)| s + a.force_kn * *u);
        let parts: Vec<_> = self
            .rcs_defs
            .iter()
            .enumerate()
            .filter(|(i, d)| d.is_some() && nozzles.iter().any(|a| a.part == *i))
            .map(|(i, _)| i)
            .collect();
        let mut budgets: Vec<_> = parts
            .iter()
            .map(|&i| (i, nozzles.iter().filter(|a| a.part == i).count() as f64))
            .collect();
        let mut planned = crate::actuators::Allocation {
            inputs,
            torque_knm: request,
            force_kn: force,
            ..Default::default()
        };
        let tanks = self.net.tanks.clone();
        let before = self.net.mass_t();
        let make_requests = |inputs: &[f64], net: &FlowNet| {
            parts
                .iter()
                .map(|&part| {
                    let def = self.rcs_defs[part].as_ref().unwrap();
                    let thrust = nozzles
                        .iter()
                        .zip(inputs)
                        .filter(|(a, _)| a.part == part)
                        .map(|(a, u)| a.force_kn.length() * *u)
                        .sum::<f64>();
                    let pressure = self.last_rcs_pressure_atm;
                    let isp = def.isp_curve.evaluate(pressure);
                    let rate = if isp > 0.0 {
                        thrust
                            / (isp
                                * ose_data::G0
                                * net.mixture_unit_mass_t(&def.propellant).max(1e-12))
                    } else {
                        0.0
                    };
                    Request {
                        part,
                        priority: 6,
                        instructions: net
                            .ingredients(&def.propellant)
                            .into_iter()
                            .map(|(resource, units)| Instruction {
                                resource,
                                units_per_s: units * rate,
                            })
                            .collect(),
                    }
                })
                .collect::<Vec<_>>()
        };
        let requested = planned.inputs.iter().sum::<f64>();
        let delivered = self
            .net
            .process(&make_requests(&planned.inputs, &self.net), dt);
        if delivered.iter().any(|f| *f < 1.0 - 1e-10) {
            for ((part, budget), fraction) in budgets.iter_mut().zip(delivered) {
                *budget = nozzles
                    .iter()
                    .zip(&planned.inputs)
                    .filter(|(a, _)| a.part == *part)
                    .map(|(_, u)| *u)
                    .sum::<f64>()
                    * fraction;
            }
            self.net.tanks = tanks;
            planned = crate::actuators::nozzle_wrench(&nozzles, request, force, &budgets);
            let delivered = self
                .net
                .process(&make_requests(&planned.inputs, &self.net), dt);
            for (a, u) in nozzles.iter().zip(&mut planned.inputs) {
                let index = parts.iter().position(|i| *i == a.part).unwrap();
                *u *= delivered[index];
            }
        }
        for (&i, input) in indices.iter().zip(&planned.inputs) {
            let a = &channels[i];
            let mounted = self.rigid.rest_position(a.part) + a.offset_m;
            let local = aero::rotate_inverse(
                self.part_rot[a.part],
                crate::rigid::to_vessel(mounted - self.rigid.rest_position(a.part)),
            );
            let rated = self.rcs_defs[a.part].as_ref().unwrap().thrust_kn;
            self.rcs_nozzles.push((
                a.part as u16,
                DVec3::new(-local.x, local.y, local.z),
                (if rated > 0. {
                    a.force_kn.length() / rated * *input
                } else {
                    0.
                }) as f32,
            ));
            let old = allocation.inputs[i];
            allocation.inputs[i] = *input;
            allocation.force_kn = allocation.force_kn + channels[i].force_kn * (*input - old);
            allocation.torque_knm = allocation.torque_knm + channels[i].torque_knm * (*input - old);
        }
        propulsion.burned_t += (before - self.net.mass_t()).max(0.0);
        p.rcs.usage = planned.inputs.iter().sum();
        p.rcs.powered = if requested > 0.0 {
            (p.rcs.usage / requested).min(1.0)
        } else {
            0.0
        };
    }

    /// Heat the parts for one step (see `thermal`): `v_air` is the velocity through the air (inertial), the
    /// air at the vessel has `density`, `mach`, `ambient_k` and `conductivity`. Returns the parts that went
    /// past their maximum temperature (when parts may burn up).
    #[allow(clippy::too_many_arguments)]
    pub fn update_heat(
        &mut self,
        frame: (DVec3, DVec3, DVec3),
        v_air: DVec3,
        density: f64,
        mach: f64,
        ambient_k: f64,
        conductivity: f64,
        settings: &thermal::HeatSettings,
        dt: f64,
    ) -> Vec<usize> {
        self.update_heat_in_flow(
            Airflow::bulk(frame, v_air, density, mach),
            ambient_k,
            conductivity,
            settings,
            dt,
        )
    }

    pub fn update_heat_in_flow(
        &mut self,
        flow: Airflow,
        ambient_k: f64,
        conductivity: f64,
        settings: &thermal::HeatSettings,
        dt: f64,
    ) -> Vec<usize> {
        let present: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .collect();
        let (bx, by, bz) = flow.frame;
        let centre = self.mass_properties().centre_m;
        let boxes: Vec<(DVec3, DVec3)> = present.iter().map(|&i| self.part_box(i)).collect();
        let local =
            |velocity: DVec3| DVec3::new(velocity.dot(bx), velocity.dot(by), velocity.dot(bz));
        let exposed = if flow.angular_velocity.length_squared() == 0.0
            && flow.velocity.length_squared() > 0.0
        {
            thermal::exposed_proportions(
                &boxes,
                local(flow.velocity).normalized(),
                flow.mach(flow.velocity.length()),
            )
        } else {
            present
                .iter()
                .enumerate()
                .map(|(k, &i)| {
                    let velocity = local(flow.velocity_at(self.part_box(i).0 - centre));
                    thermal::exposed_proportion_at(
                        &boxes,
                        k,
                        velocity,
                        flow.mach(velocity.length()),
                    )
                })
                .collect()
        };
        let mut burned = Vec::new();
        for (k, &i) in present.iter().enumerate() {
            let velocity = local(flow.velocity_at(self.part_box(i).0 - centre));
            let speed = velocity.length();
            let mach = flow.mach(speed);
            let d = if speed > 0.0 {
                velocity * (1.0 / speed)
            } else {
                DVec3::ZERO
            };
            let du = DVec3::new(d.x, d.y, -d.z);
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
            // under a fairing or in a closed bay the airflow does not reach it
            let reach = if self.shielded[i] { 0.0 } else { exposed[k] };
            let mut flux = thermal::reentry_flux(mach, reach, airstream, flow.density, cd);
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
            // legs and wheels out take part of the blow on their suspension (decelerating at about 3 g over
            // their travel: our own figure)
            let travel = feet
                .iter()
                .filter(|&&i| self.gear_out[i])
                .filter_map(|&i| self.gear_defs[i].as_ref().map(|g| g.suspension_m))
                .fold(0.0, f64::max);
            let speed_on_parts = (speed * speed - 2.0 * SUSPENSION_DECEL * travel)
                .max(0.0)
                .sqrt();
            if feet.is_empty() || speed_on_parts <= tolerance {
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

    /// Remove the actual parts struck by the bounds contact solver, preserving craft roll for debris.
    pub fn contact_damage(&mut self, parts: &[usize], p: &Powered) -> Option<Vec<Piece>> {
        if parts.contains(&self.root) {
            return None;
        }
        self.rigid.remove_parts(parts);
        self.net.remove_parts(parts);
        let (x, y, z) = crate::flight::vessel_basis(p);
        Some(self.split_off(x, y, z))
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

    /// Fire flight stage `stage`'s activations: parachutes in it arm, fairings in it jettison.
    pub fn activate_stage(&mut self, stage: usize) {
        let parts = self
            .stage_activations
            .get(stage)
            .cloned()
            .unwrap_or_default();
        for i in parts {
            self.arm_chute(i);
            if self.fairing_payload[i].is_some() && self.rigid.present[i] {
                self.fairing_deployed[i] = true;
            }
        }
        self.update_shielding();
    }

    /// Jettison every fairing; returns how many.
    pub fn deploy_fairings(&mut self) -> usize {
        let mut n = 0;
        for i in 0..self.fairing_deployed.len() {
            if self.fairing_payload[i].is_some()
                && self.rigid.present[i]
                && !self.fairing_deployed[i]
            {
                self.fairing_deployed[i] = true;
                n += 1;
            }
        }
        self.update_shielding();
        n
    }

    /// Open (or close) the cargo bays; returns how many changed.
    pub fn open_bays(&mut self, open: bool) -> usize {
        let mut n = 0;
        for i in 0..self.bay_open.len() {
            if self.cargo_bay[i] && self.rigid.present[i] && self.bay_open[i] != open {
                self.bay_open[i] = open;
                n += 1;
            }
        }
        self.update_shielding();
        n
    }

    /// Which parts the airflow cannot reach: under a fairing still on, or inside a closed cargo bay.
    pub fn update_shielding(&mut self) {
        let n = self.rigid.len();
        let mut shielded = vec![false; n];
        for f in 0..n {
            if let Some(payload) = &self.fairing_payload[f] {
                if self.rigid.present[f] && !self.fairing_deployed[f] {
                    for &j in payload {
                        shielded[j] = true;
                    }
                }
            }
            if self.cargo_bay[f] && self.rigid.present[f] && !self.bay_open[f] {
                let (bc, bh) = self.part_box(f);
                for (j, s) in shielded.iter_mut().enumerate() {
                    if j == f || !self.rigid.present[j] {
                        continue;
                    }
                    let (c, _) = self.part_box(j);
                    let d = c - bc;
                    if d.x.abs() <= bh.x && d.y.abs() <= bh.y && d.z.abs() <= bh.z {
                        *s = true;
                    }
                }
            }
        }
        self.shielded = shielded;
    }

    /// The fairings' shells still on, each around its payload: (part, frontal area, side area), m^2, about the
    /// vessel axis.
    fn fairing_shells(&self) -> Vec<(usize, f64, f64)> {
        (0..self.rigid.len())
            .filter_map(|f| {
                let payload = self.fairing_payload[f].as_ref()?;
                if !self.rigid.present[f] || self.fairing_deployed[f] || payload.is_empty() {
                    return None;
                }
                let (fc, _) = self.part_box(f);
                let (mut radius, mut top) = (0.0_f64, fc.y);
                for &j in payload.iter().filter(|&&j| self.rigid.present[j]) {
                    let (pc, ph) = self.part_box(j);
                    let d = pc - fc;
                    radius = radius.max((d.x * d.x + d.z * d.z).sqrt() + ph.x.max(ph.z));
                    top = top.max(pc.y + ph.y);
                }
                let height = (top - fc.y).max(0.1);
                Some((
                    f,
                    std::f64::consts::PI * radius * radius,
                    height * 2.0 * radius,
                ))
            })
            .collect()
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

    fn lifting_force(
        &self,
        part: usize,
        frame: (DVec3, DVec3, DVec3),
        travel: DVec3,
        q: f64,
        mach: f64,
    ) -> DVec3 {
        let Some((surface, curves)) = &self.lift_defs[part] else {
            return DVec3::ZERO;
        };
        if !self.rigid.present[part] || self.shielded[part] || surface.deflection_lift_coeff <= 0.0
        {
            return DVec3::ZERO;
        }
        let (x, y, z) = frame;
        let axis = crate::rigid::to_vessel(aero::rotate(self.part_rot[part], surface.axis));
        let normal = x * axis.x + y * axis.y + z * axis.z;
        let fixed = aero::surface_force(curves, surface, normal, travel, q, mach);
        if let Some(hinge) = &self.geometry[part].surface {
            let h =
                crate::rigid::to_vessel(aero::rotate(self.part_rot[part], hinge.axis)).normalized();
            let rotation = glam::DQuat::from_axis_angle(
                glam::DVec3::new(h.x, h.y, h.z),
                -self.surface_angles[part],
            );
            let neutral = crate::rigid::to_vessel(aero::rotate(self.part_rot[part], hinge.normal));
            let rotated = rotation * glam::DVec3::new(neutral.x, neutral.y, neutral.z);
            let moving = aero::surface_force(
                curves,
                surface,
                x * rotated.x + y * rotated.y + z * rotated.z,
                travel,
                q,
                mach,
            );
            let share = self.ctrl[part].1.clamp(0., 1.);
            fixed * (1. - share) + moving * share
        } else {
            fixed
        }
    }

    /// Lift plus induced drag of the exposed lifting surfaces, in inertial kN.
    /// `frame` includes craft roll and uses the same axes as propulsion and rendering.
    pub fn lift(&self, frame: (DVec3, DVec3, DVec3), v_air: DVec3, rho: f64, mach: f64) -> DVec3 {
        let speed = v_air.length();
        if speed <= 0.0 || rho <= 0.0 {
            return DVec3::ZERO;
        }
        (0..self.rigid.len()).fold(DVec3::ZERO, |sum, i| {
            sum + self.lifting_force(
                i,
                frame,
                v_air * (1.0 / speed),
                0.5 * rho * speed * speed,
                mach,
            )
        })
    }

    /// Actual per-surface lift and coefficient for condensation effects, using the same curves and
    /// attached-part mask as the flight force calculation.
    pub fn lifting_fx(
        &self,
        frame: (DVec3, DVec3, DVec3),
        v_air: DVec3,
        rho: f64,
        mach: f64,
    ) -> Vec<(u16, f32, f32)> {
        self.lifting_fx_in_flow(Airflow::bulk(frame, v_air, rho, mach))
    }

    pub fn lifting_fx_in_flow(&self, flow: Airflow) -> Vec<(u16, f32, f32)> {
        if flow.density <= 0.0 {
            return Vec::new();
        }
        let centre = self.mass_properties().centre_m;
        self.lift_defs
            .iter()
            .enumerate()
            .filter_map(|(i, def)| {
                let (surface, _) = def.as_ref()?;
                if !self.rigid.present[i]
                    || self.shielded[i]
                    || surface.deflection_lift_coeff <= 0.0
                {
                    return None;
                }
                let velocity = flow.velocity_at(self.part_box(i).0 - centre);
                let speed = velocity.length();
                if speed <= 0.0 {
                    return None;
                }
                let travel = velocity * (1.0 / speed);
                let q = 0.5 * flow.density * speed * speed;
                let force = self.lifting_force(i, flow.frame, travel, q, flow.mach(speed));
                let lift = force - travel * force.dot(travel);
                Some((
                    u16::try_from(i).ok()?,
                    surface.deflection_lift_coeff as f32,
                    lift.length() as f32,
                ))
            })
            .collect()
    }

    /// Per-part inertial forces with craft-frame application offsets, following the last drag update.
    fn aero_loads(
        &self,
        frame: (DVec3, DVec3, DVec3),
        v_air: DVec3,
        rho: f64,
        mach: f64,
    ) -> Vec<PartForce> {
        let speed = v_air.length();
        if speed <= 0.0 || rho <= 0.0 {
            return Vec::new();
        }
        let travel = v_air * (1.0 / speed);
        let q = 0.5 * rho * speed * speed;
        (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .map(|i| PartForce {
                part: i,
                force_kn: -travel * (q * self.part_cda_m2[i] / 1000.0)
                    + self.lifting_force(i, frame, travel, q, mach),
                offset_m: self.part_box(i).0 - self.rigid.rest_position(i),
            })
            .collect()
    }

    fn airflow_moment(&self, frame: (DVec3, DVec3, DVec3), forces: &[PartForce]) -> DVec3 {
        let (x, y, z) = frame;
        let centre = self.mass_properties().centre_m;
        forces.iter().fold(DVec3::ZERO, |sum, f| {
            let r = self.rigid.rest_position(f.part) + f.offset_m - centre;
            sum + (x * r.x + y * r.y + z * r.z).cross(f.force_kn)
        })
    }

    /// Moment of the same part forces used by translation and structural loading, about current mass centre.
    pub fn aero_torque(
        &self,
        frame: (DVec3, DVec3, DVec3),
        v_air: DVec3,
        rho: f64,
        mach: f64,
    ) -> DVec3 {
        self.airflow_moment(frame, &self.aero_loads(frame, v_air, rho, mach))
    }

    /// Resolve airflow once for flight and preserve its per-part application points for the joint solver.
    pub fn aerodynamics(
        &mut self,
        stage: usize,
        frame: (DVec3, DVec3, DVec3),
        v_air: DVec3,
        rho: f64,
        mach: f64,
    ) -> AeroForces {
        self.aerodynamics_in_flow(
            stage,
            Airflow {
                frame,
                velocity: v_air,
                angular_velocity: DVec3::ZERO,
                density: rho,
                speed_of_sound: if mach > 0.0 {
                    v_air.length() / mach
                } else {
                    f64::INFINITY
                },
            },
        )
    }

    /// Evaluate each attached part at v_COM + omega_relative cross its mass-centred application lever.
    /// Retain the individual forces even when their resultant cancels (a pure damping couple).
    pub fn aerodynamics_in_flow(&mut self, stage: usize, flow: Airflow) -> AeroForces {
        if stage != self.staged_to {
            self.trim_to(stage);
        }
        let centre = self.mass_properties().centre_m;
        let (x, y, z) = flow.frame;
        let shells = self.fairing_shells();
        let mut forces = Vec::new();
        let mut out = AeroForces::default();
        self.part_cda_m2.fill(0.0);
        self.last_air_speed_mps = flow.velocity.length();
        self.last_airflow = Some(flow);
        for i in 0..self.rigid.len() {
            if !self.rigid.present[i] {
                continue;
            }
            let at = if self.geometry[i].inertia_source.is_empty() {
                self.part_box(i).0
            } else {
                self.part_point(i, self.geometry[i].pressure_offset_m)
            };
            let velocity = flow.velocity_at(at - centre);
            let speed = velocity.length();
            let mut force_kn = DVec3::ZERO;
            if speed > 0.0 && flow.density > 0.0 && !self.shielded[i] {
                let travel = velocity * (1.0 / speed);
                let mach = flow.mach(speed);
                let unity = DVec3::new(travel.dot(x), travel.dot(y), -travel.dot(z));
                let local = aero::rotate_inverse(self.part_rot[i], unity);
                let area_drag = self.part_area_drag(i, local, mach);
                let mut cda = aero::cda_m2(&self.curves, area_drag, flow.density * speed);
                let along = unity.y.abs().min(1.0);
                let across = (1.0 - along * along).max(0.0).sqrt();
                for &(_, front, side) in shells.iter().filter(|s| s.0 == i) {
                    cda += front * FAIRING_CD_FRONT * along + side * FAIRING_CD_SIDE * across;
                }
                self.part_cda_m2[i] = cda;
                out.cda_m2 += cda;
                let q = 0.5 * flow.density * speed * speed;
                let drag = -travel * (q * cda / 1000.0);
                let lift_at = self.lift_point(i);
                let lift_velocity = flow.velocity_at(lift_at - centre);
                let lift_speed = lift_velocity.length();
                let lift_travel = if lift_speed > 0. {
                    lift_velocity * (1. / lift_speed)
                } else {
                    DVec3::ZERO
                };
                let lift = self.lifting_force(
                    i,
                    flow.frame,
                    lift_travel,
                    0.5 * flow.density * lift_speed * lift_speed,
                    flow.mach(lift_speed),
                );
                if (lift_at - at).length_squared() > 1e-14 {
                    forces.push(PartForce {
                        part: i,
                        force_kn: lift,
                        offset_m: lift_at - self.rigid.rest_position(i),
                    });
                }
                out.drag_kn = out.drag_kn + drag;
                out.lift_kn = out.lift_kn + lift;
                force_kn = if (self.lift_point(i) - at).length_squared() > 1e-14 {
                    drag
                } else {
                    drag + lift
                };
            }
            forces.push(PartForce {
                part: i,
                force_kn,
                offset_m: at - self.rigid.rest_position(i),
            });
        }
        out.torque_knm = self.airflow_moment(flow.frame, &forces);
        self.resolved_aero_forces = Some(forces);
        out
    }

    fn part_area_drag(&self, i: usize, local: DVec3, mach: f64) -> f64 {
        let area_drag = self.faces[i]
            .as_ref()
            .map(|f| f.area_drag(&self.curves, local, mach))
            .unwrap_or(0.0);
        let Some(c) = &self.chute_defs[i] else {
            return area_drag;
        };
        let shape = |cube: &Option<ose_data::DragCube>| {
            cube.as_ref()
                .map(|q| Faces::from_cube(q).area_drag(&self.curves, local, mach))
                .unwrap_or(area_drag)
        };
        let t = self.chute_open[i];
        match self.chutes[i] {
            Chute::SemiDeployed => area_drag + (shape(&c.semi_cube) - area_drag) * t,
            Chute::Deployed => {
                let semi = shape(&c.semi_cube);
                semi + (shape(&c.deployed_cube) - semi) * t
            }
            _ => area_drag,
        }
    }

    /// How far each control surface deflects for steering command `cmd` (inertial, a share of full turning
    /// about each axis): the surface turns so its lift's torque helps the command, up to its range, rad.
    pub fn surface_deflections(
        &self,
        frame: (DVec3, DVec3, DVec3),
        cmd: DVec3,
    ) -> Vec<(usize, f64)> {
        let (bx, by, bz) = frame;
        let to_inertial = |v: DVec3| bx * v.x + by * v.y + bz * v.z;
        let c = self.mass_properties().centre_m;
        (0..self.rigid.len())
            .filter_map(|i| {
                let (range, share) = self.ctrl[i];
                if self.geometry[i].surface.is_some() {
                    return (self.rigid.present[i]
                        && !self.shielded[i]
                        && range > 0.0
                        && share > 0.0)
                        .then_some((i, self.surface_angles[i]));
                }
                let (l, _) = self.lift_defs[i].as_ref()?;
                if range <= 0.0 || share <= 0.0 || !self.rigid.present[i] || self.shielded[i] {
                    return None;
                }
                let u = aero::rotate(self.part_rot[i], l.axis);
                let axis = bx * u.x + by * u.y + bz * (-u.z);
                let (centre, _) = self.part_box(i);
                // the torque a push along the surface's axis gives
                let lever = to_inertial(centre - c).cross(axis);
                let along = if lever.length() > 1e-6 {
                    cmd.dot(lever.normalized()).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                Some((i, along * range))
            })
            .collect()
    }

    /// How much torque the control surfaces can give at full deflection in this airflow, kN·m: each moving
    /// surface's extra lift at its deflection range (dynamic pressure × 0.036 × lift coefficient × moving share
    /// × the lift curve at the range × its Mach curve) at its distance from the centre of mass.
    pub fn control_authority(&self, v_air: DVec3, rho: f64, mach: f64) -> f64 {
        // Legacy bulk-velocity query; per-part flight uses the full rolled flow below.
        self.control_authority_in_flow(Airflow::bulk(
            basis(DVec3::new(0.0, 1.0, 0.0)),
            v_air,
            rho,
            mach,
        ))
    }

    pub fn control_authority_in_flow(&self, flow: Airflow) -> f64 {
        if flow.density <= 0.0 {
            return 0.0;
        }
        let c = self.mass_properties().centre_m;
        let mut total = 0.0;
        for i in 0..self.rigid.len() {
            let (range, share) = self.ctrl[i];
            let Some((l, curves)) = &self.lift_defs[i] else {
                continue;
            };
            if range <= 0.0 || share <= 0.0 || !self.rigid.present[i] || self.shielded[i] {
                continue;
            }
            let speed = flow.velocity_at(self.part_box(i).0 - c).length();
            let q = 0.5 * flow.density * speed * speed;
            let mach = flow.mach(speed);
            let lift = q
                * aero::LIFT_MULTIPLIER
                * l.deflection_lift_coeff
                * share
                * curves.lift.evaluate(range.sin())
                * curves.lift_mach.evaluate(mach);
            let (centre, _) = self.part_box(i);
            total += lift.max(0.0) * (centre - c).length();
        }
        total
    }

    /// Drag in the rolled craft frame: sets each part's `Cd * A` and returns the total, m^2.
    /// Parts without a cube add none; a fairing's shell contribution belongs to its base part.
    pub fn aero(
        &mut self,
        stage: usize,
        frame: (DVec3, DVec3, DVec3),
        v_air: DVec3,
        rho: f64,
        mach: f64,
    ) -> f64 {
        if stage != self.staged_to {
            self.trim_to(stage);
        }
        let speed = v_air.length();
        self.last_air_speed_mps = speed;
        self.last_airflow = Some(Airflow::bulk(frame, v_air, rho, mach));
        self.part_cda_m2.iter_mut().for_each(|c| *c = 0.0);
        if speed <= 0.0 {
            return 0.0;
        }
        let (bx, by, bz) = frame;
        let d = v_air * (1.0 / speed);
        // vessel frame, then the craft's Unity axes (Z negated)
        let du = DVec3::new(d.dot(bx), d.dot(by), -d.dot(bz));
        let mut total = 0.0;
        for i in 0..self.rigid.len() {
            if !self.rigid.present[i] {
                continue;
            }
            if self.shielded[i] {
                continue;
            }
            let local = aero::rotate_inverse(self.part_rot[i], du);
            let area_drag = self.part_area_drag(i, local, mach);
            let cda = aero::cda_m2(&self.curves, area_drag, rho * speed);
            self.part_cda_m2[i] = cda;
            total += cda;
        }
        // fairing shells still on: a nose cone end on, a cylinder side on
        let along = du.y.abs().min(1.0);
        let across = (1.0 - along * along).max(0.0).sqrt();
        for (part, front, side) in self.fairing_shells() {
            let cda = front * FAIRING_CD_FRONT * along + side * FAIRING_CD_SIDE * across;
            self.part_cda_m2[part] += cda;
            total += cda;
        }
        total
    }

    /// Load the structure with one flight step's thrust and drag. Pieces that broke off are removed from the
    /// structure and returned; the caller takes their mass off the vessel and spawns them.
    pub fn apply(&mut self, p: &Powered, out: &StepOutcome, dt: f64) -> Vec<Piece> {
        // RCS and converters may have changed tank contents since the flight-state sync.
        self.sync_structural_mass();
        let resolved = self.resolved_engine_thrust.take();
        let nozzle_forces = std::mem::take(&mut self.resolved_nozzle_forces);
        let aero_forces = self.resolved_aero_forces.take();
        let surface_forces = std::mem::take(&mut self.surface_loads);
        let actuators = self.resolved_actuators.take();
        self.engine_gimbals.clear();
        if let (Some(channels), Some(thrusts)) = (&actuators, &resolved) {
            for i in self.live_engines(p.stage) {
                if self.geometry[i]
                    .engines
                    .first()
                    .is_some_and(|j| !j.is_empty())
                {
                    continue;
                }
                if thrusts[i] <= 0.0 || self.gimbal_range_rad[i] <= 0.0 {
                    continue;
                }
                let axes = self.part_axes(i);
                let mut force = axes[1] * thrusts[i];
                for (a, input) in channels.iter().zip(&out.actuator_inputs) {
                    if a.part == i && a.axial.is_some() {
                        force = force + a.force_kn * *input;
                    }
                }
                for correction in &out.actuator_axial_corrections {
                    if correction.part == i {
                        force = force + correction.force_kn;
                    }
                }
                if let Ok(part) = u16::try_from(i) {
                    self.engine_gimbals.push((
                        part,
                        DVec3::new(-force.dot(axes[0]), force.dot(axes[1]), -force.dot(axes[2]))
                            .normalized(),
                    ));
                }
            }
        }
        if p.stage != self.staged_to {
            self.trim_to(p.stage);
        }
        let (bx, by, bz) = crate::flight::vessel_basis(p);
        let to_vessel = |v: DVec3| DVec3::new(v.dot(bx), v.dot(by), v.dot(bz));
        let alpha = to_vessel(out.angular_accel);
        let turning = alpha.length() > 1e-4 || out.angvel.length() > 1e-3;
        let loaded = !surface_forces.is_empty()
            || actuators.as_ref().is_some_and(|channels| {
                channels.iter().zip(&out.actuator_inputs).any(|(a, input)| {
                    a.force_kn.length() * input.abs() >= QUIET_KN
                        || a.torque_knm.length() * input.abs() >= QUIET_KN
                })
            })
            || aero_forces
                .as_ref()
                .is_some_and(|forces| forces.iter().any(|f| f.force_kn.length() >= QUIET_KN));
        if p.crashed
            || (p.landed && !loaded)
            || (out.thrust_kn < QUIET_KN && out.drag_kn.length() < QUIET_KN && !turning && !loaded)
        {
            // nothing pushes the structure: let it ring down
            self.engine_load_kn.iter_mut().for_each(|x| *x = 0.0);
            self.rigid.relax(dt);
            return Vec::new();
        }
        let present: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .collect();
        let mut forces: Vec<PartForce> = surface_forces
            .into_iter()
            .filter(|f| self.rigid.present[f.part])
            .map(|f| PartForce {
                force_kn: to_vessel(f.force_kn),
                ..f
            })
            .collect();
        // thrust over the attached engines of the burning stage
        if let Some(delivered) = resolved {
            self.engine_load_target_kn.copy_from_slice(&delivered);
        } else if out.thrust_kn > 0.0 {
            if let Some(engines) = self.stage_engines.get(p.stage) {
                // only engines that still have propellant push
                let live: Vec<usize> = engines
                    .iter()
                    .copied()
                    .filter(|&i| {
                        self.rigid.present[i] && self.engine_thrust_kn[i] > 0.0 && self.fed(i)
                    })
                    .collect();
                let rated: f64 = live
                    .iter()
                    .map(|&i| self.engine_thrust_kn[i] * self.engine_duty(i, p.throttle))
                    .sum();
                for &i in &live {
                    self.engine_load_target_kn[i] =
                        out.thrust_kn * self.engine_thrust_kn[i] * self.engine_duty(i, p.throttle)
                            / rated;
                }
            }
        }
        // Authored engine forces use the same nozzle positions/directions as flight dynamics.
        forces.extend(nozzle_forces.into_iter().filter(|f| {
            self.geometry[f.part]
                .engines
                .first()
                .is_some_and(|j| !j.is_empty())
        }));
        // Legacy catalogs: engine thrust reaches the structure through a short lag: solid boosters tail off and liquid engines
        // spool (the original's engine response and thrust curves) rather than stepping, which would ring the
        // joints far past their static load.
        let k = 1.0 - (-dt / THRUST_LAG_S).exp();
        for i in 0..self.engine_load_kn.len() {
            let target = self.engine_load_target_kn[i];
            self.engine_load_target_kn[i] = 0.0;
            self.engine_load_kn[i] += (target - self.engine_load_kn[i]) * k;
            if self.rigid.present[i]
                && self.engine_load_kn[i].abs() > 1e-6
                && self.geometry[i]
                    .engines
                    .first()
                    .is_none_or(|j| j.is_empty())
            {
                forces.push(PartForce {
                    part: i,
                    force_kn: self.part_axes(i)[1] * self.engine_load_kn[i],
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
        if let Some(loads) = aero_forces {
            forces.extend(
                loads
                    .into_iter()
                    .filter(|f| self.rigid.present[f.part])
                    .map(|f| PartForce {
                        force_kn: to_vessel(f.force_kn),
                        ..f
                    }),
            );
        } else if drag.length() >= QUIET_KN && cda_total > 0.0 {
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
        let mut turn = turning.then(|| {
            let need = out.control_torque_knm.map(to_vessel).unwrap_or_else(|| {
                self.rigid
                    .torque_for(to_vessel(out.angular_accel - out.external_angular_accel))
            });
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
        if let Some(channels) = actuators {
            let t = turn.get_or_insert_with(|| Turning {
                alpha,
                omega: to_vessel(out.angvel),
                ..Default::default()
            });
            t.torques.clear();
            t.forces.clear();
            forces.extend(
                out.actuator_axial_corrections
                    .iter()
                    .filter(|f| self.rigid.present[f.part])
                    .copied(),
            );
            for (a, input) in channels.iter().zip(&out.actuator_inputs) {
                if self.rigid.present[a.part] {
                    if a.force_kn.length_squared() > 0.0 {
                        forces.push(PartForce {
                            part: a.part,
                            force_kn: a.force_kn * *input,
                            offset_m: a.offset_m,
                        });
                    } else {
                        t.torques.push((a.part, a.torque_knm * *input));
                    }
                }
            }
        }
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

    #[test]
    fn empty_structure_has_no_drive_root() {
        let craft = Craft {
            name: "empty legacy craft".into(),
            description: String::new(),
            parts: Vec::new(),
            stages: Vec::new(),
        };
        let structure = Structure::build(&craft, &[], &[]);
        let mut p = Powered::new(
            FlightSpec {
                stages: Vec::new(),
                drag_area_m2: 0.0,
                drag_cd: 0.0,
            },
            DVec3::new(700000.0, 0.0, 0.0),
            DVec3::ZERO,
            false,
        );
        structure.sync_drive(&mut p);
        assert!(p.drive.wheels.is_empty());
    }

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

    fn airflow_fixture() -> (Structure, Powered, usize) {
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let mut s = Structure::build(craft, &defs, &res);
        let wing = s.lift_defs.iter().position(Option::is_some).unwrap();
        let mut lift = s.lift_defs[wing].clone().unwrap();
        lift.0.axis = DVec3::new(0.0, 0.0, 1.0);
        lift.0.deflection_lift_coeff = 1.0;
        lift.0.omnidirectional = true;
        let linear = ose_data::Curve {
            keys: [0.0, 1.0]
                .into_iter()
                .map(|t| ose_data::CurveKey {
                    t,
                    v: t,
                    in_tangent: 1.0,
                    out_tangent: 1.0,
                })
                .collect(),
        };
        let constant = |v| ose_data::Curve {
            keys: vec![ose_data::CurveKey {
                v,
                ..Default::default()
            }],
        };
        lift.1 = aero::LiftCurves {
            lift: linear,
            lift_mach: constant(1.0),
            drag: constant(0.0),
            drag_mach: constant(1.0),
        };
        s.lift_defs.fill(None);
        s.lift_defs[wing] = Some(lift);
        s.faces.fill(None);
        s.geometry.fill(Default::default());
        s.ctrl.fill((0.0, 0.0));
        s.ctrl[wing] = (0.2, 1.0);
        s.part_rot[wing] = [0.0, 0.0, 0.0, 1.0];
        (s, flying(DVec3::new(0.0, 1.0, 0.0)), wing)
    }

    #[test]
    fn spinning_airflow_opposes_rotation_even_without_bulk_travel() {
        let (mut s, mut p) = spinning_airflow_fixture();
        let frame = crate::flight::vessel_basis(&p);
        let omega = DVec3::new(0.0, 0.0, 10.0);
        let out = s.aerodynamics_in_flow(
            0,
            Airflow {
                frame,
                velocity: DVec3::ZERO,
                angular_velocity: omega,
                density: 1.0,
                speed_of_sound: 100.0,
            },
        );
        // Two 4.8 m^2 Cd*A boxes at +/-2 m: speed 20 m/s, 0.96 kN each.
        assert!(out.drag_kn.length() < 1e-12);
        assert!((out.torque_knm - DVec3::new(0.0, 0.0, -3.84)).length() < 1e-10);
        assert!(out.torque_knm.dot(omega) < 0.0);
        let loads = s.resolved_aero_forces.as_ref().unwrap();
        assert!((loads[0].force_kn + loads[1].force_kn).length() < 1e-12);
        assert!((loads[0].force_kn.length() - 0.96).abs() < 1e-10);
        // The angular impulse and energy change follow the same resolved couple.
        s.sync(&mut p);
        p.angvel = omega;
        p.attitude = crate::flight::Attitude::Free;
        p.aero_torque_knm = out.torque_knm;
        p.lift_kn = out.lift_kn;
        let mass = s.mass_properties();
        let engine = crate::flight::Propulsion {
            mass,
            actuators: Some(Vec::new()),
            aerodynamic_drag_kn: Some(out.drag_kn),
            ..Default::default()
        };
        let mut body = test_body();
        body.mu = 0.0;
        let step = crate::flight::integrate_with_propulsion(
            &mut p,
            &body,
            None,
            None,
            0.0,
            0.02,
            Some(engine),
        );
        let expected_rate = 10.0 - 3.84 * 0.02 / mass.inertia.apply(DVec3::new(0.0, 0.0, 1.0)).z;
        assert!((p.angvel.z - expected_rate).abs() < 1e-9);
        assert!(p.angvel.length() < omega.length());
        assert!(p.v.length() < 1e-12 && step.drag_kn.length() < 1e-12);
        s.apply(&p, &step, 0.02);
        assert!(
            s.rigid.links.iter().any(|l| l.force_kn > 0.1),
            "a zero-resultant damping couple must still load the joints"
        );
    }

    fn spinning_airflow_fixture() -> (Structure, Powered) {
        let (crafts, defs, resources) = assets();
        let mut part = crafts[0].parts[0].clone();
        part.parent = None;
        part.local_position = DVec3::ZERO;
        part.local_rotation = ose_data::craft::Quat {
            w: 1.0,
            ..Default::default()
        };
        part.own_node = None;
        part.parent_node = None;
        part.resources.clear();
        let mut second = part.clone();
        second.guid = "opposite-box".into();
        second.parent = Some(0);
        second.local_position = DVec3::new(4.0, 0.0, 0.0);
        let craft = Craft {
            name: "Damping pair".into(),
            description: String::new(),
            parts: vec![part, second],
            stages: vec![Vec::new()],
        };
        let mut s = Structure::build(&craft, &defs, &resources);
        s.geometry.fill(Default::default());
        s.net.dry_t.fill(1.0);
        s.net.tanks.clear();
        s.rigid.sizes_m.fill(DVec3::new(1.0, 1.0, 1.0));
        s.stowed_box.fill((DVec3::ZERO, DVec3::new(0.5, 0.5, 0.5)));
        s.extended_box.fill(None);
        s.faces.fill(Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        }));
        s.lift_defs.fill(None);
        s.chute_defs.fill(None);
        s.fairing_payload.fill(None);
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.gimbal_range_rad.fill(0.0);
        s.rcs_defs.fill(None);
        s.electric.fill(Default::default());
        s.crew[0] = 1;
        let constant = ose_data::Curve {
            keys: vec![ose_data::CurveKey {
                v: 1.0,
                ..Default::default()
            }],
        };
        s.curves = DragCurves {
            tip: constant.clone(),
            tail: constant.clone(),
            surface: constant.clone(),
            multiplier: constant.clone(),
            cd: constant.clone(),
            cd_power: constant.clone(),
            pseudo_reynolds: constant,
        };
        let mut p = flying(DVec3::new(0.0, 1.0, 0.0));
        p.ignited = false;
        p.throttle = 0.0;
        p.attitude = crate::flight::Attitude::Free;
        (s, p)
    }

    #[test]
    fn angular_airflow_uses_local_mach_and_dynamic_pressure_and_masks_parts() {
        let (mut s, p) = spinning_airflow_fixture();
        s.curves.multiplier = ose_data::Curve {
            keys: [0.0, 10.0]
                .into_iter()
                .map(|t| ose_data::CurveKey {
                    t,
                    v: 1.0 + t,
                    in_tangent: 1.0,
                    out_tangent: 1.0,
                })
                .collect(),
        };
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::new(0.0, 1.0, 0.0),
            angular_velocity: DVec3::new(0.0, 0.0, 1.0),
            density: 1.0,
            speed_of_sound: 1.0,
        };
        let out = s.aerodynamics_in_flow(0, flow);
        assert!((out.drag_kn.y + 0.5 * 4.8 * (9.0 * 4.0 - 2.0) / 1000.0).abs() < 1e-12);
        assert!((s.part_cda_m2[0] - 9.6).abs() < 1e-12);
        assert!((s.part_cda_m2[1] - 19.2).abs() < 1e-12);
        s.shielded[1] = true;
        let out = s.aerodynamics_in_flow(0, flow);
        assert!((out.drag_kn.y - 0.0048).abs() < 1e-12);
        s.rigid.remove_parts(&[0]);
        let out = s.aerodynamics_in_flow(0, flow);
        assert_eq!(out.drag_kn, DVec3::ZERO);
        assert_eq!(out.torque_knm, DVec3::ZERO);
        s.shielded[1] = false;
        let out = s.aerodynamics_in_flow(
            0,
            Airflow {
                density: 0.0,
                ..flow
            },
        );
        assert_eq!(out.drag_kn, DVec3::ZERO);
        assert_eq!(out.torque_knm, DVec3::ZERO);
    }

    #[test]
    fn angular_airflow_respects_rolled_frames_and_current_fuel_mass_centre() {
        let (mut s, mut p) = spinning_airflow_fixture();
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(0.0, 0.0, 1.0),
            density: 1.0,
            speed_of_sound: 100.0,
        };
        let first = s.aerodynamics_in_flow(0, flow);
        p.roll_rad = std::f64::consts::FRAC_PI_2;
        let rolled = s.aerodynamics_in_flow(
            0,
            Airflow {
                frame: crate::flight::vessel_basis(&p),
                angular_velocity: DVec3::new(1.0, 0.0, 0.0),
                ..flow
            },
        );
        assert!((rolled.torque_knm.length() - first.torque_knm.length()).abs() < 1e-12);
        assert!(rolled.torque_knm.x < 0.0);
        rcs_tank(&mut s, 0, "SolidFuel", 2.0);
        let centre = s.mass_properties().centre_m;
        assert!((centre.x - 1.0).abs() < 1e-12);
        let out = s.aerodynamics_in_flow(0, flow);
        // Lever arms -1 and +3 m give local speeds 1 and 3 m/s, respectively.
        assert!((out.drag_kn.y + 0.5 * 4.8 * 8.0 / 1000.0).abs() < 1e-12);
        assert!((out.torque_knm.z + 0.5 * 4.8 * 28.0 / 1000.0).abs() < 1e-12);
        // With asymmetric levers the local drag has a net force even though bulk airspeed
        // is zero. Flight must apply that vector exactly once rather than reconstruct bulk drag.
        p.roll_rad = 0.0;
        p.angvel = flow.angular_velocity;
        p.aero_torque_knm = out.torque_knm;
        s.sync(&mut p);
        let mut body = test_body();
        body.mu = 0.0;
        let engine = crate::flight::Propulsion {
            mass: s.mass_properties(),
            actuators: Some(Vec::new()),
            aerodynamic_drag_kn: Some(out.drag_kn),
            ..Default::default()
        };
        let step = crate::flight::integrate_with_propulsion(
            &mut p,
            &body,
            None,
            None,
            0.0,
            0.02,
            Some(engine),
        );
        assert!((p.v - out.drag_kn * (0.02 / p.mass_t)).length() < 1e-12);
        assert!((step.drag_kn - out.drag_kn).length() < 1e-12);
    }

    #[test]
    fn co_rotating_atmosphere_does_not_damp_a_co_rotating_vessel() {
        let (mut s, mut p) = spinning_airflow_fixture();
        let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
            "../../../assets/own/kerbol-system.json"
        ))
        .unwrap();
        let mut u = crate::Universe::with_bodies(&bodies);
        let body = u.body_entity("Kerbin").unwrap();
        let omega = std::f64::consts::TAU
            / u.world
                .get::<crate::bodies::Body>(body)
                .unwrap()
                .rotation_period_s;
        p.r.x = 601_000.0;
        p.angvel = DVec3::new(0.0, 0.0, omega);
        p.v = p.angvel.cross(p.r);
        let mut parked = p.clone();
        parked.landed = true;
        parked.angvel = DVec3::ZERO;
        let supported_flow = Airflow::for_vessel(&parked, DVec3::new(0.0, 0.0, omega), 1.0, 340.0);
        assert_eq!(supported_flow.velocity, DVec3::ZERO);
        assert_eq!(supported_flow.angular_velocity, DVec3::ZERO);
        s.sync(&mut p);
        let vessel = u.spawn_vessel("Co-rotating airflow", body, p.r, p.v);
        u.set_powered(vessel, p);
        u.set_structure(vessel, s);
        u.step();
        let after = u.powered(vessel).unwrap();
        assert!(after.aero_torque_knm.length() < 1e-15);
        assert!(
            u.structure(vessel).unwrap().resolved_aero_forces.is_none(),
            "joint loads are consumed in the same tick"
        );
    }

    #[test]
    fn perpendicular_lift_projects_only_lift_and_internal_drag_remains_independent() {
        let (mut s, p, wing) = airflow_fixture();
        let frame = crate::flight::vessel_basis(&p);
        let velocity = DVec3::new(0.0, 100.0, -10.0);
        let travel = velocity.normalized();
        let original = s.lift(frame, velocity, 1.0, 0.3);
        s.lift_defs[wing].as_mut().unwrap().0.perpendicular_only = true;
        let perpendicular = s.lift(frame, velocity, 1.0, 0.3);
        assert!((perpendicular - (original - travel * original.dot(travel))).length() < 1e-9);
        assert!(perpendicular.dot(travel).abs() < 1e-9);
        s.lift_defs[wing].as_mut().unwrap().1.drag.keys[0].v = 1.0;
        let with_drag = s.lift(frame, velocity, 1.0, 0.3);
        assert!(
            (with_drag.dot(travel) + 0.5 * velocity.length_squared() * aero::LIFT_DRAG_MULTIPLIER)
                .abs()
                < 1e-9
        );
        s.lift_defs[wing].as_mut().unwrap().0.use_internal_drag = false;
        assert!((s.lift(frame, velocity, 1.0, 0.3) - perpendicular).length() < 1e-9);
    }

    #[test]
    fn spinning_surfaces_share_local_flow_with_control_authority_and_wing_effects() {
        let (mut s, mut p, wing) = airflow_fixture();
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(3.0, 5.0, 7.0),
            density: 1.0,
            speed_of_sound: 100.0,
        };
        let velocity = flow.velocity_at(s.part_box(wing).0 - s.mass_properties().centre_m);
        let travel = velocity.normalized();
        let expected = s.lifting_force(
            wing,
            flow.frame,
            travel,
            0.5 * velocity.length_squared(),
            flow.mach(velocity.length()),
        );
        assert!(expected.length() > 0.01);
        let forces = s.aerodynamics_in_flow(0, flow);
        assert!((forces.lift_kn - expected).length() < 1e-10);
        let fx = s.lifting_fx_in_flow(flow);
        let perpendicular = expected - travel * expected.dot(travel);
        assert!((fx[0].2 as f64 - perpendicular.length()).abs() < 1e-4);
        assert!(s.control_authority_in_flow(flow) > 0.0);
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.gimbal_range_rad.fill(0.0);
        s.rcs_defs.fill(None);
        p.ignited = false;
        let env = crate::flight::EngineEnvironment {
            density: 1.0,
            ..Default::default()
        };
        let mut engine = s.propulsion(&p, env, 0.02);
        s.configure_actuators(&p, env, &mut engine);
        assert!(
            engine
                .actuators
                .unwrap()
                .iter()
                .any(|a| a.force_kn.length() > 0.0),
            "a stationary mass centre must not disable a spinning control surface"
        );
        s.shielded[wing] = true;
        assert!(s.lifting_fx_in_flow(flow).is_empty());
        assert_eq!(s.control_authority_in_flow(flow), 0.0);
    }

    #[test]
    fn spinning_heat_uses_local_mach_direction_and_shielding() {
        let (mut s, p) = spinning_airflow_fixture();
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(0.0, 0.0, 10.0),
            density: 1.0,
            speed_of_sound: 1.0,
        };
        s.shielded[1] = true;
        s.update_heat_in_flow(flow, 288.0, 0.0, &thermal::HeatSettings::default(), 0.02);
        let expected = thermal::reentry_flux(20.0, 1.0, 1.0, 1.0, 1.0);
        assert!(expected > 0.0);
        assert!((s.reentry_flux_kw[0] - expected).abs() < 1e-8);
        assert_eq!(s.reentry_flux_kw[1], 0.0);
        let initial = s.reentry_flux_kw[0];
        let mut rolled = flow;
        rolled.frame = (flow.frame.2, flow.frame.1, -flow.frame.0);
        rolled.angular_velocity = flow.frame.0 * -10.0;
        s.update_heat_in_flow(rolled, 288.0, 0.0, &thermal::HeatSettings::default(), 0.02);
        assert!((s.reentry_flux_kw[0] - initial).abs() < 1e-8);
    }

    #[test]
    fn banking_rotates_lift_and_matches_the_wing_effect_readout() {
        let (mut s, mut p, wing) = airflow_fixture();
        let air = DVec3::new(0.0, 100.0, -10.0);
        let expected = 0.5 * air.length_squared() * aero::LIFT_MULTIPLIER * 10.0 / air.length();
        let level = s.lift(crate::flight::vessel_basis(&p), air, 1.0, 0.3);
        assert!((level - DVec3::new(0.0, 0.0, expected)).length() < 1e-9);
        p.roll_rad = std::f64::consts::FRAC_PI_4;
        let frame = crate::flight::vessel_basis(&p);
        let banked = s.lift(frame, air, 1.0, 0.3);
        assert!((banked - DVec3::new(expected * 0.5, 0.0, expected * 0.5)).length() < 1e-9);
        let fx = s.lifting_fx(frame, air, 1.0, 0.3);
        let travel = air.normalized();
        let perpendicular = (banked - travel * banked.dot(travel)).length();
        assert!((fx[0].2 as f64 - perpendicular).abs() < 1e-5);
        // Rotating the wing mount is equivalent to rotating this single wing's craft frame.
        p.roll_rad = 0.0;
        let half = -std::f64::consts::FRAC_PI_8;
        s.part_rot[wing] = [0.0, half.sin(), 0.0, half.cos()];
        assert!((s.lift(crate::flight::vessel_basis(&p), air, 1.0, 0.3) - banked).length() < 1e-9);
    }

    #[test]
    fn shielding_removes_lift_drag_moment_control_and_wing_effects_together() {
        let (mut s, p, wing) = airflow_fixture();
        let frame = crate::flight::vessel_basis(&p);
        let air = DVec3::new(0.0, 100.0, -10.0);
        assert!(s.lift(frame, air, 1.0, 0.3).length() > 1.0);
        s.faces[wing] = Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        });
        assert!(s.aerodynamics(0, frame, air, 1.0, 0.3).cda_m2 > 0.0);
        s.shielded[wing] = true;
        let out = s.aerodynamics(0, frame, air, 1.0, 0.3);
        assert_eq!(out.cda_m2, 0.0);
        assert_eq!(out.lift_kn, DVec3::ZERO);
        assert_eq!(out.torque_knm, DVec3::ZERO);
        assert!(s.lifting_fx(frame, air, 1.0, 0.3).is_empty());
        assert!(s
            .surface_deflections(frame, DVec3::new(1.0, 0.0, 0.0))
            .is_empty());
        assert_eq!(s.control_authority(air, 1.0, 0.3), 0.0);
        assert!(s
            .resolved_aero_forces
            .as_ref()
            .unwrap()
            .iter()
            .all(|f| f.force_kn == DVec3::ZERO));
    }

    #[test]
    fn aerodynamic_moment_tracks_fuel_mass_and_retains_each_parts_force() {
        let (mut s, p, wing) = airflow_fixture();
        let frame = crate::flight::vessel_basis(&p);
        let air = DVec3::new(0.0, 100.0, -10.0);
        let first = s.aerodynamics(0, frame, air, 1.0, 0.3);
        let c0 = s.mass_properties().centre_m;
        let loads = s.resolved_aero_forces.as_ref().unwrap();
        let force = loads.iter().find(|f| f.part == wing).unwrap();
        assert!((force.force_kn - first.lift_kn).length() < 1e-10);
        assert!(
            (first.torque_knm - (s.part_box(wing).0 - c0).cross(first.lift_kn)).length() < 1e-9
        );
        assert!(loads
            .iter()
            .filter(|f| f.part != wing)
            .all(|f| f.force_kn == DVec3::ZERO));
        for tank in &mut s.net.tanks {
            tank.stored = 0.0;
        }
        let c1 = s.mass_properties().centre_m;
        let second = s.aerodynamics(0, frame, air, 1.0, 0.3);
        assert!((c1 - c0).length() > 0.01);
        assert!(
            (second.torque_knm - first.torque_knm - (c0 - c1).cross(first.lift_kn)).length() < 1e-9
        );
        s.apply(
            &p,
            &StepOutcome {
                drag_kn: second.lift_kn,
                ..Default::default()
            },
            0.02,
        );
        assert!(
            s.resolved_aero_forces.is_none(),
            "a force sample is consumed exactly once"
        );
    }

    #[test]
    fn rolling_changes_anisotropic_drag_and_rotating_the_airflow_preserves_it() {
        let (mut s, mut p, wing) = airflow_fixture();
        s.lift_defs.fill(None);
        s.faces[wing] = Some(Faces {
            area: [8.0, 8.0, 1.0, 1.0, 2.0, 2.0],
            drag: [1.0; 6],
        });
        let air = DVec3::new(100.0, 0.0, 0.0);
        let face_x = s.aero(0, crate::flight::vessel_basis(&p), air, 1.0, 0.3);
        p.roll_rad = std::f64::consts::FRAC_PI_2;
        let frame = crate::flight::vessel_basis(&p);
        let face_z = s.aero(0, frame, air, 1.0, 0.3);
        assert!(
            face_x > face_z * 2.0,
            "broad face {face_x}, narrow face {face_z}"
        );
        let rotated = s.aerodynamics(0, frame, DVec3::new(0.0, 0.0, -100.0), 1.0, 0.3);
        assert!((rotated.cda_m2 - face_x).abs() < 1e-9);
        let total: DVec3 = s
            .resolved_aero_forces
            .as_ref()
            .unwrap()
            .iter()
            .fold(DVec3::ZERO, |sum, f| sum + f.force_kn);
        assert!((total - DVec3::new(0.0, 0.0, 5.0 * face_x)).length() < 1e-9);
    }

    #[test]
    fn rolled_heating_matches_the_same_airflow_in_craft_coordinates() {
        let (mut a, mut p, _) = airflow_fixture();
        let (mut b, _, _) = airflow_fixture();
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        a.faces = Structure::build(craft, &defs, &res).faces.clone();
        b.faces = a.faces.clone();
        let settings = thermal::HeatSettings::default();
        a.update_heat(
            crate::flight::vessel_basis(&p),
            DVec3::new(1200.0, 100.0, 200.0),
            0.02,
            4.0,
            220.0,
            0.02,
            &settings,
            0.02,
        );
        p.roll_rad = std::f64::consts::FRAC_PI_2;
        b.update_heat(
            crate::flight::vessel_basis(&p),
            DVec3::new(200.0, 100.0, -1200.0),
            0.02,
            4.0,
            220.0,
            0.02,
            &settings,
            0.02,
        );
        assert!(a.reentry_flux_kw.iter().any(|f| *f > 0.0));
        for (a, b) in a.reentry_flux_kw.iter().zip(&b.reentry_flux_kw) {
            assert!((a - b).abs() < 1e-6);
        }
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
            s.lift(basis(facing), travel * 80.0, 1.2, 0.24)
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
        assert_eq!(s.lift(basis(facing), nose * 80.0, 0.0, 0.0), DVec3::ZERO);
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
    fn the_resource_manager_totals_the_vessel_and_the_burning_stage() {
        let (crafts, defs, res) = assets();
        let c = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let s = Structure::build(c, &defs, &res);
        let all = s.resource_totals();
        let get = |v: &[(String, f64, f64)], id: &str| v.iter().find(|x| x.0 == id).cloned();
        let ec = get(&all, "ElectricCharge").expect("batteries");
        assert!(ec.1 > 0.0 && ec.1 <= ec.2);
        let methane = get(&all, "Methane").expect("methalox tanks");
        assert!((methane.1 - methane.2).abs() < 1e-9, "full at launch");
        // the first stage's engines reach methane and oxidizer, not the charge
        let stage = s.stage_resource_totals(0);
        assert!(get(&stage, "Methane").is_some() && get(&stage, "Oxidizer").is_some());
        assert!(get(&stage, "ElectricCharge").is_none(), "{stage:?}");
        assert!(get(&stage, "Methane").unwrap().2 <= methane.2);
    }

    #[test]
    fn a_fairing_shields_its_payload_until_it_is_jettisoned() {
        let (crafts, defs, res) = assets();
        // the starter rocket with a fairing between the decoupler and the pod: the pod and its parachute
        // ride inside
        let mut c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap()
            .clone();
        let pod = 0;
        let fairing = c.parts.len();
        let mut f = c.parts[1].clone();
        f.guid = "fairing".into();
        f.part_id = "fairing_1v_procedural".into();
        c.parts.push(f);
        // re-root: the decoupler holds the fairing, the fairing's top holds the pod
        c.parts[fairing].parent = Some(1);
        c.parts[fairing].own_node = Some("bottom".into());
        c.parts[fairing].parent_node = Some("top".into());
        c.parts[1].parent = None;
        c.parts[pod].parent = Some(fairing);
        c.parts[pod].own_node = Some("bottom".into());
        c.parts[pod].parent_node = Some("top".into());
        c.stages.push(vec![fairing]);
        let mut s = Structure::build(&c, &defs, &res);
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        s.sync(&mut p);
        assert_eq!(
            s.fairing_payload[fairing].as_ref().map(|v| v.len()),
            Some(2),
            "pod and chute"
        );
        assert!(s.shielded[pod] && s.shielded[4], "inside");
        let facing = DVec3::new(1.0, 0.0, 0.0);
        let sideways = DVec3::new(0.0, 200.0, 0.0);
        s.aero(0, basis(facing), sideways, 1.0, 0.6);
        assert_eq!(s.part_cda_m2[pod], 0.0, "no drag inside");
        // re-entry heat does not reach it either
        s.update_heat(
            basis(facing),
            -facing * 2_300.0,
            0.01,
            7.0,
            220.0,
            0.026,
            &thermal::HeatSettings::default(),
            0.5,
        );
        assert_eq!(s.reentry_flux_kw[pod], 0.0);
        // jettisoned: the pod is in the air again
        assert_eq!(s.deploy_fairings(), 1);
        assert_eq!(s.deploy_fairings(), 0);
        assert!(!s.shielded[pod]);
        s.aero(0, basis(facing), sideways, 1.0, 0.6);
        assert!(s.part_cda_m2[pod] > 0.0);
    }

    #[test]
    fn fins_turn_a_rocket_back_into_the_airflow() {
        let (crafts, defs, res) = assets();
        let flea = crafts.iter().find(|c| c.name == "Jumping Flea").unwrap();
        let mut bare = flea.clone();
        bare.parts.retain(|p| p.part_id != "fin_0v_procedural");
        bare.stages = vec![vec![1], vec![2]];
        // nose up, flying 10 degrees off its axis (in the plane its two fins face) at 150 m/s near the ground
        let facing = DVec3::new(0.0, 1.0, 0.0);
        let a = 10f64.to_radians();
        let travel = DVec3::new(0.0, a.cos(), a.sin());
        let restoring = |c: &Craft| {
            let mut s = Structure::build(c, &defs, &res);
            s.aero(0, basis(facing), travel * 150.0, 1.2, 0.45);
            let t = s.aero_torque(basis(facing), travel * 150.0, 1.2, 0.45);
            // positive: it turns the nose toward the direction of flight
            t.dot(facing.cross(travel).normalized())
        };
        let with_fins = restoring(flea);
        let without = restoring(&bare);
        assert!(
            with_fins > 0.0,
            "fins keep it pointing into the flow: {with_fins} kN m"
        );
        assert!(
            with_fins > without + 0.1,
            "fins {with_fins} vs none {without} kN m"
        );
    }

    #[test]
    fn control_surfaces_steer_harder_the_faster_the_plane_flies() {
        let (crafts, defs, res) = assets();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let c = level_layout(c).unwrap();
        let mut s = Structure::build(&c, &defs, &res);
        let facing = DVec3::new(0.0, 1.0, 0.0);
        let slow = s.control_authority(facing * 50.0, 1.2, 0.15);
        let fast = s.control_authority(facing * 150.0, 1.2, 0.45);
        assert!(slow > 1.0, "elevators bite at 50 m/s: {slow} kN m");
        assert!(fast > 4.0 * slow, "and more with speed: {slow} -> {fast}");
        assert_eq!(
            s.control_authority(facing * 150.0, 0.0, 0.0),
            0.0,
            "no air, no authority"
        );
        // asked to pitch, the surfaces deflect (some one way, some the other), never past their range
        let pitch = DVec3::new(1.0, 0.0, 0.0);
        let mut p = Powered::new(
            crate::flight::FlightSpec {
                stages: vec![],
                drag_area_m2: 0.,
                drag_cd: 0.,
            },
            DVec3::ZERO,
            facing * 50.,
            false,
        );
        p.control_axes = pitch;
        s.last_airflow = Some(Airflow::bulk(basis(facing), facing * 50., 1.2, 0.15));
        let propulsion = crate::flight::Propulsion {
            mass: s.mass_properties(),
            actuators: Some(vec![]),
            ..Default::default()
        };
        s.resolve_surface_geometry(&p, &propulsion, &mut Default::default(), 0.02);
        let d = s.surface_deflections(basis(facing), pitch);
        assert!(!d.is_empty());
        assert!(d.iter().any(|(_, a)| a.abs() > 0.01), "{d:?}");
        assert!(d.iter().all(|(_, a)| a.abs() <= 30f64.to_radians() + 1e-9));
        p.control_axes = DVec3::ZERO;
        for _ in 0..40 {
            s.resolve_surface_geometry(&p, &propulsion, &mut Default::default(), 0.02);
        }
        assert!(s
            .surface_deflections(basis(facing), DVec3::ZERO)
            .iter()
            .all(|(_, a)| a.abs() < 1e-9));
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
        // a bit over the legs' 20 m/s: their 0.95 m of suspension takes it
        let (broken, _) = s.impact(21.0, DVec3::new(1.0, 0.0, 0.0)).unwrap();
        assert!(broken.is_empty(), "the suspension soaks it up");
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
                let burned =
                    s.update_heat(basis(facing), v_air, rho, mach, ambient, k, &settings, 0.02);
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
            s.update_heat(
                basis(facing),
                nose_first,
                rho,
                mach,
                ambient,
                k,
                &settings,
                0.02,
            );
        }
        assert!(
            s.temperature_k[pod] > 400.0 && !s.ablating[shield],
            "a capsule flying nose first is not protected: pod at {} K",
            s.temperature_k[pod]
        );
        // subsonic air only cools
        let mut s = Structure::build(&bare, &defs, &res);
        s.temperature_k[0] = 600.0;
        s.update_heat(
            basis(facing),
            v_air * 0.1,
            1.0,
            0.7,
            288.0,
            k,
            &settings,
            1.0,
        );
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

    // Two independently fuelled engines with the same vacuum thrust but different Isp.
    // Their mass flow is analytically 1 and 0.5 t/s, rather than the old equal thrust split.
    fn propulsion_fixture() -> (Structure, Powered, [usize; 2]) {
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(craft, &defs, &res);
        s.geometry.fill(Default::default());
        let engines: Vec<_> = s
            .live_engines(0)
            .into_iter()
            .filter(|&i| s.engine_mixture[i].as_deref() == Some("SolidFuel"))
            .take(2)
            .collect();
        assert_eq!(engines.len(), 2);
        let pair = [engines[0], engines[1]];
        s.stage_engines[0] = engines;
        for (part, isp) in [(pair[0], 100.0), (pair[1], 200.0)] {
            s.engine_thrust_kn[part] = 100.0 * ose_data::G0;
            s.engine_isp[part] = Some(ose_data::Curve {
                keys: vec![ose_data::CurveKey {
                    t: 0.0,
                    v: isp,
                    in_tangent: 0.0,
                    out_tangent: 0.0,
                }],
            });
            s.engine_throttle_locked[part] = false;
        }
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        p.throttle = 1.0;
        s.sync(&mut p);
        (s, p, pair)
    }

    fn rcs_fixture() -> (Structure, Powered, usize) {
        let (mut s, mut p, other) = wheel_power_fixture();
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.electric.fill(Default::default());
        s.net.tanks.clear();
        s.rcs_defs[s.root] = Some(ose_data::Rcs {
            thrust_kn: 100.0 * ose_data::G0,
            propellant: "SolidFuel".into(),
            isp_curve: ose_data::Curve {
                keys: vec![ose_data::CurveKey {
                    v: 100.0,
                    ..Default::default()
                }],
            },
        });
        p.rotation_input = DVec3::ZERO;
        p.groups.rcs = true;
        p.rcs.translate = DVec3::new(0.0, 1.0, 0.0);
        p.rcs.powered = 0.0; // Current fuel delivery must replace stale availability.
        (s, p, other)
    }

    fn rcs_tank(s: &mut Structure, part: usize, name: &str, mass_t: f64) {
        let resource = s.net.resource(name).unwrap();
        s.net.flow_mode[resource] = ose_data::FlowMode::NoFlow;
        s.net.tanks.push(crate::resources::Tank {
            part,
            resource,
            stored: mass_t / s.net.unit_mass_t[resource],
            capacity: 10000.0,
        });
    }

    #[test]
    fn control_frame_composes_authored_node_and_mount_and_resets_when_detached() {
        let (mut s, mut p, pair) = propulsion_fixture();
        let i = pair[0];
        s.control_sources[i] = true;
        let turn = glam::DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array();
        s.part_rot[i] = turn;
        s.geometry[i].control_rotation = Some(turn);
        let rotation = s.control_rotation(i);
        assert!(
            (aero::rotate(rotation, DVec3::new(0., 1., 0.)) - DVec3::new(0., -1., 0.)).length()
                < 1e-10
        );
        p.control_point = Some(i as u16);
        p.control_rotation = rotation;
        s.rigid.present[i] = false;
        assert!(!s.can_control_from(i));
        s.sync(&mut p);
        assert_eq!(p.control_point, None);
        assert_eq!(p.control_rotation, [0., 0., 0., 1.]);
    }

    #[test]
    fn authored_gimbal_moves_nozzle_about_pivot_with_limited_asymmetric_response() {
        use ose_data::flight_geometry::{Gimbal, Nozzle, PartControl};
        let (mut s, mut p, pair) = propulsion_fixture();
        let i = pair[0];
        s.part_rot[i] = [0., 0., 0., 1.];
        s.geometry[i].engines = vec![vec![Nozzle {
            position_m: DVec3::new(0., -1., 0.),
            direction: DVec3::new(0., 1., 0.),
            weight: 1.,
            gimbal: Some(0),
        }]];
        s.geometry[i].gimbals = vec![Gimbal {
            position_m: DVec3::ZERO,
            rotation: [0., 0., 0., 1.],
            limits_rad: [0.2, 0.4, 0.1, 0.3],
            response_speed: Some(10.),
            axis_mult: [1., 1.],
        }];
        s.gimbal_angles[i] = vec![[0., 0.]];
        p.control_axes = DVec3::new(1., 0., 0.);
        p.part_controls = vec![(
            i as u16,
            PartControl {
                gimbal_limiter: 0.5,
                ..Default::default()
            },
        )];
        let mut thrust = vec![0.; s.rigid.len()];
        thrust[i] = 100.;
        s.resolved_engine_thrust = Some(thrust);
        s.resolved_nozzle_forces = s.engine_forces(i, 100.);
        let prop = crate::flight::Propulsion {
            mass: crate::dynamics::MassProperties {
                centre_m: s.rigid.rest_position(i) + DVec3::new(0., 2., 0.),
                ..Default::default()
            },
            actuators: Some(vec![]),
            ..Default::default()
        };
        let mut allocation = crate::actuators::Allocation::default();
        s.resolve_authored_gimbals(&p, &prop, &mut allocation, 0.02);
        assert!((s.gimbal_angles[i][0][0] - 0.02).abs() < 1e-12);
        let force = s.engine_forces(i, 100.)[0];
        assert!(
            (force.force_kn - DVec3::new(0., 100. * 0.02_f64.cos(), -100. * 0.02_f64.sin()))
                .length()
                < 1e-10
        );
        assert!(
            (force.offset_m - DVec3::new(0., -0.02_f64.cos(), 0.02_f64.sin())).length() < 1e-10
        );
        assert!((force.force_kn.length() - 100.).abs() < 1e-10);
        assert!(
            (allocation.force_kn - (force.force_kn - DVec3::new(0., 100., 0.))).length() < 1e-10
        );
        p.control_axes = -p.control_axes;
        s.resolve_authored_gimbals(&p, &prop, &mut Default::default(), 0.02);
        assert!((s.gimbal_angles[i][0][0] - (-0.024)).abs() < 1e-12);
    }

    #[test]
    fn authored_nozzle_partial_fuel_limits_force_moment_and_consumes_only_once() {
        use ose_data::flight_geometry::Nozzle;
        for fraction in [0., 0.25, 1.] {
            let (mut s, mut p, _) = rcs_fixture();
            let i = s.root;
            let dt = 0.02;
            s.geometry[i].rcs = vec![Nozzle {
                position_m: DVec3::new(1., 0., 0.),
                direction: DVec3::new(0., 1., 0.),
                weight: 1.,
                gimbal: None,
            }];
            rcs_tank(&mut s, i, "SolidFuel", dt * fraction);
            let before = s.net.mass_t();
            let mut prop = crate::flight::Propulsion {
                mass: s.mass_properties(),
                ..Default::default()
            };
            s.configure_actuators(&p, Default::default(), &mut prop);
            let mut allocation =
                crate::actuators::allocate(prop.actuators.as_ref().unwrap(), DVec3::ZERO);
            s.resolve_rcs(&mut p, &mut prop, &mut allocation, dt);
            assert!(
                (allocation.force_kn.length() - 100. * ose_data::G0 * fraction).abs() < 1e-5,
                "{fraction}: {allocation:?}"
            );
            assert!(((before - s.net.mass_t()) - dt * fraction).abs() < 1e-9);
            let force = allocation.force_kn;
            let lever = s.part_point(i, DVec3::new(1., 0., 0.)) - prop.mass.centre_m;
            assert!((allocation.torque_knm - lever.cross(force)).length() < 1e-6);
        }
    }

    fn test_body() -> crate::bodies::Body {
        let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
            "../../../assets/own/kerbol-system.json"
        ))
        .unwrap();
        let u = crate::Universe::with_bodies(&bodies);
        u.world
            .get::<crate::bodies::Body>(u.body_entity("Kerbin").unwrap())
            .unwrap()
            .clone()
    }

    #[test]
    fn rcs_empty_and_final_partial_fuel_limit_same_tick_universe_impulse() {
        for fraction in [0.0, 0.25, 1.0] {
            let (mut s, mut p, _) = rcs_fixture();
            let dt = 1.0 / crate::TICKS_PER_SECOND as f64;
            let root = s.root;
            rcs_tank(&mut s, root, "SolidFuel", dt * fraction);
            p.facing = DVec3::new(1.0, 0.0, 0.0);
            s.sync(&mut p);
            let mass = p.mass_t;
            let radius = p.r.length();
            let body = test_body();
            let mut u = crate::Universe::with_bodies(
                &serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
                    "../../../assets/own/kerbol-system.json"
                ))
                .unwrap(),
            );
            let vessel = u.spawn_vessel(
                "Fuel-limited RCS",
                u.body_entity("Kerbin").unwrap(),
                p.r,
                p.v,
            );
            u.set_powered(vessel, p);
            u.set_structure(vessel, s);
            u.step();
            let after = u.powered(vessel).unwrap();
            let expected =
                100.0 * ose_data::G0 * dt * fraction / mass - body.mu / radius.powi(2) * dt;
            assert!(
                (after.v.x - expected).abs() < 1e-8,
                "fraction={fraction}: {} vs {expected}",
                after.v.x
            );
            assert!((after.mass_t - (mass - dt * fraction)).abs() < 1e-9);
            assert!((after.mass_t - u.structure(vessel).unwrap().net.mass_t()).abs() < 1e-9);
            if fraction > 0.0 {
                assert!(u.structure(vessel).unwrap().rigid.links.iter().any(|l| l.force_kn > 0.1),
                    "pure translation must load joints even without engine thrust or angular motion");
            }
            u.step();
            assert_eq!(u.powered(vessel).unwrap().rcs.usage, 0.0);
            assert!((u.powered(vessel).unwrap().mass_t - (mass - dt * fraction)).abs() < 1e-9);
        }
    }

    #[test]
    fn rcs_mounts_use_independent_mixtures_isp_curves_and_local_resource_access() {
        let (mut s, mut p, other) = rcs_fixture();
        let root = s.root;
        let mut def = s.rcs_defs[root].clone().unwrap();
        def.isp_curve.keys.push(ose_data::CurveKey {
            t: 5.0,
            v: 20.0,
            ..Default::default()
        });
        s.rcs_defs[root] = Some(def.clone());
        def.propellant = "MonoPropellant".into();
        def.isp_curve.keys[0].v = 200.0;
        def.isp_curve.keys[1].v = 100.0;
        s.rcs_defs[other] = Some(def);
        s.part_rot[other] = [0.0, 0.0, 0.0, 1.0];
        rcs_tank(&mut s, root, "SolidFuel", 1.0);
        rcs_tank(&mut s, other, "MonoPropellant", 1.0);
        s.sync(&mut p);
        let env = crate::flight::EngineEnvironment {
            pressure_atm: 5.0,
            ..Default::default()
        };
        let mut engine = s.propulsion(&p, env, 0.1);
        s.configure_actuators(&p, env, &mut engine);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.1);
        assert!((engine.burned_t - 0.15).abs() < 1e-10);
        let force = engine.control_allocation.unwrap().force_kn;
        assert!((force.y - 70.0 * ose_data::G0).abs() < 1e-8);
        assert!(
            (s.net.tanks[0].stored * s.net.unit_mass_t[s.net.tanks[0].resource] - 0.9).abs()
                < 1e-10
        );
        assert!(
            (s.net.tanks[1].stored * s.net.unit_mass_t[s.net.tanks[1].resource] - 0.95).abs()
                < 1e-10
        );
        // Fuel at the wrong mount cannot power a NoFlow thruster.
        s.net.tanks[0].part = other;
        let mut engine = s.propulsion(&p, env, 0.1);
        s.configure_actuators(&p, env, &mut engine);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.1);
        assert!((engine.burned_t - 0.05).abs() < 1e-10);
        assert!((engine.control_allocation.unwrap().force_kn.y - 50.0 * ose_data::G0).abs() < 1e-8);
        s.rigid.remove_parts(&[other]);
        let mut engine = s.propulsion(&p, env, 0.1);
        s.configure_actuators(&p, env, &mut engine);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.1);
        assert_eq!(engine.burned_t, 0.0);
        assert!(engine.control_allocation.unwrap().force_kn.length() < 1e-10);
    }

    #[test]
    fn rcs_partial_recipe_scales_combined_force_and_torque_without_draining_twice() {
        let (mut s, mut p, _) = rcs_fixture();
        let root = s.root;
        s.rcs_defs[root].as_mut().unwrap().propellant = "Methalox".into();
        // A full request needs 0.02 t methane and 0.08 t oxidizer.
        rcs_tank(&mut s, root, "Methane", 0.005);
        rcs_tank(&mut s, root, "Oxidizer", 1.0);
        s.sync(&mut p);
        p.rotation_input = DVec3::new(1.0, 0.0, 0.0);
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.1);
        let out = engine.control_allocation.unwrap();
        assert!((engine.burned_t - 0.025).abs() < 1e-10);
        assert!((out.force_kn.length() - 25.0 * ose_data::G0).abs() < 1e-8);
        assert!(out.force_kn.y > 0.0 && out.torque_knm.length() > 0.0);
        assert!(
            (out.torque_knm
                - (s.rigid.rest_position(root) - engine.mass.centre_m).cross(out.force_kn))
            .length()
                < 1e-8
        );
        assert!(s.net.tanks[0].stored < 1e-10);
        assert!((s.net.tanks[1].stored - 0.98).abs() < 1e-10);
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.1);
        let out = engine.control_allocation.unwrap();
        assert_eq!(engine.burned_t, 0.0);
        assert_eq!(out.force_kn, DVec3::ZERO);
        assert_eq!(out.torque_knm, DVec3::ZERO);
        assert!((s.net.tanks[1].stored - 0.98).abs() < 1e-10);
    }

    #[test]
    fn rcs_rotation_and_translation_share_one_mount_force_and_fuel_budget() {
        let (mut s, mut p, _) = rcs_fixture();
        let root = s.root;
        rcs_tank(&mut s, root, "SolidFuel", 1.0);
        s.sync(&mut p);
        p.rotation_input = DVec3::new(1.0, 0.0, 0.0);
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        let rotation = crate::flight::plan_actuators(&mut p, &test_body(), &engine, 0.1);
        assert!(rotation.torque_knm.length() > 0.0);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.1);
        let out = engine.control_allocation.unwrap();
        assert!((out.force_kn.length() - 100.0 * ose_data::G0).abs() < 1e-8);
        assert!(out.force_kn.y > 0.0 && out.torque_knm.length() > 0.0);
        assert!(
            (out.torque_knm
                - (s.rigid.rest_position(root) - engine.mass.centre_m).cross(out.force_kn))
            .length()
                < 1e-8
        );
        assert!((engine.burned_t - 0.1).abs() < 1e-10);
        // Opposing translation and rotation can cancel without a residual free force.
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        let mut allocation = crate::flight::plan_actuators(&mut p, &test_body(), &engine, 0.1);
        p.rcs.translate = -allocation.force_kn * (1.0 / (100.0 * ose_data::G0));
        s.resolve_rcs(&mut p, &mut engine, &mut allocation, 0.1);
        assert!(allocation.force_kn.length() < 1e-8);
        assert!(engine.burned_t < 1e-10);
    }

    #[test]
    fn wheel_charge_limits_the_first_tick_and_last_partial_angular_impulse() {
        for fraction in [0.0, 0.25, 1.0] {
            let (mut s, mut p, _) = wheel_power_fixture();
            let dt = 1.0 / crate::TICKS_PER_SECOND as f64;
            let ec = s.net.resource("ElectricCharge").unwrap();
            s.net.tanks.push(crate::resources::Tank {
                part: s.root,
                resource: ec,
                stored: 6.0 * dt * 0.6 * fraction,
                capacity: 10.0,
            });
            let initial = s.mass_properties();
            let bodies: Vec<ose_data::CelestialBody> =
                serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json"))
                    .unwrap();
            let mut u = crate::Universe::with_bodies(&bodies);
            let body = u.body_entity("Kerbin").unwrap();
            let vessel = u.spawn_vessel("Charge-limited wheel", body, p.r, p.v);
            p.wheel_scale = 1.0; // stale readout must not grant free torque
            u.set_powered(vessel, p);
            u.set_structure(vessel, s);
            u.step();
            let p = u.powered(vessel).unwrap();
            let (x, y, z) = crate::flight::vessel_basis(p);
            let w = p.angvel + p.facing * p.roll_rate;
            let momentum = initial
                .inertia
                .apply(DVec3::new(w.dot(x), w.dot(y), w.dot(z)));
            assert!(
                (momentum.y - 2.0 * dt * 0.6 * fraction).abs() < 1e-9,
                "fraction={fraction}: {momentum:?}"
            );
            assert!(ec_stored(u.structure(vessel).unwrap()) < 1e-10);
            // Empty on the following tick: no second impulse and no second charge draw.
            let rate = p.roll_rate;
            u.step();
            assert!((u.powered(vessel).unwrap().roll_rate - rate).abs() < 1e-8);
            assert!(ec_stored(u.structure(vessel).unwrap()) < 1e-10);
        }
    }

    fn wheel_power_fixture() -> (Structure, Powered, usize) {
        let (mut s, mut p, pair) = propulsion_fixture();
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.wheel_axes_knm[s.root] = DVec3::new(0.0, 2.0, 0.0);
        s.gimbal_range_rad.fill(0.0);
        s.ctrl.fill((0.0, 0.0));
        s.rcs_defs.fill(None);
        s.electric.fill(Default::default());
        s.electric[s.root].wheel_use_per_knm = 3.0;
        s.electric[s.root].wheel_torque_vec_knm = 2.0;
        s.crew[s.root] = 1;
        s.part_rot[s.root] = [0.0, 0.0, 0.0, 1.0];
        let ec = s.net.resource("ElectricCharge").unwrap();
        s.net.tanks.retain(|t| t.resource != ec);
        s.net.flow_mode[ec] = ose_data::FlowMode::NoFlow;
        p.ignited = false;
        p.throttle = 0.0;
        p.attitude = crate::flight::Attitude::Free;
        p.rotation_input = DVec3::new(0.0, 0.0, 1.0);
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        p.v = DVec3::new(0.0, 100.0, 0.0);
        (s, p, pair[0])
    }

    #[test]
    fn powered_wheels_share_commands_instead_of_competing_for_allocation() {
        let (mut s, mut p, other) = wheel_power_fixture();
        s.wheel_axes_knm[other] = DVec3::new(0.0, 2.0, 0.0);
        s.electric[other] = s.electric[s.root];
        let ec = s.net.resource("ElectricCharge").unwrap();
        for part in [s.root, other] {
            s.net.tanks.push(crate::resources::Tank {
                part,
                resource: ec,
                stored: 10.0,
                capacity: 10.0,
            });
        }
        p.rotation_input.z = 0.5;
        p.wheel_scale = 1.0;
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &test_body(), &mut engine, 0.02);
        let allocation = engine.control_allocation.unwrap();
        assert!((allocation.torque_knm.y - 1.2).abs() < 1e-10);
        for (a, input) in engine.actuators.unwrap().iter().zip(allocation.inputs) {
            if a.force_kn.length_squared() == 0.0 && a.axial.is_none() {
                assert!((input - 0.3).abs() < 1e-10);
            }
        }
        assert!((ec_stored(&s) - (20.0 - 2.0 * 0.6 * 3.0 * 0.02)).abs() < 1e-10);
    }

    #[test]
    fn wheel_power_is_delivered_at_each_mount_and_recovers_without_tick_lag() {
        let (mut s, mut p, other) = wheel_power_fixture();
        s.wheel_axes_knm[other] = DVec3::new(0.0, 2.0, 0.0);
        s.part_rot[other] = [0.0, 0.0, 0.0, 1.0];
        s.electric[other] = s.electric[s.root];
        let ec = s.net.resource("ElectricCharge").unwrap();
        s.net.tanks.push(crate::resources::Tank {
            part: other,
            resource: ec,
            stored: 10.0,
            capacity: 10.0,
        });
        let u = crate::Universe::with_bodies(
            &serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
                "../../../assets/own/kerbol-system.json"
            ))
            .unwrap(),
        );
        let body = u.body_entity("Kerbin").unwrap();
        let body = u.world.get::<crate::bodies::Body>(body).unwrap().clone();
        s.sync(&mut p);
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        let allocation = engine.control_allocation.as_ref().unwrap();
        assert!((allocation.torque_knm.y - 1.2).abs() < 1e-9);
        let channels = engine.actuators.as_ref().unwrap();
        for (a, input) in channels.iter().zip(&allocation.inputs) {
            if a.part == s.root {
                assert_eq!(*input, 0.0);
            }
            if a.part == other {
                assert!((*input - 0.6).abs() < 1e-9);
            }
        }
        assert!((ec_stored(&s) - 9.928).abs() < 1e-9);
        // The dry wheel must also leave allocation room for a non-electric force channel.
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        engine.actuators = Some(vec![
            crate::actuators::Actuator {
                positive_only: false,
                part: s.root,
                torque_knm: DVec3::new(0.0, 2.0, 0.0),
                force_kn: DVec3::ZERO,
                offset_m: DVec3::ZERO,
                group: 0,
                axial: None,
            },
            crate::actuators::Actuator {
                positive_only: false,
                part: other,
                torque_knm: DVec3::new(0.0, 2.0, 0.0),
                force_kn: DVec3::new(2.0, 0.0, 0.0),
                offset_m: DVec3::ZERO,
                group: 1,
                axial: None,
            },
        ]);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        let allocation = engine.control_allocation.unwrap();
        assert_eq!(allocation.inputs[0], 0.0);
        assert!((allocation.inputs[1] - 1.0).abs() < 1e-9);
        assert!((allocation.torque_knm.y - 2.0).abs() < 1e-9);
        assert!(
            (ec_stored(&s) - 9.928).abs() < 1e-9,
            "force channel draws no wheel charge"
        );
        // A generator supplies an empty wheel in this tick even if the last readout was zero.
        s.rigid.remove_parts(&[other]);
        s.net.remove_parts(&[other]);
        s.net.tanks.push(crate::resources::Tank {
            part: s.root,
            resource: ec,
            stored: 0.0,
            capacity: 10.0,
        });
        s.electric[s.root].generator_output = 6.0;
        s.engine_thrust_kn[s.root] = 0.0;
        s.electric_services(&mut p, 0.0, 0.02);
        p.wheel_scale = 1.0;
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        assert!((engine.control_allocation.unwrap().torque_knm.y - 1.872).abs() < 1e-9);
        assert!((ec_stored(&s) - 0.00768).abs() < 1e-9);
    }

    #[test]
    fn wheel_charge_tracks_vector_torque_and_sas_braking_but_not_idle_spin() {
        let (mut s, mut p, _) = wheel_power_fixture();
        s.wheel_axes_knm[s.root] = DVec3::new(3.0, 4.0, 0.0);
        let ec = s.net.resource("ElectricCharge").unwrap();
        s.net.tanks.push(crate::resources::Tank {
            part: s.root,
            resource: ec,
            stored: 10.0,
            capacity: 10.0,
        });
        let u = crate::Universe::with_bodies(
            &serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
                "../../../assets/own/kerbol-system.json"
            ))
            .unwrap(),
        );
        let body = u
            .world
            .get::<crate::bodies::Body>(u.body_entity("Kerbin").unwrap())
            .unwrap()
            .clone();
        s.sync(&mut p);
        p.rotation_input = DVec3::new(-0.6, 0.0, 0.8);
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        assert!(
            (engine.control_allocation.unwrap().torque_knm - DVec3::new(-1.08, 1.92, 0.0)).length()
                < 1e-9
        );
        assert!(
            (ec_stored(&s) - (10.0 - 3.0 * (1.08 + 1.92) * 0.02)).abs() < 1e-9,
            "manual axes independently scale their rated torques"
        );
        // A settled idle wheel has no charge demand merely because the hull is spinning.
        // Residual commanded torque during the response ramp still consumes charge.
        s.wheel_actuation_knm.fill(DVec3::ZERO);
        p.rotation_input = DVec3::ZERO;
        p.roll_rate = 0.1;
        let before = ec_stored(&s);
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        assert_eq!(ec_stored(&s), before, "free spin draws no wheel charge");
        p.attitude = crate::flight::Attitude::Fixed(p.facing);
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        assert_eq!(engine.control_allocation.unwrap().torque_knm.y, 0.0);
        p.roll_rad += 0.002;
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_actuator_resources(&mut p, &body, &mut engine, 0.02);
        assert!(engine.control_allocation.unwrap().torque_knm.y < 0.0);
        assert!(ec_stored(&s) < before, "SAS braking draws charge");
    }

    #[test]
    fn wheel_axes_use_control_frame_and_obey_power_and_attachment() {
        let (mut s, mut p, pair) = propulsion_fixture();
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.gimbal_range_rad.fill(0.0);
        s.ctrl.fill((0.0, 0.0));
        s.wheel_axes_knm[pair[0]] = DVec3::new(2.0, 0.0, 0.0);
        let h = 0.5_f64.sqrt();
        s.part_rot[pair[0]] = [0.0, 0.0, h, h]; // part X -> craft Y (roll)
        p.wheel_scale = 0.25;
        p.spec.stages[0].wheel_torque_knm = 1000.0; // stale scalar must have no effect
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        let a = engine.actuators.as_ref().unwrap();
        let roll = crate::actuators::allocate(a, DVec3::new(0.0, 100.0, 0.0));
        assert_eq!(roll.torque_knm, DVec3::ZERO);
        assert!(
            (crate::actuators::allocate(a, DVec3::new(100.0, 0.0, 0.0)).torque_knm
                - DVec3::new(0.5, 0.0, 0.0))
            .length()
                < 1e-10
        );
        p.wheel_scale = 0.0;
        s.configure_actuators(&p, Default::default(), &mut engine);
        assert!(engine.actuators.as_ref().unwrap().is_empty());
        p.wheel_scale = 1.0;
        s.rigid.remove_parts(&[pair[0]]);
        s.configure_actuators(&p, Default::default(), &mut engine);
        assert!(engine.actuators.as_ref().unwrap().is_empty());
    }

    #[test]
    fn gimbal_limits_follow_delivered_thrust_including_the_final_partial_burn() {
        let (mut s, p, pair) = propulsion_fixture();
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.gimbal_range_rad.fill(0.0);
        s.ctrl.fill((0.0, 0.0));
        s.stage_engines[0] = vec![pair[0]];
        s.gimbal_range_rad[pair[0]] = 5.0_f64.to_radians();
        let solid = s.net.resource("SolidFuel").unwrap();
        for tank in &mut s.net.tanks {
            if tank.resource == solid {
                tank.stored = if tank.part == pair[0] {
                    0.005 / s.net.unit_mass_t[solid]
                } else {
                    0.0
                };
            }
        }
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        assert!((engine.thrust_kn - 0.25 * s.engine_thrust_kn[pair[0]]).abs() < 1e-8);
        s.configure_actuators(&p, Default::default(), &mut engine);
        let channels = engine.actuators.as_ref().unwrap();
        assert_eq!(channels.len(), 2);
        let side = engine.thrust_kn * 5.0_f64.to_radians().sin();
        for a in channels {
            assert!((a.force_kn.length() - side).abs() < 1e-10);
        }
        let allocation = crate::actuators::allocate(channels, DVec3::new(1000.0, 0.0, 1000.0));
        let outcome = StepOutcome {
            thrust_kn: engine.thrust_kn,
            actuator_inputs: allocation.inputs,
            actuator_axial_corrections: allocation.axial_corrections,
            ..Default::default()
        };
        s.apply(&p, &outcome, 0.02);
        let direction = s
            .engine_gimbals
            .iter()
            .find(|g| g.0 as usize == pair[0])
            .unwrap()
            .1;
        assert!((direction.length() - 1.0).abs() < 1e-10);
        assert!((direction.y.acos() - 5.0_f64.to_radians()).abs() < 1e-8);
        let mut dry = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut dry);
        assert!(
            dry.actuators.as_ref().unwrap().is_empty(),
            "dry engines cannot steer"
        );
    }

    #[test]
    fn mounted_engines_produce_balanced_and_off_axis_forces() {
        let (mut s, p, pair) = propulsion_fixture();
        // Only two equal masses contribute, so their midpoint is the analytical mass centre.
        s.net.dry_t.fill(0.0);
        for &i in &pair {
            s.net.dry_t[i] = 1.0;
            s.part_rot[i] = [0.0, 0.0, 0.0, 1.0];
            s.engine_isp[i] = s.engine_isp[pair[0]].clone();
        }
        let solid = s.net.resource("SolidFuel").unwrap();
        for t in &mut s.net.tanks {
            t.stored = if pair.contains(&t.part) && t.resource == solid {
                1.0 / s.net.unit_mass_t[solid]
            } else {
                0.0
            };
        }
        let centre = (s.rigid.rest_position(pair[0]) + s.rigid.rest_position(pair[1])) * 0.5;
        let balanced = s.propulsion(&p, Default::default(), 0.01);
        let thrust = 100.0 * ose_data::G0;
        assert!((balanced.mass.centre_m - centre).length() < 1e-10);
        assert!((balanced.force_kn - DVec3::new(0.0, 2.0 * thrust, 0.0)).length() < 1e-9);
        assert!(balanced.torque_knm.length() < 1e-9);
        // Turn one mounting 90 degrees about Z: +Y thrust becomes -X.
        let h = 0.5_f64.sqrt();
        s.part_rot[pair[0]] = [0.0, 0.0, h, h];
        let tilted = s.propulsion(&p, Default::default(), 0.01);
        let first = DVec3::new(-thrust, 0.0, 0.0);
        let second = DVec3::new(0.0, thrust, 0.0);
        let moment = (s.rigid.rest_position(pair[0]) - centre).cross(first)
            + (s.rigid.rest_position(pair[1]) - centre).cross(second);
        assert!((tilted.force_kn - first - second).length() < 1e-9);
        assert!((tilted.torque_knm - moment).length() < 1e-9);
        assert!(moment.length() > 1.0);
    }

    #[test]
    fn fuel_drain_and_detachment_update_mass_centre_and_inertia() {
        let (mut s, mut p, pair) = propulsion_fixture();
        let before = s.mass_properties();
        let burn = s.propulsion(&p, Default::default(), 0.1);
        let after = s.mass_properties();
        let expected = (before.centre_m * before.mass_t
            - s.rigid.rest_position(pair[0]) * 0.1
            - s.rigid.rest_position(pair[1]) * 0.05)
            * (1.0 / after.mass_t);
        assert!((after.mass_t - before.mass_t + burn.burned_t).abs() < 1e-9);
        assert!((after.centre_m - expected).length() < 1e-9);
        assert!(after.inertia.x.x < before.inertia.x.x);
        s.sync(&mut p);
        for i in 0..s.rigid.len() {
            if s.rigid.present[i] {
                assert!((s.rigid.masses_t[i] - s.net.mass_of(&[i]).max(0.001)).abs() < 1e-9);
            }
        }
        // This synthetic fixture includes massless parts, for which Rapier retains a 1 kg floor.
        let rigid_total: f64 = (0..s.rigid.len())
            .filter(|&i| s.rigid.present[i])
            .map(|i| s.net.mass_of(&[i]).max(0.001))
            .sum();
        let rigid_centre = (0..s.rigid.len())
            .filter(|&i| s.rigid.present[i])
            .fold(DVec3::ZERO, |c, i| {
                c + s.rigid.rest_position(i) * s.net.mass_of(&[i]).max(0.001)
            })
            * (1.0 / rigid_total);
        assert!((s.rigid.rest_centre() - rigid_centre).length() < 1e-9);
        let removed_mass = s.net.mass_of(&[pair[0]]);
        s.rigid.remove_parts(&[pair[0]]);
        s.net.remove_parts(&[pair[0]]);
        let detached = s.mass_properties();
        let expected = (after.centre_m * after.mass_t
            - s.rigid.rest_position(pair[0]) * removed_mass)
            * (1.0 / detached.mass_t);
        assert!((detached.centre_m - expected).length() < 1e-9);
        assert!((detached.mass_t - after.mass_t + removed_mass).abs() < 1e-9);
    }

    fn assert_no_engine_impulse(out: crate::flight::Propulsion) {
        assert_eq!(
            out,
            crate::flight::Propulsion {
                mass: out.mass,
                ..Default::default()
            }
        );
    }

    #[test]
    fn per_engine_propulsion_uses_individual_isp_and_the_entire_pressure_curve() {
        let (mut s, p, pair) = propulsion_fixture();
        s.engine_isp[pair[0]] = Some(ose_data::Curve {
            keys: [(0.0, 100.0), (1.0, 80.0), (5.0, 20.0)]
                .into_iter()
                .map(|(t, v)| ose_data::CurveKey {
                    t,
                    v,
                    in_tangent: 0.0,
                    out_tangent: 0.0,
                })
                .collect(),
        });
        let before: Vec<_> = pair.iter().map(|&i| s.net.mass_of(&[i])).collect();
        let env = crate::flight::EngineEnvironment {
            pressure_atm: 5.0,
            ..Default::default()
        };
        let out = s.propulsion(&p, env, 0.1);
        // Pressure reduces the first engine's thrust, not its fuel flow. The second keeps its Isp.
        assert!((out.thrust_kn - 120.0 * ose_data::G0).abs() < 1e-9);
        assert!((out.burned_t - 0.15).abs() < 1e-9);
        assert!((before[0] - s.net.mass_of(&[pair[0]]) - 0.1).abs() < 1e-9);
        assert!((before[1] - s.net.mass_of(&[pair[1]]) - 0.05).abs() < 1e-9);
        // Mid-key interpolation is Hermite, not a clamped vacuum/sea-level straight line.
        let mid = s.engine_demands(
            0,
            1.0,
            crate::flight::EngineEnvironment {
                pressure_atm: 3.0,
                ..Default::default()
            },
        );
        assert!((mid[0].thrust_kn - 50.0 * ose_data::G0).abs() < 1e-9);
    }

    #[test]
    fn propulsion_limits_the_last_impulse_to_the_fuel_delivered() {
        let (mut s, p, pair) = propulsion_fixture();
        let solid = s.net.resource("SolidFuel").unwrap();
        for tank in &mut s.net.tanks {
            if pair.contains(&tank.part) && tank.resource == solid {
                tank.stored =
                    if tank.part == pair[0] { 0.025 } else { 0.0 } / s.net.unit_mass_t[solid];
            }
        }
        let out = s.propulsion(&p, Default::default(), 0.1);
        assert!((out.burned_t - 0.025).abs() < 1e-9);
        assert!((out.thrust_kn * 0.1 - 0.025 * 100.0 * ose_data::G0).abs() < 1e-9);
        assert!(s.fed_engines(0).is_empty());
        // Structural loading retains this final impulse even though the tank is now empty.
        s.apply(
            &p,
            &StepOutcome {
                thrust_kn: out.thrust_kn,
                ..Default::default()
            },
            0.1,
        );
        assert!(s.engine_load_kn[pair[0]] > 0.0);
        assert_eq!(s.engine_load_kn[pair[1]], 0.0);
        assert_no_engine_impulse(s.propulsion(&p, Default::default(), 0.1));
    }

    #[test]
    fn mixed_jet_and_rocket_propulsion_does_not_apply_air_limits_to_the_rocket() {
        let (mut s, p, pair) = propulsion_fixture();
        s.engine_jet[pair[0]] = Some(crate::flight::Jet {
            vel_curve: None,
            atm_curve: None,
        });
        s.intake_area_m2[s.root] = 1.0;
        let vacuum = s.engine_demands(0, 1.0, Default::default());
        assert_eq!(vacuum.len(), 1);
        assert_eq!(vacuum[0].part, pair[1]);
        let air = crate::flight::EngineEnvironment {
            density: 1.2,
            oxygen: true,
            ..Default::default()
        };
        assert_eq!(s.engine_demands(0, 1.0, air).len(), 2);
        let before = s.net.mass_of(&[pair[0]]);
        let out = s.propulsion(&p, Default::default(), 0.1);
        assert!((out.thrust_kn - 100.0 * ose_data::G0).abs() < 1e-9);
        assert!((out.burned_t - 0.05).abs() < 1e-9);
        assert_eq!(
            s.net.mass_of(&[pair[0]]),
            before,
            "unlit jet consumes no fuel"
        );
    }

    #[test]
    fn propulsion_respects_ignition_throttle_locked_engines_and_detachment() {
        let (mut s, mut p, pair) = propulsion_fixture();
        p.ignited = false;
        assert_no_engine_impulse(s.propulsion(&p, Default::default(), 0.1));
        p.ignited = true;
        p.throttle = 0.0;
        assert_no_engine_impulse(s.propulsion(&p, Default::default(), 0.1));
        s.engine_throttle_locked[pair[0]] = true;
        let out = s.propulsion(&p, Default::default(), 0.1);
        assert!((out.thrust_kn - 100.0 * ose_data::G0).abs() < 1e-9);
        s.rigid.remove_parts(&[pair[0]]);
        assert_no_engine_impulse(s.propulsion(&p, Default::default(), 0.1));
    }

    #[test]
    fn universe_applies_only_the_fuel_limited_impulse_and_does_not_drain_twice() {
        let (mut st, mut p, pair) = propulsion_fixture();
        // A nondegenerate orbital state: the conic representation needs angular momentum.
        p.v = DVec3::new(0.0, 100.0, 0.0);
        let fuel = 0.005; // One quarter of the first engine's 0.02 s request.
        let solid = st.net.resource("SolidFuel").unwrap();
        for tank in &mut st.net.tanks {
            if pair.contains(&tank.part) && tank.resource == solid {
                tank.stored =
                    if tank.part == pair[0] { fuel } else { 0.0 } / st.net.unit_mass_t[solid];
            }
        }
        st.sync(&mut p);
        let mass = p.mass_t;
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut u = crate::Universe::with_bodies(&bodies);
        let body = u.body_entity("Kerbin").unwrap();
        let mu = u.world.get::<crate::bodies::Body>(body).unwrap().mu;
        let radius = p.r.length();
        let vessel = u.spawn_vessel("Fuel-limited impulse", body, p.r, p.v);
        u.set_powered(vessel, p);
        u.set_structure(vessel, st);
        u.step();
        let after = u.powered(vessel).unwrap();
        let dt = 1.0 / crate::TICKS_PER_SECOND as f64;
        let impulse = fuel * 100.0 * ose_data::G0;
        let expected_v = impulse / mass - mu / (radius * radius) * dt;
        assert!(
            (after.v.x - expected_v).abs() < 1e-8,
            "{} vs {expected_v}",
            after.v.x
        );
        assert!((after.mass_t - (mass - fuel)).abs() < 1e-9);
        assert!((after.mass_t - u.structure(vessel).unwrap().net.mass_t()).abs() < 1e-9);
        u.step();
        assert!((u.powered(vessel).unwrap().mass_t - (mass - fuel)).abs() < 1e-9);
    }

    #[test]
    fn propulsion_impulse_and_fuel_are_invariant_when_splitting_a_step() {
        let (mut whole, p, pair) = propulsion_fixture();
        let (mut split, _, _) = propulsion_fixture();
        for s in [&mut whole, &mut split] {
            let solid = s.net.resource("SolidFuel").unwrap();
            for t in &mut s.net.tanks {
                if pair.contains(&t.part) && t.resource == solid {
                    t.stored = 0.015 / s.net.unit_mass_t[solid];
                }
            }
        }
        let one = whole.propulsion(&p, Default::default(), 0.08);
        let mut impulse = 0.0;
        let mut burned = 0.0;
        for _ in 0..4 {
            let step = split.propulsion(&p, Default::default(), 0.02);
            impulse += step.thrust_kn * 0.02;
            burned += step.burned_t;
        }
        assert!((impulse - one.thrust_kn * 0.08).abs() < 1e-9);
        assert!((burned - one.burned_t).abs() < 1e-9);
    }

    #[test]
    fn throttle_cut_keeps_solid_thrust_without_burning_liquid_fuel() {
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut s = Structure::build(craft, &defs, &res);
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        p.throttle = 1.0;
        s.sync(&mut p);
        let full = p.thrust_kn(0.0);
        p.throttle = 0.0;
        let cut = p.thrust_kn(0.0);
        assert!(
            cut > 0.0 && cut < full,
            "only the boosters should keep pushing"
        );
        let before = s.resource_totals();
        s.propulsion(&p, crate::flight::EngineEnvironment::default(), 0.02);
        s.sync(&mut p);
        let after = s.resource_totals();
        assert!(before
            .iter()
            .zip(&after)
            .any(|(a, b)| a.0 == "SolidFuel" && b.1 < a.1));
        for (a, b) in before.iter().zip(&after) {
            if a.0 == "Methane" || a.0 == "Oxidizer" {
                assert_eq!(a.1, b.1);
            }
        }
        let solid = s.net.resource("SolidFuel").unwrap();
        for tank in s.net.tanks.iter_mut().filter(|t| t.resource == solid) {
            tank.stored = 0.0;
        }
        s.sync(&mut p);
        assert_eq!(
            p.thrust_kn(0.0),
            0.0,
            "burned-out boosters produce no thrust"
        );
        assert!(s.firing_engines(p.stage, p.throttle).is_empty());
        p.throttle = 1.0;
        assert!(
            p.thrust_kn(0.0) > 0.0,
            "the liquid engine can still restart"
        );
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
        let packed = s.aero(0, basis(facing), fall, 1.0, 0.3);
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
        let semi = s.aero(0, basis(facing), fall, 1.0, 0.3);
        for _ in 0..40 {
            s.update_chutes(0.9, 800.0, 0.3, false, 100.0, 0.5);
        }
        assert_eq!(s.chutes[chute], Chute::Deployed);
        let open = s.aero(0, basis(facing), fall, 1.0, 0.3);
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

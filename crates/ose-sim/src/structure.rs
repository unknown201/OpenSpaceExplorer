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
    /// Drag-cube body lift and lifting-surface force (including surface drag), inertial kN.
    pub lift_kn: DVec3,
    pub torque_knm: DVec3,
    /// Body, fairing and parachute drag, summed as vectors at each part's local airspeed.
    pub drag_kn: DVec3,
}

/// Uniform atmospheric density and sound speed, with rigid-body velocity across the craft.
#[derive(Clone, Copy, Debug)]
pub struct Airflow {
    pub frame: (DVec3, DVec3, DVec3),
    pub velocity: DVec3,
    /// Craft angular velocity relative to the co-rotating atmosphere, inertial rad/s.
    pub angular_velocity: DVec3,
    pub density: f64,
    pub speed_of_sound: f64,
}

impl Airflow {
    pub fn at(p: &Powered, body: &crate::bodies::Body, atmo: Option<&crate::flight::Atmo>) -> Self {
        let omega = DVec3::new(
            0.0,
            0.0,
            if body.rotation_period_s > 0.0 {
                std::f64::consts::TAU / body.rotation_period_s
            } else {
                0.0
            },
        );
        let alt = p.r.length() - body.radius_m;
        Self {
            frame: crate::flight::vessel_basis(p),
            velocity: p.v - omega.cross(p.r),
            angular_velocity: if p.landed {
                DVec3::ZERO
            } else {
                p.angvel + p.facing * p.roll_rate - omega
            },
            density: atmo.map(|a| a.density(alt)).unwrap_or(0.0),
            speed_of_sound: atmo.map(|a| a.speed_of_sound(alt)).unwrap_or(f64::INFINITY),
        }
    }

    fn uniform(frame: (DVec3, DVec3, DVec3), velocity: DVec3, density: f64, mach: f64) -> Self {
        Self {
            frame,
            velocity,
            density,
            angular_velocity: DVec3::ZERO,
            speed_of_sound: if mach > 0.0 {
                velocity.length() / mach
            } else {
                f64::INFINITY
            },
        }
    }

    fn mach(self, speed: f64) -> f64 {
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
    resolved_actuators: Option<Vec<crate::actuators::Actuator>>,
    /// Last delivered gimbal direction in each engine's exported model frame.
    pub engine_gimbals: Vec<(u16, DVec3)>,
    wheel_actuation_knm: Vec<DVec3>,
    geometry: Vec<ose_data::flight_geometry::FlightGeometry>,
    controls: Vec<ose_data::flight_geometry::PartControl>,
    control_sources: Vec<bool>,
    gimbal_angles: Vec<Vec<[f64; 2]>>,
    surface_angles: Vec<f64>,
    surface_deploy_angles: Vec<f64>,
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
    engine_nozzles: Vec<Vec<ose_data::ThrustTransform>>,
    /// Each engine's gimbal range (`Data_Gimbal.gimbalRange`), rad; 0 for parts that do not gimbal.
    pub gimbal_range_rad: Vec<f64>,
    /// Each part's electric charge use and production, and crew capacity.
    pub electric: Vec<ose_data::Electric>,
    pub crew: Vec<u32>,
    /// Each part's crash tolerance, m/s.
    pub crash_tolerance: Vec<f64>,
    /// Deployable solar panels (and antennas) folded out.
    pub deployed: Vec<bool>,
    /// Antennas that extend and retract (`Data_Deployable` beside `Data_Transmitter`); they start retracted.
    pub antenna_deployable: Vec<bool>,
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
    /// The craft's +Z in flight (`flight::vessel_top`), inertial, kept current by the universe: with the
    /// facing it places the parts (a plane's belly down). Zero falls back to the unrolled frame.
    pub frame_top: DVec3,
    /// Each part's lift and drag at the last `aero_torque` call, kN, inertial (the aero overlay).
    pub part_aero_kn: Vec<(DVec3, DVec3)>,
    /// RCS thrusters on the parts.
    pub rcs_defs: Vec<Option<ose_data::Rcs>>,
    /// Lifting surfaces (their curves resolved), intake areas, and each engine's jet curves.
    lift_defs: Vec<Option<(ose_data::LiftingSurface, aero::LiftCurves)>>,
    /// Parts connected to a valid, authored lift-gating node. Presence is checked each step.
    lift_node_links: Vec<Vec<usize>>,
    pressure_offsets_m: Vec<DVec3>,
    lift_offsets_m: Vec<DVec3>,
    drag_properties: Vec<ose_data::DragProperties>,
    body_lift_curves: aero::LiftCurves,
    control_pressure_offsets_m: Vec<DVec3>,
    pub intake_area_m2: Vec<f64>,
    /// Control surfaces: deflection range (rad) and moving share of the surface.
    ctrl: Vec<(f64, f64)>,
    engine_jet: Vec<Option<crate::flight::Jet>>,
    /// Built nose-forward (a plane): craft +Z is the belly and -Z is up, in right-handed vessel axes.
    pub level: bool,
    /// Flight stage the structure was last trimmed to.
    staged_to: usize,
}

pub use ose_data::craft::{level_layout, rotated};

/// A parachute's state (`DeploymentStates`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// How it tumbles as it goes, rad/s, inertial (zero: the universe picks).
    pub spin: DVec3,
}

/// A part's bounds (part space, Unity axes) as a box in the vessel frame: centre relative to the part
/// origin and half size of the rotated box.
fn vessel_box(q: ose_data::craft::Quat, b: ose_data::PartBounds) -> (DVec3, DVec3) {
    let q = q.normalized();
    let q = [q.x, q.y, q.z, q.w];
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
    /// A position offset in the vessel frame (as the rigid body keeps them) in the inertial frame.
    pub fn offset_inertial(&self, facing: DVec3, v: DVec3) -> DVec3 {
        let (bx, by, bz) = self.frame(facing);
        bx * v.x + by * v.y + bz * v.z
    }

    /// A part axis (craft coordinates, as the part data gives it) in the inertial frame.
    pub fn axis_inertial(&self, facing: DVec3, u: DVec3) -> DVec3 {
        let (bx, by, bz) = self.frame(facing);
        bx * u.x + by * u.y + bz * (-u.z)
    }

    /// Take over what the parts of `old` were doing (`map`: old part index to this structure's): panels,
    /// chutes, heat, gear, converters, fairings, bays.
    pub fn carry_state_from(&mut self, old: &Structure, map: &[Option<usize>]) {
        for (i, j) in map.iter().enumerate() {
            let Some(j) = *j else { continue };
            if i >= old.rigid.len() || j >= self.rigid.len() {
                continue;
            }
            self.deployed[j] = old.deployed[i];
            self.chutes[j] = old.chutes[i];
            self.chute_open[j] = old.chute_open[i];
            self.temperature_k[j] = old.temperature_k[i];
            self.ablating[j] = old.ablating[i];
            self.gear_out[j] = old.gear_out[i];
            self.converter_on[j] = old.converter_on[i];
            self.fairing_deployed[j] = old.fairing_deployed[i];
            self.bay_open[j] = old.bay_open[i];
        }
        self.frame_top = old.frame_top;
        self.update_shielding();
    }

    /// The craft's axes in the inertial frame: +Y along `facing`, +Z toward `frame_top`.
    fn frame(&self, facing: DVec3) -> (DVec3, DVec3, DVec3) {
        let y = facing.normalized();
        let z = self.frame_top - y * self.frame_top.dot(y);
        if z.length_squared() < 0.25 {
            return basis(facing);
        }
        let z = z.normalized();
        (y.cross(z), y, z)
    }

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
                let q = p.local_rotation.normalized();
                [q.x, q.y, q.z, q.w]
            })
            .collect();
        let n = craft.parts.len();
        let gimbal_range_rad = (0..n)
            .map(|i| {
                def_of(i)
                    .and_then(|d| d.engines.first())
                    .and_then(|e| e.gimbal_range_deg)
                    .unwrap_or(0.0)
                    .to_radians()
            })
            .collect();
        let engine_mixture = (0..n)
            .map(|i| {
                def_of(i)
                    .and_then(|d| d.engines.first())
                    .map(|e| e.propellant_mixture.clone())
            })
            .collect();
        let antenna_deployable = (0..n)
            .map(|i| {
                def_of(i).is_some_and(|d| {
                    d.electric.comm_range_m > 0.0
                        && d.module_names
                            .iter()
                            .any(|m| m == "PartComponentModule_Deployable")
                })
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
            surface_deploy_angles: vec![0.0; n],
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
            engine_nozzles: (0..n)
                .map(|i| {
                    def_of(i)
                        .and_then(|d| d.engines.first())
                        .map(|e| e.thrust_transforms.clone())
                        .unwrap_or_default()
                })
                .collect(),
            gimbal_range_rad,
            deployed: vec![false; n],
            crash_tolerance: (0..n)
                .map(|i| def_of(i).map(|d| d.crash_tolerance).unwrap_or(0.0))
                .collect(),
            electric,
            antenna_deployable,
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
            lift_offsets_m: (0..n)
                .map(|i| def_of(i).map(|d| d.lift_offset_m).unwrap_or(DVec3::ZERO))
                .collect(),
            drag_properties: (0..n)
                .map(|i| def_of(i).map(|d| d.drag_properties).unwrap_or_default())
                .collect(),
            body_lift_curves: aero::body_lift_curves(),
            pressure_offsets_m: (0..n)
                .map(|i| {
                    def_of(i)
                        .map(|d| d.pressure_offset_m)
                        .unwrap_or(DVec3::ZERO)
                })
                .collect(),
            control_pressure_offsets_m: (0..n)
                .map(|i| {
                    def_of(i)
                        .map(|d| d.ctrl_pressure_offset_m)
                        .unwrap_or(DVec3::ZERO)
                })
                .collect(),
            lift_node_links: (0..n)
                .map(|i| {
                    let Some(def) = def_of(i) else {
                        return Vec::new();
                    };
                    let Some(lift) = &def.lifting_surface else {
                        return Vec::new();
                    };
                    if !lift.node_enabled
                        || lift.attach_node_name.is_empty()
                        || !def
                            .attach_nodes
                            .iter()
                            .any(|node| node.id == lift.attach_node_name)
                    {
                        return Vec::new();
                    }
                    let node = lift.attach_node_name.as_str();
                    craft
                        .parts
                        .iter()
                        .enumerate()
                        .filter_map(|(j, part)| {
                            if j == i && part.own_node.as_deref() == Some(node) {
                                part.parent.filter(|&parent| parent < n)
                            } else if part.parent == Some(i)
                                && part.parent_node.as_deref() == Some(node)
                            {
                                Some(j)
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .collect(),
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
                            aero::settings()
                                .lift_curves
                                .get(&l.curve_set)
                                .cloned()
                                .unwrap_or_else(aero::default_lift_curves)
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
            frame_top: DVec3::ZERO,
            part_aero_kn: vec![(DVec3::ZERO, DVec3::ZERO); n],
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
            if self.engine_nozzles[i].is_empty() {
                return vec![PartForce {
                    part: i,
                    force_kn: self.part_axes(i)[1] * thrust,
                    offset_m: DVec3::ZERO,
                }];
            }
            return self.engine_nozzles[i]
                .iter()
                .map(|point| PartForce {
                    part: i,
                    force_kn: crate::rigid::to_vessel(aero::rotate(
                        self.part_rot[i],
                        point.direction,
                    )) * (thrust * point.weight),
                    offset_m: crate::rigid::to_vessel(aero::rotate(
                        self.part_rot[i],
                        point.position_m,
                    )),
                })
                .collect();
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

    /// Current attached mass distribution. As in PartComponent, the authored centre
    /// applies to the part's total mass; its own inertia still uses the existing box proxy.
    pub fn mass_properties(&self) -> crate::dynamics::MassProperties {
        let masses = self.part_masses();
        let mut out = crate::dynamics::MassProperties::default();
        for (i, &mass) in masses.iter().enumerate() {
            if self.rigid.present[i] && self.net.present[i] {
                out.mass_t += mass;
                out.centre_m = out.centre_m + self.rigid.rest_mass_position(i) * mass;
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
                        self.rigid.rest_mass_position(i) - out.centre_m,
                    );
                    continue;
                }
                out.inertia.add_box(
                    mass,
                    self.rigid.sizes_m[i],
                    self.part_axes(i),
                    self.rigid.rest_mass_position(i) - out.centre_m,
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
                        kind: crate::actuators::Kind::Other,
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
            let base = self.engine_forces(i, thrust);
            let axial = base.iter().fold(DVec3::ZERO, |v, f| v + f.force_kn);
            let weight: f64 = base.iter().map(|f| f.force_kn.length()).sum();
            let offset = if weight > 0.0 {
                base.iter()
                    .fold(DVec3::ZERO, |v, f| v + f.offset_m * f.force_kn.length())
                    * (1.0 / weight)
            } else {
                DVec3::ZERO
            };
            let lever = r + offset;
            let side = axial.length()
                * (self.gimbal_range_rad[i] * cfg.gimbal_limiter).sin()
                * f64::from(cfg.gimbal_enabled);
            let normal = if axial.length_squared() > 0.0 {
                axial.normalized()
            } else {
                axes[1]
            };
            let tangent = axes[0] - normal * axes[0].dot(normal);
            let tangent = if tangent.length_squared() > 1e-12 {
                tangent.normalized()
            } else {
                axes[2]
            };
            for axis in [tangent, normal.cross(tangent)] {
                if side > 0.0 {
                    let force = axis * side;
                    channels.push(Actuator {
                        kind: crate::actuators::Kind::Other,
                        part: i,
                        torque_knm: lever.cross(force),
                        force_kn: force,
                        offset_m: offset,
                        group: i * 8 + 3,
                        axial: Some((axial, lever)),
                    });
                }
            }
            // Old catalogs without nozzle transforms retain the mounted force fallback.
            if p.groups.rcs && cfg.rcs_enabled {
                if let Some(rcs) = self.rcs_defs[i].as_ref().filter(|_| self.rcs_fed(i)) {
                    let isp_scale = if rcs.isp_curve.evaluate(0.0) > 0.0 {
                        rcs.isp_curve.evaluate(env.pressure_atm) / rcs.isp_curve.evaluate(0.0)
                    } else {
                        0.0
                    };
                    if !self.geometry[i].rcs.is_empty() || !rcs.thrusters.is_empty() {
                        let points: Vec<_> = if self.geometry[i].rcs.is_empty() {
                            rcs.thrusters
                                .iter()
                                .map(|p| (p.position_m, p.direction, 1.0))
                                .collect()
                        } else {
                            self.geometry[i]
                                .rcs
                                .iter()
                                .map(|p| (p.position_m, p.direction, p.weight))
                                .collect()
                        };
                        for (position, direction, weight) in points {
                            let force =
                                crate::rigid::to_vessel(aero::rotate(self.part_rot[i], direction))
                                    * (rcs.limited_thrust_kn()
                                        * isp_scale.max(0.0)
                                        * cfg.rcs_limiter
                                        * weight);
                            let offset =
                                crate::rigid::to_vessel(aero::rotate(self.part_rot[i], position));
                            let precision = crate::actuators::rcs_precision_factor(
                                if force.length_squared() > 1e-16 {
                                    force.normalized()
                                } else {
                                    DVec3::ZERO
                                },
                                r + offset,
                                p.rcs.precision,
                                self.geometry[i].rcs_tuning.use_lever,
                            );
                            let force = force * precision;
                            channels.push(Actuator {
                                kind: crate::actuators::Kind::RcsNozzle,
                                part: i,
                                torque_knm: (r + offset).cross(force),
                                force_kn: force,
                                offset_m: offset,
                                group: self.rigid.len() * 8 + channels.len(),
                                axial: None,
                            });
                        }
                    } else {
                        for axis in axes {
                            let force = axis
                                * (rcs.limited_thrust_kn() * isp_scale.max(0.0) * cfg.rcs_limiter);
                            channels.push(Actuator {
                                kind: crate::actuators::Kind::RcsMount,
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
                if range > 0.0 && share > 0.0 && self.lifting_surface_active(i) {
                    let (speed, mach) = self
                        .last_airflow
                        .map(|flow| {
                            let speed = self
                                .air_at_offset(i, self.control_offset(i), flow, centre)
                                .length();
                            (speed, flow.mach(speed))
                        })
                        .unwrap_or((self.last_air_speed_mps, env.mach));
                    let q = 0.5 * env.density * speed.powi(2);
                    let cap = (q
                        * aero::settings().lift_multiplier
                        * self.control_lift_coeff(i)
                        * curves.lift.evaluate((range * cfg.surface_authority).sin())
                        * curves.lift_mach.evaluate(mach))
                    .max(0.0);
                    let axis = aero::rotate(
                        self.part_rot[i],
                        self.geometry[i]
                            .surface
                            .as_ref()
                            .map(|g| self.surface_normal(i, g))
                            .unwrap_or(l.axis),
                    );
                    let force = DVec3::new(axis.x, axis.y, -axis.z) * cap;
                    let offset = self.control_offset(i);
                    let at = self.rigid.rest_position(i) + offset;
                    channels.push(Actuator {
                        kind: crate::actuators::Kind::Other,
                        part: i,
                        torque_knm: (at - centre).cross(force),
                        force_kn: force,
                        offset_m: offset,
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
        propulsion.rcs_controls = self
            .rcs_defs
            .iter()
            .map(|r| r.as_ref().map(|r| r.controls))
            .collect();
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
                thrust += r.limited_thrust_kn() * r.thrusters.len().max(1) as f64;
                torque += r.limited_thrust_kn() * (self.rigid.rest_position(i) - c).length();
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

    fn rcs_fed(&self, part: usize) -> bool {
        self.rcs_defs[part].as_ref().is_some_and(|r| {
            r.isp_curve.evaluate(0.0) > 0.0
                && r.limited_thrust_kn() > 0.0
                && (!self.shielded[part] || r.shielded_can_activate)
                && self.net.mixture_unit_mass_t(&r.propellant) > 0.0
                && self
                    .net
                    .ingredients(&r.propellant)
                    .iter()
                    .all(|&(resource, units)| {
                        units <= 0.0 || self.net.available(part, resource) > 1e-9
                    })
        })
    }

    /// Each mount pays for its combined translation/rotation vector once, before motion.
    fn resolve_rcs_fuel(
        &mut self,
        p: &mut Powered,
        propulsion: &mut crate::flight::Propulsion,
        allocation: &mut crate::actuators::Allocation,
        dt: f64,
    ) {
        p.rcs.usage = 0.0;
        p.rcs.powered = 0.0;
        if propulsion.actuators.as_ref().is_some_and(|a| {
            a.iter().any(|a| {
                a.kind == crate::actuators::Kind::RcsNozzle && !self.geometry[a.part].rcs.is_empty()
            })
        }) {
            self.resolve_nozzle_rcs(p, propulsion, allocation, dt);
        }
        let nozzle_usage = p.rcs.usage;
        let nozzle_powered = p.rcs.powered;
        let channels = propulsion.actuators.as_ref().expect("configured actuators");
        let mut effort = vec![0.0; self.rigid.len()];
        for (a, input) in channels.iter().zip(&allocation.inputs) {
            if a.kind.is_rcs() && self.geometry[a.part].rcs.is_empty() {
                effort[a.part] += if a.kind == crate::actuators::Kind::RcsNozzle {
                    *input
                } else {
                    input * input
                };
            }
        }
        let mut parts = Vec::new();
        let mut requests = Vec::new();
        let mut demand = 0.0;
        for (i, norm) in effort.iter_mut().enumerate() {
            if self.rcs_defs[i]
                .as_ref()
                .is_some_and(|r| r.thrusters.is_empty())
            {
                *norm = norm.sqrt().min(1.0);
            }
            if !self.rigid.present[i] || *norm <= 0.0 {
                continue;
            }
            let r = self.rcs_defs[i].as_ref().expect("RCS channel definition");
            let unit_mass = self.net.mixture_unit_mass_t(&r.propellant);
            let flow = r.limited_thrust_kn() * *norm / (r.isp_curve.evaluate(0.0) * ose_data::G0);
            parts.push(i);
            demand += r.limited_thrust_kn() * *norm;
            requests.push(Request {
                part: i,
                instructions: self
                    .net
                    .ingredients(&r.propellant)
                    .into_iter()
                    .map(|(resource, units)| Instruction {
                        resource,
                        units_per_s: units * flow / unit_mass,
                    })
                    .collect(),
                priority: 6,
            });
        }
        let before = self.net.mass_t();
        let mut delivered = vec![0.0; self.rigid.len()];
        let mut supplied = 0.0;
        for (i, fraction) in parts.into_iter().zip(self.net.process(&requests, dt)) {
            delivered[i] = fraction;
            supplied +=
                self.rcs_defs[i].as_ref().unwrap().limited_thrust_kn() * effort[i] * fraction;
        }
        propulsion.burned_t += (before - self.net.mass_t()).max(0.0);
        p.rcs.powered = if demand > 0.0 {
            supplied / demand
        } else {
            nozzle_powered
        };
        p.rcs.usage = if p.rcs.thrust_kn > 0.0 {
            supplied / p.rcs.thrust_kn
        } else {
            nozzle_usage
        };
        for (a, input) in channels.iter().zip(&mut allocation.inputs) {
            if a.kind.is_rcs() && self.geometry[a.part].rcs.is_empty() {
                let old = *input;
                *input *= delivered[a.part];
                allocation.force_kn = allocation.force_kn + a.force_kn * (*input - old);
                allocation.torque_knm = allocation.torque_knm + a.torque_knm * (*input - old);
            }
        }
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
        self.feet_along(self.up_axis())
    }

    /// `feet` with "up" along `u` (vessel frame): the parts first in line when the blow comes from -`u`.
    fn feet_along(&self, u: DVec3) -> (f64, f64, f64, Vec<usize>) {
        let present: Vec<usize> = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i])
            .collect();
        if present.is_empty() {
            return (0.0, 0.0, 0.0, Vec::new());
        }
        let c = self.rigid.rest_centre();
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

    /// Independent producers run before propulsion. Engine alternators require resolved thrust instead.
    fn electric_producers(&mut self, sun: f64, dt: f64) {
        let Some(ec) = self.net.resource("ElectricCharge") else {
            return;
        };
        let mut requests = Vec::new();
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
            if self.engine_isp[i].is_none() {
                made += e.generator_output;
            }
            if made > 0.0 {
                requests.push(Request {
                    part: i,
                    instructions: vec![instruction(-made)],
                    priority: 10,
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
        let fractions = self.net.process(&requests, dt);
        for &(r, i) in &converting {
            self.converter_running[i] = fractions[r];
        }
    }

    /// Match the reference generator's FinalThrust / mode.maxThrust output at each engine mount.
    /// A final partial burn supplies partial charge; dry, air-starved and idle engines supply none.
    fn engine_alternators(&mut self, dt: f64) {
        let (Some(ec), Some(thrust)) = (
            self.net.resource("ElectricCharge"),
            self.resolved_engine_thrust.as_ref(),
        ) else {
            return;
        };
        let requests: Vec<_> = thrust
            .iter()
            .enumerate()
            .filter(|&(i, t)| {
                self.rigid.present[i]
                    && self.engine_thrust_kn[i] > 0.0
                    && self.electric[i].generator_output > 0.0
                    && *t > 0.0
            })
            .map(|(i, t)| Request {
                part: i,
                instructions: vec![Instruction {
                    resource: ec,
                    units_per_s: -self.electric[i].generator_output * t / self.engine_thrust_kn[i],
                }],
                priority: 10,
            })
            .collect();
        self.net.process(&requests, dt);
    }

    /// Probe cores and drive motors draw once after producers, before wheel actuation is planned.
    fn electric_consumers(&mut self, p: &mut Powered, dt: f64) {
        let Some(ec) = self.net.resource("ElectricCharge") else {
            return;
        };
        let instruction = |units_per_s| Instruction {
            resource: ec,
            units_per_s,
        };
        let mut requests = Vec::new();
        for i in 0..self.rigid.len() {
            if self.rigid.present[i] && self.electric[i].command_use > 0.0 {
                requests.push(Request {
                    part: i,
                    instructions: vec![instruction(self.electric[i].command_use)],
                    priority: 5,
                });
            }
        }
        let cores = requests.len();
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
        p.controlled = crewed || cores == 0 || fractions[..cores].iter().any(|&f| f > 0.0);
        if !p.controlled {
            p.throttle = 0.0;
        }
    }

    /// Coast/rails services have no resolved burn and therefore cannot run an engine alternator.
    pub fn electric_services(&mut self, p: &mut Powered, sun: f64, dt: f64) {
        self.electric_producers(sun, dt);
        self.electric_consumers(p, dt);
    }

    /// Resolve propulsion and its electrical output together, retaining same-tick control and wheel power.
    pub fn powered_services(
        &mut self,
        p: &mut Powered,
        sun: f64,
        env: crate::flight::EngineEnvironment,
        dt: f64,
    ) -> crate::flight::Propulsion {
        self.electric_producers(sun, dt);
        self.sync(p);
        // An uncrewed controller may cut throttle after electrical delivery. Keep that trial's resource
        // state transactional: discarded thrust must not consume fuel or leave alternator charge behind.
        let needs_control = !(0..self.rigid.len())
            .any(|i| self.rigid.present[i] && self.crew[i] > 0)
            && (0..self.rigid.len())
                .any(|i| self.rigid.present[i] && self.electric[i].command_use > 0.0);
        let before = (needs_control && p.throttle > 0.0).then(|| self.net.tanks.clone());
        let throttle = p.throttle;
        let mut propulsion = self.propulsion(p, env, dt);
        self.engine_alternators(dt);
        self.electric_consumers(p, dt);
        if p.throttle != throttle {
            if let Some(before) = before {
                self.net.tanks = before;
                propulsion = self.propulsion(p, env, dt);
                self.engine_alternators(dt);
                self.electric_consumers(p, dt);
            }
        }
        propulsion
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

    /// Freeze planned controls to this tick's delivered wheel charge and RCS propellant.
    pub fn resolve_wheel_power(
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
            let manual = p.manual_axes();
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
        self.resolve_rcs_fuel(p, propulsion, &mut allocation, dt);
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

    fn control_lift_coeff(&self, i: usize) -> f64 {
        let Some((lift, _)) = &self.lift_defs[i] else {
            return 0.;
        };
        if let Some(surface) = &self.geometry[i].surface {
            if surface.ignore_sas {
                return 0.;
            }
            let deployed = if let Some(deploy) = surface.deploy.as_ref().filter(|d| d.separate) {
                if self.surface_deploy_angles[i] < deploy.min_control_angle_rad {
                    return 0.;
                }
                (self.surface_deploy_angles[i] / deploy.angle_rad.max(1e-10)).clamp(0., 1.)
            } else {
                1.
            };
            surface
                .moving_lift_coeff
                .unwrap_or(lift.deflection_lift_coeff * self.ctrl[i].1)
                * deployed
        } else {
            lift.deflection_lift_coeff * self.ctrl[i].1
        }
    }

    fn surface_normal(
        &self,
        i: usize,
        surface: &ose_data::flight_geometry::ControlSurface,
    ) -> DVec3 {
        let mut normal = glam::DVec3::new(surface.normal.x, surface.normal.y, surface.normal.z);
        if let Some(deploy) = surface.deploy.as_ref().filter(|d| d.separate) {
            let axis = glam::DVec3::new(deploy.axis.x, deploy.axis.y, deploy.axis.z).normalize();
            normal = glam::DQuat::from_axis_angle(axis, self.surface_deploy_angles[i]) * normal;
        }
        DVec3::new(normal.x, normal.y, normal.z)
    }

    pub fn surface_deployments(&self) -> Vec<(u16, f32)> {
        self.geometry
            .iter()
            .enumerate()
            .filter(|(i, g)| {
                self.rigid.present[*i]
                    && g.surface
                        .as_ref()
                        .and_then(|s| s.deploy.as_ref())
                        .is_some_and(|d| d.separate)
            })
            .map(|(i, _)| (i as u16, self.surface_deploy_angles[i] as f32))
            .collect()
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
            if !self.rigid.present[i] || !self.lifting_surface_active(i) {
                continue;
            }
            let point = self.lift_point(i);
            let input = if !p.controlled {
                DVec3::ZERO
            } else if surface.ignore_sas {
                aero::rotate(p.control_rotation, p.manual_axes())
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
            let input = aero::rotate(p.control_rotation, input);
            let velocity = flow.angular_velocity.cross({
                let r = point - centre;
                flow.frame.0 * r.x + flow.frame.1 * r.y + flow.frame.2 * r.z
            }) + flow.velocity;
            let speed = velocity.length();
            let travel = if speed > 0. {
                velocity * (1. / speed)
            } else {
                flow.frame.1
            };
            let q = 0.5 * flow.density * speed * speed;
            let old = self.lifting_force(i, flow.frame, travel, q, flow.mach(speed));
            let deploy_target = surface
                .deploy
                .as_ref()
                .map(|d| {
                    if cfg.surface_deploy {
                        d.angle_rad * cfg.surface_deploy_fraction
                    } else {
                        0.
                    }
                })
                .unwrap_or(0.);
            if surface.deploy.as_ref().is_some_and(|d| d.separate) {
                let previous = self.surface_deploy_angles[i];
                self.surface_deploy_angles[i] = if surface.exponential {
                    previous + (deploy_target - previous) * (surface.speed * dt).clamp(0., 1.)
                } else {
                    previous
                        + (deploy_target - previous).clamp(
                            -surface.speed.to_radians() * dt,
                            surface.speed.to_radians() * dt,
                        )
                };
            }
            let hinge =
                crate::rigid::to_vessel(aero::rotate(self.part_rot[i], surface.axis)).normalized();
            let normal = crate::rigid::to_vessel(aero::rotate(
                self.part_rot[i],
                self.surface_normal(i, &surface),
            ))
            .normalized();
            let pivot = self.part_point(i, surface.pivot_m);
            let up = aero::rotate(p.control_rotation, DVec3::new(0., 1., 0.));
            let mut target = crate::actuators::surface_mix(
                up,
                input,
                centre - pivot,
                hinge,
                normal,
                self.surface_angles[i],
            ) * self.ctrl[i].0
                * cfg.surface_authority;
            if let Some(deploy) = &surface.deploy {
                if deploy.separate {
                    if self.surface_deploy_angles[i] < deploy.min_control_angle_rad {
                        target = 0.;
                        self.surface_angles[i] = 0.;
                    }
                } else {
                    target += deploy_target;
                }
            }
            target = (target * if cfg.surface_invert { -1. } else { 1. })
                .clamp(-1.5 * self.ctrl[i].0, 1.5 * self.ctrl[i].0);
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

    /// Resolve authored nozzle commands against their combined propellant budgets.
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
            .filter(|(_, a)| {
                a.kind == crate::actuators::Kind::RcsNozzle && !self.geometry[a.part].rcs.is_empty()
            })
            .map(|(i, _)| i)
            .collect();
        let nozzles: Vec<_> = indices.iter().map(|&i| channels[i].clone()).collect();

        let inputs: Vec<_> = nozzles
            .iter()
            .map(|a| {
                let cfg = p.part_control(a.part);
                let controls = self.rcs_defs[a.part].as_ref().unwrap().controls;
                let translation = if p.controlled && !p.crashed {
                    aero::rotate(
                        p.control_rotation,
                        controls.translation(p.rcs.translate, p.throttle),
                    )
                } else {
                    DVec3::ZERO
                };
                let rotation = aero::rotate_inverse(p.control_rotation, p.control_axes);
                let axes = controls.rotation_axes();
                let rotation = aero::rotate(
                    p.control_rotation,
                    DVec3::new(
                        rotation.x * axes.x,
                        rotation.y * axes.y,
                        rotation.z * axes.z,
                    ),
                );
                let mask = |v: DVec3, offset: usize| {
                    let v = aero::rotate_inverse(p.control_rotation, v);
                    aero::rotate(
                        p.control_rotation,
                        DVec3::new(
                            if cfg.rcs_axes[offset] && v.x.abs() >= 0.05 {
                                v.x
                            } else {
                                0.
                            },
                            if cfg.rcs_axes[offset + 1] && (offset == 3 || v.y.abs() >= 0.05) {
                                v.y
                            } else {
                                0.
                            },
                            if cfg.rcs_axes[offset + 2] && v.z.abs() >= 0.05 {
                                v.z
                            } else {
                                0.
                            },
                        ),
                    )
                };
                if a.force_kn.length_squared() < 1e-16 {
                    0.
                } else {
                    let direction = a.force_kn.normalized();
                    let lever =
                        self.rigid.rest_position(a.part) + a.offset_m - propulsion.mass.centre_m;
                    let input = crate::actuators::rcs_input(
                        direction,
                        lever,
                        mask(rotation, 0),
                        mask(translation, 3),
                    );
                    let tuning = ose_data::flight_geometry::RcsTuning {
                        full_thrust: controls.full_thrust,
                        full_thrust_min: controls.full_thrust_min,
                        ..self.geometry[a.part].rcs_tuning
                    };
                    let factor = crate::actuators::rcs_precision_factor(
                        direction,
                        lever,
                        p.rcs.precision,
                        tuning.use_lever,
                    );
                    crate::actuators::rcs_power(input, direction, lever, p.rcs.precision, tuning)
                        / factor
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
            Airflow::uniform(frame, v_air, density, mach),
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
                    let velocity = local(self.part_air_velocity(i, flow, centre));
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
            let velocity = local(self.part_air_velocity(i, flow, centre));
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
    ///
    /// `against` is where the blow comes from in the vessel frame (the parts furthest the other way take it
    /// first); `None` takes it on the parts the vessel stands on.
    pub fn impact(
        &mut self,
        speed: f64,
        facing: DVec3,
        against: Option<DVec3>,
    ) -> Option<(Vec<usize>, Vec<Piece>)> {
        let c = self.rigid.rest_centre();
        match self.crash(speed, facing, against, c) {
            (broken, pieces, false) => Some((broken, pieces)),
            _ => None,
        }
    }

    /// `impact`, carried on when the root breaks: the blow goes on through the parts in line until it is
    /// spent, and what is left falls apart into its connected pieces (`true`: the vessel is lost; its pieces
    /// are offset from `centre`, the vessel frame point the vessel's position stands for). The parts it
    /// breaks are gone; the pieces leave the structure.
    pub fn crash(
        &mut self,
        mut speed: f64,
        facing: DVec3,
        against: Option<DVec3>,
        centre: DVec3,
    ) -> (Vec<usize>, Vec<Piece>, bool) {
        let (bx, by, bz) = self.frame(facing);
        let mut broken = Vec::new();
        let mut pieces = Vec::new();
        let mut lost = false;
        let u = against
            .filter(|a| a.length_squared() > 1e-12)
            .map(|a| a.normalized())
            .unwrap_or_else(|| self.up_axis());
        loop {
            let (_, _, tolerance, feet) = self.feet_along(u);
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
            lost |= feet.contains(&self.root);
            self.rigid.remove_parts(&feet);
            self.net.remove_parts(&feet);
            broken.extend(feet);
            if !lost {
                pieces.extend(self.split_off(bx, by, bz));
                speed = (speed * speed - tolerance * tolerance).max(0.0).sqrt();
            } else {
                // once it is coming apart each crushed layer slows what is behind it more (our own figure:
                // twice its tolerance), so the far end of a fast crash comes through
                speed = (speed - 2.0 * tolerance).max(0.0);
            }
        }
        if lost {
            // the wreck: every piece still holding together goes its own way
            let to_inertial = |v: DVec3| bx * v.x + by * v.y + bz * v.z;
            for g in self.rigid.groups() {
                let mass = self.net.mass_of(&g);
                let dry: f64 = g.iter().map(|&i| self.net.dry_t[i]).sum();
                let m: f64 = g.iter().map(|&i| self.rigid.masses_t[i]).sum();
                let c = if m > 0.0 {
                    g.iter()
                        .map(|&i| self.rigid.rest_position(i) * self.rigid.masses_t[i])
                        .fold(DVec3::ZERO, |a, b| a + b)
                        * (1.0 / m)
                } else {
                    self.rigid.rest_position(g[0])
                };
                pieces.push(Piece {
                    mass_t: mass,
                    propellant_t: mass - dry,
                    offset_m: to_inertial(c - centre),
                    dv_mps: DVec3::ZERO,
                    spin: DVec3::ZERO,
                    parts: g,
                });
            }
            for piece in &pieces {
                self.rigid.remove_parts(&piece.parts);
                self.net.remove_parts(&piece.parts);
            }
        }
        (broken, pieces, lost)
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
        let (bx, by, bz) = self.frame(facing);
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

    /// Extend all deployable antennas (or retract them); returns how many changed.
    pub fn deploy_antennas(&mut self, out: bool) -> usize {
        let mut n = 0;
        for i in 0..self.antenna_deployable.len() {
            if self.antenna_deployable[i] && self.rigid.present[i] && self.deployed[i] != out {
                self.deployed[i] = out;
                n += 1;
            }
        }
        n
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

    fn lifting_surface_active(&self, part: usize) -> bool {
        self.rigid.present[part]
            && !self.shielded[part]
            && !self.lift_node_links[part]
                .iter()
                .any(|&other| self.rigid.present[other])
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
        if !self.lifting_surface_active(part) {
            return DVec3::ZERO;
        }
        let (x, y, z) = frame;
        let axis = surface
            .reference_frame
            .as_ref()
            .map(|f| f.direction(surface.axis))
            .unwrap_or(surface.axis);
        let axis = crate::rigid::to_vessel(aero::rotate(self.part_rot[part], axis));
        let normal = x * axis.x + y * axis.y + z * axis.z;
        let fixed = aero::surface_force(curves, surface, normal, travel, q, mach);
        if let Some(hinge) = &self.geometry[part].surface {
            let h =
                crate::rigid::to_vessel(aero::rotate(self.part_rot[part], hinge.axis)).normalized();
            let rotation = glam::DQuat::from_axis_angle(
                glam::DVec3::new(h.x, h.y, h.z),
                -self.surface_angles[part],
            );
            let neutral = crate::rigid::to_vessel(aero::rotate(
                self.part_rot[part],
                self.surface_normal(part, hinge),
            ));
            let rotated = rotation * glam::DVec3::new(neutral.x, neutral.y, neutral.z);
            let share = self.ctrl[part].1.clamp(0., 1.);
            let mut fixed_surface = surface.clone();
            fixed_surface.deflection_lift_coeff = hinge
                .fixed_lift_coeff
                .unwrap_or(surface.deflection_lift_coeff * (1. - share));
            let fixed = aero::surface_force(curves, &fixed_surface, normal, travel, q, mach);
            let mut moving_surface = surface.clone();
            let deployed = hinge
                .deploy
                .as_ref()
                .filter(|d| d.separate && d.angle_rad.abs() > 1e-10)
                .map(|d| (self.surface_deploy_angles[part] / d.angle_rad).clamp(0., 1.))
                .unwrap_or(1.);
            moving_surface.deflection_lift_coeff = hinge
                .moving_lift_coeff
                .unwrap_or(surface.deflection_lift_coeff * share)
                * deployed;
            let moving = aero::surface_force(
                curves,
                &moving_surface,
                x * rotated.x + y * rotated.y + z * rotated.z,
                travel,
                q,
                mach,
            );
            fixed + moving
        } else {
            fixed
        }
    }

    fn body_lifting_force(
        &self,
        part: usize,
        frame: (DVec3, DVec3, DVec3),
        travel: DVec3,
        q: f64,
        mach: f64,
    ) -> DVec3 {
        let props = self.drag_properties[part];
        if !self.rigid.present[part]
            || self.shielded[part]
            || !props.body_lift_enabled
            || props.body_lift_multiplier <= 0.0
        {
            return DVec3::ZERO;
        }
        let Some(faces) = &self.faces[part] else {
            return DVec3::ZERO;
        };
        let (x, y, z) = frame;
        let local = aero::rotate_inverse(
            self.part_rot[part],
            DVec3::new(travel.dot(x), travel.dot(y), -travel.dot(z)),
        );
        let vector = faces.body_lift_vector(&self.body_lift_curves.lift, local);
        let vector = self.mounted_offset(part, vector);
        let force = (x * vector.x + y * vector.y + z * vector.z)
            * (q / 1000.0
                * aero::settings().body_lift_multiplier
                * props.body_lift_multiplier
                * self.body_lift_curves.lift_mach.evaluate(mach));
        force - travel * force.dot(travel)
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
        self.lifting_fx_local(Airflow::uniform(frame, v_air, rho, mach))
    }

    fn mounted_offset(&self, i: usize, offset: DVec3) -> DVec3 {
        crate::rigid::to_vessel(aero::rotate(self.part_rot[i], offset))
    }
    fn lift_offset(&self, i: usize) -> DVec3 {
        self.mounted_offset(
            i,
            self.lift_defs[i]
                .as_ref()
                .map(|(l, _)| {
                    l.reference_frame
                        .as_ref()
                        .map(|f| f.point(l.pressure_offset_m))
                        .unwrap_or(l.pressure_offset_m)
                })
                .unwrap_or(DVec3::ZERO),
        )
    }
    fn drag_offset(&self, i: usize) -> DVec3 {
        // Module_Drag adds raw module CoPOffset in the part frame, independently of
        // Module_LiftingSurface's optional named reference transform.
        self.mounted_offset(i, self.pressure_offsets_m[i] + self.raw_surface_offset(i))
    }
    fn body_lift_offset(&self, i: usize) -> DVec3 {
        self.mounted_offset(i, self.lift_offsets_m[i] + self.raw_surface_offset(i))
    }
    fn raw_surface_offset(&self, i: usize) -> DVec3 {
        self.lift_defs[i]
            .as_ref()
            .map(|(l, _)| l.pressure_offset_m)
            .unwrap_or(DVec3::ZERO)
    }
    fn control_offset(&self, i: usize) -> DVec3 {
        self.mounted_offset(
            i,
            self.pressure_offsets_m[i] + self.control_pressure_offsets_m[i],
        )
    }
    fn air_at_offset(&self, i: usize, offset: DVec3, flow: Airflow, centre: DVec3) -> DVec3 {
        let r = self.rigid.rest_position(i) + offset - centre;
        let (x, y, z) = flow.frame;
        flow.velocity + flow.angular_velocity.cross(x * r.x + y * r.y + z * r.z)
    }
    fn part_air_velocity(&self, i: usize, flow: Airflow, centre: DVec3) -> DVec3 {
        self.air_at_offset(i, self.drag_offset(i), flow, centre)
    }

    pub fn lifting_fx_local(&self, flow: Airflow) -> Vec<(u16, f32, f32)> {
        if flow.density <= 0.0 {
            return Vec::new();
        }
        let centre = self.mass_properties().centre_m;
        self.lift_defs
            .iter()
            .enumerate()
            .filter_map(|(i, def)| {
                let (surface, _) = def.as_ref()?;
                if !self.lifting_surface_active(i)
                    || (surface.deflection_lift_coeff <= 0.0
                        && self.geometry[i]
                            .surface
                            .as_ref()
                            .and_then(|s| s.moving_lift_coeff)
                            .unwrap_or(0.)
                            <= 0.)
                {
                    return None;
                }
                let v = self.air_at_offset(i, self.lift_offset(i), flow, centre);
                let speed = v.length();
                if speed <= 0.0 {
                    return None;
                }
                let travel = v * (1.0 / speed);
                let force = self.lifting_force(
                    i,
                    flow.frame,
                    travel,
                    0.5 * flow.density * speed * speed,
                    flow.mach(speed),
                );
                let lift = force - travel * force.dot(travel);
                Some((
                    u16::try_from(i).ok()?,
                    surface.deflection_lift_coeff.max(
                        self.geometry[i]
                            .surface
                            .as_ref()
                            .and_then(|s| s.moving_lift_coeff)
                            .unwrap_or(0.),
                    ) as f32,
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
            .filter(|&i| self.rigid.present[i] && !self.shielded[i])
            .flat_map(|i| {
                [
                    PartForce {
                        part: i,
                        force_kn: -travel * (q * self.part_cda_m2[i] / 1000.0),
                        offset_m: self.drag_offset(i),
                    },
                    PartForce {
                        part: i,
                        force_kn: self.lifting_force(i, frame, travel, q, mach),
                        offset_m: self.lift_offset(i),
                    },
                    PartForce {
                        part: i,
                        force_kn: self.body_lifting_force(i, frame, travel, q, mach),
                        offset_m: self.body_lift_offset(i),
                    },
                ]
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
        self.aerodynamics_local(stage, Airflow::uniform(frame, v_air, rho, mach))
    }

    /// Forces at each part's local velocity, including rotation about the current mass centre.
    pub fn aerodynamics_local(&mut self, stage: usize, flow: Airflow) -> AeroForces {
        let cda_m2 = self.aero_local(stage, flow);
        self.part_aero_kn.fill((DVec3::ZERO, DVec3::ZERO));
        let centre = self.mass_properties().centre_m;
        let mut forces = Vec::new();
        let mut out = AeroForces {
            cda_m2,
            ..Default::default()
        };
        for i in 0..self.rigid.len() {
            if !self.rigid.present[i] || self.shielded[i] || flow.density <= 0.0 {
                continue;
            }
            let v = self.part_air_velocity(i, flow, centre);
            let speed = v.length();
            let drag = if speed > 0.0 {
                -v * (0.5 * flow.density * speed * self.part_cda_m2[i] / 1000.0)
            } else {
                DVec3::ZERO
            };
            let wing_v = self.air_at_offset(i, self.lift_offset(i), flow, centre);
            let wing_speed = wing_v.length();
            let lift = if wing_speed > 0.0 {
                self.lifting_force(
                    i,
                    flow.frame,
                    wing_v * (1.0 / wing_speed),
                    0.5 * flow.density * wing_speed * wing_speed,
                    flow.mach(wing_speed),
                )
            } else {
                DVec3::ZERO
            };
            let body_v = self.air_at_offset(i, self.body_lift_offset(i), flow, centre);
            let body_speed = body_v.length();
            let body_lift = if body_speed > 0.0 {
                self.body_lifting_force(
                    i,
                    flow.frame,
                    body_v * (1.0 / body_speed),
                    0.5 * flow.density * body_speed * body_speed,
                    flow.mach(body_speed),
                )
            } else {
                DVec3::ZERO
            };
            self.part_aero_kn[i] = (lift + body_lift, drag);
            out.drag_kn = out.drag_kn + drag;
            out.lift_kn = out.lift_kn + lift + body_lift;
            forces.push(PartForce {
                part: i,
                force_kn: drag,
                offset_m: self.drag_offset(i),
            });
            forces.push(PartForce {
                part: i,
                force_kn: lift,
                offset_m: self.lift_offset(i),
            });
            forces.push(PartForce {
                part: i,
                force_kn: body_lift,
                offset_m: self.body_lift_offset(i),
            });
        }
        out.torque_knm = self.airflow_moment(flow.frame, &forces);
        self.resolved_aero_forces = Some(forces);
        out
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
                        && self.lifting_surface_active(i)
                        && range > 0.0
                        && share > 0.0)
                        .then_some((i, self.surface_angles[i]));
                }
                let (l, _) = self.lift_defs[i].as_ref()?;
                if range <= 0.0 || share <= 0.0 || !self.lifting_surface_active(i) {
                    return None;
                }
                let u = aero::rotate(self.part_rot[i], l.axis);
                let axis = bx * u.x + by * u.y + bz * (-u.z);
                let centre = self.rigid.rest_position(i) + self.control_offset(i);
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

    /// How far each engine of flight stage `stage` gimbals for steering command `cmd` (as
    /// `surface_deflections`): (part, about its own X, about its own Z), rad, each up to its gimbal range,
    /// turned so the thrust's torque helps the command.
    pub fn gimbal_deflections(
        &self,
        stage: usize,
        facing: DVec3,
        cmd: DVec3,
    ) -> Vec<(usize, f64, f64)> {
        let c = self.rigid.rest_centre();
        let engines = self.stage_engines.get(stage).cloned().unwrap_or_default();
        engines
            .into_iter()
            .filter(|&i| self.rigid.present[i] && self.gimbal_range_rad[i] > 0.0)
            .map(|i| {
                let q = self.part_rot[i];
                let thrust = self.axis_inertial(facing, aero::rotate(q, DVec3::new(0.0, 1.0, 0.0)));
                let lever = self.offset_inertial(facing, self.rigid.rest_position(i) - c);
                let range = self.gimbal_range_rad[i];
                let about = |a: DVec3| {
                    // tilting the thrust about `a` pushes it sideways along a x thrust
                    let a = self.axis_inertial(facing, aero::rotate(q, a));
                    let torque = lever.cross(a.cross(thrust));
                    if torque.length() > 1e-6 {
                        cmd.dot(torque.normalized()).clamp(-1.0, 1.0) * range
                    } else {
                        0.0
                    }
                };
                (
                    i,
                    about(DVec3::new(1.0, 0.0, 0.0)),
                    about(DVec3::new(0.0, 0.0, 1.0)),
                )
            })
            .collect()
    }

    /// How much torque the control surfaces can give at full deflection in this airflow, kN·m: each moving
    /// surface's extra lift at its deflection range (dynamic pressure × 0.036 × lift coefficient × moving share
    /// × the lift curve at the range × its Mach curve) at its distance from the centre of mass.
    pub fn control_authority(&self, v_air: DVec3, rho: f64, mach: f64) -> f64 {
        self.control_authority_local(Airflow::uniform(
            basis(DVec3::new(0.0, 1.0, 0.0)),
            v_air,
            rho,
            mach,
        ))
    }

    pub fn control_authority_local(&self, flow: Airflow) -> f64 {
        if flow.density <= 0.0 {
            return 0.0;
        }
        let c = self.mass_properties().centre_m;
        let mut total = 0.0;
        for i in 0..self.rigid.len() {
            let (range, share) = self.ctrl[i];
            let Some((_l, curves)) = &self.lift_defs[i] else {
                continue;
            };
            if range <= 0.0 || share <= 0.0 || !self.lifting_surface_active(i) {
                continue;
            }
            let speed = self
                .air_at_offset(i, self.control_offset(i), flow, c)
                .length();
            let q = 0.5 * flow.density * speed * speed;
            let mach = flow.mach(speed);
            let lift = q
                * aero::settings().lift_multiplier
                * self.control_lift_coeff(i)
                * curves
                    .lift
                    .evaluate((range * self.controls[i].surface_authority).sin())
                * curves.lift_mach.evaluate(mach);
            let centre = self.rigid.rest_position(i) + self.control_offset(i);
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
        self.aero_local(stage, Airflow::uniform(frame, v_air, rho, mach))
    }

    fn aero_local(&mut self, stage: usize, flow: Airflow) -> f64 {
        if stage != self.staged_to {
            self.trim_to(stage);
        }
        self.last_air_speed_mps = flow.velocity.length();
        self.last_airflow = Some(flow);
        self.part_cda_m2.fill(0.0);
        if flow.density <= 0.0 {
            return 0.0;
        }
        let centre = self.mass_properties().centre_m;
        let (bx, by, bz) = flow.frame;
        let mut total = 0.0;
        for i in 0..self.rigid.len() {
            if !self.rigid.present[i] || self.shielded[i] {
                continue;
            }
            let v = self.part_air_velocity(i, flow, centre);
            let speed = v.length();
            if speed <= 0.0 {
                continue;
            }
            let d = v * (1.0 / speed);
            let du = DVec3::new(d.dot(bx), d.dot(by), -d.dot(bz));
            let mach = flow.mach(speed);
            let rho = flow.density;
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
            let cda = if self.drag_properties[i].drag_enabled {
                aero::cda_m2(&self.curves, area_drag, rho * speed)
            } else {
                0.0
            };
            self.part_cda_m2[i] = cda;
            total += cda;
        }
        // Fairing drag uses the local airflow at the base part, as do its structural loads.
        for (part, front, side) in self.fairing_shells() {
            if self.shielded[part] || !self.drag_properties[part].drag_enabled {
                continue;
            }
            let v = self.part_air_velocity(part, flow, centre);
            let speed = v.length();
            if speed <= 0.0 {
                continue;
            }
            let along = (v.dot(by) / speed).abs().min(1.0);
            let across = (1.0 - along * along).max(0.0).sqrt();
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
        let loaded = actuators.as_ref().is_some_and(|channels| {
            channels.iter().zip(&out.actuator_inputs).any(|(a, input)| {
                (a.force_kn * *input).length() >= QUIET_KN
                    || (a.torque_knm * *input).length() >= QUIET_KN
            })
        }) || !surface_forces.is_empty()
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
        let fuel_resolved = resolved.is_some();
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
        // Fuel-resolved flight and the structure must receive the same delivered force this tick.
        // A separate structural filter would invent a thrust tail after a partial/empty burn.
        // Legacy stage-only callers retain their existing response proxy.
        let k = if fuel_resolved {
            1.0
        } else {
            1.0 - (-dt / THRUST_LAG_S).exp()
        };
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
                spin: DVec3::ZERO,
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
        colors: None,
        loose: Vec::new(),
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

    fn airflow_fixture() -> (Structure, Powered, usize) {
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let mut s = Structure::build(craft, &defs, &res);
        for props in &mut s.drag_properties {
            props.drag_enabled = true;
            props.body_lift_enabled = false;
        }
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
        s.ctrl.fill((0.0, 0.0));
        s.ctrl[wing] = (0.2, 1.0);
        s.part_rot[wing] = [0.0, 0.0, 0.0, 1.0];
        (s, flying(DVec3::new(0.0, 1.0, 0.0)), wing)
    }

    #[test]
    fn spinning_surfaces_share_local_flow_with_control_authority_and_wing_effects() {
        let (mut s, mut p, wing) = airflow_fixture();
        s.pressure_offsets_m[wing] = DVec3::ZERO;
        s.control_pressure_offsets_m[wing] = DVec3::ZERO;
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(3.0, 5.0, 7.0),
            density: 1.0,
            speed_of_sound: 100.0,
        };
        let velocity = s.air_at_offset(
            wing,
            s.lift_offset(wing),
            flow,
            s.mass_properties().centre_m,
        );
        let travel = velocity.normalized();
        let expected = s.lifting_force(
            wing,
            flow.frame,
            travel,
            0.5 * velocity.length_squared(),
            flow.mach(velocity.length()),
        );
        assert!(expected.length() > 0.01);
        let forces = s.aerodynamics_local(0, flow);
        assert!((forces.lift_kn - expected).length() < 1e-10);
        let fx = s.lifting_fx_local(flow);
        let perpendicular = expected - travel * expected.dot(travel);
        assert!((fx[0].2 as f64 - perpendicular.length()).abs() < 1e-4);
        assert!(s.control_authority_local(flow) > 0.0);
        s.wheel_axes_knm.fill(DVec3::ZERO);
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
        assert!(s.lifting_fx_local(flow).is_empty());
        assert_eq!(s.control_authority_local(flow), 0.0);
    }

    #[test]
    fn local_heating_uses_authored_point_velocity_and_shielding() {
        let (mut s, p, wing) = airflow_fixture();
        s.rigid.present.fill(false);
        s.rigid.present[wing] = true;
        s.net.present.fill(false);
        s.net.present[wing] = true;
        s.lift_defs[wing].as_mut().unwrap().0.pressure_offset_m = DVec3::ZERO;
        let offset =
            s.mass_properties().centre_m - s.rigid.rest_position(wing) + DVec3::new(2.0, 0.0, 0.0);
        s.pressure_offsets_m[wing] = crate::rigid::to_vessel(offset);
        s.faces[wing] = Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        });
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
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(0.0, 0.0, 10.0),
            density: 1.0,
            speed_of_sound: 1.0,
        };
        s.update_heat_in_flow(flow, 288.0, 0.0, &thermal::HeatSettings::default(), 0.02);
        let expected = thermal::reentry_flux(20.0, 1.0, 1.0, 1.0, 1.0);
        assert!(expected > 0.0);
        assert!((s.reentry_flux_kw[wing] - expected).abs() < 1e-8);
        let rolled = Airflow {
            frame: (flow.frame.2, flow.frame.1, -flow.frame.0),
            angular_velocity: flow.frame.0 * -10.0,
            ..flow
        };
        s.update_heat_in_flow(rolled, 288.0, 0.0, &thermal::HeatSettings::default(), 0.02);
        assert!((s.reentry_flux_kw[wing] - expected).abs() < 1e-8);
        s.shielded[wing] = true;
        s.update_heat_in_flow(flow, 288.0, 0.0, &thermal::HeatSettings::default(), 0.02);
        assert_eq!(s.reentry_flux_kw[wing], 0.0);
    }

    #[test]
    fn named_lifting_frame_rotates_force_and_scaled_pressure_point_under_mount_and_roll() {
        let (mut s, _, wing) = airflow_fixture();
        let h = 0.5_f64.sqrt();
        s.part_rot[wing] = [0.0, 0.0, h, h];
        let surface = &mut s.lift_defs[wing].as_mut().unwrap().0;
        surface.axis = DVec3::new(1.0, 0.0, 0.0);
        surface.pressure_offset_m = DVec3::new(1.0, 2.0, 3.0);
        surface.transform_name = "aero".into();
        surface.reference_frame = Some(ose_data::SurfaceFrame {
            position_m: DVec3::new(1.0, 2.0, 3.0),
            point_axes: [
                DVec3::new(0.0, 2.0, 0.0),
                DVec3::new(-3.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 4.0),
            ],
            direction_axes: [
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(-1.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ],
        });
        // Reference Rz(90) then mount Rz(90): +X becomes craft -X. Under a 90-degree
        // roll this is inertial +Z. Scale affects the pressure point, never this unit axis.
        let frame = (
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
        );
        let air = DVec3::new(0.0, 100.0, -10.0);
        let expected_force = DVec3::new(
            0.0,
            0.0,
            0.5 * air.length_squared() * aero::LIFT_MULTIPLIER * 10.0 / air.length(),
        );
        let offset = DVec3::new(-4.0, -5.0, -15.0);
        assert!((s.lift_offset(wing) - offset).length() < 1e-12);
        let out = s.aerodynamics(0, frame, air, 1.0, 0.3);
        assert!((out.lift_kn - expected_force).length() < 1e-9);
        let lever = s.rigid.rest_position(wing) + offset - s.mass_properties().centre_m;
        let world_lever = frame.0 * lever.x + frame.1 * lever.y + frame.2 * lever.z;
        assert!((out.torque_knm - world_lever.cross(expected_force)).length() < 1e-8);
        let forces = s.resolved_aero_forces.as_ref().unwrap();
        assert!(forces.iter().any(|f| f.part == wing
            && (f.offset_m - offset).length() < 1e-12
            && (f.force_kn - expected_force).length() < 1e-9));
        let fx = s.lifting_fx(frame, air, 1.0, 0.3);
        let perpendicular =
            expected_force - air.normalized() * expected_force.dot(air.normalized());
        assert!((fx[0].2 as f64 - perpendicular.length()).abs() < 1e-5);
        // Module_Drag's offsets stay part-local; a named surface transform cannot move them.
        s.pressure_offsets_m[wing] = DVec3::new(2.0, 0.0, 0.0);
        s.lift_offsets_m[wing] = DVec3::new(0.0, 4.0, 0.0);
        assert!((s.drag_offset(wing) - DVec3::new(-2.0, 3.0, -3.0)).length() < 1e-12);
        assert!((s.body_lift_offset(wing) - DVec3::new(-6.0, 1.0, -3.0)).length() < 1e-12);
    }

    #[test]
    fn named_lifting_point_drives_local_airflow_and_damping_at_zero_bulk_speed() {
        let (mut s, p, wing) = airflow_fixture();
        let at =
            s.mass_properties().centre_m - s.rigid.rest_position(wing) + DVec3::new(2.0, 0.0, 0.0);
        let surface = &mut s.lift_defs[wing].as_mut().unwrap().0;
        surface.axis = DVec3::new(1.0, 0.0, 0.0);
        surface.pressure_offset_m = DVec3::ZERO;
        surface.reference_frame = Some(ose_data::SurfaceFrame {
            position_m: crate::rigid::to_vessel(at),
            point_axes: [
                DVec3::new(0.0, 2.0, 0.0),
                DVec3::new(-3.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 4.0),
            ],
            direction_axes: [
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(-1.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ],
        });
        let flow = Airflow {
            frame: crate::flight::vessel_basis(&p),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(0.0, 0.0, 1.0),
            density: 1.0,
            speed_of_sound: 100.0,
        };
        let out = s.aerodynamics_local(0, flow);
        assert!((out.lift_kn - DVec3::new(0.0, -0.072, 0.0)).length() < 1e-10);
        assert!((out.torque_knm - DVec3::new(0.0, 0.0, -0.144)).length() < 1e-10);
        assert!(out.torque_knm.dot(flow.angular_velocity) < 0.0);
    }

    #[test]
    fn occupied_lift_nodes_gate_surfaces_and_controls_until_every_peer_detaches() {
        let flow = Airflow::uniform(
            basis(DVec3::new(0.0, 1.0, 0.0)),
            DVec3::new(0.0, 100.0, -10.0),
            1.0,
            0.3,
        );
        for (own, child) in [(true, false), (false, true), (true, true)] {
            let (mut s, mut p) = lift_node_fixture(true, "lift-gate", own, child);
            let env = crate::flight::EngineEnvironment {
                density: 1.0,
                mach: 0.3,
                ..Default::default()
            };
            let blocked = s.aerodynamics_local(0, flow);
            assert_eq!(blocked.lift_kn, DVec3::ZERO);
            assert!(
                blocked.drag_kn.length() > 0.0,
                "the independent drag cube must remain active"
            );
            assert!(s.lifting_fx_local(flow).is_empty());
            assert!(s
                .surface_deflections(flow.frame, DVec3::new(1.0, 0.0, 0.0))
                .is_empty());
            assert_eq!(s.control_authority_local(flow), 0.0);
            let mut engine = s.propulsion(&p, env, 0.02);
            s.configure_actuators(&p, env, &mut engine);
            assert!(!engine
                .actuators
                .unwrap()
                .iter()
                .any(|a| a.part == 1 && a.kind == crate::actuators::Kind::Other));
            let mut peers = Vec::new();
            if own {
                peers.push(0);
            }
            if child {
                peers.push(2);
            }
            for (step, peer) in peers.iter().enumerate() {
                s.rigid.remove_parts(&[*peer]);
                s.net.remove_parts(&[*peer]);
                let lift = s.aerodynamics_local(0, flow).lift_kn;
                if step + 1 < peers.len() {
                    assert_eq!(
                        lift,
                        DVec3::ZERO,
                        "another connection still occupies the gated node"
                    );
                } else {
                    assert!(lift.length() > 1.0);
                }
            }
            assert_eq!(s.lifting_fx_local(flow).len(), 1);
            assert_eq!(
                s.surface_deflections(flow.frame, DVec3::new(1.0, 0.0, 0.0))
                    .len(),
                1
            );
            s.sync(&mut p);
            let mut engine = s.propulsion(&p, env, 0.02);
            s.configure_actuators(&p, env, &mut engine);
            assert!(engine
                .actuators
                .unwrap()
                .iter()
                .any(|a| a.part == 1 && a.kind == crate::actuators::Kind::Other));
        }
    }

    #[test]
    fn disabled_missing_and_unoccupied_lift_nodes_preserve_the_analytical_surface_force() {
        let air = DVec3::new(0.0, 100.0, -10.0);
        let flow = Airflow::uniform(basis(DVec3::new(0.0, 1.0, 0.0)), air, 1.0, 0.3);
        let expected = DVec3::new(
            0.0,
            0.0,
            0.5 * air.length_squared() * aero::LIFT_MULTIPLIER * 10.0 / air.length(),
        ) - air.normalized()
            * (0.5 * air.length_squared() * aero::LIFT_DRAG_MULTIPLIER * 0.4);
        for (enabled, name, own, child) in [
            (false, "lift-gate", true, true),
            (true, "missing", true, true),
            (true, "", true, true),
            (true, "lift-gate", false, false),
        ] {
            let (mut s, _) = lift_node_fixture(enabled, name, own, child);
            let out = s.aerodynamics_local(0, flow);
            assert!(
                (out.lift_kn - expected).length() < 1e-9,
                "{enabled}/{name}: {:?}",
                out.lift_kn
            );
            assert_eq!(s.lifting_fx_local(flow).len(), 1);
            assert!(s.control_authority_local(flow) > 0.0);
        }
    }

    fn lift_node_fixture(
        enabled: bool,
        node_name: &str,
        own: bool,
        child: bool,
    ) -> (Structure, Powered) {
        let (base, _, wing) = airflow_fixture();
        let (crafts, mut defs, resources) = assets();
        let source = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let mut wing_part = source.parts[wing].clone();
        let def = defs.iter_mut().find(|d| d.id == wing_part.part_id).unwrap();
        let (mut surface, curves) = base.lift_defs[wing].clone().unwrap();
        surface.node_enabled = enabled;
        surface.attach_node_name = node_name.into();
        surface.pressure_offset_m = DVec3::ZERO;
        surface.curve_set = "Custom".into();
        surface.lift_curve = curves.lift;
        surface.lift_mach_curve = curves.lift_mach;
        surface.drag_curve = curves.drag;
        surface.use_internal_drag_model = true;
        surface.drag_curve.keys[0].v = 0.4;
        surface.drag_mach_curve = curves.drag_mach;
        def.lifting_surface = Some(surface);
        let mut gate = def.attach_nodes[0].clone();
        gate.id = "lift-gate".into();
        gate.node_type = "Surface".into();
        let mut other = gate.clone();
        other.id = "other".into();
        def.attach_nodes = vec![gate, other];
        def.ctrl_range_deg = 0.2_f64.to_degrees();
        def.ctrl_area = 1.0;
        let mut parent = source.parts[0].clone();
        parent.parent = None;
        parent.local_position = DVec3::ZERO;
        let mut peer = parent.clone();
        peer.guid = "node-child".into();
        peer.parent = Some(1);
        peer.parent_node = Some(if child { "lift-gate" } else { "other" }.into());
        peer.local_position = DVec3::new(0.0, 8.0, 0.0);
        wing_part.parent = Some(0);
        wing_part.own_node = Some(if own { "lift-gate" } else { "other" }.into());
        wing_part.local_position = DVec3::new(0.0, 3.0, 0.0);
        let mut parts = vec![parent, wing_part, peer];
        for part in &mut parts {
            part.resources.clear();
            part.stage_index = 0;
            part.local_rotation = ose_data::craft::Quat {
                w: 1.0,
                ..Default::default()
            };
        }
        let craft = Craft {
            name: "Node gating".into(),
            description: String::new(),
            parts,
            stages: vec![Vec::new()],
            colors: Default::default(),
            loose: Vec::new(),
        };
        let mut s = Structure::build(&craft, &defs, &resources);
        s.faces.fill(None);
        s.faces[1] = Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        });
        for props in &mut s.drag_properties {
            props.body_lift_enabled = false;
            props.drag_enabled = true;
        }
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.rcs_defs.fill(None);
        s.shielded.fill(false);
        let mut p = flying(DVec3::new(0.0, 1.0, 0.0));
        p.ignited = false;
        (s, p)
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
        let (lift, drag) = s.part_aero_kn[wing];
        let resolved = s
            .resolved_aero_forces
            .as_ref()
            .unwrap()
            .iter()
            .filter(|f| f.part == wing)
            .fold(DVec3::ZERO, |sum, f| sum + f.force_kn);
        assert!((lift + drag - resolved).length() < 1e-10);
        assert!(drag.dot(air) < 0.0);
        s.shielded[wing] = true;
        let out = s.aerodynamics(0, frame, air, 1.0, 0.3);
        assert_eq!(out.cda_m2, 0.0);
        assert_eq!(out.lift_kn, DVec3::ZERO);
        assert_eq!(out.torque_knm, DVec3::ZERO);
        assert_eq!(s.part_aero_kn[wing], (DVec3::ZERO, DVec3::ZERO));
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
    fn distinct_authored_drag_and_lift_points_retain_their_moments_under_mount_rotation() {
        let (mut s, _, wing) = airflow_fixture();
        s.faces[wing] = Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        });
        let h = 0.5_f64.sqrt();
        s.part_rot[wing] = [0.0, h, 0.0, h];
        s.pressure_offsets_m[wing] = DVec3::new(2.0, 0.0, 0.0);
        s.lift_defs[wing].as_mut().unwrap().0.pressure_offset_m = DVec3::new(0.0, 1.0, 0.0);
        let frame = (
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(0.0, 0.0, 1.0),
        );
        let air = DVec3::new(10.0, 100.0, -10.0);
        let out = s.aerodynamics(0, frame, air, 1.0, 0.3);
        let centre = s.mass_properties().centre_m;
        let origin = s.rigid.rest_position(wing) - centre;
        // Unity +X rotates to -Z, then the craft handedness conversion mirrors Z.
        let drag_offset = DVec3::new(0.0, 1.0, 2.0);
        let lift_offset = DVec3::new(0.0, 1.0, 0.0);
        let expected =
            (origin + drag_offset).cross(out.drag_kn) + (origin + lift_offset).cross(out.lift_kn);
        assert!((out.torque_knm - expected).length() < 1e-8);
        let forces = s.resolved_aero_forces.as_ref().unwrap();
        assert!(forces.iter().any(|f| f.part == wing
            && (f.offset_m - drag_offset).length() < 1e-12
            && (f.force_kn - out.drag_kn).length() < 1e-12));
        assert!(forces.iter().any(|f| f.part == wing
            && (f.offset_m - lift_offset).length() < 1e-12
            && (f.force_kn - out.lift_kn).length() < 1e-12));
    }

    #[test]
    fn perpendicular_lift_and_internal_drag_options_match_the_reference_projection() {
        let (s, _, wing) = airflow_fixture();
        let (mut surface, mut curves) = s.lift_defs[wing].clone().unwrap();
        curves.drag = ose_data::Curve {
            keys: vec![ose_data::CurveKey {
                t: 0.0,
                v: 2.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            }],
        };
        let axis = DVec3::new(0.0, 0.0, 1.0);
        let direction = DVec3::new(0.0, 0.8, 0.6);
        let force = aero::surface_force(&curves, &surface, axis, direction, 1000.0, 0.3);
        let lift = axis * (-1000.0 * aero::LIFT_MULTIPLIER * 0.6);
        let drag = direction * (-1000.0 * aero::LIFT_DRAG_MULTIPLIER * 2.0);
        assert!((force - lift - drag).length() < 1e-10);
        surface.perpendicular_only = true;
        let projected = aero::surface_force(&curves, &surface, axis, direction, 1000.0, 0.3);
        assert!((projected - (lift - direction * lift.dot(direction)) - drag).length() < 1e-10);
        surface.use_internal_drag_model = false;
        let without_drag = aero::surface_force(&curves, &surface, axis, direction, 1000.0, 0.3);
        assert!(without_drag.dot(direction).abs() < 1e-10);
        assert!((without_drag - lift + direction * lift.dot(direction)).length() < 1e-10);
    }

    #[test]
    fn local_airflow_uses_each_distinct_force_point_including_zero_centre_airspeed() {
        let (mut s, _, wing) = airflow_fixture();
        s.net.dry_t.fill(0.0);
        s.net.dry_t[wing] = 1.0;
        s.net.tanks.clear();
        s.part_rot[wing] = [0.0, 0.0, 0.0, 1.0];
        let surface = &mut s.lift_defs[wing].as_mut().unwrap().0;
        surface.axis = DVec3::new(0.0, 1.0, 0.0);
        surface.pressure_offset_m = DVec3::new(1.0, 0.0, 0.0);
        s.pressure_offsets_m[wing] = DVec3::new(1.0, 0.0, 0.0);
        s.faces[wing] = Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        });
        let constant = ose_data::Curve {
            keys: vec![ose_data::CurveKey {
                t: 0.0,
                v: 1.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            }],
        };
        s.curves = aero::DragCurves {
            tip: constant.clone(),
            surface: constant.clone(),
            tail: constant.clone(),
            multiplier: constant.clone(),
            cd: constant.clone(),
            cd_power: constant.clone(),
            pseudo_reynolds: constant,
        };
        let flow = Airflow {
            frame: (
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::new(0.0, 0.0, 1.0),
            density: 1.0,
            speed_of_sound: 100.0,
        };
        let out = s.aerodynamics_local(0, flow);
        // Point speeds are 2 m/s for body drag and 1 m/s for the wing.
        assert!((out.drag_kn - DVec3::new(0.0, -0.0096, 0.0)).length() < 1e-12);
        assert!((out.lift_kn - DVec3::new(0.0, -0.018, 0.0)).length() < 1e-12);
        assert!((out.torque_knm - DVec3::new(0.0, 0.0, -0.0372)).length() < 1e-12);
    }

    #[test]
    fn drag_cube_body_lift_matches_projected_face_forces_and_uses_its_own_lift_point() {
        let (mut s, _, wing) = airflow_fixture();
        s.net.dry_t.fill(0.0);
        s.net.dry_t[wing] = 1.0;
        s.net.tanks.clear();
        s.part_rot[wing] = [0.0, 0.0, 0.0, 1.0];
        s.lift_defs.fill(None);
        s.faces[wing] = Some(Faces {
            area: [2.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            drag: [1.0; 6],
        });
        s.drag_properties[wing] = ose_data::DragProperties {
            drag_enabled: false,
            body_lift_enabled: true,
            body_lift_multiplier: 1.0,
        };
        s.lift_offsets_m[wing] = DVec3::new(0.0, 0.0, 2.0);
        let frame = (
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(0.0, 0.0, 1.0),
        );
        let a = std::f64::consts::FRAC_1_SQRT_2;
        let air = DVec3::new(a, a, 0.0) * 1000.0_f64.sqrt();
        let expected = DVec3::new(-0.5 * a, 0.5 * a, 0.0) * (18.0 * 0.167);
        let out = s.aerodynamics(0, frame, air, 2.0, 0.3);
        assert_eq!(out.drag_kn, DVec3::ZERO);
        assert_eq!(out.cda_m2, 0.0);
        assert!((out.lift_kn - expected).length() < 1e-10);
        assert!(out.lift_kn.dot(air).abs() < 1e-10);
        assert!((out.torque_knm - DVec3::new(0.0, 0.0, -2.0).cross(expected)).length() < 1e-10);
        let fast = s.aerodynamics(0, frame, air, 2.0, 1.0);
        assert!((fast.lift_kn - expected * (0.125 / 0.167)).length() < 1e-10);
        s.drag_properties[wing].body_lift_multiplier = 2.0;
        assert!(
            (s.aerodynamics(0, frame, air, 2.0, 0.3).lift_kn - expected * 2.0).length() < 1e-10
        );
        s.shielded[wing] = true;
        assert_eq!(s.aerodynamics(0, frame, air, 2.0, 0.3).lift_kn, DVec3::ZERO);
        s.shielded[wing] = false;
        s.drag_properties[wing].body_lift_enabled = false;
        s.drag_properties[wing].drag_enabled = true;
        let drag_only = s.aerodynamics(0, frame, air, 2.0, 0.3);
        assert_eq!(drag_only.lift_kn, DVec3::ZERO);
        assert!(drag_only.drag_kn.dot(air) < 0.0);
        s.rigid.remove_parts(&[wing]);
        s.net.remove_parts(&[wing]);
        let removed = s.aerodynamics(0, frame, air, 2.0, 0.3);
        assert_eq!(removed.lift_kn, DVec3::ZERO);
        assert_eq!(removed.drag_kn, DVec3::ZERO);
    }

    #[test]
    fn named_speedbrake_family_does_not_fall_back_to_wing_lift() {
        let (crafts, mut defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        for def in &mut defs {
            if let Some(surface) = &mut def.lifting_surface {
                surface.curve_set = "SpeedBrake".into();
            }
        }
        let s = Structure::build(craft, &defs, &res);
        let frame = (
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(0.0, 0.0, 1.0),
        );
        let velocity = DVec3::new(0.0, 100.0, 10.0);
        let direction = velocity.normalized();
        let force = s.lift(frame, velocity, 1.0, 0.3);
        assert!(force.dot(direction) < 0.0);
        assert!((force - direction * force.dot(direction)).length() < 1e-9);
    }

    #[test]
    fn aerodynamic_moment_tracks_fuel_mass_and_retains_each_parts_force() {
        let (mut s, p, wing) = airflow_fixture();
        let frame = crate::flight::vessel_basis(&p);
        let air = DVec3::new(0.0, 100.0, -10.0);
        let first = s.aerodynamics(0, frame, air, 1.0, 0.3);
        let c0 = s.mass_properties().centre_m;
        let loads = s.resolved_aero_forces.as_ref().unwrap();
        let force = loads
            .iter()
            .filter(|f| f.part == wing)
            .fold(DVec3::ZERO, |sum, f| sum + f.force_kn);
        assert!((force - first.lift_kn).length() < 1e-10);
        assert!(
            (first.torque_knm - (s.rigid.rest_position(wing) - c0).cross(first.lift_kn)).length()
                < 1e-9
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
    fn local_wing_flow_produces_analytical_damping_and_matching_effects() {
        let (mut s, p, wing) = airflow_fixture();
        let frame = crate::flight::vessel_basis(&p);
        let r = s.part_box(wing).0 - s.mass_properties().centre_m;
        let axis = DVec3::new(0.0, 0.0, 1.0);
        let spin = r.cross(axis).normalized();
        let v = spin.cross(r);
        let speed = v.length();
        assert!(speed > 0.1);
        let flow = Airflow {
            frame,
            velocity: DVec3::ZERO,
            angular_velocity: spin,
            density: 1.0,
            speed_of_sound: 340.0,
        };
        let out = s.aerodynamics_local(0, flow);
        let expected = axis * (-0.5 * speed * v.z * aero::LIFT_MULTIPLIER);
        assert!((out.lift_kn - expected).length() < 1e-10);
        assert!((out.torque_knm - r.cross(expected)).length() < 1e-10);
        assert!(out.torque_knm.dot(spin) < 0.0);
        let fx = s.lifting_fx_local(flow);
        let perpendicular = expected - v.normalized() * expected.dot(v.normalized());
        assert!((fx[0].2 as f64 - perpendicular.length()).abs() < 1e-5);
        s.shielded[wing] = true;
        let out = s.aerodynamics_local(0, flow);
        assert_eq!(out.lift_kn, DVec3::ZERO);
        assert_eq!(out.torque_knm, DVec3::ZERO);
        assert!(s.lifting_fx_local(flow).is_empty());
    }

    #[test]
    fn local_drag_uses_local_mach_and_has_nonpositive_total_power() {
        let (mut s, p, wing) = airflow_fixture();
        s.lift_defs.fill(None);
        s.faces[wing] = Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        });
        let frame = crate::flight::vessel_basis(&p);
        let flow = Airflow {
            frame,
            velocity: DVec3::new(2.0, 1.0, 0.0),
            angular_velocity: DVec3::new(2.0, -1.0, 3.0),
            density: 1.0,
            speed_of_sound: 10.0,
        };
        let centre = s.mass_properties().centre_m;
        let v = s.part_air_velocity(wing, flow, centre);
        let uniform = s.aerodynamics(0, frame, v, flow.density, v.length() / flow.speed_of_sound);
        let out = s.aerodynamics_local(0, flow);
        assert!((out.drag_kn - uniform.drag_kn).length() < 1e-10);
        let power = out.drag_kn.dot(flow.velocity) + out.torque_knm.dot(flow.angular_velocity);
        assert!(power < 0.0);
        assert!((power - out.drag_kn.dot(v)).abs() < 1e-10);
        // Rotate the entire scenario: the force and torque must rotate with it.
        let turn = |v: DVec3| DVec3::new(v.z, v.y, -v.x);
        let rotated = s.aerodynamics_local(
            0,
            Airflow {
                frame: (turn(frame.0), turn(frame.1), turn(frame.2)),
                velocity: turn(flow.velocity),
                angular_velocity: turn(flow.angular_velocity),
                ..flow
            },
        );
        assert!((rotated.drag_kn - turn(out.drag_kn)).length() < 1e-10);
        assert!((rotated.torque_knm - turn(out.torque_knm)).length() < 1e-10);
        let vacuum = s.aerodynamics_local(
            0,
            Airflow {
                density: 0.0,
                ..flow
            },
        );
        assert_eq!(vacuum.drag_kn, DVec3::ZERO);
        assert_eq!(vacuum.torque_knm, DVec3::ZERO);
        assert!(s
            .part_aero_kn
            .iter()
            .all(|f| *f == (DVec3::ZERO, DVec3::ZERO)));
        assert!(s.resolved_aero_forces.as_ref().unwrap().is_empty());
    }

    #[test]
    fn rotating_atmosphere_does_not_damp_a_corotating_craft() {
        let (_, mut p, _) = airflow_fixture();
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let u = crate::Universe::with_bodies(&bodies);
        let body = u.body_entity("Kerbin").unwrap();
        let body = u.world.get::<crate::bodies::Body>(body).unwrap();
        let omega = DVec3::new(0.0, 0.0, std::f64::consts::TAU / body.rotation_period_s);
        p.v = omega.cross(p.r);
        p.angvel = omega - p.facing * omega.dot(p.facing);
        p.roll_rate = omega.dot(p.facing);
        let flow = Airflow::at(&p, body, None);
        assert_eq!(flow.velocity, DVec3::ZERO);
        assert_eq!(flow.angular_velocity, DVec3::ZERO);
        p.landed = true;
        p.angvel = DVec3::ZERO;
        p.roll_rate = 0.0;
        assert_eq!(Airflow::at(&p, body, None).angular_velocity, DVec3::ZERO);
    }

    #[test]
    fn local_drag_reduces_a_free_tumbles_kinetic_energy_over_time() {
        let (mut s, mut p, _) = airflow_fixture();
        s.lift_defs.fill(None);
        s.faces.fill(Some(Faces {
            area: [1.0; 6],
            drag: [1.0; 6],
        }));
        let mass = s.mass_properties();
        p.mass_t = mass.mass_t;
        p.v = DVec3::ZERO;
        p.angvel = DVec3::new(0.7, 0.0, 0.4);
        p.roll_rate = 0.2;
        p.attitude = crate::flight::Attitude::Free;
        p.controlled = false;
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let u = crate::Universe::with_bodies(&bodies);
        let body = u.body_entity("Kerbin").unwrap();
        let mut body = u.world.get::<crate::bodies::Body>(body).unwrap().clone();
        body.mu = 0.0;
        body.rotation_period_s = 0.0;
        p.r = DVec3::new(body.radius_m + 100_000.0, 0.0, 0.0);
        let energy = |p: &Powered| {
            let (x, y, z) = crate::flight::vessel_basis(p);
            let w = p.angvel + p.facing * p.roll_rate;
            let w = DVec3::new(w.dot(x), w.dot(y), w.dot(z));
            0.5 * (mass.mass_t * p.v.length_squared() + w.dot(mass.inertia.apply(w)))
        };
        let initial = energy(&p);
        let mut previous = initial;
        for _ in 0..500 {
            let flow = Airflow {
                frame: crate::flight::vessel_basis(&p),
                velocity: p.v,
                angular_velocity: p.angvel + p.facing * p.roll_rate,
                density: 1.0,
                speed_of_sound: 340.0,
            };
            let out = s.aerodynamics_local(0, flow);
            p.aero_drag_kn = Some(out.drag_kn);
            p.lift_kn = out.lift_kn;
            p.aero_torque_knm = out.torque_knm;
            crate::flight::integrate_with_propulsion(
                &mut p,
                &body,
                None,
                None,
                0.0,
                0.002,
                Some(crate::flight::Propulsion {
                    mass,
                    ..Default::default()
                }),
            );
            let now = energy(&p);
            assert!(now <= previous + initial * 1e-9, "{previous} -> {now}");
            previous = now;
        }
        assert!(previous < initial * 0.999, "{initial} -> {previous}");
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
    fn engines_gimbal_with_the_steering_up_to_their_range() {
        let (crafts, defs, res) = assets();
        let c = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let s = Structure::build(c, &defs, &res);
        let facing = DVec3::new(0.0, 0.0, 1.0);
        let still = s.gimbal_deflections(0, facing, DVec3::ZERO);
        assert!(!still.is_empty(), "the first stage gimbals");
        assert!(still.iter().all(|g| g.1 == 0.0 && g.2 == 0.0));
        let pitch = s.gimbal_deflections(0, facing, DVec3::new(1.0, 0.0, 0.0));
        for (i, x, z) in &pitch {
            let range = s.gimbal_range_rad[*i];
            assert!(x.abs() <= range + 1e-9 && z.abs() <= range + 1e-9);
            assert!(
                x.abs().max(z.abs()) > 0.5 * range,
                "{i}: {x} {z} of {range}"
            );
        }
    }

    #[test]
    fn control_surfaces_steer_harder_the_faster_the_plane_flies() {
        let (crafts, defs, res) = assets();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let c = level_layout(c).unwrap();
        let s = Structure::build(&c, &defs, &res);
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
        let d = s.surface_deflections(basis(facing), pitch);
        assert!(!d.is_empty());
        assert!(d.iter().any(|(_, a)| a.abs() > 0.01), "{d:?}");
        assert!(d.iter().all(|(_, a)| a.abs() <= 30f64.to_radians() + 1e-9));
        assert!(s
            .surface_deflections(basis(facing), DVec3::ZERO)
            .iter()
            .all(|(_, a)| *a == 0.0));
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
        let (broken, _) = s.impact(15.0, DVec3::new(1.0, 0.0, 0.0), None).unwrap();
        assert!(broken.is_empty());
        // a bit over the legs' 20 m/s: their 0.95 m of suspension takes it
        let (broken, _) = s.impact(21.0, DVec3::new(1.0, 0.0, 0.0), None).unwrap();
        assert!(broken.is_empty(), "the suspension soaks it up");
        s.deploy_gear(false);
        let (broken, _) = s.impact(15.0, DVec3::new(1.0, 0.0, 0.0), None).unwrap();
        assert_eq!(broken, vec![3], "the engine breaks");
        // with the engine gone the vessel stands on its tank and legs
        assert!(
            s.impact(200.0, DVec3::new(1.0, 0.0, 0.0), None).is_none(),
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
        let mut data: (Vec<Craft>, Vec<PartDef>, Vec<ResourceDef>) = (
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
        );
        // Analytical fixtures explicitly define their force points, bounds and inertia.
        for def in &mut data.1 {
            def.flight_geometry = Default::default();
        }
        data
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
        let engines: Vec<_> = s
            .live_engines(0)
            .into_iter()
            .filter(|&i| s.engine_mixture[i].as_deref() == Some("SolidFuel"))
            .take(2)
            .collect();
        assert_eq!(engines.len(), 2);
        let pair = [engines[0], engines[1]];
        // Synthetic analytical engines use exact +Y origin geometry.
        s.engine_nozzles.iter_mut().for_each(Vec::clear);
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

    #[test]
    fn grid_fin_deploys_before_unlocking_control_and_stowed_fin_has_no_lift() {
        let (crafts, _, res) = assets();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let mut part = crafts[0].parts[0].clone();
        part.part_id = "gridfin_1v_radial".into();
        part.parent = None;
        part.local_position = DVec3::ZERO;
        part.local_rotation = ose_data::craft::Quat {
            x: 0.,
            y: 0.,
            z: 0.,
            w: 1.,
        };
        part.resources.clear();
        let craft = Craft {
            name: "grid fin".into(),
            description: String::new(),
            parts: vec![part],
            stages: vec![],
            ..Default::default()
        };
        let mut s = Structure::build(&craft, &defs, &res);
        let mut p = flying(DVec3::new(0., 1., 0.));
        p.rotation_input = DVec3::new(1., 0., 0.);
        let frame = basis(p.facing);
        let travel = DVec3::new(0., 1., 0.);
        s.last_airflow = Some(Airflow::uniform(frame, travel * 50., 1.2, 0.15));
        assert_eq!(s.lifting_force(0, frame, travel, 1500., 0.15), DVec3::ZERO);
        assert_eq!(s.control_lift_coeff(0), 0.);
        p.part_controls = vec![(
            0,
            ose_data::flight_geometry::PartControl {
                surface_deploy: true,
                ..Default::default()
            },
        )];
        let prop = crate::flight::Propulsion {
            mass: s.mass_properties(),
            actuators: Some(vec![]),
            ..Default::default()
        };
        s.resolve_surface_geometry(&p, &prop, &mut Default::default(), 0.02);
        assert!((s.surface_deploy_angles[0].to_degrees() - 18.).abs() < 1e-8);
        assert_eq!(s.surface_angles[0], 0.);
        for _ in 1..9 {
            s.resolve_surface_geometry(&p, &prop, &mut Default::default(), 0.02);
        }
        assert!(
            (s.surface_deploy_angles[0].to_degrees() - 90. * (1. - 0.8_f64.powi(9))).abs() < 1e-8
        );
        assert!(s.surface_angles[0].abs() > 0.01);
        assert_eq!(
            s.control_lift_coeff(0),
            0.,
            "authored grid fin ignores SAS and retains manual control"
        );
        assert!(s.lifting_force(0, frame, travel, 1500., 0.15).length() > 1e-6);
        p.part_controls[0].1.surface_deploy = false;
        s.resolve_surface_geometry(&p, &prop, &mut Default::default(), 0.02);
        assert_eq!(s.surface_angles[0], 0.);
        assert_eq!(s.control_lift_coeff(0), 0.);
    }

    fn rcs_fixture() -> (Structure, Powered, [usize; 2], crate::bodies::Body) {
        let (mut s, mut p, pair) = propulsion_fixture();
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.gimbal_range_rad.fill(0.0);
        s.ctrl.fill((0.0, 0.0));
        s.electric.fill(Default::default());
        s.crew[s.root] = 1;
        s.rcs_defs.fill(None);
        s.net.tanks.clear();
        for (i, isp, propellant) in [
            (pair[0], 100.0, "MonoPropellant"),
            (pair[1], 200.0, "SolidFuel"),
        ] {
            s.part_rot[i] = [0.0, 0.0, 0.0, 1.0];
            s.rcs_defs[i] = Some(ose_data::Rcs {
                controls: Default::default(),
                enabled: true,
                thrust_fraction: 1.0,
                shielded_can_activate: false,
                thrusters: Vec::new(),
                thrust_kn: 100.0 * ose_data::G0,
                propellant: propellant.into(),
                isp_curve: ose_data::Curve {
                    keys: [(0.0, isp), (1.0, isp * 0.8), (5.0, isp * 0.2)]
                        .into_iter()
                        .map(|(t, v)| ose_data::CurveKey {
                            t,
                            v,
                            in_tangent: 0.0,
                            out_tangent: 0.0,
                        })
                        .collect(),
                },
            });
            let resource = s.net.resource(propellant).unwrap();
            s.net.flow_mode[resource] = ose_data::FlowMode::NoFlow;
            s.net.tanks.push(crate::resources::Tank {
                part: i,
                resource,
                stored: 1.0 / s.net.unit_mass_t[resource],
                capacity: 2.0 / s.net.unit_mass_t[resource],
            });
        }
        p.ignited = false;
        p.throttle = 0.0;
        p.attitude = crate::flight::Attitude::Free;
        p.rotation_input = DVec3::ZERO;
        p.groups.rcs = true;
        p.rcs.translate = DVec3::new(0.0, 1.0, 0.0);
        let u = crate::Universe::with_bodies(
            &serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
                "../../../assets/own/kerbol-system.json"
            ))
            .unwrap(),
        );
        let mut body = u
            .world
            .get::<crate::bodies::Body>(u.body_entity("Kerbin").unwrap())
            .unwrap()
            .clone();
        body.mu = 0.0;
        body.rotation_period_s = 0.0;
        s.sync(&mut p);
        (s, p, pair, body)
    }

    #[test]
    fn rcs_per_part_axis_switches_gate_commands_without_erasing_physical_moments() {
        for (axis, lever, force, command) in [
            (
                0,
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
                DVec3::new(1.0, 0.0, 0.0),
            ),
            (
                1,
                DVec3::new(0.0, 0.0, 1.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ),
            (
                2,
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(0.0, -1.0, 0.0),
            ),
        ] {
            for enabled in [false, true] {
                let (mut s, mut p, pair, body) = rcs_fixture();
                s.rcs_defs[pair[1]] = None;
                let r = s.rcs_defs[pair[0]].as_mut().unwrap();
                r.thrust_kn = 1.0;
                match axis {
                    0 => r.controls.pitch = enabled,
                    1 => r.controls.roll = enabled,
                    _ => r.controls.yaw = enabled,
                }
                r.thrusters = vec![ose_data::ThrustTransform {
                    position_m: crate::rigid::to_vessel(lever),
                    direction: crate::rigid::to_vessel(force),
                    weight: 1.0,
                }];
                p.rcs.translate = DVec3::ZERO;
                p.rotation_input = command;
                let mut engine = s.propulsion(&p, Default::default(), 0.1);
                engine.mass.centre_m = s.rigid.rest_position(pair[0]);
                s.configure_actuators(&p, Default::default(), &mut engine);
                s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
                let allocation = engine.control_allocation.unwrap();
                let scale = if enabled { 1.0 } else { 0.0 };
                assert!((allocation.force_kn - force * scale).length() < 1e-9);
                assert!((allocation.torque_knm - lever.cross(force) * scale).length() < 1e-9);
                assert!((engine.burned_t - 0.1 * scale / (100.0 * ose_data::G0)).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn rcs_throttle_and_full_thrust_options_resolve_before_final_partial_fuel() {
        for (fore, throttle, by_throttle, full, fore_enabled, stored, expected) in [
            (0.0, 0.5, true, false, true, 1.0, 0.5),
            (-0.5, 1.0, true, false, true, 1.0, 0.0),
            (0.03, 0.5, true, false, true, 1.0, 0.5),
            (0.0, 1.0, false, false, true, 1.0, 0.0),
            (0.0, 1.0, true, false, false, 1.0, 0.0),
            (0.1, 0.0, false, true, true, 1.0, 0.1),
            (0.2, 0.0, false, true, true, 1.0, 1.0),
            (0.8, 0.0, false, true, true, 0.025, 0.25),
        ] {
            let (mut s, mut p, pair, body) = rcs_fixture();
            s.rcs_defs[pair[1]] = None;
            let r = s.rcs_defs[pair[0]].as_mut().unwrap();
            r.controls.fore_by_throttle = by_throttle;
            r.controls.full_thrust = full;
            r.controls.z = fore_enabled;
            r.thrusters = vec![ose_data::ThrustTransform {
                position_m: DVec3::new(1.0, 0.0, 0.0),
                direction: DVec3::new(0.0, 1.0, 0.0),
                weight: 1.0,
            }];
            p.rcs.translate = DVec3::new(0.0, fore, 0.0);
            p.throttle = throttle;
            let tank = s.net.tanks.iter_mut().find(|t| t.part == pair[0]).unwrap();
            tank.stored = stored / s.net.unit_mass_t[tank.resource];
            let mut engine = s.propulsion(&p, Default::default(), 0.1);
            s.configure_actuators(&p, Default::default(), &mut engine);
            s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
            let allocation = engine.control_allocation.unwrap();
            assert!((allocation.force_kn.y - 100.0 * ose_data::G0 * expected).abs() < 1e-8, "fore={fore} throttle={throttle} by_throttle={by_throttle} full={full} enabled={fore_enabled} stored={stored} expected={expected} actual={:?}", allocation.force_kn);
            assert!((engine.burned_t - 0.1 * expected).abs() < 1e-9);
        }
    }

    #[test]
    fn rcs_activation_and_thrust_limits_bound_force_fuel_and_readouts() {
        for authored in [false, true] {
            for (enabled, limit, shielded, allow_shielded, stored, expected) in [
                (true, 1.0, false, false, 1.0, 1.0),
                (true, 0.25, false, false, 1.0, 0.25),
                (true, 0.25, false, false, 0.005, 0.05),
                (false, 1.0, false, false, 1.0, 0.0),
                (true, 0.0, false, false, 1.0, 0.0),
                (true, 1.0, true, false, 1.0, 0.0),
                (true, 1.0, true, true, 1.0, 1.0),
                (true, 2.0, false, false, 1.0, 1.0),
                (true, -1.0, false, false, 1.0, 0.0),
            ] {
                let (mut s, mut p, pair, body) = rcs_fixture();
                s.rcs_defs[pair[1]].as_mut().unwrap().enabled = false;
                let r = s.rcs_defs[pair[0]].as_mut().unwrap();
                r.enabled = enabled;
                r.thrust_fraction = limit;
                r.shielded_can_activate = allow_shielded;
                if authored {
                    r.thrusters.push(ose_data::ThrustTransform {
                        position_m: DVec3::new(1.0, 0.0, 0.0),
                        direction: DVec3::new(0.0, 1.0, 0.0),
                        weight: 1.0,
                    });
                }
                let tank = s.net.tanks.iter_mut().find(|t| t.part == pair[0]).unwrap();
                tank.stored = stored / s.net.unit_mass_t[tank.resource];
                s.sync(&mut p);
                s.shielded[pair[0]] = shielded;
                let before = s.net.mass_t();
                let other_before = s.net.mass_of(&[pair[1]]);
                let env = crate::flight::EngineEnvironment::default();
                let mut engine = s.propulsion(&p, env, 0.1);
                s.configure_actuators(&p, env, &mut engine);
                s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
                let allocation = engine.control_allocation.unwrap();
                assert!((allocation.force_kn.y - 100.0 * ose_data::G0 * expected).abs() < 1e-8,
                    "authored={authored}, enabled={enabled}, limit={limit}, shielded={shielded}: {:?}", allocation.force_kn);
                assert!((before - s.net.mass_t() - 0.1 * expected).abs() < 1e-9);
                assert!((engine.burned_t - 0.1 * expected).abs() < 1e-9);
                assert_eq!(s.net.mass_of(&[pair[1]]), other_before);
                if expected > 0.0 {
                    let rated_fraction = if stored < 0.1 * limit {
                        expected / limit
                    } else {
                        1.0
                    };
                    assert!((p.rcs.powered - rated_fraction).abs() < 1e-9);
                    assert!((p.rcs.usage - expected / limit.clamp(0.0, 1.0)).abs() < 1e-9);
                } else {
                    assert_eq!(p.rcs.powered, 0.0);
                    assert_eq!(p.rcs.usage, 0.0);
                }
            }
        }
    }

    #[test]
    fn rcs_mounts_use_their_own_propellant_isp_and_full_pressure_curve() {
        for pressure_atm in [0.0, 3.0, 5.0] {
            let (mut s, mut p, pair, body) = rcs_fixture();
            let before: Vec<_> = pair.iter().map(|&i| s.net.mass_of(&[i])).collect();
            let env = crate::flight::EngineEnvironment {
                pressure_atm,
                ..Default::default()
            };
            let mut engine = s.propulsion(&p, env, 0.1);
            s.configure_actuators(&p, env, &mut engine);
            s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
            let scale = match pressure_atm {
                0.0 => 1.0,
                3.0 => 0.5,
                _ => 0.2,
            };
            let allocation = engine.control_allocation.unwrap();
            assert!((allocation.force_kn.y - 200.0 * ose_data::G0 * scale).abs() < 1e-8);
            assert!((engine.burned_t - 0.15).abs() < 1e-9);
            assert!((before[0] - s.net.mass_of(&[pair[0]]) - 0.1).abs() < 1e-9);
            assert!((before[1] - s.net.mass_of(&[pair[1]]) - 0.05).abs() < 1e-9);
        }
    }

    #[test]
    fn rcs_empty_and_final_partial_fuel_limit_force_before_integration() {
        for fraction in [0.0, 0.25, 1.0] {
            let (mut s, mut p, pair, body) = rcs_fixture();
            s.rcs_defs[pair[1]] = None;
            s.net.tanks.retain(|t| t.part == pair[0]);
            let t = &mut s.net.tanks[0];
            t.stored = 0.1 * fraction / s.net.unit_mass_t[t.resource];
            s.sync(&mut p);
            let mass = p.mass_t;
            let v = p.v;
            // Control and engine impulses use the same mass at the start of this step.
            let expected = p.facing * (100.0 * ose_data::G0 * fraction * 0.1 / mass);
            let mut engine = s.propulsion(&p, Default::default(), 0.1);
            // Isotropic inertia keeps a constant body torque parallel to its inertial
            // rotation axis, giving the analytical angular impulse exactly.
            engine.mass.inertia = crate::dynamics::Inertia {
                x: DVec3::new(10.0, 0.0, 0.0),
                y: DVec3::new(0.0, 10.0, 0.0),
                z: DVec3::new(0.0, 0.0, 10.0),
            };
            s.configure_actuators(&p, Default::default(), &mut engine);
            p.rcs.powered = 1.0; // stale readout cannot create an impulse
            s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
            assert!((engine.burned_t - 0.1 * fraction).abs() < 1e-9);
            let inertia = engine.mass.inertia;
            let (x0, y0, z0) = crate::flight::vessel_basis(&p);
            let torque = engine.control_allocation.as_ref().unwrap().torque_knm;
            let expected_momentum = (x0 * torque.x + y0 * torque.y + z0 * torque.z) * 0.1;

            let out = crate::flight::integrate_with_propulsion(
                &mut p,
                &body,
                None,
                None,
                0.0,
                0.1,
                Some(engine),
            );
            assert!(
                (p.v - v - expected).length() < 1e-8,
                "fraction={fraction}, out={out:?}"
            );
            assert!((p.mass_t - s.net.mass_t()).abs() < 1e-9);
            let (x, y, z) = crate::flight::vessel_basis(&p);
            let w = p.angvel + y * p.roll_rate;
            let momentum = inertia.apply(DVec3::new(w.dot(x), w.dot(y), w.dot(z)));
            assert!(
                (x * momentum.x + y * momentum.y + z * momentum.z - expected_momentum).length()
                    < 1e-8
            );

            let v = p.v;
            let mut engine = s.propulsion(&p, Default::default(), 0.1);
            s.configure_actuators(&p, Default::default(), &mut engine);
            s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
            crate::flight::integrate_with_propulsion(
                &mut p,
                &body,
                None,
                None,
                0.1,
                0.1,
                Some(engine),
            );
            assert!((p.v - v).length() < 1e-9, "empty subsequent tick");
        }
    }

    #[test]
    fn rcs_recipe_shortfall_limits_every_ingredient_at_the_requesting_mount() {
        let (mut s, mut p, pair, body) = rcs_fixture();
        s.rcs_defs[pair[1]] = None;
        s.rcs_defs[pair[0]].as_mut().unwrap().propellant = "Methalox".into();
        s.net.tanks.clear();
        let unit_mass = s.net.mixture_unit_mass_t("Methalox");
        let ingredients = s.net.ingredients("Methalox");
        assert_eq!(ingredients.len(), 2);
        let mut before = Vec::new();
        for (j, &(resource, ratio)) in ingredients.iter().enumerate() {
            s.net.flow_mode[resource] = ose_data::FlowMode::NoFlow;
            let full = 0.1 / unit_mass * ratio;
            let stored = full * if j == 0 { 0.25 } else { 2.0 };
            before.push(stored);
            s.net.tanks.push(crate::resources::Tank {
                part: pair[0],
                resource,
                stored,
                capacity: full * 4.0,
            });
            // An isolated tank at the root cannot supply a NoFlow mount.
            s.net.tanks.push(crate::resources::Tank {
                part: s.root,
                resource,
                stored: full * 4.0,
                capacity: full * 4.0,
            });
        }
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
        assert!((engine.burned_t - 0.025).abs() < 1e-9);
        assert!((engine.control_allocation.unwrap().force_kn.y - 25.0 * ose_data::G0).abs() < 1e-8);
        for (j, &(resource, ratio)) in ingredients.iter().enumerate() {
            let stored = s
                .net
                .tanks
                .iter()
                .find(|t| t.part == pair[0] && t.resource == resource)
                .unwrap()
                .stored;
            assert!((before[j] - stored - 0.025 / unit_mass * ratio).abs() < 1e-9);
        }
        assert!(!s.rcs_fed(pair[0]));
    }

    #[test]
    fn universe_rcs_resolves_the_last_fuel_once_and_joint_loads_receive_translation() {
        let (mut s, mut p, pair, _) = rcs_fixture();
        s.rcs_defs[pair[1]] = None;
        s.rcs_defs[pair[0]].as_mut().unwrap().thrust_kn = 1.0;
        s.net.tanks.retain(|t| t.part == pair[0]);
        let dt = 1.0 / crate::TICKS_PER_SECOND as f64;
        let fuel = dt * 0.25 / (100.0 * ose_data::G0);
        let t = &mut s.net.tanks[0];
        t.stored = fuel / s.net.unit_mass_t[t.resource];
        p.facing = DVec3::new(1.0, 0.0, 0.0);
        p.v = DVec3::new(0.0, 100.0, 0.0);
        s.sync(&mut p);
        let mass = p.mass_t;
        let radius = p.r.length();
        let mut u = crate::Universe::with_bodies(
            &serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
                "../../../assets/own/kerbol-system.json"
            ))
            .unwrap(),
        );
        let body = u.body_entity("Kerbin").unwrap();
        let mu = u.world.get::<crate::bodies::Body>(body).unwrap().mu;
        let vessel = u.spawn_vessel("Fuel-limited RCS", body, p.r, p.v);
        u.set_powered(vessel, p);
        u.set_structure(vessel, s);
        u.step();
        let after = u.powered(vessel).unwrap();
        let expected = 0.25 * dt / (mass - fuel) - mu / radius.powi(2) * dt;
        assert!(
            (after.v.x - expected).abs() < 1e-8,
            "{} vs {expected}",
            after.v.x
        );
        assert!((after.mass_t - mass + fuel).abs() < 1e-9);
        let s = u.structure(vessel).unwrap();
        assert!((after.mass_t - s.net.mass_t()).abs() < 1e-9);
        assert!(
            s.rigid.links.iter().any(|l| l.force_kn > 1e-4),
            "translation reaches joint solver even without net angular acceleration"
        );
        u.step();
        assert!((u.powered(vessel).unwrap().mass_t - mass + fuel).abs() < 1e-9);
    }

    #[test]
    fn rcs_nozzles_fire_only_in_their_authored_direction_and_partial_fuel_limits_them() {
        for (direction, fraction) in [(1.0, 0.0), (1.0, 0.25), (1.0, 1.0), (-1.0, 1.0)] {
            let (mut s, mut p, pair, body) = rcs_fixture();
            s.rcs_defs[pair[1]] = None;
            let rcs = s.rcs_defs[pair[0]].as_mut().unwrap();
            rcs.thrust_kn = 1.0;
            rcs.thrusters = vec![ose_data::ThrustTransform {
                position_m: DVec3::new(0.0, 1.0, 0.0),
                direction: DVec3::new(1.0, 0.0, 0.0),
                weight: 1.0,
            }];
            s.net.tanks.retain(|t| t.part == pair[0]);
            let t = &mut s.net.tanks[0];
            t.stored = 0.1 * fraction / (100.0 * ose_data::G0 * s.net.unit_mass_t[t.resource]);
            p.rcs.translate = DVec3::new(direction, 0.0, 0.0);
            let mut engine = s.propulsion(&p, Default::default(), 0.1);
            let centre = engine.mass.centre_m;
            s.configure_actuators(&p, Default::default(), &mut engine);
            s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
            let scale = if direction > 0.0 { fraction } else { 0.0 };
            let allocation = engine.control_allocation.unwrap();
            assert!((allocation.force_kn - DVec3::new(scale, 0.0, 0.0)).length() < 1e-9);
            assert!(
                (allocation.torque_knm
                    - (s.rigid.rest_position(pair[0]) + DVec3::new(0.0, 1.0, 0.0) - centre)
                        .cross(allocation.force_kn))
                .length()
                    < 1e-9
            );
            assert!((engine.burned_t - 0.1 * scale / (100.0 * ose_data::G0)).abs() < 1e-9);
            assert!(allocation.inputs.iter().all(|x| *x >= 0.0 && *x <= 1.0));
        }
    }

    #[test]
    fn rcs_opposing_nozzles_pay_for_both_forces_even_when_translation_cancels() {
        let (mut s, mut p, pair, body) = rcs_fixture();
        s.rcs_defs[pair[1]] = None;
        let rcs = s.rcs_defs[pair[0]].as_mut().unwrap();
        rcs.thrust_kn = 1.0;
        rcs.thrusters = [1.0, -1.0]
            .into_iter()
            .map(|sign| ose_data::ThrustTransform {
                position_m: DVec3::new(0.0, sign, 0.0),
                direction: DVec3::new(sign, 0.0, 0.0),
                weight: 1.0,
            })
            .collect();
        p.rcs.translate = DVec3::new(1.0, 0.0, 0.0);
        p.rotation_input = DVec3::new(0.0, 1.0, 0.0);
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        engine.mass.centre_m = s.rigid.rest_position(pair[0]);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
        let allocation = engine.control_allocation.unwrap();
        assert!(allocation.force_kn.length() < 1e-9);
        assert!((allocation.torque_knm - DVec3::new(0.0, 0.0, -2.0)).length() < 1e-9);
        assert!((engine.burned_t - 0.2 / (100.0 * ose_data::G0)).abs() < 1e-9);
        assert_eq!(allocation.inputs, vec![1.0, 1.0]);
    }

    #[test]
    fn committed_rcs_nozzles_cover_the_five_authored_modules() {
        let (_, defs, _) = assets();
        let modules: Vec<_> = defs.iter().filter_map(|p| p.rcs.as_ref()).collect();
        assert_eq!(modules.len(), 5);
        assert_eq!(modules.iter().map(|r| r.thrusters.len()).sum::<usize>(), 22);
        assert!(modules.iter().all(|r| !r.thrusters.is_empty()));
        let block = defs
            .iter()
            .find(|p| p.id == "rcs_0v_block_monoprop")
            .unwrap()
            .rcs
            .as_ref()
            .unwrap();
        assert!(
            block.thrusters.iter().all(|t| t.direction.x == 0.0),
            "the four-way block cannot synthesize a fifth direction"
        );
    }

    #[test]
    fn rcs_translation_and_rotation_share_each_mounts_force_and_fuel_budget() {
        let (mut s, mut p, pair, body) = rcs_fixture();
        p.rotation_input = DVec3::new(1.0, 1.0, 1.0);
        p.rcs.translate = DVec3::new(1.0, 1.0, 1.0);
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
        let channels = engine.actuators.as_ref().unwrap();
        let allocation = engine.control_allocation.as_ref().unwrap();
        let mut expected_burn = 0.0;
        for i in pair {
            let mut force = DVec3::ZERO;
            for (a, input) in channels.iter().zip(&allocation.inputs) {
                if a.part == i {
                    force = force + a.force_kn * *input;
                }
            }
            let r = s.rcs_defs[i].as_ref().unwrap();
            assert!(force.length() <= r.thrust_kn + 1e-9);
            expected_burn += force.length() * 0.1 / (r.isp_curve.evaluate(0.0) * ose_data::G0);
        }
        assert!((engine.burned_t - expected_burn).abs() < 1e-9);
        // Detached and disabled mounts cannot request propellant or contribute channels.
        s.rigid.remove_parts(&[pair[0]]);
        s.net.remove_parts(&[pair[0]]);
        p.groups.rcs = false;
        let before = s.net.mass_t();
        let mut engine = s.propulsion(&p, Default::default(), 0.1);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.1);
        assert_eq!(before, s.net.mass_t());
        assert!(engine.actuators.unwrap().is_empty());
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
    fn manual_wheel_mode_uses_raw_input_and_sas_only_subtracts_raw_input() {
        use ose_data::flight_geometry::PartControl;
        for (mode, expected) in [(1, 1.2), (2, -1.08)] {
            let (mut s, mut p, _) = wheel_power_fixture();
            p.rotation_input = DVec3::new(0., 0., 0.1);
            p.raw_rotation_input = Some(DVec3::new(0., 0., 1.));
            p.rcs.precision = true;
            if mode == 2 {
                p.attitude = crate::flight::Attitude::Fixed(p.facing);
            }
            p.part_controls = vec![(
                s.root as u16,
                PartControl {
                    wheel_mode: mode,
                    ..Default::default()
                },
            )];
            let ec = s.net.resource("ElectricCharge").unwrap();
            s.net.tanks.push(crate::resources::Tank {
                part: s.root,
                resource: ec,
                stored: 10.,
                capacity: 10.,
            });
            let mut engine = s.propulsion(&p, Default::default(), 0.02);
            s.configure_actuators(&p, Default::default(), &mut engine);
            s.resolve_wheel_power(&mut p, &test_body(), &mut engine, 0.02);
            assert!(
                (engine.control_allocation.unwrap().torque_knm.y - expected).abs() < 1e-8,
                "mode={mode}"
            );
        }
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
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
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
                kind: crate::actuators::Kind::Other,
                part: s.root,
                torque_knm: DVec3::new(0.0, 2.0, 0.0),
                force_kn: DVec3::ZERO,
                offset_m: DVec3::ZERO,
                group: 0,
                axial: None,
            },
            crate::actuators::Actuator {
                kind: crate::actuators::Kind::Other,
                part: other,
                torque_knm: DVec3::new(0.0, 2.0, 0.0),
                force_kn: DVec3::new(2.0, 0.0, 0.0),
                offset_m: DVec3::ZERO,
                group: 1,
                axial: None,
            },
        ]);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
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
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
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
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
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
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
        assert_eq!(ec_stored(&s), before, "free spin draws no wheel charge");
        p.attitude = crate::flight::Attitude::Fixed(p.facing);
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
        assert_eq!(engine.control_allocation.unwrap().torque_knm.y, 0.0);
        p.roll_rad += 0.002;
        let mut engine = s.propulsion(&p, Default::default(), 0.02);
        s.configure_actuators(&p, Default::default(), &mut engine);
        s.resolve_wheel_power(&mut p, &body, &mut engine, 0.02);
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
    fn mounted_rotation_scale_cannot_change_forces_contact_bounds_or_inertia() {
        let (mut craft, defs, res) = test_capsule(false);
        let h = 0.5_f64.sqrt();
        for part in &mut craft.parts {
            part.local_rotation = ose_data::craft::Quat {
                x: 0.0,
                y: 0.0,
                z: h,
                w: h,
            };
        }
        let mut baseline = Structure::build(&craft, &defs, &res);
        for scale in [0.5, -3.0, 1.0000003] {
            let mut scaled = craft.clone();
            for part in &mut scaled.parts {
                part.local_rotation.z *= scale;
                part.local_rotation.w *= scale;
            }
            let mut actual = Structure::build(&scaled, &defs, &res);
            let expected_mass = baseline.mass_properties();
            let actual_mass = actual.mass_properties();
            assert!((actual_mass.centre_m - expected_mass.centre_m).length() < 1e-11);
            for (a, b) in [
                (actual_mass.inertia.x, expected_mass.inertia.x),
                (actual_mass.inertia.y, expected_mass.inertia.y),
                (actual_mass.inertia.z, expected_mass.inertia.z),
            ] {
                assert!((a - b).length() < 1e-10);
            }
            for i in 0..actual.rigid.len() {
                let (actual_centre, actual_half) = actual.part_box(i);
                let (expected_centre, expected_half) = baseline.part_box(i);
                assert!((actual_centre - expected_centre).length() < 1e-11);
                assert!((actual_half - expected_half).length() < 1e-11);
                let axes = actual.part_axes(i);
                assert!((axes[0] - DVec3::new(0.0, 1.0, 0.0)).length() < 1e-12);
                assert!((axes[1] - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-12);
                for (force, expected) in actual
                    .engine_forces(i, 17.0)
                    .iter()
                    .zip(baseline.engine_forces(i, 17.0))
                {
                    assert!((force.force_kn - expected.force_kn).length() < 1e-10);
                    assert!((force.offset_m - expected.offset_m).length() < 1e-11);
                }
            }
            let frame = basis(DVec3::new(0.0, 1.0, 0.0));
            let velocity = DVec3::new(30.0, 80.0, 10.0);
            let flow = |s: &mut Structure| {
                s.aerodynamics_local(
                    0,
                    Airflow {
                        frame,
                        velocity,
                        angular_velocity: DVec3::new(0.1, -0.2, 0.3),
                        density: 1.2,
                        speed_of_sound: 340.0,
                    },
                )
            };
            let expected = flow(&mut baseline);
            let aero = flow(&mut actual);
            assert!((aero.drag_kn - expected.drag_kn).length() < 1e-10);
            assert!((aero.lift_kn - expected.lift_kn).length() < 1e-10);
            assert!((aero.torque_knm - expected.torque_knm).length() < 1e-9);
            let level = level_layout(&scaled).unwrap();
            let level_base = level_layout(&craft).unwrap();
            for (a, b) in level.parts.iter().zip(level_base.parts) {
                assert!((a.local_position - b.local_position).length() < 1e-11);
            }
        }
        let rotation = ose_data::craft::Quat {
            x: 0.0,
            y: 0.0,
            z: 2.0 * h,
            w: 2.0 * h,
        };
        let rotated = rotated(&craft, rotation);
        for (a, b) in rotated.parts.iter().zip(&craft.parts) {
            let p = b.local_position;
            assert!((a.local_position - DVec3::new(-p.y, p.x, p.z)).length() < 1e-11);
        }
    }

    #[test]
    fn mass_offsets_shift_wet_centres_and_inertia_without_moving_geometry() {
        let (mut craft, mut defs, res) = test_capsule(false);
        craft.parts[0].local_position = DVec3::ZERO;
        craft.parts[1].local_position = DVec3::new(0.0, 3.0, 0.0);
        let h = 0.5_f64.sqrt();
        craft.parts[0].local_rotation = ose_data::craft::Quat {
            x: 0.0,
            y: 0.0,
            z: h,
            w: h,
        };
        craft.parts[1].local_rotation = ose_data::craft::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        for (i, part) in craft.parts.iter_mut().enumerate() {
            part.resources.clear();
            let def = defs.iter_mut().find(|d| d.id == part.part_id).unwrap();
            def.flight_geometry = Default::default();
            def.mass_t = if i == 0 { 2.0 } else { 1.0 };
            def.mass_offset_m = if i == 0 {
                DVec3::new(1.0, 0.0, 0.0)
            } else {
                DVec3::new(0.0, 0.5, 0.0)
            };
            def.resource_containers.clear();
            def.bounds = Some(ose_data::PartBounds {
                center_m: DVec3::ZERO,
                size_m: DVec3::new(1.0, 1.0, 1.0),
            });
        }
        let mut s = Structure::build(&craft, &defs, &res);
        let dry = s.mass_properties();
        let c = 5.5 / 3.0;
        let moment = 0.5 + 2.0 * (1.0_f64 - c).powi(2) + (3.5_f64 - c).powi(2);
        assert!((dry.centre_m - DVec3::new(0.0, c, 0.0)).length() < 1e-12);
        assert!((dry.inertia.x.x - moment).abs() < 1e-12);
        assert!((dry.inertia.y.y - 0.5).abs() < 1e-12);
        let fuel = s.net.resource("SolidFuel").unwrap();
        s.net.tanks.push(crate::resources::Tank {
            part: 1,
            resource: fuel,
            stored: 1.0 / s.net.unit_mass_t[fuel],
            capacity: 1.0 / s.net.unit_mass_t[fuel],
        });
        s.sync_structural_mass();
        let wet = s.mass_properties();
        assert!((wet.mass_t - 4.0).abs() < 1e-12);
        assert!((wet.centre_m - DVec3::new(0.0, 2.25, 0.0)).length() < 1e-12);
        assert!((s.rigid.rest_centre() - wet.centre_m).length() < 1e-12);
        assert_eq!(s.rigid.rest_position(0), DVec3::ZERO);
        assert_eq!(s.part_box(0).0, DVec3::ZERO);
        s.rigid.remove_parts(&[1]);
        s.net.remove_parts(&[1]);
        let remaining = s.mass_properties();
        assert!((remaining.centre_m - DVec3::new(0.0, 1.0, 0.0)).length() < 1e-12);
        assert!((remaining.inertia.x.x - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn fuel_drain_and_detachment_update_mass_centre_and_inertia() {
        let (mut s, mut p, pair) = propulsion_fixture();
        let before = s.mass_properties();
        let burn = s.propulsion(&p, Default::default(), 0.1);
        let after = s.mass_properties();
        let expected = (before.centre_m * before.mass_t
            - s.rigid.rest_mass_position(pair[0]) * 0.1
            - s.rigid.rest_mass_position(pair[1]) * 0.05)
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
                c + s.rigid.rest_mass_position(i) * s.net.mass_of(&[i]).max(0.001)
            })
            * (1.0 / rigid_total);
        assert!((s.rigid.rest_centre() - rigid_centre).length() < 1e-9);
        let removed_mass = s.net.mass_of(&[pair[0]]);
        s.rigid.remove_parts(&[pair[0]]);
        s.net.remove_parts(&[pair[0]]);
        let detached = s.mass_properties();
        let expected = (after.centre_m * after.mass_t
            - s.rigid.rest_mass_position(pair[0]) * removed_mass)
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
    fn authored_nozzles_preserve_weighted_directions_and_individual_application_points() {
        let (mut s, p, pair) = propulsion_fixture();
        let i = pair[0];
        s.stage_engines[0] = vec![i];
        s.engine_nozzles[i] = vec![
            ose_data::ThrustTransform {
                position_m: DVec3::new(1.0, -2.0, 0.0),
                direction: DVec3::new(0.6, 0.8, 0.0),
                weight: 0.25,
            },
            ose_data::ThrustTransform {
                position_m: DVec3::new(-1.0, -2.0, 0.0),
                direction: DVec3::new(-0.6, 0.8, 0.0),
                weight: 0.75,
            },
        ];
        let h = 0.5_f64.sqrt();
        s.part_rot[i] = [0.0, h, 0.0, h]; // Unity +X becomes -Z; handedness then makes it craft +Z.
        let out = s.propulsion(&p, Default::default(), 0.02);
        let thrust = 100.0 * ose_data::G0;
        let expected_force = DVec3::new(0.0, 0.8, -0.3) * thrust;
        let r = s.rigid.rest_position(i) - out.mass.centre_m;
        let expected_torque = (r + DVec3::new(0.0, -2.0, 1.0))
            .cross(DVec3::new(0.0, 0.8, 0.6) * (thrust * 0.25))
            + (r + DVec3::new(0.0, -2.0, -1.0)).cross(DVec3::new(0.0, 0.8, -0.6) * (thrust * 0.75));
        assert!((out.force_kn - expected_force).length() < 1e-9);
        assert!((out.torque_knm - expected_torque).length() < 1e-8);
        assert!(
            (out.thrust_kn - thrust).abs() < 1e-9,
            "rated scalar remains sum of nozzle magnitudes"
        );
        assert!(
            (out.burned_t - 0.02).abs() < 1e-9,
            "cant does not alter fuel demand"
        );
        let points = s.engine_forces(i, thrust);
        assert_eq!(points.len(), 2);
        assert!((points[0].offset_m - DVec3::new(0.0, -2.0, 1.0)).length() < 1e-9);
    }

    #[test]
    fn committed_nozzle_catalog_covers_every_engine_mode_and_authored_cant() {
        let (_, defs, _) = assets();
        let mut count = 0;
        let mut points = 0;
        for part in &defs {
            for mode in &part.engines {
                assert!(
                    !mode.thrust_transforms.is_empty(),
                    "{} {}",
                    part.id,
                    mode.id
                );
                let sum: f64 = mode.thrust_transforms.iter().map(|p| p.weight).sum();
                assert!((sum - 1.0).abs() < 1e-6);
                for p in &mode.thrust_transforms {
                    assert!((p.direction.length() - 1.0).abs() < 1e-9);
                }
                count += 1;
                points += mode.thrust_transforms.len();
            }
        }
        assert_eq!((count, points), (41, 63));
        let twitch = defs
            .iter()
            .find(|p| p.id == "engine_0v_methalox_twitch")
            .unwrap();
        assert!(
            (twitch.engines[0].thrust_transforms[0].direction.x - 0.08808842447530461).abs()
                < 1e-12
        );
        let cornet = defs
            .iter()
            .find(|p| p.id == "engine_1v_methalox_cornet")
            .unwrap();
        assert!(
            (cornet.engines[0].thrust_transforms[0].position_m.y + 1.0079088807106014).abs()
                < 1e-12
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
    fn resolved_structural_thrust_delivers_the_exact_single_body_impulse() {
        let (mut craft, mut defs, res) = test_capsule(false);
        craft.parts.truncate(1);
        craft.parts[0].resources.clear();
        craft.parts[0].parent = None;
        craft.parts[0].local_position = DVec3::ZERO;
        craft.parts[0].local_rotation = ose_data::craft::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        craft.stages = vec![vec![0]];
        let engine = defs
            .iter()
            .flat_map(|d| &d.engines)
            .find(|e| e.propellant_mixture == "SolidFuel")
            .unwrap()
            .clone();
        let def = defs
            .iter_mut()
            .find(|d| d.id == craft.parts[0].part_id)
            .unwrap();
        def.mass_t = 2.0;
        def.mass_offset_m = DVec3::ZERO;
        def.resource_containers.clear();
        def.engines = vec![engine];
        let mut s = Structure::build(&craft, &defs, &res);
        s.stage_engines = vec![vec![0]];
        s.engine_thrust_kn[0] = 100.0 * ose_data::G0;
        s.engine_throttle_locked[0] = false;
        s.engine_nozzles[0].clear();
        s.engine_isp[0] = Some(ose_data::Curve {
            keys: vec![ose_data::CurveKey {
                t: 0.0,
                v: 100.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            }],
        });
        let solid = s.net.resource("SolidFuel").unwrap();
        s.net.tanks = vec![crate::resources::Tank {
            part: 0,
            resource: solid,
            stored: 0.025 / s.net.unit_mass_t[solid],
            capacity: 0.1 / s.net.unit_mass_t[solid],
        }];
        let mut p = flying(DVec3::new(1.0, 0.0, 0.0));
        p.throttle = 1.0;
        p.ignited = true;
        s.sync(&mut p);
        let engine = s.propulsion(&p, Default::default(), 0.1);
        assert!((engine.burned_t - 0.025).abs() < 1e-12);
        let pieces = s.apply(
            &p,
            &StepOutcome {
                thrust_kn: engine.thrust_kn,
                ..Default::default()
            },
            0.1,
        );
        assert!(pieces.is_empty());
        let expected = DVec3::new(0.0, 0.025 * 100.0 * ose_data::G0 / 2.0, 0.0);
        assert!((s.rigid.centre_of(&[0]).1 - expected).length() < 1e-10);
        let idle = s.propulsion(&p, Default::default(), 0.1);
        assert_no_engine_impulse(idle);
        s.apply(
            &p,
            &StepOutcome {
                angvel: DVec3::new(0.1, 0.0, 0.0),
                ..Default::default()
            },
            0.1,
        );
        assert_eq!(s.engine_load_kn[0], 0.0);
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
        assert!((s.engine_load_kn[pair[0]] - out.thrust_kn).abs() < 1e-9);
        assert_eq!(s.engine_load_kn[pair[1]], 0.0);
        let coast = s.propulsion(&p, Default::default(), 0.1);
        assert_no_engine_impulse(coast);
        // Keep structural stepping active with rotation: exhausted engines still cannot leave a tail.
        s.apply(
            &p,
            &StepOutcome {
                angvel: DVec3::new(0.1, 0.0, 0.0),
                ..Default::default()
            },
            0.1,
        );
        assert_eq!(s.engine_load_kn[pair[0]], 0.0);
        assert_eq!(s.engine_load_kn[pair[1]], 0.0);
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

    fn alternator_fixture(fuel_t: f64) -> (Structure, Powered, usize) {
        let (mut s, mut p, pair) = propulsion_fixture();
        let engine = pair[0];
        s.stage_engines[0] = vec![engine];
        s.electric.fill(Default::default());
        s.converter_defs.fill(None);
        s.crew.fill(0);
        s.crew[s.root] = 1;
        s.wheel_axes_knm.fill(DVec3::ZERO);
        s.gimbal_range_rad.fill(0.0);
        s.ctrl.fill((0.0, 0.0));
        s.rcs_defs.fill(None);
        s.electric[engine].generator_output = 12.0;
        let solid = s.net.resource("SolidFuel").unwrap();
        let ec = s.net.resource("ElectricCharge").unwrap();
        s.net.flow_mode[ec] = ose_data::FlowMode::AllVessel;
        s.net.tanks = vec![
            crate::resources::Tank {
                part: engine,
                resource: solid,
                stored: fuel_t / s.net.unit_mass_t[solid],
                capacity: 10.0 / s.net.unit_mass_t[solid],
            },
            crate::resources::Tank {
                part: s.root,
                resource: ec,
                stored: 0.0,
                capacity: 10.0,
            },
        ];
        p.v = DVec3::new(0.0, 100.0, 0.0);
        p.attitude = crate::flight::Attitude::Free;
        p.rotation_input = DVec3::ZERO;
        s.sync(&mut p);
        (s, p, engine)
    }

    #[test]
    fn alternator_charge_and_impulse_share_the_same_final_partial_fuel() {
        let dt = 1.0 / crate::TICKS_PER_SECOND as f64;
        for fraction in [0.0, 0.25, 1.0] {
            let fuel = dt * fraction; // Synthetic engine requests exactly 1 t/s.
            let (s, p, _) = alternator_fixture(fuel);
            let mass = p.mass_t;
            let radius = p.r.length();
            let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
                "../../../assets/own/kerbol-system.json"
            ))
            .unwrap();
            let mut u = crate::Universe::with_bodies(&bodies);
            let body = u.body_entity("Kerbin").unwrap();
            let mu = u.world.get::<crate::bodies::Body>(body).unwrap().mu;
            let e = u.spawn_vessel("Fuel-limited alternator", body, p.r, p.v);
            u.set_powered(e, p);
            u.set_structure(e, s);
            u.step();
            let p = u.powered(e).unwrap();
            let expected_v = fuel * 100.0 * ose_data::G0 / mass - mu / radius.powi(2) * dt;
            assert!((p.v.x - expected_v).abs() < 1e-8);
            assert!((p.mass_t - (mass - fuel)).abs() < 1e-9);
            assert!((p.mass_t - u.structure(e).unwrap().net.mass_t()).abs() < 1e-9);
            assert!((ec_stored(u.structure(e).unwrap()) - 12.0 * dt * fraction).abs() < 1e-9);
            u.step();
            assert!((ec_stored(u.structure(e).unwrap()) - 12.0 * dt * fraction).abs() < 1e-9);
        }
    }

    #[test]
    fn alternators_follow_pressure_throttle_air_and_engine_lifecycle() {
        for (throttle, locked, pressure, isp, expected) in [
            (0.5, false, 0.0, 100.0, 0.6),
            (0.0, true, 0.0, 100.0, 1.2),
            (1.0, false, 5.0, 20.0, 0.24),
        ] {
            let (mut s, mut p, engine) = alternator_fixture(1.0);
            p.throttle = throttle;
            s.engine_throttle_locked[engine] = locked;
            s.engine_isp[engine]
                .as_mut()
                .unwrap()
                .keys
                .push(ose_data::CurveKey {
                    t: 5.0,
                    v: isp,
                    in_tangent: 0.0,
                    out_tangent: 0.0,
                });
            let out = s.powered_services(
                &mut p,
                0.0,
                crate::flight::EngineEnvironment {
                    pressure_atm: pressure,
                    ..Default::default()
                },
                0.1,
            );
            assert!((ec_stored(&s) - expected).abs() < 1e-9);
            assert!(
                (ec_stored(&s) - 12.0 * out.thrust_kn / (100.0 * ose_data::G0) * 0.1).abs() < 1e-9
            );
        }
        // No actual output means no alternator charge, even with fuel and a stale running readout.
        for case in 0..6 {
            let (mut s, mut p, engine) = alternator_fixture(1.0);
            match case {
                0 => p.ignited = false,
                1 => p.crashed = true,
                2 => p.throttle = 0.0,
                3 => s.rigid.remove_parts(&[engine]),
                4 => s.stage_engines[0].clear(),
                _ => {
                    s.engine_jet[engine] = Some(crate::flight::Jet {
                        vel_curve: None,
                        atm_curve: None,
                    });
                    s.intake_area_m2[s.root] = 1.0;
                }
            }
            let out = s.powered_services(&mut p, 0.0, Default::default(), 0.1);
            assert_no_engine_impulse(out);
            assert_eq!(ec_stored(&s), 0.0, "case {case}");
        }
    }

    #[test]
    fn alternator_power_reaches_controls_and_wheels_in_the_current_tick() {
        let (mut s, mut p, _) = alternator_fixture(0.025);
        s.crew.fill(0);
        s.electric[s.root].command_use = 1.0;
        s.electric[s.root].wheel_use_per_knm = 1.0;
        s.wheel_axes_knm[s.root] = DVec3::new(0.0, 2.0, 0.0);
        s.part_rot[s.root] = [0.0, 0.0, 0.0, 1.0];
        p.rotation_input = DVec3::new(0.0, 0.0, 1.0);
        p.wheel_scale = 0.0;
        p.controlled = false; // Stale readout must not delay recovery.
        let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
            "../../../assets/own/kerbol-system.json"
        ))
        .unwrap();
        let u = crate::Universe::with_bodies(&bodies);
        let body = u
            .world
            .get::<crate::bodies::Body>(u.body_entity("Kerbin").unwrap())
            .unwrap();
        let mut out = s.powered_services(&mut p, 0.0, Default::default(), 0.1);
        assert!(p.controlled);
        assert!((ec_stored(&s) - 0.2).abs() < 1e-9); // 0.3 generated, 0.1 for the core.
        p.wheel_scale = 1.0;
        s.configure_actuators(&p, Default::default(), &mut out);
        s.resolve_wheel_power(&mut p, body, &mut out, 0.1);
        assert!((out.control_allocation.unwrap().torque_knm.y - 2.0).abs() < 1e-9);
        assert!(ec_stored(&s) < 1e-9);
        assert!((out.burned_t - 0.025).abs() < 1e-9);
    }

    #[test]
    fn isolated_unpowered_control_discards_trial_fuel_and_alternator_charge() {
        let (mut s, mut p, engine) = alternator_fixture(1.0);
        s.crew.fill(0);
        s.electric[s.root].command_use = 1.0;
        let ec = s.net.resource("ElectricCharge").unwrap();
        s.net.flow_mode[ec] = ose_data::FlowMode::NoFlow;
        // The engine can fill its own tank but cannot power the isolated command core.
        s.net.tanks.push(crate::resources::Tank {
            part: engine,
            resource: ec,
            stored: 0.0,
            capacity: 10.0,
        });
        let before = s.net.tanks.clone();
        let out = s.powered_services(&mut p, 0.0, Default::default(), 0.1);
        assert_no_engine_impulse(out);
        assert!(!p.controlled);
        assert_eq!(p.throttle, 0.0);
        assert_eq!(s.net.tanks, before);
        assert_eq!(s.resolved_engine_thrust.as_ref().unwrap()[engine], 0.0);
    }

    #[test]
    fn rails_services_do_not_reuse_an_old_burn_but_standalone_generators_still_run() {
        let (mut s, mut p, _) = alternator_fixture(1.0);
        s.powered_services(&mut p, 0.0, Default::default(), 0.1);
        let before = ec_stored(&s);
        let fuel = s.net.mass_t();
        // Deliberately leave ignition/throttle and the previous thrust snapshot set.
        s.electric_services(&mut p, 0.0, 10.0);
        assert_eq!(ec_stored(&s), before);
        assert_eq!(s.net.mass_t(), fuel);
        s.electric[s.root].generator_output = 0.2;
        s.electric_services(&mut p, 0.0, 10.0);
        assert!((ec_stored(&s) - before - 2.0).abs() < 1e-9);
    }

    #[test]
    fn alternator_fuel_impulse_and_charge_are_invariant_under_timestep_subdivision() {
        let mut totals = Vec::new();
        for steps in [1, 4, 20] {
            let (mut s, mut p, _) = alternator_fixture(0.025);
            let mut impulse = 0.0;
            let mut burned = 0.0;
            let dt = 0.1 / steps as f64;
            for _ in 0..steps {
                let out = s.powered_services(&mut p, 0.0, Default::default(), dt);
                impulse += out.thrust_kn * dt;
                burned += out.burned_t;
            }
            assert!((burned - 0.025).abs() < 1e-9);
            assert!((impulse - 0.025 * 100.0 * ose_data::G0).abs() < 1e-9);
            assert!((ec_stored(&s) - 0.3).abs() < 1e-9);
            totals.push((impulse, burned, ec_stored(&s)));
        }
        for pair in totals.windows(2) {
            assert!((pair[0].0 - pair[1].0).abs() < 1e-9);
        }
    }

    #[test]
    fn converter_fuel_is_reserved_once_before_alternator_output_is_resolved() {
        let (mut s, mut p, engine) = alternator_fixture(0.075);
        let solid = s.net.resource("SolidFuel").unwrap();
        s.converter_defs[engine] = Some(ose_data::Converter {
            formula: "Analytical generator sharing engine fuel".into(),
            inputs: vec![("SolidFuel".into(), 0.5 / s.net.unit_mass_t[solid])],
            outputs: vec![("ElectricCharge".into(), 1.0)],
            conversion_rate: 1.0,
        });
        s.converter_on[engine] = true;
        s.crew.fill(0);
        s.electric[s.root].command_use = 0.5;
        let mass = s.net.mass_t();
        let out = s.powered_services(&mut p, 0.0, Default::default(), 0.1);
        assert!(p.controlled);
        // The converter uses 0.05 t and makes 0.1 EC; the final 0.025 t burn makes 0.3 EC.
        assert!((s.converter_running[engine] - 1.0).abs() < 1e-9);
        assert!((out.burned_t - 0.025).abs() < 1e-9);
        assert!((out.mass.mass_t - (mass - 0.05)).abs() < 1e-9);
        assert!((s.net.mass_t() - (mass - 0.075)).abs() < 1e-9);
        assert!((ec_stored(&s) - 0.35).abs() < 1e-9); // One 0.05 EC core request.
        let coast = s.powered_services(&mut p, 0.0, Default::default(), 0.1);
        assert_no_engine_impulse(coast);
        assert!((ec_stored(&s) - 0.30).abs() < 1e-9);
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
    #[test]
    fn empty_structure_has_no_drive_root() {
        let craft = Craft {
            name: "empty legacy craft".into(),
            description: String::new(),
            parts: Vec::new(),
            stages: Vec::new(),
            colors: Default::default(),
            loose: Vec::new(),
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
            let (mut s, mut p, _, _) = rcs_fixture();
            let i = s.root;
            let mut def = s.rcs_defs.iter().flatten().next().unwrap().clone();
            def.propellant = "SolidFuel".into();
            def.isp_curve.keys.iter_mut().for_each(|k| k.v = 100.);
            s.rcs_defs.fill(None);
            s.rcs_defs[i] = Some(def);
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
            s.resolve_nozzle_rcs(&mut p, &mut prop, &mut allocation, dt);
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
        s.resolve_wheel_power(&mut p, &test_body(), &mut engine, 0.02);
        let allocation = engine.control_allocation.unwrap();
        assert!((allocation.torque_knm.y - 1.2).abs() < 1e-10);
        for (a, input) in engine.actuators.unwrap().iter().zip(allocation.inputs) {
            if a.force_kn.length_squared() == 0.0 && a.axial.is_none() {
                assert!((input - 0.3).abs() < 1e-10);
            }
        }
        assert!((ec_stored(&s) - (20.0 - 2.0 * 0.6 * 3.0 * 0.02)).abs() < 1e-10);
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
}

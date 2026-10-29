//! Our own game-data schemas (parts, resources, celestial bodies, craft, missions).
//!
//! Imported from the KSP2 JSON by `ose-importer`, but owned and versioned here. Units are SI unless a field name
//! says otherwise: metres, seconds, kilograms, kilonewtons for thrust (as in the original data), kPa for pressure.

#![forbid(unsafe_code)]

pub mod craft;
pub mod flight_geometry;
pub mod missions;
pub mod science;
pub mod staging;
pub mod terrain;
pub mod terrain_detail;

use ose_math::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Schema version written into every data file and save.
pub const SCHEMA_VERSION: u32 = 1;

/// Simulation ticks per second of universal time.
pub const TICKS_PER_SECOND: u64 = 50;

/// Universal time in ticks since the start of the save. Shared by the sim, the protocol and the client.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Tick(pub u64);

impl Tick {
    pub fn seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }
}

/// Standard gravity used by the original game to turn `gravityASL` into a gravitational parameter.
pub const G0: f64 = 9.80665;
/// Newtonian constant of gravitation used by the original game.
pub const BIG_G: f64 = 6.6743e-11;

/// A piecewise curve sampled at key points (time, value, in/out tangent). Evaluated with cubic Hermite
/// interpolation like Unity's `AnimationCurve`, clamped outside the key range.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub keys: Vec<CurveKey>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CurveKey {
    pub t: f64,
    pub v: f64,
    pub in_tangent: f64,
    pub out_tangent: f64,
}

impl Curve {
    pub fn evaluate(&self, t: f64) -> f64 {
        let k = &self.keys;
        match k.len() {
            0 => 0.0,
            1 => k[0].v,
            _ => {
                if t <= k[0].t {
                    return k[0].v;
                }
                if t >= k[k.len() - 1].t {
                    return k[k.len() - 1].v;
                }
                let i = k.partition_point(|key| key.t <= t) - 1;
                let (a, b) = (&k[i], &k[i + 1]);
                let dt = b.t - a.t;
                if dt <= 0.0 {
                    return a.v;
                }
                let s = (t - a.t) / dt;
                let (s2, s3) = (s * s, s * s * s);
                let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
                let h10 = s3 - 2.0 * s2 + s;
                let h01 = -2.0 * s3 + 3.0 * s2;
                let h11 = s3 - s2;
                h00 * a.v + h10 * dt * a.out_tangent + h01 * b.v + h11 * dt * b.in_tangent
            }
        }
    }
}

/// A celestial body in the on-rails hierarchy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelestialBody {
    pub id: String,
    /// Localisation key or plain name.
    pub display_name: String,
    /// Parent body id; `None` for the root star.
    pub parent: Option<String>,
    pub is_star: bool,
    pub is_home_world: bool,
    pub radius_m: f64,
    /// Surface gravity in multiples of `G0`, as authored.
    pub gravity_asl: f64,
    /// Standard gravitational parameter, m^3/s^2 (`gravity_asl * G0 * radius^2`).
    pub mu: f64,
    /// Derived from `mu / BIG_G`.
    pub mass_kg: f64,
    /// `None` means infinite (the root body).
    pub soi_radius_m: Option<f64>,
    pub rotation_period_s: f64,
    pub initial_rotation_deg: f64,
    pub is_tidally_locked: bool,
    pub has_solid_surface: bool,
    pub ocean: Option<Ocean>,
    pub orbit: Option<Orbit>,
    pub atmosphere: Option<Atmosphere>,
    pub star_luminosity: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ocean {
    /// PQS height-map datum below sea level. The sea surface itself is at the body radius.
    pub altitude_m: f64,
    pub density: f64,
}

/// Classical orbital elements at a reference epoch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Orbit {
    pub semi_major_axis_m: f64,
    pub eccentricity: f64,
    pub inclination_deg: f64,
    pub longitude_of_ascending_node_deg: f64,
    pub argument_of_periapsis_deg: f64,
    pub mean_anomaly_at_epoch_rad: f64,
    pub epoch_s: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Atmosphere {
    pub depth_m: f64,
    pub contains_oxygen: bool,
    pub sea_level_pressure_kpa: f64,
    pub sea_level_temperature_k: f64,
    pub molar_mass_kg_per_mol: f64,
    pub adiabatic_index: f64,
    /// Pressure (kPa) against altitude (m).
    pub pressure_curve: Curve,
    /// Temperature (K) against altitude (m).
    pub temperature_curve: Curve,
}

/// How a resource flows between parts; values mirror the original data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowMode {
    Null,
    NoFlow,
    AllVessel,
    StagePriorityFlow,
    StackPrioritySearch,
    StageStackFlowBalance,
}

impl FlowMode {
    pub fn from_code(c: i64) -> Self {
        match c {
            1 => FlowMode::NoFlow,
            2 => FlowMode::AllVessel,
            3 => FlowMode::StagePriorityFlow,
            4 => FlowMode::StackPrioritySearch,
            5 => FlowMode::StageStackFlowBalance,
            _ => FlowMode::Null,
        }
    }
}

/// A resource type (Methane, Oxidizer, ElectricCharge...) or a recipe mixing several (Methalox).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceDef {
    pub id: String,
    pub display_name_key: String,
    pub abbreviation_key: String,
    pub kind: ResourceKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResourceKind {
    Basic {
        /// Tonnes per unit, as authored.
        mass_per_unit_t: f64,
        volume_per_unit: f64,
        specific_heat_capacity_per_unit: f64,
        flow_mode: FlowMode,
        pumpable: bool,
        cost_per_unit: f64,
        non_stageable: bool,
    },
    Recipe {
        ingredients: Vec<RecipeIngredient>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeIngredient {
    pub resource: String,
    pub units_per_recipe_unit: f64,
}

/// Rough extent of a part for thumbnails and editor placement: from the first drag cube, else the attach
/// node bounding box. Unity Y-up metres as authored.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartBounds {
    pub size_m: ose_math::DVec3,
    pub center_m: ose_math::DVec3,
}

/// A part definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartDef {
    #[serde(default)]
    pub flight_geometry: flight_geometry::FlightGeometry,
    pub id: String,
    pub title_key: String,
    pub category: String,
    pub family: String,
    pub size_category: String,
    pub stage_type: String,
    /// Dry mass in tonnes, as authored.
    pub mass_t: f64,
    /// Authored part centre of mass in Unity part-local axes (`coMassOffset`).
    #[serde(default)]
    pub mass_offset_m: ose_math::DVec3,
    /// Body drag application offset (`coPressureOffset`), Unity part-local axes.
    #[serde(default)]
    pub pressure_offset_m: ose_math::DVec3,
    #[serde(default)]
    pub lift_offset_m: ose_math::DVec3,
    #[serde(default)]
    pub drag_properties: DragProperties,

    pub cost: f64,
    pub crew_capacity: u32,
    pub max_temp_k: f64,
    pub crash_tolerance: f64,
    pub breaking_force: f64,
    pub breaking_torque: f64,
    pub fuel_cross_feed: bool,
    pub physics_mode: String,
    pub attach_rules: AttachRules,
    pub attach_nodes: Vec<AttachNode>,
    pub resource_containers: Vec<ResourceContainer>,
    pub engines: Vec<EngineMode>,
    pub drag_cubes: Vec<DragCube>,
    #[serde(default)]
    pub bounds: Option<PartBounds>,
    /// Reaction wheel torque (the larger of pitch and yaw), kN*m; 0 for parts without a wheel.
    #[serde(default)]
    pub reaction_wheel_torque_knm: f64,
    /// Per-axis pitch, roll, yaw torque in part-local X,Y,Z; old catalogs use the scalar fallback.
    #[serde(default)]
    pub reaction_wheel_axes_knm: Option<ose_math::DVec3>,
    /// Electric charge use and production of the part's modules.
    #[serde(default)]
    pub electric: Electric,
    /// Experiments the part can run.
    #[serde(default)]
    pub experiments: Vec<science::PartExperiment>,
    /// A docking port (`Data_DockingNode`).
    #[serde(default)]
    pub docking: Option<DockingPort>,
    /// A parachute (`Data_Parachute`), with its semi-deployed and deployed drag shapes.
    #[serde(default)]
    pub parachute: Option<Parachute>,
    /// Scales the part's thermal mass (`thermalMassModifier`).
    #[serde(default = "one")]
    pub thermal_mass_modifier: f64,
    /// A heat shield (`Data_Heatshield`).
    #[serde(default)]
    pub heatshield: Option<Heatshield>,
    /// A landing leg or wheel (`Data_WheelBase`, `Data_WheelSuspension`, `Data_Deployable`).
    #[serde(default)]
    pub landing_gear: Option<LandingGear>,
    /// A resource converter (`Data_ResourceConverter`: fuel cells, the fission reactor).
    #[serde(default)]
    pub converter: Option<Converter>,
    /// A decoupler's push when it fires (`Data_Decouple.ejectionForce`).
    #[serde(default)]
    pub ejection_force: f64,
    /// A fairing base (`Data_Fairing`, shielding what it covers until jettisoned) or a cargo bay
    /// (`Data_CargoBay`, shielding what is inside while closed).
    #[serde(default)]
    pub fairing: bool,
    #[serde(default)]
    pub cargo_bay: bool,
    /// RCS thrusters (`Data_RCS`).
    #[serde(default)]
    pub rcs: Option<Rcs>,
    /// EVA movement and rotation settings (`Data_Kerbal`).
    #[serde(default)]
    pub jetpack: Option<Jetpack>,
    /// A wing, fin or control surface (`Data_LiftingSurface`).
    #[serde(default)]
    pub lifting_surface: Option<LiftingSurface>,
    /// A control surface's deflection range, degrees, and the share of the surface that moves
    /// (`Data_ControlSurface.CtrlSurfaceRange`, `CtrlSurfaceArea`); 0 for fixed surfaces.
    #[serde(default)]
    pub ctrl_range_deg: f64,
    #[serde(default)]
    pub ctrl_area: f64,
    #[serde(default)]
    pub ctrl_pressure_offset_m: ose_math::DVec3,
    /// An air intake's area, m^2 (`Data_ResourceIntake.area`; jets with their own intake have it too).
    #[serde(default)]
    pub intake_area_m2: f64,
    /// Names of every module on the part, in order, so the sim knows what else to implement.
    pub module_names: Vec<String>,
    /// Module data not yet mapped into typed fields, keyed by module name.
    pub raw_modules: BTreeMap<String, serde_json::Value>,
}

/// Electric charge (units per second) used and made by a part's modules, from their `RequiredResources` /
/// `ResourceSettings` in the part data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Electric {
    /// A probe core's draw while it holds control (`Data_Command.requiredResources`; crewed pods draw none).
    pub command_use: f64,
    /// Reaction wheel draw at full input per kN*m of the |(pitch, yaw, roll)| torque.
    pub wheel_use_per_knm: f64,
    /// Source request acceptance minimum. `None` retains proportional delivery for older/custom data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wheel_acceptance_threshold: Option<f64>,
    /// Reaction wheel |(pitch, yaw, roll)| torque, kN*m.
    pub wheel_torque_vec_knm: f64,
    /// Solar panel output in full sun at the home world's distance (`Data_SolarPanel.ResourceSettings.Rate`).
    pub solar_output: f64,
    /// Whether the panel turns to track the sun (`Data_Deployable.isTracking`).
    pub solar_tracking: bool,
    /// The panel folds out (`Data_Deployable.extendable`) and starts folded: it makes nothing until deployed.
    #[serde(default)]
    pub solar_deployable: bool,
    /// Antenna range (`Data_Transmitter.CommunicationRange`), m; 0 without one.
    #[serde(default)]
    pub comm_range_m: f64,
    /// A probe core that needs a CommNet connection to be flown (`Data_Command.requiresCommNet`).
    #[serde(default)]
    pub command_requires_commnet: bool,
    /// Generator output (`Data_ModuleGenerator.ResourceSetting.Rate`): an engine's alternator while it runs,
    /// otherwise always on (RTGs).
    pub generator_output: f64,
    /// The antenna's data packet (`Data_Transmitter.DataPacketSize`), science data units, its time per packet
    /// (`DataTransmissionInterval`), s, and its draw while sending (`RequiredResources`), EC/s.
    #[serde(default)]
    pub transmit_packet: f64,
    #[serde(default)]
    pub transmit_interval_s: f64,
    #[serde(default)]
    pub transmit_ec_per_s: f64,
}

fn one() -> f64 {
    1.0
}

/// A resource converter as the original defines it (`Data_ResourceConverter`, its selected formula): while
/// switched on it takes its inputs and makes its outputs, each at its rate times `conversion_rate`, units/s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Converter {
    pub formula: String,
    pub inputs: Vec<(String, f64)>,
    pub outputs: Vec<(String, f64)>,
    pub conversion_rate: f64,
}

fn enabled() -> bool {
    true
}

/// Enable flags and body-lift multiplier from `Data_Drag`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DragProperties {
    pub drag_enabled: bool,
    pub body_lift_enabled: bool,
    pub body_lift_multiplier: f64,
}
impl Default for DragProperties {
    fn default() -> Self {
        Self {
            drag_enabled: true,
            body_lift_enabled: true,
            body_lift_multiplier: 1.0,
        }
    }
}

/// Static model reference frame for a lifting module, expressed in Unity part-local axes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceFrame {
    pub position_m: ose_math::DVec3,
    /// Columns of the model-to-part affine transform; include hierarchy scale for points.
    pub point_axes: [ose_math::DVec3; 3],
    /// Rotation-only columns, matching Unity TransformDirection (independent of scale).
    pub direction_axes: [ose_math::DVec3; 3],
}

impl SurfaceFrame {
    pub fn point(&self, v: ose_math::DVec3) -> ose_math::DVec3 {
        self.position_m
            + self.point_axes[0] * v.x
            + self.point_axes[1] * v.y
            + self.point_axes[2] * v.z
    }

    pub fn direction(&self, v: ose_math::DVec3) -> ose_math::DVec3 {
        self.direction_axes[0] * v.x + self.direction_axes[1] * v.y + self.direction_axes[2] * v.z
    }
}

/// A lifting surface as the original defines it (`Data_LiftingSurface`). Its axis and pressure offset
/// remain module-local; reference_frame resolves a named model transform when exported geometry exists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiftingSurface {
    #[serde(default)]
    pub transform_name: String,
    #[serde(default)]
    pub reference_frame: Option<SurfaceFrame>,
    /// Suppress this module's lift and internal drag while its named node is occupied.
    #[serde(default)]
    pub node_enabled: bool,
    #[serde(default)]
    pub attach_node_name: String,
    #[serde(default)]
    pub pressure_offset_m: ose_math::DVec3,
    #[serde(default)]
    pub perpendicular_only: bool,
    #[serde(default = "enabled")]
    pub use_internal_drag_model: bool,
    pub deflection_lift_coeff: f64,
    pub axis: ose_math::DVec3,
    pub omnidirectional: bool,
    /// The curve set it uses (`Default`, `SpeedBrake`, ... `Custom` keeps its own curves).
    pub curve_set: String,
    pub lift_curve: Curve,
    pub lift_mach_curve: Curve,
    pub drag_curve: Curve,
    pub drag_mach_curve: Curve,
}

/// Authored EVA jetpack force, resource and rotation settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Jetpack {
    pub move_force_kn: f64,
    pub rotate_torque_knm: f64,
    pub rotate_offset_m: f64,
    pub rotate_max_speed_deg_s: f64,
    pub reset_enabled: bool,
    pub reset_speed_deg_s: f64,
    pub reset_threshold_deg_s: f64,
    pub propellant: String,
    pub isp_curve: Curve,
}

impl Default for Jetpack {
    fn default() -> Self {
        // Migration fallback for old catalogs/saves, matching the installed eva_kerbal definition.
        Self {
            move_force_kn: 0.25,
            rotate_torque_knm: 0.125,
            rotate_offset_m: 0.25,
            rotate_max_speed_deg_s: 280.0,
            reset_enabled: true,
            reset_speed_deg_s: 200.0,
            reset_threshold_deg_s: 0.1,
            propellant: "MonoPropellant".into(),
            isp_curve: Curve {
                keys: [
                    (0.0, 240.0, 0.0),
                    (1.0, 100.0, -59.99975),
                    (4.0, 0.001, 0.0),
                ]
                .into_iter()
                .map(|(t, v, tangent)| CurveKey {
                    t,
                    v,
                    in_tangent: tangent,
                    out_tangent: tangent,
                })
                .collect(),
            },
        }
    }
}

/// RCS thrusters as the original defines them (`Data_RCS`): thrust per thruster, kN, the propellant
/// (mixture) and Isp against pressure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rcs {
    #[serde(default)]
    pub controls: RcsControls,
    /// Authored per-module enable state and thrust limit (0..1).
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub thrust_fraction: f64,
    /// Whether this module may fire inside a closed fairing/cargo bay.
    #[serde(default)]
    pub shielded_can_activate: bool,
    /// Individual directional nozzle points, Unity part-local axes; empty for legacy mount proxies.
    #[serde(default)]
    pub thrusters: Vec<ThrustTransform>,
    pub thrust_kn: f64,
    pub propellant: String,
    pub isp_curve: Curve,
}

/// Reference module input switches, in flight-control axes rather than mounted part axes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RcsControls {
    pub pitch: bool,
    pub yaw: bool,
    pub roll: bool,
    pub x: bool,
    pub y: bool,
    pub z: bool,
    pub fore_by_throttle: bool,
    pub full_thrust: bool,
    pub full_thrust_min: f64,
}

impl Default for RcsControls {
    fn default() -> Self {
        Self {
            pitch: true,
            yaw: true,
            roll: true,
            x: true,
            y: true,
            z: true,
            fore_by_throttle: false,
            full_thrust: false,
            full_thrust_min: 0.2,
        }
    }
}

impl RcsControls {
    pub fn rotation_axes(self) -> DVec3 {
        DVec3::new(
            u8::from(self.pitch) as f64,
            u8::from(self.roll) as f64,
            u8::from(self.yaw) as f64,
        )
    }

    /// Craft axes: right, fore and top. Explicit fore/aft overrides throttle outside the source deadband.
    pub fn translation(self, input: DVec3, throttle: f64) -> DVec3 {
        let fore = if input.y.abs() < 0.05 {
            if self.fore_by_throttle {
                throttle.clamp(0.0, 1.0)
            } else {
                0.0
            }
        } else {
            input.y.clamp(-1.0, 1.0)
        };
        let axis = |value: f64, enabled: bool| {
            if enabled && value.abs() >= 0.05 {
                value.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        DVec3::new(
            axis(input.x, self.x),
            if self.z { fore } else { 0.0 },
            axis(input.z, self.y),
        )
    }

    pub fn nozzle_input(self, effort: f64) -> f64 {
        let effort = effort.clamp(0.0, 1.0);
        if self.full_thrust && effort > 0.0 && // The reference compares single-precision module inputs at this boundary.
            (effort as f32) >= (self.full_thrust_min.clamp(0.0, 1.0) as f32)
        {
            1.0
        } else {
            effort
        }
    }
}

impl Rcs {
    pub fn limited_thrust_kn(&self) -> f64 {
        if self.enabled {
            self.thrust_kn.max(0.0) * self.thrust_fraction.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A landing leg or wheel as the original defines it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandingGear {
    /// A landing leg (`WheelType` LEG) rather than a wheel.
    pub leg: bool,
    /// Suspension travel, m (`suspensionDistance`).
    pub suspension_m: f64,
    /// Folds in and out (`Data_Deployable`), and how it starts.
    pub retractable: bool,
    pub starts_extended: bool,
    /// Its shape folded out (the `EXTENDED` drag cube), when it folds.
    pub extended: Option<PartBounds>,
    /// Wheel radius, m (`Data_WheelBase.Radius`).
    #[serde(default)]
    pub radius_m: f64,
    /// A drive motor (`Data_WheelMotor`).
    #[serde(default)]
    pub motor: Option<WheelMotor>,
    /// Brake torque, kN·m (`Data_WheelBrakes.MaxBrakeTorque`), and whether it steers (`Data_WheelSteering`).
    #[serde(default)]
    pub brake_torque_knm: f64,
    #[serde(default)]
    pub steerable: bool,
}

/// A wheel's drive motor as the original defines it (`Data_WheelMotor`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WheelMotor {
    /// Drive torque, kN·m, against ground speed, m/s (`torqueCurve`); none past its last key.
    pub torque_curve: Curve,
    /// Electric charge at full drive, units/s (`requiredResource.Rate`).
    pub ec_per_s: f64,
}

/// A Kerbal of the original's roster (`kerbal_variety_text`): full name, the veterans first, personality
/// (0..1) and looks (head type, hair, eyes, skin, colours ... by the original's keys).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KerbalDef {
    pub name: String,
    pub veteran: bool,
    pub bravery: f64,
    pub stupidity: f64,
    pub optimism: f64,
    pub constitution: f64,
    pub looks: BTreeMap<String, String>,
}

/// A heat shield as the original defines it (`Data_Heatshield`): above its ablation temperature it burns its
/// ablator and turns away re-entry heat when it faces the airflow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heatshield {
    /// Ablates above this temperature, K, shielding fully this far above it.
    pub ablation_temp_k: f64,
    pub ablation_max_over_k: f64,
    pub pyrolysis_loss_factor: f64,
    pub shielding_scale: f64,
    /// The way the shield faces, part space (Unity axes).
    pub shielding_direction: ose_math::DVec3,
    /// The ablator resource and its use per unit of heat removed.
    pub ablator: String,
    pub ablator_rate: f64,
}

/// A parachute as the original defines it (`Data_Parachute`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parachute {
    /// Opens partly once the air is this thick, atm (`defaultMinAirPressureToOpen`).
    pub min_pressure_atm: f64,
    /// Opens fully this high above the ground, m (`defaultDeployAltitude`).
    pub deploy_altitude_m: f64,
    /// Cut away once landed slower than this, m/s.
    pub auto_cut_speed_mps: f64,
    /// Seconds to open partly and fully (`semiDeploymentSpeed`, `deploymentSpeed` as animation rates).
    pub semi_deploy_rate: f64,
    pub deploy_rate: f64,
    pub semi_cube: Option<DragCube>,
    pub deployed_cube: Option<DragCube>,
}

/// A docking port as the original defines it (`Data_DockingNode`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockingPort {
    /// The attach node that docks (`AttachmentNodeId`, usually "top").
    pub node: String,
    /// Port sizes that fit (`NodeTypes`, like "size1").
    pub sizes: Vec<String>,
    /// Ports within this range, facing within `acquire_min_fwd_dot`, pull together, m.
    pub acquire_range_m: f64,
    pub acquire_min_fwd_dot: f64,
    /// Ports within this range, facing within `capture_min_fwd_dot`, slower than `capture_max_rvel`, latch.
    pub capture_range_m: f64,
    pub capture_min_fwd_dot: f64,
    pub capture_max_rvel_mps: f64,
    /// Push on undocking, kN.
    pub undock_ejection_kn: f64,
    /// Distance before the two can dock again after undocking, m.
    pub min_distance_to_reengage_m: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttachRules {
    pub stack: bool,
    pub srf_attach: bool,
    pub allow_stack: bool,
    pub allow_srf_attach: bool,
    pub allow_collision: bool,
    pub allow_dock: bool,
    pub allow_rotate: bool,
    pub allow_root: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttachNode {
    pub id: String,
    pub node_type: String,
    pub attach_method: String,
    pub position: ose_math::DVec3,
    pub orientation: ose_math::DVec3,
    pub size: i32,
    pub is_resource_crossfeed: bool,
    pub is_rigid: bool,
    pub angular_strength_multiplier: f64,
    /// Keep full aggregate strength for large nodes (`MultiJointFullBreakStrength`).
    #[serde(default)]
    pub full_break_strength: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceContainer {
    pub resource: String,
    pub capacity_units: f64,
    pub initial_units: f64,
}

/// Authored engine thrust transform, in Unity part-local axes. Force points opposite transform +Z.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThrustTransform {
    pub position_m: ose_math::DVec3,
    pub direction: ose_math::DVec3,
    pub weight: f64,
}

/// One engine mode (most engines have one; multi-mode engines like jets have several).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineMode {
    #[serde(default)]
    pub thrust_transforms: Vec<ThrustTransform>,
    pub id: String,
    pub engine_type: String,
    pub propellant_mixture: String,
    pub min_thrust_kn: f64,
    pub max_thrust_kn: f64,
    /// Isp (s) against atmospheric pressure (atm).
    pub isp_curve: Curve,
    pub throttle_locked: bool,
    pub allow_restart: bool,
    pub allow_shutdown: bool,
    pub gimbal_range_deg: Option<f64>,
    /// Breathes intake air (a jet): thrust scales with Mach (`velCurve`) and air density (`atmCurve`).
    #[serde(default)]
    pub air_breathing: bool,
    #[serde(default)]
    pub vel_curve: Option<Curve>,
    #[serde(default)]
    pub atm_curve: Option<Curve>,
}

/// Drag cube in the original six-face form: +X, -X, +Y, -Y, +Z, -Z.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DragCube {
    pub name: String,
    pub area: [f64; 6],
    pub drag: [f64; 6],
    pub depth: [f64; 6],
    pub center: ose_math::DVec3,
    pub size: ose_math::DVec3,
}

/// Everything the sim needs to start a universe, in one file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameData {
    pub schema_version: u32,
    pub bodies: Vec<CelestialBody>,
    pub resources: Vec<ResourceDef>,
    pub parts: Vec<PartDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_round_trips_through_json() {
        let kerbin = CelestialBody {
            id: "Kerbin".into(),
            display_name: "Kerbin".into(),
            parent: Some("Kerbol".into()),
            is_star: false,
            is_home_world: true,
            radius_m: 600_000.0,
            gravity_asl: 1.0,
            mu: G0 * 600_000.0f64.powi(2),
            mass_kg: G0 * 600_000.0f64.powi(2) / BIG_G,
            soi_radius_m: Some(84_159_286.0),
            rotation_period_s: 21_549.425,
            initial_rotation_deg: 90.0,
            is_tidally_locked: false,
            has_solid_surface: true,
            ocean: Some(Ocean {
                altitude_m: 4270.0,
                density: 1.0,
            }),
            orbit: None,
            atmosphere: None,
            star_luminosity: None,
        };
        let s = serde_json::to_string(&kerbin).unwrap();
        let back: CelestialBody = serde_json::from_str(&s).unwrap();
        assert_eq!(kerbin, back);
    }

    #[test]
    fn curve_interpolates_and_clamps() {
        let c = Curve {
            keys: vec![
                CurveKey {
                    t: 0.0,
                    v: 335.0,
                    in_tangent: 0.0,
                    out_tangent: 0.0,
                },
                CurveKey {
                    t: 1.0,
                    v: 170.0,
                    in_tangent: 0.0,
                    out_tangent: 0.0,
                },
            ],
        };
        assert_eq!(c.evaluate(-1.0), 335.0);
        assert_eq!(c.evaluate(0.0), 335.0);
        assert_eq!(c.evaluate(1.0), 170.0);
        assert_eq!(c.evaluate(5.0), 170.0);
        let mid = c.evaluate(0.5);
        assert!(
            (mid - 252.5).abs() < 1e-9,
            "flat tangents give the midpoint, got {mid}"
        );
    }
}

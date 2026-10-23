//! Our own game-data schemas (parts, resources, celestial bodies, craft, missions).
//!
//! Imported from the KSP2 JSON by `ose-importer`, but owned and versioned here. Units are SI unless a field name
//! says otherwise: metres, seconds, kilograms, kilonewtons for thrust (as in the original data), kPa for pressure.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Schema version written into every data file and save.
pub const SCHEMA_VERSION: u32 = 1;

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

/// A part definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartDef {
    pub id: String,
    pub title_key: String,
    pub category: String,
    pub family: String,
    pub size_category: String,
    pub stage_type: String,
    /// Dry mass in tonnes, as authored.
    pub mass_t: f64,
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
    /// Names of every module on the part, in order, so the sim knows what else to implement.
    pub module_names: Vec<String>,
    /// Module data not yet mapped into typed fields, keyed by module name.
    pub raw_modules: BTreeMap<String, serde_json::Value>,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceContainer {
    pub resource: String,
    pub capacity_units: f64,
    pub initial_units: f64,
}

/// One engine mode (most engines have one; multi-mode engines like jets have several).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineMode {
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

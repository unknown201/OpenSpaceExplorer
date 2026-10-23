//! Our own game-data schemas (parts, resources, celestial bodies, craft, missions).
//!
//! Imported from the KSP2 JSON by `ose-importer`, but owned and versioned here.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Schema version written into every data file and save.
pub const SCHEMA_VERSION: u32 = 1;

/// A celestial body in the on-rails hierarchy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelestialBody {
    pub id: String,
    pub display_name: String,
    /// Parent body id; `None` for the root star.
    pub parent: Option<String>,
    /// Standard gravitational parameter, m^3/s^2.
    pub mu: f64,
    pub radius_m: f64,
    pub soi_radius_m: Option<f64>,
    pub rotation_period_s: f64,
    pub orbit: Option<Orbit>,
    pub atmosphere: Option<Atmosphere>,
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
    pub height_m: f64,
    pub sea_level_pressure_kpa: f64,
}

/// A resource type (Methalox, Monopropellant, ElectricCharge...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceDef {
    pub id: String,
    pub display_name: String,
    pub density_kg_per_unit: f64,
}

/// A part definition. Modules are kept as tagged data until each module type is implemented.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartDef {
    pub id: String,
    pub display_name: String,
    pub category: String,
    pub mass_kg: f64,
    pub attach_nodes: Vec<AttachNode>,
    pub modules: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttachNode {
    pub id: String,
    pub position: ose_math::DVec3,
    pub orientation: ose_math::DVec3,
    pub size: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_round_trips_through_json() {
        let kerbin = CelestialBody {
            id: "kerbin".into(),
            display_name: "Kerbin".into(),
            parent: Some("kerbol".into()),
            mu: 3.5316e12,
            radius_m: 600_000.0,
            soi_radius_m: Some(84_159_286.0),
            rotation_period_s: 21_549.425,
            orbit: None,
            atmosphere: Some(Atmosphere {
                height_m: 70_000.0,
                sea_level_pressure_kpa: 101.325,
            }),
        };
        let s = serde_json::to_string(&kerbin).unwrap();
        let back: CelestialBody = serde_json::from_str(&s).unwrap();
        assert_eq!(kerbin, back);
    }
}

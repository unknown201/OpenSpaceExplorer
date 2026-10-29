//! Authored part-space geometry. Coordinates are Unity Y-up; the simulation reflects Z once.
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightGeometry {
    /// RigidbodyBehavior MassScaleType: 0 = physical, 2 = inverse mass.
    #[serde(default)]
    pub inertia_mode: u8,
    #[serde(default)]
    pub rcs_tuning: RcsTuning,
    #[serde(default)]
    pub control_rotation: Option<[f64; 4]>,
    #[serde(default)]
    pub surface: Option<ControlSurface>,
    pub mass_offset_m: DVec3,
    pub pressure_offset_m: DVec3,
    pub lift_offset_m: DVec3,
    /// Inertia per tonne about mass_offset_m, column-major, in part axes.
    pub inertia_per_t: Option<[DVec3; 3]>,
    pub inertia_source: String,
    pub engines: Vec<Vec<Nozzle>>,
    pub rcs: Vec<Nozzle>,
    pub gimbals: Vec<Gimbal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlSurface {
    #[serde(default = "surface_sign")]
    pub mirror_sign: f64,
    #[serde(default)]
    pub deploy: Option<SurfaceDeploy>,
    #[serde(default)]
    pub fixed_lift_coeff: Option<f64>,
    #[serde(default)]
    pub moving_lift_coeff: Option<f64>,
    pub normal: DVec3,
    pub axis: DVec3,
    pub pivot_m: DVec3,
    pub speed: f64,
    pub exponential: bool,
    pub ignore_sas: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Nozzle {
    pub position_m: DVec3,
    /// Force direction, not exhaust direction.
    pub direction: DVec3,
    pub weight: f64,
    pub gimbal: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gimbal {
    pub position_m: DVec3,
    pub rotation: [f64; 4],
    /// Positive/negative local X and Y travel, radians.
    pub limits_rad: [f64; 4],
    pub response_speed: Option<f64>,
    pub axis_mult: [f64; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartControl {
    pub wheel_enabled: bool,
    pub wheel_authority: f64,
    /// 0 = all, 1 = manual only, 2 = SAS only.
    pub wheel_mode: u8,
    pub gimbal_enabled: bool,
    pub gimbal_limiter: f64,
    pub pitch: bool,
    pub yaw: bool,
    pub roll: bool,
    pub rcs_enabled: bool,
    pub rcs_limiter: f64,
    pub surface_authority: f64,
    pub surface_deploy: bool,
    pub surface_deploy_fraction: f64,
    pub surface_axes: [bool; 3],
    pub surface_invert: bool,
    /// Pitch/roll/yaw followed by translation X/Y/Z in control axes.
    pub rcs_axes: [bool; 6],
    pub thrust_limiter: f64,
}
impl Default for PartControl {
    fn default() -> Self {
        Self {
            wheel_enabled: true,
            wheel_authority: 1.,
            wheel_mode: 0,
            gimbal_enabled: true,
            gimbal_limiter: 1.,
            pitch: true,
            yaw: true,
            roll: true,
            rcs_enabled: true,
            rcs_limiter: 1.,
            surface_authority: 1.,
            surface_deploy: false,
            surface_deploy_fraction: 1.,
            surface_axes: [true; 3],
            surface_invert: false,
            rcs_axes: [true; 6],
            thrust_limiter: 1.,
        }
    }
}
impl PartControl {
    pub fn valid(self) -> bool {
        self.wheel_mode <= 2
            && [
                self.wheel_authority,
                self.gimbal_limiter,
                self.rcs_limiter,
                self.surface_authority,
                self.surface_deploy_fraction,
                self.thrust_limiter,
            ]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RcsTuning {
    pub use_lever: bool,
    pub full_thrust: bool,
    pub full_thrust_min: f64,
    pub shielded_can_activate: bool,
}
impl Default for RcsTuning {
    fn default() -> Self {
        Self {
            use_lever: false,
            full_thrust: false,
            full_thrust_min: 0.2,
            shielded_can_activate: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceDeploy {
    pub axis: DVec3,
    pub pivot_m: DVec3,
    pub separate: bool,
    pub angle_rad: f64,
    pub min_control_angle_rad: f64,
}

/// Per-instance procedural shape measurements. They never change engine/wheel ratings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartFlight {
    #[serde(default)]
    pub mirrored: bool,
    pub part_id: String,
    pub dry_mass_t: Option<f64>,
    pub mass_offset_m: Option<DVec3>,
    pub inertia_per_t: Option<[DVec3; 3]>,
    pub surface: Option<SurfaceFlight>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceFlight {
    pub fixed_lift_coeff: f64,
    pub moving_lift_coeff: f64,
    pub pivot_m: DVec3,
    pub pressure_offset_m: DVec3,
    pub range_deg: f64,
    pub speed: f64,
}
impl PartFlight {
    pub fn valid(&self) -> bool {
        let vector = |v: DVec3| {
            [v.x, v.y, v.z]
                .iter()
                .all(|x| x.is_finite() && x.abs() < 1e4)
        };
        self.dry_mass_t
            .is_none_or(|m| m.is_finite() && (0.001..=1e6).contains(&m))
            && self.mass_offset_m.is_none_or(vector)
            && self.inertia_per_t.is_none_or(|t| {
                t.iter().all(|v| vector(*v))
                    && t[0].x > 0.
                    && t[0].x * t[1].y - t[0].y * t[1].x > 0.
                    && t[0].dot(t[1].cross(t[2])) > 0.
                    && (t[0].y - t[1].x).abs() < 1e-8
                    && (t[0].z - t[2].x).abs() < 1e-8
                    && (t[1].z - t[2].y).abs() < 1e-8
            })
            && self.surface.as_ref().is_none_or(|s| {
                vector(s.pivot_m)
                    && vector(s.pressure_offset_m)
                    && [
                        s.fixed_lift_coeff,
                        s.moving_lift_coeff,
                        s.range_deg,
                        s.speed,
                    ]
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1e4).contains(v))
            })
    }
}

fn surface_sign() -> f64 {
    1.
}

#[cfg(test)]
mod instance_tests {
    use super::*;
    #[test]
    fn instance_geometry_rejects_nonphysical_tensors_and_old_crafts_default_cleanly() {
        let mut f = PartFlight {
            part_id: "part".into(),
            inertia_per_t: Some([
                DVec3::new(1., 0., 0.),
                DVec3::new(0., 2., 0.),
                DVec3::new(0., 0., 3.),
            ]),
            ..Default::default()
        };
        assert!(f.valid());
        f.inertia_per_t.as_mut().unwrap()[1].y = -2.;
        assert!(!f.valid());
        f.inertia_per_t = None;
        f.dry_mass_t = Some(f64::NAN);
        assert!(!f.valid());
        f.dry_mass_t = Some(2.);
        let text = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<PartFlight>(&text).unwrap(), f);
    }
}

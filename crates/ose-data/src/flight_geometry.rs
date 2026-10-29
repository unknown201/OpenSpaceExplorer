//! Authored part-space geometry. Coordinates are Unity Y-up; the simulation reflects Z once.
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightGeometry {
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

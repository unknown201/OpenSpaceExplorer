//! Powered flight, version 1: a vessel as a point mass integrated in its reference body's inertial frame.
//!
//! This is the server-side flight model until the rigid-body bubble (Phase 6 proper) lands. It gives real
//! launches, ascents, burns that consume propellant, staging, atmospheric drag and landings, with the craft
//! reduced to a [`FlightSpec`] (per-stage thrust, Isp, propellant and dry mass from `ose_data::staging`).
//!
//! Frame: body-centred, inertial, Z-up, the body spinning about +Z at `2 pi / rotation_period`. Surface
//! velocity is `v - omega x r`. Atmosphere density follows the body's pressure and temperature curves
//! (`docs/reference-notes/physics-joints-drag.md`). Drag is `1/2 rho v^2 Cd A` on a single reference area.

use crate::bodies::Body;
use bevy_ecs::prelude::*;
use ose_data::{Curve, G0};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

pub const IDEAL_GAS_CONSTANT: f64 = 8.31447;
/// Touchdown faster than this (surface speed) counts as a crash.
pub const CRASH_SPEED_MPS: f64 = 12.0;
/// Highest warp allowed while any vessel is under powered flight (the original's physics warp).
pub const MAX_PHYSICS_WARP: u32 = 4;
/// Drag coefficient for the single reference area of a craft (tuned later against drag cubes).
pub const DEFAULT_CD: f64 = 0.35;

/// One stage of a craft as the flight model sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageSpec {
    pub stage_id: u32,
    /// Vacuum thrust of the engines running in this stage, kN.
    pub thrust_vac_kn: f64,
    /// Thrust at one atmosphere, kN (Isp scales thrust at constant mass flow).
    pub thrust_asl_kn: f64,
    pub isp_vac_s: f64,
    pub isp_asl_s: f64,
    /// Propellant this stage can burn, t.
    pub propellant_t: f64,
    /// Total mass at the start of the stage with full propellant, t.
    pub start_mass_t: f64,
}

/// The craft reduced to what the point-mass model needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlightSpec {
    /// Stages in firing order (first to fire first).
    pub stages: Vec<StageSpec>,
    /// Reference area for drag, m^2.
    pub drag_area_m2: f64,
    pub drag_cd: f64,
}

impl FlightSpec {
    /// Build from the staging report; `drag_area_m2` is the largest part cross-section (rough).
    pub fn from_report(
        report: &ose_data::staging::DeltaVReport,
        isp_asl_by_stage: &[f64],
        drag_area_m2: f64,
    ) -> FlightSpec {
        let mut stages = Vec::new();
        for (i, s) in report.stages.iter().enumerate() {
            if s.thrust_kn <= 0.0 && s.start_mass_t <= 0.0 {
                continue;
            }
            let prop = (s.start_mass_t - s.end_mass_t).max(0.0);
            let isp_vac = if s.delta_v_vacuum_mps > 0.0
                && s.end_mass_t > 0.0
                && s.start_mass_t > s.end_mass_t
            {
                s.delta_v_vacuum_mps / (G0 * (s.start_mass_t / s.end_mass_t).ln())
            } else {
                0.0
            };
            let isp_asl = isp_asl_by_stage.get(i).copied().unwrap_or(isp_vac);
            let thrust_asl = if isp_vac > 0.0 {
                s.thrust_kn * isp_asl / isp_vac
            } else {
                0.0
            };
            stages.push(StageSpec {
                stage_id: s.stage_id as u32,
                thrust_vac_kn: s.thrust_kn,
                thrust_asl_kn: thrust_asl,
                isp_vac_s: isp_vac,
                isp_asl_s: isp_asl,
                propellant_t: prop,
                start_mass_t: s.start_mass_t,
            });
        }
        FlightSpec {
            stages,
            drag_area_m2: drag_area_m2.max(0.1),
            drag_cd: DEFAULT_CD,
        }
    }
}

/// How the vessel points. Surface directions use the local up / east / north frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Attitude {
    /// Radially outward.
    Up,
    SurfacePrograde,
    SurfaceRetrograde,
    OrbitPrograde,
    OrbitRetrograde,
    Normal,
    AntiNormal,
    RadialOut,
    RadialIn,
    /// Pitch above the horizon (degrees) and compass heading (degrees, 90 = east), surface frame.
    PitchHeading {
        pitch_deg: f64,
        heading_deg: f64,
    },
    /// A fixed direction in the body's inertial frame (normalised on use).
    Fixed(DVec3),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Situation {
    Landed,
    Flying,
    SubOrbital,
    Orbit,
    Escaping,
    Crashed,
}

/// Powered-flight state. While present the vessel is integrated here and its `OnRails` orbit is refreshed
/// from `(r, v)` each step so everything that reads orbits keeps working.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Powered {
    pub spec: FlightSpec,
    /// Index into `spec.stages` of the stage currently able to burn; `>= len` when all are spent.
    pub stage: usize,
    pub propellant_t: f64,
    pub mass_t: f64,
    pub r: DVec3,
    pub v: DVec3,
    pub throttle: f64,
    pub attitude: Attitude,
    pub landed: bool,
    pub crashed: bool,
    /// Universal time since which the vessel has been coasting above the atmosphere (for the rails handoff).
    pub coasting_since: Option<f64>,
}

/// The flight state of a vessel that is on rails: stage, propellant and mass are kept so the vessel can
/// be put back under power (a burn from orbit, a descent) with what it had left. `r`/`v` are stale while
/// dormant and are refreshed from the rails orbit on wake.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dormant(pub Powered);

#[derive(Clone, Debug, PartialEq)]
pub struct Atmo {
    pub depth_m: f64,
    pub pressure_kpa: Curve,
    pub temperature_k: Curve,
    pub molar_mass: f64,
}

impl Atmo {
    pub fn pressure_kpa(&self, alt_m: f64) -> f64 {
        if alt_m >= self.depth_m {
            0.0
        } else {
            self.pressure_kpa.evaluate(alt_m).max(0.0)
        }
    }
    pub fn density(&self, alt_m: f64) -> f64 {
        let p = self.pressure_kpa(alt_m);
        let t = self.temperature_k.evaluate(alt_m);
        if p <= 0.0 || t <= 0.0 {
            0.0
        } else {
            p * 1000.0 * self.molar_mass / (IDEAL_GAS_CONSTANT * t)
        }
    }
}

/// Terrain height model attached to bodies that have a height map.
#[derive(Component, Clone, Debug)]
pub struct Terrain(pub std::sync::Arc<ose_data::terrain::TerrainMap>);

/// Rotation angle of a body at `ut`, radians (body-fixed longitude 0 is at this inertial angle).
pub fn rotation_angle(body: &Body, ut: f64) -> f64 {
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    body.initial_rotation_deg.to_radians() + omega * ut
}

/// Body-fixed latitude and longitude (degrees) of a body-centred inertial position at `ut`.
pub fn lat_lon(body: &Body, r: DVec3, ut: f64) -> (f64, f64) {
    let m = r.length().max(1e-9);
    let lat = (r.z / m).clamp(-1.0, 1.0).asin().to_degrees();
    let lon = (r.y.atan2(r.x) - rotation_angle(body, ut)).to_degrees();
    let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
    (lat, lon)
}

/// Radius of the ground (terrain or sea) under `r` at `ut`, m from the body centre.
pub fn ground_radius(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    r: DVec3,
    ut: f64,
) -> f64 {
    match terrain {
        Some(t) => {
            let (lat, lon) = lat_lon(body, r, ut);
            body.radius_m + t.ground_m(lat, lon)
        }
        None => body.radius_m,
    }
}

/// Atmosphere model attached to bodies that have one.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Atmosphere(pub Atmo);

impl Powered {
    pub fn new(spec: FlightSpec, r: DVec3, v: DVec3, landed: bool) -> Powered {
        let (mass, prop) = match spec.stages.first() {
            Some(s) => (s.start_mass_t, s.propellant_t),
            None => (1.0, 0.0),
        };
        Powered {
            spec,
            stage: 0,
            propellant_t: prop,
            mass_t: mass,
            r,
            v,
            throttle: 0.0,
            attitude: Attitude::Up,
            landed,
            crashed: false,
            coasting_since: None,
        }
    }

    pub fn current_stage(&self) -> Option<&StageSpec> {
        self.spec.stages.get(self.stage)
    }

    /// Fire the next stage: drop what the current stage leaves behind and take the next one's engines.
    pub fn stage_next(&mut self) -> bool {
        if self.stage + 1 >= self.spec.stages.len() {
            if self.stage < self.spec.stages.len() {
                // last stage spent: nothing left to drop but mark it done
                self.stage += 1;
                self.propellant_t = 0.0;
            }
            return false;
        }
        self.stage += 1;
        let s = &self.spec.stages[self.stage];
        self.mass_t = s.start_mass_t;
        self.propellant_t = s.propellant_t;
        true
    }

    pub fn thrust_kn(&self, pressure_atm: f64) -> f64 {
        match self.current_stage() {
            Some(s) if self.propellant_t > 0.0 && self.throttle > 0.0 => {
                let p = pressure_atm.clamp(0.0, 1.0);
                (s.thrust_vac_kn + (s.thrust_asl_kn - s.thrust_vac_kn) * p) * self.throttle
            }
            _ => 0.0,
        }
    }
}

/// Local surface frame at `r`: (up, east, north) for a body spinning about +Z.
pub fn surface_frame(r: DVec3) -> (DVec3, DVec3, DVec3) {
    let up = r.normalized();
    let z = DVec3::new(0.0, 0.0, 1.0);
    let mut east = z.cross(up);
    if east.length_squared() < 1e-12 {
        east = DVec3::new(0.0, 1.0, 0.0);
    }
    let east = east.normalized();
    let north = up.cross(east).normalized();
    (up, east, north)
}

/// Thrust direction for `attitude` given the state.
pub fn attitude_direction(att: Attitude, r: DVec3, v: DVec3, omega: f64) -> DVec3 {
    let (up, east, north) = surface_frame(r);
    let v_surf = v - DVec3::new(0.0, 0.0, omega).cross(r);
    let h = r.cross(v);
    let normal = if h.length_squared() > 0.0 {
        h.normalized()
    } else {
        up
    };
    let dir = match att {
        Attitude::Up => up,
        Attitude::SurfacePrograde => {
            if v_surf.length() > 1e-6 {
                v_surf.normalized()
            } else {
                up
            }
        }
        Attitude::SurfaceRetrograde => {
            if v_surf.length() > 1e-6 {
                -v_surf.normalized()
            } else {
                up
            }
        }
        Attitude::OrbitPrograde => {
            if v.length() > 1e-6 {
                v.normalized()
            } else {
                up
            }
        }
        Attitude::OrbitRetrograde => {
            if v.length() > 1e-6 {
                -v.normalized()
            } else {
                up
            }
        }
        Attitude::Normal => normal,
        Attitude::AntiNormal => -normal,
        Attitude::RadialOut => up,
        Attitude::RadialIn => -up,
        Attitude::PitchHeading {
            pitch_deg,
            heading_deg,
        } => {
            let (sp, cp) = pitch_deg.to_radians().sin_cos();
            let (sh, ch) = heading_deg.to_radians().sin_cos();
            up * sp + (east * sh + north * ch) * cp
        }
        Attitude::Fixed(d) => d,
    };
    dir.normalized()
}

pub struct StepOutcome {
    pub crashed: bool,
    pub touched_down: bool,
}

/// Integrate one `dt` of powered flight around `body`.
pub fn integrate(
    p: &mut Powered,
    body: &Body,
    atmo: Option<&Atmo>,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    ut: f64,
    dt: f64,
) -> StepOutcome {
    let mut out = StepOutcome {
        crashed: false,
        touched_down: false,
    };
    if p.crashed {
        return out;
    }
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let omega_v = DVec3::new(0.0, 0.0, omega);
    let r_mag = p.r.length();
    let alt = r_mag - body.radius_m;
    let (pressure_kpa, rho) = match atmo {
        Some(a) => (a.pressure_kpa(alt), a.density(alt)),
        None => (0.0, 0.0),
    };
    let pressure_atm = pressure_kpa / 101.325;

    // thrust and propellant
    let thrust_kn = p.thrust_kn(pressure_atm);
    let mut thrust_acc = DVec3::ZERO;
    if thrust_kn > 0.0 {
        let s = p.current_stage().expect("stage");
        let isp = s.isp_vac_s + (s.isp_asl_s - s.isp_vac_s) * pressure_atm.clamp(0.0, 1.0);
        let flow = if isp > 0.0 {
            thrust_kn / (isp * G0)
        } else {
            0.0
        }; // t/s
        let burn = (flow * dt).min(p.propellant_t);
        let frac = if flow * dt > 0.0 {
            burn / (flow * dt)
        } else {
            0.0
        };
        let dir = attitude_direction(p.attitude, p.r, p.v, omega);
        thrust_acc = dir * (thrust_kn * frac / p.mass_t); // kN / t = m/s^2
        p.propellant_t -= burn;
        p.mass_t -= burn;
    }

    if p.landed {
        // sitting on the surface: co-rotate; lift off only when thrust beats gravity
        let g = body.mu / (r_mag * r_mag);
        let (up, _, _) = surface_frame(p.r);
        if thrust_acc.dot(up) > g {
            p.landed = false;
        } else {
            let ang = omega * dt;
            let (s, c) = ang.sin_cos();
            p.r = DVec3::new(p.r.x * c - p.r.y * s, p.r.x * s + p.r.y * c, p.r.z);
            p.v = omega_v.cross(p.r);
            return out;
        }
    }

    // gravity + drag
    let g_acc = p.r * (-body.mu / (r_mag * r_mag * r_mag));
    let v_surf = p.v - omega_v.cross(p.r);
    let drag_acc = if rho > 0.0 && v_surf.length_squared() > 0.0 {
        let q = 0.5 * rho * v_surf.length_squared();
        let f_n = q * p.spec.drag_cd * p.spec.drag_area_m2; // newtons
        -v_surf.normalized() * (f_n / (p.mass_t * 1000.0))
    } else {
        DVec3::ZERO
    };
    let acc = g_acc + thrust_acc + drag_acc;
    p.v = p.v + acc * dt;
    p.r = p.r + p.v * dt;

    // surface contact against the terrain (or sea) under the vessel
    let new_r = p.r.length();
    let ground = ground_radius(body, terrain, p.r, ut + dt);
    if new_r <= ground {
        let v_surf = p.v - omega_v.cross(p.r);
        let (up, _, _) = surface_frame(p.r);
        let speed = v_surf.length();
        p.r = up * ground;
        p.v = omega_v.cross(p.r);
        p.landed = true;
        out.touched_down = true;
        if speed > CRASH_SPEED_MPS {
            p.crashed = true;
            p.throttle = 0.0;
            out.crashed = true;
        }
    }

    // coasting bookkeeping for the rails handoff
    let above_atmo = atmo.map(|a| alt > a.depth_m).unwrap_or(true);
    if p.throttle <= 0.0 && !p.landed && above_atmo {
        if p.coasting_since.is_none() {
            p.coasting_since = Some(ut);
        }
    } else {
        p.coasting_since = None;
    }
    out
}

/// Classify the flight state.
pub fn situation(p: &Powered, body: &Body, atmo: Option<&Atmo>) -> Situation {
    if p.crashed {
        return Situation::Crashed;
    }
    if p.landed {
        return Situation::Landed;
    }
    let alt = p.r.length() - body.radius_m;
    if let Some(a) = atmo {
        if alt < a.depth_m {
            return Situation::Flying;
        }
    }
    let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, 0.0);
    if orbit.eccentricity() >= 1.0 {
        return Situation::Escaping;
    }
    let atmo_top = body.radius_m + atmo.map(|a| a.depth_m).unwrap_or(0.0);
    if orbit.periapsis() < atmo_top {
        Situation::SubOrbital
    } else {
        Situation::Orbit
    }
}

/// Position on the surface at geographic latitude/longitude (degrees) at time `ut`, and its co-rotating
/// velocity. Longitude 0 is the body's +X axis at `initial_rotation_deg = 0`, `ut = 0`.
pub fn surface_state(body: &Body, lat_deg: f64, lon_deg: f64, ut: f64) -> (DVec3, DVec3) {
    surface_state_at(body, lat_deg, lon_deg, 0.0, ut)
}

/// Like [`surface_state`] but `altitude_m` above the nominal radius.
pub fn surface_state_at(
    body: &Body,
    lat_deg: f64,
    lon_deg: f64,
    altitude_m: f64,
    ut: f64,
) -> (DVec3, DVec3) {
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let theta = rotation_angle(body, ut) + lon_deg.to_radians();
    let (sl, cl) = lat_deg.to_radians().sin_cos();
    let r = DVec3::new(cl * theta.cos(), cl * theta.sin(), sl) * (body.radius_m + altitude_m);
    let v = DVec3::new(0.0, 0.0, omega).cross(r);
    (r, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kerbin() -> Body {
        let mu = 1.00034160493135 * G0 * 600_000.0 * 600_000.0;
        Body {
            id: "Kerbin".into(),
            mu,
            mass_kg: mu / ose_data::BIG_G,
            radius_m: 600_000.0,
            soi_radius_m: Some(84_159_286.0),
            rotation_period_s: 21_549.425,
            initial_rotation_deg: 0.0,
            is_star: false,
        }
    }

    fn kerbin_atmo() -> Atmo {
        // exponential-ish pressure with 5.6 km scale height, 70 km depth; temperature 290 K to 180 K
        let mut keys = Vec::new();
        let mut tkeys = Vec::new();
        let n = 30;
        for i in 0..=n {
            let h = 70_000.0 * i as f64 / n as f64;
            keys.push(ose_data::CurveKey {
                t: h,
                v: 101.325 * (-h / 5600.0).exp(),
                in_tangent: 0.0,
                out_tangent: 0.0,
            });
            tkeys.push(ose_data::CurveKey {
                t: h,
                v: 290.0 - 110.0 * h / 70_000.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            });
        }
        Atmo {
            depth_m: 70_000.0,
            pressure_kpa: Curve { keys },
            temperature_k: Curve { keys: tkeys },
            molar_mass: 0.0289644,
        }
    }

    fn rocket() -> FlightSpec {
        // two stages: booster 2000 kN, Isp 290/320, 60 t prop, 70 t total at liftoff (dry below ~7 t)
        // upper: 250 kN, Isp 280/345, 12 t prop, 16 t total
        FlightSpec {
            stages: vec![
                StageSpec {
                    stage_id: 1,
                    thrust_vac_kn: 2000.0,
                    thrust_asl_kn: 1812.0,
                    isp_vac_s: 320.0,
                    isp_asl_s: 290.0,
                    propellant_t: 50.0,
                    start_mass_t: 72.0,
                },
                StageSpec {
                    stage_id: 0,
                    thrust_vac_kn: 250.0,
                    thrust_asl_kn: 203.0,
                    isp_vac_s: 345.0,
                    isp_asl_s: 280.0,
                    propellant_t: 12.0,
                    start_mass_t: 16.0,
                },
            ],
            drag_area_m2: 5.0,
            drag_cd: DEFAULT_CD,
        }
    }

    #[test]
    fn sits_on_the_pad_until_thrust_beats_gravity_then_climbs() {
        let body = kerbin();
        let atmo = kerbin_atmo();
        let (r, v) = surface_state(&body, -0.1, -74.5, 0.0);
        let mut p = Powered::new(rocket(), r, v, true);
        for i in 0..50 {
            integrate(&mut p, &body, Some(&atmo), None, i as f64 * 0.02, 0.02);
        }
        assert!(p.landed, "no thrust: stays landed");
        assert!((p.r.length() - body.radius_m).abs() < 1e-6);
        p.throttle = 1.0;
        p.attitude = Attitude::Up;
        let mut ut = 1.0;
        for _ in 0..(10 * 50) {
            integrate(&mut p, &body, Some(&atmo), None, ut, 0.02);
            ut += 0.02;
        }
        let alt = p.r.length() - body.radius_m;
        assert!(
            !p.landed && alt > 200.0 && alt < 2000.0,
            "alt after 10 s {alt}"
        );
        assert!(p.mass_t < 72.0 && p.propellant_t < 50.0);
        assert_eq!(situation(&p, &body, Some(&atmo)), Situation::Flying);
    }

    #[test]
    fn gravity_turn_reaches_a_stable_orbit() {
        let body = kerbin();
        let atmo = kerbin_atmo();
        let (r, v) = surface_state(&body, -0.1, -74.5, 0.0);
        let mut p = Powered::new(rocket(), r, v, true);
        p.throttle = 1.0;
        let dt = 0.02;
        let mut ut = 0.0;
        let target_ap = 80_000.0;
        // ascent: pitch over from 90 deg at 1 km to 0 deg at 45 km, heading east; coast after Ap reached
        let mut coasting = false;
        for _ in 0..(1500 * 50) {
            let alt = p.r.length() - body.radius_m;
            let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
            let ap_alt = orbit.apoapsis() - body.radius_m;
            if !coasting {
                let pitch = if alt < 1_000.0 {
                    90.0
                } else {
                    (90.0 * (1.0 - (alt - 1_000.0) / 44_000.0)).clamp(0.0, 90.0)
                };
                p.attitude = Attitude::PitchHeading {
                    pitch_deg: pitch,
                    heading_deg: 90.0,
                };
                p.throttle = 1.0;
                if p.propellant_t <= 0.0 && p.stage == 0 {
                    p.stage_next();
                }
                if ap_alt >= target_ap && orbit.eccentricity() < 1.0 {
                    coasting = true;
                    p.throttle = 0.0;
                }
            } else {
                // circularise near apoapsis
                let t_ap = orbit.time_to_true_anomaly(std::f64::consts::PI, ut);
                let pe_alt = orbit.periapsis() - body.radius_m;
                if pe_alt > 72_000.0 {
                    p.throttle = 0.0;
                    break;
                }
                if t_ap < 20.0 || t_ap > orbit.period - 20.0 {
                    if p.propellant_t <= 0.0 && p.stage == 0 {
                        p.stage_next();
                    }
                    p.attitude = Attitude::OrbitPrograde;
                    p.throttle = 1.0;
                } else {
                    p.throttle = 0.0;
                }
            }
            let out = integrate(&mut p, &body, Some(&atmo), None, ut, dt);
            assert!(!out.crashed, "crashed at ut {ut}");
            ut += dt;
        }
        let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
        let pe = orbit.periapsis() - body.radius_m;
        let ap = orbit.apoapsis() - body.radius_m;
        assert!(
            pe > 70_000.0 && ap < 250_000.0,
            "orbit {pe:.0} x {ap:.0} m after {ut:.0} s, prop left {:.2} t, stage {}",
            p.propellant_t,
            p.stage
        );
        assert_eq!(situation(&p, &body, Some(&atmo)), Situation::Orbit);
        assert!(p.propellant_t > 0.0, "some margin left");
    }

    #[test]
    fn touches_down_on_terrain_not_the_nominal_radius() {
        use ose_data::terrain::{BodyTerrain, MapRef, TerrainMap};
        let body = kerbin();
        // flat map 2 km above the nominal radius
        let w = 32;
        let h = 16;
        let mut blob = Vec::new();
        for _ in 0..w * h {
            blob.extend_from_slice(&(65535u16 / 2).to_le_bytes());
        }
        let entry = BodyTerrain {
            id: "Kerbin".into(),
            has_ocean: false,
            sites: vec![],
            map: Some(MapRef {
                offset: 0,
                width: w,
                height: h,
                scale_m: 4000.0,
                offset_m: 0.0,
                source: String::new(),
            }),
        };
        let t = TerrainMap::from_entry(&entry, &blob, body.radius_m).unwrap();
        let ground = t.ground_m(0.0, 0.0);
        assert!((ground - 2000.0).abs() < 1.0, "ground {ground}");
        let (r, v) = surface_state_at(&body, 0.0, 0.0, 2050.0, 0.0);
        let up = r.normalized();
        let mut p = Powered::new(rocket(), r, v - up * 3.0, false);
        let mut landed_at = None;
        for i in 0..2000 {
            let ut = i as f64 * 0.02;
            if integrate(&mut p, &body, None, Some(&t), ut, 0.02).touched_down {
                landed_at = Some(p.r.length() - body.radius_m);
                break;
            }
        }
        let alt = landed_at.expect("touched down");
        assert!(
            (alt - 2000.0).abs() < 1.0,
            "landed at {alt} m, terrain at 2000 m"
        );
        let (lat, lon) = lat_lon(&body, p.r, 0.0);
        assert!(lat.abs() < 0.1 && lon.abs() < 1.0, "lat {lat} lon {lon}");
    }

    #[test]
    fn hard_landing_crashes_soft_landing_does_not() {
        let body = kerbin();
        let (r0, _) = surface_state(&body, 0.0, 0.0, 0.0);
        let up = r0.normalized();
        // drop from 50 m with no thrust: ~31 m/s at impact -> crash
        let mut p = Powered::new(
            rocket(),
            up * (body.radius_m + 50.0),
            DVec3::new(0.0, 0.0, 1.0).cross(up * (body.radius_m + 50.0))
                * (std::f64::consts::TAU / body.rotation_period_s),
            false,
        );
        let mut crashed = false;
        for i in 0..1000 {
            if integrate(&mut p, &body, None, None, i as f64 * 0.02, 0.02).crashed {
                crashed = true;
                break;
            }
        }
        assert!(crashed);
        // descend at 2 m/s from 20 m: lands
        let mut q = Powered::new(
            rocket(),
            up * (body.radius_m + 20.0),
            DVec3::new(0.0, 0.0, 1.0).cross(up * (body.radius_m + 20.0))
                * (std::f64::consts::TAU / body.rotation_period_s)
                - up * 2.0,
            false,
        );
        q.attitude = Attitude::Up;
        // hover-ish throttle: thrust = weight -> throttle = m g / T_vac
        let g = body.mu / (body.radius_m * body.radius_m);
        q.stage_next();
        // a little under hover so it keeps descending as propellant burns off
        q.throttle = (0.9 * q.mass_t * g / q.spec.stages[1].thrust_vac_kn).min(1.0);
        let mut landed = false;
        for i in 0..5000 {
            let out = integrate(&mut q, &body, None, None, i as f64 * 0.02, 0.02);
            assert!(!out.crashed, "soft descent crashed at step {i}");
            if out.touched_down {
                landed = true;
                break;
            }
        }
        assert!(landed);
        assert_eq!(situation(&q, &body, None), Situation::Landed);
    }
}

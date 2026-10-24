//! Powered landing from a low orbit, flown through `SetControls` and `Stage`.
//!
//! Brake: hold surface-retrograde at full throttle until the surface speed is small. Descend: hold
//! surface-retrograde (vertical once slow) and fly a descent-speed profile `v(h) = sqrt(2 a h)` against the
//! height above the terrain under the craft, which the bot samples from the same height maps the server
//! uses (`assets/own/terrain`). The server decides the touchdown: under 12 m/s is `Landed`, faster `Crashed`.

use crate::client::Client;
use anyhow::{bail, Context, Result};
use ose_data::terrain::{TerrainIndex, TerrainMap};
use ose_math::DVec3;
use ose_net::{AttitudeMode, FlightSituation, ServerMessage};
use std::io::Write;
use std::time::{Duration, Instant};

/// The terrain height map of body `id`, from the committed `assets/own/terrain`.
pub fn load_terrain(id: &str, radius_m: f64) -> Result<Option<TerrainMap>> {
    let dir = crate::data::repo_root().join("assets/own/terrain");
    let index: TerrainIndex = serde_json::from_str(
        &std::fs::read_to_string(dir.join("index.json")).context("read terrain index")?,
    )?;
    let Some(entry) = index.bodies.iter().find(|b| b.id == id) else {
        return Ok(None);
    };
    let blob = std::fs::read(dir.join("terrain.bin")).context("read terrain.bin")?;
    Ok(TerrainMap::from_entry(entry, &blob, radius_m))
}

/// Body-fixed latitude and longitude (degrees) of body-relative inertial position `r` at `ut`, with the
/// server's convention (longitude 0 on +X at `initial_rotation_deg = 0`, `ut = 0`).
pub fn lat_lon(r: DVec3, spin: &ose_net::BodySpin, ut: f64) -> (f64, f64) {
    let m = r.length().max(1e-9);
    let lat = (r.z / m).clamp(-1.0, 1.0).asin().to_degrees();
    let omega = if spin.rotation_period_s > 0.0 {
        std::f64::consts::TAU / spin.rotation_period_s
    } else {
        0.0
    };
    let rot = spin.initial_rotation_deg.to_radians() + omega * ut;
    let lon = (r.y.atan2(r.x) - rot).to_degrees();
    (lat, (lon + 180.0).rem_euclid(360.0) - 180.0)
}

/// Equatorial surface speed of a spinning body, m/s.
pub fn equatorial_speed(radius_m: f64, spin: &ose_net::BodySpin) -> f64 {
    if spin.rotation_period_s > 0.0 {
        std::f64::consts::TAU * radius_m / spin.rotation_period_s
    } else {
        0.0
    }
}

/// Why landing on this body cannot work with the data the server has, if it cannot.
pub fn landing_precondition(mu: f64, radius_m: f64, spin: &ose_net::BodySpin) -> Option<String> {
    let v_surface = equatorial_speed(radius_m, spin);
    let v_orbit = (mu / radius_m).sqrt();
    (v_surface > v_orbit).then(|| {
        format!(
            "the body spins once every {:.0} s, so its equator moves at {:.0} m/s, faster than orbital speed \
             at the surface ({:.0} m/s); a tidally locked moon should turn once per orbit",
            spin.rotation_period_s, v_surface, v_orbit
        )
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Landing {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub altitude_m: f64,
    pub touchdown_speed_mps: f64,
    pub propellant_t: f64,
    pub game_time_s: f64,
    pub wall_time_s: f64,
    pub stages_fired: u32,
    /// Whether the craft had folded landing legs that the bot extended.
    pub gear_deployed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Brake,
    Descend,
}

/// Fly `vessel` from orbit to the surface of the body at catalog index `body`.
pub async fn land(c: &mut Client, vessel: u64, body: u16, timeout: Duration) -> Result<Landing> {
    let info = c.body(body).cloned().context("body not in catalog")?;
    let spin = c
        .latest
        .spins
        .get(body as usize)
        .cloned()
        .context("no BodyRotation for the body")?;
    let terrain = load_terrain(&info.id, info.radius_m)?;
    let (mu, radius) = (info.mu, info.radius_m);
    let start_wall = Instant::now();
    let start_ut = c.latest.ut();
    let mut phase = Phase::Brake;
    let mut last_cmd: Option<(f64, AttitudeMode)> = None;
    let mut last_stage_ut = f64::NEG_INFINITY;
    let mut stages_fired = 0;
    let mut touch_speed = 0.0_f64;
    let mut real_time = false;
    let mut gear_requested = false;
    let mut gear_deployed = false;
    let mut trace = match std::env::var_os("OSE_BOT_LANDING_TRACE") {
        Some(path) => {
            let mut f = std::fs::File::create(path)?;
            writeln!(
                f,
                "t_s,phase,alt_m,ground_m,h_m,vz_mps,horizontal_mps,v_target_mps,throttle_cmd,throttle,tilt_deg,twr,stage,prop_frac"
            )?;
            Some(f)
        }
        None => None,
    };
    let mut last_trace_ut = f64::NEG_INFINITY;
    c.request_warp(ose_sim::flight::MAX_PHYSICS_WARP).await?;
    // wake the flight model: SetControls with throttle > 0 takes the craft off rails
    c.set_controls(vessel, 1.0, AttitudeMode::SurfaceRetrograde)
        .await?;
    last_cmd = last_cmd.or(Some((1.0, AttitudeMode::SurfaceRetrograde)));

    loop {
        if start_wall.elapsed() > timeout {
            c.request_warp(1).await.ok();
            bail!("landing did not finish within {timeout:?} (phase {phase:?})");
        }
        let msg = c.poll(Duration::from_secs(10)).await?;
        if let ServerMessage::FlightAck {
            vessel: v,
            accepted,
            reason,
        } = &msg
        {
            let gear_answer = reason.as_deref().is_some_and(|r| r.contains("gear"));
            if *v == vessel && !*accepted && gear_answer {
                // a craft without folded legs lands on whatever it has
                continue;
            }
            if *v == vessel && !*accepted {
                bail!(
                    "server refused a flight command: {}",
                    reason.clone().unwrap_or_default()
                );
            }
        }
        let ServerMessage::Vessels { .. } = msg else {
            continue;
        };
        let ut = c.latest.ut();
        let v = c
            .latest
            .vessel(vessel)
            .cloned()
            .context("vessel vanished during the landing")?;
        let Some(f) = v.flight.clone() else {
            continue; // still waking
        };
        let body_pos = c
            .latest
            .bodies
            .get(body as usize)
            .map(|b| b.position_m)
            .unwrap_or(DVec3::ZERO);
        let r = v.position_m - body_pos;
        let (lat, lon) = lat_lon(r, &spin, ut);
        match f.situation {
            FlightSituation::Landed => {
                c.set_controls(vessel, 0.0, AttitudeMode::Up).await?;
                c.request_warp(1).await?;
                return Ok(Landing {
                    lat_deg: lat,
                    lon_deg: lon,
                    altitude_m: f.altitude_m,
                    touchdown_speed_mps: touch_speed,
                    propellant_t: f.propellant_t,
                    game_time_s: ut - start_ut,
                    wall_time_s: start_wall.elapsed().as_secs_f64(),
                    stages_fired,
                    gear_deployed,
                });
            }
            FlightSituation::Crashed => bail!(
                "crashed at lat {lat:.3} lon {lon:.3}, last seen at {touch_speed:.1} m/s ({phase:?})"
            ),
            _ => {}
        }
        let ground = terrain
            .as_ref()
            .map(|t| t.ground_m(lat, lon))
            .unwrap_or(0.0);
        let h = (f.altitude_m - ground).max(0.0);
        let g = mu / (radius + f.altitude_m).powi(2);
        let a_max = (f.twr * g).max(0.01);
        let speed = f.surface_speed_mps;
        touch_speed = speed;
        let descending = -f.vertical_speed_mps;
        let horizontal = (speed * speed - f.vertical_speed_mps.powi(2))
            .max(0.0)
            .sqrt();

        // The descent-speed profile: brake at a modest fraction of what the engines can do, never faster than
        // 200 m/s, and settle at 1.5 m/s for the last few metres.
        let a_profile = (0.3 * (a_max - g)).clamp(0.5, 3.0);
        let v_target = if h < 8.0 {
            1.5
        } else {
            (2.0 * a_profile * h).sqrt().clamp(1.5, 200.0)
        };
        if phase == Phase::Brake && (horizontal < 20.0 || descending > v_target + 5.0) {
            phase = Phase::Descend;
        }
        // Legs come out a couple of kilometres up, well before touchdown.
        if !gear_requested && h < 2_000.0 {
            c.deploy_gear(vessel, true).await?;
            gear_requested = true;
        }
        if gear_requested && !gear_deployed {
            gear_deployed = v
                .flight
                .as_ref()
                .is_some_and(|fl| fl.gear.iter().any(|&(_, out)| out));
        }
        // Near the ground the bot needs every snapshot: at the physics warp cap each one is 0.4 s of flight.
        if !real_time && h < 3_000.0 {
            c.request_warp(1).await?;
            real_time = true;
        }
        let up = r.normalized();
        let cos_tilt = f.facing.dot(up).max(0.2);
        let (throttle, attitude) = match phase {
            Phase::Brake => {
                // keep braking, but never let the descent outrun what the engines can stop
                (1.0, AttitudeMode::SurfaceRetrograde)
            }
            Phase::Descend => {
                let thr = (g + 2.0 * (descending - v_target)) / (a_max * cos_tilt);
                let att = if speed > 4.0 && horizontal > 1.0 {
                    AttitudeMode::SurfaceRetrograde
                } else {
                    AttitudeMode::Up
                };
                (thr.clamp(0.0, 1.0), att)
            }
        };

        if let Some(tf) = trace.as_mut() {
            if ut - last_trace_ut >= if real_time { 0.2 } else { 2.0 } {
                last_trace_ut = ut;
                writeln!(
                    tf,
                    "{:.1},{:?},{:.1},{:.1},{:.1},{:.2},{:.2},{:.2},{:.3},{:.3},{:.1},{:.2},{},{:.3}",
                    ut - start_ut,
                    phase,
                    f.altitude_m,
                    ground,
                    h,
                    f.vertical_speed_mps,
                    horizontal,
                    v_target,
                    throttle,
                    f.throttle,
                    cos_tilt.clamp(-1.0, 1.0).acos().to_degrees(),
                    f.twr,
                    f.stage,
                    f.propellant_fraction
                )?;
            }
        }

        let dry = f.propellant_fraction <= 1e-6;
        if throttle > 0.0 && dry && f.stage + 1 < f.stages && ut - last_stage_ut > 1.0 {
            c.stage(vessel).await?;
            stages_fired += 1;
            last_stage_ut = ut;
        } else if throttle > 0.0 && dry && f.stage + 1 >= f.stages {
            bail!(
                "out of propellant {h:.0} m above the ground, falling at {descending:.0} m/s ({phase:?})"
            );
        }
        let changed = match last_cmd {
            None => true,
            Some((t, a)) => (t - throttle).abs() > 0.02 || a != attitude,
        };
        if changed {
            c.set_controls(vessel, throttle, attitude).await?;
            last_cmd = Some((throttle, attitude));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_100_second_mun_day_is_flagged() {
        let mu = 6.5138e10;
        let spin = ose_net::BodySpin {
            rotation_period_s: 100.0,
            initial_rotation_deg: 0.0,
        };
        assert!(landing_precondition(mu, 200_000.0, &spin).is_some());
        let locked = ose_net::BodySpin {
            rotation_period_s: 138_984.38,
            initial_rotation_deg: 230.0,
        };
        assert!(landing_precondition(mu, 200_000.0, &locked).is_none());
    }

    #[test]
    fn lat_lon_matches_the_servers_frame() {
        let spin = ose_net::BodySpin {
            rotation_period_s: 0.0,
            initial_rotation_deg: 90.0,
        };
        let (lat, lon) = lat_lon(DVec3::new(0.0, 1.0, 0.0), &spin, 0.0);
        assert!(lat.abs() < 1e-9 && lon.abs() < 1e-9, "{lat} {lon}");
        let (lat, _) = lat_lon(DVec3::new(0.0, 0.0, 1.0), &spin, 0.0);
        assert!((lat - 90.0).abs() < 1e-9);
    }

    #[test]
    fn the_committed_mun_terrain_loads() {
        let t = load_terrain("Mun", 200_000.0).unwrap().expect("Mun map");
        let h = t.ground_m(0.0, 0.0);
        assert!(h > -5_000.0 && h < 10_000.0, "{h}");
    }
}

//! Closed-loop ascent from the KSC pad to a low Kerbin orbit, flown through `SetControls` and `Stage`.
//!
//! The bot reacts to each `Vessels` snapshot (10 Hz) the way a careful player would: a gravity turn from
//! 90 degrees at `turn_start_m` to 0 at `turn_end_m` heading east, throttle back when dynamic pressure is high,
//! stage when the current stage is dry, cut the engines once apoapsis reaches the target, coast to apoapsis
//! and circularise prograde until periapsis clears the atmosphere. Warp is requested at the physics cap.

use crate::client::{orbit_from, Client};
use anyhow::{bail, Result};
use ose_net::{AttitudeMode, FlightSituation, ServerMessage, VesselState};
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct AscentParams {
    pub target_apoapsis_m: f64,
    pub target_periapsis_m: f64,
    pub turn_start_m: f64,
    pub turn_end_m: f64,
    pub heading_deg: f64,
    pub max_q_kpa: f64,
    /// Wall-clock limit for the whole ascent.
    pub timeout: Duration,
    /// Write a CSV trace of the flight here (one row about every two seconds of flight).
    pub trace: Option<std::path::PathBuf>,
}

impl Default for AscentParams {
    fn default() -> Self {
        AscentParams {
            target_apoapsis_m: 80_000.0,
            target_periapsis_m: 72_000.0,
            turn_start_m: 1_000.0,
            turn_end_m: 45_000.0,
            heading_deg: 90.0,
            max_q_kpa: 25.0,
            timeout: Duration::from_secs(600),
            trace: None,
        }
    }
}

/// What the ascent measured.
#[derive(Clone, Debug, PartialEq)]
pub struct AscentResult {
    pub apoapsis_alt_m: f64,
    pub periapsis_alt_m: f64,
    pub eccentricity: f64,
    pub game_time_s: f64,
    pub wall_time_s: f64,
    pub stages_fired: u32,
    pub stage: u32,
    pub stages: u32,
    pub propellant_t: f64,
    pub max_q_kpa: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Ascent,
    Coast,
    Circularise,
}

/// Pitch above the horizon for the gravity turn at `alt_m`.
pub fn pitch_program(p: &AscentParams, alt_m: f64) -> f64 {
    if alt_m < p.turn_start_m {
        90.0
    } else {
        let f = (alt_m - p.turn_start_m) / (p.turn_end_m - p.turn_start_m);
        // a square-root profile turns early and flattens out, which loses less to gravity than a line
        (90.0 * (1.0 - f.clamp(0.0, 1.0).sqrt())).clamp(0.0, 90.0)
    }
}

/// Fly `vessel` (on the pad, landed) to orbit around the body at catalog index `body`.
pub async fn fly_ascent(
    c: &mut Client,
    vessel: u64,
    body: u16,
    p: &AscentParams,
) -> Result<AscentResult> {
    let info = c
        .body(body)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no body {body} in the catalog"))?;
    let (mu, radius) = (info.mu, info.radius_m);
    let atmo_top = if info.has_atmosphere {
        info.atmosphere_depth_m
    } else {
        0.0
    };
    let start_wall = Instant::now();
    let start_ut = c.latest.ut();
    let mut phase = Phase::Ascent;
    let mut stages_fired = 0u32;
    let mut last_stage_ut = f64::NEG_INFINITY;
    let mut last_cmd: Option<(f64, AttitudeMode)> = None;
    let mut last_cmd_ut = f64::NEG_INFINITY;
    let mut full_accel = 10.0_f64; // m/s^2 at full throttle, refreshed while burning
    let mut max_q = 0.0_f64;
    let mut trace = match &p.trace {
        Some(path) => {
            let mut f = std::fs::File::create(path)?;
            writeln!(
                f,
                "t_s,phase,alt_m,vz_mps,speed_mps,pitch_cmd_deg,pitch_actual_deg,throttle,thrust_kn,twr,q_kpa,stage,prop_frac,ap_km,pe_km"
            )?;
            Some(f)
        }
        None => None,
    };
    let mut last_trace_ut = f64::NEG_INFINITY;
    c.request_warp(ose_sim::flight::MAX_PHYSICS_WARP).await?;

    loop {
        if start_wall.elapsed() > p.timeout {
            c.request_warp(1).await.ok();
            bail!(
                "ascent did not finish within {:?} (phase {phase:?})",
                p.timeout
            );
        }
        let msg = c.poll(Duration::from_secs(10)).await?;
        if let ServerMessage::FlightAck {
            vessel: v,
            accepted: false,
            reason,
        } = &msg
        {
            if *v == vessel {
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
        let Some(v): Option<VesselState> = c.latest.vessel(vessel).cloned() else {
            bail!("vessel {vessel} vanished during the ascent");
        };
        let orbit = orbit_from(mu, &v.orbit);
        let bound = orbit.eccentricity() < 1.0;
        let ap_alt = if bound {
            orbit.apoapsis() - radius
        } else {
            f64::INFINITY
        };
        let pe_alt = orbit.periapsis() - radius;
        let Some(f) = v.flight.clone() else {
            // back on rails: only acceptable once the orbit is done
            if phase == Phase::Circularise && pe_alt >= p.target_periapsis_m.min(atmo_top + 1_000.0)
            {
                break;
            }
            bail!(
                "vessel went on rails mid-ascent (phase {phase:?}, {:.0} x {:.0} km)",
                pe_alt / 1000.0,
                ap_alt / 1000.0
            );
        };
        if f.situation == FlightSituation::Crashed {
            bail!(
                "crashed at {:.0} m during {phase:?} (stage {}/{})",
                f.altitude_m,
                f.stage,
                f.stages
            );
        }
        if !f.has_control {
            bail!("lost control (no electric charge) at {:.0} m", f.altitude_m);
        }
        max_q = max_q.max(f.dynamic_pressure_kpa);
        let g = mu / (radius + f.altitude_m).powi(2);
        if f.throttle > 0.5 && f.twr > 0.0 {
            full_accel = f.twr * g;
        }

        // Phase transitions.
        match phase {
            Phase::Ascent if bound && ap_alt >= p.target_apoapsis_m => phase = Phase::Coast,
            Phase::Coast => {
                let t_ap = orbit.time_to_true_anomaly(std::f64::consts::PI, ut);
                let r_ap = orbit.apoapsis();
                let v_ap = (mu * (2.0 / r_ap - 1.0 / orbit.semi_major_axis())).sqrt();
                let dv = (mu / r_ap).sqrt() - v_ap;
                let lead = 0.5 * dv / full_accel.max(0.1) + 2.0;
                let past_ap = t_ap > orbit.period * 0.5;
                if (t_ap < lead || past_ap) && f.altitude_m > atmo_top {
                    phase = Phase::Circularise;
                }
            }
            Phase::Circularise if pe_alt >= p.target_periapsis_m => {
                c.set_controls(vessel, 0.0, AttitudeMode::OrbitPrograde)
                    .await?;
                c.request_warp(1).await?;
                return Ok(AscentResult {
                    apoapsis_alt_m: ap_alt,
                    periapsis_alt_m: pe_alt,
                    eccentricity: orbit.eccentricity(),
                    game_time_s: ut - start_ut,
                    wall_time_s: start_wall.elapsed().as_secs_f64(),
                    stages_fired,
                    stage: f.stage,
                    stages: f.stages,
                    propellant_t: f.propellant_t,
                    max_q_kpa: max_q,
                });
            }
            _ => {}
        }

        // Controls for this snapshot.
        let (throttle, attitude) = match phase {
            Phase::Ascent => {
                let pitch = pitch_program(p, f.altitude_m);
                let q_ratio = if f.dynamic_pressure_kpa > p.max_q_kpa {
                    (p.max_q_kpa / f.dynamic_pressure_kpa).clamp(0.4, 1.0)
                } else {
                    1.0
                };
                // ease off near the target so apoapsis is not overshot by much
                let ap_margin = ((p.target_apoapsis_m - ap_alt.min(p.target_apoapsis_m)) / 5_000.0)
                    .clamp(0.15, 1.0);
                (
                    q_ratio * if bound { ap_margin } else { 1.0 },
                    AttitudeMode::PitchHeading {
                        pitch_deg: pitch,
                        heading_deg: p.heading_deg,
                    },
                )
            }
            Phase::Coast => {
                // drag bleeds apoapsis inside the atmosphere: top it up
                let top_up = ap_alt < p.target_apoapsis_m - 1_000.0;
                (if top_up { 0.3 } else { 0.0 }, AttitudeMode::OrbitPrograde)
            }
            Phase::Circularise => {
                let need = (p.target_periapsis_m - pe_alt).max(0.0);
                (
                    (need / 10_000.0).clamp(0.2, 1.0),
                    AttitudeMode::OrbitPrograde,
                )
            }
        };

        if let Some(tf) = trace.as_mut() {
            if ut - last_trace_ut >= 2.0 {
                last_trace_ut = ut;
                let body_pos = c
                    .latest
                    .bodies
                    .get(body as usize)
                    .map(|b| b.position_m)
                    .unwrap_or(ose_math::DVec3::ZERO);
                let up = (v.position_m - body_pos).normalized();
                let pitch_actual = f.facing.dot(up).clamp(-1.0, 1.0).asin().to_degrees();
                let pitch_cmd = match attitude {
                    AttitudeMode::PitchHeading { pitch_deg, .. } => pitch_deg,
                    _ => f64::NAN,
                };
                writeln!(
                    tf,
                    "{:.1},{:?},{:.0},{:.1},{:.1},{:.1},{:.1},{:.2},{:.0},{:.2},{:.1},{},{:.3},{:.1},{:.1}",
                    ut - start_ut,
                    phase,
                    f.altitude_m,
                    f.vertical_speed_mps,
                    f.surface_speed_mps,
                    pitch_cmd,
                    pitch_actual,
                    f.throttle,
                    f.thrust_kn,
                    f.twr,
                    f.dynamic_pressure_kpa,
                    f.stage,
                    f.propellant_fraction,
                    ap_alt / 1000.0,
                    pe_alt / 1000.0
                )?;
            }
        }

        // Staging: the current stage is dry, or the engines give nothing while we ask for thrust.
        let dry = f.propellant_fraction <= 1e-6;
        let dead = f.throttle > 0.05 && f.thrust_kn <= 1e-6 && ut - last_cmd_ut > 1.0;
        if throttle > 0.0 && (dry || dead) && f.stage + 1 < f.stages && ut - last_stage_ut > 1.0 {
            c.stage(vessel).await?;
            stages_fired += 1;
            last_stage_ut = ut;
        } else if throttle > 0.0 && dry && f.stage + 1 >= f.stages {
            bail!(
                "out of propellant in the last stage during {phase:?} ({:.0} x {:.0} km)",
                pe_alt / 1000.0,
                ap_alt / 1000.0
            );
        }

        let changed = match last_cmd {
            None => true,
            Some((t, a)) => (t - throttle).abs() > 0.02 || !same_attitude(a, attitude),
        };
        if changed {
            c.set_controls(vessel, throttle, attitude).await?;
            last_cmd = Some((throttle, attitude));
            last_cmd_ut = ut;
        }
    }
    let v = c.vessel_state(vessel, Duration::from_secs(10)).await?;
    let orbit = orbit_from(mu, &v.orbit);
    c.request_warp(1).await?;
    Ok(AscentResult {
        apoapsis_alt_m: orbit.apoapsis() - radius,
        periapsis_alt_m: orbit.periapsis() - radius,
        eccentricity: orbit.eccentricity(),
        game_time_s: c.latest.ut() - start_ut,
        wall_time_s: start_wall.elapsed().as_secs_f64(),
        stages_fired,
        stage: 0,
        stages: 0,
        propellant_t: 0.0,
        max_q_kpa: max_q,
    })
}

/// Coast with the engines off (warp at the physics cap) until the server puts `vessel` back on rails, which
/// it does once the orbit clears the atmosphere and terrain and the craft has coasted for ten seconds.
pub async fn wait_for_rails(c: &mut Client, vessel: u64, timeout: Duration) -> Result<()> {
    c.request_warp(ose_sim::flight::MAX_PHYSICS_WARP).await?;
    let r = c
        .wait_for(timeout, "the craft to go back on rails", |l, m| match m {
            ServerMessage::Vessels { .. } => {
                l.vessel(vessel).filter(|v| v.flight.is_none()).map(|_| ())
            }
            _ => None,
        })
        .await;
    c.request_warp(1).await?;
    r
}

fn same_attitude(a: AttitudeMode, b: AttitudeMode) -> bool {
    match (a, b) {
        (
            AttitudeMode::PitchHeading {
                pitch_deg: p1,
                heading_deg: h1,
            },
            AttitudeMode::PitchHeading {
                pitch_deg: p2,
                heading_deg: h2,
            },
        ) => (p1 - p2).abs() < 0.5 && (h1 - h2).abs() < 0.5,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_program_goes_from_vertical_to_horizontal() {
        let p = AscentParams::default();
        assert_eq!(pitch_program(&p, 0.0), 90.0);
        assert_eq!(pitch_program(&p, 999.0), 90.0);
        assert!(pitch_program(&p, 10_000.0) < 60.0);
        assert_eq!(pitch_program(&p, 45_000.0), 0.0);
        assert_eq!(pitch_program(&p, 60_000.0), 0.0);
        let mut last = 90.0;
        for h in (1_000..45_000).step_by(1_000) {
            let x = pitch_program(&p, h as f64);
            assert!(x <= last);
            last = x;
        }
    }
}

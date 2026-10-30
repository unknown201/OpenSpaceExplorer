//! Flying the Mun run through the server: plan from the streamed state, schedule burns with
//! `ScheduleBurn`, read the server's patched-conic prediction back, warp, and watch `Vessels`.

use crate::client::{orbit_from, Client};
use crate::plan::{in_plane_angle, plan_hohmann, TransferPlan};
use anyhow::{anyhow, bail, Context, Result};
use ose_net::{PatchEnd, ServerMessage, TrajectoryPatch, VesselState};
use std::time::Duration;

/// Warp the bot asks for while coasting. The server clamps it so burns and SOI crossings land on tick.
pub const COAST_WARP: u32 = 10_000;

/// Universal time that can pass between two `Vessels` messages while coasting at [`COAST_WARP`]: the sim
/// steps `TICKS_PER_SECOND` times a second, each step covering `warp` ticks, and the server snapshots at
/// `SNAPSHOT_HZ`. One extra step covers the handoff landing on a step boundary.
pub fn snapshot_lag_s() -> f64 {
    let steps_per_snapshot =
        (ose_sim::TICKS_PER_SECOND as f64 / ose_net::SNAPSHOT_HZ as f64).ceil();
    (steps_per_snapshot + 1.0) * COAST_WARP as f64 / ose_sim::TICKS_PER_SECOND as f64
}

/// Catalog indices and constants the run needs.
#[derive(Clone, Copy, Debug)]
pub struct Bodies {
    pub kerbin: u16,
    pub mun: u16,
    pub mu_kerbin: f64,
    pub mu_mun: f64,
    pub mun_radius_m: f64,
    pub mun_soi_m: f64,
}

pub fn bodies(c: &Client) -> Result<Bodies> {
    let kerbin = c.body_index("Kerbin").context("catalog has no Kerbin")?;
    let mun = c.body_index("Mun").context("catalog has no Mun")?;
    let mun_info = c.body(mun).expect("mun info");
    Ok(Bodies {
        kerbin,
        mun,
        mu_kerbin: c.body(kerbin).expect("kerbin info").mu,
        mu_mun: mun_info.mu,
        mun_radius_m: mun_info.radius_m,
        mun_soi_m: mun_info.soi_radius_m.unwrap_or(f64::INFINITY),
    })
}

/// Plan the transfer for `vessel` (around Kerbin) from the server's own orbit elements, with the burn no
/// earlier than `margin_s` from the server's current time.
pub fn plan_from_server(
    c: &Client,
    b: &Bodies,
    vessel: &VesselState,
    margin_s: f64,
) -> Result<TransferPlan> {
    if vessel.reference_body != b.kerbin {
        bail!(
            "vessel {} is around catalog body {} not Kerbin",
            vessel.id,
            vessel.reference_body
        );
    }
    let park = orbit_from(b.mu_kerbin, &vessel.orbit);
    if park.eccentricity() > 0.05 {
        bail!(
            "parking orbit is not circular (e = {})",
            park.eccentricity()
        );
    }
    let mun_el = c
        .body(b.mun)
        .and_then(|m| m.orbit)
        .context("Mun has no orbit in the catalog")?;
    let mun = orbit_from(b.mu_kerbin, &mun_el);
    let ut0 = c.latest.ut() + margin_s;
    let (mp, mv) = mun.state_at(ut0);
    let (vp, _) = park.state_at(ut0);
    let normal = mp.cross(mv).normalized();
    let phase = -in_plane_angle(mp.normalized(), normal, vp.normalized());
    Ok(plan_hohmann(
        b.mu_kerbin,
        park.semi_major_axis(),
        mun.semi_major_axis(),
        phase,
        ut0,
    ))
}

/// The first predicted patch around the Mun: (periapsis altitude above the surface, patch start UT, patch).
pub fn mun_patch<'a>(
    patches: &'a [TrajectoryPatch],
    b: &Bodies,
) -> Option<(f64, f64, &'a TrajectoryPatch)> {
    patches.iter().find(|p| p.body == b.mun).map(|p| {
        let rp = p.orbit.semi_major_axis_m * (1.0 - p.orbit.eccentricity);
        (rp - b.mun_radius_m, p.start_ut, p)
    })
}

/// Result of tuning the transfer burn against the server's prediction.
#[derive(Clone, Debug, PartialEq)]
pub struct TunedBurn {
    pub dv_mps: f64,
    pub burn_ut: f64,
    pub predicted_periapsis_alt_m: f64,
    pub predicted_encounter_ut: f64,
    pub attempts: u32,
    pub patch_ends: Vec<PatchEnd>,
}

type Sample = (Option<(f64, f64)>, Vec<TrajectoryPatch>);

/// Schedule a prograde burn of `dv` at `burn_ut` and read back the predicted Mun periapsis altitude and
/// encounter time, if the server predicts an encounter.
async fn eval(
    c: &mut Client,
    b: &Bodies,
    vessel: u64,
    burn_ut: f64,
    dv: f64,
    attempts: &mut u32,
) -> Result<Sample> {
    *attempts += 1;
    let patches = c.schedule_burn(vessel, burn_ut, dv, 0.0, 0.0).await?;
    Ok((mun_patch(&patches, b).map(|(a, t, _)| (a, t)), patches))
}

/// Find a transfer burn the server predicts will pass the Mun with a periapsis between `alt_min_m` and
/// `alt_max_m` above the surface. An exact Hohmann aims at the Mun's centre, and from a slightly elliptical
/// parking orbit it can miss either way, so the bot searches like a player dragging a maneuver node: delta-v
/// from 15 m/s below to 25 m/s above the Hohmann value in 0.5 m/s steps, at the planned burn time and then
/// at burn times shifted by up to a minute either way. Each sample is one `ScheduleBurn` and the prediction
/// the server sends back. Among in-band samples it keeps the one closest to the middle of the band, then
/// schedules that burn.
pub async fn tune_transfer_burn(
    c: &mut Client,
    b: &Bodies,
    vessel: u64,
    plan: &TransferPlan,
    alt_min_m: f64,
    alt_max_m: f64,
) -> Result<TunedBurn> {
    let mut attempts = 0u32;
    let mid = 0.5 * (alt_min_m + alt_max_m);
    let mut best: Option<(f64, f64, f64, f64)> = None; // (score, dv, burn_ut, alt)
    let mut closest: Option<(f64, f64, f64)> = None; // (|alt - mid|, dv, alt) over every encounter
    let now = c.latest.ut();
    for shift in [0.0, -20.0, 20.0, -40.0, 40.0, -60.0, 60.0] {
        let burn_ut = plan.burn_ut + shift;
        if burn_ut < now + 10.0 {
            continue;
        }
        let mut k = -30;
        while k <= 50 {
            let dv = plan.dv_mps + k as f64 * 0.5;
            k += 1;
            let (hit, _) = eval(c, b, vessel, burn_ut, dv, &mut attempts).await?;
            let Some((alt, _)) = hit else { continue };
            let score = (alt - mid).abs();
            if closest.is_none_or(|(s, _, _)| score < s) {
                closest = Some((score, dv, alt));
            }
            if alt >= alt_min_m && alt <= alt_max_m && best.is_none_or(|(s, ..)| score < s) {
                best = Some((score, dv, burn_ut, alt));
            }
        }
        if best.is_some() {
            break;
        }
    }
    let Some((_, dv, burn_ut, _)) = best else {
        return Err(match closest {
            Some((_, dv, alt)) => anyhow!(
                "no burn within -15..+25 m/s of Hohmann and a minute of the window gives a Mun periapsis in \
                 [{:.0}, {:.0}] km; closest {:.0} km at {dv:.2} m/s",
                alt_min_m / 1000.0,
                alt_max_m / 1000.0,
                alt / 1000.0
            ),
            None => anyhow!(
                "server predicts no Mun encounter for any burn within -15..+25 m/s of Hohmann ({:.1} m/s)",
                plan.dv_mps
            ),
        });
    };
    // schedule the chosen burn last so it is the one the server keeps
    let (hit, patches) = eval(c, b, vessel, burn_ut, dv, &mut attempts).await?;
    let (alt, enc) =
        hit.ok_or_else(|| anyhow!("the chosen burn lost its encounter on re-check"))?;
    Ok(TunedBurn {
        dv_mps: dv,
        burn_ut,
        predicted_periapsis_alt_m: alt,
        predicted_encounter_ut: enc,
        attempts,
        patch_ends: patches.iter().map(|p| p.end).collect(),
    })
}

/// Warp until the vessel's reference body is `body`, then drop back to real time. Returns the vessel state
/// at the first snapshot that showed it there.
pub async fn warp_until_around(
    c: &mut Client,
    vessel: u64,
    body: u16,
    timeout: Duration,
) -> Result<VesselState> {
    c.request_warp(COAST_WARP).await?;
    let result = c
        .wait_for(timeout, "the vessel to change reference body", |l, _| {
            l.vessel(vessel)
                .filter(|v| v.reference_body == body)
                .cloned()
        })
        .await;
    c.request_warp(1).await?;
    result
}

/// Warp until the vessel has flown its planned burn at `burn_ut` (the server flies nodes with the engines,
/// at physics warp, from half the burn time before): its time is past the node and its trajectory has no
/// maneuver left. Then drop back to real time.
pub async fn warp_until_flown(
    c: &mut Client,
    vessel: u64,
    burn_ut: f64,
    timeout: Duration,
) -> Result<()> {
    c.request_warp(COAST_WARP).await?;
    let result = c
        .wait_for(timeout, "the planned burn to be flown", |l, m| match m {
            ServerMessage::Trajectory {
                vessel: v, patches, ..
            } if *v == vessel
                && l.ut() > burn_ut
                && !patches.iter().any(|p| p.end == PatchEnd::Maneuver)
                && l.vessel(vessel)
                    .and_then(|v| v.flight.as_ref())
                    .is_none_or(|f| f.burn_left_mps.is_none()) =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
    c.request_warp(1).await?;
    result
}

/// After the transfer burn is flown (its finite burn lands a little off the impulsive plan), aim the Mun
/// periapsis back into `[alt_min_m, alt_max_m]` with a small prograde correction twenty minutes on, as a
/// player would: sample -20..+20 m/s in 0.5 m/s steps against the server's prediction and keep the one
/// nearest the middle of the band (none when the trajectory already is in it).
pub async fn correct_course(
    c: &mut Client,
    b: &Bodies,
    vessel: u64,
    alt_min_m: f64,
    alt_max_m: f64,
) -> Result<TunedBurn> {
    let mid = 0.5 * (alt_min_m + alt_max_m);
    // far enough on that the search (one round trip a sample) is done well before it
    let burn_ut = c.latest.ut() + 1_200.0;
    let mut attempts = 0u32;
    let mut best: Option<(f64, f64, f64, f64)> = None; // (|alt - mid|, dv, alt, encounter)
    for k in -40..=40 {
        let dv = k as f64 * 0.5;
        let (hit, _) = eval(c, b, vessel, burn_ut, dv, &mut attempts).await?;
        let Some((alt, enc)) = hit else { continue };
        let score = (alt - mid).abs();
        if best.is_none_or(|(s, ..)| score < s) {
            best = Some((score, dv, alt, enc));
        }
    }
    let Some((_, dv, alt, _)) = best else {
        bail!("no correction within 20 m/s keeps a Mun encounter");
    };
    if alt < alt_min_m || alt > alt_max_m {
        bail!(
            "the best correction ({dv:.2} m/s) gives a Mun periapsis of {:.0} km, outside [{:.0}, {:.0}] km",
            alt / 1000.0,
            alt_min_m / 1000.0,
            alt_max_m / 1000.0
        );
    }
    // schedule it last so it is the one the server keeps
    let (hit, patches) = eval(c, b, vessel, burn_ut, dv, &mut attempts).await?;
    let (alt, enc) = hit.ok_or_else(|| anyhow!("the correction lost its encounter on re-check"))?;
    Ok(TunedBurn {
        dv_mps: dv,
        burn_ut,
        predicted_periapsis_alt_m: alt,
        predicted_encounter_ut: enc,
        attempts,
        patch_ends: patches.iter().map(|p| p.end).collect(),
    })
}

/// Warp until the server's time passes `ut`, then drop back to real time.
pub async fn warp_past(c: &mut Client, ut: f64, timeout: Duration) -> Result<()> {
    c.request_warp(COAST_WARP).await?;
    let result = c
        .wait_for(timeout, "time to pass the burn", |l, _| {
            (l.ut() > ut).then_some(())
        })
        .await;
    c.request_warp(1).await?;
    result
}

/// A capture burn: retrograde at the next Mun periapsis to circularise there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapturePlan {
    pub burn_ut: f64,
    pub dv_mps: f64,
    pub periapsis_alt_m: f64,
    pub arrival_speed_mps: f64,
}

pub fn plan_capture(b: &Bodies, vessel: &VesselState, now_ut: f64) -> Result<CapturePlan> {
    if vessel.reference_body != b.mun {
        bail!("vessel is not around the Mun");
    }
    let o = orbit_from(b.mu_mun, &vessel.orbit);
    let rp = o.periapsis();
    if rp <= b.mun_radius_m {
        bail!(
            "Mun periapsis {:.0} km is below the surface",
            (rp - b.mun_radius_m) / 1000.0
        );
    }
    let dt = o.time_to_periapsis(now_ut);
    if dt <= 0.0 {
        bail!("periapsis is {dt:.0} s in the past");
    }
    let v_peri = (b.mu_mun * (2.0 / rp - 1.0 / o.semi_major_axis())).sqrt();
    let v_circ = (b.mu_mun / rp).sqrt();
    Ok(CapturePlan {
        burn_ut: now_ut + dt,
        dv_mps: v_circ - v_peri,
        periapsis_alt_m: rp - b.mun_radius_m,
        arrival_speed_mps: v_peri,
    })
}

/// Periapsis and apoapsis altitude and eccentricity of a vessel's current orbit around `mu`/`radius`.
pub fn orbit_summary(mu: f64, radius_m: f64, v: &VesselState) -> (f64, f64, f64) {
    let o = orbit_from(mu, &v.orbit);
    (
        o.periapsis() - radius_m,
        o.apoapsis() - radius_m,
        o.eccentricity(),
    )
}

/// The patch that starts at the capture burn, if the server predicts the vessel stays bound to the Mun.
pub fn capture_predicted<'a>(
    patches: &'a [TrajectoryPatch],
    b: &Bodies,
    burn_ut: f64,
) -> Result<&'a TrajectoryPatch> {
    patches
        .iter()
        .find(|p| p.body == b.mun && (p.start_ut - burn_ut).abs() < 1.0)
        .ok_or_else(|| {
            anyhow!(
                "no post-burn patch around the Mun at UT {burn_ut:.0}; patches: {:?}",
                patches
                    .iter()
                    .map(|p| (p.body, p.start_ut.round(), p.end))
                    .collect::<Vec<_>>()
            )
        })
        .and_then(|p| {
            if p.orbit.eccentricity < 1.0 && p.end != PatchEnd::Escape {
                Ok(p)
            } else {
                Err(anyhow!(
                    "post-burn orbit is not bound (e = {:.3}, ends {:?})",
                    p.orbit.eccentricity,
                    p.end
                ))
            }
        })
}

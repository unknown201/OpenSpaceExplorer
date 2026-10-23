//! Flying the Mun run through the server: plan from the streamed state, schedule burns with
//! `ScheduleBurn`, read the server's patched-conic prediction back, warp, and watch `Vessels`.

use crate::client::{orbit_from, Client};
use crate::plan::{in_plane_angle, plan_hohmann, TransferPlan};
use anyhow::{anyhow, bail, Context, Result};
use ose_net::{PatchEnd, TrajectoryPatch, VesselState};
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
    if park.eccentricity() > 0.01 {
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

/// Schedule the Hohmann burn, then nudge its delta-v upward in small steps until the server predicts a Mun
/// periapsis between `alt_min_m` and `alt_max_m` above the surface (an exact Hohmann aims at the centre).
pub async fn tune_transfer_burn(
    c: &mut Client,
    b: &Bodies,
    vessel: u64,
    plan: &TransferPlan,
    alt_min_m: f64,
    alt_max_m: f64,
) -> Result<TunedBurn> {
    const STEP_MPS: f64 = 0.5;
    const MAX_STEPS: u32 = 40;
    let mut attempts = 0u32;
    let (first, patches) = eval(c, b, vessel, plan.burn_ut, plan.dv_mps, &mut attempts).await?;
    let Some((alt0, _)) = first else {
        bail!(
            "server predicts no Mun encounter for the Hohmann burn ({:.1} m/s); patches end {:?}",
            plan.dv_mps,
            patches.iter().map(|p| p.end).collect::<Vec<_>>()
        );
    };
    let mut lo = (plan.dv_mps, alt0);
    let mut chosen = None;
    for k in 1..=MAX_STEPS {
        let dv = plan.dv_mps + k as f64 * STEP_MPS;
        let (hit, patches) = eval(c, b, vessel, plan.burn_ut, dv, &mut attempts).await?;
        let Some((alt, enc)) = hit else {
            bail!("lost the Mun encounter at {dv:.2} m/s");
        };
        if alt >= alt_min_m && alt <= alt_max_m {
            chosen = Some((dv, alt, enc, patches));
            break;
        }
        if alt > alt_max_m {
            // overshot: bisect between the last low sample and this one
            let (mut a, mut bb) = (lo.0, dv);
            let mut last = (dv, alt, enc, patches);
            for _ in 0..8 {
                let mid = 0.5 * (a + bb);
                let (hit, patches) = eval(c, b, vessel, plan.burn_ut, mid, &mut attempts).await?;
                let Some((alt, enc)) = hit else {
                    bail!("lost the Mun encounter at {mid:.2} m/s");
                };
                last = (mid, alt, enc, patches);
                if alt >= alt_min_m && alt <= alt_max_m {
                    break;
                }
                if alt > alt_max_m {
                    bb = mid;
                } else {
                    a = mid;
                }
            }
            chosen = Some(last);
            break;
        }
        lo = (dv, alt);
    }
    let Some((dv, alt, enc, patches)) = chosen else {
        bail!(
            "could not raise the Mun periapsis above {:.0} km within {} m/s of the Hohmann burn (last {:.0} km at {:.2} m/s)",
            alt_min_m / 1000.0,
            MAX_STEPS as f64 * STEP_MPS,
            lo.1 / 1000.0,
            lo.0
        );
    };
    if alt < alt_min_m || alt > alt_max_m {
        bail!(
            "tuned burn predicts a Mun periapsis of {:.0} km, outside [{:.0}, {:.0}] km",
            alt / 1000.0,
            alt_min_m / 1000.0,
            alt_max_m / 1000.0
        );
    }
    Ok(TunedBurn {
        dv_mps: dv,
        burn_ut: plan.burn_ut,
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

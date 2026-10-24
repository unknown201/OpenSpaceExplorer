//! The Mun campaign as an ordered chain of steps.
//!
//! Each step is what a player does next. A step whose game feature does not exist yet returns
//! `Outcome::Blocked` naming the feature; the chain keeps going so later steps that can run still report.
//! Steps that the server supports run through it (`Mode::Server`); the orbital-mechanics steps fall back to
//! the bot's in-process rails model (`Mode::Local`) when no server is given. When a feature lands, replace
//! the `Blocked` body with the real protocol exchange and update the frontier in `tests/mun_run.rs`.

use crate::ascent::{self, AscentParams};
use crate::client::{Client, ConnectError};
use crate::data::{self, BodySource};
use crate::landing;
use crate::plan::TransferPlan;
use crate::rails::{fly_to_mun_encounter, park_in_kerbin_orbit, plan_mun_transfer, RailsWorld};
use crate::report::{Mode, Outcome, StepResult};
use crate::server_flight::{self, Bodies, TunedBurn};
use ose_net::SessionMode;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The step names in order. Tests key their expectations on these.
pub const STEP_NAMES: [&str; 11] = [
    "server_health",
    "connect_hello",
    "start_campaign",
    "load_stock_craft",
    "upload_and_launch_craft",
    "ascend_to_orbit",
    "plan_mun_transfer",
    "execute_transfer_burn",
    "coast_to_mun_encounter",
    "mun_capture_burn",
    "mun_landing",
];

/// Wall-clock allowance for the powered descent.
const LANDING_TIMEOUT: Duration = Duration::from_secs(600);
/// Name the bot gives the campaign it sets up on a fresh server.
const SESSION_NAME: &str = "ose-bot Mun run";
/// Seconds of universal time between planning and the earliest allowed burn.
const BURN_MARGIN_S: f64 = 30.0;
/// Target Mun periapsis band for the tuned transfer burn, m above the surface.
const MUN_PERIAPSIS_MIN_M: f64 = 30_000.0;
const MUN_PERIAPSIS_MAX_M: f64 = 150_000.0;
/// Wall-clock allowance for a warped coast.
const COAST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct BotConfig {
    /// `host:port` of a running `ose-server`; `None` skips the server steps.
    pub server: Option<String>,
    pub game_data: PathBuf,
    pub stock_dir: PathBuf,
    /// Stock craft to cross-check in the install's assemblies, by file stem, in order of preference.
    pub craft_preference: Vec<String>,
    /// Stock craft to fly from the server's library, by name, in order of preference.
    pub server_craft: Vec<String>,
    /// Parking orbit altitude above Kerbin for the local model, m (the server picks its own).
    pub parking_altitude_m: f64,
    pub display_name: String,
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            server: None,
            game_data: data::default_game_data_path(),
            stock_dir: data::default_stock_dir(),
            craft_preference: vec!["Stock Kerbal-K2".into(), "Stock Kerbal-K1".into()],
            server_craft: vec!["Kerbal K2".into()],
            parking_altitude_m: 100_000.0,
            display_name: "ose-bot".into(),
        }
    }
}

/// Mutable state carried from step to step.
#[derive(Default)]
struct Flight {
    client: Option<Client>,
    bodies: Option<Bodies>,
    vessel_id: Option<u64>,
    plan: Option<TransferPlan>,
    tuned: Option<TunedBurn>,
    rails: Option<RailsWorld>,
    /// Library id of the craft being flown.
    craft_id: Option<u64>,
    /// The craft stands on the pad (the ascent step flies it from there).
    on_pad: bool,
    /// The campaign is a Sandbox (no tech gating).
    sandbox: bool,
    /// The craft is in orbit around the Mun.
    captured: bool,
}

async fn timed<F>(name: &'static str, mode: Mode, f: F) -> StepResult
where
    F: std::future::Future<Output = Outcome>,
{
    let start = Instant::now();
    let outcome = f.await;
    StepResult {
        name,
        mode,
        outcome,
        elapsed: start.elapsed(),
    }
}

/// Run the whole chain and return one result per step, in `STEP_NAMES` order.
pub async fn run_mun_campaign(cfg: &BotConfig) -> Vec<StepResult> {
    let mut results = Vec::with_capacity(STEP_NAMES.len());
    let mut flight = Flight::default();
    let orbital_mode = if cfg.server.is_some() {
        Mode::Server
    } else {
        Mode::Local
    };

    // 1. The server is up and ticking.
    results.push(
        timed("server_health", Mode::Server, async {
            let Some(base) = &cfg.server else {
                return Outcome::skipped("no --server given");
            };
            match crate::client::health(base).await {
                Ok(h) => Outcome::passed(format!("tick {}", h.tick.0)),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 2. Open a protocol session: Hello -> Welcome -> BodyCatalog, and find the probe the server gives us.
    results.push(
        timed("connect_hello", Mode::Server, async {
            let Some(base) = &cfg.server else {
                return Outcome::skipped("no --server given");
            };
            let mut c = match Client::connect(base, &cfg.display_name).await {
                Ok(c) => c,
                Err(ConnectError::NoEndpoint { status }) => {
                    return Outcome::blocked(format!(
                        "server WebSocket endpoint at {} (upgrade answered HTTP {status}); Phase 3",
                        crate::client::WS_PATH
                    ))
                }
                Err(e) => return Outcome::failed(e),
            };
            let vessel = match c.my_vessel(Duration::from_secs(10)).await {
                Ok(v) => v,
                Err(e) => return Outcome::failed(e),
            };
            let bodies = match server_flight::bodies(&c) {
                Ok(b) => b,
                Err(e) => return Outcome::failed(e),
            };
            let detail = format!(
                "player {} welcomed at tick {}; catalog of {} bodies; own vessel {} ({:?}) around {}",
                c.player_id,
                c.latest.tick.0,
                c.catalog.len(),
                vessel.id,
                vessel.name,
                c.body(vessel.reference_body)
                    .map(|b| b.id.as_str())
                    .unwrap_or("?")
            );
            flight.vessel_id = Some(vessel.id);
            flight.bodies = Some(bodies);
            flight.client = Some(c);
            Outcome::passed(detail)
        })
        .await,
    );

    // 3. Start the campaign: a Sandbox session (the run flies the stock K2, whose parts an Exploration
    // campaign would have to research first), then join it.
    results.push(
        timed("start_campaign", Mode::Server, async {
            let Some(c) = flight.client.as_mut() else {
                return Outcome::skipped("no server session");
            };
            let created = match c.create_session(SESSION_NAME, SessionMode::Sandbox).await {
                Ok(s) => s.created,
                Err(e) => return Outcome::failed(e),
            };
            match c.join_session().await {
                Ok(s) => {
                    flight.sandbox = s.mode == SessionMode::Sandbox;
                    Outcome::passed(format!(
                        "{} {:?} as a {:?} campaign and joined it ({} member{})",
                        if created {
                            "CreateSession set up"
                        } else {
                            "joined the existing"
                        },
                        s.name,
                        s.mode,
                        s.members,
                        if s.members == 1 { "" } else { "s" }
                    ))
                }
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 4. Read the stock craft from the install's assemblies (the same file the server's library is built
    // from), as a cross-check on this machine.
    results.push(
        timed("load_stock_craft", Mode::Local, async {
            let prefs: Vec<&str> = cfg.craft_preference.iter().map(String::as_str).collect();
            match data::find_stock_craft(&cfg.stock_dir, &prefs) {
                Ok(Some(c)) => Outcome::passed(format!(
                    "{} ({} parts, {} stages, {:.2} t) from {}",
                    c.name,
                    c.part_names.len(),
                    c.stages,
                    c.mass_t,
                    c.file.file_name().unwrap_or_default().to_string_lossy()
                )),
                Ok(None) => Outcome::skipped(format!(
                    "no stock assemblies at {} (run tools/extract_bundles.py)",
                    cfg.stock_dir.display()
                )),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 5. Pick the stock craft from the server's library and roll it out to the launch pad.
    results.push(
        timed("upload_and_launch_craft", Mode::Server, async {
            let Some(c) = flight.client.as_mut() else {
                return Outcome::skipped("no server session");
            };
            let list = match c.craft_list().await {
                Ok(l) => l,
                Err(e) => return Outcome::failed(e),
            };
            let Some(craft) = cfg
                .server_craft
                .iter()
                .find_map(|want| list.iter().find(|s| s.stock && s.name == *want))
            else {
                return Outcome::failed(format!(
                    "none of {:?} in the server's stock library ({:?})",
                    cfg.server_craft,
                    list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
                ));
            };
            flight.craft_id = Some(craft.id);
            match c.launch_saved_on_pad(craft.id).await {
                Ok(l) => {
                    let pad = c.vessel_state(l.vessel, Duration::from_secs(10)).await;
                    let where_ = match pad.as_ref().ok().and_then(|v| v.flight.clone()) {
                        Some(f) => format!(
                            "{:?} at {:.0} m, TWR {:.2}",
                            f.situation, f.altitude_m, f.twr
                        ),
                        None => "no flight readout".into(),
                    };
                    flight.vessel_id = Some(l.vessel);
                    flight.on_pad = true;
                    Outcome::passed(format!(
                        "LaunchSavedOnPad {:?}: vessel {} with {} parts, {:.2} t wet / {:.2} t dry, {:.0} m/s \
                         vacuum delta-v; on the pad: {where_}",
                        l.name,
                        l.vessel,
                        l.parts,
                        l.wet_mass_t,
                        l.dry_mass_t,
                        l.delta_v_vacuum_mps
                    ))
                }
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 6. Fly the ascent under thrust: gravity turn, staging, coast, circularise. If it fails, put the craft
    // straight into orbit so the rest of the run still reports.
    results.push(
        timed("ascend_to_orbit", Mode::Server, async {
            let (Some(c), Some(b), Some(id)) =
                (flight.client.as_mut(), &flight.bodies, flight.vessel_id)
            else {
                return Outcome::skipped("no server session");
            };
            if !flight.on_pad {
                return Outcome::skipped("no craft on the pad");
            }
            let params = AscentParams {
                target_apoapsis_m: cfg.parking_altitude_m.max(75_000.0),
                ..AscentParams::default()
            };
            // Stage 0 must not be able to reach later stages' tanks: the staging readout assumes the
            // decouplers block crossfeed, and the ascent profile is planned stage by stage.
            let pad = match c.vessel_state(id, Duration::from_secs(10)).await {
                Ok(v) => v,
                Err(e) => return Outcome::failed(e),
            };
            let reach = pad
                .flight
                .as_ref()
                .map(|f| f.propellant_fraction)
                .unwrap_or(1.0);
            let outcome = if reach > 1.02 {
                Outcome::blocked(format!(
                    "resource flow: stage 0 engines on the pad reach {:.0}% of their own stage's propellant, \
                     so they drain the upper stages' tanks through the decouplers and the K2 cannot make \
                     orbit (the server's staging readout assumes no crossfeed)",
                    reach * 100.0
                ))
            } else {
                match ascent::fly_ascent(c, id, b.kerbin, &params).await {
                Ok(r) => {
                    // let the server put the craft back on rails before any burn is scheduled
                    let rails = ascent::wait_for_rails(c, id, Duration::from_secs(90)).await;
                    match rails {
                        Ok(()) => Outcome::passed(format!(
                            "pad to a {:.1} x {:.1} km orbit (e {:.4}) in {:.0} s of flight ({:.0} s wall), \
                             {} stage{} fired, now stage {}/{} with {:.1} t propellant; max Q {:.1} kPa",
                            r.periapsis_alt_m / 1000.0,
                            r.apoapsis_alt_m / 1000.0,
                            r.eccentricity,
                            r.game_time_s,
                            r.wall_time_s,
                            r.stages_fired,
                            if r.stages_fired == 1 { "" } else { "s" },
                            r.stage + 1,
                            r.stages,
                            r.propellant_t,
                            r.max_q_kpa
                        )),
                        Err(e) => Outcome::failed(e.context(format!(
                            "reached a {:.1} x {:.1} km orbit but",
                            r.periapsis_alt_m / 1000.0,
                            r.apoapsis_alt_m / 1000.0
                        ))),
                    }
                }
                Err(e) => Outcome::failed(e),
            }
            };
            if !outcome.is_passed() {
                // fallback: same craft straight to orbit, so planning and the transfer still run; a craft
                // left on the pad holds warp at the physics cap, so recover it first
                c.request_warp(1).await.ok();
                let landed = c
                    .vessel_state(id, Duration::from_secs(10))
                    .await
                    .ok()
                    .and_then(|v| v.flight)
                    .is_some_and(|f| f.situation == ose_net::FlightSituation::Landed);
                if landed {
                    c.recover_vessel(id).await.ok();
                }
                if let Some(craft) = flight.craft_id {
                    if let Ok(l) = c.launch_saved_to_orbit(craft, cfg.parking_altitude_m).await {
                        flight.vessel_id = Some(l.vessel);
                    }
                }
            }
            outcome
        })
        .await,
    );

    // 7. Plan the transfer window and burn from the parking orbit.
    results.push(
        timed("plan_mun_transfer", orbital_mode, async {
            if let (Some(c), Some(b), Some(id)) =
                (flight.client.as_mut(), &flight.bodies, flight.vessel_id)
            {
                let vessel = match c.vessel_state(id, Duration::from_secs(10)).await {
                    Ok(v) => v,
                    Err(e) => return Outcome::failed(e),
                };
                let plan = match server_flight::plan_from_server(c, b, &vessel, BURN_MARGIN_S) {
                    Ok(p) => p,
                    Err(e) => return Outcome::failed(e),
                };
                let alt = vessel.orbit.semi_major_axis_m
                    - c.body(b.kerbin).map(|k| k.radius_m).unwrap_or(0.0);
                flight.plan = Some(plan);
                return Outcome::passed(format!(
                    "from the server's {:.0} km orbit at UT {:.0}: burn at UT {:.0} ({:.0} s wait), \
                     {:.1} m/s prograde, coast {:.1} h, lead angle {:.1} deg",
                    alt / 1000.0,
                    c.latest.ut(),
                    plan.burn_ut,
                    plan.wait_s,
                    plan.dv_mps,
                    plan.transfer_time_s / 3600.0,
                    plan.lead_angle_rad.to_degrees()
                ));
            }
            let (bodies, source) = match data::load_bodies(&cfg.game_data) {
                Ok(x) => x,
                Err(e) => return Outcome::failed(e),
            };
            let mut rw = match RailsWorld::new(&bodies) {
                Ok(rw) => rw,
                Err(e) => return Outcome::failed(e),
            };
            let park = park_in_kerbin_orbit(&mut rw, "bot", cfg.parking_altitude_m, 0.0, 0.0);
            let plan = match plan_mun_transfer(&rw, park.vessel, 0.0) {
                Ok(p) => p,
                Err(e) => return Outcome::failed(e),
            };
            flight.rails = Some(rw);
            flight.plan = Some(plan);
            Outcome::passed(format!(
                "local model from {:.0} km: wait {:.0} s, burn {:.1} m/s prograde, coast {:.1} h, lead \
                 angle {:.1} deg (bodies: {})",
                park.altitude_m / 1000.0,
                plan.wait_s,
                plan.dv_mps,
                plan.transfer_time_s / 3600.0,
                plan.lead_angle_rad.to_degrees(),
                match &source {
                    BodySource::Builtin => "built-in Kerbol fixture".to_string(),
                    BodySource::Converted(p) => p.display().to_string(),
                }
            ))
        })
        .await,
    );

    // 8. Schedule the burn on the server and tune it against the server's own prediction.
    results.push(
        timed("execute_transfer_burn", Mode::Server, async {
            let (Some(c), Some(b), Some(id), Some(plan)) = (
                flight.client.as_mut(),
                &flight.bodies,
                flight.vessel_id,
                &flight.plan,
            ) else {
                return Outcome::skipped("no server session (the local model burns in the next step)");
            };
            match server_flight::tune_transfer_burn(
                c,
                b,
                id,
                plan,
                MUN_PERIAPSIS_MIN_M,
                MUN_PERIAPSIS_MAX_M,
            )
            .await
            {
                Ok(t) => {
                    let detail = format!(
                        "ScheduleBurn {:.2} m/s prograde at UT {:.0} ({:+.2} m/s over Hohmann after {} \
                         tries); server predicts Mun SOI at UT {:.0} and periapsis {:.0} km; patches end {:?}",
                        t.dv_mps,
                        t.burn_ut,
                        t.dv_mps - plan.dv_mps,
                        t.attempts,
                        t.predicted_encounter_ut,
                        t.predicted_periapsis_alt_m / 1000.0,
                        t.patch_ends
                    );
                    flight.tuned = Some(t);
                    Outcome::passed(detail)
                }
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 9. Warp through the burn and the coast until the Mun's SOI takes the vessel.
    results.push(
        timed("coast_to_mun_encounter", orbital_mode, async {
            if let (Some(c), Some(b), Some(id), Some(t)) = (
                flight.client.as_mut(),
                &flight.bodies,
                flight.vessel_id,
                &flight.tuned,
            ) {
                let v = match server_flight::warp_until_around(c, id, b.mun, COAST_TIMEOUT).await {
                    Ok(v) => v,
                    Err(e) => return Outcome::failed(e),
                };
                let ut = c.latest.ut();
                let (peri, _, e) = server_flight::orbit_summary(b.mu_mun, b.mun_radius_m, &v);
                let speed = crate::client::orbit_from(b.mu_mun, &v.orbit)
                    .state_at(ut)
                    .1
                    .length();
                // The server lands the handoff exactly on its predicted tick, but the bot only sees it in
                // the next Vessels message: at COAST_WARP each snapshot interval covers this much UT.
                let lag = ut - t.predicted_encounter_ut;
                if !(-1.0..=server_flight::snapshot_lag_s()).contains(&lag) {
                    return Outcome::failed(format!(
                        "Mun SOI entered at UT {ut:.0} but the server predicted UT {:.0} (allowed lag \
                         {:.0} s)",
                        t.predicted_encounter_ut,
                        server_flight::snapshot_lag_s()
                    ));
                }
                if (peri - t.predicted_periapsis_alt_m).abs() > 20_000.0 {
                    return Outcome::failed(format!(
                        "Mun periapsis {:.0} km after the handoff but the server predicted {:.0} km",
                        peri / 1000.0,
                        t.predicted_periapsis_alt_m / 1000.0
                    ));
                }
                return Outcome::passed(format!(
                    "vessel {id} around the Mun at UT {ut:.0} ({:.2} h after the burn), as predicted; \
                     hyperbola e {e:.3}, periapsis {:.0} km, speed {speed:.0} m/s",
                    (ut - t.burn_ut) / 3600.0,
                    peri / 1000.0
                ));
            }
            let Some(rw) = flight.rails.as_mut() else {
                return Outcome::failed("no rails world (plan_mun_transfer did not run)");
            };
            match fly_to_mun_encounter(rw, cfg.parking_altitude_m, 0.0) {
                Ok(run) => Outcome::passed(format!(
                    "local model: Mun SOI entered at UT {:.0} s ({:.2} h after the burn); periapsis {:.0} \
                     km above the Mun, arrival speed {:.0} m/s",
                    run.encounter_ut,
                    (run.encounter_ut - run.plan.burn_ut) / 3600.0,
                    run.mun_periapsis_alt_m / 1000.0,
                    run.arrival_speed_mps
                )),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 10. Capture: retrograde at Mun periapsis, through the server.
    results.push(
        timed("mun_capture_burn", Mode::Server, async {
            let (Some(c), Some(b), Some(id)) =
                (flight.client.as_mut(), &flight.bodies, flight.vessel_id)
            else {
                return Outcome::skipped("no server session");
            };
            if flight.tuned.is_none() {
                return Outcome::skipped("transfer did not run on the server");
            }
            let v = match c.vessel_state(id, Duration::from_secs(10)).await {
                Ok(v) => v,
                Err(e) => return Outcome::failed(e),
            };
            let cap = match server_flight::plan_capture(b, &v, c.latest.ut()) {
                Ok(p) => p,
                Err(e) => return Outcome::failed(e),
            };
            let patches = match c.schedule_burn(id, cap.burn_ut, cap.dv_mps, 0.0, 0.0).await {
                Ok(p) => p,
                Err(e) => return Outcome::failed(e),
            };
            if let Err(e) = server_flight::capture_predicted(&patches, b, cap.burn_ut) {
                return Outcome::failed(e);
            }
            if let Err(e) = server_flight::warp_past(c, cap.burn_ut + 5.0, COAST_TIMEOUT).await {
                return Outcome::failed(e);
            }
            let after = match c.vessel_state(id, Duration::from_secs(10)).await {
                Ok(v) => v,
                Err(e) => return Outcome::failed(e),
            };
            if after.reference_body != b.mun {
                return Outcome::failed("vessel left the Mun's SOI after the capture burn");
            }
            let (peri, apo, e) = server_flight::orbit_summary(b.mu_mun, b.mun_radius_m, &after);
            if e >= 1.0 {
                return Outcome::failed(format!("still hyperbolic after the burn (e {e:.3})"));
            }
            if e > 0.05 {
                return Outcome::failed(format!(
                    "capture burn did not circularise (e {e:.3}, {:.0} x {:.0} km)",
                    peri / 1000.0,
                    apo / 1000.0
                ));
            }
            flight.captured = true;
            Outcome::passed(format!(
                "ScheduleBurn {:.1} m/s retrograde at Mun periapsis (UT {:.0}, arriving at {:.0} m/s); \
                 now in a {:.0} x {:.0} km Mun orbit, e {e:.4}",
                cap.dv_mps,
                cap.burn_ut,
                cap.arrival_speed_mps,
                peri / 1000.0,
                apo / 1000.0
            ))
        })
        .await,
    );

    // 11. Land (stretch goal).
    results.push(
        timed("mun_landing", Mode::Server, async {
            let (Some(c), Some(b), Some(id)) =
                (flight.client.as_mut(), &flight.bodies, flight.vessel_id)
            else {
                return Outcome::skipped("no server session");
            };
            if !flight.captured {
                return Outcome::skipped("the craft is not in Mun orbit");
            }
            let Some(spin) = c.latest.spins.get(b.mun as usize).cloned() else {
                return Outcome::failed("the server sent no BodyRotation for the Mun");
            };
            if let Some(why) = landing::landing_precondition(b.mu_mun, b.mun_radius_m, &spin) {
                return Outcome::blocked(format!(
                    "Mun rotation in the server's body data: {why} (assets/own/kerbol-system.json)"
                ));
            }
            match landing::land(c, id, b.mun, LANDING_TIMEOUT).await {
                Ok(l) => Outcome::passed(format!(
                    "landed on the Mun at lat {:.3} lon {:.3}, {:.0} m above the datum, touching down at \
                     {:.1} m/s after {:.0} s of powered descent ({:.0} s wall); {:.1} t propellant left",
                    l.lat_deg,
                    l.lon_deg,
                    l.altitude_m,
                    l.touchdown_speed_mps,
                    l.game_time_s,
                    l.wall_time_s,
                    l.propellant_t
                )),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    debug_assert_eq!(results.len(), STEP_NAMES.len());
    for (r, n) in results.iter().zip(STEP_NAMES) {
        debug_assert_eq!(r.name, n);
    }
    results
}

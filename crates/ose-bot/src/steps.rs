//! The Mun campaign as an ordered chain of steps.
//!
//! Each step is what a player does next. A step whose game feature does not exist yet returns
//! `Outcome::Blocked` naming the feature; the chain keeps going so later steps that can run still report.
//! Steps that the server supports run through it (`Mode::Server`); the orbital-mechanics steps fall back to
//! the bot's in-process rails model (`Mode::Local`) when no server is given. When a feature lands, replace
//! the `Blocked` body with the real protocol exchange and update the frontier in `tests/mun_run.rs`.

use crate::client::{Client, ConnectError};
use crate::data::{self, BodySource};
use crate::plan::TransferPlan;
use crate::rails::{fly_to_mun_encounter, park_in_kerbin_orbit, plan_mun_transfer, RailsWorld};
use crate::report::{Mode, Outcome, StepResult};
use crate::server_flight::{self, Bodies, TunedBurn};
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
    /// Stock craft to fly, by file stem, in order of preference.
    pub craft_preference: Vec<String>,
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
    /// The stock craft file chosen in `load_stock_craft`, parsed.
    craft_json: Option<serde_json::Value>,
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

    // 3. Start or join the shared campaign.
    results.push(
        timed("start_campaign", Mode::Server, async {
            let Some(c) = flight.client.as_mut() else {
                return Outcome::skipped("no server session");
            };
            match c.join_campaign().await {
                Ok(cs) => Outcome::passed(format!(
                    "JoinCampaign: member of {:?} (started tick {}, {} member{}, {} vessel{})",
                    cs.name,
                    cs.started_tick.0,
                    cs.members.len(),
                    if cs.members.len() == 1 { "" } else { "s" },
                    cs.vessels,
                    if cs.vessels == 1 { "" } else { "s" }
                )),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 4. Pick a Mun-capable stock craft from the install's assemblies.
    results.push(
        timed("load_stock_craft", Mode::Local, async {
            let prefs: Vec<&str> = cfg.craft_preference.iter().map(String::as_str).collect();
            match data::find_stock_craft(&cfg.stock_dir, &prefs) {
                Ok(Some(c)) => {
                    flight.craft_json = std::fs::read_to_string(&c.file)
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok());
                    Outcome::passed(format!(
                        "{} ({} parts, {} stages, {:.2} t) from {}",
                        c.name,
                        c.part_names.len(),
                        c.stages,
                        c.mass_t,
                        c.file.file_name().unwrap_or_default().to_string_lossy()
                    ))
                }
                Ok(None) => Outcome::skipped(format!(
                    "no stock assemblies at {} (run tools/extract_bundles.py)",
                    cfg.stock_dir.display()
                )),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 5. Upload the craft and put it on the pad.
    results.push(
        timed("upload_and_launch_craft", Mode::Server, async {
            let Some(c) = flight.client.as_mut() else {
                return Outcome::skipped("no server session");
            };
            let Some(json) = &flight.craft_json else {
                return Outcome::skipped("no stock craft loaded on this machine");
            };
            let craft = match ose_importer::craft::craft_from_value(json) {
                Ok(c) => c,
                Err(e) => return Outcome::failed(e),
            };
            match c.launch_craft(craft, cfg.parking_altitude_m).await {
                Ok(l) => {
                    // From here on the bot flies the launched craft, not the welcome probe.
                    flight.vessel_id = Some(l.vessel);
                    Outcome::passed(format!(
                        "LaunchCraft {:?}: vessel {} with {} parts, {:.2} t wet / {:.2} t dry, {:.0} m/s \
                         vacuum delta-v, placed in a {:.0} km circular orbit (no pad ascent yet)",
                        l.name,
                        l.vessel,
                        l.parts,
                        l.wet_mass_t,
                        l.dry_mass_t,
                        l.delta_v_vacuum_mps,
                        cfg.parking_altitude_m / 1000.0
                    ))
                }
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 6. Fly the ascent under thrust.
    results.push(
        timed("ascend_to_orbit", Mode::Server, async {
            Outcome::blocked(
                "physics bubble, engines and attitude/throttle intents; Phase 6. Stand-in: the server \
                 spawns the probe already in a 100 km Kerbin orbit",
            )
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
            Outcome::blocked("terrain, surface collision and powered descent; Phase 7")
        })
        .await,
    );

    debug_assert_eq!(results.len(), STEP_NAMES.len());
    for (r, n) in results.iter().zip(STEP_NAMES) {
        debug_assert_eq!(r.name, n);
    }
    results
}

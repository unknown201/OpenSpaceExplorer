//! The Mun campaign as an ordered chain of steps.
//!
//! Each step is what a player does next. A step whose game feature does not exist yet returns
//! `Outcome::Blocked` naming the feature; the chain keeps going so later steps that can run (the local
//! orbital-mechanics ones) still report. When a feature lands, replace the `Blocked` body with the real
//! protocol exchange and update the frontier in `tests/mun_run.rs`.

use crate::client::{Client, ConnectError};
use crate::data::{self, BodySource};
use crate::rails::{fly_to_mun_encounter, park_in_kerbin_orbit, plan_mun_transfer, RailsWorld};
use crate::report::{Mode, Outcome, StepResult};
use std::path::PathBuf;
use std::time::Instant;

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

#[derive(Clone, Debug)]
pub struct BotConfig {
    /// `host:port` of a running `ose-server`; `None` skips the server steps.
    pub server: Option<String>,
    pub game_data: PathBuf,
    pub stock_dir: PathBuf,
    /// Stock craft to fly, by file stem, in order of preference.
    pub craft_preference: Vec<String>,
    /// Parking orbit altitude above Kerbin, m.
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
struct Flight {
    rails: Option<RailsWorld>,
    body_source: Option<BodySource>,
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
    let mut flight = Flight {
        rails: None,
        body_source: None,
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

    // 2. Open a protocol session: Hello -> Welcome.
    results.push(
        timed("connect_hello", Mode::Server, async {
            let Some(base) = &cfg.server else {
                return Outcome::skipped("no --server given");
            };
            match Client::connect(base).await {
                Ok(mut c) => match c.hello(&cfg.display_name).await {
                    Ok(w) => Outcome::passed(format!(
                        "player {} welcomed at tick {}",
                        w.player_id, w.tick.0
                    )),
                    Err(e) => Outcome::failed(e),
                },
                Err(ConnectError::NoEndpoint { status }) => Outcome::blocked(format!(
                    "server WebSocket endpoint at {} (upgrade answered HTTP {status}); Phase 3",
                    crate::client::WS_PATH
                )),
                Err(e) => Outcome::failed(e),
            }
        })
        .await,
    );

    // 3. Start or join the shared campaign.
    results.push(
        timed("start_campaign", Mode::Server, async {
            Outcome::blocked(
                "campaign/save protocol (no ClientMessage to start or join a campaign); Phase 4",
            )
        })
        .await,
    );

    // 4. Pick a Mun-capable stock craft from the install's assemblies.
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

    // 5. Upload the craft and put it on the pad.
    results.push(
        timed("upload_and_launch_craft", Mode::Server, async {
            Outcome::blocked(
                "craft upload and spawn-at-launch-site protocol (ose-data has no Craft schema); Phase 5",
            )
        })
        .await,
    );

    // 6. Fly the ascent under thrust.
    results.push(
        timed("ascend_to_orbit", Mode::Server, async {
            Outcome::blocked(
                "physics bubble, engines and attitude/throttle intents; Phase 6. Local fallback: the bot \
                 parks itself in a 100 km orbit on rails",
            )
        })
        .await,
    );

    // 7. Plan the transfer window and burn from the parking orbit.
    results.push(
        timed("plan_mun_transfer", Mode::Local, async {
            let (bodies, source) = match data::load_bodies(&cfg.game_data) {
                Ok(x) => x,
                Err(e) => return Outcome::failed(e),
            };
            let mut rw = match RailsWorld::new(&bodies) {
                Ok(rw) => rw,
                Err(e) => return Outcome::failed(e),
            };
            let ut0 = 0.0;
            let park = park_in_kerbin_orbit(&mut rw, "bot", cfg.parking_altitude_m, 0.0, ut0);
            let plan = match plan_mun_transfer(&rw, park.vessel, ut0) {
                Ok(p) => p,
                Err(e) => return Outcome::failed(e),
            };
            flight.body_source = Some(source.clone());
            flight.rails = Some(rw);
            Outcome::passed(format!(
                "from {:.0} km: wait {:.0} s, burn {:.1} m/s prograde, coast {:.1} h, lead angle {:.1} deg \
                 (bodies: {})",
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

    // 8. Execute the burn through the server.
    results.push(
        timed("execute_transfer_burn", Mode::Server, async {
            Outcome::blocked(
                "maneuver-node or burn intent in the protocol and server-driven vessels; Phase 2/4. \
                 Local fallback: impulsive prograde burn on rails in the next step",
            )
        })
        .await,
    );

    // 9. Coast until the Mun's SOI takes the vessel (locally, on rails).
    results.push(
        timed("coast_to_mun_encounter", Mode::Local, async {
            let Some(rw) = flight.rails.as_mut() else {
                return Outcome::failed("no rails world (plan_mun_transfer did not run)");
            };
            // Re-fly from a fresh vessel so the step is self-contained.
            match fly_to_mun_encounter(rw, cfg.parking_altitude_m, 0.0) {
                Ok(run) => Outcome::passed(format!(
                    "Mun SOI entered at UT {:.0} s ({:.2} h after the burn); periapsis {:.0} km above \
                     the Mun, arrival speed {:.0} m/s",
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

    // 10. Capture into Mun orbit.
    results.push(
        timed("mun_capture_burn", Mode::Server, async {
            Outcome::blocked("patched-conic predictor (Mun periapsis) plus burn intents; Phase 2/4")
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

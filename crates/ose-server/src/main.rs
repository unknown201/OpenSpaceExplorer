//! Authoritative game server.
//!
//! - `/health`: plain text with the current tick.
//! - `/ws`: the protocol session (`ose-net`): `Hello` -> `Welcome` + `BodyCatalog`, then `TimeState` and
//!   `Snapshot` streamed at `SNAPSHOT_HZ`; `RequestWarp` sets the shared warp factor (lowest-wins arbitration
//!   comes with multiplayer in Phase 4).
//! - With `--static-dir`, also serves the built web client so one process is the whole deployment.
//!
//! The Kerbol system is embedded from `assets/own/kerbol-system.json` (bodies only) and can be overridden
//! with `--bodies PATH`.

mod accounts;
mod save;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use ose_data::{CelestialBody, Tick};
use ose_net::{
    BodyInfo, BodyState, ClientMessage, OrbitElements, PatchEnd, ServerMessage, TrajectoryPatch,
    VesselState, PROTOCOL_VERSION, SNAPSHOT_HZ, TRAJECTORY_HZ, WS_PATH,
};
use ose_sim::trajectory::Transition;
use ose_sim::Universe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::services::{ServeDir, ServeFile};

const EMBEDDED_BODIES: &str = include_str!("../../../assets/own/kerbol-system.json");
const EMBEDDED_PARTS: &str = include_str!("../../../assets/own/parts-catalog.json");
const EMBEDDED_RESOURCES: &str = include_str!("../../../assets/own/resources.json");

#[derive(Parser, Debug)]
#[command(name = "ose-server")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7777")]
    listen: String,
    /// Directory with the built web client to serve at `/` (its `index.html` answers unknown paths).
    #[arg(long)]
    static_dir: Option<PathBuf>,
    /// JSON array of `ose_data::CelestialBody` to use instead of the embedded Kerbol system.
    #[arg(long)]
    bodies: Option<PathBuf>,
    /// JSON file for player accounts (resume tokens); `:memory:` keeps nothing.
    #[arg(long, default_value = "ose-accounts.json")]
    db: PathBuf,
    /// Universe save file (time, warp, vessels); loaded at start, written periodically. `none` disables.
    #[arg(long, default_value = "ose-save.json")]
    save: PathBuf,
    /// Seconds between periodic saves.
    #[arg(long, default_value_t = 30.0)]
    save_interval_s: f64,
}

struct Shared {
    universe: Mutex<Universe>,
    catalog: Vec<BodyInfo>,
    parts: Vec<ose_data::PartDef>,
    resources: Vec<ose_data::ResourceDef>,
    accounts: Mutex<accounts::Accounts>,
    /// Vessel entity -> (vessel id, owner player id).
    vessels: Mutex<Vec<(bevy_ecs::entity::Entity, u64, Option<u64>)>>,
    campaign: Mutex<save::Campaign>,
    /// Display names by player id for this process lifetime.
    names: Mutex<std::collections::HashMap<u64, String>>,
    /// Warp each connected player is willing to run at; the universe runs at the minimum (lowest wins).
    warp_requests: Mutex<std::collections::HashMap<u64, u32>>,
}

/// Apply lowest-wins warp arbitration. Returns (effective warp, the player holding it down, if any).
fn apply_warp(s: &Shared) -> (u32, Option<u64>) {
    let reqs = s.warp_requests.lock().expect("warp lock");
    let (warp, holder) = match reqs.iter().min_by_key(|(_, &w)| w) {
        Some((&p, &w)) => (w, Some(p)),
        None => (1, None),
    };
    let max = reqs.values().copied().max().unwrap_or(1);
    drop(reqs);
    s.universe.lock().expect("sim lock").set_warp(warp);
    (warp, if max > warp { holder } else { None })
}

fn elements(o: &ose_math::Orbit) -> OrbitElements {
    let e = &o.elements;
    OrbitElements {
        semi_major_axis_m: e.semi_major_axis,
        eccentricity: e.eccentricity,
        inclination_deg: e.inclination_deg,
        longitude_of_ascending_node_deg: e.longitude_of_ascending_node_deg,
        argument_of_periapsis_deg: e.argument_of_periapsis_deg,
        mean_anomaly_at_epoch_rad: e.mean_anomaly_at_epoch,
        epoch_s: e.epoch,
    }
}

type AppState = Arc<Shared>;

fn catalog_from(defs: &[CelestialBody]) -> Vec<BodyInfo> {
    defs.iter()
        .map(|b| BodyInfo {
            id: b.id.clone(),
            parent: b
                .parent
                .as_ref()
                .and_then(|p| defs.iter().position(|x| &x.id == p))
                .map(|i| i as u16),
            radius_m: b.radius_m,
            mu: b.mu,
            soi_radius_m: b.soi_radius_m,
            is_star: b.is_star,
            has_atmosphere: b.atmosphere.is_some(),
            atmosphere_depth_m: b.atmosphere.as_ref().map(|a| a.depth_m).unwrap_or(0.0),
            orbit: b.orbit.as_ref().map(|o| OrbitElements {
                semi_major_axis_m: o.semi_major_axis_m,
                eccentricity: o.eccentricity,
                inclination_deg: o.inclination_deg,
                longitude_of_ascending_node_deg: o.longitude_of_ascending_node_deg,
                argument_of_periapsis_deg: o.argument_of_periapsis_deg,
                mean_anomaly_at_epoch_rad: o.mean_anomaly_at_epoch_rad,
                epoch_s: o.epoch_s,
            }),
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    let args = Args::parse();

    let bodies_json = match &args.bodies {
        Some(p) => std::fs::read_to_string(p)?,
        None => EMBEDDED_BODIES.to_string(),
    };
    let defs: Vec<CelestialBody> = serde_json::from_str(&bodies_json)?;
    let catalog = catalog_from(&defs);
    tracing::info!(bodies = defs.len(), "loaded celestial bodies");

    let mut universe = Universe::with_bodies(&defs);
    let save_path = (args.save.to_string_lossy() != "none").then(|| args.save.clone());
    let mut registry: Vec<save::VesselEntry> = Vec::new();
    let mut campaign = save::Campaign::default();
    if let Some(p) = &save_path {
        match save::read(p) {
            Ok(Some(file)) => {
                registry = save::restore(&mut universe, &file);
                campaign = file.campaign.clone();
                tracing::info!(path = %p.display(), tick = file.tick.0, vessels = registry.len(), "save loaded");
            }
            Ok(None) => tracing::info!(path = %p.display(), "no save file, starting fresh"),
            Err(e) => {
                tracing::error!(path = %p.display(), error = %e, "save file unreadable; refusing to start");
                return Err(e);
            }
        }
    }
    let shared: AppState = Arc::new(Shared {
        universe: Mutex::new(universe),
        catalog,
        parts: serde_json::from_str(EMBEDDED_PARTS)?,
        resources: serde_json::from_str(EMBEDDED_RESOURCES)?,
        accounts: Mutex::new({
            let acc = accounts::Accounts::open(&args.db)?;
            tracing::info!(db = %args.db.display(), players = acc.count()?, "accounts opened");
            acc
        }),
        vessels: Mutex::new(registry),
        campaign: Mutex::new(campaign),
        names: Mutex::new(std::collections::HashMap::new()),
        warp_requests: Mutex::new(std::collections::HashMap::new()),
    });

    let sim = shared.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(
            1000 / ose_sim::TICKS_PER_SECOND,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            sim.universe.lock().expect("sim lock").step();
        }
    });

    if let Some(p) = save_path.clone() {
        let saver = shared.clone();
        let every = std::time::Duration::from_secs_f64(args.save_interval_s.max(0.2));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            interval.tick().await;
            loop {
                interval.tick().await;
                let file = {
                    let u = saver.universe.lock().expect("sim lock");
                    let v = saver.vessels.lock().expect("vessels lock");
                    let c = saver.campaign.lock().expect("campaign lock");
                    save::capture(&u, &v, &c)
                };
                match save::write(&p, &file) {
                    Ok(()) => {
                        tracing::debug!(tick = file.tick.0, vessels = file.vessels.len(), "saved")
                    }
                    Err(e) => tracing::error!(error = %e, "save failed"),
                }
            }
        });
    }

    let mut app = Router::new()
        .route(
            "/health",
            get(|State(s): State<AppState>| async move {
                format!("ok tick={}", s.universe.lock().expect("sim lock").tick().0)
            }),
        )
        .route(WS_PATH, get(ws_upgrade))
        .with_state(shared);
    if let Some(dir) = &args.static_dir {
        let index = ServeFile::new(dir.join("index.html"));
        app = app.fallback_service(ServeDir::new(dir).fallback(index));
        tracing::info!(dir = %dir.display(), "serving web client");
    }

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, "server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(s): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| session(socket, s))
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage) -> bool {
    match ose_net::encode(msg) {
        Ok(bytes) => socket.send(Message::Binary(bytes.into())).await.is_ok(),
        Err(e) => {
            tracing::error!(error = %e, "encode");
            false
        }
    }
}

fn catalog_index(s: &Shared, u: &Universe, body: bevy_ecs::entity::Entity) -> u16 {
    let id = u.body_id(body).unwrap_or("");
    s.catalog.iter().position(|b| b.id == id).unwrap_or(0) as u16
}

fn vessels_message(s: &Shared, u: &Universe, tick: Tick) -> ServerMessage {
    let ut = tick.seconds();
    let list = s.vessels.lock().expect("vessels lock");
    let vessels = list
        .iter()
        .filter_map(|&(e, id, owner)| {
            let (v, body, orbit) = u.vessel(e)?;
            let (bp, bv) = ose_sim::bodies::star_space_state(&u.world, body, ut);
            let (rp, rv) = orbit.state_at(ut);
            Some(VesselState {
                id,
                name: v.name.clone(),
                owner,
                reference_body: catalog_index(s, u, body),
                position_m: bp + rp,
                velocity_mps: bv + rv,
                orbit: elements(&orbit),
            })
        })
        .collect();
    ServerMessage::Vessels { tick, vessels }
}

fn trajectories(s: &Shared) -> Vec<ServerMessage> {
    let u = s.universe.lock().expect("sim lock");
    let tick = u.tick();
    let list = s.vessels.lock().expect("vessels lock");
    list.iter()
        .map(|&(e, id, _)| {
            let patches = u
                .predict(e)
                .iter()
                .map(|p| TrajectoryPatch {
                    body: catalog_index(s, &u, p.body),
                    orbit: elements(&p.orbit),
                    start_ut: p.start_ut,
                    end_ut: p.end_ut,
                    end: match p.end {
                        Transition::Encounter => PatchEnd::Encounter,
                        Transition::Escape => PatchEnd::Escape,
                        Transition::Collision => PatchEnd::Collision,
                        Transition::Maneuver => PatchEnd::Maneuver,
                        Transition::Initial | Transition::Final => PatchEnd::Final,
                    },
                })
                .collect();
            ServerMessage::Trajectory {
                tick,
                vessel: id,
                patches,
            }
        })
        .collect()
}

fn snapshot(s: &Shared) -> (ServerMessage, ServerMessage, ServerMessage) {
    let (warp, warp_limited_by) = apply_warp(s);
    let u = s.universe.lock().expect("sim lock");
    let tick = u.tick();
    let ut = tick.seconds();
    let bodies = s
        .catalog
        .iter()
        .map(|b| {
            let (p, v) = u.body_state(&b.id, ut).unwrap_or_default();
            BodyState {
                position_m: p,
                velocity_mps: v,
            }
        })
        .collect();
    (
        ServerMessage::TimeState {
            tick,
            warp,
            warp_limited_by,
        },
        ServerMessage::Snapshot { tick, bodies },
        vessels_message(s, &u, tick),
    )
}

/// The entity of vessel `id` if `player` owns it.
fn owned_vessel(s: &Shared, id: u64, player: u64) -> Option<bevy_ecs::entity::Entity> {
    s.vessels
        .lock()
        .expect("vessels lock")
        .iter()
        .find(|&&(_, vid, owner)| vid == id && owner == Some(player))
        .map(|&(e, _, _)| e)
}

fn has_vessel(s: &Shared, player: u64) -> bool {
    s.vessels
        .lock()
        .expect("vessels lock")
        .iter()
        .any(|&(_, _, owner)| owner == Some(player))
}

/// Display name a player used at Hello.
fn display_name_for(s: &Shared, player_id: u64) -> String {
    s.names
        .lock()
        .expect("names lock")
        .get(&player_id)
        .cloned()
        .unwrap_or_else(|| format!("player {player_id}"))
}

/// Campaign state; with `join = Some(name)` the player is added as a member first (idempotent).
fn campaign_state(s: &Shared, player_id: u64, join: Option<&String>) -> ServerMessage {
    let tick = s.universe.lock().expect("sim lock").tick();
    let mut c = s.campaign.lock().expect("campaign lock");
    if let Some(name) = join {
        if !c.members.iter().any(|m| m.player_id == player_id) {
            c.members.push(save::CampaignMember {
                player_id,
                display_name: name.clone(),
                joined_tick: tick,
            });
            tracing::info!(player = player_id, "joined the campaign");
        }
    }
    let vessels = s.vessels.lock().expect("vessels lock").len() as u32;
    ServerMessage::CampaignState {
        name: c.name.clone(),
        started_tick: c.started_tick,
        members: c
            .members
            .iter()
            .map(|m| ose_net::CampaignMember {
                player_id: m.player_id,
                display_name: m.display_name.clone(),
                joined_tick: m.joined_tick,
            })
            .collect(),
        you_are_member: c.members.iter().any(|m| m.player_id == player_id),
        vessels,
    }
}

/// Validate an uploaded craft against the part catalog and spawn it as a vessel in a circular orbit.
fn launch_craft(
    s: &Shared,
    player_id: u64,
    craft: &ose_data::craft::Craft,
    altitude_m: f64,
) -> ServerMessage {
    let name = craft.name.clone();
    let parts = craft.parts.len() as u32;
    let m = craft.mass(&s.parts, &s.resources);
    let report = ose_data::staging::delta_v_by_stage(craft, &s.parts, &s.resources);
    let stages: Vec<ose_net::StageReadout> = report
        .stages
        .iter()
        .map(|st| ose_net::StageReadout {
            stage_id: st.stage_id as u32,
            start_mass_t: st.start_mass_t,
            end_mass_t: st.end_mass_t,
            delta_v_vacuum_mps: st.delta_v_vacuum_mps,
            burn_time_s: st.burn_time_s,
            thrust_kn: st.thrust_kn,
        })
        .collect();
    let dv = if report.total_vacuum_mps > 0.0 {
        report.total_vacuum_mps
    } else {
        ose_data::craft::delta_v_vacuum_mps(craft, &s.parts, &s.resources)
    };
    let fail = |reason: String| ServerMessage::CraftLaunched {
        vessel: None,
        name: name.clone(),
        parts,
        wet_mass_t: m.wet_t(),
        dry_mass_t: m.dry_t,
        delta_v_vacuum_mps: dv,
        stages: stages.clone(),
        reason: Some(reason),
    };
    if craft.parts.is_empty() {
        return fail("craft has no parts".into());
    }
    if !craft.is_tree() {
        return fail("craft parts do not form a single tree".into());
    }
    if !m.unknown_parts.is_empty() {
        return fail(format!("unknown parts: {:?}", m.unknown_parts));
    }
    if !(altitude_m.is_finite() && (70_000.0..=1.0e9).contains(&altitude_m)) {
        return fail("altitude must be between 70 km and 1,000,000 km".into());
    }
    let mut u = s.universe.lock().expect("sim lock");
    let Some(home) = s
        .catalog
        .iter()
        .find(|b| b.id == "Kerbin")
        .or_else(|| s.catalog.first())
    else {
        return fail("no home body".into());
    };
    let Some(body) = u.body_entity(&home.id) else {
        return fail("home body missing".into());
    };
    let e = u.spawn_vessel_in_circular_orbit(&name, body, altitude_m);
    let mut list = s.vessels.lock().expect("vessels lock");
    let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
    list.push((e, id, Some(player_id)));
    tracing::info!(player = player_id, vessel = id, craft = %name, parts, wet_t = m.wet_t(), dv, "craft launched");
    ServerMessage::CraftLaunched {
        vessel: Some(id),
        name,
        parts,
        wet_mass_t: m.wet_t(),
        dry_mass_t: m.dry_t,
        delta_v_vacuum_mps: dv,
        stages,
        reason: None,
    }
}

/// Every player gets a probe in a 100 km circular orbit around the home world on joining.
fn spawn_player_probe(s: &Shared, player_id: u64, name: &str) -> Option<u64> {
    let mut u = s.universe.lock().expect("sim lock");
    let home = s
        .catalog
        .iter()
        .find(|b| b.id == "Kerbin")
        .or_else(|| s.catalog.first())?;
    let body = u.body_entity(&home.id)?;
    let e = u.spawn_vessel_in_circular_orbit(&format!("{name} probe"), body, 100_000.0);
    let mut list = s.vessels.lock().expect("vessels lock");
    let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
    list.push((e, id, Some(player_id)));
    Some(id)
}

async fn session(mut socket: WebSocket, s: AppState) {
    // Handshake.
    let player_id = loop {
        match socket.recv().await {
            Some(Ok(Message::Binary(b))) => {
                match ose_net::decode::<ClientMessage>(&b) {
                    Ok(ClientMessage::Hello {
                        protocol,
                        display_name,
                        token,
                    }) => {
                        if protocol != PROTOCOL_VERSION {
                            let reason = format!("protocol {protocol} not supported, server speaks {PROTOCOL_VERSION}");
                            send(&mut socket, &ServerMessage::Rejected { reason }).await;
                            return;
                        }
                        let outcome: Result<(u64, String, bool), String> = {
                            let mut acc = s.accounts.lock().expect("accounts lock");
                            let resumed = match token.as_deref() {
                                Some(t) => acc.resume(t, &display_name).ok().flatten(),
                                None => None,
                            };
                            match resumed {
                                Some(p) => Ok((p.id, token.clone().unwrap_or_default(), true)),
                                None => acc
                                    .create(&display_name)
                                    .map(|(p, t)| (p.id, t, false))
                                    .map_err(|e| format!("account error: {e}")),
                            }
                        };
                        let (id, token_out, resumed) = match outcome {
                            Ok(v) => v,
                            Err(reason) => {
                                send(&mut socket, &ServerMessage::Rejected { reason }).await;
                                return;
                            }
                        };
                        tracing::info!(player = id, name = %display_name, resumed, "player joined");
                        let tick: Tick = s.universe.lock().expect("sim lock").tick();
                        let welcome = ServerMessage::Welcome {
                            player_id: id,
                            token: token_out,
                            tick,
                        };
                        if !send(&mut socket, &welcome).await {
                            return;
                        }
                        if !send(
                            &mut socket,
                            &ServerMessage::BodyCatalog {
                                bodies: s.catalog.clone(),
                            },
                        )
                        .await
                        {
                            return;
                        }
                        if !has_vessel(&s, id) {
                            spawn_player_probe(&s, id, &display_name);
                        }
                        s.warp_requests.lock().expect("warp lock").insert(id, 1);
                        s.names
                            .lock()
                            .expect("names lock")
                            .insert(id, display_name.clone());
                        break id;
                    }
                    Ok(_) => {
                        send(
                            &mut socket,
                            &ServerMessage::Rejected {
                                reason: "say Hello first".into(),
                            },
                        )
                        .await;
                        return;
                    }
                    Err(e) => {
                        send(
                            &mut socket,
                            &ServerMessage::Rejected {
                                reason: format!("bad message: {e}"),
                            },
                        )
                        .await;
                        return;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
            Some(Ok(_)) => continue,
        }
    };

    // Stream.
    let mut interval =
        tokio::time::interval(std::time::Duration::from_millis(1000 / SNAPSHOT_HZ as u64));
    let mut traj_interval = tokio::time::interval(std::time::Duration::from_millis(
        1000 / TRAJECTORY_HZ as u64,
    ));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let (time, snap, vessels) = snapshot(&s);
                if !send(&mut socket, &time).await
                    || !send(&mut socket, &snap).await
                    || !send(&mut socket, &vessels).await
                {
                    break;
                }
            }
            _ = traj_interval.tick() => {
                for msg in trajectories(&s) {
                    if !send(&mut socket, &msg).await {
                        break;
                    }
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Binary(b))) => match ose_net::decode::<ClientMessage>(&b) {
                        Ok(ClientMessage::RequestWarp { factor }) => {
                            let f = factor.clamp(1, 10_000_000);
                            s.warp_requests.lock().expect("warp lock").insert(player_id, f);
                            let (eff, _) = apply_warp(&s);
                            tracing::info!(player = player_id, requested = f, effective = eff, "warp request");
                        }
                        Ok(ClientMessage::Ping { sent_tick }) => {
                            if !send(&mut socket, &ServerMessage::Pong { sent_tick }).await { break; }
                        }
                        Ok(ClientMessage::ScheduleBurn { vessel, ut, prograde_mps, normal_mps, radial_mps }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let burn = ose_sim::vessels::ScheduledBurn { ut, prograde_mps, normal_mps, radial_mps };
                                    let ok = s.universe.lock().expect("sim lock").schedule_burn(e, burn);
                                    ServerMessage::BurnState { vessel, scheduled: ok, reason: (!ok).then(|| "burn time is in the past".to_string()) }
                                }
                                None => ServerMessage::BurnState { vessel, scheduled: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                            for msg in trajectories(&s) {
                                if !send(&mut socket, &msg).await { break; }
                            }
                        }
                        Ok(ClientMessage::ClearBurn { vessel }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    s.universe.lock().expect("sim lock").world.entity_mut(e).remove::<ose_sim::vessels::ScheduledBurn>();
                                    ServerMessage::BurnState { vessel, scheduled: false, reason: None }
                                }
                                None => ServerMessage::BurnState { vessel, scheduled: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::JoinCampaign) => {
                            let name = display_name_for(&s, player_id);
                            let reply = campaign_state(&s, player_id, Some(&name));
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::CampaignInfo) => {
                            let reply = campaign_state(&s, player_id, None);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LaunchCraft { craft, altitude_m }) => {
                            let reply = launch_craft(&s, player_id, &craft, altitude_m);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::Hello { .. }) => {}
                        Err(e) => tracing::warn!(player = player_id, error = %e, "bad client message"),
                    },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
    s.warp_requests
        .lock()
        .expect("warp lock")
        .remove(&player_id);
    apply_warp(&s);
    tracing::info!(player = player_id, "player left");
}

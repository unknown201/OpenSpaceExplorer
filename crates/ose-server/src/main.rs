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
mod science;

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
use ose_sim::flight::{Attitude, Situation};
use ose_sim::trajectory::Transition;
use ose_sim::Universe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::services::{ServeDir, ServeFile};

const EMBEDDED_BODIES: &str = include_str!("../../../assets/own/kerbol-system.json");
const EMBEDDED_PARTS: &str = include_str!("../../../assets/own/parts-catalog.json");
const EMBEDDED_RESOURCES: &str = include_str!("../../../assets/own/resources.json");
const EMBEDDED_DIFFICULTY: &str = include_str!("../../../assets/own/difficulty.json");
const EMBEDDED_STOCK_CRAFT: &str = include_str!("../../../assets/own/stock-craft.json");
const EMBEDDED_SCIENCE: &str = include_str!("../../../assets/own/science/index.json");
const EMBEDDED_REGIONS: &[u8] = include_bytes!("../../../assets/own/science/regions.bin");
const EMBEDDED_TECH_TREE: &str = include_str!("../../../assets/own/tech-tree.json");
const EMBEDDED_MISSIONS: &str = include_str!("../../../assets/own/missions.json");
const EMBEDDED_TERRAIN_INDEX: &str = include_str!("../../../assets/own/terrain/index.json");
const EMBEDDED_TERRAIN: &[u8] = include_bytes!("../../../assets/own/terrain/terrain.bin");
/// The launch pad craft are placed on (from the Kerbin body data's LocalSimObjectsData).
const LAUNCH_PAD_SITE: &str = "kerbin_KSCLaunchpad_spawn_01";
/// Saved craft ids start here; stock crafts use 1..=999.
const FIRST_SAVED_CRAFT_ID: u64 = 1000;
pub const SHARED_SESSION_ID: &str = "shared";

#[derive(serde::Deserialize)]
struct DifficultyFile {
    definitions: Vec<ose_net::DifficultyDefinition>,
    presets:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>>,
}

fn difficulty_message() -> anyhow::Result<ServerMessage> {
    let f: DifficultyFile = serde_json::from_str(EMBEDDED_DIFFICULTY)?;
    let presets = f
        .presets
        .into_iter()
        .map(|(name, opts)| ose_net::DifficultyPreset {
            name,
            options: opts
                .into_iter()
                .map(|(id, v)| ose_net::DifficultyOption {
                    id,
                    value: match v {
                        serde_json::Value::Bool(b) => ose_net::DifficultyValue::Bool(b),
                        serde_json::Value::Number(n) => {
                            ose_net::DifficultyValue::Number(n.as_f64().unwrap_or(0.0))
                        }
                        other => ose_net::DifficultyValue::Choice(other.to_string()),
                    },
                })
                .collect(),
        })
        .collect();
    Ok(ServerMessage::DifficultyOptions {
        definitions: f.definitions,
        presets,
    })
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Parser, Debug)]
#[command(name = "ose-server")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7777")]
    listen: String,
    /// Directory with the built web client to serve at `/` (its `index.html` answers unknown paths).
    #[arg(long)]
    static_dir: Option<PathBuf>,
    /// Directory of converted game assets (glTF, textures) served at `/assets/` with a one-day cache;
    /// missing files are 404, no index fallback.
    #[arg(long)]
    assets_dir: Option<PathBuf>,
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
    /// Players who never joined the campaign are removed, with their vessels, after this many seconds
    /// without a connection (e2e pilots and one-off visitors). 0 disables.
    #[arg(long, default_value_t = 86_400)]
    guest_ttl_s: u64,
    /// Seconds between guest sweeps.
    #[arg(long, default_value_t = 60.0)]
    guest_sweep_s: f64,
}

/// What the client needs to draw and place a part (served at `/parts.json`).
#[derive(serde::Serialize)]
struct PartInfo {
    id: String,
    title: String,
    category: String,
    size_category: String,
    mass_t: f64,
    crew_capacity: u32,
    stage_type: String,
    size_m: ose_math::DVec3,
    center_m: ose_math::DVec3,
    attach_nodes: Vec<AttachNodeInfo>,
    attach_rules: AttachRulesInfo,
    resource_containers: Vec<ContainerInfo>,
    engines: Vec<EngineInfo>,
    has_engine: bool,
    is_decoupler: bool,
}

#[derive(serde::Serialize)]
struct AttachRulesInfo {
    stack: bool,
    srf_attach: bool,
    allow_stack: bool,
    allow_srf_attach: bool,
}

#[derive(serde::Serialize)]
struct ContainerInfo {
    resource: String,
    capacity_units: f64,
    initial_units: f64,
}

#[derive(serde::Serialize)]
struct EngineInfo {
    id: String,
    max_thrust_kn: f64,
    isp_vacuum_s: f64,
    isp_sea_level_s: f64,
    propellant_mixture: String,
}

#[derive(serde::Serialize)]
struct AttachNodeInfo {
    id: String,
    node_type: String,
    position: ose_math::DVec3,
    orientation: ose_math::DVec3,
    size: i32,
}

fn part_infos(parts: &[ose_data::PartDef]) -> Vec<PartInfo> {
    parts
        .iter()
        .map(|p| {
            let (size_m, center_m) = match p.bounds {
                Some(b) => (b.size_m, b.center_m),
                None => (ose_math::DVec3::new(1.0, 1.0, 1.0), ose_math::DVec3::ZERO),
            };
            PartInfo {
                id: p.id.clone(),
                title: if p.title_key.is_empty() {
                    p.id.clone()
                } else {
                    p.title_key.clone()
                },
                category: p.category.clone(),
                size_category: p.size_category.clone(),
                mass_t: p.mass_t,
                crew_capacity: p.crew_capacity,
                stage_type: p.stage_type.clone(),
                size_m,
                center_m,
                attach_nodes: p
                    .attach_nodes
                    .iter()
                    .map(|n| AttachNodeInfo {
                        id: n.id.clone(),
                        node_type: n.node_type.clone(),
                        position: n.position,
                        orientation: n.orientation,
                        size: n.size,
                    })
                    .collect(),
                attach_rules: AttachRulesInfo {
                    stack: p.attach_rules.stack,
                    srf_attach: p.attach_rules.srf_attach,
                    allow_stack: p.attach_rules.allow_stack,
                    allow_srf_attach: p.attach_rules.allow_srf_attach,
                },
                resource_containers: p
                    .resource_containers
                    .iter()
                    .map(|c| ContainerInfo {
                        resource: c.resource.clone(),
                        capacity_units: c.capacity_units,
                        initial_units: c.initial_units,
                    })
                    .collect(),
                engines: p
                    .engines
                    .iter()
                    .map(|e| EngineInfo {
                        id: e.id.clone(),
                        max_thrust_kn: e.max_thrust_kn,
                        isp_vacuum_s: e.isp_curve.evaluate(0.0),
                        isp_sea_level_s: e.isp_curve.evaluate(1.0),
                        propellant_mixture: e.propellant_mixture.clone(),
                    })
                    .collect(),
                has_engine: !p.engines.is_empty(),
                is_decoupler: matches!(
                    p.stage_type.as_str(),
                    "DecouplerHorizontal" | "DecouplerVertical"
                ) || p.category == "Coupling",
            }
        })
        .collect()
}

async fn json_route(body: Arc<String>) -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body.as_str().to_owned(),
    )
}

struct Shared {
    universe: Mutex<Universe>,
    /// Messages for one player's connections (finished experiments), picked up by their stream.
    pending: Mutex<Vec<(u64, ServerMessage)>>,
    science: science::ScienceData,
    catalog: Vec<BodyInfo>,
    /// Body rotation, index-aligned with `catalog`.
    spins: Vec<ose_net::BodySpin>,
    parts: Vec<ose_data::PartDef>,
    resources: Vec<ose_data::ResourceDef>,
    accounts: Mutex<accounts::Accounts>,
    /// Vessel entity -> (vessel id, owner player id).
    vessels: Mutex<Vec<(bevy_ecs::entity::Entity, u64, Option<u64>)>>,
    campaign: Mutex<save::Campaign>,
    difficulty: ServerMessage,
    stock_crafts: Vec<ose_data::craft::Craft>,
    crafts: Mutex<Vec<save::SavedCraft>>,
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
    let requested = warp;
    let mut u = s.universe.lock().expect("sim lock");
    let warp = if u.has_powered() {
        warp.min(ose_sim::flight::MAX_PHYSICS_WARP)
    } else {
        warp
    };
    u.set_warp(warp);
    // only another player's lower request counts as "holding warp down"; the physics cap does not
    (warp, if max > requested { holder } else { None })
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
    let spins = defs
        .iter()
        .map(|b| ose_net::BodySpin {
            rotation_period_s: b.rotation_period_s,
            initial_rotation_deg: b.initial_rotation_deg,
        })
        .collect();
    tracing::info!(bodies = defs.len(), "loaded celestial bodies");

    let mut universe = Universe::with_bodies(&defs);
    let mut terrains = 0;
    for entry in &terrain_index().bodies {
        let Some(def) = defs.iter().find(|d| d.id == entry.id) else {
            continue;
        };
        // A terrain pack from the container's assets (finer map, tiled layers, decals) when there is one;
        // the embedded global-only map otherwise. The client reads the same pack from /assets/terrain/.
        let pack = args
            .assets_dir
            .as_ref()
            .and_then(|dir| load_terrain_pack(dir, &entry.id, def.radius_m));
        let map = pack.or_else(|| {
            ose_data::terrain::TerrainMap::from_entry(entry, EMBEDDED_TERRAIN, def.radius_m)
        });
        if let Some(map) = map {
            if universe.set_terrain(&entry.id, map) {
                terrains += 1;
            }
        }
    }
    tracing::info!(bodies = terrains, "terrain height maps loaded");
    let save_path = (args.save.to_string_lossy() != "none").then(|| args.save.clone());
    let mut registry: Vec<save::VesselEntry> = Vec::new();
    let mut campaign = save::Campaign::default();
    let mut crafts: Vec<save::SavedCraft> = Vec::new();
    if let Some(p) = &save_path {
        match save::read(p) {
            Ok(Some(file)) => {
                registry = save::restore(&mut universe, &file);
                campaign = file.campaign.clone();
                crafts = file.crafts.clone();
                tracing::info!(path = %p.display(), tick = file.tick.0, vessels = registry.len(), "save loaded");
            }
            Ok(None) => tracing::info!(path = %p.display(), "no save file, starting fresh"),
            Err(e) => {
                tracing::error!(path = %p.display(), error = %e, "save file unreadable; refusing to start");
                return Err(e);
            }
        }
    }
    universe.set_science(
        serde_json::from_str(EMBEDDED_SCIENCE)?,
        EMBEDDED_REGIONS.to_vec(),
    );
    let shared: AppState = Arc::new(Shared {
        universe: Mutex::new(universe),
        pending: Mutex::new(Vec::new()),
        science: science::ScienceData {
            tech: serde_json::from_str(EMBEDDED_TECH_TREE)?,
            missions: serde_json::from_str(EMBEDDED_MISSIONS)?,
        },
        catalog,
        spins,
        parts: serde_json::from_str(EMBEDDED_PARTS)?,
        resources: serde_json::from_str(EMBEDDED_RESOURCES)?,
        accounts: Mutex::new({
            let acc = accounts::Accounts::open(&args.db)?;
            tracing::info!(db = %args.db.display(), players = acc.count()?, "accounts opened");
            acc
        }),
        vessels: Mutex::new(registry),
        campaign: Mutex::new(campaign),
        difficulty: difficulty_message()?,
        stock_crafts: serde_json::from_str(EMBEDDED_STOCK_CRAFT)?,
        crafts: Mutex::new(crafts),
        names: Mutex::new(std::collections::HashMap::new()),
        warp_requests: Mutex::new(std::collections::HashMap::new()),
    });

    let sim = shared.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(
            1000 / ose_sim::TICKS_PER_SECOND,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ticks: u64 = 0;
        loop {
            interval.tick().await;
            sim.universe.lock().expect("sim lock").step();
            ticks += 1;
            // missions look at the agency's vessels once a second
            if ticks.is_multiple_of(ose_sim::TICKS_PER_SECOND) {
                science::evaluate_missions(&sim);
                science::deliver_experiment_events(&sim);
            }
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
                    let k = saver.crafts.lock().expect("crafts lock");
                    save::capture(&u, &v, &c, &k)
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

    if args.guest_ttl_s > 0 {
        let sweeper = shared.clone();
        let ttl = args.guest_ttl_s;
        let every = std::time::Duration::from_secs_f64(args.guest_sweep_s.max(0.2));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            interval.tick().await;
            loop {
                interval.tick().await;
                sweep_guests(&sweeper, ttl);
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
        .route(
            "/parts.json",
            get({
                let body = Arc::new(serde_json::to_string(&part_infos(&shared.parts))?);
                move || json_route(body.clone())
            }),
        )
        .route(
            "/resources.json",
            get({
                let body = Arc::new(serde_json::to_string(&shared.resources)?);
                move || json_route(body.clone())
            }),
        )
        .route(
            "/tech-tree.json",
            get({
                let body = Arc::new(serde_json::to_string(&shared.science.tech)?);
                move || json_route(body.clone())
            }),
        )
        .route(
            "/missions.json",
            get({
                let body = Arc::new(serde_json::to_string(&shared.science.missions)?);
                move || json_route(body.clone())
            }),
        )
        .with_state(shared);
    if let Some(dir) = &args.assets_dir {
        let assets = Router::new().fallback_service(ServeDir::new(dir)).layer(
            axum::middleware::map_response(|mut r: axum::response::Response| async move {
                if r.status().is_success() {
                    r.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("public, max-age=86400"),
                    );
                }
                r
            }),
        );
        app = app.nest_service("/assets", assets);
        tracing::info!(dir = %dir.display(), "serving game assets");
    }
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
                flight: flight_readout(u, e, body),
            })
        })
        .collect();
    ServerMessage::Vessels { tick, vessels }
}

fn to_net_attitude(a: Attitude) -> ose_net::AttitudeMode {
    use ose_net::AttitudeMode as M;
    match a {
        Attitude::Up => M::Up,
        Attitude::SurfacePrograde => M::SurfacePrograde,
        Attitude::SurfaceRetrograde => M::SurfaceRetrograde,
        Attitude::OrbitPrograde => M::OrbitPrograde,
        Attitude::OrbitRetrograde => M::OrbitRetrograde,
        Attitude::Normal => M::Normal,
        Attitude::AntiNormal => M::AntiNormal,
        Attitude::RadialOut => M::RadialOut,
        Attitude::RadialIn => M::RadialIn,
        Attitude::PitchHeading {
            pitch_deg,
            heading_deg,
        } => M::PitchHeading {
            pitch_deg,
            heading_deg,
        },
        Attitude::Fixed(d) => M::Fixed(d),
    }
}

fn from_net_attitude(a: ose_net::AttitudeMode) -> Attitude {
    use ose_net::AttitudeMode as M;
    match a {
        M::Up => Attitude::Up,
        M::SurfacePrograde => Attitude::SurfacePrograde,
        M::SurfaceRetrograde => Attitude::SurfaceRetrograde,
        M::OrbitPrograde => Attitude::OrbitPrograde,
        M::OrbitRetrograde => Attitude::OrbitRetrograde,
        M::Normal => Attitude::Normal,
        M::AntiNormal => Attitude::AntiNormal,
        M::RadialOut => Attitude::RadialOut,
        M::RadialIn => Attitude::RadialIn,
        M::PitchHeading {
            pitch_deg,
            heading_deg,
        } => Attitude::PitchHeading {
            pitch_deg,
            heading_deg,
        },
        M::Fixed(d) => Attitude::Fixed(d),
    }
}

fn flight_readout(
    u: &Universe,
    e: bevy_ecs::entity::Entity,
    body_e: bevy_ecs::entity::Entity,
) -> Option<ose_net::FlightReadout> {
    let p = u.powered(e)?;
    let body = u.world.get::<ose_sim::bodies::Body>(body_e)?;
    let atmo = u.atmosphere(body_e);
    let r_mag = p.r.length();
    let alt = r_mag - body.radius_m;
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let v_surf = p.v - ose_math::DVec3::new(0.0, 0.0, omega).cross(p.r);
    let up = p.r.normalized();
    let pressure_kpa = atmo.map(|a| a.pressure_kpa(alt)).unwrap_or(0.0);
    let rho = atmo.map(|a| a.density(alt)).unwrap_or(0.0);
    let thrust = p.thrust_kn(pressure_kpa / 101.325);
    let g = body.mu / (r_mag * r_mag);
    let max_thrust = p
        .current_stage()
        .map(|s| {
            s.thrust_vac_kn
                + (s.thrust_asl_kn - s.thrust_vac_kn) * (pressure_kpa / 101.325).clamp(0.0, 1.0)
        })
        .unwrap_or(0.0);
    let sit = match ose_sim::flight::situation(p, body, atmo) {
        Situation::Landed => ose_net::FlightSituation::Landed,
        Situation::Flying => ose_net::FlightSituation::Flying,
        Situation::SubOrbital => ose_net::FlightSituation::SubOrbital,
        Situation::Orbit => ose_net::FlightSituation::Orbit,
        Situation::Escaping => ose_net::FlightSituation::Escaping,
        Situation::Crashed => ose_net::FlightSituation::Crashed,
    };
    let stage_prop = p.current_stage().map(|s| s.propellant_t).unwrap_or(0.0);
    let ec = u
        .structure(e)
        .and_then(|st| {
            let r = st.net.resource("ElectricCharge")?;
            Some(
                st.net
                    .tanks
                    .iter()
                    .filter(|t| t.resource == r && st.net.present[t.part])
                    .fold((0.0, 0.0), |a, t| (a.0 + t.stored, a.1 + t.capacity)),
            )
        })
        .unwrap_or((0.0, 0.0));
    Some(ose_net::FlightReadout {
        situation: sit,
        altitude_m: alt,
        surface_speed_mps: v_surf.length(),
        vertical_speed_mps: v_surf.dot(up),
        throttle: p.throttle,
        attitude: to_net_attitude(p.attitude),
        stage: p.stage as u32,
        stages: p.spec.stages.len() as u32,
        propellant_t: p.propellant_t,
        // of what the burning engines' tanks hold when full (the staging report's stage share otherwise)
        propellant_fraction: match u.structure(e).map(|st| st.reachable_capacity_t(p.stage)) {
            Some(cap) if cap > 0.0 => (p.propellant_t / cap).clamp(0.0, 1.0),
            Some(_) => 0.0,
            None if stage_prop > 0.0 => p.propellant_t / stage_prop,
            None => 0.0,
        },
        mass_t: p.mass_t,
        thrust_kn: thrust,
        twr: if p.mass_t > 0.0 && g > 0.0 {
            max_thrust / (p.mass_t * g)
        } else {
            0.0
        },
        dynamic_pressure_kpa: 0.0005 * rho * v_surf.length_squared(),
        facing: p.facing,
        roll_ref: roll_reference(p.facing, up),
        electric_charge: ec.0,
        electric_capacity: ec.1,
        has_control: p.controlled,
        joint_stress: u
            .structure(e)
            .map(|st| st.rigid.max_stress())
            .unwrap_or(0.0),
        flex: u
            .structure(e)
            .map(|st| {
                st.rigid
                    .flex(1e-4, 2e-3)
                    .into_iter()
                    .map(|f| ose_net::PartFlex {
                        part: f.part as u16,
                        // the vessel frame is the craft frame with Z negated; a rotation vector is an
                        // axial vector, so it flips the other two components
                        rotation: [
                            -f.rotation.x as f32,
                            -f.rotation.y as f32,
                            f.rotation.z as f32,
                        ],
                        offset_m: [
                            f.offset_m.x as f32,
                            f.offset_m.y as f32,
                            -f.offset_m.z as f32,
                        ],
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Local north (east when the vessel axis is north-south) made perpendicular to `facing`.
fn roll_reference(facing: ose_math::DVec3, up: ose_math::DVec3) -> ose_math::DVec3 {
    let (_, east, north) = ose_sim::flight::surface_frame(up);
    for d in [north, east] {
        let across = d - facing * d.dot(facing);
        if across.length_squared() > 1e-6 {
            return across.normalized();
        }
    }
    up
}

/// Fallback launch pad (Kerbin) when the terrain index has no pad site, degrees.
const KSC_LAT_DEG: f64 = -0.6836;
const KSC_LON_DEG: f64 = -74.8198;

/// `<assets>/terrain/<Body>.json` + `.bin` (tools/extract_terrain.py --packs), if present and readable.
fn load_terrain_pack(
    assets: &std::path::Path,
    id: &str,
    radius_m: f64,
) -> Option<ose_data::terrain::TerrainMap> {
    let dir = assets.join("terrain");
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    let mut pack: ose_data::terrain::TerrainPack = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(body = id, error = %e, "terrain pack unreadable");
            return None;
        }
    };
    let blob = std::fs::read(dir.join(format!("{id}.bin"))).ok()?;
    pack.radius_m = radius_m;
    let map = ose_data::terrain::TerrainMap::from_pack(&pack, &blob);
    if map.is_some() {
        tracing::info!(
            body = id,
            bytes = blob.len(),
            decals = pack.decals.len(),
            "terrain pack loaded"
        );
    }
    map
}

fn terrain_index() -> ose_data::terrain::TerrainIndex {
    serde_json::from_str(EMBEDDED_TERRAIN_INDEX).expect("embedded terrain index")
}

/// Latitude and longitude of the launch pad site on the home world.
fn launch_pad(home: &str) -> (f64, f64) {
    terrain_index()
        .bodies
        .iter()
        .find(|b| b.id == home)
        .and_then(|b| ose_data::terrain::TerrainMap::site(b, LAUNCH_PAD_SITE))
        .map(|s| (s.lat_deg, s.lon_deg))
        .unwrap_or((KSC_LAT_DEG, KSC_LON_DEG))
}

/// Reduce a craft to a flight spec: per-stage thrust/Isp/propellant from the staging report, sea-level Isp
/// thrust-weighted over the stage engines, drag area as the largest part cross-section.
fn flight_spec_for(s: &Shared, craft: &ose_data::craft::Craft) -> ose_sim::flight::FlightSpec {
    let report = ose_data::staging::delta_v_by_stage(craft, &s.parts, &s.resources);
    let mut isp_asl = Vec::new();
    for st in &report.stages {
        let mut num = 0.0;
        let mut den = 0.0;
        for &pi in &st.engines {
            let Some(def) = s.parts.iter().find(|d| d.id == craft.parts[pi].part_id) else {
                continue;
            };
            for e in def.engines.iter().take(1) {
                num += e.max_thrust_kn * e.isp_curve.evaluate(1.0);
                den += e.max_thrust_kn;
            }
        }
        isp_asl.push(if den > 0.0 { num / den } else { 0.0 });
    }
    let mut area: f64 = 0.0;
    for p in &craft.parts {
        if let Some(def) = s.parts.iter().find(|d| d.id == p.part_id) {
            if let Some(b) = def.bounds {
                area = area.max(
                    b.size_m.x.max(b.size_m.z)
                        * b.size_m.x.min(b.size_m.z)
                        * std::f64::consts::FRAC_PI_4,
                );
            }
        }
    }
    let inertia: Vec<ose_sim::flight::StageInertia> = report
        .stages
        .iter()
        .map(|st| stage_inertia(s, craft, st))
        .collect();
    ose_sim::flight::FlightSpec::from_report(&report, &isp_asl, &inertia, area)
}

/// Transverse moment of inertia (largest of the two axes across the thrust axis, Unity Y up in craft space),
/// reaction wheel torque and gimbal torque for the parts attached during one stage. Each part is a point mass
/// at its craft-space position plus its own box inertia from its bounds; propellant is counted full.
fn stage_inertia(
    s: &Shared,
    craft: &ose_data::craft::Craft,
    st: &ose_data::staging::StageDeltaV,
) -> ose_sim::flight::StageInertia {
    let unit_mass = |res: &str| {
        s.resources
            .iter()
            .find(|r| r.id.eq_ignore_ascii_case(res))
            .and_then(|r| match r.kind {
                ose_data::ResourceKind::Basic {
                    mass_per_unit_t, ..
                } => Some(mass_per_unit_t),
                ose_data::ResourceKind::Recipe { .. } => None,
            })
            .unwrap_or(0.0)
    };
    let mut items = Vec::new(); // (mass t, position, size)
    let mut wheel = 0.0;
    for &i in &st.alive {
        let p = &craft.parts[i];
        let Some(def) = s.parts.iter().find(|d| d.id == p.part_id) else {
            continue;
        };
        let m = def.mass_t
            + p.resources
                .iter()
                .map(|r| r.stored_units * unit_mass(&r.resource))
                .sum::<f64>();
        let size = def
            .bounds
            .map(|b| b.size_m)
            .unwrap_or(ose_math::DVec3::new(1.0, 1.0, 1.0));
        items.push((m, p.local_position, size));
        wheel += def.reaction_wheel_torque_knm;
    }
    let total: f64 = items.iter().map(|x| x.0).sum();
    if total <= 0.0 {
        return ose_sim::flight::StageInertia::default();
    }
    let c = items
        .iter()
        .fold(ose_math::DVec3::ZERO, |acc, x| acc + x.1 * x.0)
        * (1.0 / total);
    let (mut ix, mut iz) = (0.0, 0.0);
    for (m, pos, size) in &items {
        let d = *pos - c;
        // about the X axis: y and z distances; about the Z axis: x and y distances
        ix += m * (d.y * d.y + d.z * d.z) + m * (size.y * size.y + size.z * size.z) / 12.0;
        iz += m * (d.x * d.x + d.y * d.y) + m * (size.x * size.x + size.y * size.y) / 12.0;
    }
    let mut gimbal = 0.0;
    for &ei in &st.engines {
        let p = &craft.parts[ei];
        let Some(def) = s.parts.iter().find(|d| d.id == p.part_id) else {
            continue;
        };
        if let Some(e) = def.engines.first() {
            let range = e.gimbal_range_deg.unwrap_or(0.0).to_radians();
            let arm = (p.local_position.y - c.y).abs();
            gimbal += e.max_thrust_kn * range.sin() * arm;
        }
    }
    ose_sim::flight::StageInertia {
        moi_tm2: ix.max(iz),
        wheel_torque_knm: wheel,
        gimbal_torque_knm: gimbal,
    }
}

/// Put a craft on the KSC pad as a landed, powered vessel.
fn launch_on_pad(s: &Shared, player_id: u64, craft: &ose_data::craft::Craft) -> ServerMessage {
    let mut filled = craft.clone();
    fill_default_resources(s, &mut filled);
    let craft = &filled;
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
    let fail = |reason: String| ServerMessage::CraftLaunched {
        vessel: None,
        name: name.clone(),
        parts,
        wet_mass_t: m.wet_t(),
        dry_mass_t: m.dry_t,
        delta_v_vacuum_mps: report.total_vacuum_mps,
        stages: stages.clone(),
        reason: Some(reason),
    };
    if craft.parts.is_empty() || !craft.is_tree() {
        return fail("craft must be a non-empty single tree".into());
    }
    let locked = {
        let c = s.campaign.lock().expect("campaign lock");
        // the tech tree binds the campaign's members; guests and visitors fly anything
        if c.members.iter().any(|m| m.player_id == player_id) {
            s.science.locked_parts(&c, craft)
        } else {
            Vec::new()
        }
    };
    if !locked.is_empty() {
        return fail(format!("not researched yet: {}", locked.join(", ")));
    }
    if !m.unknown_parts.is_empty() {
        return fail(format!("unknown parts: {:?}", m.unknown_parts));
    }
    let spec = flight_spec_for(s, craft);
    if spec.stages.is_empty() {
        return fail("craft has no stages".into());
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
    let (pad_lat, pad_lon) = launch_pad(&home.id);
    let e = u.spawn_powered_on_surface(&name, body, spec, pad_lat, pad_lon);
    u.set_structure(
        e,
        ose_sim::structure::Structure::build(craft, &s.parts, &s.resources),
    );
    let mut list = s.vessels.lock().expect("vessels lock");
    let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
    list.push((e, id, Some(player_id)));
    tracing::info!(player = player_id, vessel = id, craft = %name, parts, "craft on the pad");
    ServerMessage::CraftLaunched {
        vessel: Some(id),
        name,
        parts,
        wet_mass_t: m.wet_t(),
        dry_mass_t: m.dry_t,
        delta_v_vacuum_mps: report.total_vacuum_mps,
        stages,
        reason: None,
    }
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
/// Science requests: run experiments, transmit, recover, state, research. Replies in order.
fn science_request(s: &Shared, player_id: u64, msg: ClientMessage) -> Vec<ServerMessage> {
    let state = |s: &Shared| s.science.state(&s.campaign.lock().expect("campaign lock"));
    match msg {
        ClientMessage::RunExperiments { vessel } => {
            let Some(e) = owned_vessel(s, vessel, player_id) else {
                return vec![ServerMessage::ScienceReports {
                    vessel,
                    new: vec![],
                    skipped: vec![],
                    carried: vec![],
                    reason: Some("not your vessel".into()),
                    running: vec![],
                }];
            };
            let mut u = s.universe.lock().expect("sim lock");
            let located = u.science_location(e).is_some();
            let out = u.run_experiments(e);
            vec![ServerMessage::ScienceReports {
                vessel,
                new: out.reports.iter().map(science::report_info).collect(),
                skipped: out
                    .skipped
                    .iter()
                    .map(|k| ose_net::SkippedExperiment {
                        part: k.part as u16,
                        experiment: k.experiment.clone(),
                        reason: k.reason.clone(),
                    })
                    .collect(),
                carried: u
                    .science_carried(e)
                    .iter()
                    .map(science::report_info)
                    .collect(),
                reason: (!located).then(|| "no science here".to_string()),
                running: u
                    .running_experiments(e)
                    .iter()
                    .map(|r| ose_net::RunningExperiment {
                        part: r.part as u16,
                        experiment: r.experiment.clone(),
                        remaining_s: r.remaining_s,
                    })
                    .collect(),
            }]
        }
        ClientMessage::TransmitScience { vessel } | ClientMessage::RecoverVessel { vessel } => {
            let recover = matches!(msg, ClientMessage::RecoverVessel { .. });
            let fail = |reason: &str| {
                vec![ServerMessage::ScienceSubmitted {
                    vessel,
                    reports: vec![],
                    earned: 0.0,
                    recovered: false,
                    reason: Some(reason.to_string()),
                }]
            };
            let Some(e) = owned_vessel(s, vessel, player_id) else {
                return fail("not your vessel");
            };
            let (reports, tick) = {
                let mut u = s.universe.lock().expect("sim lock");
                if recover {
                    let home = s
                        .catalog
                        .iter()
                        .find(|b| b.id == "Kerbin")
                        .map(|b| b.id.clone())
                        .unwrap_or_default();
                    let on_home = u
                        .vessel(e)
                        .and_then(|(_, body, _)| u.body_id(body).map(|b| b == home))
                        .unwrap_or(false);
                    let landed = u.powered(e).is_some_and(|p| p.landed && !p.crashed);
                    if !(on_home && landed) {
                        return fail("only a vessel standing on Kerbin can be recovered");
                    }
                }
                let r = u.take_science(e);
                if recover {
                    u.despawn_vessel(e);
                }
                (r, u.tick().0)
            };
            if recover {
                s.vessels
                    .lock()
                    .expect("vessels lock")
                    .retain(|&(v, _, _)| v != e);
            }
            let (reports, earned) = {
                let mut c = s.campaign.lock().expect("campaign lock");
                science::submit(&mut c, reports, player_id, tick)
            };
            tracing::info!(
                player = player_id,
                vessel,
                earned,
                recover,
                "science submitted"
            );
            vec![
                ServerMessage::ScienceSubmitted {
                    vessel,
                    reports,
                    earned,
                    recovered: recover,
                    reason: None,
                },
                state(s),
            ]
        }
        ClientMessage::ScienceInfo => vec![state(s)],
        ClientMessage::UnlockTechNode { id } => {
            let res = {
                let mut c = s.campaign.lock().expect("campaign lock");
                s.science.unlock(&mut c, &id)
            };
            if res.is_ok() {
                tracing::info!(player = player_id, node = %id, "tech node unlocked");
            }
            vec![
                ServerMessage::TechUnlock {
                    id,
                    accepted: res.is_ok(),
                    reason: res.err(),
                },
                state(s),
            ]
        }
        _ => Vec::new(),
    }
}

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

/// Remove players who never joined the campaign and have been away for `ttl_s`, with their vessels and
/// saved crafts. Connected players are always kept.
fn sweep_guests(s: &Shared, ttl_s: u64) {
    let members: Vec<u64> = s
        .campaign
        .lock()
        .expect("campaign lock")
        .members
        .iter()
        .map(|m| m.player_id)
        .collect();
    let connected: Vec<u64> = s
        .warp_requests
        .lock()
        .expect("warp lock")
        .keys()
        .copied()
        .collect();
    let mut keep = members;
    keep.extend(connected);
    let stale = s
        .accounts
        .lock()
        .expect("accounts lock")
        .stale(ttl_s, &keep);
    if stale.is_empty() {
        return;
    }
    let mut u = s.universe.lock().expect("sim lock");
    let mut vessels = s.vessels.lock().expect("vessels lock");
    let mut removed_vessels = 0;
    vessels.retain(|&(e, _, owner)| {
        let gone = owner.is_some_and(|o| stale.contains(&o));
        if gone {
            u.despawn_vessel(e);
            removed_vessels += 1;
        }
        !gone
    });
    drop(vessels);
    drop(u);
    s.crafts
        .lock()
        .expect("crafts lock")
        .retain(|c| !stale.contains(&c.owner));
    let mut acc = s.accounts.lock().expect("accounts lock");
    for id in &stale {
        let _ = acc.remove(*id);
    }
    tracing::info!(
        players = stale.len(),
        vessels = removed_vessels,
        "swept stale guests"
    );
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
                agency: None,
            });
            tracing::info!(player = player_id, "joined the campaign");
        }
        c.last_played_unix = now_unix();
    }
    let vessels = s.vessels.lock().expect("vessels lock").len() as u32;
    ServerMessage::CampaignState {
        id: SHARED_SESSION_ID.into(),
        name: c.name.clone(),
        mode: c.mode,
        difficulty: c.difficulty.clone(),
        started_tick: c.started_tick,
        members: c
            .members
            .iter()
            .map(|m| ose_net::CampaignMember {
                player_id: m.player_id,
                display_name: m.display_name.clone(),
                joined_tick: m.joined_tick,
                agency: m.agency.clone(),
            })
            .collect(),
        you_are_member: c.members.iter().any(|m| m.player_id == player_id),
        vessels,
        created: false,
    }
}

fn session_list(s: &Shared) -> ServerMessage {
    let ut_s = s.universe.lock().expect("sim lock").ut();
    let c = s.campaign.lock().expect("campaign lock");
    ServerMessage::SessionList {
        sessions: vec![ose_net::SessionInfo {
            id: SHARED_SESSION_ID.into(),
            name: c.name.clone(),
            mode: c.mode,
            ut_s,
            last_played_unix: c.last_played_unix,
            players: c.members.len() as u32,
        }],
    }
}

fn craft_summary(
    s: &Shared,
    id: u64,
    owner: Option<u64>,
    craft: &ose_data::craft::Craft,
) -> ose_net::CraftSummary {
    let m = craft.mass(&s.parts, &s.resources);
    let report = ose_data::staging::delta_v_by_stage(craft, &s.parts, &s.resources);
    ose_net::CraftSummary {
        id,
        name: craft.name.clone(),
        description: craft.description.clone(),
        parts: craft.parts.len() as u32,
        stages: craft.stages.len() as u32,
        dry_mass_t: m.dry_t,
        wet_mass_t: m.wet_t(),
        delta_v_vacuum_mps: report.total_vacuum_mps,
        owner,
        stock: owner.is_none(),
    }
}

/// Stock crafts (ids 1..) plus the player's own saved crafts.
fn craft_list(s: &Shared, player_id: u64) -> ServerMessage {
    let mut crafts: Vec<ose_net::CraftSummary> = s
        .stock_crafts
        .iter()
        .enumerate()
        .map(|(i, c)| craft_summary(s, i as u64 + 1, None, c))
        .collect();
    let mine = s.crafts.lock().expect("crafts lock");
    crafts.extend(
        mine.iter()
            .filter(|c| c.owner == player_id)
            .map(|c| craft_summary(s, c.id, Some(c.owner), &c.craft)),
    );
    ServerMessage::CraftList { crafts }
}

fn find_craft(s: &Shared, id: u64, player_id: u64) -> Option<ose_data::craft::Craft> {
    if (1..FIRST_SAVED_CRAFT_ID).contains(&id) {
        return s.stock_crafts.get(id as usize - 1).cloned();
    }
    s.crafts
        .lock()
        .expect("crafts lock")
        .iter()
        .find(|c| c.id == id && c.owner == player_id)
        .map(|c| c.craft.clone())
}

/// Parts that list no resources get their definition containers at `initial_units` (the editor default of
/// full tanks); parts that list resources are left as sent.
fn fill_default_resources(s: &Shared, craft: &mut ose_data::craft::Craft) {
    for p in &mut craft.parts {
        if !p.resources.is_empty() {
            continue;
        }
        if let Some(def) = s.parts.iter().find(|d| d.id == p.part_id) {
            p.resources = def
                .resource_containers
                .iter()
                .map(|c| ose_data::craft::StoredResource {
                    resource: c.resource.clone(),
                    stored_units: c.initial_units,
                    capacity_units: c.capacity_units,
                })
                .collect();
        }
    }
}

fn save_craft(s: &Shared, player_id: u64, mut craft: ose_data::craft::Craft) -> ServerMessage {
    fill_default_resources(s, &mut craft);
    if craft.parts.is_empty() || !craft.is_tree() {
        return ServerMessage::CraftSaved {
            id: None,
            summary: None,
            reason: Some("craft must be a non-empty single tree".into()),
        };
    }
    let m = craft.mass(&s.parts, &s.resources);
    if !m.unknown_parts.is_empty() {
        return ServerMessage::CraftSaved {
            id: None,
            summary: None,
            reason: Some(format!("unknown parts: {:?}", m.unknown_parts)),
        };
    }
    let mut list = s.crafts.lock().expect("crafts lock");
    let id = list
        .iter()
        .map(|c| c.id)
        .max()
        .map(|m| m + 1)
        .unwrap_or(FIRST_SAVED_CRAFT_ID);
    let summary = craft_summary(s, id, Some(player_id), &craft);
    list.push(save::SavedCraft {
        id,
        owner: player_id,
        craft,
    });
    tracing::info!(player = player_id, craft = id, "craft saved");
    ServerMessage::CraftSaved {
        id: Some(id),
        summary: Some(summary),
        reason: None,
    }
}

/// Validate an uploaded craft against the part catalog and spawn it as a vessel in a circular orbit.
fn launch_craft(
    s: &Shared,
    player_id: u64,
    craft: &ose_data::craft::Craft,
    altitude_m: f64,
) -> ServerMessage {
    let mut filled = craft.clone();
    fill_default_resources(s, &mut filled);
    let craft = &filled;
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
    let spec = flight_spec_for(s, craft);
    if !spec.stages.is_empty() {
        if let Some((_, _, orbit)) = u.vessel(e) {
            let (r, v) = orbit.state_at(u.ut());
            u.set_dormant(e, ose_sim::flight::Powered::new(spec, r, v, false));
            u.set_structure(
                e,
                ose_sim::structure::Structure::build(craft, &s.parts, &s.resources),
            );
        }
    }
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
                        if !send(
                            &mut socket,
                            &ServerMessage::BodyRotation {
                                bodies: s.spins.clone(),
                            },
                        )
                        .await
                        {
                            return;
                        }
                        if !has_vessel(&s, id) {
                            if !send(&mut socket, &s.difficulty).await {
                                return;
                            }
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
    let mut science_rev = s.campaign.lock().expect("campaign lock").science.rev;
    let mut traj_interval = tokio::time::interval(std::time::Duration::from_millis(
        1000 / TRAJECTORY_HZ as u64,
    ));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                {
                    let mut u = s.universe.lock().expect("sim lock");
                    let mut list = s.vessels.lock().expect("vessels lock");
                    // pieces that broke off belong to whoever flew the vessel
                    for (from, debris) in u.take_broken_off() {
                        let owner = list.iter().find(|v| v.0 == from).and_then(|v| v.2);
                        let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
                        list.push((debris, id, owner));
                    }
                    list.retain(|&(e, _, _)| u.vessel(e).is_some());
                }
                let (time, snap, vessels) = snapshot(&s);
                if !send(&mut socket, &time).await
                    || !send(&mut socket, &snap).await
                    || !send(&mut socket, &vessels).await
                {
                    break;
                }
                let mine: Vec<ServerMessage> = {
                    let mut q = s.pending.lock().expect("pending lock");
                    let (m, rest): (Vec<_>, Vec<_>) = q.drain(..).partition(|(p, _)| *p == player_id);
                    *q = rest;
                    m.into_iter().map(|(_, msg)| msg).collect()
                };
                let mut ok = true;
                for m in &mine {
                    ok = ok && send(&mut socket, m).await;
                }
                if !ok {
                    break;
                }
                let changed = {
                    let c = s.campaign.lock().expect("campaign lock");
                    (c.science.rev != science_rev).then(|| (c.science.rev, s.science.state(&c)))
                };
                if let Some((rev, state)) = changed {
                    science_rev = rev;
                    if !send(&mut socket, &state).await {
                        break;
                    }
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
                        Ok(ClientMessage::ListSessions) => {
                            let reply = session_list(&s);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SelectSession { id }) => {
                            let reply = if id == SHARED_SESSION_ID {
                                let name = display_name_for(&s, player_id);
                                campaign_state(&s, player_id, Some(&name))
                            } else {
                                ServerMessage::Rejected { reason: format!("no session {id}") }
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::CreateSession { name, mode, difficulty }) => {
                            let created = {
                                let mut c = s.campaign.lock().expect("campaign lock");
                                if c.members.is_empty() {
                                    c.name = if name.trim().is_empty() { c.name.clone() } else { name.trim().to_string() };
                                    c.mode = mode;
                                    c.difficulty = difficulty;
                                    c.started_tick = s.universe.lock().expect("sim lock").tick();
                                    true
                                } else {
                                    false
                                }
                            };
                            let pname = display_name_for(&s, player_id);
                            let mut reply = campaign_state(&s, player_id, Some(&pname));
                            if let ServerMessage::CampaignState { created: c, .. } = &mut reply {
                                *c = created;
                            }
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetAgency { agency }) => {
                            {
                                let mut c = s.campaign.lock().expect("campaign lock");
                                if let Some(m) = c.members.iter_mut().find(|m| m.player_id == player_id) {
                                    m.agency = Some(agency.clone());
                                } else {
                                    let tick = s.universe.lock().expect("sim lock").tick();
                                    c.members.push(save::CampaignMember {
                                        player_id,
                                        display_name: display_name_for(&s, player_id),
                                        joined_tick: tick,
                                        agency: Some(agency.clone()),
                                    });
                                }
                            }
                            let reply = campaign_state(&s, player_id, None);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LaunchCraft { craft, altitude_m }) => {
                            let reply = launch_craft(&s, player_id, &craft, altitude_m);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SaveCraft { craft }) => {
                            let reply = save_craft(&s, player_id, craft);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::DeleteCraft { id }) => {
                            s.crafts.lock().expect("crafts lock").retain(|c| !(c.id == id && c.owner == player_id));
                            let reply = craft_list(&s, player_id);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::CraftList) => {
                            let reply = craft_list(&s, player_id);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::GetCraft { id }) => {
                            let craft = find_craft(&s, id, player_id);
                            if !send(&mut socket, &ServerMessage::CraftDocument { id, craft }).await { break; }
                        }
                        Ok(ClientMessage::LaunchOnPad { craft }) => {
                            let reply = launch_on_pad(&s, player_id, &craft);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LaunchSavedOnPad { id }) => {
                            let reply = match find_craft(&s, id, player_id) {
                                Some(c) => launch_on_pad(&s, player_id, &c),
                                None => ServerMessage::CraftLaunched {
                                    vessel: None,
                                    name: String::new(),
                                    parts: 0,
                                    wet_mass_t: 0.0,
                                    dry_mass_t: 0.0,
                                    delta_v_vacuum_mps: 0.0,
                                    stages: Vec::new(),
                                    reason: Some(format!("no craft {id} in your library")),
                                },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetControls { vessel, throttle, attitude }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let ok = s.universe.lock().expect("sim lock").set_controls(e, throttle, from_net_attitude(attitude));
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "vessel has no flight state (bare probe), has crashed, or has no power for its probe core".to_string()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::Stage { vessel }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let res = s.universe.lock().expect("sim lock").stage(e);
                                    if let Some(d) = res.debris {
                                        let mut list = s.vessels.lock().expect("vessels lock");
                                        let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
                                        list.push((d, id, Some(player_id)));
                                    }
                                    let ok = res.fired;
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "no further stage".to_string()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LaunchSavedCraft { id, altitude_m }) => {
                            let reply = match find_craft(&s, id, player_id) {
                                Some(c) => launch_craft(&s, player_id, &c, altitude_m),
                                None => ServerMessage::CraftLaunched {
                                    vessel: None,
                                    name: String::new(),
                                    parts: 0,
                                    wet_mass_t: 0.0,
                                    dry_mass_t: 0.0,
                                    delta_v_vacuum_mps: 0.0,
                                    stages: Vec::new(),
                                    reason: Some(format!("no craft {id} in your library")),
                                },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(msg @ (ClientMessage::RunExperiments { .. }
                        | ClientMessage::TransmitScience { .. }
                        | ClientMessage::RecoverVessel { .. }
                        | ClientMessage::ScienceInfo
                        | ClientMessage::UnlockTechNode { .. })) => {
                            let replies = science_request(&s, player_id, msg);
                            science_rev = s.campaign.lock().expect("campaign lock").science.rev;
                            let mut ok = true;
                            for r in &replies {
                                ok = ok && send(&mut socket, r).await;
                            }
                            if !ok { break; }
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
    let _ = s.accounts.lock().expect("accounts lock").touch(player_id);
    tracing::info!(player = player_id, "player left");
}

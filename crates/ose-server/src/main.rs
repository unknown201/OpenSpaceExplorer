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
mod campaigns;
mod crew_docking;
mod save;
mod science;
mod streaming;

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
    VesselState, PROTOCOL_VERSION, WS_PATH,
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
const EMBEDDED_KERBALS: &str = include_str!("../../../assets/own/kerbals.json");
/// How many Kerbals a new campaign starts with (the veterans and the next four).
const STARTING_CREW: usize = 8;
const EMBEDDED_SCIENCE: &str = include_str!("../../../assets/own/science/index.json");
const EMBEDDED_REGIONS: &[u8] = include_bytes!("../../../assets/own/science/regions.bin");
const EMBEDDED_TECH_TREE: &str = include_str!("../../../assets/own/tech-tree.json");
const EMBEDDED_MISSIONS: &str = include_str!("../../../assets/own/missions.json");
const EMBEDDED_TERRAIN_INDEX: &str = include_str!("../../../assets/own/terrain/index.json");
const EMBEDDED_TERRAIN: &[u8] = include_bytes!("../../../assets/own/terrain/terrain.bin");
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
    streams: streaming::Streams,
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
    /// What a new campaign's universe is built from, the campaigns waiting to be resumed, and the players
    /// playing the live one (joined and not left).
    setup: campaigns::WorldSetup,
    shelf: Mutex<Vec<save::ShelvedCampaign>>,
    playing: Mutex<std::collections::HashSet<u64>>,
    /// The last pad launch, for revert.
    revert: Mutex<Option<campaigns::RevertPoint>>,
    /// The original's Kerbals (veterans first), and the roster last pushed to the players.
    kerbals: Vec<ose_data::KerbalDef>,
    last_roster: Mutex<Option<ServerMessage>>,
    /// The vessel each player last commanded (`CampaignMember::flying`).
    flying: Mutex<std::collections::HashMap<u64, u64>>,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();
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

    let mut setup = campaigns::WorldSetup {
        bodies: defs.clone(),
        ..Default::default()
    };
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
    setup.terrains = universe.terrain_maps();
    // the KSC is the CommNet source, and where a campaign's first morning is
    let site = terrain_index()
        .bodies
        .iter()
        .find(|b| b.id == "Kerbin")
        .and_then(|b| ose_data::terrain::TerrainMap::site(b, "kerbin_KSC_Object").cloned());
    setup.ksc = site
        .map(|s| (s.lat_deg, s.lon_deg, s.altitude_m))
        .unwrap_or((KSC_LAT_DEG, KSC_LON_DEG, 100.0));
    let part_defs: Vec<ose_data::PartDef> = serde_json::from_str(EMBEDDED_PARTS)?;
    if let Some(k) = part_defs.iter().find(|d| d.id == "eva_kerbal") {
        setup.kerbal_experiments = k.experiments.clone();
    }
    setup.science = serde_json::from_str(EMBEDDED_SCIENCE)?;
    setup.regions = EMBEDDED_REGIONS.to_vec();
    // the first campaign starts on a morning at the KSC unless a save says otherwise
    let mut universe = setup.new_campaign_universe();
    let mut shelf: Vec<save::ShelvedCampaign> = Vec::new();
    let save_path = (args.save.to_string_lossy() != "none").then(|| args.save.clone());
    let mut registry: Vec<save::VesselEntry> = Vec::new();
    let mut campaign = save::Campaign::default();
    let mut crafts: Vec<save::SavedCraft> = Vec::new();
    if let Some(p) = &save_path {
        match save::read(p) {
            Ok(Some(file)) => {
                let defs: Vec<ose_data::PartDef> = serde_json::from_str(EMBEDDED_PARTS)?;
                let res: Vec<ose_data::ResourceDef> = serde_json::from_str(EMBEDDED_RESOURCES)?;
                universe = setup.universe();
                registry = save::restore(&mut universe, &file, &defs, &res);
                campaign = file.campaign.clone();
                crafts = file.crafts.clone();
                shelf = file.shelf.clone();
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
        streams: streaming::Streams::default(),
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
        setup,
        shelf: Mutex::new(shelf),
        playing: Mutex::new(std::collections::HashSet::new()),
        revert: Mutex::new(None),
        kerbals: serde_json::from_str(EMBEDDED_KERBALS)?,
        last_roster: Mutex::new(None),
        flying: Mutex::new(std::collections::HashMap::new()),
    });

    streaming::start(shared.clone());
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
            // docking ports latch ten times a second
            if ticks.is_multiple_of(5) {
                let msgs = crew_docking::dock_latched(&sim);
                if !msgs.is_empty() {
                    sim.pending.lock().expect("pending lock").extend(msgs);
                }
            }
            // missions look at the agency's vessels once a second
            if ticks.is_multiple_of(ose_sim::TICKS_PER_SECOND) {
                push_roster(&sim);
                science::evaluate_missions(&sim);
                science::deliver_experiment_events(&sim);
                let (required, heat) = {
                    let c = sim.campaign.lock().expect("campaign lock");
                    let get = |id: &str| c.difficulty.iter().find(|d| d.id == id).map(|d| &d.value);
                    let on = |id: &str, default: bool| match get(id) {
                        Some(ose_net::DifficultyValue::Bool(b)) => *b,
                        _ => default,
                    };
                    let heat = ose_sim::thermal::HeatSettings {
                        enabled: on("HeatEnabled", true),
                        scale: match get("HeatScaling") {
                            Some(ose_net::DifficultyValue::Number(x)) => *x,
                            _ => 1.0,
                        },
                        part_destruction: on("HeatPartDestruction", true),
                    };
                    (on("CommNetRequired", false), heat)
                };
                let mut u = sim.universe.lock().expect("sim lock");
                u.set_commnet_required(required);
                u.set_heat_settings(heat);
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
                    let mut file = save::capture(&u, &v, &c, &k);
                    file.shelf = saver.shelf.lock().expect("shelf lock").clone();
                    file
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
        // Precompressed `.gz` copies (terrain packs, tools/extract_terrain.py) go out to browsers that accept gzip.
        let assets = Router::new()
            .fallback_service(ServeDir::new(dir).precompressed_gzip())
            .layer(axum::middleware::map_response(
                |mut r: axum::response::Response| async move {
                    if r.status().is_success() {
                        r.headers_mut().insert(
                            axum::http::header::CACHE_CONTROL,
                            axum::http::HeaderValue::from_static("public, max-age=86400"),
                        );
                    }
                    r
                },
            ));
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
        Ok(bytes) => streaming::send_bytes(socket, bytes.into()).await,
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
                flight: flight_readout(u, e, body, &s.parts).map(|mut f| {
                    // the staging list from the vessel as it is now, here
                    if let (Some(craft), Some(p)) = (u.current_craft(e), u.powered(e)) {
                        let b = u.world.get::<ose_sim::bodies::Body>(body);
                        let r = p.r.length();
                        let g = b.map(|b| b.mu / (r * r)).unwrap_or(0.0);
                        let atm = u
                            .atmosphere(body)
                            .zip(b)
                            .map(|(a, b)| a.pressure_kpa(r - b.radius_m) / 101.325)
                            .unwrap_or(0.0);
                        let report =
                            ose_data::staging::delta_v_by_stage(&craft, &s.parts, &s.resources);
                        f.stage_list = stage_readouts_at(s, &craft, &report, Some((atm, g)));
                        f.delta_v_now_mps = f.stage_list.iter().map(|x| x.delta_v_now_mps).sum();
                    }
                    f
                }),
                // the count of parts still on: it changes when a stage drops or a part breaks or docks
                craft_rev: u.craft_rev(e),
                target: u.target(e).and_then(|t| {
                    let id = match list.iter().find(|x| x.0 == t) {
                        Some(&(_, vid, _)) => ose_net::TargetId::Vessel(vid),
                        None if u.body_id(t).is_some() => {
                            ose_net::TargetId::Body(catalog_index(s, u, t))
                        }
                        None => return None,
                    };
                    let (d, rel, dist) = u.target_relative(e)?;
                    let closing = if d.length() > 0.0 {
                        -rel.dot(d.normalized())
                    } else {
                        0.0
                    };
                    Some(ose_net::TargetReadout {
                        target: id,
                        distance_m: dist,
                        relative_velocity_mps: rel,
                        closing_mps: closing,
                    })
                }),
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
        Attitude::Maneuver(_) => M::Maneuver,
        Attitude::Free => M::Free,
        Attitude::Target(_) => M::Target,
        Attitude::AntiTarget(_) => M::AntiTarget,
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
        // the universe points it at the burn on its next step
        M::Maneuver => Attitude::Maneuver(ose_math::DVec3::ZERO),
        M::Free => Attitude::Free,
        M::Target => Attitude::Target(ose_math::DVec3::ZERO),
        M::AntiTarget => Attitude::AntiTarget(ose_math::DVec3::ZERO),
    }
}

fn flight_readout(
    u: &Universe,
    e: bevy_ecs::entity::Entity,
    body_e: bevy_ecs::entity::Entity,
    defs: &[ose_data::PartDef],
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
        static_pressure_kpa: pressure_kpa,
        firing_engines: if thrust > 0.0 {
            u.structure(e)
                .map(|st| {
                    st.fed_engines(p.stage)
                        .into_iter()
                        .filter_map(|i| u16::try_from(i).ok())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        },
        facing: p.facing,
        // a craft standing on the ground keeps its own heading (rovers turn); a plane's +Z (its belly)
        // points down
        roll_ref: if p.landed && !p.tipped && p.level {
            -up
        } else if p.landed && !p.tipped {
            p.drive.roll_ref(p.r)
        } else {
            // rolled about the axis from the horizon reference
            ose_sim::flight::vessel_top(p)
        },
        rcs_thrust_kn: p.rcs.thrust_kn as f32,
        rcs_firing: p.rcs.usage as f32,
        lift_kn: p.lift_kn.length() as f32,
        angvel: p.angvel,
        resources: u
            .structure(e)
            .map(|st| st.resource_totals())
            .unwrap_or_default(),
        stage_resources: u
            .structure(e)
            .map(|st| st.stage_resource_totals(p.stage))
            .unwrap_or_default(),
        fairings: u
            .structure(e)
            .map(|st| {
                (0..st.fairing_deployed.len())
                    .filter(|&i| st.fairing_payload[i].is_some() && st.rigid.present[i])
                    .map(|i| (i as u16, st.fairing_deployed[i]))
                    .collect()
            })
            .unwrap_or_default(),
        cargo_bays: u
            .structure(e)
            .map(|st| {
                (0..st.bay_open.len())
                    .filter(|&i| st.cargo_bay[i] && st.rigid.present[i])
                    .map(|i| (i as u16, st.bay_open[i]))
                    .collect()
            })
            .unwrap_or_default(),
        // filled in by `vessels_message`, which knows the craft
        stage_list: Vec::new(),
        delta_v_now_mps: 0.0,
        aero_forces: if p.aero_overlay {
            u.structure(e)
                .map(|st| {
                    st.part_aero_kn
                        .iter()
                        .enumerate()
                        .filter(|(i, (l, d))| {
                            st.rigid.present[*i] && (l.length() > 1e-3 || d.length() > 1e-3)
                        })
                        .map(|(i, (l, d))| (i as u16, *l, *d))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        },
        control_surfaces: u
            .structure(e)
            .map(|st| {
                st.surface_deflections(p.facing, p.steer_cmd)
                    .into_iter()
                    .map(|(i, d)| (i as u16, d as f32))
                    .collect()
            })
            .unwrap_or_default(),
        shielded: u
            .structure(e)
            .map(|st| {
                (0..st.shielded.len())
                    .filter(|&i| st.shielded[i] && st.rigid.present[i])
                    .map(|i| i as u16)
                    .collect()
            })
            .unwrap_or_default(),
        jet: p.jet.is_some(),
        lights: p.groups.lights,
        rcs: p.groups.rcs,
        abort: p.groups.abort,
        custom_groups: p.groups.custom,
        drive_speed_mps: p.drive.speed_mps as f32,
        brakes: p.drive.brakes,
        converters: u
            .structure(e)
            .map(|st| {
                (0..st.converter_on.len())
                    .filter(|&i| st.converter_defs[i].is_some() && st.rigid.present[i])
                    .map(|i| (i as u16, st.converter_on[i], st.converter_running[i] as f32))
                    .collect()
            })
            .unwrap_or_default(),
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
        crew: u.crew(e).into_iter().map(|(i, n)| (i as u16, n)).collect(),
        eva: u.eva_kerbal(e).is_some(),
        docking_ports: u
            .current_craft(e)
            .map(|c| {
                ose_sim::docking::free_ports(&c, defs)
                    .into_iter()
                    .map(|p| p.part as u16)
                    .collect()
            })
            .unwrap_or_default(),
        docked: u
            .docked_links(e)
            .into_iter()
            .map(|(a, b)| (a as u16, b as u16))
            .collect(),
        comm_connected: u.comm_link(e).connected,
        comm_hops: u.comm_link(e).hops,
        comm_range_m: u.comm_link(e).range_m,
        control_part: u.control_part(e).map(|i| i as u16),
        panels_deployed: u.panels_deployed(e),
        antennas: u
            .structure(e)
            .map(|st| {
                (0..st.antenna_deployable.len())
                    .filter(|&i| st.antenna_deployable[i] && st.rigid.present[i])
                    .map(|i| (i as u16, st.deployed[i]))
                    .collect()
            })
            .unwrap_or_default(),
        panels: u
            .structure(e)
            .map(|st| {
                st.electric
                    .iter()
                    .enumerate()
                    .filter(|(i, d)| {
                        d.solar_deployable && d.solar_output > 0.0 && st.rigid.present[*i]
                    })
                    .map(|(i, _)| (i as u16, st.deployed[i]))
                    .collect()
            })
            .unwrap_or_default(),
        tipped: p.tipped,
        parachutes: u
            .structure(e)
            .map(|st| {
                (0..st.chutes.len())
                    .filter(|&i| st.chute_defs[i].is_some() && st.rigid.present[i])
                    .map(|i| {
                        use ose_sim::structure::Chute;
                        let state = match st.chutes[i] {
                            Chute::Stowed => ose_net::ChuteState::Stowed,
                            Chute::Armed => ose_net::ChuteState::Armed,
                            Chute::SemiDeployed => ose_net::ChuteState::SemiDeployed,
                            Chute::Deployed => ose_net::ChuteState::Deployed,
                            Chute::Cut => ose_net::ChuteState::Cut,
                        };
                        (i as u16, state, st.chute_open[i] as f32)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        heat: u
            .structure(e)
            .map(|st| {
                (0..st.temperature_k.len())
                    .filter(|&i| {
                        st.rigid.present[i]
                            && (st.reentry_flux_kw[i] > 0.0
                                || (st.max_temp_k[i] > 0.0
                                    && st.temperature_k[i] > 0.4 * st.max_temp_k[i]))
                    })
                    .map(|i| {
                        (
                            i as u16,
                            st.temperature_k[i] as f32,
                            st.max_temp_k[i] as f32,
                            st.reentry_flux_kw[i] as f32,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        gear: u
            .structure(e)
            .map(|st| {
                (0..st.gear_out.len())
                    .filter(|&i| st.gear_defs[i].is_some() && st.rigid.present[i])
                    .map(|i| (i as u16, st.gear_out[i]))
                    .collect()
            })
            .unwrap_or_default(),
    })
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

/// The KSC campus as the client draws it (`flight3d::ksc_pose` over `assets/web/ksc/scene.json`): the
/// scene's launch pad 1 spawn on the pad site, turned so the pads line up best, rigid and unscaled. Its
/// spawn points and flat areas (scene coordinates, glTF axes) follow.
const KSC_ANCHOR: (f64, f64, f64) = (-0.6836, -74.8198, 115.2);
const KSC_YAW_DEG: f64 = -4.655_211_045_966;
const KSC_SPAWNS: [(&str, &str, bool, [f64; 3]); 5] = [
    (
        "LaunchPad_1",
        "Launchpad 1",
        false,
        [10.088_055, -117.589_006, -2_104.040_008],
    ),
    (
        "LaunchPad_2",
        "Launchpad 2",
        false,
        [-1_626.699_936, -117.589_006, -4_039.800_016],
    ),
    (
        "LaunchPad_3",
        "Launchpad 3",
        false,
        [-3_286.311_939, -117.489_000, -2_101.540_008],
    ),
    (
        "LaunchPad_4",
        "Launchpad 4",
        false,
        [-4_989.200_180, -117.589_006, -4_039.749_967],
    ),
    (
        "Runway_1",
        "Runway",
        true,
        [-4_989.899_017, -125.833_997, 2.0],
    ),
];
/// Facility footprints (min x, max x, min z, max z) and the surface a craft stands on there: the pads'
/// decks, the runways, then the campus ground.
const KSC_AREAS: [(f64, f64, f64, f64, f64); 7] = [
    (-87.1, 107.2, -2_158.7, -1_948.1, -117.589),
    (-1_723.8, -1_529.6, -4_094.4, -3_883.8, -117.589),
    (-3_383.5, -3_189.2, -2_156.2, -1_945.6, -117.489),
    (-5_086.3, -4_892.1, -4_094.4, -3_883.8, -117.589),
    (-5_375.2, 2_048.0, -511.2, 58.1, -125.834),
    (-5_375.2, 2_048.0, 184.3, 300.6, -125.834),
    (-5_375.2, 2_048.0, -4_094.4, 300.6, -126.5),
];

/// The campus as a built site on Kerbin.
fn ksc_site() -> ose_sim::campus::Site {
    let s = KSC_SPAWNS[0].3;
    ose_sim::campus::Site {
        lat_deg: KSC_ANCHOR.0,
        lon_deg: KSC_ANCHOR.1,
        alt_m: KSC_ANCHOR.2,
        yaw_rad: KSC_YAW_DEG.to_radians(),
        anchor: ose_math::DVec3::new(s[0], s[1], s[2]),
        areas: KSC_AREAS
            .iter()
            .map(|&(min_x, max_x, min_z, max_z, y)| ose_sim::campus::Area {
                min_x,
                max_x,
                min_z,
                max_z,
                y,
            })
            .collect(),
    }
}

/// The KSC launch sites: the campus scene's spawn points, placed as the client draws the campus.
fn launch_sites(_home: &str) -> Vec<ose_net::LaunchSite> {
    let site = ksc_site();
    KSC_SPAWNS
        .iter()
        .map(|&(id, name, runway, p)| {
            let (lat, lon, _) = site.place(600_000.0, ose_math::DVec3::new(p[0], p[1], p[2]));
            ose_net::LaunchSite {
                id: id.into(),
                name: name.into(),
                runway,
                lat_deg: lat,
                lon_deg: lon,
                // planes face down the runway (the scene's +X)
                heading_deg: site.x_heading_deg(),
            }
        })
        .collect()
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

/// Put a craft on the KSC pad and remember the moments before and after for revert.
fn launch_on_pad(s: &Shared, player_id: u64, craft: &ose_data::craft::Craft) -> ServerMessage {
    launch_at(s, player_id, craft, None, None)
}

/// Put a craft on a launch site (pad 1 without one) and remember the moments before and after for revert.
fn launch_at(
    s: &Shared,
    player_id: u64,
    craft: &ose_data::craft::Craft,
    site: Option<&str>,
    crew: Option<&[String]>,
) -> ServerMessage {
    let editor = campaigns::snapshot(s);
    let reply = launch_on_pad_now(s, player_id, craft, site, crew);
    if matches!(
        reply,
        ServerMessage::CraftLaunched {
            vessel: Some(_),
            ..
        }
    ) {
        *s.revert.lock().expect("revert lock") = Some(campaigns::RevertPoint {
            player: player_id,
            editor,
            launch: campaigns::snapshot(s),
            craft: craft.clone(),
        });
    }
    reply
}

/// Put a craft on the KSC pad as a landed, powered vessel.
fn launch_on_pad_now(
    s: &Shared,
    player_id: u64,
    craft: &ose_data::craft::Craft,
    site: Option<&str>,
    crew: Option<&[String]>,
) -> ServerMessage {
    let mut filled = craft.clone();
    // loose assemblies stay in the editor
    filled.loose.clear();
    fill_default_resources(s, &mut filled);
    // a craft built nose-forward (a plane) is turned to point along +Y and lies level
    let level = ose_sim::structure::level_layout(&filled);
    let is_level = level.is_some();
    if let Some(l) = level {
        filled = l;
    }
    let craft = &filled;
    let name = craft.name.clone();
    let parts = craft.parts.len() as u32;
    let m = craft.mass(&s.parts, &s.resources);
    let report = ose_data::staging::delta_v_by_stage(craft, &s.parts, &s.resources);
    let stages = stage_readouts(s, craft, &report);
    let fail = |reason: String| ServerMessage::CraftLaunched {
        vessel: None,
        name: name.clone(),
        parts,
        wet_mass_t: m.wet_t(),
        dry_mass_t: m.dry_t,
        delta_v_vacuum_mps: vacuum_dv(s, craft, &report),
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
    let sites = launch_sites(&home.id);
    let at = match site {
        None => sites.first(),
        Some(id) => sites.iter().find(|x| x.id == id),
    };
    let Some(at) = at.cloned() else {
        return fail(format!("no launch site {}", site.unwrap_or("")));
    };
    let (pad_lat, pad_lon) = (at.lat_deg, at.lon_deg);
    let e = u.spawn_powered_on_surface(&name, body, spec, pad_lat, pad_lon);
    u.set_craft(e, craft.clone());
    u.set_structure(
        e,
        ose_sim::structure::Structure::build(craft, &s.parts, &s.resources),
    );
    if is_level {
        u.set_level(e, at.heading_deg);
    }
    if let Err(reason) = crew_launch(s, &mut u, e, crew) {
        u.despawn_vessel(e);
        return fail(reason);
    }
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
        delta_v_vacuum_mps: vacuum_dv(s, craft, &report),
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
/// EVA, boarding, flags and undocking.
fn crew_request(s: &Shared, player_id: u64, msg: ClientMessage) -> ServerMessage {
    let ack = |vessel: u64, r: Result<Option<u64>, String>| match r {
        Ok(new_vessel) => ServerMessage::CrewAck {
            vessel,
            accepted: true,
            reason: None,
            new_vessel,
        },
        Err(reason) => ServerMessage::CrewAck {
            vessel,
            accepted: false,
            reason: Some(reason),
            new_vessel: None,
        },
    };
    match msg {
        ClientMessage::GoEva { vessel, kerbal } => {
            let Some(e) = owned_vessel(s, vessel, player_id) else {
                return ack(vessel, Err("not your vessel".into()));
            };
            let r = s
                .universe
                .lock()
                .expect("sim lock")
                .go_eva(e, kerbal.as_deref());
            ack(
                vessel,
                r.map(|k| Some(crew_docking::register(s, k, Some(player_id)))),
            )
        }
        ClientMessage::Board { vessel, target } => {
            let Some(e) = owned_vessel(s, vessel, player_id) else {
                return ack(vessel, Err("not your Kerbal".into()));
            };
            let t = s
                .vessels
                .lock()
                .expect("vessels lock")
                .iter()
                .find(|v| v.1 == target)
                .map(|v| v.0);
            let Some(t) = t else {
                return ack(vessel, Err("no such vessel".into()));
            };
            let r = s.universe.lock().expect("sim lock").board(e, t);
            if r.is_ok() {
                s.vessels.lock().expect("vessels lock").retain(|v| v.0 != e);
            }
            ack(vessel, r.map(|_| None))
        }
        ClientMessage::PlantFlag { vessel } => {
            let Some(e) = owned_vessel(s, vessel, player_id) else {
                return ack(vessel, Err("not your Kerbal".into()));
            };
            let label = format!("{} flag", display_name_for(s, player_id));
            let r = s.universe.lock().expect("sim lock").plant_flag(e, &label);
            ack(
                vessel,
                r.map(|f| Some(crew_docking::register(s, f, Some(player_id)))),
            )
        }
        ClientMessage::Undock { vessel, port } => {
            let Some(e) = owned_vessel(s, vessel, player_id) else {
                return ServerMessage::Undocked {
                    vessel,
                    new_vessel: None,
                    reason: Some("not your vessel".into()),
                };
            };
            match crew_docking::undock(s, e, port.map(|p| p as usize)) {
                Ok(ne) => ServerMessage::Undocked {
                    vessel,
                    new_vessel: Some(crew_docking::register(s, ne, Some(player_id))),
                    reason: None,
                },
                Err(reason) => ServerMessage::Undocked {
                    vessel,
                    new_vessel: None,
                    reason: Some(reason),
                },
            }
        }
        _ => ServerMessage::CrewAck {
            vessel: 0,
            accepted: false,
            reason: Some("unexpected".into()),
            new_vessel: None,
        },
    }
}

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
                if !recover {
                    u.update_commnet();
                }
                if !recover && !u.comm_link(e).connected {
                    return fail("no CommNet connection to the KSC");
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
        ClientMessage::AcceptMission { ref id } | ClientMessage::TurnInMission { ref id } => {
            let id = id.clone();
            let accept = matches!(msg, ClientMessage::AcceptMission { .. });
            let now = s.universe.lock().expect("sim lock").tick().0;
            let res = {
                let mut c = s.campaign.lock().expect("campaign lock");
                if accept {
                    s.science.accept(&mut c, &id, now).map(|_| 0.0)
                } else {
                    s.science.turn_in(&mut c, &id)
                }
            };
            if let Ok(paid) = res {
                tracing::info!(player = player_id, mission = %id, accept, paid, "mission");
            }
            vec![
                ServerMessage::MissionAck {
                    id,
                    accepted: res.is_ok(),
                    science: *res.as_ref().unwrap_or(&0.0),
                    reason: res.err(),
                },
                state(s),
            ]
        }
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
    let e = s
        .vessels
        .lock()
        .expect("vessels lock")
        .iter()
        .find(|&&(_, vid, owner)| vid == id && owner == Some(player))
        .map(|&(e, _, _)| e);
    if e.is_some() {
        // commanding a vessel is flying it
        s.flying.lock().expect("flying lock").insert(player, id);
    }
    e
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
    let mut arrived = false;
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
        arrived = s.playing.lock().expect("playing lock").insert(player_id);
    }
    let (vessels, ids): (u32, std::collections::HashSet<u64>) = {
        let list = s.vessels.lock().expect("vessels lock");
        (list.len() as u32, list.iter().map(|v| v.1).collect())
    };
    let playing = s.playing.lock().expect("playing lock").clone();
    let flying = s.flying.lock().expect("flying lock").clone();
    let msg = ServerMessage::CampaignState {
        id: c.id.clone(),
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
                online: playing.contains(&m.player_id),
                flying: flying
                    .get(&m.player_id)
                    .copied()
                    .filter(|v| ids.contains(v)),
            })
            .collect(),
        you_are_member: c.members.iter().any(|m| m.player_id == player_id),
        vessels,
        created: false,
    };
    drop(c);
    if arrived {
        push_campaign(s, player_id);
    }
    msg
}

/// Someone joined or left: everyone else in the campaign gets its state again.
fn push_campaign(s: &Shared, except: u64) {
    let players: Vec<u64> = s
        .playing
        .lock()
        .expect("playing lock")
        .iter()
        .copied()
        .filter(|&p| p != except)
        .collect();
    for p in players {
        let msg = campaign_state(s, p, None);
        s.pending.lock().expect("pending lock").push((p, msg));
    }
}

/// Quicksave, quickload and revert.
fn save_request(s: &Shared, player_id: u64, msg: &ClientMessage) -> ServerMessage {
    use ose_net::SaveAction as A;
    let action = match msg {
        ClientMessage::QuickSave => A::QuickSave,
        ClientMessage::QuickLoad => A::QuickLoad,
        ClientMessage::RevertToLaunch => A::RevertToLaunch,
        _ => A::RevertToEditor,
    };
    let mut craft = None;
    let result: Result<(), String> = match action {
        A::QuickSave => {
            let file = campaigns::snapshot(s);
            s.campaign.lock().expect("campaign lock").quicksave = Some(Box::new(file));
            Ok(())
        }
        A::QuickLoad => {
            let file = s.campaign.lock().expect("campaign lock").quicksave.clone();
            match file {
                _ if !campaigns::allowed(s, "AllowQuickLoad") => {
                    Err("quickload is off for this campaign".into())
                }
                None => Err("no quicksave yet".into()),
                Some(f) => campaigns::reload(s, player_id, &f).map(|_| {
                    *s.revert.lock().expect("revert lock") = None;
                }),
            }
        }
        A::RevertToLaunch | A::RevertToEditor => {
            let point = s
                .revert
                .lock()
                .expect("revert lock")
                .as_ref()
                .filter(|r| r.player == player_id)
                .map(|r| {
                    let file = if action == A::RevertToLaunch {
                        r.launch.clone()
                    } else {
                        r.editor.clone()
                    };
                    (file, r.craft.clone())
                });
            match point {
                _ if !campaigns::allowed(s, "AllowRevert") => {
                    Err("revert is off for this campaign".into())
                }
                None => Err("nothing to revert to".into()),
                Some((file, c)) => campaigns::reload(s, player_id, &file).map(|_| {
                    if action == A::RevertToEditor {
                        craft = Some(c);
                        *s.revert.lock().expect("revert lock") = None;
                    }
                }),
            }
        }
    };
    if result.is_ok() {
        tracing::info!(player = player_id, ?action, "save action");
    }
    ServerMessage::SaveAck {
        action,
        accepted: result.is_ok(),
        reason: result.err(),
        craft,
    }
}

/// A craft's wet centre of mass, and the thrust centre and direction of its first stage to fire (craft
/// frame; an engine pushes along its own +Y).
fn balance(
    s: &Shared,
    craft: &ose_data::craft::Craft,
    report: &ose_data::staging::DeltaVReport,
) -> (ose_math::DVec3, ose_math::DVec3, ose_math::DVec3) {
    use ose_math::DVec3;
    let unit = |id: &str| {
        s.resources
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| match &r.kind {
                ose_data::ResourceKind::Basic {
                    mass_per_unit_t, ..
                } => Some(*mass_per_unit_t),
                _ => None,
            })
            .unwrap_or(0.0)
    };
    let def = |i: usize| s.parts.iter().find(|d| d.id == craft.parts[i].part_id);
    let (mut m, mut c) = (0.0, DVec3::ZERO);
    for (i, p) in craft.parts.iter().enumerate() {
        let wet = def(i).map(|d| d.mass_t).unwrap_or(0.0)
            + p.resources
                .iter()
                .map(|r| r.stored_units * unit(&r.resource))
                .sum::<f64>();
        m += wet;
        c = c + p.local_position * wet;
    }
    let com = if m > 0.0 { c * (1.0 / m) } else { DVec3::ZERO };
    let rotate = |q: ose_data::craft::Quat, v: DVec3| {
        let u = DVec3::new(q.x, q.y, q.z);
        let t = u.cross(v) * 2.0;
        v + t * q.w + u.cross(t)
    };
    let (mut thrust, mut at, mut dir) = (0.0, DVec3::ZERO, DVec3::ZERO);
    if let Some(stage) = report.stages.iter().find(|st| st.thrust_kn > 0.0) {
        for &i in &stage.engines {
            let t = def(i)
                .and_then(|d| d.engines.first())
                .map(|e| e.max_thrust_kn)
                .unwrap_or(0.0);
            let p = &craft.parts[i];
            thrust += t;
            at = at + p.local_position * t;
            dir = dir + rotate(p.local_rotation, DVec3::new(0.0, 1.0, 0.0)) * t;
        }
    }
    if thrust > 0.0 {
        (com, at * (1.0 / thrust), dir.normalized())
    } else {
        (com, DVec3::ZERO, DVec3::ZERO)
    }
}

/// The lifting surfaces' centre, weighted by their lift coefficient (craft frame; zero without any).
fn centre_of_lift(s: &Shared, craft: &ose_data::craft::Craft) -> ose_math::DVec3 {
    let (mut w, mut c) = (0.0, ose_math::DVec3::ZERO);
    for p in &craft.parts {
        let coeff = s
            .parts
            .iter()
            .find(|d| d.id == p.part_id)
            .and_then(|d| d.lifting_surface.as_ref())
            .map(|l| l.deflection_lift_coeff)
            .unwrap_or(0.0);
        w += coeff;
        c = c + p.local_position * coeff;
    }
    if w > 0.0 {
        c * (1.0 / w)
    } else {
        ose_math::DVec3::ZERO
    }
}

/// The craft's delta-v in vacuum: jets breathe air, so their stages count nothing (never negative).
fn vacuum_dv(
    s: &Shared,
    craft: &ose_data::craft::Craft,
    report: &ose_data::staging::DeltaVReport,
) -> f64 {
    let dv: f64 = report
        .stages
        .iter()
        .filter(|st| !jet_stage(s, craft, st))
        .map(|st| st.delta_v_vacuum_mps)
        .sum();
    dv.max(0.0)
}

/// A stage whose engines all breathe air (jets): no thrust and no delta-v in vacuum.
fn jet_stage(
    s: &Shared,
    craft: &ose_data::craft::Craft,
    st: &ose_data::staging::StageDeltaV,
) -> bool {
    !st.engines.is_empty()
        && st.engines.iter().all(|&i| {
            s.parts
                .iter()
                .find(|d| d.id == craft.parts[i].part_id)
                .and_then(|d| d.engines.first())
                .is_some_and(|e| e.air_breathing)
        })
}

/// The staging report as the protocol carries it, with sea-level thrust and TWR.
fn stage_readouts(
    s: &Shared,
    craft: &ose_data::craft::Craft,
    report: &ose_data::staging::DeltaVReport,
) -> Vec<ose_net::StageReadout> {
    stage_readouts_at(s, craft, report, None)
}

/// The staging readout, and with `here = Some((pressure atm, gravity m/s^2))` the thrust, TWR and
/// delta-v at that place too.
fn stage_readouts_at(
    s: &Shared,
    craft: &ose_data::craft::Craft,
    report: &ose_data::staging::DeltaVReport,
    here: Option<(f64, f64)>,
) -> Vec<ose_net::StageReadout> {
    // an engine's thrust and Isp at `atm` (the original keeps fuel flow fixed: thrust follows Isp)
    let at = |part: usize, atm: f64| -> (f64, f64, f64) {
        s.parts
            .iter()
            .find(|d| d.id == craft.parts[part].part_id)
            .and_then(|d| d.engines.first())
            .map(|e| {
                let vac = e.isp_curve.evaluate(0.0);
                let isp = e.isp_curve.evaluate(atm);
                let thrust = if vac > 0.0 {
                    e.max_thrust_kn * isp / vac
                } else {
                    0.0
                };
                (thrust, isp, vac)
            })
            .unwrap_or((0.0, 0.0, 0.0))
    };
    let asl = |part: usize| -> f64 {
        s.parts
            .iter()
            .find(|d| d.id == craft.parts[part].part_id)
            .and_then(|d| d.engines.first())
            .map(|e| {
                let (vac, sea) = (e.isp_curve.evaluate(0.0), e.isp_curve.evaluate(1.0));
                if vac > 0.0 {
                    e.max_thrust_kn * sea / vac
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0)
    };
    report
        .stages
        .iter()
        .map(|st| {
            let thrust_asl_kn: f64 = st.engines.iter().map(|&i| asl(i)).sum();
            let jet = jet_stage(s, craft, st);
            let (thrust_now_kn, twr_now, delta_v_now_mps) = match here {
                Some((atm, g)) => {
                    let e: Vec<(f64, f64, f64)> = st.engines.iter().map(|&i| at(i, atm)).collect();
                    let thrust: f64 = e.iter().map(|x| x.0).sum();
                    // thrust-weighted Isp ratio here against vacuum
                    let (num, den) = e
                        .iter()
                        .fold((0.0, 0.0), |(n, d), x| (n + x.0 * x.1, d + x.0 * x.2));
                    let ratio = if den > 0.0 { num / den } else { 0.0 };
                    let dv = if jet {
                        // jets: what they would make burning everything here
                        st.delta_v_vacuum_mps
                    } else {
                        st.delta_v_vacuum_mps * ratio
                    };
                    let twr = if st.start_mass_t > 0.0 && g > 0.0 {
                        thrust / (st.start_mass_t * g)
                    } else {
                        0.0
                    };
                    (thrust, twr, dv.max(0.0))
                }
                None => (0.0, 0.0, 0.0),
            };
            ose_net::StageReadout {
                stage_id: st.stage_id as u32,
                start_mass_t: st.start_mass_t,
                end_mass_t: st.end_mass_t,
                delta_v_vacuum_mps: if jet_stage(s, craft, st) {
                    0.0
                } else {
                    st.delta_v_vacuum_mps
                },
                burn_time_s: st.burn_time_s,
                thrust_kn: st.thrust_kn,
                thrust_asl_kn,
                twr_asl: if st.start_mass_t > 0.0 {
                    thrust_asl_kn / (st.start_mass_t * 9.81)
                } else {
                    0.0
                },
                thrust_now_kn,
                twr_now,
                delta_v_now_mps,
            }
        })
        .collect()
}

/// The campaign's hired Kerbals (a new campaign hires the first few), then the rest of the original's in
/// their order: who fills seats when nobody is chosen.
fn crew_order(s: &Shared, c: &mut save::Campaign) -> Vec<String> {
    if c.roster.is_empty() {
        c.roster = s
            .kerbals
            .iter()
            .take(STARTING_CREW)
            .map(|k| (k.name.clone(), 0.0))
            .collect();
    }
    let mut out: Vec<String> = c.roster.iter().map(|(n, _)| n.clone()).collect();
    out.extend(
        s.kerbals
            .iter()
            .map(|k| k.name.clone())
            .filter(|n| !c.roster.iter().any(|(r, _)| r == n)),
    );
    out
}

/// Crew a freshly launched vessel: the chosen Kerbal per seat, or the roster in order. Kerbals new to the
/// campaign are hired.
fn crew_launch(
    s: &Shared,
    u: &mut Universe,
    e: bevy_ecs::entity::Entity,
    chosen: Option<&[String]>,
) -> Result<(), String> {
    let ut = u.ut();
    let mut c = s.campaign.lock().expect("campaign lock");
    let order = crew_order(s, &mut c);
    // the sim's default fill is replaced
    u.set_crew(e, Vec::new());
    let members = match chosen {
        None => u.crew_vessel(e, &order),
        Some(names) => {
            let seats = u.crew_seats(e);
            let busy = u.kerbals_in_use();
            let mut members = Vec::new();
            for (seat, name) in seats.iter().zip(names) {
                if name.is_empty() {
                    continue;
                }
                if !c.roster.iter().any(|(n, _)| n == name) {
                    return Err(format!("{name} is not on the roster"));
                }
                if busy.contains(name) || members.iter().any(|(_, n)| n == name) {
                    return Err(format!("{name} is not available"));
                }
                members.push((*seat, name.clone()));
            }
            if names.iter().filter(|n| !n.is_empty()).count() > seats.len() {
                return Err(format!("only {} seats", seats.len()));
            }
            u.set_crew(e, members.clone());
            members
        }
    };
    for (_, name) in members {
        if !c.roster.iter().any(|(n, _)| *n == name) {
            c.roster.push((name, ut));
        }
    }
    Ok(())
}

/// The campaign's astronauts with where each one is.
fn kerbal_roster(s: &Shared) -> ServerMessage {
    let u = s.universe.lock().expect("sim lock");
    let list = s.vessels.lock().expect("vessels lock").clone();
    let mut c = s.campaign.lock().expect("campaign lock");
    crew_order(s, &mut c);
    let mut status: std::collections::HashMap<String, ose_net::KerbalStatus> = Default::default();
    for &(e, id, _) in &list {
        if let Some(k) = u.eva_kerbal(e) {
            status.insert(k.name.clone(), ose_net::KerbalStatus::Eva { vessel: id });
        }
        let crashed = u.powered(e).is_some_and(|p| p.crashed);
        for (_, name) in u.crew(e) {
            status.insert(
                name,
                if crashed {
                    ose_net::KerbalStatus::Missing
                } else {
                    ose_net::KerbalStatus::Assigned { vessel: id }
                },
            );
        }
    }
    let kerbals = c
        .roster
        .iter()
        .map(|(name, enrolled)| {
            let def = s.kerbals.iter().find(|k| k.name == *name);
            ose_net::KerbalInfo {
                name: name.clone(),
                status: status
                    .get(name)
                    .copied()
                    .unwrap_or(ose_net::KerbalStatus::Available),
                veteran: def.is_some_and(|d| d.veteran),
                enrolled_ut: *enrolled,
                bravery: def.map(|d| d.bravery as f32).unwrap_or(0.5),
                stupidity: def.map(|d| d.stupidity as f32).unwrap_or(0.5),
                optimism: def.map(|d| d.optimism as f32).unwrap_or(0.5),
                constitution: def.map(|d| d.constitution as f32).unwrap_or(0.5),
                looks: def
                    .map(|d| {
                        d.looks
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect();
    ServerMessage::KerbalRoster { kerbals }
}

/// Push the roster to every connected player when it has changed.
fn push_roster(s: &Shared) {
    let msg = kerbal_roster(s);
    let mut last = s.last_roster.lock().expect("roster lock");
    if last.as_ref() == Some(&msg) {
        return;
    }
    *last = Some(msg.clone());
    drop(last);
    let players: Vec<u64> = s
        .warp_requests
        .lock()
        .expect("warp lock")
        .keys()
        .copied()
        .collect();
    let mut q = s.pending.lock().expect("pending lock");
    for p in players {
        q.push((p, msg.clone()));
    }
}

fn session_list(s: &Shared) -> ServerMessage {
    campaigns::list(s)
}

fn craft_summary(
    s: &Shared,
    id: u64,
    owner: Option<u64>,
    craft: &ose_data::craft::Craft,
) -> ose_net::CraftSummary {
    let m = craft.mass(&s.parts, &s.resources);
    let report = ose_data::staging::delta_v_by_stage(craft, &s.parts, &s.resources);
    let dv = vacuum_dv(s, craft, &report);
    ose_net::CraftSummary {
        id,
        name: craft.name.clone(),
        description: craft.description.clone(),
        parts: craft.parts.len() as u32,
        stages: craft.stages.len() as u32,
        dry_mass_t: m.dry_t,
        wet_mass_t: m.wet_t(),
        delta_v_vacuum_mps: dv,
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
    let recipe_of = |id: &str| {
        s.resources
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| match &r.kind {
                ose_data::ResourceKind::Recipe { ingredients } => Some(ingredients.clone()),
                _ => None,
            })
    };
    for p in &mut craft.parts {
        if !p.resources.is_empty() {
            // an editor may list a mixture tank by its mixture (Methalox): store the ingredients instead
            let mut out = Vec::new();
            for r in p.resources.drain(..) {
                match recipe_of(&r.resource) {
                    Some(ingredients) => {
                        for i in ingredients {
                            out.push(ose_data::craft::StoredResource {
                                resource: i.resource.clone(),
                                stored_units: r.stored_units * i.units_per_recipe_unit,
                                capacity_units: r.capacity_units * i.units_per_recipe_unit,
                            });
                        }
                    }
                    None => out.push(r),
                }
            }
            p.resources = out;
            continue;
        }
        if let Some(def) = s.parts.iter().find(|d| d.id == p.part_id) {
            // a tank defined for a mixture (Methalox) holds its ingredients, as the original's craft files do
            let mut out: Vec<ose_data::craft::StoredResource> = Vec::new();
            for c in &def.resource_containers {
                let recipe = s
                    .resources
                    .iter()
                    .find(|r| r.id == c.resource)
                    .and_then(|r| match &r.kind {
                        ose_data::ResourceKind::Recipe { ingredients } => Some(ingredients.clone()),
                        _ => None,
                    });
                match recipe {
                    Some(ingredients) => {
                        for i in ingredients {
                            out.push(ose_data::craft::StoredResource {
                                resource: i.resource.clone(),
                                stored_units: c.initial_units * i.units_per_recipe_unit,
                                capacity_units: c.capacity_units * i.units_per_recipe_unit,
                            });
                        }
                    }
                    None => out.push(ose_data::craft::StoredResource {
                        resource: c.resource.clone(),
                        stored_units: c.initial_units,
                        capacity_units: c.capacity_units,
                    }),
                }
            }
            p.resources = out;
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
    let stages = stage_readouts(s, craft, &report);
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
            u.set_craft(e, craft.clone());
            u.set_structure(
                e,
                ose_sim::structure::Structure::build(craft, &s.parts, &s.resources),
            );
            let _ = crew_launch(s, &mut u, e, None);
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
                        // the difficulty options every time: the new-campaign form needs them
                        if !send(&mut socket, &s.difficulty).await {
                            return;
                        }
                        if !send(
                            &mut socket,
                            &ServerMessage::LaunchSites {
                                sites: launch_sites("Kerbin"),
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

    // Subscribe after Hello has registered the player. Wait for fresh world state rather than
    // replaying a cached snapshot from before their probe was created or a campaign was loaded.
    let mut snapshots = s.streams.snapshots.subscribe();
    let mut predictions = s.streams.trajectories.subscribe();
    let subscribed_at = streaming::mark();
    let mut science_rev = s.campaign.lock().expect("campaign lock").science.rev;
    'session: loop {
        tokio::select! {
            frames = streaming::next_batch(&mut snapshots, subscribed_at) => {
                let Some(frames) = frames else { break; };
                if !streaming::send_batch(&mut socket, &frames).await {
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
            frames = streaming::next_batch(&mut predictions, subscribed_at) => {
                let Some(frames) = frames else { break; };
                if !streaming::send_batch(&mut socket, &frames).await { break; }
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
                                if !send(&mut socket, &msg).await { break 'session; }
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
                            let live = s.campaign.lock().expect("campaign lock").id.clone();
                            let switched = if id == live {
                                Ok(())
                            } else {
                                campaigns::switch(&s, player_id, campaigns::Target::Shelved(id.clone()))
                            };
                            let reply = match switched {
                                Ok(()) => {
                                    let name = display_name_for(&s, player_id);
                                    // a pilot probe for a player who has nothing in this campaign yet
                                    if !has_vessel(&s, player_id) {
                                        spawn_player_probe(&s, player_id, &name);
                                    }
                                    campaign_state(&s, player_id, Some(&name))
                                }
                                Err(reason) => ServerMessage::Rejected { reason },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::CreateSession { name, mode, difficulty }) => {
                            let name = if name.trim().is_empty() { "New campaign".to_string() } else { name.trim().to_string() };
                            let new = campaigns::NewCampaign { name, mode, difficulty };
                            let reply = match campaigns::switch(&s, player_id, campaigns::Target::New(new)) {
                                Ok(()) => {
                                    let pname = display_name_for(&s, player_id);
                                    if !has_vessel(&s, player_id) {
                                        spawn_player_probe(&s, player_id, &pname);
                                    }
                                    let mut reply = campaign_state(&s, player_id, Some(&pname));
                                    if let ServerMessage::CampaignState { created: c, .. } = &mut reply {
                                        *c = true;
                                    }
                                    reply
                                }
                                Err(reason) => ServerMessage::Rejected { reason },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LeaveSession) => {
                            if s.playing.lock().expect("playing lock").remove(&player_id) {
                                push_campaign(&s, player_id);
                            }
                            let reply = session_list(&s);
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
                        Ok(ClientMessage::KerbalRoster) => {
                            let reply = kerbal_roster(&s);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LaunchAt { craft, site, crew }) => {
                            let reply = launch_at(&s, player_id, &craft, site.as_deref(), crew.as_deref());
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::LaunchSavedAt { id, site, crew }) => {
                            let reply = match find_craft(&s, id, player_id) {
                                Some(c) => launch_at(&s, player_id, &c, site.as_deref(), crew.as_deref()),
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
                                Some(e) if attitude == ose_net::AttitudeMode::Maneuver
                                    && s.universe.lock().expect("sim lock").scheduled_burn(e).is_none() =>
                                {
                                    ServerMessage::FlightAck { vessel, accepted: false, reason: Some("no maneuver planned".into()) }
                                }
                                Some(e) if matches!(attitude, ose_net::AttitudeMode::Target | ose_net::AttitudeMode::AntiTarget)
                                    && s.universe.lock().expect("sim lock").target(e).is_none() =>
                                {
                                    ServerMessage::FlightAck { vessel, accepted: false, reason: Some("no target set".into()) }
                                }
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
                        | ClientMessage::UnlockTechNode { .. }
                        | ClientMessage::AcceptMission { .. }
                        | ClientMessage::TurnInMission { .. })) => {
                            let replies = science_request(&s, player_id, msg);
                            science_rev = s.campaign.lock().expect("campaign lock").science.rev;
                            let mut ok = true;
                            for r in &replies {
                                ok = ok && send(&mut socket, r).await;
                            }
                            if !ok { break; }
                        }
                        Ok(ClientMessage::DeployParachutes { vessel }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").arm_parachutes(e) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    _ => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("no packed parachutes".into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::Translate { vessel, x, y, z }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let t = ose_math::DVec3::new(x as f64, y as f64, z as f64);
                                    let ok = s.universe.lock().expect("sim lock").set_translate(e, t);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not under power".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetRotation { vessel, pitch, yaw, roll }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let ok = s.universe.lock().expect("sim lock").set_rotation(e, pitch as f64, yaw as f64, roll as f64);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not under power".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::EvaControls { vessel, run, jump }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let ok = s.universe.lock().expect("sim lock").set_eva(e, run, jump);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not a Kerbal on EVA".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetTrim { vessel, pitch, yaw, roll }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let t = ose_math::DVec3::new(pitch as f64, yaw as f64, roll as f64);
                                    let ok = s.universe.lock().expect("sim lock").set_trim(e, t);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not under power".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::AeroOverlay { vessel, on }) => {
                            // anyone may look at any vessel's airflow
                            let e = s.vessels.lock().expect("vessels lock").iter().find(|x| x.1 == vessel).map(|x| x.0);
                            let ok = e.is_some_and(|e| s.universe.lock().expect("sim lock").set_aero_overlay(e, on));
                            let reply = ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "no such vessel under power".into()) };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::GetVesselCraft { vessel }) => {
                            let e = s.vessels.lock().expect("vessels lock").iter().find(|x| x.1 == vessel).map(|x| x.0);
                            let craft = e.and_then(|e| s.universe.lock().expect("sim lock").current_craft(e));
                            let reply = ServerMessage::VesselCraft { vessel, craft };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetRoll { vessel, input }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let ok = s.universe.lock().expect("sim lock").set_roll(e, input as f64);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not under power".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::ActionGroup { vessel, group, on }) => {
                            let group = match group {
                                ose_net::ActionGroup::Lights => ose_sim::ActionGroup::Lights,
                                ose_net::ActionGroup::Rcs => ose_sim::ActionGroup::Rcs,
                                ose_net::ActionGroup::Abort => ose_sim::ActionGroup::Abort,
                                ose_net::ActionGroup::Custom(n) => ose_sim::ActionGroup::Custom(n),
                            };
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let state = s.universe.lock().expect("sim lock").action_group(e, group, on);
                                    ServerMessage::FlightAck { vessel, accepted: state.is_some(), reason: state.is_none().then(|| "not under power, or no such group".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::StageReport { craft }) => {
                            let mut filled = craft;
                            fill_default_resources(&s, &mut filled);
                            let report = ose_data::staging::delta_v_by_stage(&filled, &s.parts, &s.resources);
                            let (com_m, cot_m, thrust_dir) = balance(&s, &filled, &report);
                            let col_m = centre_of_lift(&s, &filled);
                            let reply = ServerMessage::StageReport {
                                stages: stage_readouts(&s, &filled, &report),
                                delta_v_vacuum_mps: vacuum_dv(&s, &filled, &report),
                                com_m,
                                cot_m,
                                thrust_dir,
                                col_m,
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(msg @ (ClientMessage::QuickSave
                        | ClientMessage::QuickLoad
                        | ClientMessage::RevertToLaunch
                        | ClientMessage::RevertToEditor)) => {
                            let reply = save_request(&s, player_id, &msg);
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetTarget { vessel, target }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let t = match target {
                                        None => Ok(None),
                                        Some(ose_net::TargetId::Vessel(id)) => s
                                            .vessels
                                            .lock()
                                            .expect("vessels lock")
                                            .iter()
                                            .find(|x| x.1 == id)
                                            .map(|x| Some(x.0))
                                            .ok_or("no such vessel"),
                                        Some(ose_net::TargetId::Body(i)) => {
                                            let u = s.universe.lock().expect("sim lock");
                                            s.catalog
                                                .get(i as usize)
                                                .and_then(|b| u.body_entity(&b.id))
                                                .map(Some)
                                                .ok_or("no such body")
                                        }
                                    };
                                    match t {
                                        Ok(t) => {
                                            let ok = s.universe.lock().expect("sim lock").set_target(e, t);
                                            ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "cannot target that".into()) }
                                        }
                                        Err(reason) => ServerMessage::FlightAck { vessel, accepted: false, reason: Some(reason.into()) },
                                    }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::DeployFairings { vessel }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").deploy_fairings(e) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    _ => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("no fairings on".into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetCargoBays { vessel, open }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").open_cargo_bays(e, open) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    _ => ServerMessage::FlightAck { vessel, accepted: false, reason: Some(if open { "no closed cargo bays" } else { "no open cargo bays" }.into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::SetConverters { vessel, on }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").set_converters(e, on) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    Some(_) => ServerMessage::FlightAck { vessel, accepted: false, reason: Some(if on { "no converters off" } else { "no converters on" }.into()) },
                                    None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("vessel has no parts".into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::Drive { vessel, throttle, steer, brakes }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let ok = s.universe.lock().expect("sim lock").set_drive(e, throttle as f64, steer as f64, brakes);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not under power".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::DeployGear { vessel, deploy }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").deploy_gear(e, deploy) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    Some(_) => ServerMessage::FlightAck { vessel, accepted: false, reason: Some(if deploy { "no folded gear" } else { "no gear out" }.into()) },
                                    None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("vessel has no parts".into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::DeployAntennas { vessel, deploy }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").deploy_antennas(e, deploy) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    Some(_) => ServerMessage::FlightAck { vessel, accepted: false, reason: Some(if deploy { "no retracted antennas" } else { "no antennas extended" }.into()) },
                                    None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("vessel has no parts".into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::MoveToStage { vessel, part, stage }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let spec_for = |c: &ose_data::craft::Craft| flight_spec_for(&s, c);
                                    let r = s.universe.lock().expect("sim lock").move_to_stage(e, part as usize, stage as usize, &s.parts, &s.resources, &spec_for);
                                    ServerMessage::FlightAck { vessel, accepted: r.is_ok(), reason: r.err().map(String::from) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::ControlFrom { vessel, part }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => {
                                    let spec_for = |c: &ose_data::craft::Craft| flight_spec_for(&s, c);
                                    let r = s.universe.lock().expect("sim lock").control_from(e, part as usize, &s.parts, &s.resources, &spec_for);
                                    ServerMessage::FlightAck { vessel, accepted: r.is_ok(), reason: r.err().map(String::from) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::RenameVessel { vessel, name }) => {
                            let name = name.trim().to_string();
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(_) if name.is_empty() || name.chars().count() > 64 => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("a name is 1 to 64 characters".into()) },
                                Some(e) => {
                                    let ok = s.universe.lock().expect("sim lock").rename_vessel(e, &name);
                                    ServerMessage::FlightAck { vessel, accepted: ok, reason: (!ok).then(|| "not a vessel".into()) }
                                }
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(ClientMessage::DeployPanels { vessel, deploy }) => {
                            let reply = match owned_vessel(&s, vessel, player_id) {
                                Some(e) => match s.universe.lock().expect("sim lock").deploy_panels(e, deploy) {
                                    Some(n) if n > 0 => ServerMessage::FlightAck { vessel, accepted: true, reason: None },
                                    Some(_) => ServerMessage::FlightAck { vessel, accepted: false, reason: Some(if deploy { "no folded panels" } else { "no panels out" }.into()) },
                                    None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("vessel has no parts".into()) },
                                },
                                None => ServerMessage::FlightAck { vessel, accepted: false, reason: Some("not your vessel".into()) },
                            };
                            if !send(&mut socket, &reply).await { break; }
                        }
                        Ok(msg @ (ClientMessage::GoEva { .. }
                        | ClientMessage::Board { .. }
                        | ClientMessage::PlantFlag { .. }
                        | ClientMessage::Undock { .. })) => {
                            let reply = crew_request(&s, player_id, msg);
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
    if s.playing.lock().expect("playing lock").remove(&player_id) {
        push_campaign(&s, player_id);
    }
    apply_warp(&s);
    let _ = s.accounts.lock().expect("accounts lock").touch(player_id);
    tracing::info!(player = player_id, "player left");
}

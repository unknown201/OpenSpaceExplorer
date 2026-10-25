//! Browser client: rendering, input, UI and snapshot interpolation only (ADR-0002). Game logic is native.
//!
//! The page (`index.html`) is a full-window `<canvas id="map">`, a `#labels` layer with one positioned
//! element per celestial body, and a `#hud` with the universal-time readout and warp buttons. The test hooks
//! the e2e suite looks for are `data-ose="..."` attributes (`e2e/README.md`). The map is drawn with Canvas 2D
//! for now (ADR-0005); the WebGPU flight view comes later.
//!
//! Session: connect to `/ws` on the page's host, send `Hello`, keep the `BodyCatalog`, and on every
//! `TimeState` resync the clock. Each animation frame extrapolates UT from the last `TimeState` at the current
//! warp, places every body from its Keplerian elements, and redraws.

#![forbid(unsafe_code)]

pub mod animation;
pub mod audio;
pub mod authored_particles;
#[cfg(target_arch = "wasm32")]
mod authored_sprite_gpu;
pub mod engine_fx;
#[cfg(target_arch = "wasm32")]
mod engine_fx_gpu;
#[cfg(target_arch = "wasm32")]
pub mod flight3d;
pub mod ftue;
#[cfg(target_arch = "wasm32")]
mod fx_fixture;
#[cfg(target_arch = "wasm32")]
mod fx_post;
pub mod hud;
pub mod input;
pub mod kerbin_time;
#[cfg(target_arch = "wasm32")]
pub mod ksc3d;
#[cfg(target_arch = "wasm32")]
pub mod menu3d;
pub mod missions;
#[cfg(target_arch = "wasm32")]
pub mod navball;
pub mod ocean_fft;
pub mod orientation;
pub mod particles;
#[cfg(target_arch = "wasm32")]
pub mod planet;
pub mod planet_lod;
#[cfg(target_arch = "wasm32")]
pub mod planet_preview;
pub mod planet_scatter;
#[cfg(target_arch = "wasm32")]
pub mod render;
pub mod rnd;
pub mod scene;
pub mod settings;
pub mod thumb;
pub mod tracking;
pub mod training;
pub mod trip;
pub mod tutorials;
pub mod vab;
#[cfg(target_arch = "wasm32")]
pub mod vab3d;

use scene::{Camera, Scene, TimeSync};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasRenderingContext2d, Document, Element, HtmlCanvasElement, HtmlElement, MessageEvent,
    MouseEvent, WebSocket, WheelEvent,
};

use ose_net::{
    Agency, CampaignMember, ClientMessage, DifficultyDefinition, DifficultyOption,
    DifficultyPreset, DifficultyValue, PatchEnd, ServerMessage, SessionInfo, SessionMode,
    PROTOCOL_VERSION, WS_PATH,
};

const BODY_LABEL_CLASS: &str = "body";

struct App {
    doc: Document,
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    labels: Element,
    label_els: Vec<HtmlElement>,
    /// One label per vessel, keyed by vessel id, rebuilt when the vessel list changes.
    vessel_els: Vec<(u64, HtmlElement)>,
    /// Vessel the camera follows instead of the focus body, if any.
    follow: Option<u64>,
    /// The planned burn on the player's vessel, if any (ADR-0005).
    node: Option<Node>,
    node_el: Option<HtmlElement>,
    player_name: String,
    /// True once the server confirmed us as a campaign member (the map is "in session").
    in_session: bool,
    campaign: Option<(String, Vec<CampaignMember>)>,
    crafts: Vec<ose_net::CraftSummary>,
    /// Craft documents fetched for thumbnails, by library id.
    craft_docs: std::collections::HashMap<u64, ose_data::craft::Craft>,
    /// Part geometry from GET /parts.json, by part id: (size_m, center_m). Empty until it loads.
    part_sizes: std::collections::HashMap<String, (ose_math::DVec3, ose_math::DVec3)>,
    /// The full part catalog from /parts.json for the editor.
    catalog: vab::Catalog,
    editor: vab::Editor,
    vab_category: String,
    /// Picker sort (0 default, 1 name, 2 mass), size filter, and favourite parts (middle click; kept per browser).
    vab_sort: u8,
    vab_size: Option<String>,
    vab_favs: std::collections::HashSet<String>,
    /// Boxes of the last editor draw in canvas pixels, for click selection.
    vab_boxes: Vec<(usize, f64, f64, f64, f64)>,
    /// Library id of the craft being edited (set by Edit on the launch pad or after a save).
    vab_craft_id: Option<u64>,
    /// Launch as soon as the next `CraftSaved` arrives.
    launch_after_save: bool,
    difficulty_defs: Vec<DifficultyDefinition>,
    difficulty_presets: Vec<DifficultyPreset>,
    agency_sent: bool,
    warp_holder: Option<u64>,
    /// Breadcrumb text last rendered, to rebuild the DOM only on change.
    crumbs: String,
    ut_el: Element,
    warp_el: Element,
    focus_el: Element,
    status_el: Element,
    scene: Option<Scene>,
    time: TimeSync,
    camera: Camera,
    ws: Option<WebSocket>,
    player_id: Option<u64>,
    drag: Option<(f64, f64)>,
    dpr: f64,
    /// The WebGPU renderer when the browser has an adapter; the 2D canvas is the fallback.
    #[cfg(target_arch = "wasm32")]
    gpu: Option<render::Renderer>,
    /// `?planet=<Body>`: draw only that body from a fixed viewpoint (planet_preview.rs).
    #[cfg(target_arch = "wasm32")]
    planet_preview: Option<planet_preview::Preview>,
    /// The main menu's 3D backdrop (the original's menu scene), drawn while no session is joined.
    #[cfg(target_arch = "wasm32")]
    menu3d: menu3d::MenuBackdrop,
    /// The Kerbal Space Center as a 3D scene (the original's campus), shown behind the facility panel.
    #[cfg(target_arch = "wasm32")]
    ksc3d: ksc3d::KscView,
    /// The Vehicle Assembly Building hall with the craft in 3D (the original's OAB interior).
    #[cfg(target_arch = "wasm32")]
    vab3d: vab3d::VabView,
    /// Where the last mousedown happened, to tell a click from a drag.
    press: Option<(f64, f64)>,
    /// A press on a VAB picker row (part id, where), which becomes a drag after a few pixels; and a flag
    /// that swallows the row's click after a drag.
    vab_press: Option<(String, f64, f64)>,
    /// VAB undo/redo, a placed part pressed with the left button (part index, where), and the subtree lifted
    /// off the craft while it is carried.
    vab_history: vab::History,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    vab_grab: Option<(usize, f64, f64)>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    vab_frag: Option<vab::Fragment>,
    /// Symmetry count for surface parts (X / Shift+X) and angle snap (C), as the original's toolbar.
    vab_symmetry: u32,
    vab_snap: bool,
    /// Ctrl+C clipboard, and whether the fragment in hand is a paste (dropped nowhere it just goes away).
    vab_clipboard: Option<vab::Fragment>,
    vab_pasting: bool,
    /// The held part's turn and its yaw part (W/A/S/D/Q/E while holding, Space resets).
    vab_turn: ose_data::craft::Quat,
    vab_yaw: ose_data::craft::Quat,
    /// The server's staging readout of the craft in the editor (StageReport, protocol 26), and when to ask
    /// again after an edit (ms; asked 300 ms after the last change).
    vab_stages_readout: Vec<ose_net::StageReadout>,
    /// A part pressed on a stage card, to drop on another card.
    vab_stage_drag: Option<usize>,
    /// The trip planner's delta-v map, once loaded.
    trip: Option<trip::TripMap>,
    /// First-time hints: the scripts, the ones already shown (kept per browser) and the messages queued.
    ftue_state: std::cell::RefCell<FtueState>,
    /// The delta-v tool's body for stage TWR (`None`: Kerbin at sea level from the server).
    vab_twr_body: Option<String>,
    /// The server's centre of mass and centre/direction of thrust, and whether their markers show.
    vab_com: Option<ose_math::DVec3>,
    vab_cot: Option<(ose_math::DVec3, ose_math::DVec3)>,
    vab_col: Option<ose_math::DVec3>,
    vab_show_col: bool,
    vab_show_com: bool,
    vab_show_cot: bool,
    /// The part whose manager window is open, and whether this opening's first change went into the history.
    vab_pam: Option<usize>,
    vab_pam_recorded: bool,
    vab_report_due: Option<f64>,
    vab_dragged: bool,
    /// The flight view (the vessel in 3D near its body).
    #[cfg(target_arch = "wasm32")]
    flight3d: flight3d::FlightView,
    #[cfg(target_arch = "wasm32")]
    navball: navball::Navball,
    /// The vessel the player is flying (launched from the pad), the craft it was built from, and whether
    /// the map is shown instead of the flight view (M).
    flying: Option<u64>,
    flight_craft: Option<ose_data::craft::Craft>,
    map_mode: bool,
    /// Craft id of a launch waiting for `CraftLaunched`.
    pending_launch: Option<u64>,
    /// Body spin from `BodyRotation`, index-aligned with the catalog (empty until it arrives).
    body_spins: Vec<ose_net::BodySpin>,
    /// The tech tree, the original's English text (loaded from assets/loc/en.json), the researched nodes
    /// and the node shown in the R&D info panel.
    tech: Vec<rnd::TechNode>,
    loc: std::collections::HashMap<String, String>,
    researched: std::collections::HashSet<String>,
    rd_selected: Option<String>,
    /// The campaign's science from `ScienceState` (protocol 13): tech tree on, points, missions in progress.
    science: Option<SciencePoints>,
    /// Missions, the Mission Control tab (primary or secondary) and the mission shown.
    missions: Vec<missions::Mission>,
    mc_primary: bool,
    mc_selected: Option<String>,
    /// The last Mission Control answer (accepted, turned in, or why not).
    mc_note: String,
    /// Music and sound effects, and the last stage seen in flight (vessel, stage, parts jettisoned) to play
    /// staging and decoupling sounds once.
    audio: audio::Audio,
    last_stage: Option<(u64, u32, usize)>,
    /// The flown vessel's last situation, to play the explosion once when it crashes.
    last_situation: Option<(u64, ose_net::FlightSituation)>,
    /// Timed experiments running on the flown vessel and when the server reported them (ms), for the countdown.
    running: Vec<ose_net::RunningExperiment>,
    running_since_ms: f64,
    /// The live campaign's id (CampaignState), so stored per-vessel data stays with its campaign.
    campaign_id: String,
    /// What the flight sounds last saw, to play each transition once.
    flight_sound_state: Option<FlightSoundState>,
    /// The library craft the flown vessel was launched from, to fill `flight_craft` when its document arrives.
    flight_craft_id: Option<u64>,
    /// The craft a Kerbal asked to board, flown once the server accepts.
    board_target: std::cell::Cell<Option<u64>>,
    /// The crew action awaiting its CrewAck (the HUD button's id).
    crew_pending: std::cell::Cell<Option<&'static str>>,
    /// The vehicle selected on the launch pad.
    lp_selected: Option<u64>,
    /// The Tracking Station: open, the selected vessel, the "mine only" filter and the last list refresh (ms).
    tracking_open: bool,
    ts_selected: Option<u64>,
    ts_mine_only: bool,
    ts_rendered_ms: f64,
    /// Whether the KSC camera is on the launch pad (glides there while the pad dialog is open).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    ksc_on_pad: bool,
    /// Controls as last sent.
    throttle: f64,
    attitude: ose_net::AttitudeMode,
    /// Keys held down (by physical code) for the flight keys that act while held, when the held keys last
    /// moved the controls and were last sent, and the wheel input last sent (throttle, steer, brakes).
    keys: input::Held,
    /// The player's key rebinds (Settings > Input), and the action waiting for its new key.
    remap: input::Remap,
    rebinding: Option<String>,
    keys_tick_ms: f64,
    control_cadence: input::ControlCadence,
    controls_dirty: bool,
    drive_sent: (f32, f32, bool),
    /// Frames counted since `fps_since_ms`, for the FPS counter (Settings > Graphics).
    fps_frames: u32,
    fps_since_ms: f64,
    /// Flight HUD: speed readout in orbit (true) or surface mode, brakes latched from the action bar.
    speed_orbit: bool,
    brakes_latched: bool,
    /// The target vessel (right click its label on the map), the target speed mode, and SAS pointing at the
    /// target (true) or away from it (false), held by resending a fixed direction.
    target: Option<ose_net::TargetId>,
    speed_target: bool,
    /// Altimeter in terrain mode (height above the ground below) instead of sea level.
    alt_terrain: bool,
    /// Warp to a moment (UT) with a name for the toast, stepping the warp down on the way; the UT of the
    /// apoapsis and periapsis markers from the last map frame, for clicks on them.
    warp_to: Option<(f64, String)>,
    warp_to_sent_ms: f64,
    apsis_ut: [f64; 2],
    toasts: hud::Toasts,
    /// The flown vessel as last seen, for event toasts (signal, charge, sphere of influence, orbit, landing).
    flight_watch: std::cell::RefCell<Option<hud::FlightWatch>>,
    /// The flown vessel's records for the flight report.
    flight_records: std::cell::RefCell<Option<hud::FlightRecords>>,
    /// Vessels this client has staged at least once: their first stage is lit, so the next is the one after.
    staged: std::collections::HashSet<u64>,
    roll_sent: f32,
    /// Rotation input last sent with SAS off (protocol 31 SetRotation): pitch, yaw, roll.
    rotation_sent: (f32, f32, f32),
    /// Launch sites from the server (protocol 30) and the one chosen (None: launch pad 1), kept per browser.
    launch_sites: Vec<ose_net::LaunchSite>,
    launch_site: Option<String>,
    /// The campaign's astronauts (`KerbalRoster`), the one shown in the Training Center, and the launch
    /// dialog's crew picks with the craft they were made for.
    roster: Vec<ose_net::KerbalInfo>,
    tc_selected: Option<String>,
    /// Training Center's Lessons tab (Kerbal Space Academy): shown instead of the Kerbals, the lesson picked,
    /// and the lessons from assets/ui/tutorials.json.
    tc_lessons: bool,
    /// VAB offset tool (2): arrows and Q/E move the selected part.
    vab_offset: bool,
    /// VAB frame of reference (F): turn the held part about its own axes instead of the craft's.
    vab_local_frame: bool,
    lesson_selected: Option<String>,
    academy: Option<tutorials::Academy>,
    lesson_run: std::cell::RefCell<Option<LessonRun>>,
    lp_crew: Vec<String>,
    lp_crew_for: Option<u64>,
    /// The loading screen: the place it last saw (None on the main menu) and when it went up.
    loading_place: Option<&'static str>,
    loading_since: Option<f64>,
    /// Take the attitude the server holds from the next readout (after a launch or a vessel switch), so the
    /// first key press turns from there: a plane on the runway holds level along it.
    adopt_attitude: bool,
    /// The flown vessel's part whose window is open (right click in flight).
    flight_pam: Option<usize>,
    /// Docking mode (Delete, the original's ToggleRotLinControls): W/A/S/D and Shift/Ctrl translate instead of
    /// turning and throttling.
    lin_mode: bool,
    translate_sent: (f32, f32, f32),
    /// EVA: run as last sent, and a Space press not yet sent as a jump.
    eva_run_sent: bool,
    eva_jump: bool,
    /// Screen points (CSS px) along the planning vessel's current orbit with their UT, from the last map
    /// frame: a click near one adds a maneuver node there, as clicking an orbit does in the original.
    orbit_pick: Vec<(f64, f64, f64)>,
    /// Maneuver gizmo: each handle's screen direction from the node (CSS px, unit) from the last map frame,
    /// and the handle being dragged (index, press x, y, the node's delta-v components at the press).
    mnv_dirs: [(f64, f64); 6],
    mnv_drag: Option<(usize, f64, f64, [f64; 3])>,
    mnv_sent_ms: f64,
    gpu_canvas: HtmlCanvasElement,
    /// 3D orbit camera: yaw and pitch (radians) around the focus, distance in kilometres.
    orbit_yaw: f64,
    orbit_pitch: f64,
    orbit_dist_km: f64,
    /// Line keys for trajectory strips, versioned so a new prediction replaces the old buffers.
    traj_version: u64,
}

type Shared = Rc<RefCell<App>>;

/// First-time hint state (see `ftue.rs`).
struct FtueState {
    scripts: Option<ftue::Scripts>,
    seen: std::collections::HashSet<String>,
    queue: std::collections::VecDeque<ftue::Message>,
    enabled: bool,
    /// The queue is a Training Center lesson (End Tutorial instead of No more hints).
    lesson: bool,
}

/// A lesson being played: its stage, whether PAIGE has said her piece and the stage waits on the game, and
/// the events seen since the stage began; the VAB's last part placed and picked.
#[derive(Default)]
struct LessonRun {
    id: String,
    stage: usize,
    waiting: bool,
    events: std::collections::HashSet<String>,
    last_placed: Option<String>,
    last_selected: Option<String>,
}

/// The campaign's science as the server last sent it.
struct SciencePoints {
    tech_tree: bool,
    capacity: u32,
    allocated: u32,
    missions: Vec<ose_net::MissionInfo>,
    submitted: usize,
}

/// A maneuver node: an impulsive burn the player is planning. `anchor_ut` is UT when the node was added;
/// the time slider sets `ut = anchor_ut + dt`.
#[derive(Clone, Copy, Debug)]
struct Node {
    vessel: u64,
    anchor_ut: f64,
    ut: f64,
    prograde_mps: f64,
    normal_mps: f64,
    radial_mps: f64,
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn stored(key: &str) -> Option<String> {
    local_storage()?.get_item(key).ok()?
}

fn store(key: &str, value: &str) {
    if let Some(s) = local_storage() {
        s.set_item(key, value).ok();
    }
}

fn input_value(doc: &Document, id: &str) -> Option<f64> {
    doc.get_element_by_id(id)?
        .dyn_into::<web_sys::HtmlInputElement>()
        .ok()?
        .value()
        .parse()
        .ok()
}

fn set_input_value(doc: &Document, id: &str, v: f64) {
    if let Some(el) = doc
        .get_element_by_id(id)
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        el.set_value(&format!("{v}"));
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn mode_name(m: SessionMode) -> &'static str {
    match m {
        SessionMode::Sandbox => "Sandbox",
        SessionMode::Exploration => "Exploration",
    }
}

fn input_string(doc: &Document, id: &str) -> Option<String> {
    doc.get_element_by_id(id)?
        .dyn_into::<web_sys::HtmlInputElement>()
        .ok()
        .map(|i| i.value())
}

fn select_value(doc: &Document, id: &str) -> Option<String> {
    doc.get_element_by_id(id)?
        .dyn_into::<web_sys::HtmlSelectElement>()
        .ok()
        .map(|s| s.value())
}

fn checkbox(doc: &Document, id: &str) -> Option<bool> {
    doc.get_element_by_id(id)?
        .dyn_into::<web_sys::HtmlInputElement>()
        .ok()
        .map(|i| i.checked())
}

/// "#rrggbb" to bytes; anything else is grey.
fn parse_rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        }
    }
    [128, 128, 128]
}

fn rgb_css(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn set_text(doc: &Document, id: &str, text: &str) {
    if let Some(el) = doc.get_element_by_id(id) {
        el.set_text_content(Some(text));
    }
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

fn log(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let window = web_sys::window().ok_or("no window")?;
    let doc = window.document().ok_or("no document")?;
    let get = |id: &str| {
        doc.get_element_by_id(id)
            .ok_or_else(|| JsValue::from_str(&format!("no #{id}")))
    };
    let status_el = get("status")?;
    status_el.set_text_content(Some("OpenSpaceExplorer client loaded"));

    let canvas: HtmlCanvasElement = get("map")?.dyn_into()?;
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or("no 2d context")?
        .dyn_into()?;
    let app: Shared = Rc::new(RefCell::new(App {
        labels: get("labels")?,
        label_els: Vec::new(),
        vessel_els: Vec::new(),
        follow: None,
        node: None,
        node_el: None,
        player_name: String::new(),
        in_session: false,
        campaign: None,
        crafts: Vec::new(),
        craft_docs: std::collections::HashMap::new(),
        part_sizes: std::collections::HashMap::new(),
        catalog: vab::Catalog::new(),
        editor: vab::Editor::default(),
        vab_category: "Pods".into(),
        vab_sort: 0,
        vab_size: None,
        vab_favs: stored("ose.vab.favs")
            .map(|s| {
                s.split(',')
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        vab_boxes: Vec::new(),
        vab_craft_id: None,
        launch_after_save: false,
        difficulty_defs: Vec::new(),
        difficulty_presets: Vec::new(),
        agency_sent: false,
        warp_holder: None,
        crumbs: String::new(),
        ut_el: get("ut")?,
        warp_el: get("warp")?,
        focus_el: get("focus")?,
        status_el,
        scene: None,
        time: TimeSync::default(),
        camera: Camera::new(0),
        ws: None,
        player_id: None,
        drag: None,
        // Settings > Graphics: render resolution as a share of the screen's pixels.
        dpr: window.device_pixel_ratio().max(1.0)
            * settings::render_scale(stored("ose.renderScale").as_deref()),
        #[cfg(target_arch = "wasm32")]
        gpu: None,
        #[cfg(target_arch = "wasm32")]
        planet_preview: window
            .location()
            .search()
            .ok()
            .and_then(|q| planet_preview::Preview::from_query(&q)),
        #[cfg(target_arch = "wasm32")]
        menu3d: menu3d::MenuBackdrop::new(),
        #[cfg(target_arch = "wasm32")]
        ksc3d: ksc3d::KscView::new(),
        #[cfg(target_arch = "wasm32")]
        vab3d: vab3d::VabView::new().with_icon_mode(
            window
                .location()
                .search()
                .is_ok_and(|q| q.contains("particons")),
        ),
        press: None,
        vab_press: None,
        vab_history: vab::History::default(),
        vab_grab: None,
        vab_frag: None,
        vab_symmetry: 1,
        vab_snap: true,
        vab_clipboard: None,
        vab_pasting: false,
        vab_turn: vab::NO_TURN,
        vab_yaw: vab::NO_TURN,
        vab_stages_readout: Vec::new(),
        vab_stage_drag: None,
        trip: None,
        ftue_state: std::cell::RefCell::new(FtueState {
            scripts: None,
            seen: stored("ose.ftue.seen")
                .map(|s| {
                    s.split(',')
                        .filter(|x| !x.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            queue: Default::default(),
            enabled: stored("ose.ftue").as_deref() != Some("off"),
            lesson: false,
        }),
        vab_twr_body: None,
        vab_com: None,
        vab_cot: None,
        vab_col: None,
        vab_show_col: stored("ose.vab.col").as_deref() != Some("off"),
        vab_show_com: stored("ose.vab.com").as_deref() != Some("off"),
        vab_show_cot: stored("ose.vab.cot").as_deref() != Some("off"),
        vab_pam: None,
        vab_pam_recorded: false,
        vab_report_due: None,
        vab_dragged: false,
        #[cfg(target_arch = "wasm32")]
        flight3d: flight3d::FlightView::new(),
        #[cfg(target_arch = "wasm32")]
        navball: navball::Navball::new(),
        flying: None,
        flight_craft: None,
        map_mode: false,
        pending_launch: None,
        body_spins: Vec::new(),
        tech: rnd::tree(),
        loc: std::collections::HashMap::new(),
        researched: ["tNode_1v_start".to_string()].into_iter().collect(),
        rd_selected: None,
        science: None,
        missions: missions::all(),
        mc_primary: true,
        mc_selected: None,
        mc_note: String::new(),
        audio: audio::Audio::new(stored("ose.sound").as_deref() == Some("off")),
        last_stage: None,
        last_situation: None,
        running: Vec::new(),
        running_since_ms: 0.0,
        lp_selected: None,
        campaign_id: String::new(),
        flight_sound_state: None,
        flight_craft_id: None,
        board_target: std::cell::Cell::new(None),
        crew_pending: std::cell::Cell::new(None),
        tracking_open: false,
        ts_selected: None,
        ts_mine_only: false,
        ts_rendered_ms: 0.0,
        ksc_on_pad: false,
        throttle: 0.0,
        attitude: ose_net::AttitudeMode::Up,
        keys: input::Held::default(),
        remap: input::Remap::parse(&stored("ose.keys").unwrap_or_default()),
        rebinding: None,
        keys_tick_ms: 0.0,
        control_cadence: input::ControlCadence::default(),
        controls_dirty: false,
        drive_sent: (0.0, 0.0, false),
        fps_frames: 0,
        fps_since_ms: 0.0,
        speed_orbit: false,
        brakes_latched: false,
        target: None,
        speed_target: false,
        alt_terrain: false,
        warp_to: None,
        warp_to_sent_ms: 0.0,
        apsis_ut: [f64::NAN; 2],
        toasts: hud::Toasts::default(),
        flight_watch: std::cell::RefCell::new(None),
        flight_records: std::cell::RefCell::new(None),
        staged: Default::default(),
        roll_sent: 0.0,
        rotation_sent: (0.0, 0.0, 0.0),
        launch_sites: Vec::new(),
        launch_site: stored("ose.launchSite"),
        loading_place: None,
        roster: Vec::new(),
        tc_selected: None,
        tc_lessons: false,
        vab_offset: false,
        vab_local_frame: false,
        lesson_selected: None,
        academy: None,
        lesson_run: std::cell::RefCell::new(None),
        lp_crew: Vec::new(),
        lp_crew_for: None,
        loading_since: None,
        adopt_attitude: false,
        lin_mode: false,
        flight_pam: None,
        translate_sent: (0.0, 0.0, 0.0),
        eva_run_sent: false,
        eva_jump: false,
        orbit_pick: Vec::new(),
        mnv_dirs: [(0.0, 0.0); 6],
        mnv_drag: None,
        mnv_sent_ms: 0.0,
        gpu_canvas: get("gpu")?.dyn_into()?,
        orbit_yaw: 0.6,
        orbit_pitch: 0.5,
        orbit_dist_km: 5.0e7,
        traj_version: 0,
        doc,
        canvas,
        ctx,
    }));

    app.borrow_mut().resize();
    install_input(&app)?;
    install_label_clicks(&app)?;
    install_menu(&app)?;
    load_part_sizes(app.clone());
    load_localization(app.clone());
    load_trip_planner(app.clone());
    load_ftue(app.clone());
    load_tutorials(app.clone());
    load_sounds(app.clone());
    install_audio(&app)?;
    #[cfg(target_arch = "wasm32")]
    start_webgpu(app.clone());
    animate(app);
    Ok(())
}

/// Create the wgpu renderer on the `#gpu` canvas; on success the page switches to the 3D view.
#[cfg(target_arch = "wasm32")]
fn start_webgpu(app: Shared) {
    let has_gpu = js_sys::Reflect::get(&web_sys::window().unwrap().navigator(), &"gpu".into())
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false);
    if !has_gpu {
        log("no navigator.gpu: staying on the 2D map");
        return;
    }
    let canvas = app.borrow().gpu_canvas.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match render::Renderer::new(canvas).await {
            Some(r) => {
                let mut a = app.borrow_mut();
                a.gpu = Some(r);
                if let Some(body) = a.doc.body() {
                    body.class_list().add_1("webgpu").ok();
                }
                a.gpu_canvas
                    .set_attribute("data-ose-renderer", "webgpu")
                    .ok();
                a.resize();
                log("webgpu renderer ready");
            }
            None => log("webgpu unavailable: staying on the 2D map"),
        }
    });
}

/// Fetch the part catalog (GET /parts.json) in the background: sizes for thumbnails, everything for the
/// editor. A missing route leaves the default cubes in place and the VAB picker empty.
/// Fetch the original's English text (assets/loc/en.json, exported by tools/export_localization.py); screens
/// fall back to the localization term when it is missing.
/// The sound list (`audio/sounds.json`, `tools/export_audio.py`); without it the client is silent.
fn load_sounds(app: Shared) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(resp) =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_str("assets/audio/sounds.json"))
                .await
        else {
            return;
        };
        let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
            return;
        };
        if !resp.ok() {
            return;
        }
        let Ok(text) = resp.text() else { return };
        let Ok(text) = wasm_bindgen_futures::JsFuture::from(text).await else {
            return;
        };
        let Some(text) = text.as_string() else { return };
        app.borrow_mut()
            .audio
            .set_manifest(audio::parse_manifest(&text));
    });
}

/// Audio starts on the first gesture; buttons click like the original's (its button-select sound), and the
/// sound toggle mutes everything.
fn install_audio(app: &Shared) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let a = app.clone();
    let unlock = Closure::<dyn FnMut()>::new(move || {
        if let Ok(mut app) = a.try_borrow_mut() {
            if !app.audio.unlocked() {
                app.audio.unlock();
            }
        }
    });
    for ev in ["pointerdown", "keydown"] {
        window.add_event_listener_with_callback_and_bool(
            ev,
            unlock.as_ref().unchecked_ref(),
            true,
        )?;
    }
    unlock.forget();
    let a = app.clone();
    let click = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let Some(el) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        let Some(button) = el.closest("button:not([disabled])").ok().flatten() else {
            return;
        };
        let Ok(mut app) = a.try_borrow_mut() else {
            return;
        };
        let key = match button.id().as_str() {
            "rd-research" => "rd_research",
            "sound-toggle" | "main-sound" => return,
            _ => "ui_select",
        };
        app.audio.play(key);
    });
    window.add_event_listener_with_callback_and_bool(
        "click",
        click.as_ref().unchecked_ref(),
        true,
    )?;
    click.forget();
    let doc = app.borrow().doc.clone();
    for btn in ["sound-toggle", "main-sound", "settings-sound"]
        .iter()
        .filter_map(|id| doc.get_element_by_id(id))
    {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut app = a.borrow_mut();
            let muted = !app.audio.muted;
            app.audio.set_muted(muted);
            store("ose.sound", if muted { "off" } else { "on" });
            app.show_sound_toggle();
        });
        btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    app.borrow().show_sound_toggle();
    install_settings(app)?;
    // Key rebinding: a click on an action's key waits for the next key press.
    app.borrow().render_rebind();
    if let Some(el) = app.borrow().doc.get_element_by_id("settings-rebind") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let t = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            let mut app = a.borrow_mut();
            if let Some(d) = t.as_ref().and_then(|e| e.get_attribute("data-rebind")) {
                app.rebinding = Some(d);
                app.render_rebind();
            } else if t.as_ref().is_some_and(|e| e.id() == "rebind-reset") {
                app.remap = input::Remap::default();
                store("ose.keys", "");
                app.render_rebind();
            }
        });
        el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // Graphics: planet detail (low, medium, high), applied at once.
    #[cfg(target_arch = "wasm32")]
    {
        let doc = app.borrow().doc.clone();
        let q = stored("ose.planetQuality");
        crate::planet::set_quality(settings::planet_quality(q.as_deref()));
        if let Some(sel) = doc
            .get_element_by_id("settings-quality")
            .and_then(|e| e.dyn_into::<web_sys::HtmlSelectElement>().ok())
        {
            sel.set_value(q.as_deref().unwrap_or("high"));
            let s2 = sel.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let v = s2.value();
                store("ose.planetQuality", &v);
                crate::planet::set_quality(settings::planet_quality(Some(&v)));
            });
            sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    // Graphics: render resolution (applied at once) and the FPS counter.
    {
        let doc = app.borrow().doc.clone();
        let scale = settings::render_scale(stored("ose.renderScale").as_deref());
        set_text(&doc, "settings-res-text", &format!("{:.0}%", scale * 100.0));
        if let Some(input) = doc
            .get_element_by_id("settings-res")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            input.set_value(&format!("{:.0}", scale * 100.0));
            let a = app.clone();
            let i2 = input.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let v = i2.value();
                store("ose.renderScale", &v);
                let scale = settings::render_scale(Some(&v));
                let mut app = a.borrow_mut();
                set_text(
                    &app.doc,
                    "settings-res-text",
                    &format!("{:.0}%", scale * 100.0),
                );
                let ratio = web_sys::window()
                    .map(|w| w.device_pixel_ratio())
                    .unwrap_or(1.0)
                    .max(1.0);
                app.dpr = ratio * scale;
                app.resize();
            });
            input.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
        let fps = stored("ose.fps").as_deref() == Some("on");
        if let Some(b) = doc.body() {
            b.class_list().toggle_with_force("show-fps", fps).ok();
        }
        if let Some(input) = doc
            .get_element_by_id("settings-fps")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            input.set_checked(fps);
            let d = doc.clone();
            let i2 = input.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let on = i2.checked();
                store("ose.fps", if on { "on" } else { "off" });
                if let Some(b) = d.body() {
                    b.class_list().toggle_with_force("show-fps", on).ok();
                }
            });
            input.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    // Volume sliders (Settings > Audio), kept per browser.
    {
        let pct = |key: &str| {
            stored(key)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(100.0)
                .clamp(0.0, 100.0)
        };
        let (music, sfx) = (pct("ose.vol.music"), pct("ose.vol.sfx"));
        app.borrow_mut()
            .audio
            .set_volumes(music / 100.0, sfx / 100.0);
        let doc = app.borrow().doc.clone();
        for (id, key, v) in [
            ("settings-music", "ose.vol.music", music),
            ("settings-sfx", "ose.vol.sfx", sfx),
        ] {
            set_text(&doc, &format!("{id}-text"), &format!("{v:.0}%"));
            let Some(input) = doc
                .get_element_by_id(id)
                .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                continue;
            };
            input.set_value(&format!("{v:.0}"));
            let a = app.clone();
            let i2 = input.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let v = i2.value_as_number().clamp(0.0, 100.0);
                store(key, &format!("{v:.0}"));
                let mut app = a.borrow_mut();
                set_text(&app.doc, &format!("{id}-text"), &format!("{v:.0}%"));
                let (m, s) = (app.audio.music_scale, app.audio.sfx_scale);
                if id == "settings-music" {
                    app.audio.set_volumes(v / 100.0, s);
                } else {
                    app.audio.set_volumes(m, v / 100.0);
                }
            });
            input.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    install_rnd_nav(&app.borrow().doc.clone())?;
    // Maneuver handles take their own presses (they sit above vessel labels, which take clicks too).
    if let Some(labels) = app.borrow().doc.get_element_by_id("labels") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let target = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            let on = |sel: &str| {
                target
                    .as_ref()
                    .and_then(|e| e.closest(sel).ok().flatten())
                    .is_some()
            };
            if ev.button() != 0 {
                return;
            }
            if on(".mnv-h") {
                ev.prevent_default();
                ev.stop_propagation();
                a.borrow_mut()
                    .mnv_press(ev.client_x() as f64, ev.client_y() as f64);
            } else if on(".node") {
                // The node itself slides along the orbit (the original's DoMove).
                ev.prevent_default();
                ev.stop_propagation();
                let mut app = a.borrow_mut();
                if let Some(n) = app.node {
                    app.mnv_drag = Some((
                        6,
                        ev.client_x() as f64,
                        ev.client_y() as f64,
                        [n.prograde_mps, n.normal_mps, n.radial_mps],
                    ));
                }
            }
        });
        labels.add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // Any [data-save-action="QuickSave|QuickLoad|RevertToLaunch|RevertToEditor"] element asks the server.
    let a = app.clone();
    let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let action = ev
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|e| e.closest("[data-save-action]").ok().flatten())
            .and_then(|e| e.get_attribute("data-save-action"));
        let action = match action.as_deref() {
            Some("QuickSave") => ose_net::SaveAction::QuickSave,
            Some("QuickLoad") => ose_net::SaveAction::QuickLoad,
            Some("RevertToLaunch") => ose_net::SaveAction::RevertToLaunch,
            Some("RevertToEditor") => ose_net::SaveAction::RevertToEditor,
            _ => return,
        };
        if let Ok(app) = a.try_borrow() {
            app.save_action(action);
        }
    });
    app.borrow()
        .doc
        .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
    cb.forget();
    Ok(())
}

/// R&D tree navigation as the original's (`RDInputDefinition`): drag with the left button to pan, wheel to
/// zoom about the pointer, arrow keys to pan. A drag does not count as a click on a node.
fn install_rnd_nav(doc: &Document) -> Result<(), JsValue> {
    use std::cell::Cell;
    let Some(tree) = doc
        .get_element_by_id("rd-tree")
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
    else {
        return Ok(());
    };
    // (pointer x, y, scroll left, top) at the press, and whether it moved far enough to be a drag.
    let press = Rc::new(Cell::new(None::<(f64, f64, f64, f64)>));
    let dragged = Rc::new(Cell::new(false));
    let zoom = Rc::new(Cell::new(1.0f64));

    let (p, d) = (press.clone(), dragged.clone());
    let t = tree.clone();
    let down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if ev.button() == 0 {
            p.set(Some((
                ev.client_x() as f64,
                ev.client_y() as f64,
                t.scroll_left() as f64,
                t.scroll_top() as f64,
            )));
            d.set(false);
        }
    });
    tree.add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref())?;
    down.forget();

    let (p, d) = (press.clone(), dragged.clone());
    let t = tree.clone();
    let mv = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let Some((x0, y0, sl, st)) = p.get() else {
            return;
        };
        let (dx, dy) = (ev.client_x() as f64 - x0, ev.client_y() as f64 - y0);
        if dx.abs() + dy.abs() > 4.0 {
            d.set(true);
            t.class_list().add_1("dragging").ok();
        }
        if d.get() {
            t.set_scroll_left((sl - dx) as i32);
            t.set_scroll_top((st - dy) as i32);
        }
    });
    if let Some(w) = web_sys::window() {
        w.add_event_listener_with_callback("mousemove", mv.as_ref().unchecked_ref())?;
    }
    mv.forget();

    let p = press.clone();
    let t = tree.clone();
    let up = Closure::<dyn FnMut(MouseEvent)>::new(move |_: MouseEvent| {
        p.set(None);
        t.class_list().remove_1("dragging").ok();
    });
    if let Some(w) = web_sys::window() {
        w.add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref())?;
    }
    up.forget();

    // A click that ends a drag is swallowed before it reaches the node handler.
    let d = dragged.clone();
    let swallow = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if d.get() {
            ev.stop_propagation();
            ev.prevent_default();
            d.set(false);
        }
    });
    tree.add_event_listener_with_callback_and_bool(
        "click",
        swallow.as_ref().unchecked_ref(),
        true,
    )?;
    swallow.forget();

    let t = tree.clone();
    let z = zoom.clone();
    let wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
        ev.prevent_default();
        let old = z.get();
        let new = (old * if ev.delta_y() > 0.0 { 0.9 } else { 1.1 }).clamp(0.4, 1.6);
        z.set(new);
        // Keep the point under the pointer where it is.
        let rect = t.get_bounding_client_rect();
        let (cx, cy) = (
            ev.client_x() as f64 - rect.left(),
            ev.client_y() as f64 - rect.top(),
        );
        let k = new / old;
        let (sl, st) = (t.scroll_left() as f64, t.scroll_top() as f64);
        t.style()
            .set_property("--rd-zoom", &format!("{new:.3}"))
            .ok();
        t.set_scroll_left(((sl + cx) * k - cx) as i32);
        t.set_scroll_top(((st + cy) * k - cy) as i32);
    });
    tree.add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())?;
    wheel.forget();

    let t = tree.clone();
    let d2 = doc.clone();
    let keys =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let open = d2
                .query_selector("#menu:not(.hidden) #screen-rnd.open")
                .ok()
                .flatten()
                .is_some();
            if !open {
                return;
            }
            let (dx, dy) = match ev.code().as_str() {
                "ArrowLeft" => (-80, 0),
                "ArrowRight" => (80, 0),
                "ArrowUp" => (0, -80),
                "ArrowDown" => (0, 80),
                _ => return,
            };
            ev.prevent_default();
            t.set_scroll_left(t.scroll_left() + dx);
            t.set_scroll_top(t.scroll_top() + dy);
        });
    if let Some(w) = web_sys::window() {
        w.add_event_listener_with_callback("keydown", keys.as_ref().unchecked_ref())?;
    }
    keys.forget();
    Ok(())
}

/// The settings overlay: any `[data-open-settings]` element opens it; tabs, UI scale and key hints, all kept
/// in localStorage and applied at start.
fn install_settings(app: &Shared) -> Result<(), JsValue> {
    let doc = app.borrow().doc.clone();
    if let Some(el) = doc.get_element_by_id("settings-input") {
        el.set_inner_html(&format!(
            "<div id=\"settings-rebind\" data-ose=\"settings-rebind\"></div>{}",
            settings::input_html()
        ));
    }
    let apply_scale = |doc: &Document, pct: f64| {
        if let Some(root) = doc
            .document_element()
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            root.style()
                .set_property(
                    "--u",
                    &format!(
                        "calc(min(calc(100vh / 1080), calc(100vw / 1920)) * {:.2})",
                        pct / 100.0
                    ),
                )
                .ok();
        }
        set_text(doc, "settings-scale-text", &format!("{pct:.0}%"));
    };
    let scale = settings::ui_scale(stored("ose.uiScale").as_deref());
    apply_scale(&doc, scale);
    if let Some(input) = doc
        .get_element_by_id("settings-scale")
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.set_value(&format!("{scale:.0}"));
        let d = doc.clone();
        let i2 = input.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let pct = settings::ui_scale(Some(&i2.value()));
            store("ose.uiScale", &format!("{pct:.0}"));
            apply_scale(&d, pct);
        });
        input.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    let hints = stored("ose.hints").as_deref() != Some("off");
    if let Some(b) = doc.body() {
        b.class_list().toggle_with_force("no-hints", !hints).ok();
    }
    if let Some(input) = doc
        .get_element_by_id("settings-hints")
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.set_checked(hints);
        let d = doc.clone();
        let i2 = input.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let on = i2.checked();
            store("ose.hints", if on { "on" } else { "off" });
            if let Some(b) = d.body() {
                b.class_list().toggle_with_force("no-hints", !on).ok();
            }
        });
        input.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // Open from any [data-open-settings], close with the X; tabs switch sections.
    let d = doc.clone();
    let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let Some(el) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        if el.closest("[data-open-settings]").ok().flatten().is_some() {
            show_settings(&d, true);
        } else if el.closest("#settings-close").ok().flatten().is_some() {
            show_settings(&d, false);
        } else if let Some(tab) = el
            .closest("#settings-tabs [data-tab]")
            .ok()
            .flatten()
            .and_then(|t| t.get_attribute("data-tab"))
        {
            if let Ok(all) = d.query_selector_all("#settings [data-tab]") {
                for k in 0..all.length() {
                    if let Some(e) = all.item(k).and_then(|n| n.dyn_into::<Element>().ok()) {
                        let on = e.get_attribute("data-tab").as_deref() == Some(tab.as_str());
                        e.class_list().toggle_with_force("on", on).ok();
                    }
                }
            }
        }
    });
    doc.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
    cb.forget();
    Ok(())
}

fn show_settings(doc: &Document, open: bool) {
    if let Some(el) = doc.get_element_by_id("settings") {
        el.class_list().toggle_with_force("open", open).ok();
    }
}

fn settings_open(doc: &Document) -> bool {
    doc.get_element_by_id("settings")
        .map(|e| e.class_list().contains("open"))
        .unwrap_or(false)
}

/// What a vessel without a craft looks like; a Kerbal on EVA wears the head their roster looks give.
#[cfg(target_arch = "wasm32")]
fn stand_of(roster: &[ose_net::KerbalInfo], v: &ose_net::VesselState) -> flight3d::Stand {
    match flight3d::Stand::of(v) {
        flight3d::Stand::Kerbal { .. } => {
            let k = roster
                .iter()
                .find(|k| k.status == ose_net::KerbalStatus::Eva { vessel: v.id });
            let look = |key: &str| {
                k.and_then(|k| k.looks.iter().find(|(a, _)| a == key))
                    .map(|(_, b)| b.as_str())
            };
            let skin = look("SKINCOLOR")
                .filter(|h| h.len() == 6)
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
                .unwrap_or(flight3d::DEFAULT_SKIN);
            flight3d::Stand::Kerbal {
                round: look("TYPE") == Some("CIRCLE"),
                skin,
            }
        }
        s => s,
    }
}

/// The flown vessel's state the flight sounds react to.
struct FlightSoundState {
    vessel: u64,
    supersonic: bool,
    gear_out: bool,
    panels: bool,
    hot: Vec<u16>,
    chutes: Vec<(u16, ose_net::ChuteState)>,
    bays: Vec<(u16, bool)>,
    fairings_gone: usize,
    /// When the last footstep played (page ms).
    step_at: f64,
}

/// An experiment id as a readable name: "TemperatureScan" reads "Temperature Scan".
fn experiment_name(id: &str) -> String {
    let mut out = String::new();
    for (i, c) in id.trim_start_matches("Experiment_").chars().enumerate() {
        if c == '_' {
            out.push(' ');
        } else {
            if i > 0 && c.is_uppercase() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(c);
        }
    }
    out
}

/// The Kerbal Space Academy lessons (assets/ui/tutorials.json); the Lessons tab stays empty without it.
fn load_tutorials(app: Shared) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(resp) =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_str("assets/ui/tutorials.json"))
                .await
        else {
            return;
        };
        let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
            return;
        };
        if !resp.ok() {
            return;
        }
        let Ok(text) = resp.text() else { return };
        let Ok(text) = wasm_bindgen_futures::JsFuture::from(text).await else {
            return;
        };
        let Some(text) = text.as_string() else { return };
        if let Some(a) = tutorials::parse(&text) {
            app.borrow_mut().academy = Some(a);
        }
    });
}

/// The first-time hints (assets/ui/ftue.json); none show without it.
fn load_ftue(app: Shared) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(resp) =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_str("assets/ui/ftue.json"))
                .await
        else {
            return;
        };
        let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
            return;
        };
        if !resp.ok() {
            return;
        }
        let Ok(text) = resp.text() else { return };
        let Ok(text) = wasm_bindgen_futures::JsFuture::from(text).await else {
            return;
        };
        let Some(text) = text.as_string() else { return };
        if let Some(scripts) = ftue::parse(&text) {
            app.borrow().ftue_state.borrow_mut().scripts = Some(scripts);
        }
    });
}

/// The trip planner's delta-v map (assets/ui/trip_planner.json); the Trip button stays hidden without it.
fn load_trip_planner(app: Shared) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(resp) = wasm_bindgen_futures::JsFuture::from(
            window.fetch_with_str("assets/ui/trip_planner.json"),
        )
        .await
        else {
            return;
        };
        let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
            return;
        };
        if !resp.ok() {
            return;
        }
        let Ok(text) = resp.text() else { return };
        let Ok(text) = wasm_bindgen_futures::JsFuture::from(text).await else {
            return;
        };
        let Some(text) = text.as_string() else { return };
        if let Some(map) = trip::TripMap::parse(&text) {
            let mut a = app.borrow_mut();
            a.trip = Some(map);
            a.render_trip_options();
        }
    });
}

fn load_localization(app: Shared) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(resp) =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_str("assets/loc/en.json")).await
        else {
            return;
        };
        let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
            return;
        };
        if !resp.ok() {
            return;
        }
        let Ok(text) = resp.text() else { return };
        let Ok(text) = wasm_bindgen_futures::JsFuture::from(text).await else {
            return;
        };
        let Some(text) = text.as_string() else { return };
        if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(&text) {
            let mut a = app.borrow_mut();
            a.loc = map;
            a.name_untitled_parts();
            if a.rd_selected.is_some() {
                a.render_rnd();
            }
            if a.mc_selected.is_some() {
                a.render_missions();
            }
        }
    });
}

fn load_part_sizes(app: Shared) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(resp) =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_str("parts.json")).await
        else {
            return;
        };
        let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
            return;
        };
        // Until the server has the route, the static fallback answers with the index page.
        let is_json = resp
            .headers()
            .get("content-type")
            .ok()
            .flatten()
            .map(|t| t.contains("json"))
            .unwrap_or(false);
        if !resp.ok() || !is_json {
            return;
        }
        let Ok(text) = resp.text() else {
            return;
        };
        let Ok(text) = wasm_bindgen_futures::JsFuture::from(text).await else {
            return;
        };
        let Some(text) = text.as_string() else {
            return;
        };
        match serde_json::from_str::<Vec<vab::PartSpec>>(&text) {
            Ok(parts) => {
                let mut a = app.borrow_mut();
                for p in parts {
                    if let Some(size) = p.size_m {
                        a.part_sizes.insert(
                            p.id.clone(),
                            (size, p.center_m.unwrap_or(ose_math::DVec3::ZERO)),
                        );
                    }
                    a.catalog.insert(p.id.clone(), p);
                }
                a.name_untitled_parts();
                log(&format!(
                    "part catalog loaded for {} parts",
                    a.catalog.len()
                ));
                a.draw_thumbnails();
                a.render_vab_picker();
            }
            Err(e) => log(&format!("parts.json: {e}")),
        }
    });
}

/// The main menu: a name and Play. Play connects; Esc reopens the menu (then it only resumes).
fn install_menu(app: &Shared) -> Result<(), JsValue> {
    let doc = app.borrow().doc.clone();
    if let Some(name) = stored("ose.name") {
        if let Some(input) = doc
            .get_element_by_id("player-name")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            input.set_value(&name);
        }
    }
    if let Some(btn) = doc.get_element_by_id("play") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let connected = a.borrow().ws.is_some();
            if connected {
                a.borrow_mut().open_sessions_screen();
                return;
            }
            let name = a
                .borrow()
                .doc
                .get_element_by_id("player-name")
                .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|i| i.value().trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Pilot".into());
            store("ose.name", &name);
            a.borrow_mut().player_name = name;
            set_text(&a.borrow().doc, "menu-note", "connecting...");
            if let Err(e) = connect(&a) {
                set_text(
                    &a.borrow().doc,
                    "menu-note",
                    &format!("could not connect: {e:?}"),
                );
            }
        });
        btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(input) = doc.get_element_by_id("player-name") {
        let a = app.clone();
        let cb =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" {
                    if let Some(btn) = a
                        .borrow()
                        .doc
                        .get_element_by_id("play")
                        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                    {
                        btn.click();
                    }
                }
            });
        input.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    let wire = |id: &str, f: Box<dyn Fn(&Shared)>| -> Result<(), JsValue> {
        if let Some(el) = doc.get_element_by_id(id) {
            let a = app.clone();
            let cb = Closure::<dyn FnMut()>::new(move || f(&a));
            el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
        Ok(())
    };
    wire(
        "sessions-refresh",
        Box::new(|a| a.borrow_mut().open_sessions_screen()),
    )?;
    wire(
        "sessions-back",
        Box::new(|a| a.borrow().menu_screen("name")),
    )?;
    wire("to-sessions", Box::new(|a| a.borrow_mut().leave_session()))?;
    wire("resume", Box::new(|a| a.borrow().show_menu(false)))?;
    wire("ksc-sessions", Box::new(|a| a.borrow_mut().leave_session()))?;
    wire(
        "to-ksc",
        Box::new(|a| {
            a.borrow_mut().end_flight();
            a.borrow().open_ksc()
        }),
    )?;
    wire(
        "to-tracking",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.end_flight();
            app.open_tracking();
        }),
    )?;
    wire("fh-stage-btn", Box::new(|a| a.borrow_mut().flight_stage()))?;
    wire("fh-full", Box::new(|a| a.borrow_mut().set_throttle(1.0)))?;
    wire("fh-cut", Box::new(|a| a.borrow_mut().set_throttle(0.0)))?;
    wire(
        "fh-map",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.map_mode = !app.map_mode;
            if app.map_mode {
                app.ftue("FTUE_Map");
                app.lesson_event("MapViewEnteredMessage");
            }
        }),
    )?;
    wire(
        "fh-ksc",
        Box::new(|a| {
            a.borrow_mut().end_flight();
            a.borrow().open_ksc()
        }),
    )?;
    // SAS modes and the action bar: one listener per container, the button id says which.
    hud::install(&doc);
    for container in ["fh-sas", "fh-actions"] {
        if let Some(el) = doc.get_element_by_id(container) {
            let a = app.clone();
            let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
                let id = ev
                    .target()
                    .and_then(|t| t.dyn_into::<Element>().ok())
                    .and_then(|e| e.closest("button").ok().flatten())
                    .map(|b| b.id());
                if let Some(id) = id {
                    a.borrow_mut().hud_button(&id);
                }
            });
            el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    wire(
        "fh-alt-mode",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.alt_terrain = !app.alt_terrain;
        }),
    )?;
    wire(
        "fh-missions-btn",
        Box::new(|a| {
            let doc = a.borrow().doc.clone();
            if let Some(el) = doc.get_element_by_id("fh-missions") {
                let open = el.class_list().toggle("open").unwrap_or(false);
                store("ose.tracker", if open { "open" } else { "closed" });
                if open {
                    for id in ["fh-parts", "fh-res", "fh-ag"] {
                        if let Some(e) = doc.get_element_by_id(id) {
                            e.class_list().remove_1("open").ok();
                        }
                    }
                    for id in ["fh-parts-btn", "fh-res-btn", "fh-ag-btn"] {
                        if let Some(e) = doc.get_element_by_id(id) {
                            e.class_list().remove_1("on").ok();
                        }
                    }
                }
                if let Some(b) = doc.get_element_by_id("fh-missions-btn") {
                    b.class_list().toggle_with_force("on", open).ok();
                }
            }
        }),
    )?;
    if stored("ose.tracker").as_deref() == Some("open") {
        for id in ["fh-missions", "fh-missions-btn"] {
            if let Some(el) = doc.get_element_by_id(id) {
                el.class_list()
                    .add_1(if id == "fh-missions" { "open" } else { "on" })
                    .ok();
            }
        }
    }
    wire(
        "fh-report-btn",
        Box::new(|a| {
            let app = a.borrow();
            let open = app
                .doc
                .get_element_by_id("flight-report")
                .is_some_and(|e| e.class_list().contains("open"));
            if open {
                if let Some(e) = app.doc.get_element_by_id("flight-report") {
                    e.class_list().remove_1("open").ok();
                }
            } else {
                app.show_flight_report("In flight", None);
            }
        }),
    )?;
    // Kerbal portraits: shown unless hidden from the app bar (kept per browser).
    let crew_off = stored("ose.crewPortraits").as_deref() == Some("off");
    if let Some(el) = doc.get_element_by_id("fh-crewbox") {
        el.class_list().toggle_with_force("off", crew_off).ok();
    }
    if let Some(el) = doc.get_element_by_id("fh-crew-btn") {
        el.class_list().toggle_with_force("on", !crew_off).ok();
    }
    const APP_WINDOWS: [(&str, &str); 4] = [
        ("fh-missions-btn", "fh-missions"),
        ("fh-parts-btn", "fh-parts"),
        ("fh-res-btn", "fh-res"),
        ("fh-ag-btn", "fh-ag"),
    ];
    for (btn, win) in [
        ("fh-parts-btn", "fh-parts"),
        ("fh-res-btn", "fh-res"),
        ("fh-ag-btn", "fh-ag"),
    ] {
        wire(
            btn,
            Box::new(move |a| {
                let app = a.borrow();
                if let Some(el) = app.doc.get_element_by_id(win) {
                    let open = el.class_list().toggle("open").unwrap_or(false);
                    if let Some(b) = app.doc.get_element_by_id(btn) {
                        b.class_list().toggle_with_force("on", open).ok();
                    }
                    // The app windows share a place: opening one closes the others.
                    if open {
                        for (b2, w2) in APP_WINDOWS.iter().filter(|(_, w)| *w != win) {
                            for (id, class) in [(*w2, "open"), (*b2, "on")] {
                                if let Some(e) = app.doc.get_element_by_id(id) {
                                    e.class_list().remove_1(class).ok();
                                }
                            }
                        }
                        store("ose.tracker", "closed");
                    }
                }
                app.render_parts_manager();
                app.render_resource_manager();
                app.render_action_groups();
            }),
        )?;
    }
    // An action group button toggles that group, as its key does.
    if let Some(list) = doc.get_element_by_id("fh-ag-list") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let g = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-ag]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ag"));
            let group = match g.as_deref() {
                Some("lights") => ose_net::ActionGroup::Lights,
                Some("rcs") => ose_net::ActionGroup::Rcs,
                Some("abort") => ose_net::ActionGroup::Abort,
                Some(n) => match n.parse::<u8>() {
                    Ok(n) => ose_net::ActionGroup::Custom(n),
                    Err(_) => return,
                },
                None => return,
            };
            a.borrow().action_group(group);
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // A parts manager row opens that part's window beside the list.
    if let Some(list) = doc.get_element_by_id("fh-parts-list") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let row = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-part]").ok().flatten());
            let Some(i) = row
                .and_then(|r| r.get_attribute("data-part"))
                .and_then(|v| v.parse::<usize>().ok())
            else {
                return;
            };
            let x = (ev.client_x() as f64 - 620.0).max(10.0);
            a.borrow_mut()
                .flight_open_pam(Some(i), x, ev.client_y() as f64);
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "fh-crew-btn",
        Box::new(|a| {
            let doc = a.borrow().doc.clone();
            if let Some(el) = doc.get_element_by_id("fh-crewbox") {
                let off = el.class_list().toggle("off").unwrap_or(false);
                store("ose.crewPortraits", if off { "off" } else { "on" });
                if let Some(b) = doc.get_element_by_id("fh-crew-btn") {
                    b.class_list().toggle_with_force("on", !off).ok();
                }
            }
        }),
    )?;
    wire(
        "fh-speed-mode",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            // Surface, orbit, then target when one is set, as the original's speed mode button.
            match (app.speed_orbit, app.speed_target) {
                (false, false) => app.speed_orbit = true,
                (true, false) if app.target.is_some() => {
                    app.speed_orbit = false;
                    app.speed_target = true;
                }
                _ => {
                    app.speed_orbit = false;
                    app.speed_target = false;
                }
            }
        }),
    )?;
    wire("ksc-button", Box::new(|a| a.borrow().open_ksc()))?;
    wire("ksc-tracking", Box::new(|a| a.borrow_mut().open_tracking()))?;
    wire(
        "ts-back",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.tracking_open = false;
            app.open_ksc();
        }),
    )?;
    for (id, mine) in [("ts-tab-all", false), ("ts-tab-mine", true)] {
        wire(
            id,
            Box::new(move |a| {
                let mut app = a.borrow_mut();
                app.ts_mine_only = mine;
                app.render_tracking();
            }),
        )?;
    }
    if let Some(panel) = doc.get_element_by_id("tracking") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let Some(el) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
                return;
            };
            let num = |attr: &str| {
                el.closest(&format!("[{attr}]"))
                    .ok()
                    .flatten()
                    .and_then(|e| e.get_attribute(attr))
                    .and_then(|s| s.parse::<u64>().ok())
            };
            let mut app = a.borrow_mut();
            if let Some(id) = num("data-ose-ts-focus") {
                app.track_focus(id);
            } else if let Some(id) = num("data-ose-ts-fly") {
                app.tracking_open = false;
                let craft = stored(&app.craft_link_key(id)).and_then(|s| s.parse::<u64>().ok());
                app.start_flight(id, craft);
            } else if let Some(id) = num("data-ose-ts-recover") {
                if let Some(ws) = &app.ws {
                    send(ws, &ClientMessage::RecoverVessel { vessel: id });
                }
            } else if let Some(id) = el
                .closest("li[data-ose=\"ts-vessel\"]")
                .ok()
                .flatten()
                .and_then(|e| e.get_attribute("data-ose-id"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                app.ts_selected = Some(id);
                app.track_focus(id);
                app.render_tracking();
            }
        });
        panel.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "ksc-launchpad",
        Box::new(|a| a.borrow_mut().open_launchpad()),
    )?;
    wire(
        "launchpad-refresh",
        Box::new(|a| a.borrow_mut().open_launchpad()),
    )?;
    wire("launchpad-back", Box::new(|a| a.borrow().open_ksc()))?;
    wire("ksc-vab", Box::new(|a| a.borrow_mut().open_vab(None)))?;
    wire("ksc-rnd", Box::new(|a| a.borrow_mut().open_rnd()))?;
    // The research button lives in the info panel, which is re-rendered; listen on the panel.
    if let Some(info) = doc.get_element_by_id("rd-info") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .filter(|e| e.id() == "rd-research")
                .and_then(|e| e.get_attribute("data-ose-node"));
            if let Some(id) = id {
                let app = a.borrow();
                set_text(&app.doc, "rd-note", "researching...");
                if let Some(ws) = &app.ws {
                    send(ws, &ClientMessage::UnlockTechNode { id });
                }
            }
        });
        info.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire("fh-science", Box::new(|a| a.borrow().flight_science(0)))?;
    for id in ["fh-eva", "fh-board", "fh-flag", "fh-undock"] {
        wire(id, Box::new(move |a| a.borrow().crew_action(id)))?;
    }
    // A crew portrait's EVA button sends that Kerbal out.
    if let Some(box_el) = doc.get_element_by_id("fh-crewbox") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let name = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-eva"));
            if let Some(name) = name {
                a.borrow().crew_eva(Some(name));
            }
        });
        box_el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire("fh-transmit", Box::new(|a| a.borrow().flight_science(1)))?;
    wire("fh-recover", Box::new(|a| a.borrow().flight_science(2)))?;
    // The pause menu's Recover Vessel, as the flight HUD's.
    wire("to-recover", Box::new(|a| a.borrow().flight_science(2)))?;
    wire("rnd-back", Box::new(|a| a.borrow().open_ksc()))?;
    wire("ksc-mission", Box::new(|a| a.borrow_mut().open_missions()))?;
    wire("ksc-training", Box::new(|a| a.borrow_mut().open_training()))?;
    wire("tc-back", Box::new(|a| a.borrow().open_ksc()))?;
    for (id, lessons) in [("tc-tab-kerbals", false), ("tc-tab-lessons", true)] {
        wire(
            id,
            Box::new(move |a| {
                let mut app = a.borrow_mut();
                app.tc_lessons = lessons;
                app.render_training();
            }),
        )?;
    }
    if let Some(list) = doc.get_element_by_id("tl-list") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-lesson]").ok().flatten())
                .and_then(|e| e.get_attribute("data-lesson"));
            if let Some(id) = id {
                let mut app = a.borrow_mut();
                app.lesson_selected = Some(id);
                app.render_training();
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(info) = doc.get_element_by_id("tc-info") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("#tl-start").ok().flatten())
                .and_then(|e| e.get_attribute("data-lesson"));
            if let Some(id) = id {
                a.borrow().play_lesson(&id);
            }
        });
        info.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(list) = doc.get_element_by_id("tc-list") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let name = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-ose-kerbal]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ose-kerbal"));
            if let Some(name) = name {
                let mut app = a.borrow_mut();
                app.tc_selected = Some(name);
                app.render_training();
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(info) = doc.get_element_by_id("lp-info") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut app = a.borrow_mut();
            let mut crew = Vec::new();
            if let Ok(seats) = app.doc.query_selector_all("#lp-info [data-ose-seat]") {
                for i in 0..seats.length() {
                    if let Some(sel) = seats
                        .get(i)
                        .and_then(|n| n.dyn_into::<web_sys::HtmlSelectElement>().ok())
                    {
                        crew.push(sel.value());
                    }
                }
            }
            app.lp_crew = crew;
            app.lp_crew_for = app.lp_selected;
            app.render_launchpad_info();
        });
        info.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire("mc-back", Box::new(|a| a.borrow().open_ksc()))?;
    wire(
        "mc-tab-primary",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.mc_primary = true;
            app.render_missions();
        }),
    )?;
    wire(
        "mc-tab-secondary",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.mc_primary = false;
            app.render_missions();
        }),
    )?;
    if let Some(panel) = doc.get_element_by_id("mc-briefing") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let Some(el) = ev.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
                return;
            };
            let app = a.borrow();
            let Some(ws) = &app.ws else { return };
            if let Some(id) = el.get_attribute("data-ose-mission-accept") {
                send(ws, &ClientMessage::AcceptMission { id });
            } else if let Some(id) = el.get_attribute("data-ose-mission-turnin") {
                send(ws, &ClientMessage::TurnInMission { id });
            }
        });
        panel.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(list) = doc.get_element_by_id("mc-list") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let mut t = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            while let Some(el) = t.clone() {
                if let Some(id) = el.get_attribute("data-ose-mission") {
                    let mut app = a.borrow_mut();
                    app.mc_selected = Some(id);
                    app.render_missions();
                    break;
                }
                t = el.parent_element();
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(tree) = doc.get_element_by_id("rd-tree") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let mut t = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            while let Some(el) = t.clone() {
                if let Some(id) = el.get_attribute("data-ose-node") {
                    let mut app = a.borrow_mut();
                    app.rd_selected = Some(id);
                    app.render_rnd();
                    break;
                }
                t = el.parent_element();
            }
        });
        tree.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire("vab-back", Box::new(|a| a.borrow().open_ksc()))?;
    wire("vab-remove", Box::new(|a| a.borrow_mut().vab_remove()))?;
    wire("vab-clear", Box::new(|a| a.borrow_mut().vab_clear()))?;
    wire(
        "vab-stage-up",
        Box::new(|a| a.borrow_mut().vab_shift_stage(1)),
    )?;
    wire(
        "vab-stage-down",
        Box::new(|a| a.borrow_mut().vab_shift_stage(-1)),
    )?;
    wire("vab-save", Box::new(|a| a.borrow_mut().vab_save(false)))?;
    wire("vab-undo", Box::new(|a| a.borrow_mut().vab_undo(false)))?;
    if let Some(box_el) = doc.get_element_by_id("ftue") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .map(|e| e.id())
                .unwrap_or_default();
            let app = a.borrow();
            match id.as_str() {
                "ftue-next" if app.ftue_state.borrow().lesson => {
                    let last = app.ftue_state.borrow().queue.len() <= 1;
                    if last {
                        app.lesson_continue();
                    } else {
                        app.ftue_state.borrow_mut().queue.pop_front();
                        app.ftue_show();
                    }
                }
                "ftue-next" => {
                    {
                        let mut st = app.ftue_state.borrow_mut();
                        st.queue.pop_front();
                    }
                    app.ftue_show();
                }
                "ftue-off" if app.ftue_state.borrow().lesson => {
                    *app.lesson_run.borrow_mut() = None;
                    {
                        let mut st = app.ftue_state.borrow_mut();
                        st.queue.clear();
                        st.lesson = false;
                    }
                    app.ftue_show();
                }
                "ftue-off" => {
                    {
                        let mut st = app.ftue_state.borrow_mut();
                        st.enabled = false;
                        st.queue.clear();
                    }
                    store("ose.ftue", "off");
                    if let Some(i) = app
                        .doc
                        .get_element_by_id("settings-ftue")
                        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                    {
                        i.set_checked(false);
                    }
                    app.ftue_show();
                }
                _ => {}
            }
        });
        box_el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(input) = doc
        .get_element_by_id("settings-ftue")
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.set_checked(app.borrow().ftue_state.borrow().enabled);
        let a = app.clone();
        let i2 = input.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let on = i2.checked();
            store("ose.ftue", if on { "on" } else { "off" });
            let app = a.borrow();
            let mut st = app.ftue_state.borrow_mut();
            st.enabled = on;
            if on {
                // Turning hints back on shows them all again, as a new player would see them.
                st.seen.clear();
                store("ose.ftue.seen", "");
            }
        });
        input.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(sel) = doc.get_element_by_id("vab-twr-body") {
        let a = app.clone();
        let s2 = sel.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let v = s2
                .dyn_ref::<web_sys::HtmlSelectElement>()
                .map(|s| s.value())
                .unwrap_or_default();
            let mut app = a.borrow_mut();
            app.vab_twr_body = (!v.is_empty()).then_some(v);
            app.render_vab_view();
        });
        sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "vab-trip",
        Box::new(|a| {
            let app = a.borrow();
            if let Some(el) = app.doc.get_element_by_id("vab-trip-pop") {
                el.class_list().toggle("open").ok();
            }
            app.render_trip();
        }),
    )?;
    for id in ["trip-from", "trip-to", "trip-round"] {
        if let Some(el) = doc.get_element_by_id(id) {
            let a = app.clone();
            let cb = Closure::<dyn FnMut()>::new(move || a.borrow().render_trip());
            el.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    if let Some(fr) = doc.get_element_by_id("flight-report") {
        let f2 = fr.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let close = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .is_some_and(|e| e.id() == "fr-close");
            if close {
                f2.class_list().remove_1("open").ok();
            }
        });
        fr.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(pam) = doc.get_element_by_id("fh-pam") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let t = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            let act = t
                .as_ref()
                .and_then(|e| e.closest("[data-act]").ok().flatten())
                .and_then(|e| e.get_attribute("data-act"));
            let close = t
                .as_ref()
                .and_then(|e| e.closest("#fpam-close").ok().flatten())
                .is_some();
            let mut app = a.borrow_mut();
            if close {
                app.flight_open_pam(None, 0.0, 0.0);
            } else if let Some(act) = act {
                app.hud_button(&act);
            }
        });
        pam.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "vab-orient",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.vab_record();
            let plane = !app.editor.plane_new;
            app.editor.set_plane(plane);
            // Planes go to the runway, rockets to the pad, as the original's hangar and VAB do.
            let site = if plane { "Runway_1" } else { "LaunchPad_1" };
            if app.launch_sites.iter().any(|s| s.id == site) {
                app.launch_site = Some(site.to_string());
                store("ose.launchSite", site);
                app.render_launch_sites();
            }
            app.render_vab();
        }),
    )?;
    for id in ["vab-site", "lp-site"] {
        if let Some(sel) = doc.get_element_by_id(id) {
            let a = app.clone();
            let s2 = sel.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let v = s2
                    .dyn_ref::<web_sys::HtmlSelectElement>()
                    .map(|s| s.value())
                    .unwrap_or_default();
                let mut app = a.borrow_mut();
                store("ose.launchSite", &v);
                app.launch_site = (!v.is_empty()).then_some(v);
                app.render_launch_sites();
            });
            sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    if let Some(pam) = doc.get_element_by_id("vab-pam") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
            let Some(input) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            let mut app = a.borrow_mut();
            if input.id() == "pam-crossfeed" {
                app.vab_pam_changed(None, 0.0, Some(input.checked()));
            } else if let Some(k) = input.get_attribute("data-res").and_then(|v| v.parse().ok()) {
                app.vab_pam_changed(Some(k), input.value_as_number(), None);
            }
        });
        pam.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
        cb.forget();
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let close = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("#pam-close").ok().flatten())
                .is_some();
            if close {
                a.borrow_mut().vab_open_pam(None, 0.0, 0.0);
            }
        });
        pam.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "vab-report-btn",
        Box::new(|a| {
            if let Some(el) = a.borrow().doc.get_element_by_id("vab-report-pop") {
                el.class_list().toggle("open").ok();
            }
        }),
    )?;
    wire(
        "vab-col",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.vab_show_col = !app.vab_show_col;
            store("ose.vab.col", if app.vab_show_col { "on" } else { "off" });
            app.render_vab_view();
        }),
    )?;
    for (id, com) in [("vab-com", true), ("vab-cot", false)] {
        wire(
            id,
            Box::new(move |a| {
                let mut app = a.borrow_mut();
                let on = if com {
                    app.vab_show_com = !app.vab_show_com;
                    app.vab_show_com
                } else {
                    app.vab_show_cot = !app.vab_show_cot;
                    app.vab_show_cot
                };
                store(
                    if com { "ose.vab.com" } else { "ose.vab.cot" },
                    if on { "on" } else { "off" },
                );
                app.render_vab_view();
            }),
        )?;
    }
    wire(
        "vab-load",
        Box::new(|a| {
            let app = a.borrow();
            if let Some(el) = app.doc.get_element_by_id("vab-load-pop") {
                let open = el.class_list().toggle("open").unwrap_or(false);
                if open {
                    app.render_vab_load();
                    if let Some(ws) = &app.ws {
                        send(ws, &ClientMessage::CraftList);
                    }
                }
            }
        }),
    )?;
    if let Some(list) = doc.get_element_by_id("vab-load-list") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-ose-id]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ose-id"))
                .and_then(|v| v.parse::<u64>().ok());
            if let Some(id) = id {
                let mut app = a.borrow_mut();
                if let Some(el) = app.doc.get_element_by_id("vab-load-pop") {
                    el.class_list().remove_1("open").ok();
                }
                app.vab_record();
                app.open_vab(Some(id));
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "vab-sym",
        Box::new(|a| a.borrow_mut().vab_cycle_symmetry(false)),
    )?;
    wire(
        "vab-snap",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.vab_snap = !app.vab_snap;
            app.render_vab();
        }),
    )?;
    wire("vab-redo", Box::new(|a| a.borrow_mut().vab_undo(true)))?;
    // A part dragged from one stage card to another changes stage (the original's moveStagePartGroup).
    if let Some(list) = doc.get_element_by_id("vab-stages") {
        let a = app.clone();
        let down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let i = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-ose-index]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ose-index"))
                .and_then(|v| v.parse::<usize>().ok());
            if ev.button() == 0 {
                if let Some(i) = i {
                    ev.prevent_default();
                    a.borrow_mut().vab_stage_drag = Some(i);
                }
            }
        });
        list.add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref())?;
        down.forget();
        let a = app.clone();
        let up = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let mut app = a.borrow_mut();
            let Some(i) = app.vab_stage_drag.take() else {
                return;
            };
            let stage = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-ose-stage]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ose-stage"))
                .and_then(|v| v.parse::<usize>().ok());
            let current = app.editor.craft.stages.iter().position(|s| s.contains(&i));
            if let Some(stage) = stage.filter(|s| Some(*s) != current) {
                app.vab_record();
                let cat = std::mem::take(&mut app.catalog);
                app.editor.move_to_stage(i, stage, &cat);
                app.catalog = cat;
                app.render_vab();
            }
        });
        list.add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref())?;
        up.forget();
    }
    // A part named on a stage card selects it.
    if let Some(list) = doc.get_element_by_id("vab-stages") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let i = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("[data-ose-index]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ose-index"))
                .and_then(|v| v.parse::<usize>().ok());
            if let Some(i) = i {
                let mut app = a.borrow_mut();
                app.editor.selected = Some(i);
                app.render_vab();
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // The same part tooltip over the R&D node's part list (expandPartInfoTooltip in the original).
    if let Some(info) = doc.get_element_by_id("rd-info") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let row = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("li[data-ose-part]").ok().flatten());
            a.borrow().vab_tooltip(row.as_ref());
        });
        info.add_event_listener_with_callback("mouseover", cb.as_ref().unchecked_ref())?;
        cb.forget();
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |_: MouseEvent| {
            a.borrow().vab_tooltip(None);
        });
        info.add_event_listener_with_callback("mouseleave", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // Sort and size filter, and middle click on a row to make it a favourite.
    if let Some(sel) = doc.get_element_by_id("vab-sort") {
        let a = app.clone();
        let s2 = sel.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let v = s2
                .dyn_ref::<web_sys::HtmlSelectElement>()
                .map(|s| s.value())
                .unwrap_or_default();
            let mut app = a.borrow_mut();
            app.vab_sort = v.parse().unwrap_or(0);
            app.render_vab_picker();
        });
        sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(chips) = doc.get_element_by_id("vab-sizes") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let size = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-size"));
            if let Some(size) = size {
                let mut app = a.borrow_mut();
                app.vab_size = (size != "All").then_some(size);
                app.render_vab_picker();
            }
        });
        chips.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(parts) = doc.get_element_by_id("vab-parts") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            if ev.button() != 1 {
                return;
            }
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("li[data-ose-part]").ok().flatten())
                .and_then(|e| e.get_attribute("data-ose-part"));
            if let Some(id) = id {
                ev.prevent_default();
                let mut app = a.borrow_mut();
                if !app.vab_favs.remove(&id) {
                    app.vab_favs.insert(id);
                }
                let list: Vec<&str> = app.vab_favs.iter().map(|s| s.as_str()).collect();
                store("ose.vab.favs", &list.join(","));
                app.render_vab_picker();
            }
        });
        parts.add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    // Part tooltip (the original's part info overlay) while the pointer is over a picker row.
    if let Some(parts) = doc.get_element_by_id("vab-parts") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let row = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("li[data-ose-part]").ok().flatten());
            let app = a.borrow();
            app.vab_tooltip(row.as_ref());
        });
        parts.add_event_listener_with_callback("mouseover", cb.as_ref().unchecked_ref())?;
        cb.forget();
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |_: MouseEvent| {
            a.borrow().vab_tooltip(None);
        });
        parts.add_event_listener_with_callback("mouseleave", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire(
        "vab-frame",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.vab_local_frame = !app.vab_local_frame;
            app.render_vab();
        }),
    )?;
    wire(
        "vab-offset",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.vab_offset = !app.vab_offset;
            app.render_vab();
        }),
    )?;
    wire(
        "vab-launch",
        Box::new(|a| {
            a.borrow().lesson_event("VABLaunchButtonPressedMessage");
            a.borrow_mut().vab_save(true)
        }),
    )?;
    if let Some(cats) = doc.get_element_by_id("vab-cats") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let cat = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-ose-category"));
            if let Some(cat) = cat {
                let mut app = a.borrow_mut();
                app.vab_category = cat;
                app.render_vab_picker();
            }
        });
        cats.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(search) = doc.get_element_by_id("vab-search") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow().render_vab_picker());
        search.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(parts) = doc.get_element_by_id("vab-parts") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let mut t = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            // Clicks land on the <small> inside the row as often as on the row itself.
            while let Some(el) = t.clone() {
                if let Some(id) = el.get_attribute("data-ose-part") {
                    // A drag that ended in the hall already placed the part (or dropped it).
                    let dragged = std::mem::take(&mut a.borrow_mut().vab_dragged);
                    if !dragged {
                        a.borrow_mut().vab_add(&id);
                    }
                    break;
                }
                t = el.parent_element();
            }
        });
        parts.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
        // Press on a row: the start of a possible drag into the hall.
        let a = app.clone();
        let down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            if ev.button() != 0 {
                return;
            }
            let mut t = ev.target().and_then(|t| t.dyn_into::<Element>().ok());
            while let Some(el) = t.clone() {
                if let Some(id) = el.get_attribute("data-ose-part") {
                    ev.prevent_default();
                    // The search box lets go, so W/A/S/D/Q/E turn the part in hand instead of typing.
                    if let Some(active) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.active_element())
                        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                    {
                        active.blur().ok();
                    }
                    a.borrow_mut().vab_press =
                        Some((id, ev.client_x() as f64, ev.client_y() as f64));
                    break;
                }
                t = el.parent_element();
            }
        });
        parts.add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref())?;
        down.forget();
    }
    if let Some(canvas) = doc.get_element_by_id("vab-canvas") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            a.borrow_mut()
                .vab_click(ev.offset_x() as f64, ev.offset_y() as f64);
        });
        canvas.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(name) = doc.get_element_by_id("vab-name") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut app = a.borrow_mut();
            if let Some(n) = input_string(&app.doc, "vab-name") {
                app.editor.craft.name = n;
            }
        });
        name.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(cb_el) = doc.get_element_by_id("launch-stock") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow().render_crafts());
        cb_el.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    for list in ["crafts", "lp-info"]
        .iter()
        .filter_map(|id| doc.get_element_by_id(id))
    {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-ose-craft-launch"))
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(id) = id {
                a.borrow_mut().launch_craft(id);
                return;
            }
            let orbit = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-ose-craft-orbit"))
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(id) = orbit {
                a.borrow_mut().launch_craft_to_orbit(id);
                return;
            }
            let edit = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-ose-craft-edit"))
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(id) = edit {
                a.borrow_mut().open_vab(Some(id));
                return;
            }
            let picked = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.closest("li[data-ose=\"craft\"]").ok().flatten())
                .and_then(|li| li.get_attribute("data-ose-id"))
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(id) = picked {
                let mut app = a.borrow_mut();
                app.lp_selected = Some(id);
                app.render_crafts();
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    wire("create", Box::new(|a| a.borrow_mut().create_session()))?;
    wire("agency-apply", Box::new(|a| a.borrow_mut().send_agency()))?;
    wire(
        "leave",
        Box::new(|_a| {
            // Leaving is a reload: the socket closes and the name screen comes back.
            if let Some(w) = web_sys::window() {
                w.location().reload().ok();
            }
        }),
    )?;
    // Join buttons are created per session; one delegated handler on the list.
    if let Some(list) = doc.get_element_by_id("sessions") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            let id = ev
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|e| e.get_attribute("data-ose-session"));
            if let Some(id) = id {
                a.borrow_mut().join_session(&id);
            }
        });
        list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(sel) = doc.get_element_by_id("create-preset") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow().apply_preset());
        sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(sel) = doc.get_element_by_id("create-mode") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow().render_difficulty_form());
        sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(grid) = doc.get_element_by_id("difficulty") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow().refresh_difficulty_outputs());
        grid.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    for (id, key) in [
        ("agency-flag", "ose.flag"),
        ("agency-primary", "ose.primary"),
        ("agency-secondary", "ose.secondary"),
    ] {
        if let (Some(v), Some(input)) = (
            stored(key),
            doc.get_element_by_id(id)
                .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok()),
        ) {
            input.set_value(&v);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Networking

fn ws_url() -> Result<String, JsValue> {
    let loc = web_sys::window().ok_or("no window")?.location();
    let scheme = if loc.protocol()? == "https:" {
        "wss"
    } else {
        "ws"
    };
    Ok(format!("{scheme}://{}{WS_PATH}", loc.host()?))
}

fn send(ws: &WebSocket, msg: &ClientMessage) {
    match ose_net::encode(msg) {
        Ok(bytes) => {
            if let Err(e) = ws.send_with_u8_array(&bytes) {
                log(&format!("ws send failed: {e:?}"));
            }
        }
        Err(e) => log(&format!("encode failed: {e}")),
    }
}

fn connect(app: &Shared) -> Result<(), JsValue> {
    let url = ws_url()?;
    let ws = WebSocket::new(&url)?;
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
    app.borrow().set_status(&format!("connecting to {url}"));

    {
        let app = app.clone();
        let ws2 = ws.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            let display_name = app.borrow().player_name.clone();
            send(
                &ws2,
                &ClientMessage::Hello {
                    protocol: PROTOCOL_VERSION,
                    token: stored("ose.token"),
                    display_name,
                },
            );
            app.borrow().set_status("connected, waiting for the server");
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();
    }
    {
        let app = app.clone();
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            if let Ok(buf) = ev.data().dyn_into::<js_sys::ArrayBuffer>() {
                let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                match ose_net::decode::<ServerMessage>(&bytes) {
                    Ok(msg) => app.borrow_mut().on_server(msg),
                    Err(e) => log(&format!("bad server message: {e}")),
                }
            }
        });
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();
    }
    {
        let app = app.clone();
        let on_close = Closure::<dyn FnMut()>::new(move || {
            let mut a = app.borrow_mut();
            a.ws = None;
            a.in_session = false;
            a.set_status("disconnected; reload to reconnect");
            set_text(
                &a.doc,
                "menu-note",
                "disconnected; reload the page to reconnect",
            );
            if let Some(m) = a.doc.get_element_by_id("menu") {
                m.class_list().remove_1("hidden").ok();
            }
            a.menu_screen("name");
        });
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();
    }
    {
        let app = app.clone();
        let on_error = Closure::<dyn FnMut()>::new(move || {
            app.borrow().set_status("connection error");
        });
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();
    }
    app.borrow_mut().ws = Some(ws);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Input

fn install_input(app: &Shared) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let canvas = app.borrow().canvas.clone();

    let a = app.clone();
    let on_resize = Closure::<dyn FnMut()>::new(move || a.borrow_mut().resize());
    window.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())?;
    on_resize.forget();

    let gpu_canvas = app.borrow().gpu_canvas.clone();
    for target in [&canvas, &gpu_canvas] {
        let a = app.clone();
        let on_wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
            ev.prevent_default();
            let mut factor: f64 = if ev.delta_y() > 0.0 { 1.25 } else { 0.8 };
            // Shift zooms faster, as in the original's VAB (fastScrollModifier).
            if ev.shift_key() {
                factor = factor.powi(3);
            }
            let mut app = a.borrow_mut();
            #[cfg(target_arch = "wasm32")]
            if app.in_session && app.ksc_active() {
                app.ksc3d.zoom(factor);
                return;
            }
            #[cfg(target_arch = "wasm32")]
            if app.in_session && app.vab_active() {
                app.vab3d.zoom(factor);
                app.lesson_event("CameraZoomedMessage");
                return;
            }
            #[cfg(target_arch = "wasm32")]
            if app.flight_active() && app.gpu.is_some() {
                app.flight3d.zoom(factor);
                app.lesson_event("CameraZoomedMessage");
                return;
            }
            if app.is_3d() {
                app.orbit_dist_km = (app.orbit_dist_km * factor).clamp(1.0e-2, 1.0e10);
            } else {
                app.camera
                    .zoom_at(factor, ev.offset_x() as f64, ev.offset_y() as f64);
            }
        });
        target.add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref())?;
        on_wheel.forget();

        let a = app.clone();
        // Left or right button drags the camera; the original drags with the right button everywhere and
        // keeps the left for picking, so the right one must work too.
        let on_down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            if ev.button() == 0 || ev.button() == 2 {
                let mut app = a.borrow_mut();
                if ev.button() == 0 && app.mnv_press(ev.client_x() as f64, ev.client_y() as f64) {
                    return;
                }
                if ev.button() == 0 && app.vab_pasting {
                    return;
                }
                #[cfg(target_arch = "wasm32")]
                if ev.button() == 0 && app.in_session && app.vab_active() {
                    let rect = app.gpu_canvas.get_bounding_client_rect();
                    let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
                    let (px, py) = ((x - rect.left()) * app.dpr, (y - rect.top()) * app.dpr);
                    if let Some(i) = app.gpu.as_ref().and_then(|g| app.vab3d.pick(g, px, py)) {
                        app.vab_grab = Some((i, x, y));
                        app.press = Some((x, y));
                        return;
                    }
                }
                app.drag = Some((ev.client_x() as f64, ev.client_y() as f64));
                app.press = app.drag;
                app.canvas.class_list().add_1("dragging").ok();
                app.gpu_canvas.class_list().add_1("dragging").ok();
            }
        });
        target.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref())?;
        on_down.forget();
        let no_menu = Closure::<dyn FnMut(MouseEvent)>::new(|ev: MouseEvent| ev.prevent_default());
        target.add_event_listener_with_callback("contextmenu", no_menu.as_ref().unchecked_ref())?;
        no_menu.forget();
    }

    let a = app.clone();
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let mut app = a.borrow_mut();
        if app.mnv_drag.is_some() {
            app.mnv_drag_to(ev.client_x() as f64, ev.client_y() as f64, ev.shift_key());
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if app.vab_pasting {
            let rect = app.gpu_canvas.get_bounding_client_rect();
            app.vab3d.cursor = (
                (ev.client_x() as f64 - rect.left()) * app.dpr,
                (ev.client_y() as f64 - rect.top()) * app.dpr,
            );
            return;
        }
        // A placed part dragged with the left button comes off the craft with everything under it.
        #[cfg(target_arch = "wasm32")]
        if let Some((i, x0, y0)) = app.vab_grab {
            let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
            if app.vab_frag.is_none() && ((nx - x0).abs() > 6.0 || (ny - y0).abs() > 6.0) {
                app.vab_record();
                let cat = std::mem::take(&mut app.catalog);
                let frag = app.editor.detach(i, &cat);
                if let Some(frag) = frag {
                    let nodes = vab_nodes(&cat, frag.root_id());
                    app.vab3d.held = Some((frag.root_id().to_string(), nodes));
                    app.vab_frag = Some(frag);
                    if let Some(b) = app.doc.body() {
                        b.class_list().add_1("vab-holding").ok();
                    }
                }
                app.catalog = cat;
                app.render_vab();
            }
            if app.vab_frag.is_some() {
                let rect = app.gpu_canvas.get_bounding_client_rect();
                app.vab3d.cursor = ((nx - rect.left()) * app.dpr, (ny - rect.top()) * app.dpr);
            }
            return;
        }
        // A picker press that moves becomes a part held over the hall.
        #[cfg(target_arch = "wasm32")]
        if let Some((id, x0, y0)) = app.vab_press.clone() {
            let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
            if app.vab3d.held.is_none() && ((nx - x0).abs() > 6.0 || (ny - y0).abs() > 6.0) {
                let nodes = app
                    .catalog
                    .get(&id)
                    .map(|s| {
                        s.attach_nodes
                            .iter()
                            .map(|n| {
                                (
                                    glam::DVec3::new(n.position.x, n.position.y, n.position.z),
                                    glam::DVec3::new(
                                        n.orientation.x,
                                        n.orientation.y,
                                        n.orientation.z,
                                    ),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                app.lesson_part(&id, false);
                app.vab3d.held = Some((id, nodes));
                app.vab_turn = vab::NO_TURN;
                app.vab_yaw = vab::NO_TURN;
                app.vab_sync_turn();
                if let Some(b) = app.doc.body() {
                    b.class_list().add_1("vab-holding").ok();
                }
            }
            if app.vab3d.held.is_some() {
                let rect = app.gpu_canvas.get_bounding_client_rect();
                app.vab3d.cursor = ((nx - rect.left()) * app.dpr, (ny - rect.top()) * app.dpr);
                return;
            }
        }
        #[cfg(target_arch = "wasm32")]
        if app.in_session && app.vab_active() {
            if let Some((x, y)) = app.drag {
                let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
                app.vab3d.orbit(nx - x, ny - y);
                app.lesson_event("CameraRotatedMessage");
                app.drag = Some((nx, ny));
            } else {
                // The part under the pointer lights up, as the original's hover highlight.
                let rect = app.gpu_canvas.get_bounding_client_rect();
                let (px, py) = (
                    (ev.client_x() as f64 - rect.left()) * app.dpr,
                    (ev.client_y() as f64 - rect.top()) * app.dpr,
                );
                let over_hall = ev
                    .target()
                    .and_then(|t| t.dyn_into::<Element>().ok())
                    .is_some_and(|e| e.id() == "gpu");
                let hit = if over_hall {
                    app.gpu.as_ref().and_then(|g| app.vab3d.pick(g, px, py))
                } else {
                    None
                };
                app.vab3d.hovered = hit;
            }
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if app.flight_active() && app.gpu.is_some() {
            if let Some((x, y)) = app.drag {
                let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
                app.flight3d.orbit(nx - x, ny - y);
                app.lesson_event("CameraRotatedMessage");
                app.drag = Some((nx, ny));
            }
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if app.in_session && app.ksc_active() {
            if let Some((x, y)) = app.drag {
                let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
                app.ksc3d.orbit(nx - x, ny - y);
                app.drag = Some((nx, ny));
            } else {
                let rect = app.gpu_canvas.get_bounding_client_rect();
                let (px, py) = (
                    (ev.client_x() as f64 - rect.left()) * app.dpr,
                    (ev.client_y() as f64 - rect.top()) * app.dpr,
                );
                let hit = app.gpu.as_ref().and_then(|g| app.ksc3d.pick(g, px, py));
                app.ksc3d.hovered = hit;
                app.gpu_canvas
                    .style()
                    .set_property("cursor", if hit.is_some() { "pointer" } else { "grab" })
                    .ok();
            }
            return;
        }
        if let Some((x, y)) = app.drag {
            let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
            if app.is_3d() {
                app.orbit_yaw -= (nx - x) * 0.005;
                app.orbit_pitch = (app.orbit_pitch - (ny - y) * 0.005).clamp(-1.5, 1.5);
            } else {
                app.camera.pan_px(nx - x, ny - y);
            }
            app.drag = Some((nx, ny));
        }
    });
    window.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())?;
    on_move.forget();

    let a = app.clone();
    let on_up = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let mut app = a.borrow_mut();
        // A stage-card drag released anywhere else is dropped (the card list handles its own mouseup first).
        app.vab_stage_drag = None;
        if app.mnv_drag.take().is_some() {
            app.send_node();
            app.press = None;
            app.drag = None;
            return;
        }
        // Drop a held part: on its snapped node, or as the root of an empty craft; over the panels it is put
        // back.
        #[cfg(target_arch = "wasm32")]
        {
            let held = app.vab3d.held.take();
            let pressed = app.vab_press.take();
            app.vab_grab = None;
            let pasting = std::mem::take(&mut app.vab_pasting);
            if let (Some(frag), Some(_)) = (app.vab_frag.take(), held.as_ref()) {
                if pasting {
                    app.vab_record();
                }
                if let Some(b) = app.doc.body() {
                    b.class_list().remove_1("vab-holding").ok();
                }
                let free = app.editor.free_nodes(&app.catalog);
                let rect = app.gpu_canvas.get_bounding_client_rect();
                app.vab3d.cursor = (
                    (ev.client_x() as f64 - rect.left()) * app.dpr,
                    (ev.client_y() as f64 - rect.top()) * app.dpr,
                );
                let snap = app
                    .gpu
                    .as_ref()
                    .and_then(|g| app.vab3d.snap_now(g, &free))
                    .or_else(|| app.vab3d.snap.take());
                let surface = if snap.is_none() {
                    app.vab_surface(frag.root_id())
                } else {
                    None
                };
                let cat = std::mem::take(&mut app.catalog);
                let result = match surface {
                    Some((spot, _)) => app.editor.attach_surface(&frag, &cat, spot),
                    None => {
                        app.editor
                            .attach(&frag, &cat, snap.as_ref().map(|(t, n)| (*t, n.as_str())))
                    }
                };
                app.catalog = cat;
                if let Err(e) = result {
                    // Nowhere to hang it: a moved part goes back where it was, as the original does; a paste
                    // is dropped.
                    let mut history = std::mem::take(&mut app.vab_history);
                    history.undo(&mut app.editor);
                    app.vab_history = history;
                    set_text(&app.doc, "vab-report", &e);
                }
                app.press = None;
                app.render_vab();
            } else if let Some((id, _)) = held {
                app.vab_dragged = pressed.is_some();
                if let Some(b) = app.doc.body() {
                    b.class_list().remove_1("vab-holding").ok();
                }
                let over_panel = ev
                    .target()
                    .and_then(|t| t.dyn_into::<Element>().ok())
                    .map(|e| e.closest("#vab-picker, #vab-side").ok().flatten().is_some())
                    .unwrap_or(false);
                // Where the cursor is now decides, not the last frame (frames may lag behind the mouse).
                let free = app.snap_nodes(Some(&id));
                let rect = app.gpu_canvas.get_bounding_client_rect();
                app.vab3d.cursor = (
                    (ev.client_x() as f64 - rect.left()) * app.dpr,
                    (ev.client_y() as f64 - rect.top()) * app.dpr,
                );
                let snap = app
                    .gpu
                    .as_ref()
                    .and_then(|g| app.vab3d.snap_now(g, &free))
                    .or_else(|| app.vab3d.snap.take())
                    .filter(|_| !free.is_empty());
                if !over_panel {
                    let surface = if snap.is_none() {
                        app.vab_surface(&id)
                    } else {
                        None
                    };
                    app.vab_record();
                    let cat = std::mem::take(&mut app.catalog);
                    let result = match (cat.get(&id), snap) {
                        (Some(spec), Some((target, node))) => {
                            let yaw = app.vab_yaw;
                            app.editor.add_part_on(spec, &cat, target, &node).map(|i| {
                                if yaw != vab::NO_TURN {
                                    app.editor.turn_on_own_node(i, yaw, &cat);
                                }
                            })
                        }
                        (Some(spec), None) if surface.is_some() => {
                            let (spot, sym) = surface.unwrap();
                            app.editor.add_surface(spec, &cat, spot, sym).map(|_| ())
                        }
                        (Some(spec), None) if app.editor.is_empty() => {
                            app.editor.add_part(spec, &cat).map(|_| ())
                        }
                        (Some(_), None) => Err("drop the part on a green attach point".into()),
                        (None, _) => Err(format!("unknown part {id}")),
                    };
                    app.catalog = cat;
                    match result {
                        Ok(()) => app.lesson_part(&id, true),
                        Err(e) => set_text(&app.doc, "vab-report", &e),
                    }
                    app.render_vab();
                }
            }
        }
        app.drag = None;
        app.canvas.class_list().remove_1("dragging").ok();
        app.gpu_canvas.class_list().remove_1("dragging").ok();
        // A press and release within a few pixels on the map near the planning vessel's orbit adds a node.
        if let Some((x, y)) = app.press {
            let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
            let menu_open = app
                .doc
                .get_element_by_id("menu")
                .map(|m| !m.class_list().contains("hidden"))
                .unwrap_or(false);
            if ev.button() == 0
                && (nx - x).abs() < 4.0
                && (ny - y).abs() < 4.0
                && app.in_session
                && !menu_open
                && !app.flight_active()
                && app.is_3d()
            {
                let rect = app.gpu_canvas.get_bounding_client_rect();
                if app.map_click(nx - rect.left(), ny - rect.top()) {
                    app.press = None;
                }
            }
        }
        // A press and release within a few pixels on the KSC scene is a click on a facility.
        #[cfg(target_arch = "wasm32")]
        if let Some((x, y)) = app.press.take() {
            let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
            if ev.button() == 0
                && (nx - x).abs() < 4.0
                && (ny - y).abs() < 4.0
                && app.in_session
                && app.ksc_active()
            {
                let rect = app.gpu_canvas.get_bounding_client_rect();
                let (px, py) = ((nx - rect.left()) * app.dpr, (ny - rect.top()) * app.dpr);
                let hit = app.gpu.as_ref().and_then(|g| app.ksc3d.pick(g, px, py));
                if let Some(i) = hit {
                    let facility = app.ksc3d.buildings()[i].facility.clone();
                    app.open_facility(&facility);
                }
            } else if ev.button() == 0
                && (nx - x).abs() < 4.0
                && (ny - y).abs() < 4.0
                && app.in_session
                && app.vab_active()
            {
                let rect = app.gpu_canvas.get_bounding_client_rect();
                let (px, py) = ((nx - rect.left()) * app.dpr, (ny - rect.top()) * app.dpr);
                let hit = app.gpu.as_ref().and_then(|g| app.vab3d.pick(g, px, py));
                if let Some(i) = hit {
                    app.editor.selected = Some(i);
                    app.render_vab();
                }
                app.vab_open_pam(None, 0.0, 0.0);
            } else if ev.button() == 2
                && (nx - x).abs() < 4.0
                && (ny - y).abs() < 4.0
                && app.flight_active()
            {
                // A right click on a part in flight opens its window.
                let rect = app.gpu_canvas.get_bounding_client_rect();
                let (px, py) = ((nx - rect.left()) * app.dpr, (ny - rect.top()) * app.dpr);
                let hit = app.gpu.as_ref().and_then(|g| app.flight3d.pick(g, px, py));
                app.flight_open_pam(hit, nx, ny);
            } else if ev.button() == 2
                && (nx - x).abs() < 4.0
                && (ny - y).abs() < 4.0
                && app.in_session
                && app.vab_active()
            {
                // A right click (not a drag) on a part opens its manager window.
                let rect = app.gpu_canvas.get_bounding_client_rect();
                let (px, py) = ((nx - rect.left()) * app.dpr, (ny - rect.top()) * app.dpr);
                let hit = app.gpu.as_ref().and_then(|g| app.vab3d.pick(g, px, py));
                if let Some(i) = hit {
                    app.editor.selected = Some(i);
                    app.render_vab();
                }
                app.vab_open_pam(hit, nx, ny);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = &ev;
            app.press = None;
        }
    });
    window.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())?;
    on_up.forget();

    let a = app.clone();
    let on_key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let mut app = a.borrow_mut();
            let typing = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                .is_some();
            let typing = typing
                || ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                    .is_some();
            let menu_open = app
                .doc
                .get_element_by_id("menu")
                .map(|m| !m.class_list().contains("hidden"))
                .unwrap_or(false);
            // Settings > Input waits for a key: this one is the new binding (Esc keeps the old one).
            if let Some(default) = app.rebinding.take() {
                ev.prevent_default();
                let raw = ev.code();
                if raw != "Escape" {
                    app.remap.bind(&default, &raw);
                    store("ose.keys", &app.remap.to_stored());
                }
                app.render_rebind();
                return;
            }
            // The client acts on the original's default keys; a rebound key stands in for its action's.
            let code = app.remap.translate(&ev.code()).unwrap_or_default();
            if code == "Escape" && settings_open(&app.doc) {
                show_settings(&app.doc, false);
                return;
            }
            if code == "Escape" && app.flight_pam.is_some() {
                app.flight_open_pam(None, 0.0, 0.0);
                return;
            }
            if typing {
                app.keys.clear();
                return;
            }
            app.keys.down(&code);
            if code == "CapsLock" {
                app.keys.precision = ev.get_modifier_state("CapsLock");
            }
            if app.in_session && app.vab_active() {
                let ctrl = ev.ctrl_key() || ev.meta_key();
                let handled = match code.as_str() {
                    "Delete" | "Backspace" => {
                        app.vab_remove();
                        true
                    }
                    "KeyZ" if ctrl && ev.shift_key() => {
                        app.vab_undo(true);
                        true
                    }
                    "KeyZ" if ctrl => {
                        app.vab_undo(false);
                        true
                    }
                    "KeyY" if ctrl => {
                        app.vab_undo(true);
                        true
                    }
                    "KeyF" if ctrl => {
                        if let Some(el) = app
                            .doc
                            .get_element_by_id("vab-search")
                            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                        {
                            el.focus().ok();
                        }
                        true
                    }
                    "KeyN" if ctrl => {
                        app.vab_clear();
                        true
                    }
                    "KeyC" if ctrl => {
                        let cat = std::mem::take(&mut app.catalog);
                        let frag = app.editor.selected.and_then(|i| app.editor.copy(i, &cat));
                        app.catalog = cat;
                        if frag.is_some() {
                            app.vab_clipboard = frag;
                            hud::push(&app.doc, "Copied", "passive");
                        }
                        true
                    }
                    "KeyV" if ctrl => {
                        app.vab_paste();
                        true
                    }
                    "Escape" if app.vab_pam.is_some() => {
                        app.vab_open_pam(None, 0.0, 0.0);
                        true
                    }
                    "Escape" if app.vab_pasting => {
                        app.vab_cancel_paste();
                        true
                    }
                    "KeyX" if !ctrl => {
                        app.vab_cycle_symmetry(ev.shift_key());
                        true
                    }
                    "KeyW" | "KeyS" | "KeyA" | "KeyD" | "KeyQ" | "KeyE" | "Space"
                        if !ctrl && app.vab_holding() =>
                    {
                        app.vab_turn_held(&code, ev.shift_key());
                        true
                    }
                    "KeyC" if !ctrl => {
                        app.vab_snap = !app.vab_snap;
                        app.render_vab();
                        true
                    }
                    "KeyF" if !ctrl => {
                        app.vab_local_frame = !app.vab_local_frame;
                        app.render_vab();
                        true
                    }
                    // The original's tool numbers: 1 places parts, 2 is the offset tool.
                    "Digit1" | "Digit2" if !ctrl => {
                        app.vab_offset = code == "Digit2";
                        app.render_vab();
                        true
                    }
                    "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "KeyQ" | "KeyE"
                        if app.vab_offset && app.editor.selected.is_some() && !ctrl =>
                    {
                        let step = if ev.shift_key() { 0.25 } else { 0.05 };
                        let d = match code.as_str() {
                            "ArrowUp" => ose_math::DVec3::new(0.0, step, 0.0),
                            "ArrowDown" => ose_math::DVec3::new(0.0, -step, 0.0),
                            "ArrowLeft" => ose_math::DVec3::new(-step, 0.0, 0.0),
                            "ArrowRight" => ose_math::DVec3::new(step, 0.0, 0.0),
                            "KeyQ" => ose_math::DVec3::new(0.0, 0.0, -step),
                            _ => ose_math::DVec3::new(0.0, 0.0, step),
                        };
                        if let Some(i) = app.editor.selected {
                            app.vab_record();
                            if app.editor.offset(i, d) {
                                app.render_vab();
                            } else {
                                set_text(&app.doc, "vab-report", "the root part stays put");
                            }
                        }
                        true
                    }
                    "ArrowUp" | "ArrowDown" => {
                        #[cfg(target_arch = "wasm32")]
                        app.vab3d.pan(if code == "ArrowUp" { 60.0 } else { -60.0 });
                        true
                    }
                    "KeyS" if ctrl => {
                        app.vab_save(false);
                        true
                    }
                    _ => false,
                };
                if handled {
                    ev.prevent_default();
                    return;
                }
            }
            if app.flying.is_some() && !menu_open {
                // The original's flight bindings (KSP.Input/GameInput.cs). Rotation, throttle steps and wheels
                // act while held and are read every frame (flight_keys_tick); these act once.
                let handled = match code.as_str() {
                    // The original's cameraPan: the arrow keys turn the flight camera round the vessel.
                    "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" if !app.map_mode => {
                        #[cfg(target_arch = "wasm32")]
                        {
                            let (dx, dy) = match code.as_str() {
                                "ArrowLeft" => (-30.0, 0.0),
                                "ArrowRight" => (30.0, 0.0),
                                "ArrowUp" => (0.0, -30.0),
                                _ => (0.0, 30.0),
                            };
                            app.flight3d.orbit(dx, dy);
                            app.lesson_event("CameraRotatedMessage");
                        }
                        true
                    }
                    "KeyZ" => {
                        app.set_throttle(1.0);
                        true
                    }
                    "KeyX" => {
                        app.set_throttle(0.0);
                        true
                    }
                    "Space" => {
                        // On EVA Space jumps or fires the jetpack (flight_keys_tick); a Kerbal has nothing to stage.
                        if !ev.repeat() {
                            if app.flying_eva() {
                                app.eva_jump = true;
                            } else {
                                app.flight_stage();
                            }
                        }
                        true
                    }
                    "KeyT" if !ev.repeat() => {
                        app.toggle_sas();
                        true
                    }
                    "KeyG" => {
                        if !ev.repeat() {
                            app.toggle_gear();
                        }
                        true
                    }
                    "KeyM" => {
                        app.map_mode = !app.map_mode;
                        if app.map_mode {
                            app.ftue("FTUE_Map");
                            app.lesson_event("MapViewEnteredMessage");
                        }
                        true
                    }
                    "BracketRight" | "BracketLeft" if !app.map_mode => {
                        app.switch_vessel(code == "BracketLeft");
                        true
                    }
                    "KeyV" if !ev.repeat() => {
                        #[cfg(target_arch = "wasm32")]
                        {
                            app.flight3d.mode = app.flight3d.mode.next();
                            let name = app.flight3d.mode.name();
                            hud::push(&app.doc, &format!("Camera: {name}"), "passive");
                        }
                        true
                    }
                    // Interact on EVA (the original's F): board the craft within reach.
                    "KeyF" if !ev.repeat() && app.flying_eva() => {
                        app.crew_action("fh-board");
                        true
                    }
                    "KeyU" | "KeyR" if !ev.repeat() => {
                        app.hud_button(if code == "KeyU" {
                            "fh-act-lights"
                        } else {
                            "fh-act-rcs"
                        });
                        true
                    }
                    c if c.starts_with("Digit") && !ev.repeat() && !ev.ctrl_key() => {
                        // 1..9 are custom groups 1..9 and 0 is group 10, as the original binds them.
                        let n: u8 = c[5..].parse().unwrap_or(0);
                        app.action_group(ose_net::ActionGroup::Custom(if n == 0 { 10 } else { n }));
                        true
                    }
                    "KeyI" | "KeyJ" | "KeyK" | "KeyL" | "KeyH" | "KeyN" => true,
                    "Delete" if !ev.repeat() => {
                        app.lin_mode = !app.lin_mode;
                        hud::push(
                            &app.doc,
                            if app.lin_mode {
                                "Docking mode: W/A/S/D and Shift/Ctrl translate (RCS on)"
                            } else {
                                "Normal controls"
                            },
                            "passive",
                        );
                        true
                    }
                    "KeyW" | "KeyA" | "KeyS" | "KeyD" | "KeyQ" | "KeyE" | "KeyB" | "ShiftLeft"
                    | "ShiftRight" | "ControlLeft" | "ControlRight" => true,
                    _ => false,
                };
                if handled {
                    ev.prevent_default();
                    return;
                }
            }
            match code.as_str() {
                "Slash" if app.in_session => {
                    app.warp_to = None;
                    app.request_warp(1);
                }
                "F2" => {
                    ev.prevent_default();
                    if let Some(b) = app.doc.body() {
                        b.class_list().toggle("ui-hidden").ok();
                    }
                }
                "F4" => {
                    ev.prevent_default();
                    if let Some(b) = app.doc.body() {
                        b.class_list().toggle("labels-hidden").ok();
                    }
                }
                "Home" => app.reset_camera(),
                // The original's audio keys: Numpad 1 mutes the game, Numpad 2 the music.
                "Numpad1" => {
                    let muted = !app.audio.muted;
                    app.audio.set_muted(muted);
                    store("ose.sound", if muted { "off" } else { "on" });
                    app.show_sound_toggle();
                    hud::push(
                        &app.doc,
                        if muted { "Sound off" } else { "Sound on" },
                        "passive",
                    );
                }
                "Numpad2" => {
                    let sfx = app.audio.sfx_scale;
                    let on = app.audio.music_scale > 0.0;
                    let level = if on {
                        0.0
                    } else {
                        stored("ose.vol.music")
                            .and_then(|v| v.parse::<f64>().ok())
                            .filter(|v| *v > 0.0)
                            .unwrap_or(100.0)
                            / 100.0
                    };
                    app.audio.set_volumes(level, sfx);
                    hud::push(
                        &app.doc,
                        if on { "Music off" } else { "Music on" },
                        "passive",
                    );
                }
                "F5" | "F9" if app.in_session => {
                    ev.prevent_default();
                    app.save_action(if code == "F5" {
                        ose_net::SaveAction::QuickSave
                    } else {
                        ose_net::SaveAction::QuickLoad
                    });
                }
                "BracketRight" | "BracketLeft" if app.in_session && !app.flight_active() => {
                    app.cycle_vessel(code == "BracketLeft");
                }
                "KeyM" if app.in_session && !app.flying.is_some() && app.ksc_active() => {
                    app.open_facility("TrackingCenter");
                }
                _ => {}
            }
            match ev.key().as_str() {
                "f" | "F" => app.frame_focus(),
                "Escape" => {
                    let open = app
                        .doc
                        .get_element_by_id("menu")
                        .map(|m| !m.class_list().contains("hidden"))
                        .unwrap_or(false);
                    app.show_menu(!open);
                    if !open && app.in_session {
                        app.audio.play("ui_esc_open");
                    }
                }
                "." | ">" => {
                    app.warp_to = None;
                    let w = kerbin_time::next_warp(app.time.warp);
                    app.request_warp(w);
                }
                "," | "<" => {
                    app.warp_to = None;
                    let w = kerbin_time::prev_warp(app.time.warp);
                    app.request_warp(w);
                }
                _ => {}
            }
        });
    window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())?;
    on_key.forget();

    let a = app.clone();
    let on_key_up =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let mut app = a.borrow_mut();
            let code = app.remap.translate(&ev.code()).unwrap_or_default();
            app.keys.up(&code);
        });
    window.add_event_listener_with_callback("keyup", on_key_up.as_ref().unchecked_ref())?;
    on_key_up.forget();
    // A key released while the tab is in the background never sends keyup: forget all held keys.
    let a = app.clone();
    let on_blur = Closure::<dyn FnMut()>::new(move || a.borrow_mut().keys.clear());
    window.add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref())?;
    on_blur.forget();

    let a = app.clone();
    let on_warp = Closure::<dyn FnMut()>::new(move || {
        let mut app = a.borrow_mut();
        let w = kerbin_time::next_warp(app.time.warp);
        app.request_warp(w);
    });
    app.borrow()
        .warp_el
        .add_event_listener_with_callback("click", on_warp.as_ref().unchecked_ref())?;
    on_warp.forget();

    if let Some(down) = app.borrow().doc.get_element_by_id("warp-down") {
        let a = app.clone();
        let on_down = Closure::<dyn FnMut()>::new(move || {
            let mut app = a.borrow_mut();
            let w = kerbin_time::prev_warp(app.time.warp);
            app.request_warp(w);
        });
        down.add_event_listener_with_callback("click", on_down.as_ref().unchecked_ref())?;
        on_down.forget();
    }

    let doc = app.borrow().doc.clone();
    if let Some(btn) = doc.get_element_by_id("add-node") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow_mut().add_node());
        btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("node-warp") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut app = a.borrow_mut();
            // To a little before the burn should start (half its length before the node), as the original.
            let Some(n) = app.node else { return };
            let burn = app
                .scene
                .as_ref()
                .and_then(|s| s.vessel(n.vessel))
                .and_then(|v| v.flight.as_ref())
                .filter(|f| f.throttle > 0.01)
                .map(|f| {
                    let dv = (n.prograde_mps.powi(2) + n.normal_mps.powi(2) + n.radial_mps.powi(2))
                        .sqrt();
                    dv * f.mass_t / (f.thrust_kn / f.throttle).max(1e-6)
                })
                .unwrap_or(0.0);
            app.start_warp_to(n.ut - burn * 0.5 - 15.0, "the maneuver");
        });
        btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("clear-node") {
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || a.borrow_mut().clear_node());
        btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    for id in ["node-dt", "node-prograde", "node-normal", "node-radial"] {
        if let Some(input) = doc.get_element_by_id(id) {
            let a = app.clone();
            let cb = Closure::<dyn FnMut()>::new(move || a.borrow_mut().node_inputs_changed());
            input.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())?;
            cb.forget();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Frame loop

type FrameCallback = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

fn animate(app: Shared) {
    let f: FrameCallback = Rc::new(RefCell::new(None));
    let g = f.clone();
    *g.borrow_mut() = Some(Closure::new(move || {
        app.borrow_mut().frame();
        request_frame(f.borrow().as_ref().unwrap());
    }));
    request_frame(g.borrow().as_ref().unwrap());
}

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        w.request_animation_frame(f.as_ref().unchecked_ref()).ok();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// App

impl App {
    fn set_status(&self, s: &str) {
        self.status_el
            .set_text_content(Some(&format!("OpenSpaceExplorer client loaded; {s}")));
    }

    fn resize(&mut self) {
        let rect = self.canvas.get_bounding_client_rect();
        let (w, h) = (rect.width().max(1.0), rect.height().max(1.0));
        self.canvas.set_width((w * self.dpr) as u32);
        self.canvas.set_height((h * self.dpr) as u32);
        self.camera.width = w;
        self.camera.height = h;
        let grect = self.gpu_canvas.get_bounding_client_rect();
        let (gw, gh) = (grect.width().max(w), grect.height().max(h));
        self.gpu_canvas.set_width((gw * self.dpr) as u32);
        self.gpu_canvas.set_height((gh * self.dpr) as u32);
        #[cfg(target_arch = "wasm32")]
        if let Some(gpu) = &mut self.gpu {
            gpu.resize((gw * self.dpr) as u32, (gh * self.dpr) as u32);
        }
    }

    fn request_warp(&mut self, factor: u32) {
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::RequestWarp { factor });
        }
        // Show the request at once; the next TimeState confirms or corrects it.
        let now = now_ms();
        let ut = self.time.ut_at(now);
        self.time.update(ut, factor, now);
        self.update_hud(ut);
    }

    fn frame_focus(&mut self) {
        if let Some(scene) = &self.scene {
            let extent = scene.system_extent(self.camera.focus);
            self.camera.frame(extent);
            self.orbit_dist_km = (extent / 1000.0) * 2.4;
        }
    }

    /// True while the VAB screen is the open menu screen.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn vab_active(&self) -> bool {
        self.doc
            .query_selector("#menu:not(.hidden) #screen-vab.open")
            .ok()
            .flatten()
            .is_some()
    }

    /// True while the menu shows the named screen (`screen-<name>`).
    fn screen_open(&self, name: &str) -> bool {
        self.doc
            .query_selector(&format!("#menu:not(.hidden) #screen-{name}.open"))
            .ok()
            .flatten()
            .is_some()
    }

    /// True while the launch pad dialog is open (the KSC stays in 3D behind it).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn launchpad_active(&self) -> bool {
        self.doc
            .query_selector("#menu:not(.hidden) #screen-launchpad.open")
            .ok()
            .flatten()
            .is_some()
    }

    /// True while the KSC hub screen is the open menu screen.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn ksc_active(&self) -> bool {
        self.doc
            .query_selector("#menu:not(.hidden) #screen-ksc.open")
            .ok()
            .flatten()
            .is_some()
    }

    /// Move the facility tooltip over the hovered building, or hide it.
    #[cfg(target_arch = "wasm32")]
    fn place_ksc_tooltip(&self) {
        let Some(tip) = self
            .doc
            .get_element_by_id("ksc-tooltip")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        else {
            return;
        };
        let (Some(gpu), Some(i)) = (&self.gpu, self.ksc3d.hovered) else {
            tip.style().set_property("display", "none").ok();
            return;
        };
        let Some(b) = self.ksc3d.buildings().get(i) else {
            tip.style().set_property("display", "none").ok();
            return;
        };
        match self.ksc3d.label_anchor(gpu, i) {
            Some((x, y)) => {
                let (x, y) = (x / self.dpr, y / self.dpr);
                let text = if b.is_open() {
                    b.label().to_string()
                } else {
                    format!("{} (not yet)", b.label())
                };
                tip.set_text_content(Some(&text));
                tip.set_attribute("data-ose-facility", &b.facility).ok();
                tip.style().set_property("display", "block").ok();
                tip.style().set_property("left", &format!("{x:.0}px")).ok();
                tip.style()
                    .set_property("top", &format!("{:.0}px", y - 12.0))
                    .ok();
            }
            None => {
                tip.style().set_property("display", "none").ok();
            }
        }
    }

    /// A facility was clicked in the KSC scene: open its screen.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn open_facility(&mut self, facility: &str) {
        match facility {
            "VAB" => self.open_vab(None),
            "ResearchCenter" => self.open_rnd(),
            "AstronautComplex" => self.open_missions(),
            "TrainingCenter" => self.open_training(),
            "TrackingCenter" => self.open_tracking(),
            f if f.starts_with("Launchpad") => self.open_launchpad(),
            _ => {}
        }
    }

    fn is_3d(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.gpu.is_some()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    }

    fn set_focus(&mut self, i: usize) {
        self.follow = None;
        self.camera.focus = i;
        self.frame_focus();
        if let Some(scene) = &self.scene {
            self.focus_el
                .set_text_content(Some(&format!("focus: {}", scene.bodies[i].id)));
        }
    }

    fn on_server(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::Welcome {
                player_id,
                tick,
                token,
            } => {
                self.player_id = Some(player_id);
                store("ose.token", &token);
                self.time.update(tick.seconds(), 1, now_ms());
                self.set_status(&format!("player {player_id}"));
                set_text(
                    &self.doc,
                    "player",
                    &format!("{} (#{player_id})", self.player_name),
                );
                set_text(&self.doc, "menu-note", "");
                self.open_sessions_screen();
            }
            ServerMessage::Rejected { reason } => {
                self.set_status(&format!("rejected: {reason}"));
                set_text(&self.doc, "menu-note", &format!("rejected: {reason}"));
                if let Some(m) = self.doc.get_element_by_id("menu") {
                    m.class_list().remove_1("hidden").ok();
                }
            }
            ServerMessage::SessionList { sessions } => {
                self.render_sessions(&sessions);
            }
            ServerMessage::CraftList { crafts } => {
                if let Some(ws) = &self.ws {
                    for c in &crafts {
                        if !self.craft_docs.contains_key(&c.id) {
                            send(ws, &ClientMessage::GetCraft { id: c.id });
                        }
                    }
                }
                self.crafts = crafts;
                self.render_crafts();
                self.render_vab_load();
            }
            ServerMessage::CraftDocument {
                id,
                craft: Some(craft),
            } => {
                if self.flying.is_some() && self.flight_craft_id == Some(id) {
                    self.flight_craft = Some(craft.clone());
                }
                self.craft_docs.insert(id, craft);
                self.draw_thumbnail(id);
                self.mark_craft_research(id);
                if self.vab_craft_id == Some(id) && self.editor.is_empty() {
                    self.open_vab(Some(id));
                }
            }
            ServerMessage::CraftDocument { craft: None, .. } => {}
            ServerMessage::CraftSaved {
                id,
                summary,
                reason,
            } => match (id, summary) {
                (Some(id), Some(s)) => {
                    self.vab_craft_id = Some(id);
                    // The saved craft is the editor's: keep it, so Save and launch can draw it on the pad
                    // straight away, and ask for the server's copy to replace it.
                    self.craft_docs.insert(id, self.editor.craft.clone());
                    if let Some(ws) = &self.ws {
                        send(ws, &ClientMessage::GetCraft { id });
                    }
                    set_text(
                        &self.doc,
                        "vab-dv",
                        &format!("\u{394}v {:.0} m/s", s.delta_v_vacuum_mps.max(0.0) + 0.0),
                    );
                    set_text(
                            &self.doc,
                            "vab-report",
                            &format!(
                                "Saved as #{id}: {} parts, {} stages, {:.2} t dry, {:.2} t wet, {:.0} m/s vacuum delta-v",
                                s.parts, s.stages, s.dry_mass_t, s.wet_mass_t, s.delta_v_vacuum_mps
                            ),
                        );
                    if self.launch_after_save {
                        self.launch_after_save = false;
                        set_text(&self.doc, "menu-note", "rolling out to the pad...");
                        self.launch_craft(id);
                    }
                }
                _ => {
                    self.launch_after_save = false;
                    set_text(
                        &self.doc,
                        "vab-report",
                        &format!(
                            "Not saved: {}",
                            reason.unwrap_or_else(|| "the server gave no reason".into())
                        ),
                    );
                }
            },
            ServerMessage::FlightAck {
                accepted, reason, ..
            } => {
                set_text(
                    &self.doc,
                    "fh-note",
                    if accepted {
                        ""
                    } else {
                        reason.as_deref().unwrap_or("refused")
                    },
                );
            }
            ServerMessage::CraftLaunched {
                vessel,
                name,
                parts,
                wet_mass_t,
                delta_v_vacuum_mps,
                stages,
                reason,
                ..
            } => match (vessel, reason) {
                (Some(id), _) => {
                    if let Some(craft_id) = self.pending_launch.take() {
                        self.start_flight(id, Some(craft_id));
                    }
                    // Mission elapsed time counts from the launch (kept per vessel in this browser).
                    let ut = self.time.ut_at(now_ms());
                    store(&format!("ose.launchUt.{id}"), &format!("{ut:.1}"));
                    self.audio.play("launch_seq");
                    set_text(
                        &self.doc,
                        "menu-note",
                        &format!(
                            "launched {name}: {parts} parts, {wet_mass_t:.1} t, {delta_v_vacuum_mps:.0} m/s over {} stages",
                            stages.len()
                        ),
                    );
                    self.set_status(&format!("launched {name}"));
                    self.follow = Some(id);
                    self.show_menu(false);
                }
                (None, reason) => {
                    let msg = format!(
                        "launch refused: {}",
                        reason.unwrap_or_else(|| "unknown reason".into())
                    );
                    set_text(&self.doc, "menu-note", &msg);
                    // From the VAB the refusal belongs next to the craft, where the player is looking.
                    if self.vab_active() {
                        set_text(&self.doc, "vab-report", &msg);
                    }
                }
            },
            ServerMessage::DifficultyOptions {
                definitions,
                presets,
            } => {
                self.difficulty_defs = definitions;
                self.difficulty_presets = presets;
                self.render_presets();
                self.render_difficulty_form();
                self.apply_preset();
            }
            ServerMessage::CampaignState {
                id: campaign_id,
                name,
                mode,
                members,
                you_are_member,
                created,
                ..
            } => {
                let names: Vec<&str> = members.iter().map(|m| m.display_name.as_str()).collect();
                set_text(
                    &self.doc,
                    "campaign",
                    &format!(
                        "{name} ({}): {} in the campaign ({})",
                        mode_name(mode),
                        members.len(),
                        names.join(", ")
                    ),
                );
                if let Some(me) = self
                    .player_id
                    .and_then(|id| members.iter().find(|m| m.player_id == id))
                {
                    let swatch = me
                        .agency
                        .as_ref()
                        .map(|a| {
                            format!(
                                "<span class=\"swatch\" style=\"background:{}\"></span>",
                                rgb_css(a.primary_rgb)
                            )
                        })
                        .unwrap_or_default();
                    if let Some(el) = self.doc.get_element_by_id("player") {
                        el.set_inner_html(&format!(
                            "{swatch}{} (#{})",
                            esc(&me.display_name),
                            me.player_id
                        ));
                    }
                }
                self.campaign = Some((name, members));
                if self.campaign_id != campaign_id {
                    if let Some(scene) = &mut self.scene {
                        scene.reset_attitudes();
                    }
                }
                self.campaign_id = campaign_id;
                self.update_warp_holder();
                if created {
                    set_text(&self.doc, "menu-note", "session created");
                }
                if you_are_member {
                    if !self.in_session {
                        self.in_session = true;
                        self.open_ksc();
                        if let Some(ws) = &self.ws {
                            send(ws, &ClientMessage::ScienceInfo);
                            // The crew portraits and the launch dialog need the roster from the start.
                            send(ws, &ClientMessage::KerbalRoster);
                        }
                    }
                    if !self.agency_sent {
                        self.send_agency();
                    }
                } else {
                    // A listing or a create answer: refresh the list so the entry shows the new name.
                    if let Some(ws) = &self.ws {
                        send(ws, &ClientMessage::ListSessions);
                    }
                }
            }
            ServerMessage::ScienceState {
                tech_tree,
                capacity,
                allocated,
                unlocked,
                submitted,
                missions,
            } => {
                self.researched = if tech_tree {
                    unlocked.into_iter().collect()
                } else {
                    self.tech.iter().map(|n| n.id.clone()).collect()
                };
                self.science = Some(SciencePoints {
                    tech_tree,
                    capacity,
                    allocated,
                    missions,
                    submitted: submitted.len(),
                });
                self.render_rnd();
                self.render_missions();
                self.render_vab_picker();
                let ids: Vec<u64> = self.craft_docs.keys().copied().collect();
                for id in ids {
                    self.mark_craft_research(id);
                }
            }
            ServerMessage::TechUnlock {
                id,
                accepted,
                reason,
            } => {
                let msg = if accepted {
                    format!(
                        "researched {}",
                        rnd::text(
                            &self.loc,
                            &self
                                .tech
                                .iter()
                                .find(|n| n.id == id)
                                .map(|n| n.name_key.clone())
                                .unwrap_or(id.clone())
                        )
                    )
                } else {
                    reason.unwrap_or_else(|| "refused".into())
                };
                set_text(&self.doc, "rd-note", &msg);
            }
            ServerMessage::CrewAck {
                vessel,
                accepted,
                reason,
                new_vessel,
            } => {
                if !accepted {
                    set_text(
                        &self.doc,
                        "fh-note",
                        &reason.unwrap_or_else(|| "refused".into()),
                    );
                } else {
                    match (self.crew_pending.take(), new_vessel) {
                        // Out of the hatch: fly the Kerbal.
                        (Some("fh-eva"), Some(nv)) => {
                            set_text(&self.doc, "fh-note", "");
                            self.audio.play("eva_exit");
                            self.start_flight(nv, None);
                        }
                        (Some("fh-flag"), _) => {
                            set_text(&self.doc, "fh-note", "flag planted");
                            self.audio.play("flag_plant");
                        }
                        // Boarded: fly the craft the Kerbal climbed into.
                        (Some("fh-board"), _) if self.flying == Some(vessel) => {
                            self.audio.play("eva_enter");
                            if let Some(target) = self.board_target.take() {
                                let craft = stored(&self.craft_link_key(target))
                                    .and_then(|s| s.parse::<u64>().ok());
                                self.start_flight(target, craft);
                                set_text(&self.doc, "fh-note", "boarded");
                            }
                        }
                        _ => {}
                    }
                }
            }
            ServerMessage::Docked { vessel, absorbed } => {
                if self.flying == Some(absorbed) {
                    let craft =
                        stored(&self.craft_link_key(vessel)).and_then(|s| s.parse::<u64>().ok());
                    self.start_flight(vessel, craft);
                }
                if self.flying == Some(vessel) {
                    set_text(&self.doc, "fh-note", "docked");
                    self.audio.play("decouple");
                }
            }
            ServerMessage::Undocked {
                new_vessel, reason, ..
            } => {
                let msg = match (new_vessel, reason) {
                    (Some(_), _) => "undocked".to_string(),
                    (None, r) => r.unwrap_or_else(|| "could not undock".into()),
                };
                set_text(&self.doc, "fh-note", &msg);
                if new_vessel.is_some() {
                    self.audio.play("decouple");
                }
            }
            ServerMessage::MissionAck {
                accepted,
                reason,
                science,
                ..
            } => {
                let msg = match (accepted, reason) {
                    (true, _) if science > 0.0 => format!("Turned in: {science:.0} science"),
                    (true, _) => "Mission accepted".to_string(),
                    (false, r) => r.unwrap_or_else(|| "refused".into()),
                };
                if accepted {
                    self.audio.play("rd_research");
                }
                // The ScienceState that follows re-renders the log; keep the note across it.
                self.mc_note = msg.clone();
                set_text(&self.doc, "mc-note", &msg);
            }
            ServerMessage::ScienceReports {
                new,
                skipped,
                carried,
                reason,
                running,
                ..
            } => {
                self.running = running;
                self.running_since_ms = now_ms();
                let msg = match reason {
                    Some(r) => r,
                    None => format!(
                        "{} new report{}, {} carried{}",
                        new.len(),
                        if new.len() == 1 { "" } else { "s" },
                        carried.len(),
                        if skipped.is_empty() {
                            String::new()
                        } else {
                            format!(", {} experiments could not run here", skipped.len())
                        }
                    ),
                };
                set_text(&self.doc, "fh-note", &msg);
            }
            ServerMessage::ExperimentFinished {
                part,
                experiment,
                reports,
                interrupted,
                ..
            } => {
                self.running
                    .retain(|r| !(r.part == part && r.experiment == experiment));
                let name = experiment_name(&experiment);
                let msg = if interrupted {
                    format!("{name} stopped: the vessel left the situation it started in")
                } else {
                    format!(
                        "{name} finished: {} new report{}",
                        reports.len(),
                        if reports.len() == 1 { "" } else { "s" }
                    )
                };
                set_text(&self.doc, "fh-note", &msg);
                self.audio.play(if interrupted {
                    "ui_esc_open"
                } else {
                    "rd_research"
                });
            }
            ServerMessage::ScienceSubmitted {
                reports,
                earned,
                recovered,
                reason,
                ..
            } => {
                let msg = match reason {
                    Some(r) => r,
                    None => format!(
                        "{} {} report{} for {:.0} science",
                        if recovered {
                            "recovered"
                        } else {
                            "transmitted"
                        },
                        reports.len(),
                        if reports.len() == 1 { "" } else { "s" },
                        earned
                    ),
                };
                set_text(&self.doc, "fh-note", &msg);
                if recovered {
                    self.show_flight_report("Recovered", Some(earned));
                    self.tracking_open = false;
                    self.end_flight();
                    self.open_ksc();
                }
            }
            ServerMessage::BodyRotation { bodies } => {
                self.body_spins = bodies;
                self.apply_spins();
            }
            ServerMessage::BodyCatalog { bodies } => {
                let scene = Scene::new(bodies);
                self.build_labels(&scene);
                let focus = scene
                    .index_of("Kerbin")
                    .or_else(|| scene.root())
                    .unwrap_or(0);
                // Frame the whole system first so the player sees everything, centred on the star.
                let root = scene.root().unwrap_or(focus);
                self.scene = Some(scene);
                self.apply_spins();
                self.set_focus(root);
            }
            ServerMessage::TimeState {
                tick,
                warp,
                warp_limited_by,
            } => {
                self.time.update(tick.seconds(), warp, now_ms());
                if warp_limited_by != self.warp_holder {
                    self.warp_holder = warp_limited_by;
                    self.update_warp_holder();
                }
            }
            ServerMessage::Snapshot { bodies, .. } => {
                if let Some(scene) = &mut self.scene {
                    scene.snapshot = bodies;
                }
            }
            ServerMessage::Vessels { vessels, tick } => {
                let ids: Vec<u64> = vessels.iter().map(|v| v.id).collect();
                let known: Vec<u64> = self.vessel_els.iter().map(|(id, _)| *id).collect();
                let changed = ids != known;
                if changed {
                    self.build_vessel_labels(&vessels);
                }
                let mine = vessels
                    .iter()
                    .find(|v| v.owner.is_some() && v.owner == self.player_id)
                    .map(|v| (v.id, v.reference_body as usize));
                if let Some(scene) = &mut self.scene {
                    scene.update_vessels(vessels, tick.seconds(), now_ms(), self.time.warp);
                }
                // Follow the player's own vessel the first time it appears.
                if changed && self.follow.is_none() {
                    if let Some((id, rb)) = mine {
                        self.follow_vessel(id, rb);
                    }
                }
            }
            ServerMessage::Trajectory {
                vessel, patches, ..
            } => {
                if let Some(scene) = &mut self.scene {
                    scene.trajectories.insert(vessel, patches);
                }
                self.traj_version = self.traj_version.wrapping_add(1);
            }
            ServerMessage::BurnState {
                vessel,
                scheduled,
                reason,
            } => {
                if self.node.map(|n| n.vessel) == Some(vessel) {
                    let text = match (scheduled, reason) {
                        (true, _) => String::new(),
                        (false, Some(r)) => r,
                        (false, None) => "no burn scheduled".into(),
                    };
                    set_text(&self.doc, "node-reason", &text);
                }
            }
            ServerMessage::Pong { .. } => {}
            ServerMessage::StageReport {
                stages,
                delta_v_vacuum_mps,
                com_m,
                cot_m,
                thrust_dir,
                col_m,
            } => {
                // Zero without wings.
                self.vab_col = (col_m.length() > 1e-9).then_some(col_m);
                self.vab_stages_readout = stages;
                // The server sends zeros for all three when the craft has no engines.
                self.vab_com =
                    (com_m.length() > 1e-9 || thrust_dir.length() > 0.5).then_some(com_m);
                self.vab_cot = (thrust_dir.length() > 0.5).then_some((cot_m, thrust_dir));
                set_text(
                    &self.doc,
                    "vab-dv",
                    // `+ 0.0` turns -0 into 0, which `{:.0}` would print as "-0".
                    &format!("\u{394}v {:.0} m/s", delta_v_vacuum_mps.max(0.0) + 0.0),
                );
                if self.vab_active() {
                    self.render_vab_view();
                }
                self.render_trip();
            }
            ServerMessage::KerbalRoster { kerbals } => {
                self.roster = kerbals;
                if self.screen_open("training") {
                    self.render_training();
                }
                if self.screen_open("launchpad") {
                    self.render_launchpad_info();
                }
            }
            ServerMessage::LaunchSites { sites } => {
                self.launch_sites = sites;
                if self
                    .launch_site
                    .as_ref()
                    .is_some_and(|s| !self.launch_sites.iter().any(|l| &l.id == s))
                {
                    self.launch_site = None;
                }
                self.render_launch_sites();
            }
            ServerMessage::SaveAck {
                action,
                accepted,
                reason,
                craft,
            } => {
                let what = match action {
                    ose_net::SaveAction::QuickSave => "Quicksave",
                    ose_net::SaveAction::QuickLoad => "Quickload",
                    ose_net::SaveAction::RevertToLaunch => "Revert to launch",
                    ose_net::SaveAction::RevertToEditor => "Revert to the VAB",
                };
                if accepted {
                    if action != ose_net::SaveAction::QuickSave {
                        if let Some(scene) = &mut self.scene {
                            scene.reset_attitudes();
                        }
                    }
                    hud::push(&self.doc, &format!("{what} done"), "triumph");
                    if let (ose_net::SaveAction::RevertToEditor, Some(c)) = (action, craft) {
                        self.end_flight();
                        self.vab_history.record(&self.editor);
                        self.editor = vab::Editor::from_craft(c);
                        let cat = std::mem::take(&mut self.catalog);
                        self.editor.rebuild_stages(&cat);
                        self.catalog = cat;
                        self.open_vab(None);
                    }
                } else {
                    let why = reason.unwrap_or_else(|| "refused".into());
                    hud::push(&self.doc, &format!("{what}: {why}"), "alert");
                }
            }
            // Messages for features this client does not draw yet are ignored.
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    fn build_labels(&mut self, scene: &Scene) {
        self.labels.set_inner_html("");
        self.label_els.clear();
        for (i, b) in scene.bodies.iter().enumerate() {
            let el: HtmlElement = match self
                .doc
                .create_element("div")
                .ok()
                .and_then(|e| e.dyn_into().ok())
            {
                Some(e) => e,
                None => continue,
            };
            let class = if b.is_star {
                format!("{BODY_LABEL_CLASS} star")
            } else {
                BODY_LABEL_CLASS.to_string()
            };
            el.set_class_name(&class);
            el.set_attribute("data-ose", "body").ok();
            el.set_attribute("data-ose-id", &b.id).ok();
            el.set_attribute("data-ose-index", &i.to_string()).ok();
            el.set_text_content(Some(&b.id));
            self.labels.append_child(&el).ok();
            self.label_els.push(el);
        }
    }

    fn build_vessel_labels(&mut self, vessels: &[ose_net::VesselState]) {
        for (_, el) in self.vessel_els.drain(..) {
            el.remove();
        }
        for v in vessels {
            let Some(el) = self
                .doc
                .create_element("div")
                .ok()
                .and_then(|e| e.dyn_into::<HtmlElement>().ok())
            else {
                continue;
            };
            let mine = v.owner.is_some() && v.owner == self.player_id;
            el.set_class_name(if mine { "vessel mine" } else { "vessel" });
            if Some(ose_net::TargetId::Vessel(v.id)) == self.target {
                el.class_list().add_1("target").ok();
            }
            el.set_attribute("data-ose", "vessel").ok();
            el.set_attribute("data-ose-id", &v.id.to_string()).ok();
            el.set_attribute("data-ose-vessel", &v.id.to_string()).ok();
            el.set_text_content(Some(&v.name));
            self.labels.append_child(&el).ok();
            self.vessel_els.push((v.id, el));
        }
    }

    /// Local storage key for the craft a vessel was launched from: vessel ids restart in every campaign.
    fn craft_link_key(&self, vessel: u64) -> String {
        format!("ose.flight.{}.{vessel}", self.campaign_id)
    }

    fn apply_spins(&mut self) {
        let omega: Vec<f64> = self
            .body_spins
            .iter()
            .map(|b| {
                if b.rotation_period_s > 0.0 {
                    std::f64::consts::TAU / b.rotation_period_s
                } else {
                    0.0
                }
            })
            .collect();
        if let Some(scene) = &mut self.scene {
            scene.spin_omega = omega;
        }
    }

    /// Leave the campaign for the main menu: the server keeps it on the list to resume (protocol 22).
    fn leave_session(&mut self) {
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::LeaveSession);
        }
        self.end_flight();
        self.in_session = false;
        self.tracking_open = false;
        self.campaign = None;
        self.science = None;
        self.lp_selected = None;
        self.last_stage = None;
        self.last_situation = None;
        if let Some(scene) = &mut self.scene {
            scene.vessels.clear();
            scene.trajectories.clear();
            scene.reset_attitudes();
        }
        let vessels: Vec<ose_net::VesselState> = Vec::new();
        self.build_vessel_labels(&vessels);
        set_text(&self.doc, "campaign", "");
        set_text(&self.doc, "menu-note", "");
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        self.menu_screen("sessions");
    }

    /// Centre the camera on a vessel: focus its reference body (so orbits draw around it) and follow.
    /// `]` / `[` in flight: take control of the next or previous of your vessels within 2.5 km (the
    /// original's FocusNext/FocusPrev over the vessels loaded around you: stages dropped, Kerbals on EVA).
    fn switch_vessel(&mut self, back: bool) {
        let Some(cur) = self.flying else { return };
        let Some(scene) = &self.scene else { return };
        let ut = self.time.ut_at(now_ms());
        let Some(here) = scene.vessel(cur).map(|v| scene.vessel_position(v, ut)) else {
            return;
        };
        let mut ids: Vec<u64> = scene
            .vessels
            .iter()
            .filter(|v| v.owner.is_some() && v.owner == self.player_id && v.flight.is_some())
            .filter(|v| (scene.vessel_position(v, ut) - here).length() < 2500.0)
            .map(|v| v.id)
            .collect();
        ids.sort();
        if ids.len() < 2 {
            hud::push(&self.doc, "No other vessel of yours nearby", "passive");
            return;
        }
        let i = ids.iter().position(|id| *id == cur).unwrap_or(0);
        let n = ids.len();
        let next = ids[if back { (i + n - 1) % n } else { (i + 1) % n }];
        let craft = stored(&format!("ose.flight.{next}")).and_then(|c| c.parse::<u64>().ok());
        self.start_flight(next, craft);
        if let Some(v) = self.scene.as_ref().and_then(|s| s.vessel(next)) {
            hud::push(&self.doc, &format!("Switched to {}", v.name), "passive");
        }
    }

    /// `]` / `[` on the map: follow the next or previous vessel (yours first), as the original's FocusNext.
    fn cycle_vessel(&mut self, back: bool) {
        let Some(scene) = &self.scene else { return };
        let mut ids: Vec<(bool, u64, usize)> = scene
            .vessels
            .iter()
            .map(|v| {
                (
                    !(v.owner.is_some() && v.owner == self.player_id),
                    v.id,
                    v.reference_body as usize,
                )
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        ids.sort();
        let n = ids.len();
        let i = self
            .follow
            .and_then(|f| ids.iter().position(|(_, id, _)| *id == f));
        let j = match (i, back) {
            (None, _) => 0,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        let (_, id, body) = ids[j];
        self.follow_vessel(id, body);
    }

    fn follow_vessel(&mut self, id: u64, reference_body: usize) {
        self.follow = Some(id);
        self.camera.focus = reference_body;
        if let Some(scene) = &self.scene {
            if let Some(v) = scene.vessel(id) {
                let body = &scene.bodies[reference_body];
                let a = scene::orbit_from(body.mu, &v.orbit)
                    .apoapsis()
                    .abs()
                    .max(body.radius_m * 1.5);
                self.camera.frame(a * 1.3);
                self.orbit_dist_km = (a / 1000.0) * 2.5;
                self.focus_el
                    .set_text_content(Some(&format!("following: {}", v.name)));
            }
        }
    }

    fn show_menu(&self, open: bool) {
        if let Some(m) = self.doc.get_element_by_id("menu") {
            if open {
                m.class_list().remove_1("hidden").ok();
            } else {
                m.class_list().add_1("hidden").ok();
            }
        }
        if open {
            self.menu_screen(if self.in_session {
                "paused"
            } else if self.ws.is_some() {
                "sessions"
            } else {
                "name"
            });
        }
    }

    /// Show one of the menu screens: "name", "sessions" or "paused".
    fn menu_screen(&self, which: &str) {
        for id in [
            "name",
            "sessions",
            "ksc",
            "launchpad",
            "vab",
            "rnd",
            "mission",
            "training",
            "paused",
        ] {
            if let Some(el) = self.doc.get_element_by_id(&format!("screen-{id}")) {
                if id == which {
                    el.class_list().add_1("open").ok();
                } else {
                    el.class_list().remove_1("open").ok();
                }
            }
        }
        if let Some(card) = self.doc.get_element_by_id("menu-card") {
            if which == "vab" {
                card.class_list().add_1("vab").ok();
            } else {
                card.class_list().remove_1("vab").ok();
            }
            if matches!(which, "sessions" | "ksc" | "launchpad") {
                card.class_list().add_1("wide").ok();
            } else {
                card.class_list().remove_1("wide").ok();
            }
        }
    }

    /// Ask the server what can be joined and show the sessions screen.
    fn open_sessions_screen(&mut self) {
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        self.menu_screen("sessions");
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::ListSessions);
        }
    }

    fn open_ksc(&self) {
        self.ftue("FTUE_New_Game");
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        set_text(&self.doc, "menu-note", "");
        self.menu_screen("ksc");
    }

    // ---- Mission Control

    fn open_missions(&mut self) {
        self.ftue("FTUE_Mission_Control");
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        set_text(&self.doc, "menu-note", "");
        if self.mc_selected.is_none() {
            self.mc_selected = self
                .missions
                .iter()
                .find(|m| m.primary == self.mc_primary)
                .map(|m| m.id.clone());
        }
        self.menu_screen("mission");
        self.render_missions();
    }

    fn render_missions(&self) {
        let sel = self.mc_selected.as_deref();
        let progress = self.science.as_ref().map(|s| s.missions.as_slice());
        if let Some(el) = self.doc.get_element_by_id("mc-list") {
            el.set_inner_html(&missions::render_list(
                &self.missions,
                &self.loc,
                self.mc_primary,
                sel,
                progress,
            ));
        }
        for (id, on) in [
            ("mc-tab-primary", self.mc_primary),
            ("mc-tab-secondary", !self.mc_primary),
        ] {
            if let Some(el) = self.doc.get_element_by_id(id) {
                el.class_list().toggle_with_force("on", on).ok();
            }
        }
        if let (Some(el), Some(m)) = (
            self.doc.get_element_by_id("mc-briefing"),
            sel.and_then(|id| self.missions.iter().find(|m| m.id == id)),
        ) {
            let info = progress.and_then(|p| p.iter().find(|i| i.id == m.id));
            el.set_inner_html(&missions::render_briefing(m, &self.loc, info));
            set_text(&self.doc, "mc-note", &self.mc_note);
        }
    }

    // ---- Research and Development

    fn open_rnd(&mut self) {
        self.ftue("FTUE_RND");
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        set_text(&self.doc, "menu-note", "");
        if self.rd_selected.is_none() {
            self.rd_selected = self
                .tech
                .iter()
                .find(|n| n.requires.is_empty())
                .map(|n| n.id.clone());
        }
        self.menu_screen("rnd");
        self.render_rnd();
        if let Ok(Some(el)) = self.doc.query_selector("#rd-tree .rd-node.selected") {
            el.scroll_into_view_with_bool(false);
        }
    }

    fn render_rnd(&self) {
        let sel = self.rd_selected.as_deref();
        if let Some(el) = self.doc.get_element_by_id("rd-tree") {
            el.set_inner_html(&rnd::render_tree(
                &self.tech,
                &self.loc,
                &self.researched,
                sel,
            ));
        }
        if let (Some(el), Some(node)) = (
            self.doc.get_element_by_id("rd-info"),
            sel.and_then(|id| self.tech.iter().find(|n| n.id == id)),
        ) {
            let title = |pid: &str| {
                self.catalog
                    .get(pid)
                    .map(|p| p.display_title().to_string())
                    .unwrap_or_else(|| pid.to_string())
            };
            let free = self
                .science
                .as_ref()
                .filter(|s| s.tech_tree)
                .map(|s| s.capacity.saturating_sub(s.allocated));
            el.set_inner_html(&rnd::render_info(
                node,
                &self.loc,
                &self.researched,
                &title,
                free,
            ));
        }
        let total: f64 = self
            .tech
            .iter()
            .filter(|n| self.researched.contains(&n.id))
            .map(|n| n.required_science)
            .sum();
        let points = match &self.science {
            Some(s) if !s.tech_tree => " \u{b7} sandbox: everything is unlocked".to_string(),
            Some(s) => format!(
                " \u{b7} {} science points free of {} \u{b7} {} reports submitted",
                s.capacity.saturating_sub(s.allocated),
                s.capacity,
                s.submitted
            ),
            None => String::new(),
        };
        set_text(
            &self.doc,
            "rd-summary",
            &format!(
                "{} of {} technologies researched \u{b7} {:.0} science spent{points}",
                self.researched.len(),
                self.tech.len(),
                total
            ),
        );
    }

    // ---- Vehicle Assembly Building

    /// Open the editor, empty or on a library craft (its document is fetched if not cached).
    fn open_vab(&mut self, craft_id: Option<u64>) {
        self.ftue("FTUE_VAB");
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        self.menu_screen("vab");
        self.launch_after_save = false;
        match craft_id {
            Some(id) => {
                self.vab_craft_id = Some(id);
                if let Some(c) = self.craft_docs.get(&id) {
                    self.editor = vab::Editor::from_craft(c.clone());
                    let cat = std::mem::take(&mut self.catalog);
                    self.editor.rebuild_stages(&cat);
                    self.catalog = cat;
                } else if let Some(ws) = &self.ws {
                    send(ws, &ClientMessage::GetCraft { id });
                }
            }
            None => {
                if self.editor.is_empty() {
                    self.vab_craft_id = None;
                }
            }
        }
        set_text(&self.doc, "vab-report", "");
        self.render_vab_picker();
        self.render_vab();
    }

    fn render_vab_picker(&self) {
        let Some(cats) = self.doc.get_element_by_id("vab-cats") else {
            return;
        };
        let mut html = String::new();
        // Favourites first, as the original's picker has them (middle click a part to add it).
        let mut all = vec!["Favorites".to_string()];
        all.extend(vab::categories(&self.catalog));
        for c in all {
            let on = if c == self.vab_category { " on" } else { "" };
            html.push_str(&format!(
                "<button class=\"{on}\" data-ose-category=\"{c}\">{c}</button>",
                on = on.trim(),
                c = esc(&c)
            ));
        }
        cats.set_inner_html(&html);
        let Some(list) = self.doc.get_element_by_id("vab-parts") else {
            return;
        };
        let query = input_string(&self.doc, "vab-search")
            .unwrap_or_default()
            .to_lowercase();
        let mut parts: Vec<&vab::PartSpec> = self
            .catalog
            .values()
            .filter(|p| {
                if query.is_empty() {
                    if self.vab_category == "Favorites" {
                        self.vab_favs.contains(&p.id)
                    } else {
                        p.category == self.vab_category
                    }
                } else {
                    p.display_title().to_lowercase().contains(&query)
                        || p.id.to_lowercase().contains(&query)
                }
            })
            .filter(|p| self.vab_size.as_ref().is_none_or(|s| &p.size_category == s))
            .collect();
        let size_rank = |s: &str| {
            ["XS", "S", "M", "L", "XL", "XXL"]
                .iter()
                .position(|x| *x == s)
                .unwrap_or(9)
        };
        match self.vab_sort {
            1 => parts.sort_by(|a, b| a.display_title().cmp(b.display_title())),
            2 => parts.sort_by(|a, b| a.mass_t.total_cmp(&b.mass_t)),
            _ => parts.sort_by(|a, b| {
                size_rank(&a.size_category)
                    .cmp(&size_rank(&b.size_category))
                    .then(a.mass_t.total_cmp(&b.mass_t))
                    .then_with(|| a.display_title().cmp(b.display_title()))
            }),
        }
        if let Some(chips) = self.doc.get_element_by_id("vab-sizes") {
            let mut h = String::new();
            for s in ["All", "XS", "S", "M", "L", "XL"] {
                let on = match &self.vab_size {
                    None => s == "All",
                    Some(v) => v == s,
                };
                h.push_str(&format!(
                    "<button data-size=\"{s}\" class=\"{}\" title=\"{}\">{s}</button>",
                    if on { "on" } else { "" },
                    if s == "All" {
                        "every size".to_string()
                    } else {
                        format!("size {s} parts only")
                    }
                ));
            }
            chips.set_inner_html(&h);
        }
        // Parts of technologies the campaign has not researched are greyed out (the pad refuses them).
        let locked: std::collections::HashSet<&str> =
            if self.science.as_ref().map(|s| s.tech_tree).unwrap_or(false) {
                self.tech
                    .iter()
                    .filter(|n| !self.researched.contains(&n.id))
                    .flat_map(|n| n.parts.iter().map(|p| p.as_str()))
                    .collect()
            } else {
                Default::default()
            };
        let mut html = String::new();
        for p in parts {
            let lock = locked.contains(p.id.as_str());
            html.push_str(&format!(
                "<li data-ose=\"vab-part\" data-ose-part=\"{id}\"{lock}><img class=\"pi\" src=\"assets/ui/parts/{id}.png\" alt=\"\" loading=\"lazy\" draggable=\"false\" onerror=\"this.parentNode.classList.add('noicon');this.remove()\" /><span class=\"pt\">{fav}{title}</span><small>{cat} \u{b7} size {size} \u{b7} {mass:.2} t{extra}</small></li>",
                id = esc(&p.id),
                lock = if lock {
                    " class=\"locked\" title=\"not researched yet: the launch pad will refuse it\""
                } else {
                    ""
                },
                title = esc(p.display_title()),
                fav = if self.vab_favs.contains(&p.id) {
                    "<span class=\"fav\" title=\"favourite (middle click to remove)\">\u{2605}</span> "
                } else {
                    ""
                },
                cat = esc(&p.category),
                size = esc(&p.size_category),
                mass = p.mass_t,
                extra = if p.has_engine {
                    " \u{b7} engine"
                } else if p.is_decoupler {
                    " \u{b7} decoupler"
                } else if p.crew_capacity > 0 {
                    " \u{b7} crew"
                } else {
                    ""
                },
            ));
        }
        if html.is_empty() {
            html = if self.catalog.is_empty() {
                "<li>part catalog not loaded (server has no /parts.json)</li>".into()
            } else {
                "<li>no parts match</li>".into()
            };
        }
        list.set_inner_html(&html);
    }

    fn vab_add(&mut self, part_id: &str) {
        self.vab_history.record(&self.editor);
        let cat = std::mem::take(&mut self.catalog);
        let result = match cat.get(part_id) {
            Some(spec) => self.editor.add_part(spec, &cat).map(|_| ()),
            None => Err(format!("unknown part {part_id}")),
        };
        self.catalog = cat;
        match result {
            Ok(()) => self.lesson_part(part_id, true),
            Err(e) => set_text(&self.doc, "vab-report", &e),
        }
        self.render_vab();
    }

    fn vab_remove(&mut self) {
        if self.editor.selected.is_none() {
            return;
        }
        self.vab_history.record(&self.editor);
        let cat = std::mem::take(&mut self.catalog);
        self.editor.remove_selected(&cat);
        self.catalog = cat;
        self.render_vab();
    }

    fn vab_clear(&mut self) {
        self.vab_history.record(&self.editor);
        let name = self.editor.craft.name.clone();
        self.editor = vab::Editor::new(&name);
        self.vab_craft_id = None;
        set_text(&self.doc, "vab-report", "");
        self.render_vab();
    }

    /// Show the part tooltip for a picker row (`None` hides it): the original's title, subtitle and
    /// manufacturer, mass, size, crew, tanks, engine thrust and specific impulse, and the description.
    fn vab_tooltip(&self, row: Option<&Element>) {
        let Some(tip) = self.doc.get_element_by_id("vab-tip") else {
            return;
        };
        let spec = row
            .and_then(|r| r.get_attribute("data-ose-part"))
            .and_then(|id| self.catalog.get(&id));
        let (Some(row), Some(spec)) = (row, spec) else {
            tip.class_list().remove_1("open").ok();
            return;
        };
        let loc = |k: &str| self.loc.get(&format!("Parts/{k}/{}", spec.id)).cloned();
        let mut rows = vec![
            ("Mass".to_string(), format!("{:.3} t", spec.mass_t)),
            ("Size".to_string(), spec.size_category.clone()),
        ];
        if spec.crew_capacity > 0 {
            rows.push(("Crew".into(), spec.crew_capacity.to_string()));
        }
        for c in &spec.resource_containers {
            rows.push((c.resource.clone(), format!("{:.0} units", c.capacity_units)));
        }
        for e in &spec.engines {
            rows.push(("Thrust".into(), format!("{:.1} kN", e.max_thrust_kn)));
            rows.push((
                "Isp".into(),
                format!(
                    "{:.0} s vac / {:.0} s ASL",
                    e.isp_vacuum_s, e.isp_sea_level_s
                ),
            ));
        }
        let table: String = rows
            .iter()
            .map(|(k, v)| format!("<tr><td>{}</td><td>{}</td></tr>", esc(k), esc(v)))
            .collect();
        let sub = [loc("Subtitle"), loc("Manufacturer")]
            .into_iter()
            .flatten()
            .map(|s| esc(&s))
            .collect::<Vec<_>>()
            .join(" \u{b7} ");
        let locked = if row.class_list().contains("locked") {
            "<p style=\"color:#f0a070\">Not researched yet: the launch pad will refuse a craft with it.</p>"
        } else {
            ""
        };
        tip.set_inner_html(&format!(
            "<h4>{}</h4><div class=\"sub\">{}</div><table>{}</table>{}<p>{}</p>",
            esc(spec.display_title()),
            sub,
            table,
            locked,
            esc(&loc("Description").unwrap_or_default()),
        ));
        let r = row.get_bounding_client_rect();
        if let Some(t) = tip.dyn_ref::<HtmlElement>() {
            let top = r.top().min(
                web_sys::window()
                    .and_then(|w| w.inner_height().ok())
                    .and_then(|h| h.as_f64())
                    .unwrap_or(900.0)
                    - 320.0,
            );
            // Right of the row, or left of it when that would run off the screen (the R&D panel is on the right).
            let width = web_sys::window()
                .and_then(|w| w.inner_width().ok())
                .and_then(|w| w.as_f64())
                .unwrap_or(1600.0);
            let left = if r.right() + 324.0 > width {
                r.left() - 312.0
            } else {
                r.right() + 12.0
            };
            t.style().set_property("left", &format!("{left}px")).ok();
            t.style()
                .set_property("top", &format!("{}px", top.max(8.0)))
                .ok();
        }
        tip.class_list().add_1("open").ok();
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    /// Where the held part would go on a part's side under the cursor, and how many symmetry copies (a
    /// lifted fragment goes on alone). `None` when the part has no surface node or the cursor is on no part.
    fn vab_surface(&self, id: &str) -> Option<(vab::SurfaceSpot, u32)> {
        #[cfg(target_arch = "wasm32")]
        {
            let spec = self.catalog.get(id)?;
            vab::surface_node(spec)?;
            let (target, hit) = self.vab3d.surface_hit(self.gpu.as_ref()?)?;
            let spot = self.editor.surface_spot(
                target,
                hit,
                &self.catalog,
                self.vab_snap.then_some(15.0),
            )?;
            let sym = if self.vab_frag.is_some() {
                1
            } else {
                self.vab_symmetry
            };
            Some((
                vab::SurfaceSpot {
                    turn: self.vab_turn,
                    ..spot
                },
                sym,
            ))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = id;
            None
        }
    }

    fn vab_holding(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.vab3d.held.is_some()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    }

    /// W/S pitch, A/D yaw, Q/E roll the held part: 90 degrees a press with angle snap, 15 without (5 with
    /// Shift), as the original's rotate keys; Space puts it back upright.
    fn vab_turn_held(&mut self, code: &str, fine: bool) {
        let step = if fine {
            5.0
        } else if self.vab_snap {
            90.0
        } else {
            15.0
        };
        let (axis, sign) = match code {
            "KeyW" => (0, 1.0),
            "KeyS" => (0, -1.0),
            "KeyA" => (1, 1.0),
            "KeyD" => (1, -1.0),
            "KeyQ" => (2, 1.0),
            "KeyE" => (2, -1.0),
            _ => {
                self.vab_turn = vab::NO_TURN;
                self.vab_yaw = vab::NO_TURN;
                self.vab_sync_turn();
                return;
            }
        };
        let t = vab::axis_turn(axis, sign * step);
        // The frame of reference (F): about the craft's fixed axes, or about the part's own.
        self.vab_turn = if self.vab_local_frame {
            vab::turn_then(self.vab_turn, t)
        } else {
            vab::turn_then(t, self.vab_turn)
        };
        if axis == 1 {
            self.vab_yaw = vab::turn_then(t, self.vab_yaw);
        }
        self.vab_sync_turn();
    }

    fn vab_sync_turn(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            self.vab3d.held_turn = self.vab_turn;
            self.vab3d.held_yaw = self.vab_yaw;
        }
    }

    /// Ctrl+V: a copy of the clipboard in hand, following the cursor until a click puts it down.
    fn vab_paste(&mut self) {
        let Some(frag) = self.vab_clipboard.clone() else {
            return;
        };
        #[cfg(target_arch = "wasm32")]
        {
            self.vab3d.held = Some((
                frag.root_id().to_string(),
                vab_nodes(&self.catalog, frag.root_id()),
            ));
            if let Some(b) = self.doc.body() {
                b.class_list().add_1("vab-holding").ok();
            }
        }
        self.vab_frag = Some(frag);
        self.vab_pasting = true;
        self.vab_turn = vab::NO_TURN;
        self.vab_yaw = vab::NO_TURN;
        self.vab_sync_turn();
    }

    fn vab_cancel_paste(&mut self) {
        self.vab_frag = None;
        self.vab_pasting = false;
        #[cfg(target_arch = "wasm32")]
        {
            self.vab3d.held = None;
        }
        if let Some(b) = self.doc.body() {
            b.class_list().remove_1("vab-holding").ok();
        }
    }

    /// X / Shift+X: the next or previous symmetry count.
    fn vab_cycle_symmetry(&mut self, back: bool) {
        let n = vab::SYMMETRY.len();
        let i = vab::SYMMETRY
            .iter()
            .position(|s| *s == self.vab_symmetry)
            .unwrap_or(0);
        let j = if back { (i + n - 1) % n } else { (i + 1) % n };
        self.vab_symmetry = vab::SYMMETRY[j];
        self.render_vab();
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn vab_record(&mut self) {
        self.vab_history.record(&self.editor);
    }

    /// The flight report window for the flight's records, how it ended and the science recovered.
    fn show_flight_report(&self, ended: &str, science: Option<f64>) {
        let Some(r) = self.flight_records.borrow().clone() else {
            return;
        };
        let Some(el) = self.doc.get_element_by_id("flight-report") else {
            return;
        };
        let ut = self.time.ut_at(now_ms());
        el.set_inner_html(&hud::flight_report_html(&r, ended, ut, science));
        el.class_list().add_1("open").ok();
    }

    /// Fill the delta-v tool's body list: Kerbin at sea level, then every body with a surface, in vacuum.
    fn render_twr_bodies(&self) {
        let Some(el) = self.doc.get_element_by_id("vab-twr-body") else {
            return;
        };
        let Some(scene) = &self.scene else { return };
        let sel = |v: &str| {
            if self.vab_twr_body.as_deref() == Some(v) {
                " selected"
            } else {
                ""
            }
        };
        let mut html = format!(
            "<option value=\"\">Kerbin, sea level</option><option value=\"Kerbin@vacuum\"{}>Kerbin, vacuum</option>",
            sel("Kerbin@vacuum")
        );
        for b in scene
            .bodies
            .iter()
            .filter(|b| !b.is_star && b.id != "Kerbin")
        {
            let air = vab::surface_pressure_atm(&b.id);
            html.push_str(&format!(
                "<option value=\"{id}\"{s}>{id}, {what}</option>",
                id = esc(&b.id),
                s = sel(&b.id),
                what = if air > 0.0 {
                    format!("surface ({air} atm)")
                } else {
                    "surface (vacuum)".into()
                }
            ));
        }
        el.set_inner_html(&html);
    }

    /// Fill the trip planner's from/to lists (surfaces and orbits of every body).
    fn render_trip_options(&self) {
        let Some(map) = &self.trip else { return };
        let mut html = String::new();
        for n in map.places() {
            html.push_str(&format!(
                "<option value=\"{}\">{}</option>",
                esc(&n.name),
                esc(&map.label(&n.name))
            ));
        }
        for (id, default) in [("trip-from", "kerbin-surf"), ("trip-to", "mun-surf")] {
            if let Some(el) = self
                .doc
                .get_element_by_id(id)
                .and_then(|e| e.dyn_into::<web_sys::HtmlSelectElement>().ok())
            {
                el.set_inner_html(&html);
                el.set_value(default);
            }
        }
        if let Some(b) = self
            .doc
            .get_element_by_id("vab-trip")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            b.style().set_property("display", "").ok();
        }
        self.render_trip();
    }

    /// The trip planner's answer for the chosen trip, against the craft's delta-v.
    fn render_trip(&self) {
        let Some(map) = &self.trip else { return };
        let value = |id: &str| {
            self.doc
                .get_element_by_id(id)
                .and_then(|e| e.dyn_into::<web_sys::HtmlSelectElement>().ok())
                .map(|s| s.value())
                .unwrap_or_default()
        };
        let round = self
            .doc
            .get_element_by_id("trip-round")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|i| i.checked())
            .unwrap_or(false);
        let (from, to) = (value("trip-from"), value("trip-to"));
        let have: f64 = self
            .vab_stages_readout
            .iter()
            .map(|r| r.delta_v_vacuum_mps.max(0.0))
            .sum();
        let text = match map.plan(&from, &to, round) {
            Some(p) => {
                let need = p.delta_v_mps + p.inclination_mps;
                let mut s = format!("<b>\u{394}v {:.0} m/s</b>", need);
                if p.aerobrake_mps > 0.0 {
                    s.push_str(&format!(
                        " <small>({:.0} m/s less with aerobraking)</small>",
                        p.aerobrake_mps
                    ));
                }
                if let Some(a) = p.phase_angle {
                    s.push_str(&format!(
                        "<br><small>Transfer window: phase angle {a:.0}\u{b0}</small>"
                    ));
                }
                if have > 0.0 {
                    let ok = have >= need - p.aerobrake_mps;
                    s.push_str(&format!(
                        "<br><span class=\"{}\">This vehicle: {:.0} m/s {}</span>",
                        if ok { "ok" } else { "short" },
                        have,
                        if ok { "\u{2714}" } else { "(not enough)" }
                    ));
                }
                s
            }
            None => "No route between these".into(),
        };
        if let Some(el) = self.doc.get_element_by_id("trip-result") {
            el.set_inner_html(&text);
        }
    }

    /// Show the first-time hints of script `id` (FTUE_VAB, FTUE_Flight, ...) once per browser, if hints are on.
    fn ftue(&self, id: &str) {
        let mut st = self.ftue_state.borrow_mut();
        if !st.enabled || st.seen.contains(id) {
            return;
        }
        let Some(script) = st.scripts.as_ref().and_then(|s| s.get(id)).cloned() else {
            return;
        };
        st.seen.insert(id.to_string());
        let list: Vec<&str> = st.seen.iter().map(|s| s.as_str()).collect();
        store("ose.ftue.seen", &list.join(","));
        let was_empty = st.queue.is_empty();
        st.queue.extend(script.messages);
        drop(st);
        if was_empty {
            self.ftue_show();
        }
    }

    /// Show the next queued hint, or hide the window when none is left.
    fn ftue_show(&self) {
        let Some(el) = self.doc.get_element_by_id("ftue") else {
            return;
        };
        let st = self.ftue_state.borrow();
        let Some(m) = st.queue.front() else {
            el.class_list().remove_1("open").ok();
            return;
        };
        // A lesson stage waiting on the game: PAIGE's last line stays up until it is done.
        let waiting = st.lesson && self.lesson_run.borrow().as_ref().is_some_and(|r| r.waiting);
        let text = self
            .loc
            .get(&m.key)
            .map(|t| ftue::rich_text(t))
            .unwrap_or_else(|| esc(&m.key));
        let image = if m.image.is_empty() {
            "PAIGE_emote_01_neutral_0001"
        } else {
            m.image.as_str()
        };
        el.set_inner_html(&format!(
            "<img src=\"assets/ui/hud/{image}.png\" alt=\"\" onerror=\"this.style.display='none'\" /><div class=\"ftue-body\"><div class=\"ftue-name\">PAIGE</div><div class=\"ftue-text\">{text}</div><div class=\"ftue-acts\"><button id=\"ftue-off\" class=\"secondary\">{off}</button><button id=\"ftue-next\"{dis}>{next}</button></div></div>",
            next = if waiting {
                "Waiting for you\u{2026}"
            } else if st.queue.len() > 1 || st.lesson {
                "Continue"
            } else {
                "Got it"
            },
            off = if st.lesson { "End Tutorial" } else { "No more hints" },
            dis = if waiting { " disabled" } else { "" },
        ));
        el.class_list().add_1("open").ok();
    }

    /// Settings > Input: each rebindable action with the key it is on now.
    fn render_rebind(&self) {
        let Some(el) = self.doc.get_element_by_id("settings-rebind") else {
            return;
        };
        let mut html =
            String::from("<h4>Keys (click to change, Esc keeps the old one)</h4><table>");
        for (action, default) in input::REBINDABLE {
            let waiting = self.rebinding.as_deref() == Some(*default);
            let key = self.remap.key_for(default);
            html.push_str(&format!(
                "<tr><td>{action}</td><td><button data-rebind=\"{default}\" class=\"{}\">{}</button></td></tr>",
                if waiting { "waiting" } else if key != *default { "changed" } else { "" },
                if waiting { "press a key...".to_string() } else { input::key_name(key) },
            ));
        }
        html.push_str("</table><button id=\"rebind-reset\">Reset all keys</button>");
        el.set_inner_html(&html);
    }

    /// Open the flight part window for craft part `i` at a screen point, or close it with `None`.
    fn flight_open_pam(&mut self, i: Option<usize>, x: f64, y: f64) {
        self.flight_pam = i;
        let Some(el) = self
            .doc
            .get_element_by_id("fh-pam")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        else {
            return;
        };
        if i.is_none() {
            el.class_list().remove_1("open").ok();
            return;
        }
        let w = web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|w| w.as_f64())
            .unwrap_or(1600.0);
        el.style()
            .set_property("left", &format!("{}px", (x + 14.0).min(w - 300.0)))
            .ok();
        el.style()
            .set_property("top", &format!("{}px", y.max(70.0)))
            .ok();
        el.class_list().add_1("open").ok();
        self.flight_pam_refresh();
    }

    /// Redraw the flight part window from the latest readout (it follows the part's state).
    fn flight_pam_refresh(&self) {
        let Some(i) = self.flight_pam else { return };
        let Some(el) = self.doc.get_element_by_id("fh-pam") else {
            return;
        };
        let Some(f) = self
            .flying
            .and_then(|id| self.scene.as_ref()?.vessel(id))
            .and_then(|v| v.flight.as_ref())
        else {
            return;
        };
        let Some(part) = self.flight_craft.as_ref().and_then(|c| c.parts.get(i)) else {
            return;
        };
        let spec = self.catalog.get(&part.part_id);
        let title = spec
            .map(|s| s.display_title().to_string())
            .unwrap_or_else(|| part.part_id.clone());
        let html = hud::flight_pam_html(
            &title,
            i,
            f,
            spec.is_some_and(|s| s.has_engine),
            part.part_id.contains("solar"),
        );
        let key = format!("{}:{}", html.len(), hud::fnv(&html));
        if el.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
            el.set_inner_html(&html);
            el.set_attribute("data-ose-key", &key).ok();
        }
    }

    /// Open the part manager for part `i` at a screen point, or close it with `None`.
    fn vab_open_pam(&mut self, i: Option<usize>, x: f64, y: f64) {
        self.vab_pam = i;
        self.vab_pam_recorded = false;
        let Some(el) = self
            .doc
            .get_element_by_id("vab-pam")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        else {
            return;
        };
        match i {
            Some(i) => {
                el.set_inner_html(&vab::pam_html(&self.editor.craft, i, &self.catalog));
                let w = web_sys::window()
                    .and_then(|w| w.inner_width().ok())
                    .and_then(|w| w.as_f64())
                    .unwrap_or(1600.0);
                el.style()
                    .set_property("left", &format!("{}px", (x + 14.0).min(w - 320.0)))
                    .ok();
                el.style()
                    .set_property("top", &format!("{}px", y.max(70.0)))
                    .ok();
                el.class_list().add_1("open").ok();
            }
            None => {
                el.class_list().remove_1("open").ok();
            }
        }
    }

    /// A change in the part manager: a tank's fill (`res` index) or the crossfeed switch.
    fn vab_pam_changed(&mut self, res: Option<usize>, value: f64, crossfeed: Option<bool>) {
        let Some(i) = self.vab_pam else { return };
        if i >= self.editor.craft.parts.len() {
            return;
        }
        if !self.vab_pam_recorded {
            self.vab_pam_recorded = true;
            self.vab_record();
        }
        let p = &mut self.editor.craft.parts[i];
        if let Some(k) = res {
            if let Some(r) = p.resources.get_mut(k) {
                r.stored_units = value.clamp(0.0, r.capacity_units);
                let text = format!("{:.0} / {:.0}", r.stored_units, r.capacity_units);
                if let Ok(Some(b)) = self
                    .doc
                    .query_selector(&format!("#vab-pam input[data-res=\"{k}\"] + b"))
                {
                    b.set_text_content(Some(&text));
                }
            }
        }
        if let Some(cf) = crossfeed {
            p.fuel_crossfeed = cf;
        }
        // Ask for a fresh staging readout (delta-v follows the fill) without redrawing the window.
        self.vab_report_due = Some(now_ms() + 300.0);
    }

    /// Ctrl+Z / Ctrl+Y.
    fn vab_undo(&mut self, redo: bool) {
        let mut h = std::mem::take(&mut self.vab_history);
        let done = if redo {
            h.redo(&mut self.editor)
        } else {
            h.undo(&mut self.editor)
        };
        self.vab_history = h;
        if done {
            let cat = std::mem::take(&mut self.catalog);
            self.editor.rebuild_stages(&cat);
            self.catalog = cat;
            self.render_vab();
        }
    }

    fn vab_shift_stage(&mut self, delta: i32) {
        self.vab_history.record(&self.editor);
        let cat = std::mem::take(&mut self.catalog);
        self.editor.shift_stage(delta, &cat);
        self.catalog = cat;
        self.render_vab();
    }

    fn vab_save(&mut self, then_launch: bool) {
        if self.editor.is_empty() {
            set_text(
                &self.doc,
                "vab-report",
                "Nothing to save: add a part first.",
            );
            return;
        }
        if !self.editor.craft.is_tree() {
            set_text(&self.doc, "vab-report", "The craft is not a single tree.");
            return;
        }
        self.launch_after_save = then_launch;
        set_text(&self.doc, "vab-report", "saving...");
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::SaveCraft {
                    craft: self.editor.craft.clone(),
                },
            );
        }
    }

    /// Select the part under a canvas click (topmost box wins).
    fn vab_click(&mut self, x: f64, y: f64) {
        let Some(canvas) = self
            .doc
            .get_element_by_id("vab-canvas")
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
        else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let sx = canvas.width() as f64 / rect.width().max(1.0);
        let sy = canvas.height() as f64 / rect.height().max(1.0);
        let (px, py) = (x * sx, y * sy);
        let hit = self
            .vab_boxes
            .iter()
            .rev()
            .find(|(_, x0, y0, x1, y1)| px >= *x0 && px <= *x1 && py >= *y0 && py <= *y1)
            .map(|(i, ..)| *i);
        if let Some(i) = hit {
            self.editor.selected = Some(i);
            self.render_vab();
        }
    }

    /// Draw the craft side view, the selection, and refresh the side panel.
    fn render_vab(&mut self) {
        self.vab_report_due = Some(now_ms() + 300.0);
        self.render_twr_bodies();
        self.render_vab_view();
    }

    /// The header's Load list (OABLoadDialog): your crafts first, then the stock ones, newest first.
    fn render_vab_load(&self) {
        let Some(list) = self.doc.get_element_by_id("vab-load-list") else {
            return;
        };
        let mut rows: Vec<&ose_net::CraftSummary> = self.crafts.iter().collect();
        rows.sort_by_key(|c| (c.stock, std::cmp::Reverse(c.id)));
        let mut html = String::new();
        for c in rows {
            html.push_str(&format!(
                "<li data-ose=\"vab-load-craft\" data-ose-id=\"{}\"><b>{}</b>{}<small>{} parts \u{b7} {} stages \u{b7} {:.1} t \u{b7} \u{394}v {:.0} m/s</small></li>",
                c.id,
                esc(&c.name),
                if c.stock { " <i>stock</i>" } else { "" },
                c.parts,
                c.stages,
                c.wet_mass_t,
                c.delta_v_vacuum_mps
            ));
        }
        if html.is_empty() {
            html = "<li class=\"empty\">No saved vehicles yet.</li>".into();
        }
        list.set_inner_html(&html);
    }

    /// Ask the server for the staging readout of the craft being built, once edits settle.
    fn vab_report_tick(&mut self, now: f64) {
        let Some(due) = self.vab_report_due else {
            return;
        };
        if now < due {
            return;
        }
        self.vab_report_due = None;
        if self.editor.is_empty() || !self.editor.craft.is_tree() {
            self.vab_stages_readout.clear();
            set_text(&self.doc, "vab-dv", "");
            return;
        }
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::StageReport {
                    craft: self.editor.craft.clone(),
                },
            );
        }
    }

    /// Draw the craft and the side panel without asking for a new staging readout.
    fn render_vab_view(&mut self) {
        set_text(&self.doc, "vab-selected", "");
        let (parts_n, dry) = (
            self.editor.craft.parts.len(),
            self.editor.dry_mass_t(&self.catalog),
        );
        if let Some(el) = self.doc.get_element_by_id("vab-stats") {
            el.set_inner_html(&format!(
                "<b>{parts_n}</b> parts \u{b7} <b>{:.2} t</b> dry (catalog) \u{b7} <b>{}</b> stages",
                dry.max(0.0),
                self.editor.craft.stages.len()
            ));
        }
        if let Some(sel) = self
            .editor
            .selected
            .and_then(|i| self.editor.craft.parts.get(i))
        {
            let title = self
                .catalog
                .get(&sel.part_id)
                .map(|s| s.display_title().to_string())
                .unwrap_or_else(|| sel.part_id.clone());
            let stage = if sel.stage_index > 0 {
                format!(" \u{b7} stage {}", sel.stage_index)
            } else {
                String::new()
            };
            set_text(
                &self.doc,
                "vab-selected",
                &format!("selected: {title}{stage}"),
            );
        }
        if let Some(list) = self.doc.get_element_by_id("vab-stages") {
            // One card per stage, the last to fire on top and the first (Space) at the bottom, as the
            // original's staging stack. The highest stage index fires first (ose_data::staging), so the cards
            // run in index order and count up from the bottom.
            let mut html = String::new();
            let count = self.editor.craft.stages.len();
            for (n, stage) in self.editor.craft.stages.iter().enumerate() {
                let parts: Vec<String> = stage
                    .iter()
                    .filter_map(|&i| self.editor.craft.parts.get(i).map(|p| (i, p)))
                    .map(|(i, p)| {
                        let spec = self.catalog.get(&p.part_id);
                        let icon = match spec {
                            Some(s) if s.has_engine => "Staging-Icon-Thruster",
                            Some(s) if s.is_decoupler => "Staging-Icon-Inline",
                            Some(s) if s.stage_type.contains("Parachute") => "Staging-ICO-Chute-Stowed",
                            _ => "Staging-ICO-Structure01",
                        };
                        format!(
                            "<span class=\"p{sel}\" data-ose-index=\"{i}\" title=\"select\"><i style=\"--ico:url(assets/ui/hud/{icon}.png)\"></i>{name}</span>",
                            sel = if self.editor.selected == Some(i) { " sel" } else { "" },
                            name = spec
                                .map(|s| esc(s.display_title()))
                                .unwrap_or_else(|| esc(&p.part_id)),
                        )
                    })
                    .collect();
                // The server's readout for this stage: delta-v, TWR at its start (Kerbin's sea level, or in
                // vacuum on the surface the delta-v tool names), burn time.
                // The delta-v tool's situation: a body's surface (its air pressure) or Kerbin's vacuum.
                let (body_id, vacuum) = match self.vab_twr_body.as_deref() {
                    None => ("Kerbin", false),
                    Some("Kerbin@vacuum") => ("Kerbin", true),
                    Some(b) => (b, false),
                };
                let surface_g = self.scene.as_ref().and_then(|s| {
                    let body = s.bodies.iter().find(|x| x.id == body_id)?;
                    Some(body.mu / (body.radius_m * body.radius_m))
                });
                let p_atm = if vacuum {
                    0.0
                } else {
                    vab::surface_pressure_atm(body_id)
                };
                let stats = self
                    .vab_stages_readout
                    .iter()
                    .find(|r| r.stage_id as usize == n)
                    .map(|r| {
                        let (dv, twr) = match surface_g {
                            Some(g) => vab::stage_in_situation(
                                r.delta_v_vacuum_mps,
                                r.thrust_kn,
                                r.thrust_asl_kn,
                                r.start_mass_t,
                                g,
                                p_atm,
                            ),
                            None => (r.delta_v_vacuum_mps, r.twr_asl),
                        };
                        format!(
                            "<span class=\"sv\" title=\"delta-v, thrust-to-weight and burn time in the delta-v tool's situation\">\u{394}v {:.0} m/s \u{b7} TWR {:.2} \u{b7} {:.0} s</span>",
                            dv.max(0.0), twr.max(0.0), r.burn_time_s.max(0.0)
                        )
                    })
                    .unwrap_or_default();
                html.push_str(&format!(
                    "<li data-ose-stage=\"{n}\"><span class=\"st\">STAGE {}</span>{stats}{}</li>",
                    count - n,
                    parts.join("")
                ));
            }
            if html.is_empty() {
                html = "<li class=\"empty\">No stages yet: engines, decouplers and parachutes get one each.</li>".into();
            }
            list.set_inner_html(&html);
        }
        // Engineer's report: the flaw count on the header button and the list in its pop-over.
        let readout = |f: &dyn Fn(&ose_net::StageReadout) -> f64| -> Vec<(usize, f64)> {
            self.vab_stages_readout
                .iter()
                .map(|r| (r.stage_id as usize, f(r)))
                .collect()
        };
        let flaws = vab::engineer_report(
            &self.editor.craft,
            &self.catalog,
            &readout(&|r| r.twr_asl),
            &readout(&|r| r.delta_v_vacuum_mps),
        );
        let worst = flaws.iter().map(|f| f.severity).max().unwrap_or(0);
        let count = flaws.iter().filter(|f| f.severity > 0).count();
        if let Some(b) = self.doc.get_element_by_id("vab-report-btn") {
            b.set_attribute("data-ose-flaws", &count.to_string()).ok();
            b.class_list().toggle_with_force("warn", worst == 1).ok();
            b.class_list().toggle_with_force("bad", worst == 2).ok();
        }
        set_text(
            &self.doc,
            "vab-report-count",
            &if count > 0 {
                count.to_string()
            } else {
                String::new()
            },
        );
        if let Some(list) = self.doc.get_element_by_id("vab-flaws") {
            let html: String = flaws
                .iter()
                .map(|f| format!("<li class=\"sev{}\">{}</li>", f.severity, esc(&f.text)))
                .collect();
            list.set_inner_html(&html);
        }
        set_text(
            &self.doc,
            "vab-orient",
            if self.editor.plane_new {
                "Plane"
            } else {
                "Rocket"
            },
        );
        // A plane's pair is a mirror image (vab.rs), which the button says.
        let sym = if self.vab_symmetry == 2 && self.editor.is_plane() {
            "Mirror".to_string()
        } else {
            format!("\u{d7}{}", self.vab_symmetry)
        };
        set_text(&self.doc, "vab-sym", &sym);
        for (id, on) in [
            ("vab-com", self.vab_show_com),
            ("vab-cot", self.vab_show_cot),
            ("vab-col", self.vab_show_col),
        ] {
            if let Some(b) = self.doc.get_element_by_id(id) {
                b.class_list().toggle_with_force("on", on).ok();
            }
        }
        set_text(
            &self.doc,
            "vab-frame",
            if self.vab_local_frame {
                "Local"
            } else {
                "Absolute"
            },
        );
        if let Some(b) = self.doc.get_element_by_id("vab-offset") {
            b.class_list().toggle_with_force("on", self.vab_offset).ok();
        }
        if let Some(b) = self.doc.get_element_by_id("vab-snap") {
            b.class_list().toggle_with_force("on", self.vab_snap).ok();
        }
        for (id, on) in [
            ("vab-undo", self.vab_history.can_undo()),
            ("vab-redo", self.vab_history.can_redo()),
        ] {
            if let Some(b) = self.doc.get_element_by_id(id) {
                if on {
                    b.remove_attribute("disabled").ok();
                } else {
                    b.set_attribute("disabled", "").ok();
                }
            }
        }
        if let Some(name) = self
            .doc
            .get_element_by_id("vab-name")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            if name.value() != self.editor.craft.name {
                name.set_value(&self.editor.craft.name);
            }
        }
        let Some(canvas) = self
            .doc
            .get_element_by_id("vab-canvas")
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
        else {
            return;
        };
        let Ok(Some(ctx)) = canvas.get_context("2d") else {
            return;
        };
        let Ok(ctx) = ctx.dyn_into::<CanvasRenderingContext2d>() else {
            return;
        };
        let (w, h) = (canvas.width() as f64, canvas.height() as f64);
        ctx.clear_rect(0.0, 0.0, w, h);
        self.vab_boxes.clear();
        let sizes = &self.part_sizes;
        let size_of = |pid: &str| sizes.get(pid).copied();
        let Some((boxes, bounds)) = thumb::layout(&self.editor.craft, &size_of) else {
            ctx.set_fill_style_str("#6f7f9c");
            ctx.set_font("14px system-ui");
            ctx.fill_text("Pick a part on the left to start the craft.", 20.0, 32.0)
                .ok();
            return;
        };
        // Keep a sane scale: never larger than 40 px per metre, so small crafts do not fill the canvas.
        let to_px = {
            let fitted = thumb::fit(&bounds, w, h, 24.0);
            let scale_fit =
                ((w - 48.0) / bounds.width().max(1e-6)).min((h - 48.0) / bounds.height().max(1e-6));
            if scale_fit > 40.0 {
                let cx = (bounds.min.x + bounds.max.x) * 0.5;
                let cy = (bounds.min.y + bounds.max.y) * 0.5;
                Box::new(move |x: f64, y: f64| {
                    (w * 0.5 + (x - cx) * 40.0, h * 0.5 - (y - cy) * 40.0)
                }) as Box<dyn Fn(f64, f64) -> (f64, f64)>
            } else {
                Box::new(fitted) as Box<dyn Fn(f64, f64) -> (f64, f64)>
            }
        };
        let stages = self.editor.craft.stages.len().max(1) as f64;
        let mut px_boxes = Vec::with_capacity(boxes.len());
        for b in &boxes {
            let (x0, y0) = to_px(b.min.x, b.max.y);
            let (x1, y1) = to_px(b.max.x, b.min.y);
            let t = if b.stage <= 0 {
                1.0
            } else {
                1.0 - 0.5 * (b.stage as f64 / stages).min(1.0)
            };
            let c = (120.0 + 100.0 * t) as u32;
            let selected = self.editor.selected == Some(b.part);
            ctx.set_fill_style_str(&if selected {
                "#9ef0c0".to_string()
            } else {
                format!(
                    "rgb({c},{},{})",
                    (c as f64 * 0.95) as u32,
                    (c as f64 * 1.05).min(255.0) as u32
                )
            });
            ctx.set_stroke_style_str(if selected { "#ffffff" } else { "#2c3d63" });
            ctx.set_line_width(if selected { 2.0 } else { 1.0 });
            let (bw, bh) = ((x1 - x0).max(2.0), (y1 - y0).max(2.0));
            ctx.fill_rect(x0, y0, bw, bh);
            ctx.stroke_rect(x0, y0, bw, bh);
            px_boxes.push((b.part, x0, y0, x0 + bw, y0 + bh));
        }
        self.vab_boxes = px_boxes;
        canvas
            .set_attribute("data-ose-vab-parts", &boxes.len().to_string())
            .ok();
    }

    fn open_launchpad(&mut self) {
        self.menu_screen("launchpad");
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::CraftList);
            send(ws, &ClientMessage::KerbalRoster);
        }
    }

    // ---- Training Center

    fn open_training(&mut self) {
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        set_text(&self.doc, "menu-note", "");
        self.menu_screen("training");
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::KerbalRoster);
        }
        self.render_training();
    }

    fn vessel_name(&self, id: u64) -> Option<String> {
        self.scene
            .as_ref()
            .and_then(|s| s.vessel(id))
            .map(|v| v.name.clone())
    }

    fn render_training(&mut self) {
        for (id, on) in [
            ("tc-tab-kerbals", !self.tc_lessons),
            ("tc-tab-lessons", self.tc_lessons),
        ] {
            if let Some(b) = self.doc.get_element_by_id(id) {
                b.class_list().toggle_with_force("on", on).ok();
            }
        }
        for (id, show) in [("tc-list", !self.tc_lessons), ("tl-list", self.tc_lessons)] {
            if let Some(el) = self.doc.get_element_by_id(id) {
                el.class_list().toggle_with_force("hidden", !show).ok();
            }
        }
        if self.tc_lessons {
            self.render_lessons();
            return;
        }
        if !self
            .roster
            .iter()
            .any(|k| Some(&k.name) == self.tc_selected.as_ref())
        {
            self.tc_selected = training::sorted(&self.roster)
                .first()
                .map(|k| k.name.clone());
        }
        let name = |id: u64| self.vessel_name(id);
        let sel = self.tc_selected.as_deref();
        if let Some(el) = self.doc.get_element_by_id("tc-list") {
            el.set_inner_html(&training::render_list(&self.roster, sel, &name));
        }
        if let Some(el) = self.doc.get_element_by_id("tc-info") {
            let k = self.roster.iter().find(|k| Some(k.name.as_str()) == sel);
            el.set_inner_html(&training::render_info(k, &name));
        }
        let free = self
            .roster
            .iter()
            .filter(|k| k.status == ose_net::KerbalStatus::Available)
            .count();
        set_text(
            &self.doc,
            "tc-count",
            &format!("{} astronauts, {free} available", self.roster.len()),
        );
    }

    /// The Lessons tab: the lessons by group, and the one picked (the first until one is).
    fn render_lessons(&mut self) {
        let Some(a) = &self.academy else {
            set_text(&self.doc, "tc-count", "lessons are loading");
            return;
        };
        if !a
            .lessons
            .iter()
            .any(|l| Some(&l.id) == self.lesson_selected.as_ref())
        {
            self.lesson_selected = a.lessons.first().map(|l| l.id.clone());
        }
        let sel = self.lesson_selected.as_deref();
        if let Some(el) = self.doc.get_element_by_id("tl-list") {
            el.set_inner_html(&tutorials::list_html(a, &self.loc, sel));
        }
        if let Some(el) = self.doc.get_element_by_id("tc-info") {
            let html = a
                .lessons
                .iter()
                .find(|l| Some(l.id.as_str()) == sel)
                .map(|l| tutorials::info_html(l, &self.loc))
                .unwrap_or_default();
            el.set_inner_html(&html);
        }
        set_text(
            &self.doc,
            "tc-count",
            &format!("Kerbal Space Academy: {} lessons", a.lessons.len()),
        );
    }

    /// Play a lesson: PAIGE's messages, stage after stage, in the hint box (whatever the hint setting).
    fn play_lesson(&self, id: &str) {
        let Some(l) = self
            .academy
            .as_ref()
            .and_then(|a| a.lessons.iter().find(|l| l.id == id))
        else {
            return;
        };
        let id = l.id.clone();
        *self.lesson_run.borrow_mut() = Some(LessonRun {
            id,
            ..Default::default()
        });
        self.ftue_state.borrow_mut().lesson = true;
        self.lesson_enter_stage();
    }

    /// The lesson stage now running, if a lesson is.
    fn lesson_stage(&self) -> Option<tutorials::Stage> {
        let run = self.lesson_run.borrow();
        let run = run.as_ref()?;
        self.academy
            .as_ref()?
            .lessons
            .iter()
            .find(|l| l.id == run.id)?
            .stages
            .get(run.stage)
            .cloned()
    }

    /// Show the current stage's messages; past the last stage the lesson is over.
    fn lesson_enter_stage(&self) {
        let Some(stage) = self.lesson_stage() else {
            *self.lesson_run.borrow_mut() = None;
            {
                let mut st = self.ftue_state.borrow_mut();
                st.queue.clear();
                st.lesson = false;
            }
            self.ftue_show();
            hud::push(&self.doc, "Lesson complete", "triumph");
            return;
        };
        if let Some(run) = self.lesson_run.borrow_mut().as_mut() {
            run.waiting = stage.messages.is_empty();
            run.events.clear();
        }
        // A stage with nothing to say keeps PAIGE's last line up while it waits.
        if !stage.messages.is_empty() {
            let mut st = self.ftue_state.borrow_mut();
            st.queue.clear();
            st.queue.extend(stage.messages.iter().cloned());
        }
        self.ftue_show();
    }

    /// Continue on the stage's last message: on to the next stage if its condition allows, else wait for the
    /// game (the box says so).
    fn lesson_continue(&self) {
        let Some(stage) = self.lesson_stage() else {
            return;
        };
        let world = self.lesson_world();
        if stage.done(&world, true) {
            self.lesson_next();
        } else {
            if let Some(run) = self.lesson_run.borrow_mut().as_mut() {
                run.waiting = true;
            }
            self.ftue_show();
        }
    }

    fn lesson_next(&self) {
        if let Some(run) = self.lesson_run.borrow_mut().as_mut() {
            run.stage += 1;
        }
        self.lesson_enter_stage();
    }

    /// A lesson event (the original's message name) seen in the game.
    fn lesson_event(&self, name: &str) {
        if let Some(run) = self.lesson_run.borrow_mut().as_mut() {
            run.events.insert(name.to_string());
        }
    }

    /// The VAB part picked or placed, for lessons that ask for one.
    fn lesson_part(&self, id: &str, placed: bool) {
        if let Some(run) = self.lesson_run.borrow_mut().as_mut() {
            run.last_selected = Some(id.to_string());
            if placed {
                run.last_placed = Some(id.to_string());
                run.events.insert("PartPlacedOnMainAssemblyMessage".into());
            }
        }
    }

    /// What a lesson can watch now: the flown vessel's readout and orbit, the map, the VAB and the events.
    fn lesson_world(&self) -> tutorials::World {
        let mut w = tutorials::World::default();
        if let Some(run) = self.lesson_run.borrow().as_ref() {
            w.events = run.events.clone();
            w.last_placed = run.last_placed.clone();
            w.last_selected = run.last_selected.clone();
        }
        w.map = self.map_mode;
        // Not flying: nothing is up or moving yet (so "fly above 200 m" waits for a flight).
        w.altitude_sea_m = Some(0.0);
        w.altitude_ground_m = Some(0.0);
        w.surface_speed_mps = Some(0.0);
        w.throttle = Some(0.0);
        let Some(scene) = &self.scene else {
            return w;
        };
        let Some(v) = self.flying.and_then(|id| scene.vessel(id)) else {
            return w;
        };
        if let Some(f) = &v.flight {
            w.altitude_sea_m = Some(f.altitude_m);
            w.altitude_ground_m = Some(f.altitude_m);
            w.surface_speed_mps = Some(f.surface_speed_mps);
            w.throttle = Some(f.throttle);
            w.stage_fuel_pct = Some(f.propellant_fraction * 100.0);
            w.sas_mode = Some(match f.attitude {
                ose_net::AttitudeMode::OrbitPrograde | ose_net::AttitudeMode::SurfacePrograde => 1,
                ose_net::AttitudeMode::OrbitRetrograde
                | ose_net::AttitudeMode::SurfaceRetrograde => 2,
                ose_net::AttitudeMode::Normal => 3,
                ose_net::AttitudeMode::AntiNormal => 4,
                ose_net::AttitudeMode::RadialIn => 5,
                ose_net::AttitudeMode::RadialOut => 6,
                ose_net::AttitudeMode::Target => 7,
                ose_net::AttitudeMode::AntiTarget => 8,
                ose_net::AttitudeMode::Maneuver => 9,
                _ => 0,
            });
        }
        if let Some(b) = scene.bodies.get(v.reference_body as usize) {
            let ut = self.time.ut_at(now_ms());
            let (ap, pe) = hud::apsides(&v.orbit, b.radius_m, b.mu, ut);
            w.apoapsis_m = ap.map(|a| a.0);
            w.periapsis_m = Some(pe.0);
        }
        w
    }

    /// Every frame while a lesson waits on the game: move on once the stage's condition holds. Lift-off and
    /// parachutes opening are seen here from the readout.
    fn lesson_tick(&self) {
        if self.lesson_run.borrow().is_none() {
            return;
        }
        if let Some(f) = self
            .flying
            .and_then(|id| self.scene.as_ref()?.vessel(id))
            .and_then(|v| v.flight.as_ref())
        {
            if f.situation != ose_net::FlightSituation::Landed {
                self.lesson_event("VesselLaunchedMessage");
            }
            for (_, st, _) in &f.parachutes {
                match st {
                    ose_net::ChuteState::Deployed => self.lesson_event("ParachuteDeployedMessage"),
                    ose_net::ChuteState::SemiDeployed => {
                        self.lesson_event("ParachuteSemiDeployedMessage")
                    }
                    _ => {}
                }
            }
        }
        let waiting = self.lesson_run.borrow().as_ref().is_some_and(|r| r.waiting);
        if !waiting {
            return;
        }
        if let Some(stage) = self.lesson_stage() {
            if stage.done(&self.lesson_world(), false) {
                self.lesson_next();
            }
        }
    }

    /// Seats on a saved craft: the crew capacity of its parts (0 until its document arrives).
    fn craft_seats(&self, id: u64) -> usize {
        self.craft_docs
            .get(&id)
            .map(|d| {
                d.parts
                    .iter()
                    .filter_map(|p| self.catalog.get(&p.part_id))
                    .map(|cp| cp.crew_capacity as usize)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Who flies craft `id`: the launch dialog's picks for it, else the roster's first available Kerbals.
    fn launch_crew(&self, id: u64) -> Vec<String> {
        let picks: &[String] = if self.lp_crew_for == Some(id) {
            &self.lp_crew
        } else {
            &[]
        };
        training::fill_crew(picks, self.craft_seats(id), &self.roster)
    }

    fn render_crafts(&self) {
        let Some(list) = self.doc.get_element_by_id("crafts") else {
            return;
        };
        let show_stock = checkbox(&self.doc, "launch-stock").unwrap_or(true);
        let mut crafts: Vec<&ose_net::CraftSummary> = self
            .crafts
            .iter()
            .filter(|c| show_stock || !c.stock)
            .collect();
        crafts.sort_by(|a, b| (a.stock, &a.name).cmp(&(b.stock, &b.name)));
        let mut html = String::new();
        for c in crafts {
            let badge = if c.stock {
                "<span class=\"badge\">stock</span>"
            } else if c.owner == self.player_id {
                "<span class=\"badge\">yours</span>"
            } else {
                ""
            };
            let sel = if self.lp_selected == Some(c.id) {
                " class=\"selected\""
            } else {
                ""
            };
            html.push_str(&format!(
                "<li{sel} data-ose=\"craft\" data-ose-id=\"{id}\" data-ose-dv=\"{dv:.0}\" data-ose-stock=\"{stock}\"><canvas class=\"thumb\" width=\"128\" height=\"168\" data-ose-thumb=\"{id}\"></canvas><div><div class=\"name\">{name}{badge}</div>\
                 <div class=\"meta\">{parts} parts \u{b7} {stages} {st} \u{b7} {wet:.1} t \u{b7} {dv:.0} m/s vacuum</div>\
                 {desc}</div><span><button class=\"secondary\" data-ose=\"craft-edit\" data-ose-craft-edit=\"{id}\">Edit</button> \
                 <button class=\"secondary\" data-ose=\"craft-orbit\" data-ose-craft-orbit=\"{id}\" title=\"skip the ascent: put it straight into orbit\">Orbit</button> \
                 <button data-ose=\"craft-launch\" data-ose-craft-launch=\"{id}\">Launch</button></span></li>",
                id = c.id,
                name = esc(&c.name),
                parts = c.parts,
                stages = c.stages,
                st = if c.stages == 1 { "stage" } else { "stages" },
                wet = c.wet_mass_t,
                dv = c.delta_v_vacuum_mps.max(0.0) + 0.0, // never "-0"
                stock = if c.stock { 1 } else { 0 },
                desc = if c.description.is_empty() {
                    String::new()
                } else {
                    format!("<div class=\"desc\">{}</div>", esc(&c.description))
                },
            ));
        }
        if html.is_empty() {
            html = "<li class=\"empty\">no crafts in the library</li>".into();
        }
        list.set_inner_html(&html);
        self.render_launchpad_info();
        self.draw_thumbnails();
        for c in &self.crafts {
            self.mark_craft_research(c.id);
        }
    }

    /// The selected vehicle on the launch pad: blueprint, name, description and engineering report, with Edit,
    /// Orbit and Launch.
    fn render_launchpad_info(&self) {
        let Some(el) = self.doc.get_element_by_id("lp-info") else {
            return;
        };
        let Some(c) = self
            .lp_selected
            .and_then(|id| self.crafts.iter().find(|c| c.id == id))
        else {
            el.set_inner_html("<div class=\"lp-empty\">Select a vehicle</div>");
            return;
        };
        let locked = self.locked_parts();
        let missing = self
            .craft_docs
            .get(&c.id)
            .map(|d| {
                d.parts
                    .iter()
                    .filter(|p| locked.contains(p.part_id.as_str()))
                    .count()
            })
            .unwrap_or(0);
        let control = self
            .craft_docs
            .get(&c.id)
            .map(|d| {
                d.parts.iter().any(|p| {
                    self.catalog
                        .get(&p.part_id)
                        .map(|cp| cp.crew_capacity > 0 || cp.category == "Pods")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(true);
        let mut warn = String::new();
        if !control {
            warn.push_str(
                "<div class=\"warn\">Warning: no control. This vessel has no command module.</div>",
            );
        }
        if missing > 0 {
            warn.push_str(&format!(
                "<div class=\"warn\">Not researched yet: {missing} part{}.</div>",
                if missing == 1 { "" } else { "s" }
            ));
        }
        el.set_inner_html(&format!(
            "<div class=\"lp-blueprint\"><canvas width=\"388\" height=\"220\" data-ose-thumb=\"{id}\"></canvas></div>\
             <h3>{name}</h3><div class=\"lp-at\">At {site}</div>{desc}\
             <div class=\"lp-report\"><b>Engineering Report</b><dl>\
             <dt>Parts</dt><dd>{parts}</dd><dt>Stages</dt><dd>{stages}</dd><dt>Mass</dt><dd>{wet:.1} t</dd>\
             <dt>Delta-v (vacuum)</dt><dd>{dv:.0} m/s</dd></dl>{warn}</div>{crew}\
             <div class=\"lp-actions\"><button data-ose=\"lp-edit\" data-ose-craft-edit=\"{id}\">Edit Vehicle</button>\
             <button data-ose=\"lp-orbit\" data-ose-craft-orbit=\"{id}\" title=\"skip the ascent: put it straight into orbit\">Orbit</button>\
             <button class=\"go\" data-ose=\"lp-launch\" data-ose-craft-launch=\"{id}\">Launch</button></div>",
            id = c.id,
            site = esc(&self.launch_site_name()),
                name = esc(&c.name),
            // Some stock crafts carry the original's untranslated placeholder.
            desc = if c.description.is_empty()
                || c.description == "This should be a localized string"
            {
                String::new()
            } else {
                format!("<p>{}</p>", esc(&c.description))
            },
            crew = training::render_crew(&self.launch_crew(c.id), &self.roster),
            parts = c.parts,
            stages = c.stages,
            wet = c.wet_mass_t,
            dv = c.delta_v_vacuum_mps.max(0.0),
        ));
    }

    // ---- Tracking Station

    fn open_tracking(&mut self) {
        self.ftue("FTUE_Tracking_Station");
        self.tracking_open = true;
        self.show_menu(false);
        self.render_tracking();
    }

    fn track_focus(&mut self, id: u64) {
        let rb = self
            .scene
            .as_ref()
            .and_then(|s| s.vessel(id))
            .map(|v| v.reference_body as usize);
        if let Some(rb) = rb {
            self.follow_vessel(id, rb);
        }
    }

    fn tracking_rows(&self) -> Vec<tracking::Row> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let ut = self.time.ut_at(now_ms());
        scene
            .vessels
            .iter()
            .filter_map(|v| {
                let body = scene.bodies.get(v.reference_body as usize)?;
                let orbit = scene::orbit_from(body.mu, &v.orbit);
                let (pe, ap) = (orbit.periapsis(), orbit.apoapsis());
                let atmo = if body.has_atmosphere {
                    body.atmosphere_depth_m
                } else {
                    0.0
                };
                let altitude = v.flight.as_ref().map(|f| f.altitude_m).unwrap_or_else(|| {
                    let rb = v.reference_body as usize;
                    (scene.vessel_position(v, ut) - scene.position(rb, ut)).length() - body.radius_m
                });
                Some(tracking::Row {
                    id: v.id,
                    name: v.name.clone(),
                    mine: v.owner.is_some() && v.owner == self.player_id,
                    body: body.id.clone(),
                    situation: tracking::situation(v, body.radius_m, atmo, pe),
                    altitude_m: altitude,
                    apoapsis_m: ap - body.radius_m,
                    periapsis_m: pe - body.radius_m,
                })
            })
            .collect()
    }

    fn render_tracking(&mut self) {
        self.ts_rendered_ms = now_ms();
        let rows = self.tracking_rows();
        if let Some(el) = self.doc.get_element_by_id("ts-list") {
            el.set_inner_html(&tracking::render_list(
                &rows,
                self.ts_selected,
                self.ts_mine_only,
            ));
        }
        for (id, on) in [
            ("ts-tab-all", !self.ts_mine_only),
            ("ts-tab-mine", self.ts_mine_only),
        ] {
            if let Some(el) = self.doc.get_element_by_id(id) {
                el.class_list().toggle_with_force("on", on).ok();
            }
        }
        if let Some(el) = self.doc.get_element_by_id("ts-info") {
            match self
                .ts_selected
                .and_then(|id| rows.iter().find(|r| r.id == id))
            {
                Some(r) => el.set_inner_html(&tracking::render_info(r)),
                None => {
                    el.set_inner_html("<div class=\"ts-hint\">Select a vessel to track it</div>")
                }
            }
        }
    }

    /// Show or hide the station and refresh it twice a second while it is open.
    fn update_tracking(&mut self, now: f64) {
        let visible = self.tracking_open && self.flying.is_none() && self.in_session;
        if let Some(el) = self.doc.get_element_by_id("tracking") {
            el.class_list().toggle_with_force("open", visible).ok();
        }
        if visible && now - self.ts_rendered_ms > 500.0 {
            self.render_tracking();
        }
    }

    fn draw_thumbnails(&self) {
        let ids: Vec<u64> = self.craft_docs.keys().copied().collect();
        for id in ids {
            self.draw_thumbnail(id);
        }
    }

    /// Paint the side-view silhouette of a library craft into its list canvas, if that canvas is on screen.
    fn draw_thumbnail(&self, id: u64) {
        let Some(craft) = self.craft_docs.get(&id) else {
            return;
        };
        let Ok(all) = self
            .doc
            .query_selector_all(&format!("canvas[data-ose-thumb=\"{id}\"]"))
        else {
            return;
        };
        for i in 0..all.length() {
            if let Some(canvas) = all
                .item(i)
                .and_then(|n| n.dyn_into::<HtmlCanvasElement>().ok())
            {
                self.paint_thumbnail(craft, &canvas);
            }
        }
    }

    fn paint_thumbnail(&self, craft: &ose_data::craft::Craft, canvas: &HtmlCanvasElement) {
        let Ok(Some(ctx)) = canvas.get_context("2d") else {
            return;
        };
        let Ok(ctx) = ctx.dyn_into::<CanvasRenderingContext2d>() else {
            return;
        };
        let sizes = &self.part_sizes;
        let size_of = |pid: &str| sizes.get(pid).copied();
        let Some((boxes, bounds)) = thumb::layout(craft, &size_of) else {
            return;
        };
        let (w, h) = (canvas.width() as f64, canvas.height() as f64);
        ctx.clear_rect(0.0, 0.0, w, h);
        let to_px = thumb::fit(&bounds, w, h, 6.0);
        let stages = craft.stages.len().max(1) as f64;
        for b in &boxes {
            let (x0, y0) = to_px(b.min.x, b.max.y);
            let (x1, y1) = to_px(b.max.x, b.min.y);
            // Later stages (fired first) darker, so the stack reads top to bottom.
            let t = if b.stage < 0 {
                1.0
            } else {
                1.0 - 0.5 * (b.stage as f64 / stages).min(1.0)
            };
            let c = (120.0 + 100.0 * t) as u32;
            ctx.set_fill_style_str(&format!(
                "rgb({c},{},{})",
                (c as f64 * 0.95) as u32,
                (c as f64 * 1.05).min(255.0) as u32
            ));
            ctx.set_stroke_style_str("#2c3d63");
            ctx.fill_rect(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0));
            ctx.stroke_rect(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0));
        }
        canvas
            .set_attribute("data-ose-thumb-parts", &boxes.len().to_string())
            .ok();
    }

    /// The chosen launch site's name (Launchpad 1 when none is chosen or the server has not listed them).
    fn launch_site_name(&self) -> String {
        let id = self.launch_site.as_deref().unwrap_or("LaunchPad_1");
        self.launch_sites
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Launchpad 1".to_string())
    }

    /// Fill the launch-site choosers (VAB header and launch pad dialog) from the server's list.
    fn render_launch_sites(&self) {
        let chosen = self.launch_site.clone().unwrap_or_default();
        let mut html = String::new();
        for s in &self.launch_sites {
            html.push_str(&format!(
                "<option value=\"{id}\"{sel}>{name}</option>",
                id = esc(&s.id),
                sel = if s.id == chosen || (chosen.is_empty() && s.id == "LaunchPad_1") {
                    " selected"
                } else {
                    ""
                },
                name = esc(&s.name),
            ));
        }
        // The launch dialog is titled after the site, as the original's is.
        set_text(&self.doc, "lp-title", &self.launch_site_name());
        self.render_launchpad_info();
        for id in ["vab-site", "lp-site"] {
            if let Some(el) = self.doc.get_element_by_id(id) {
                el.set_inner_html(&html);
                if let Ok(e) = el.clone().dyn_into::<HtmlElement>() {
                    e.style()
                        .set_property("display", if html.is_empty() { "none" } else { "" })
                        .ok();
                }
            }
        }
    }

    /// Roll a library craft out to the chosen launch site (the original's launch: landed, engines off).
    fn launch_craft(&mut self, id: u64) {
        set_text(&self.doc, "menu-note", "rolling out to the launch pad...");
        self.pending_launch = Some(id);
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::LaunchSavedAt {
                    id,
                    site: self.launch_site.clone(),
                    // The server fills the seats itself unless the player chose the crew for this craft (a
                    // client-side fill could name a Kerbal the server has just sent flying).
                    crew: (self.lp_crew_for == Some(id) && !self.roster.is_empty())
                        .then(|| self.launch_crew(id)),
                },
            );
        }
        // The picks were for this launch; the next one starts from the roster again.
        self.lp_crew_for = None;
    }

    /// Put a library craft straight into orbit (the earlier shortcut, kept for testing orbits).
    fn launch_craft_to_orbit(&mut self, id: u64) {
        let altitude_m = input_value(&self.doc, "launch-altitude").unwrap_or(100.0) * 1000.0;
        set_text(&self.doc, "menu-note", "launching to orbit...");
        self.pending_launch = None;
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::LaunchSavedCraft { id, altitude_m });
        }
    }

    // ---- Flight

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn flight_active(&self) -> bool {
        self.flying.is_some()
            && !self.map_mode
            && self
                .doc
                .get_element_by_id("menu")
                .map(|m| m.class_list().contains("hidden"))
                .unwrap_or(false)
    }

    fn start_flight(&mut self, vessel: u64, craft_id: Option<u64>) {
        self.ftue("FTUE_Flight");
        self.lesson_event("VesselCreatedMessage");
        self.lesson_event("LaunchSequenceInitiatedMessage");
        self.flying = Some(vessel);
        // The server keeps a target per vessel; the one picked carries over to the vessel flown now.
        if self.target == Some(ose_net::TargetId::Vessel(vessel)) {
            self.target = None;
        }
        self.send_target();
        self.flight_craft = craft_id.and_then(|c| self.craft_docs.get(&c).cloned());
        self.flight_craft_id = craft_id;
        // A craft not fetched yet (launched from another screen, or after a reload): ask for it; the flight
        // view draws it when it arrives.
        if let (None, Some(c), Some(ws)) = (&self.flight_craft, craft_id, &self.ws) {
            send(ws, &ClientMessage::GetCraft { id: c });
        }
        if let Some(c) = craft_id {
            store(&self.craft_link_key(vessel), &c.to_string());
        }
        self.map_mode = false;
        self.throttle = 0.0;
        self.attitude = ose_net::AttitudeMode::Up;
        self.adopt_attitude = true;
        self.follow = Some(vessel);
        if let Some(el) = self.doc.get_element_by_id("flight-hud") {
            el.class_list().add_1("open").ok();
            el.set_attribute("data-ose-vessel", &vessel.to_string())
                .ok();
        }
    }

    fn end_flight(&mut self) {
        self.flying = None;
        self.flight_craft = None;
        if let Some(el) = self.doc.get_element_by_id("flight-hud") {
            el.class_list().remove_1("open").ok();
        }
    }

    fn send_controls(&mut self) {
        let Some(vessel) = self.flying else { return };
        if let Some(ws) = &self.ws {
            self.controls_dirty = false;
            send(
                ws,
                &ClientMessage::SetControls {
                    vessel,
                    throttle: self.throttle,
                    attitude: self.attitude,
                },
            );
        }
    }

    /// Parts the campaign has not researched (empty in sandbox or before the science state arrives).
    fn locked_parts(&self) -> std::collections::HashSet<String> {
        if !self.science.as_ref().map(|s| s.tech_tree).unwrap_or(false) {
            return Default::default();
        }
        self.tech
            .iter()
            .filter(|n| !self.researched.contains(&n.id))
            .flat_map(|n| n.parts.iter().cloned())
            .collect()
    }

    /// Grey out a launch pad entry whose craft uses parts the campaign has not researched (the pad refuses it).
    fn mark_craft_research(&self, id: u64) {
        let Some(craft) = self.craft_docs.get(&id) else {
            return;
        };
        let locked = self.locked_parts();
        let missing: Vec<&str> = craft
            .parts
            .iter()
            .map(|p| p.part_id.as_str())
            .filter(|p| locked.contains(*p))
            .collect();
        if let Ok(Some(li)) = self
            .doc
            .query_selector(&format!("#crafts li[data-ose-id=\"{id}\"]"))
        {
            li.class_list()
                .toggle_with_force("locked", !missing.is_empty())
                .ok();
            if missing.is_empty() {
                li.remove_attribute("title").ok();
            } else {
                li.set_attribute(
                    "title",
                    &format!("not researched yet: {} parts", missing.len()),
                )
                .ok();
            }
        }
    }

    /// Crew actions from the flight HUD: EVA, board the nearest craft, plant a flag, undock.
    fn crew_action(&self, id: &str) {
        let Some(vessel) = self.flying else { return };
        let Some(ws) = &self.ws else { return };
        let msg = match id {
            "fh-eva" => {
                self.crew_eva(None);
                return;
            }
            "fh-board" => {
                let Some(target) = self.board_candidate() else {
                    set_text(&self.doc, "fh-note", "no craft within reach to board");
                    return;
                };
                self.board_target.set(Some(target));
                ClientMessage::Board { vessel, target }
            }
            "fh-flag" => ClientMessage::PlantFlag { vessel },
            _ => ClientMessage::Undock { vessel, port: None },
        };
        self.crew_pending.set(Some(match id {
            "fh-eva" => "fh-eva",
            "fh-board" => "fh-board",
            "fh-flag" => "fh-flag",
            _ => "fh-undock",
        }));
        send(ws, &msg);
    }

    /// Send a Kerbal of the flown vessel out on EVA (`None`: whoever the server picks).
    fn crew_eva(&self, kerbal: Option<String>) {
        let Some(vessel) = self.flying else { return };
        let Some(ws) = &self.ws else { return };
        self.ftue("FTUE_First_EVA");
        self.crew_pending.set(Some("fh-eva"));
        send(ws, &ClientMessage::GoEva { vessel, kerbal });
    }

    /// The nearest other vessel (not a Kerbal or a flag) within 10 m of the Kerbal flown.
    fn board_candidate(&self) -> Option<u64> {
        let scene = self.scene.as_ref()?;
        let me = scene.vessel(self.flying?)?;
        let ut = self.time.ut_at(now_ms());
        let here = scene.vessel_position(me, ut);
        scene
            .vessels
            .iter()
            .filter(|v| v.id != me.id && v.reference_body == me.reference_body)
            .filter(|v| !v.flight.as_ref().is_some_and(|f| f.eva) && !v.name.ends_with(" flag"))
            .map(|v| (v.id, (scene.vessel_position(v, ut) - here).length()))
            .filter(|(_, d)| *d < 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Show the crew buttons that apply: EVA with crew aboard, Board and Plant Flag for a Kerbal (the flag on
    /// the ground), Undock while docked.
    /// Parts the catalog has no title for take theirs from the localization table (`Parts/Title/<id>`).
    fn name_untitled_parts(&mut self) {
        let loc = &self.loc;
        for p in self.catalog.values_mut() {
            if p.display_title() == p.id {
                if let Some(t) = loc.get(&format!("Parts/Title/{}", p.id)) {
                    p.title = t.clone();
                }
            }
        }
    }

    /// The free stack nodes a held part can snap to: none for a part that only goes on another's side
    /// (wings, radial decouplers), which goes where the pointer meets a part instead.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn snap_nodes(
        &self,
        held: Option<&str>,
    ) -> Vec<(usize, String, ose_math::DVec3, ose_math::DVec3)> {
        let stacks = held
            .and_then(|id| self.catalog.get(id))
            .is_none_or(|s| s.attach_nodes.iter().any(vab::PartSpec::is_stack_node))
            && !(self.keys.is("AltLeft") || self.keys.is("AltRight"));
        if stacks {
            self.editor.free_nodes(&self.catalog)
        } else {
            Vec::new()
        }
    }

    /// The parts manager window: the parts still on the flown vessel (staged-off ones gone), by name.
    fn render_parts_manager(&self) {
        let Some(el) = self.doc.get_element_by_id("fh-parts-list") else {
            return;
        };
        let open = self
            .doc
            .get_element_by_id("fh-parts")
            .is_some_and(|e| e.class_list().contains("open"));
        if !open {
            return;
        }
        let Some(craft) = &self.flight_craft else {
            el.set_inner_html("");
            return;
        };
        let fired = self
            .flying
            .and_then(|id| self.scene.as_ref()?.vessel(id))
            .and_then(|v| v.flight.as_ref())
            .map(|f| f.stage as usize)
            .unwrap_or(0);
        let catalog = &self.catalog;
        let is_dec = |pid: &str| catalog.get(pid).map(|p| p.is_decoupler).unwrap_or(false);
        #[cfg(target_arch = "wasm32")]
        let gone = flight3d::jettisoned(craft, fired, &is_dec);
        #[cfg(not(target_arch = "wasm32"))]
        let gone = {
            let _ = (fired, &is_dec);
            vec![false; craft.parts.len()]
        };
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, p) in craft.parts.iter().enumerate() {
            if gone.get(i).copied().unwrap_or(false) {
                continue;
            }
            let title = catalog
                .get(&p.part_id)
                .map(|s| s.display_title().to_string())
                .unwrap_or_else(|| p.part_id.clone());
            match groups.iter_mut().find(|(t, _)| *t == title) {
                Some((_, v)) => v.push(i),
                None => groups.push((title, vec![i])),
            }
        }
        let html = hud::parts_html(&groups);
        let key = format!("{}:{}", html.len(), hud::fnv(&html));
        if el.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
            el.set_inner_html(&html);
            el.set_attribute("data-ose-key", &key).ok();
        }
    }

    /// The resource manager window: the burning stage's and the whole vessel's resources (protocol 34).
    fn render_resource_manager(&self) {
        let Some(el) = self.doc.get_element_by_id("fh-res-list") else {
            return;
        };
        let open = self
            .doc
            .get_element_by_id("fh-res")
            .is_some_and(|e| e.class_list().contains("open"));
        if !open {
            return;
        }
        let Some(f) = self
            .flying
            .and_then(|id| self.scene.as_ref()?.vessel(id))
            .and_then(|v| v.flight.as_ref())
        else {
            return;
        };
        let html = hud::resources_html(&f.stage_resources, &f.resources);
        let key = format!("{}:{}", html.len(), hud::fnv(&html));
        if el.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
            el.set_inner_html(&html);
            el.set_attribute("data-ose-key", &key).ok();
        }
    }

    /// The action groups window: each group's state from the readout.
    fn render_action_groups(&self) {
        let Some(el) = self.doc.get_element_by_id("fh-ag-list") else {
            return;
        };
        let open = self
            .doc
            .get_element_by_id("fh-ag")
            .is_some_and(|e| e.class_list().contains("open"));
        if !open {
            return;
        }
        let Some(f) = self
            .flying
            .and_then(|id| self.scene.as_ref()?.vessel(id))
            .and_then(|v| v.flight.as_ref())
        else {
            return;
        };
        let html = hud::action_groups_html(f.lights, f.rcs, f.abort, &f.custom_groups);
        let key = format!("{}:{}", html.len(), hud::fnv(&html));
        if el.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
            el.set_inner_html(&html);
            el.set_attribute("data-ose-key", &key).ok();
        }
    }

    fn update_crew_buttons(&self, f: &ose_net::FlightReadout) {
        self.render_parts_manager();
        self.render_resource_manager();
        self.render_action_groups();
        let landed = matches!(f.situation, ose_net::FlightSituation::Landed);
        if let Some(el) = self.doc.get_element_by_id("fh-crewbox") {
            let face = |name: &str| {
                self.roster
                    .iter()
                    .find(|k| k.name == name)
                    .map(|k| training::face(k, "fh-face"))
            };
            let html = hud::crew_html(&f.crew, !f.eva, &face);
            let key = format!("{}:{}", html.len(), hud::fnv(&html));
            if el.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
                el.set_inner_html(&html);
                el.set_attribute("data-ose-key", &key).ok();
            }
        }
        for (id, show) in [
            ("fh-eva", !f.eva && !f.crew.is_empty()),
            ("fh-board", f.eva),
            ("fh-flag", f.eva && landed),
            ("fh-undock", !f.docked.is_empty()),
        ] {
            if let Some(el) = self.doc.get_element_by_id(id) {
                el.class_list().toggle_with_force("hidden", !show).ok();
            }
        }
    }

    /// Science from the flight HUD: 0 runs the vessel's experiments, 1 transmits its reports, 2 recovers it.
    fn flight_science(&self, what: u8) {
        let Some(vessel) = self.flying else { return };
        let Some(ws) = &self.ws else { return };
        let msg = match what {
            0 => ClientMessage::RunExperiments { vessel },
            1 => ClientMessage::TransmitScience { vessel },
            _ => ClientMessage::RecoverVessel { vessel },
        };
        set_text(&self.doc, "fh-note", "");
        send(ws, &msg);
    }

    fn flight_stage(&mut self) {
        let Some(vessel) = self.flying else { return };
        self.lesson_event("StageActivatedMessage");
        self.staged.insert(vessel);
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::Stage { vessel });
        }
    }

    fn set_throttle(&mut self, t: f64) {
        self.throttle = t.clamp(0.0, 1.0);
        self.control_cadence.sent(now_ms());
        self.send_controls();
    }

    fn set_attitude(&mut self, a: ose_net::AttitudeMode) {
        self.attitude = a;
        self.control_cadence.sent(now_ms());
        self.send_controls();
    }

    /// Where the vessel points now as pitch above the horizon and compass heading, from the flight view's
    /// navball frame (None before the first flight frame).
    fn facing_pitch_heading(&self) -> Option<(f64, f64)> {
        #[cfg(target_arch = "wasm32")]
        {
            let nav = self.flight3d.nav?;
            let f = nav.forward;
            let pitch = f.dot(nav.up).clamp(-1.0, 1.0).asin().to_degrees();
            // Straight up the heading is noise; take north, so D (yaw right) leans the rocket east as
            // the original's pad orientation does for a gravity turn.
            let heading = if pitch > 89.5 {
                0.0
            } else {
                f.dot(nav.east).atan2(f.dot(nav.north)).to_degrees()
            };
            Some((pitch, heading.rem_euclid(360.0)))
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// T: SAS stability assist, holding the direction the vessel points now. (The server always holds some
    /// direction, so there is no free-drifting "SAS off".)
    /// T: SAS off (the craft keeps turning as it was, keys apply torque) or back on, holding where it points.
    fn toggle_sas(&mut self) {
        if matches!(self.attitude, ose_net::AttitudeMode::Free) {
            self.sas_hold();
            hud::push(&self.doc, "SAS on", "passive");
        } else {
            self.set_attitude(ose_net::AttitudeMode::Free);
            hud::push(&self.doc, "SAS off", "passive");
        }
    }

    fn sas_hold(&mut self) {
        let (pitch_deg, heading_deg) = self.facing_pitch_heading().unwrap_or((90.0, 0.0));
        self.set_attitude(ose_net::AttitudeMode::PitchHeading {
            pitch_deg,
            heading_deg,
        });
    }

    /// G: fold the landing legs and wheels out, or in when any is out.
    fn toggle_gear(&mut self) {
        let Some(vessel) = self.flying else { return };
        let out = self
            .scene
            .as_ref()
            .and_then(|s| s.vessel(vessel))
            .and_then(|v| v.flight.as_ref())
            .map(|f| f.gear.iter().any(|(_, o)| *o))
            .unwrap_or(false);
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::DeployGear {
                    vessel,
                    deploy: !out,
                },
            );
        }
    }

    /// Flight keys that act while held, once per frame: W/S/A/D turn the attitude target at a steady rate
    /// (CapsLock for precision), Shift/Ctrl move the throttle at 100% a second, and on the ground W/S/A/D and
    /// B also drive the wheels (the original binds both, `WheelThrottle`/`WheelSteer`/`WheelBrakes`).
    /// True while the flown vessel is a Kerbal on EVA.
    fn flying_eva(&self) -> bool {
        self.flying
            .and_then(|id| self.scene.as_ref()?.vessel(id))
            .and_then(|v| v.flight.as_ref())
            .is_some_and(|f| f.eva)
    }

    /// A Kerbal on EVA, as the original's EVA keys (protocol 33). On the ground W walks ahead, S backs up,
    /// A/D turn, Q/E step sideways, Shift runs and Space jumps. In the air the jetpack: Shift or Space up,
    /// Ctrl down, W/S and Q/E push ahead, back and sideways. The server walks a Kerbal with throttle and a
    /// level heading; the pack takes `Translate` (y up along the Kerbal, z its front, x its right).
    fn eva_keys_tick(&mut self, now: f64, dt: f64) {
        let k = &self.keys;
        let (walk, strafe, turn) = (-k.pitch(), k.roll(), k.yaw());
        let shift = k.is("ShiftLeft") || k.is("ShiftRight");
        let ctrl = k.is("ControlLeft") || k.is("ControlRight");
        let space = k.is("Space");
        let Some(vessel) = self.flying else { return };
        let landed = self
            .scene
            .as_ref()
            .and_then(|s| s.vessel(vessel))
            .and_then(|v| v.flight.as_ref())
            .is_some_and(|f| matches!(f.situation, ose_net::FlightSituation::Landed));
        let heading = match self.attitude {
            ose_net::AttitudeMode::PitchHeading { heading_deg, .. } => heading_deg,
            _ => self.facing_pitch_heading().map(|(_, h)| h).unwrap_or(90.0),
        };
        let heading = (heading + turn * 90.0 * dt).rem_euclid(360.0);
        // Run and jump: run held while Shift is down on the ground, a jump on each new Space press there.
        let run = landed && shift;
        // A press is caught on key down, so a tap shorter than a frame still jumps.
        let jump = landed && std::mem::take(&mut self.eva_jump);
        if run != self.eva_run_sent || jump {
            self.eva_run_sent = run;
            if let Some(ws) = &self.ws {
                send(ws, &ClientMessage::EvaControls { vessel, run, jump });
            }
        }
        // The jetpack in the air.
        let translate = if landed {
            (0.0, 0.0, 0.0)
        } else {
            let up = f32::from(shift || space) - f32::from(ctrl);
            (strafe as f32, up, walk as f32)
        };
        if translate != self.translate_sent {
            self.translate_sent = translate;
            if let Some(ws) = &self.ws {
                send(
                    ws,
                    &ClientMessage::Translate {
                        vessel,
                        x: translate.0,
                        y: translate.1,
                        z: translate.2,
                    },
                );
            }
        }
        // Walking: the direction relative to the Kerbal's own heading (ahead, back, either side or between).
        // Under the pack in the air the Kerbal stands upright and keeps its heading.
        let moving = landed && (walk != 0.0 || strafe != 0.0);
        let held_pitch = match self.attitude {
            ose_net::AttitudeMode::PitchHeading { pitch_deg, .. } => pitch_deg,
            _ => 0.0,
        };
        let (pitch_deg, heading_deg, throttle) = if !landed && translate != (0.0, 0.0, 0.0) {
            (85.0, heading, 0.0)
        } else if !landed {
            // A hop or a coast: hold the pose it has.
            (held_pitch, heading, 0.0)
        } else if moving {
            let off = strafe.atan2(walk).to_degrees();
            (
                0.0,
                (heading + off).rem_euclid(360.0),
                walk.hypot(strafe).min(1.0),
            )
        } else {
            (0.0, heading, 0.0)
        };
        let attitude = ose_net::AttitudeMode::PitchHeading {
            pitch_deg,
            // Backing up or stepping sideways faces the walk direction for the server; the kept heading stays
            // the Kerbal's own.
            heading_deg,
        };
        self.attitude = if moving {
            ose_net::AttitudeMode::PitchHeading {
                pitch_deg: 0.0,
                heading_deg: heading,
            }
        } else {
            attitude
        };
        let changed = throttle != self.throttle || turn != 0.0 || moving;
        if changed || self.controls_dirty {
            self.throttle = throttle;
            self.controls_dirty = true;
        }
        if self
            .control_cadence
            .due(now, self.controls_dirty, [walk, turn, strafe])
        {
            self.controls_dirty = false;
            if let Some(ws) = &self.ws {
                send(
                    ws,
                    &ClientMessage::SetControls {
                        vessel,
                        throttle,
                        attitude,
                    },
                );
            }
        }
    }

    fn flight_keys_tick(&mut self, now: f64) {
        let dt = ((now - self.keys_tick_ms) / 1000.0).clamp(0.0, 0.1);
        self.keys_tick_ms = now;
        let menu_open = self
            .doc
            .get_element_by_id("menu")
            .map(|m| !m.class_list().contains("hidden"))
            .unwrap_or(false);
        let Some(vessel) = self.flying else { return };
        if menu_open {
            self.keys.clear();
        }
        if self.adopt_attitude {
            let held = self
                .scene
                .as_ref()
                .and_then(|s| s.vessel(vessel))
                .and_then(|v| v.flight.as_ref())
                .map(|f| (f.attitude, f.throttle));
            if let Some((att, thr)) = held {
                self.attitude = att;
                self.throttle = thr;
                self.adopt_attitude = false;
            }
        }
        if self.flying_eva() {
            self.eva_keys_tick(now, dt);
            return;
        }
        let k = &self.keys;
        let (pitch_in, yaw_in, thr_in) = if self.lin_mode {
            (0.0, 0.0, 0.0)
        } else {
            (k.pitch(), k.yaw(), k.throttle())
        };
        let scale = k.scale();
        let drive = (
            k.wheel_throttle() as f32 * scale as f32,
            k.wheel_steer() as f32 * scale as f32,
            k.brakes() || self.brakes_latched,
        );
        if thr_in != 0.0 {
            self.throttle = (self.throttle + thr_in * input::THROTTLE_RATE * dt).clamp(0.0, 1.0);
            self.controls_dirty = true;
        }
        let free = matches!(self.attitude, ose_net::AttitudeMode::Free);
        if (pitch_in != 0.0 || yaw_in != 0.0) && !free {
            // Start turning from where the vessel points when the target is not a free direction yet.
            let (p, h) = match self.attitude {
                ose_net::AttitudeMode::PitchHeading {
                    pitch_deg,
                    heading_deg,
                } => (pitch_deg, heading_deg),
                _ => self.facing_pitch_heading().unwrap_or((90.0, 0.0)),
            };
            let step = input::TURN_RATE_DEG_S * scale * dt;
            // W (-1) lowers the nose: positive pitch input is S.
            let (pitch_deg, heading_deg) = input::turn(p, h, pitch_in * step, yaw_in * step);
            self.attitude = ose_net::AttitudeMode::PitchHeading {
                pitch_deg,
                heading_deg,
            };
            self.controls_dirty = true;
        }
        // Twenty updates a second while held; input edges flush the newest target immediately.
        if self
            .control_cadence
            .due(now, self.controls_dirty, [pitch_in, yaw_in, thr_in])
        {
            self.controls_dirty = false;
            self.send_controls();
        }
        let landed = self
            .scene
            .as_ref()
            .and_then(|s| s.vessel(vessel))
            .and_then(|v| v.flight.as_ref())
            .map(|f| f.situation == ose_net::FlightSituation::Landed)
            .unwrap_or(false);
        let roll = self.keys.roll() as f32 * self.keys.scale() as f32;
        // SAS off: the keys turn the craft directly (pitch toward its top, yaw to its right, roll clockwise).
        let s = self.keys.scale() as f32;
        let rotation = if free {
            (pitch_in as f32 * s, yaw_in as f32 * s, roll)
        } else {
            (0.0, 0.0, 0.0)
        };
        if rotation != self.rotation_sent {
            self.rotation_sent = rotation;
            if let Some(ws) = &self.ws {
                send(
                    ws,
                    &ClientMessage::SetRotation {
                        vessel,
                        pitch: rotation.0,
                        yaw: rotation.1,
                        roll: rotation.2,
                    },
                );
            }
        }
        let roll = if free { 0.0 } else { roll };
        if roll != self.roll_sent {
            self.roll_sent = roll;
            if let Some(ws) = &self.ws {
                send(
                    ws,
                    &ClientMessage::SetRoll {
                        vessel,
                        input: roll,
                    },
                );
            }
        }
        let (tx, ty, tz) = self.keys.translate();
        // Docking mode adds W/S up and down, A/D left and right, Shift/Ctrl forward and back.
        let (tx, ty, tz) = if self.lin_mode {
            (
                (tx + self.keys.yaw()).clamp(-1.0, 1.0),
                (ty + self.keys.throttle()).clamp(-1.0, 1.0),
                (tz - self.keys.pitch()).clamp(-1.0, 1.0),
            )
        } else {
            (tx, ty, tz)
        };
        let s = self.keys.scale() as f32;
        let translate = (tx as f32 * s, ty as f32 * s, tz as f32 * s);
        if translate != self.translate_sent {
            self.translate_sent = translate;
            if let Some(ws) = &self.ws {
                send(
                    ws,
                    &ClientMessage::Translate {
                        vessel,
                        x: translate.0,
                        y: translate.1,
                        z: translate.2,
                    },
                );
            }
        }
        let drive = if landed { drive } else { (0.0, 0.0, drive.2) };
        if drive != self.drive_sent {
            self.drive_sent = drive;
            if let Some(ws) = &self.ws {
                send(
                    ws,
                    &ClientMessage::Drive {
                        vessel,
                        throttle: drive.0,
                        steer: drive.1,
                        brakes: drive.2,
                    },
                );
            }
        }
    }

    /// Start warping to `ut` (the original's WarpTo on nodes and orbit markers).
    fn start_warp_to(&mut self, ut: f64, what: &str) {
        let now = self.time.ut_at(now_ms());
        if !ut.is_finite() || ut <= now + 10.0 {
            return;
        }
        self.warp_to = Some((ut, what.to_string()));
        self.warp_to_sent_ms = 0.0;
        self.lesson_event("TimeWarpToLocationInitializedMessage");
        hud::push(&self.doc, &format!("Warping to {what}"), "passive");
    }

    /// Step the warp toward the warp-to point: as fast as leaves a few seconds, 1x for the last ten.
    fn warp_to_tick(&mut self, now: f64) {
        let Some((at, what)) = self.warp_to.clone() else {
            return;
        };
        let ut = self.time.ut_at(now);
        let remaining = at - ut;
        if remaining <= 2.0 {
            self.warp_to = None;
            if self.time.warp != 1 {
                self.request_warp(1);
            }
            hud::push(&self.doc, &format!("Arrived: {what}"), "triumph");
            self.lesson_event("TimeWarpToLocationCompletedMessage");
            return;
        }
        let want = kerbin_time::warp_toward(remaining);
        if want != self.time.warp && now - self.warp_to_sent_ms > 600.0 {
            self.warp_to_sent_ms = now;
            self.request_warp(want);
        }
    }

    /// The direction from the flown vessel to the target (star frame, unit), if both exist.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn target_dir(&self) -> Option<ose_math::DVec3> {
        let scene = self.scene.as_ref()?;
        let ut = self.time.ut_at(now_ms());
        let me = scene.vessel(self.flying?)?;
        let there = match self.target? {
            ose_net::TargetId::Vessel(id) => scene.vessel_position(scene.vessel(id)?, ut),
            ose_net::TargetId::Body(b) => scene.position(b as usize, ut),
        };
        let d = there - scene.vessel_position(me, ut);
        (d.length() > 1e-6).then(|| d * (1.0 / d.length()))
    }

    /// Tell the server the flown vessel's target (protocol 36), or that it has none.
    fn send_target(&self) {
        let (Some(vessel), Some(ws)) = (self.flying, &self.ws) else {
            return;
        };
        send(
            ws,
            &ClientMessage::SetTarget {
                vessel,
                target: self.target,
            },
        );
    }

    /// The target's name: the vessel's, or the body's.
    fn target_name(&self, t: ose_net::TargetId) -> String {
        let Some(scene) = &self.scene else {
            return String::new();
        };
        match t {
            ose_net::TargetId::Vessel(id) => scene.vessel(id).map(|v| v.name.clone()),
            ose_net::TargetId::Body(b) => scene.bodies.get(b as usize).map(|b| b.id.clone()),
        }
        .unwrap_or_default()
    }

    /// Right click on a vessel or body label: make it the target, or clear it when it already is.
    fn toggle_target(&mut self, id: ose_net::TargetId) {
        if Some(id) == self.flying.map(ose_net::TargetId::Vessel) {
            return;
        }
        let name = self.target_name(id);
        if self.target == Some(id) {
            self.target = None;
            self.speed_target = false;
            // A target hold has nothing left to point at.
            if matches!(
                self.attitude,
                ose_net::AttitudeMode::Target | ose_net::AttitudeMode::AntiTarget
            ) {
                self.sas_hold();
            }
            hud::push(&self.doc, "Target cleared", "passive");
        } else {
            self.target = Some(id);
            hud::push(&self.doc, &format!("Target: {name}"), "passive");
        }
        self.send_target();
        for (vid, el) in &self.vessel_els {
            el.class_list()
                .toggle_with_force(
                    "target",
                    Some(ose_net::TargetId::Vessel(*vid)) == self.target,
                )
                .ok();
        }
    }

    /// Toggle an action group of the flown vessel.
    fn action_group(&self, group: ose_net::ActionGroup) {
        let (Some(vessel), Some(ws)) = (self.flying, &self.ws) else {
            return;
        };
        send(
            ws,
            &ClientMessage::ActionGroup {
                vessel,
                group,
                on: None,
            },
        );
    }

    /// Quicksave, quickload and the reverts (F5, F9 and `[data-save-action]` buttons such as the pause menu's).
    fn save_action(&self, action: ose_net::SaveAction) {
        let Some(ws) = &self.ws else { return };
        let msg = match action {
            ose_net::SaveAction::QuickSave => ClientMessage::QuickSave,
            ose_net::SaveAction::QuickLoad => ClientMessage::QuickLoad,
            ose_net::SaveAction::RevertToLaunch => ClientMessage::RevertToLaunch,
            ose_net::SaveAction::RevertToEditor => ClientMessage::RevertToEditor,
        };
        send(ws, &msg);
    }

    /// A SAS mode or action bar button.
    fn hud_button(&mut self, id: &str) {
        let Some(vessel) = self.flying else { return };
        if id == "fh-sas-stability" {
            self.sas_hold();
            return;
        }
        if id == "fh-sas-target" || id == "fh-sas-antitarget" {
            if self.target.is_some() {
                self.set_attitude(if id == "fh-sas-target" {
                    ose_net::AttitudeMode::Target
                } else {
                    ose_net::AttitudeMode::AntiTarget
                });
            }
            return;
        }
        if let Some(mode) = hud::sas_mode(id, self.speed_orbit) {
            self.set_attitude(mode);
            return;
        }
        let f = self
            .scene
            .as_ref()
            .and_then(|s| s.vessel(vessel))
            .and_then(|v| v.flight.clone());
        let msg = match id {
            "fh-act-gear" => {
                self.toggle_gear();
                return;
            }
            "fh-act-brakes" => {
                // Latched brakes add to the held B key; the next frame sends them with the wheel input.
                self.brakes_latched = !self.brakes_latched;
                return;
            }
            "fh-act-science" => {
                self.flight_science(0);
                return;
            }
            "fh-act-solar" => ClientMessage::DeployPanels {
                vessel,
                deploy: !f.map(|f| f.panels_deployed).unwrap_or(false),
            },
            "fh-act-chutes" => ClientMessage::DeployParachutes { vessel },
            "fh-act-fairings" => ClientMessage::DeployFairings { vessel },
            "fh-act-bays" => ClientMessage::SetCargoBays {
                vessel,
                open: !f
                    .map(|f| f.cargo_bays.iter().any(|(_, open)| *open))
                    .unwrap_or(false),
            },
            "fh-act-lights" => ClientMessage::ActionGroup {
                vessel,
                group: ose_net::ActionGroup::Lights,
                on: None,
            },
            "fh-act-rcs" => ClientMessage::ActionGroup {
                vessel,
                group: ose_net::ActionGroup::Rcs,
                on: None,
            },
            "fh-act-abort" => ClientMessage::ActionGroup {
                vessel,
                group: ose_net::ActionGroup::Abort,
                on: Some(true),
            },
            "fh-act-converters" => ClientMessage::SetConverters {
                vessel,
                on: !f
                    .map(|f| f.converters.iter().any(|(_, on, _)| *on))
                    .unwrap_or(false),
            },
            _ => return,
        };
        if let Some(ws) = &self.ws {
            send(ws, &msg);
        }
    }

    /// The new flight instruments (`hud`): speed mode, apsides, SAS and action bar lights.
    fn update_flight_instruments(&self) {
        let Some(id) = self.flying else { return };
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        let Some(v) = scene.vessel(id) else { return };
        let Some(f) = &v.flight else { return };
        let body = scene.bodies.get(v.reference_body as usize);
        let body_vel = scene
            .snapshot
            .get(v.reference_body as usize)
            .map(|b| b.velocity_mps)
            .unwrap_or_default();
        if let (Some(craft), Some(list)) =
            (&self.flight_craft, self.doc.get_element_by_id("fh-stages"))
        {
            // `stage` is the stage burning now; before the first Space nothing burns and stage 0 goes next.
            let lit = self.staged.contains(&id)
                || f.stage > 0
                || f.thrust_kn > 0.1
                || f.situation != ose_net::FlightSituation::Landed;
            let next = f.stage as usize + usize::from(lit);
            let html = hud::staging_html(craft, &self.catalog, next, &f.parachutes, &f.stage_list);
            // Compare with what was set last (the browser rewrites inner HTML, so it would never match).
            let key = format!("{}:{}", html.len(), hud::fnv(&html));
            if list.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
                list.set_inner_html(&html);
                list.set_attribute("data-ose-key", &key).ok();
            }
        }
        if let (Some(sci), Some(list)) = (
            &self.science,
            self.doc.get_element_by_id("fh-missions-list"),
        ) {
            let html = hud::tracker_html(&sci.missions, &self.loc);
            let key = format!("{}:{}", html.len(), hud::fnv(&html));
            if list.get_attribute("data-ose-key").as_deref() != Some(key.as_str()) {
                list.set_inner_html(&html);
                list.set_attribute("data-ose-key", &key).ok();
            }
        }
        let ut = self.time.ut_at(now_ms());
        // Burn timer for a node planned on the flown vessel.
        if let Some(el) = self.doc.get_element_by_id("fh-burn") {
            match self.node.filter(|n| n.vessel == id) {
                Some(n) => {
                    let dv = (n.prograde_mps.powi(2) + n.normal_mps.powi(2) + n.radial_mps.powi(2))
                        .sqrt();
                    let full = if f.throttle > 0.01 {
                        f.thrust_kn / f.throttle
                    } else {
                        0.0
                    };
                    el.set_text_content(Some(&hud::burn_text(n.ut - ut, dv, f.mass_t, full)));
                    el.class_list().add_1("on").ok();
                }
                None => {
                    el.class_list().remove_1("on").ok();
                }
            }
        }
        let apsides = body
            .map(|b| hud::apsides(&v.orbit, b.radius_m, b.mu, ut))
            .unwrap_or((None, (f64::NAN, f64::NAN)));
        hud::update(
            &self.doc,
            &hud::HudState {
                f,
                attitude: &self.attitude,
                orbit_speed: self.speed_orbit,
                orbital_speed_mps: (v.velocity_mps - body_vel).length(),
                apsides,
                brakes_latched: self.brakes_latched,
            },
        );
        // Mission elapsed time and the records for the flight report.
        let launch_ut = stored(&format!("ose.launchUt.{id}"))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        set_text(
            &self.doc,
            "fh-met",
            &if launch_ut > 0.0 {
                format!("MET {}", hud::fmt_met(ut - launch_ut))
            } else {
                String::new()
            },
        );
        {
            let mut rec = self.flight_records.borrow_mut();
            if rec.as_ref().map(|r| r.vessel) != Some(id) {
                *rec = Some(hud::FlightRecords {
                    vessel: id,
                    name: v.name.clone(),
                    launch_ut,
                    ..Default::default()
                });
            }
            if let (Some(r), Some(b)) = (rec.as_mut(), body) {
                r.note(f.altitude_m, f.surface_speed_mps.max(0.0), &b.id);
            }
        }
        if f.situation == ose_net::FlightSituation::Crashed
            && self.flight_watch.borrow().as_ref().is_some_and(|w| {
                w.vessel == id && w.situation != Some(ose_net::FlightSituation::Crashed)
            })
        {
            self.show_flight_report("The vessel was lost", None);
        }
        // Event toasts from what changed since the last frame.
        let watch = hud::FlightWatch {
            vessel: id,
            comm: f.comm_connected || f.comm_range_m <= 0.0,
            control: f.has_control,
            body: v.reference_body,
            situation: Some(f.situation),
            charge_low: f.electric_capacity > 0.0 && f.electric_charge < f.electric_capacity * 0.05,
        };
        let before = self.flight_watch.replace(Some(watch.clone()));
        if let Some(before) = before {
            let name = body.map(|b| b.id.as_str()).unwrap_or("the body");
            for (text, tier) in hud::flight_events(&before, &watch, name) {
                hud::push(&self.doc, &text, tier);
            }
        }
        // Altimeter mode: sea level (the readout above) or terrain, from the planet map the ground is drawn from.
        #[cfg(target_arch = "wasm32")]
        let agl = self.flight3d.agl_m;
        #[cfg(not(target_arch = "wasm32"))]
        let agl: Option<f64> = None;
        match (self.alt_terrain, agl) {
            (true, Some(h)) => {
                set_text(&self.doc, "fh-alt-mode", "TERRAIN");
                set_text(&self.doc, "fh-altitude", &hud::fmt_dist(h.max(0.0)));
            }
            _ => set_text(&self.doc, "fh-alt-mode", "SEA LEVEL"),
        }
        // Target: distance and closing speed, the TARGET speed mode, and the SAS target buttons.
        let tgt = match self.target {
            Some(ose_net::TargetId::Vessel(t)) => scene.vessel(t),
            _ => None,
        };
        if matches!(self.target, Some(ose_net::TargetId::Vessel(_))) && tgt.is_none() {
            // The target is gone (recovered, crashed).
            set_text(&self.doc, "fh-target", "");
        }
        // A body target: the server's distance to its surface and speed relative to it.
        let body_rel = match (self.target, v.target) {
            (Some(ose_net::TargetId::Body(b)), Some(r))
                if r.target == ose_net::TargetId::Body(b) =>
            {
                Some((
                    self.target_name(ose_net::TargetId::Body(b)),
                    r.distance_m,
                    r.relative_velocity_mps.length(),
                ))
            }
            _ => None,
        };
        let rel = body_rel.or_else(|| {
            tgt.map(|t| match v.target {
                Some(r) if r.target == ose_net::TargetId::Vessel(t.id) => (
                    t.name.clone(),
                    r.distance_m,
                    r.relative_velocity_mps.length(),
                ),
                _ => {
                    let d = scene.vessel_position(t, ut) - scene.vessel_position(v, ut);
                    (
                        t.name.clone(),
                        d.length(),
                        (t.velocity_mps - v.velocity_mps).length(),
                    )
                }
            })
        });
        match &rel {
            Some((name, dist, speed)) => set_text(
                &self.doc,
                "fh-target",
                &format!(
                    "Target {name} \u{b7} {} \u{b7} {speed:.1} m/s",
                    hud::fmt_dist(*dist)
                ),
            ),
            None => set_text(&self.doc, "fh-target", ""),
        }
        if self.speed_target {
            match &rel {
                Some((_, _, speed)) => {
                    set_text(&self.doc, "fh-speed-mode", "TARGET");
                    set_text(&self.doc, "fh-speed", &format!("{speed:.1} m/s"));
                }
                None => set_text(&self.doc, "fh-speed-mode", "SURFACE"),
            }
        }
        for id in ["fh-sas-target", "fh-sas-antitarget"] {
            if let Some(b) = self.doc.get_element_by_id(id) {
                if rel.is_some() {
                    b.remove_attribute("disabled").ok();
                } else {
                    b.set_attribute("disabled", "").ok();
                }
                let on = match self.attitude {
                    ose_net::AttitudeMode::Target => id == "fh-sas-target",
                    ose_net::AttitudeMode::AntiTarget => id == "fh-sas-antitarget",
                    _ => false,
                };
                b.class_list().toggle_with_force("on", on).ok();
            }
        }
    }

    /// Home: put the camera of the current screen back where it starts.
    fn reset_camera(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            if self.flight_active() {
                let d = flight3d::FlightView::new();
                (
                    self.flight3d.yaw,
                    self.flight3d.pitch,
                    self.flight3d.distance,
                ) = (d.yaw, d.pitch, d.distance);
                return;
            }
            if self.in_session && self.vab_active() {
                let d = vab3d::VabView::new();
                (self.vab3d.yaw, self.vab3d.pitch, self.vab3d.distance) =
                    (d.yaw, d.pitch, d.distance);
                self.vab3d.pan_y = 0.0;
                return;
            }
            if self.in_session && self.ksc_active() {
                self.ksc3d.focus(None);
                return;
            }
        }
        self.frame_focus();
    }

    /// The original's loading screen on a scene change: the destination's art and a loading tip, held until the
    /// scene's models arrive (at least a moment, at most a few seconds). It never takes clicks.
    fn loading_screen(&mut self, place: Option<&'static str>, now: f64) {
        if place != self.loading_place {
            let from = std::mem::replace(&mut self.loading_place, place);
            if let Some(to) = place {
                let art = match (from, to) {
                    (None, _) => {
                        if js_sys::Math::random() < 0.5 {
                            "game"
                        } else {
                            "game2"
                        }
                    }
                    (Some("flight"), "ksc") => "recover",
                    (Some("vab") | Some("ksc"), "flight") => {
                        if self
                            .launch_site
                            .as_deref()
                            .is_some_and(|s| s.starts_with("Runway"))
                        {
                            "runway"
                        } else {
                            "launchpad"
                        }
                    }
                    (_, "flight") => "game2",
                    (_, p) => p,
                };
                let tips: Vec<&String> = (2..400)
                    .map_while(|i| self.loc.get(&format!("LoadingTips/Tip{i}")))
                    .collect();
                let tip = if tips.is_empty() {
                    "Loading...".to_string()
                } else {
                    let i = (js_sys::Math::random() * tips.len() as f64) as usize;
                    tips[i.min(tips.len() - 1)].replace('\u{fffd}', "...")
                };
                if let Some(img) = self.doc.get_element_by_id("loading-art") {
                    img.set_attribute("src", &format!("assets/loading/{art}.jpg"))
                        .ok();
                }
                set_text(&self.doc, "loading-tip", &tip);
                if let Some(el) = self.doc.get_element_by_id("loading") {
                    el.set_attribute("data-ose-art", art).ok();
                    el.class_list().add_1("show").ok();
                }
                self.loading_since = Some(now);
            }
        }
        let Some(t0) = self.loading_since else {
            return;
        };
        #[cfg(target_arch = "wasm32")]
        let loading = self.gpu.as_ref().map(|g| g.models_loading()).unwrap_or(0);
        #[cfg(not(target_arch = "wasm32"))]
        let loading = 0usize;
        let shown = now - t0;
        if self.loading_place.is_none() || (shown > 900.0 && loading == 0) || shown > 6000.0 {
            if let Some(el) = self.doc.get_element_by_id("loading") {
                el.class_list().remove_1("show").ok();
            }
            self.loading_since = None;
        }
    }

    /// The flight's other sounds: wind rising with dynamic pressure, the re-entry roar with heat flux, a sonic
    /// boom through Mach 1, a part overheating, chutes opening and cut, legs and panels moving.
    fn flight_sounds(&mut self, flight: Option<(u64, &ose_net::FlightReadout)>, menu_open: bool) {
        let Some((id, f)) = flight.filter(|_| !menu_open) else {
            self.audio.level_loop("wind", 0.0, 1.0);
            self.audio.level_loop("reentry", 0.0, 1.0);
            self.audio.level_loop("jetpack", 0.0, 1.0);
            self.flight_sound_state = None;
            return;
        };
        let wind = (f.dynamic_pressure_kpa / 25.0).clamp(0.0, 1.0);
        self.audio.level_loop("wind", wind, 0.8 + 0.4 * wind);
        let flux = f.heat.iter().map(|h| h.3).fold(0.0f32, f32::max) as f64;
        self.audio
            .level_loop("reentry", (flux / 300.0).clamp(0.0, 1.0), 1.0);
        let chutes: Vec<(u16, ose_net::ChuteState)> =
            f.parachutes.iter().map(|c| (c.0, c.1)).collect();
        let now = FlightSoundState {
            vessel: id,
            supersonic: f.surface_speed_mps > 343.0 && f.dynamic_pressure_kpa > 1.0,
            gear_out: f.gear.iter().any(|g| g.1),
            panels: f.panels_deployed,
            hot: f
                .heat
                .iter()
                .filter(|h| h.2 > 0.0 && h.1 / h.2 > 0.8)
                .map(|h| h.0)
                .collect(),
            chutes,
            bays: f.cargo_bays.clone(),
            fairings_gone: f.fairings.iter().filter(|x| x.1).count(),
            step_at: self
                .flight_sound_state
                .as_ref()
                .filter(|w| w.vessel == id)
                .map(|w| w.step_at)
                .unwrap_or(0.0),
        };
        // A Kerbal on EVA: footsteps while walking, the jetpack while flying it.
        let landed = matches!(f.situation, ose_net::FlightSituation::Landed);
        // Walking moves the Kerbal without a surface speed, so the keys say when it walks (and runs).
        let k = &self.keys;
        let moving = k.pitch() != 0.0 || k.roll() != 0.0;
        let thrusting = moving || k.is("Space") || k.is("ShiftLeft") || k.is("ControlLeft");
        let running = k.is("ShiftLeft") || k.is("ShiftRight");
        self.audio.level_loop(
            "jetpack",
            if f.eva && !landed && thrusting {
                1.0
            } else {
                0.0
            },
            1.0,
        );
        let mut now = now;
        let t = now_ms();
        let pace = if running { 300.0 } else { 450.0 };
        if f.eva && landed && moving && t - now.step_at > pace {
            self.audio.play("footstep");
            now.step_at = t;
        }
        if let Some(was) = self.flight_sound_state.as_ref().filter(|w| w.vessel == id) {
            for (part, open) in &now.bays {
                if was.bays.iter().any(|b| b.0 == *part && b.1 != *open) {
                    self.audio
                        .play(if *open { "bay_open" } else { "bay_close" });
                }
            }
            if now.fairings_gone > was.fairings_gone {
                self.audio.play("decouple");
            }
            if now.supersonic && !was.supersonic {
                self.audio.play("sonic_boom");
            }
            if now.gear_out != was.gear_out {
                self.audio.play(if now.gear_out {
                    "leg_deploy"
                } else {
                    "leg_retract"
                });
            }
            if now.panels != was.panels {
                self.audio.play(if now.panels {
                    "panel_open"
                } else {
                    "panel_close"
                });
            }
            if now.hot.iter().any(|p| !was.hot.contains(p)) {
                self.audio.play("overheat");
            }
            for (part, state) in &now.chutes {
                let before = was.chutes.iter().find(|c| c.0 == *part).map(|c| c.1);
                if before != Some(*state) {
                    match state {
                        ose_net::ChuteState::Deployed | ose_net::ChuteState::SemiDeployed
                            if !matches!(
                                before,
                                Some(ose_net::ChuteState::Deployed)
                                    | Some(ose_net::ChuteState::SemiDeployed)
                            ) =>
                        {
                            self.audio.play("parachute")
                        }
                        ose_net::ChuteState::Cut => self.audio.play("chute_cut"),
                        _ => {}
                    }
                }
            }
        }
        self.flight_sound_state = Some(now);
    }

    fn show_sound_toggle(&self) {
        for id in ["sound-toggle", "main-sound", "settings-sound"] {
            if let Some(el) = self.doc.get_element_by_id(id) {
                el.set_text_content(Some(if self.audio.muted {
                    "Sound: off"
                } else {
                    "Sound: on"
                }));
            }
        }
    }

    /// The scene's music, the engine loop and staging sounds in flight.
    fn update_audio(&mut self, now: f64) {
        let menu_open = self
            .doc
            .get_element_by_id("menu")
            .map(|m| !m.class_list().contains("hidden"))
            .unwrap_or(false);
        let front = self
            .doc
            .query_selector(
                "#menu:not(.hidden) #screen-name.open, #menu:not(.hidden) #screen-sessions.open",
            )
            .ok()
            .flatten()
            .is_some();
        let flight = self.flying.and_then(|id| {
            let scene = self.scene.as_ref()?;
            let v = scene.vessel(id)?;
            let body = scene.bodies.get(v.reference_body as usize)?.id.clone();
            Some((id, body, v.flight.clone()))
        });
        let scene = match &flight {
            _ if !self.in_session || front => audio::Scene::MainMenu,
            _ if self.vab_active() => audio::Scene::Vab,
            _ if self.tracking_open && self.flying.is_none() && !menu_open => {
                audio::Scene::Tracking
            }
            _ if self.screen_open("rnd") => audio::Scene::Rnd,
            _ if self.screen_open("mission") => audio::Scene::Mission,
            Some((_, body, _)) if !menu_open => audio::Scene::Flight(body),
            _ => audio::Scene::Ksc,
        };
        self.audio.music(Some(audio::scene_music(scene)));
        let place = match scene {
            audio::Scene::MainMenu => None,
            audio::Scene::Vab => Some("vab"),
            audio::Scene::Rnd => Some("rnd"),
            audio::Scene::Mission => Some("mission"),
            _ if self.screen_open("training") => Some("training"),
            _ if self.flying.is_some() => Some("flight"),
            audio::Scene::Tracking => Some("tracking"),
            _ if self.tracking_open && menu_open => self.loading_place,
            _ => Some("ksc"),
        };
        self.loading_screen(place, now);
        // Ambience under the music: the launch pad emitter around the KSC, the hall in the VAB.
        self.audio.ambience(match scene {
            audio::Scene::Vab => Some("vab_ambience"),
            _ => None,
        });
        let mut level = 0.0;
        let mut solid = false;
        if let Some((id, _, Some(f))) = &flight {
            if f.thrust_kn > 0.1 && !menu_open {
                // Thrust with the throttle closed is solid boosters, which have no throttle.
                solid = f.throttle <= 0.01;
                level = if solid { 1.0 } else { f.throttle.max(0.2) };
            }
            #[cfg(not(target_arch = "wasm32"))]
            let gone = 0usize;
            #[cfg(target_arch = "wasm32")]
            let gone = self
                .flight_craft
                .as_ref()
                .map(|craft| {
                    let catalog = &self.catalog;
                    let is_dec =
                        |pid: &str| catalog.get(pid).map(|p| p.is_decoupler).unwrap_or(false);
                    flight3d::jettisoned(craft, f.stage as usize, &is_dec)
                        .iter()
                        .filter(|g| **g)
                        .count()
                })
                .unwrap_or(0);
            if let Some((vid, stage, was_gone)) = self.last_stage {
                if vid == *id && f.stage > stage {
                    self.audio.play("stage");
                    if gone > was_gone {
                        self.audio.play("decouple");
                    }
                }
            }
            self.last_stage = Some((*id, f.stage, gone));
            if f.situation == ose_net::FlightSituation::Crashed
                && self
                    .last_situation
                    .as_ref()
                    .is_some_and(|(v, s)| *v == *id && *s != ose_net::FlightSituation::Crashed)
            {
                self.audio.play("explosion");
            }
            self.last_situation = Some((*id, f.situation));
        } else {
            self.last_stage = None;
        }
        let mass = flight
            .as_ref()
            .and_then(|(_, _, f)| f.as_ref().map(|f| f.mass_t))
            .unwrap_or(10.0);
        // Heavier rockets rumble lower.
        let rate = (1.15 - 0.08 * mass.max(1.0).ln()).clamp(0.75, 1.15);
        let key = if solid { "engine_srb" } else { "engine_liquid" };
        self.audio.engine(key, level, rate);
        self.flight_sounds(
            flight
                .as_ref()
                .and_then(|(id, _, f)| f.as_ref().map(|f| (*id, f))),
            menu_open,
        );
        self.audio.tick(now, &self.doc);
    }

    /// Refresh the flight HUD from the vessel's readout.
    fn update_flight_hud(&self) {
        let Some(id) = self.flying else { return };
        if let Some(el) = self.doc.get_element_by_id("flight-hud") {
            el.class_list().toggle_with_force("map", self.map_mode).ok();
        }
        let Some(v) = self.scene.as_ref().and_then(|s| s.vessel(id)) else {
            return;
        };
        let Some(f) = &v.flight else {
            // On rails: the readout's last values would be stale; altitude and speed from the orbit.
            if let Some(scene) = &self.scene {
                let rb = v.reference_body as usize;
                if let Some(body) = scene.bodies.get(rb) {
                    let ut = self.time.ut_at(now_ms());
                    let alt = (scene.vessel_position(v, ut) - scene.position(rb, ut)).length()
                        - body.radius_m;
                    set_text(&self.doc, "fh-altitude", &tracking::km(alt));
                    if let Some(el) = self.doc.get_element_by_id("flight-hud") {
                        el.set_attribute("data-ose-altitude", &format!("{alt:.1}"))
                            .ok();
                    }
                }
            }
            for id in ["fh-speed", "fh-vspeed", "fh-twr"] {
                set_text(&self.doc, id, "--");
            }
            set_text(&self.doc, "fh-situation", "on rails");
            return;
        };
        let alt = if f.altitude_m.abs() >= 100_000.0 {
            format!("{:.1} km", f.altitude_m / 1000.0)
        } else {
            format!("{:.0} m", f.altitude_m)
        };
        set_text(&self.doc, "fh-altitude", &alt);
        set_text(
            &self.doc,
            "fh-speed",
            &format!("{:.1} m/s", f.surface_speed_mps),
        );
        set_text(
            &self.doc,
            "fh-vspeed",
            &format!("{:+.1} m/s", f.vertical_speed_mps),
        );
        set_text(&self.doc, "fh-situation", &format!("{:?}", f.situation));
        self.update_crew_buttons(f);
        // Where the vessel stands on its body (latitude, longitude in degrees), for the walking test hook.
        if let Some(scene) = &self.scene {
            let rb = v.reference_body as usize;
            let ut = self.time.ut_at(now_ms());
            let r = scene.vessel_position(v, ut) - scene.position(rb, ut);
            let spin = self
                .body_spins
                .get(rb)
                .map(|b| {
                    let w = if b.rotation_period_s > 0.0 {
                        std::f64::consts::TAU / b.rotation_period_s
                    } else {
                        0.0
                    };
                    b.initial_rotation_deg.to_radians() + w * ut
                })
                .unwrap_or(0.0);
            let lat = (r.z / r.length().max(1.0))
                .clamp(-1.0, 1.0)
                .asin()
                .to_degrees();
            let lon = (r.y.atan2(r.x) - spin).to_degrees().rem_euclid(360.0);
            if let Some(el) = self.doc.get_element_by_id("flight-hud") {
                el.set_attribute("data-ose-latlon", &format!("{lat:.6},{lon:.6}"))
                    .ok();
            }
        }
        // Timed experiments counting down from the server's last report.
        let elapsed = (now_ms() - self.running_since_ms) / 1000.0;
        let running: Vec<String> = self
            .running
            .iter()
            .map(|r| {
                format!(
                    "{} {:.0} s",
                    experiment_name(&r.experiment),
                    (r.remaining_s - elapsed).max(0.0).ceil()
                )
            })
            .collect();
        if let Some(el) = self.doc.get_element_by_id("fh-running") {
            el.set_text_content(Some(&running.join(" \u{b7} ")));
            el.set_attribute("data-ose-running", &self.running.len().to_string())
                .ok();
        }
        set_text(
            &self.doc,
            "fh-stage",
            &format!(
                "stage {} of {} \u{b7} \u{394}v {:.0} m/s",
                (f.stage + 1).min(f.stages.max(1)),
                f.stages.max(1),
                f.delta_v_now_mps.max(0.0) + 0.0
            ),
        );
        set_text(
            &self.doc,
            "fh-twr",
            &format!("TWR {:.2} \u{b7} {:.0} kN", f.twr, f.thrust_kn),
        );
        set_text(
            &self.doc,
            "fh-fuel",
            &format!(
                "{:.1} t ({:.0}%)",
                f.propellant_t,
                f.propellant_fraction * 100.0
            ),
        );
        let att = match f.attitude {
            ose_net::AttitudeMode::PitchHeading {
                pitch_deg,
                heading_deg,
            } => format!("pitch {pitch_deg:.0}\u{b0} hdg {heading_deg:.0}\u{b0}"),
            ose_net::AttitudeMode::Fixed(_) => "Holding a direction".to_string(),
            ose_net::AttitudeMode::SurfacePrograde => "Prograde (surface)".to_string(),
            ose_net::AttitudeMode::SurfaceRetrograde => "Retrograde (surface)".to_string(),
            ose_net::AttitudeMode::OrbitPrograde => "Prograde (orbit)".to_string(),
            ose_net::AttitudeMode::OrbitRetrograde => "Retrograde (orbit)".to_string(),
            ose_net::AttitudeMode::AntiNormal => "Anti-normal".to_string(),
            ose_net::AttitudeMode::RadialOut => "Radial out".to_string(),
            ose_net::AttitudeMode::RadialIn => "Radial in".to_string(),
            ose_net::AttitudeMode::Free => "SAS off".to_string(),
            ose_net::AttitudeMode::Target => "Target".to_string(),
            ose_net::AttitudeMode::AntiTarget => "Anti-target".to_string(),
            other => format!("{other:?}"),
        };
        set_text(&self.doc, "fh-attitude", &att);
        // Joint stress: worst joint load over its break limit (1 breaks), as the bar's fill and colour.
        let stress = f.joint_stress.clamp(0.0, 1.0);
        if let Some(bar) = self
            .doc
            .get_element_by_id("fh-stress-fill")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            let hue = 120.0 * (1.0 - stress);
            bar.style()
                .set_property("width", &format!("{:.0}%", stress * 100.0))
                .ok();
            bar.style()
                .set_property("background", &format!("hsl({hue:.0}, 85%, 50%)"))
                .ok();
        }
        if let Some(el) = self.doc.get_element_by_id("flight-hud") {
            el.set_attribute("data-ose-stress", &format!("{:.3}", f.joint_stress))
                .ok();
            el.set_attribute("data-ose-flex", &f.flex.len().to_string())
                .ok();
        }
        set_text(
            &self.doc,
            "fh-ec",
            &format!("EC {:.0} / {:.0}", f.electric_charge, f.electric_capacity),
        );
        set_text(
            &self.doc,
            "fh-control",
            if f.has_control {
                ""
            } else {
                "NO CONTROL: the probe cores are out of electric charge"
            },
        );
        if let Some(bar) = self
            .doc
            .get_element_by_id("fh-throttle-fill")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            bar.style()
                .set_property("height", &format!("{:.0}%", f.throttle * 100.0))
                .ok();
        }
        if let Some(el) = self.doc.get_element_by_id("flight-hud") {
            el.set_attribute("data-ose-altitude", &format!("{:.1}", f.altitude_m))
                .ok();
            el.set_attribute("data-ose-situation", &format!("{:?}", f.situation))
                .ok();
        }
    }

    fn render_sessions(&self, sessions: &[SessionInfo]) {
        let Some(list) = self.doc.get_element_by_id("sessions") else {
            return;
        };
        let mut html = String::new();
        for s in sessions {
            html.push_str(&format!(
                "<li data-ose=\"session\" data-ose-id=\"{id}\"><div><div class=\"name\">{name}</div>\
                 <div class=\"meta\">{mode} \u{b7} {ut} \u{b7} {players} {pl}</div></div>\
                 <button data-ose=\"session-join\" data-ose-session=\"{id}\">Resume</button></li>",
                id = esc(&s.id),
                name = esc(&s.name),
                mode = mode_name(s.mode),
                ut = kerbin_time::format_ut(s.ut_s),
                players = s.players,
                pl = if s.players == 1 { "player" } else { "players" },
            ));
        }
        if html.is_empty() {
            html = "<li class=\"empty\">no campaigns yet; start a new one</li>".into();
        }
        list.set_inner_html(&html);
    }

    fn join_session(&mut self, id: &str) {
        set_text(&self.doc, "menu-note", &format!("joining {id}..."));
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::SelectSession { id: id.to_string() });
        }
    }

    fn render_presets(&self) {
        let Some(sel) = self.doc.get_element_by_id("create-preset") else {
            return;
        };
        let mut html = String::new();
        for (i, p) in self.difficulty_presets.iter().enumerate() {
            let selected = if p.name.eq_ignore_ascii_case("normal") {
                " selected"
            } else {
                ""
            };
            html.push_str(&format!(
                "<option value=\"{i}\"{selected}>{}</option>",
                esc(&p.name)
            ));
        }
        html.push_str("<option value=\"custom\">Custom</option>");
        sel.set_inner_html(&html);
    }

    /// One row per definition: a checkbox for Bool, a slider for Percentage; exploration rows only in
    /// Exploration mode.
    fn render_difficulty_form(&self) {
        let Some(grid) = self.doc.get_element_by_id("difficulty") else {
            return;
        };
        let mode = select_value(&self.doc, "create-mode").unwrap_or_default();
        let mut html = String::new();
        let mut section = String::new();
        for d in &self.difficulty_defs {
            if d.group == "exploration" && mode != "Exploration" {
                continue;
            }
            if d.section != section {
                section = d.section.clone();
                html.push_str(&format!("<div class=\"section\">{}</div>", esc(&section)));
            }
            let id = format!("diff-{}", esc(&d.id));
            let label = d.id.replace('_', " ");
            if d.kind == "Bool" {
                html.push_str(&format!(
                    "<span>{label}</span><input id=\"{id}\" type=\"checkbox\" data-ose-diff=\"{did}\" />",
                    did = esc(&d.id)
                ));
            } else {
                let (min, max, step) = (
                    d.min.unwrap_or(0.0),
                    d.max.unwrap_or(2.0),
                    d.step.unwrap_or(0.1),
                );
                html.push_str(&format!(
                    "<span>{label}</span><span><input id=\"{id}\" type=\"range\" min=\"{min}\" max=\"{max}\" step=\"{step}\" data-ose-diff=\"{did}\" /> <output for=\"{id}\"></output></span>",
                    did = esc(&d.id)
                ));
            }
        }
        grid.set_inner_html(&html);
        self.apply_preset();
    }

    /// Copy the chosen preset's values into the form.
    fn apply_preset(&self) {
        let Some(v) = select_value(&self.doc, "create-preset") else {
            return;
        };
        let Some(preset) = v
            .parse::<usize>()
            .ok()
            .and_then(|i| self.difficulty_presets.get(i))
        else {
            self.refresh_difficulty_outputs();
            return;
        };
        for o in &preset.options {
            let id = format!("diff-{}", o.id);
            match &o.value {
                DifficultyValue::Bool(b) => {
                    if let Some(el) = self
                        .doc
                        .get_element_by_id(&id)
                        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                    {
                        el.set_checked(*b);
                    }
                }
                DifficultyValue::Number(n) => set_input_value(&self.doc, &id, *n),
                DifficultyValue::Choice(c) => {
                    if let Some(el) = self
                        .doc
                        .get_element_by_id(&id)
                        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                    {
                        el.set_value(c);
                    }
                }
            }
        }
        self.refresh_difficulty_outputs();
    }

    fn refresh_difficulty_outputs(&self) {
        for d in &self.difficulty_defs {
            if d.kind != "Bool" {
                let id = format!("diff-{}", d.id);
                if let Some(v) = input_value(&self.doc, &id) {
                    if let Ok(Some(out)) = self.doc.query_selector(&format!("output[for=\"{id}\"]"))
                    {
                        out.set_text_content(Some(&format!("{:.0}%", v * 100.0)));
                    }
                }
            }
        }
    }

    fn difficulty_from_form(&self) -> Vec<DifficultyOption> {
        let mode = select_value(&self.doc, "create-mode").unwrap_or_default();
        self.difficulty_defs
            .iter()
            .filter(|d| d.group != "exploration" || mode == "Exploration")
            .filter_map(|d| {
                let id = format!("diff-{}", d.id);
                let value = if d.kind == "Bool" {
                    DifficultyValue::Bool(checkbox(&self.doc, &id)?)
                } else {
                    DifficultyValue::Number(input_value(&self.doc, &id)?)
                };
                Some(DifficultyOption {
                    id: d.id.clone(),
                    value,
                })
            })
            .collect()
    }

    fn create_session(&mut self) {
        let name = input_string(&self.doc, "create-name")
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "New campaign".into());
        let mode = match select_value(&self.doc, "create-mode").as_deref() {
            Some("Sandbox") => SessionMode::Sandbox,
            _ => SessionMode::Exploration,
        };
        let difficulty = self.difficulty_from_form();
        set_text(&self.doc, "menu-note", "creating...");
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::CreateSession {
                    name,
                    mode,
                    difficulty,
                },
            );
        }
    }

    fn send_agency(&mut self) {
        let flag_id = input_string(&self.doc, "agency-flag")
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "default".into());
        let primary = input_string(&self.doc, "agency-primary").unwrap_or_else(|| "#4aa3ff".into());
        let secondary =
            input_string(&self.doc, "agency-secondary").unwrap_or_else(|| "#ffd87a".into());
        store("ose.flag", &flag_id);
        store("ose.primary", &primary);
        store("ose.secondary", &secondary);
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::SetAgency {
                    agency: Agency {
                        flag_id,
                        primary_rgb: parse_rgb(&primary),
                        secondary_rgb: parse_rgb(&secondary),
                    },
                },
            );
            self.agency_sent = true;
        }
    }

    fn player_name_of(&self, id: u64) -> String {
        self.campaign
            .as_ref()
            .and_then(|(_, m)| m.iter().find(|m| m.player_id == id))
            .map(|m| m.display_name.clone())
            .unwrap_or_else(|| format!("player #{id}"))
    }

    /// "warp held by X" when another player's lower warp request wins the lowest-wins arbitration.
    fn update_warp_holder(&self) {
        let text = match self.warp_holder {
            Some(p) if Some(p) != self.player_id => {
                format!("warp held by {}", self.player_name_of(p))
            }
            _ => String::new(),
        };
        set_text(&self.doc, "warp-holder", &text);
    }

    /// Breadcrumbs: star > body > ... > vessel for what the camera looks at; rebuilt only on change.
    fn update_breadcrumbs(&mut self) {
        let Some(scene) = &self.scene else {
            return;
        };
        let (mut body, vessel) = match self.follow.and_then(|id| scene.vessel(id)) {
            Some(v) => (v.reference_body as usize, Some((v.id, v.name.clone()))),
            None => (self.camera.focus, None),
        };
        let mut chain = vec![body];
        while let Some(p) = scene.bodies[body].parent {
            body = p as usize;
            chain.push(body);
            if chain.len() > 32 {
                break;
            }
        }
        chain.reverse();
        let mut html = String::new();
        for (k, i) in chain.iter().enumerate() {
            if k > 0 {
                html.push_str(" \u{203a} ");
            }
            html.push_str(&format!(
                "<span data-ose-index=\"{i}\">{}</span>",
                scene.bodies[*i].id
            ));
        }
        if let Some((id, name)) = vessel {
            html.push_str(&format!(
                " \u{203a} <span data-ose-vessel=\"{id}\">{}</span>",
                name.replace('<', "&lt;")
            ));
        }
        if html != self.crumbs {
            if let Some(el) = self.doc.get_element_by_id("crumbs") {
                el.set_inner_html(&html);
            }
            self.crumbs = html;
        }
    }

    /// The player's own vessel, if the server has sent one.
    /// The vessel to plan for: the one followed if it is yours, else one of yours in orbit (on rails), else
    /// any of yours. A craft standing on the pad has no orbit to put a node on.
    fn my_vessel(&self) -> Option<(u64, usize)> {
        let scene = self.scene.as_ref()?;
        let mine: Vec<&ose_net::VesselState> = scene
            .vessels
            .iter()
            .filter(|v| v.owner.is_some() && v.owner == self.player_id)
            .collect();
        // The vessel being flown first: nodes and the orbit pick belong to it.
        mine.iter()
            .find(|v| Some(v.id) == self.flying)
            .or_else(|| mine.iter().find(|v| Some(v.id) == self.follow))
            .or_else(|| mine.iter().find(|v| v.flight.is_none()))
            .or_else(|| mine.first())
            .map(|v| (v.id, v.reference_body as usize))
    }

    fn add_node(&mut self) {
        let Some((vessel, rb)) = self.my_vessel() else {
            set_text(&self.doc, "node-reason", "no vessel to plan for yet");
            return;
        };
        self.ftue("FTUE_First_Maneuver");
        self.lesson_event("ManeuverCreatedMessage");
        self.lesson_event("OrbitClickedMessage");
        let now = self.time.ut_at(now_ms());
        let dt = 120.0;
        self.node = Some(Node {
            vessel,
            anchor_ut: now,
            ut: now + dt,
            prograde_mps: 0.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        });
        set_input_value(&self.doc, "node-dt", dt);
        set_input_value(&self.doc, "node-prograde", 0.0);
        set_input_value(&self.doc, "node-normal", 0.0);
        set_input_value(&self.doc, "node-radial", 0.0);
        // Let the time slider span one orbit of the vessel.
        if let Some(scene) = &self.scene {
            if let Some(v) = scene.vessel(vessel) {
                let period = scene::orbit_from(scene.bodies[rb].mu, &v.orbit).period;
                if period.is_finite() {
                    if let Some(el) = self
                        .doc
                        .get_element_by_id("node-dt")
                        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                    {
                        el.set_max(&format!("{:.0}", period.max(600.0)));
                    }
                }
            }
        }
        if let Some(panel) = self.doc.get_element_by_id("node-panel") {
            panel.class_list().add_1("open").ok();
        }
        if self.node_el.is_none() {
            if let Some(el) = self
                .doc
                .create_element("div")
                .ok()
                .and_then(|e| e.dyn_into::<HtmlElement>().ok())
            {
                el.set_class_name("node");
                el.set_attribute("data-ose", "maneuver-node").ok();
                el.set_attribute("title", "maneuver node").ok();
                self.labels.append_child(&el).ok();
                self.node_el = Some(el);
            }
        }
        self.follow_vessel(vessel, rb);
        self.send_node();
    }

    /// A left press on a maneuver handle (the handles sit in #labels, which takes no pointer events, so the
    /// press is matched by position): start dragging it. True when a handle took the press.
    fn mnv_press(&mut self, x: f64, y: f64) -> bool {
        let Some(node) = self.node else { return false };
        let hit = (0..6).find(|k| {
            self.doc
                .get_element_by_id(&format!(
                    "mnv-{}",
                    [
                        "prograde",
                        "retrograde",
                        "normal",
                        "antinormal",
                        "radialout",
                        "radialin"
                    ][*k]
                ))
                .filter(|e| {
                    e.dyn_ref::<HtmlElement>()
                        .map(|h| {
                            h.style().get_property_value("visibility").ok().as_deref()
                                == Some("visible")
                        })
                        .unwrap_or(false)
                })
                .map(|e| {
                    let r = e.get_bounding_client_rect();
                    let (cx, cy) = (r.left() + r.width() * 0.5, r.top() + r.height() * 0.5);
                    (cx - x).powi(2) + (cy - y).powi(2) < 13.0 * 13.0
                })
                .unwrap_or(false)
        });
        let Some(k) = hit else { return false };
        self.mnv_drag = Some((
            k,
            x,
            y,
            [node.prograde_mps, node.normal_mps, node.radial_mps],
        ));
        true
    }

    /// Dragging a handle outward adds delta-v along it: 1 m/s per pixel, 0.1 with Shift.
    fn mnv_drag_to(&mut self, x: f64, y: f64, fine: bool) {
        let Some((k, x0, y0, start)) = self.mnv_drag else {
            return;
        };
        if k == 6 {
            self.mnv_slide(x, y);
            return;
        }
        let (ux, uy) = self.mnv_dirs[k];
        let along = (x - x0) * ux + (y - y0) * uy;
        let dv = along * if fine { 0.1 } else { 1.0 };
        let (axis, sign) = (k / 2, if k % 2 == 0 { 1.0 } else { -1.0 });
        let Some(node) = self.node.as_mut() else {
            return;
        };
        let value = (start[axis] + sign * dv).clamp(-5000.0, 5000.0);
        match axis {
            0 => node.prograde_mps = value,
            1 => node.normal_mps = value,
            _ => node.radial_mps = value,
        }
        let (id, v) = (["node-prograde", "node-normal", "node-radial"][axis], value);
        if let Some(el) = self
            .doc
            .get_element_by_id(id)
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            let max: f64 = el.max().parse().unwrap_or(1500.0);
            if v.abs() > max {
                el.set_max(&format!("{:.0}", v.abs().ceil()));
                el.set_min(&format!("{:.0}", -v.abs().ceil()));
            }
        }
        set_input_value(&self.doc, id, v);
        set_text(&self.doc, &format!("{id}-text"), &format!("{v:.1} m/s"));
        let now = now_ms();
        if now - self.mnv_sent_ms > 100.0 {
            self.mnv_sent_ms = now;
            self.send_node();
        }
    }

    /// Slide the node along the orbit to the sampled point nearest the pointer (screen px).
    fn mnv_slide(&mut self, x: f64, y: f64) {
        let rect = self.gpu_canvas.get_bounding_client_rect();
        let (x, y) = (x - rect.left(), y - rect.top());
        let Some((_, t)) = self
            .orbit_pick
            .iter()
            .map(|(px, py, t)| ((px - x).powi(2) + (py - y).powi(2), *t))
            .min_by(|a, b| a.0.total_cmp(&b.0))
        else {
            return;
        };
        let Some(node) = self.node.as_mut() else {
            return;
        };
        node.ut = t;
        let dt = t - node.anchor_ut;
        if let Some(el) = self
            .doc
            .get_element_by_id("node-dt")
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            let max: f64 = el.max().parse().unwrap_or(600.0);
            if dt > max {
                el.set_max(&format!("{:.0}", dt.ceil()));
            }
        }
        set_input_value(&self.doc, "node-dt", dt);
        let now = now_ms();
        if now - self.mnv_sent_ms > 100.0 {
            self.mnv_sent_ms = now;
            self.send_node();
        }
    }

    /// A click on the map near the planning vessel's orbit puts a maneuver node there (CSS px).
    fn map_click(&mut self, x: f64, y: f64) -> bool {
        let best = self
            .orbit_pick
            .iter()
            .map(|(px, py, t)| ((px - x).powi(2) + (py - y).powi(2), *t))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((d2, t)) = best else { return false };
        if d2 > 14.0 * 14.0 {
            return false;
        }
        // Keep the view where it is: add_node frames the vessel's orbit.
        let (dist, focus, offset) = (self.orbit_dist_km, self.camera.focus, self.camera.offset);
        let m_per_px = self.camera.m_per_px;
        self.add_node();
        (self.orbit_dist_km, self.camera.focus, self.camera.offset) = (dist, focus, offset);
        self.camera.m_per_px = m_per_px;
        if let Some(node) = self.node.as_mut() {
            node.anchor_ut = self.time.ut_at(now_ms());
            node.ut = t;
            let dt = t - node.anchor_ut;
            if let Some(el) = self
                .doc
                .get_element_by_id("node-dt")
                .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                let max: f64 = el.max().parse().unwrap_or(600.0);
                if dt > max {
                    el.set_max(&format!("{:.0}", dt.ceil()));
                }
            }
            set_input_value(&self.doc, "node-dt", dt);
        }
        self.send_node();
        true
    }

    fn clear_node(&mut self) {
        if let (Some(node), Some(ws)) = (self.node.take(), &self.ws) {
            send(
                ws,
                &ClientMessage::ClearBurn {
                    vessel: node.vessel,
                },
            );
        }
        if let Some(el) = self.node_el.take() {
            el.remove();
        }
        if let Some(panel) = self.doc.get_element_by_id("node-panel") {
            panel.class_list().remove_1("open").ok();
        }
        set_text(&self.doc, "node-reason", "");
    }

    fn node_inputs_changed(&mut self) {
        let Some(mut node) = self.node else {
            return;
        };
        if let Some(dt) = input_value(&self.doc, "node-dt") {
            node.ut = node.anchor_ut + dt;
        }
        if let Some(p) = input_value(&self.doc, "node-prograde") {
            node.prograde_mps = p;
        }
        if let Some(n) = input_value(&self.doc, "node-normal") {
            node.normal_mps = n;
        }
        if let Some(r) = input_value(&self.doc, "node-radial") {
            node.radial_mps = r;
        }
        self.node = Some(node);
        self.send_node();
    }

    fn send_node(&mut self) {
        let Some(node) = self.node else {
            return;
        };
        if let Some(ws) = &self.ws {
            send(
                ws,
                &ClientMessage::ScheduleBurn {
                    vessel: node.vessel,
                    ut: node.ut,
                    prograde_mps: node.prograde_mps,
                    normal_mps: node.normal_mps,
                    radial_mps: node.radial_mps,
                },
            );
        }
        set_text(
            &self.doc,
            "node-prograde-text",
            &format!("{:.0} m/s", node.prograde_mps),
        );
        set_text(
            &self.doc,
            "node-normal-text",
            &format!("{:.0} m/s", node.normal_mps),
        );
        set_text(
            &self.doc,
            "node-radial-text",
            &format!("{:.0} m/s", node.radial_mps),
        );
        let dv =
            (node.prograde_mps.powi(2) + node.normal_mps.powi(2) + node.radial_mps.powi(2)).sqrt();
        if let Some(el) = &self.node_el {
            el.set_attribute("data-ose-dv", &format!("{dv:.1}")).ok();
        }
        if let Some(el) = &self.node_el {
            el.set_attribute("data-ose-ut", &format!("{:.1}", node.ut))
                .ok();
            el.set_attribute("data-ose-prograde", &format!("{:.1}", node.prograde_mps))
                .ok();
        }
    }

    /// Place the node marker on the vessel's current orbit at the burn time and refresh its countdown.
    fn draw_node(&self, ut: f64, positions: &[ose_math::DVec3], centre: ose_math::DVec3) {
        let (Some(node), Some(el), Some(scene)) = (self.node, &self.node_el, &self.scene) else {
            return;
        };
        let Some(v) = scene.vessel(node.vessel) else {
            return;
        };
        let rb = v.reference_body as usize;
        let rel = scene::orbit_from(scene.bodies[rb].mu, &v.orbit).position_at(node.ut);
        let (x, y) = self.camera.to_screen(positions[rb] + rel, centre);
        let style = el.style();
        style.set_property("left", &format!("{x:.1}px")).ok();
        style.set_property("top", &format!("{y:.1}px")).ok();
        let remaining = node.ut - ut;
        let sign = if remaining < 0.0 { "+" } else { "-" };
        let r = remaining.abs();
        set_text(
            &self.doc,
            "node-dt-text",
            &format!(
                "T{sign}{}:{:02}",
                (r / 60.0).floor() as u64,
                (r % 60.0) as u64
            ),
        );
        let ctx = &self.ctx;
        ctx.set_stroke_style_str("rgba(150,170,255,0.9)");
        ctx.set_line_width(1.0);
        ctx.begin_path();
        ctx.arc(x, y, 9.0, 0.0, std::f64::consts::TAU).ok();
        ctx.stroke();
    }

    fn vessel_click_target(ev: &MouseEvent) -> Option<u64> {
        let t: Element = ev.target()?.dyn_into().ok()?;
        t.get_attribute("data-ose-vessel")?.parse().ok()
    }

    fn label_click_target(ev: &MouseEvent) -> Option<usize> {
        let t: Element = ev.target()?.dyn_into().ok()?;
        t.get_attribute("data-ose-index")?.parse().ok()
    }

    fn update_hud(&self, ut: f64) {
        self.ut_el
            .set_text_content(Some(&format!("UT {}", kerbin_time::format_ut(ut))));
        self.warp_el
            .set_text_content(Some(&format!("\u{d7}{}", self.time.warp)));
    }

    fn frame(&mut self) {
        let now = now_ms();
        let ut = self.time.ut_at(now);
        self.update_tracking(now);
        self.update_audio(now);
        if self.time.have_time {
            self.update_hud(ut);
        }
        self.update_flight_hud();
        self.update_flight_instruments();
        self.toasts.watch(&self.doc, "fh-note", "alert");
        self.flight_keys_tick(now);
        // FPS counter, refreshed twice a second while shown.
        self.fps_frames += 1;
        if now - self.fps_since_ms >= 500.0 {
            let fps = self.fps_frames as f64 * 1000.0 / (now - self.fps_since_ms).max(1.0);
            self.fps_frames = 0;
            self.fps_since_ms = now;
            set_text(&self.doc, "fps", &format!("{fps:.0} FPS"));
        }
        self.warp_to_tick(now);
        self.lesson_tick();
        if self.in_session && self.vab_active() {
            self.vab_report_tick(now);
        }
        if self.flight_pam.is_some() {
            if self.flying.is_none() {
                self.flight_open_pam(None, 0.0, 0.0);
            } else {
                self.flight_pam_refresh();
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let (Some(p), Some(gpu)) = (self.planet_preview.as_mut(), self.gpu.as_mut()) {
            if let Some(menu) = self.doc.get_element_by_id("menu") {
                menu.class_list().add_1("hidden").ok();
            }
            if let Some(gc) = self.doc.get_element_by_id("gpu") {
                gc.set_attribute("data-ose-planet-preview", &p.body).ok();
            }
            p.frame(gpu, now);
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if self.flight_active() && self.frame_flight(ut) {
            return;
        }
        let dpr = self.dpr;
        let (w, h) = (self.camera.width, self.camera.height);
        let ctx = &self.ctx;
        ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0).ok();
        ctx.set_fill_style_str("#06080f");
        ctx.fill_rect(0.0, 0.0, w, h);

        // The KSC hub screen shows the campus in 3D; elsewhere its tooltip must not linger.
        #[cfg(target_arch = "wasm32")]
        if !(self.in_session && self.ksc_active()) && self.ksc3d.hovered.is_some() {
            self.ksc3d.hovered = None;
            self.place_ksc_tooltip();
        }
        #[cfg(target_arch = "wasm32")]
        if self.in_session && self.vab_active() {
            let empty = self.editor.is_empty();
            self.vab3d.com = self.vab_com.filter(|_| self.vab_show_com && !empty);
            self.vab3d.cot = self.vab_cot.filter(|_| self.vab_show_cot && !empty);
            self.vab3d.col = self.vab_col.filter(|_| self.vab_show_col && !empty);
            self.vab3d.ghosts = self
                .vab3d
                .held
                .as_ref()
                .map(|(id, _)| id.clone())
                .and_then(|id| {
                    let (spot, sym) = self.vab_surface(&id)?;
                    let spec = self.catalog.get(&id)?;
                    Some(self.editor.surface_poses(spec, &self.catalog, spot, sym))
                })
                .unwrap_or_default();
            let held = self.vab3d.held.as_ref().map(|(id, _)| id.clone());
            let free = self.snap_nodes(held.as_deref());
            if let Some(gpu) = &mut self.gpu {
                let was_ready = self.vab3d.is_ready();
                if self
                    .vab3d
                    .frame(gpu, &self.editor.craft, self.editor.selected, &free)
                    && !was_ready
                {
                    self.gpu_canvas
                        .set_attribute("data-ose-vab-scene", "ready")
                        .ok();
                }
                return;
            }
        }
        #[cfg(target_arch = "wasm32")]
        if self.in_session && (self.ksc_active() || self.launchpad_active()) {
            let on_pad = self.launchpad_active();
            if on_pad != self.ksc_on_pad {
                self.ksc_on_pad = on_pad;
                self.ksc3d
                    .focus(if on_pad { Some("Launchpad_1") } else { None });
            }
            if let Some(gpu) = &mut self.gpu {
                let was_ready = self.ksc3d.is_ready();
                // The real sun over the KSC at the server's time.
                let sun = self.scene.as_ref().and_then(|scene| {
                    let k = scene.index_of("Kerbin")?;
                    let star = scene.root()?;
                    let ut = self.time.ut_at(now_ms());
                    let d = scene.position(star, ut) - scene.position(k, ut);
                    let spin = self.body_spins.get(k).map(|b| flight3d::Spin {
                        period_s: b.rotation_period_s,
                        initial_deg: b.initial_rotation_deg,
                    });
                    self.flight3d.ksc_sun(
                        scene.bodies[k].radius_m,
                        glam::DVec3::new(d.x, d.y, d.z),
                        spin,
                        ut,
                    )
                });
                if self.ksc3d.frame(gpu, sun) {
                    if !was_ready {
                        self.gpu_canvas
                            .set_attribute("data-ose-ksc-scene", "ready")
                            .ok();
                    }
                    self.place_ksc_tooltip();
                }
                return;
            }
        }
        // Before a session is joined the 3D view shows the menu backdrop.
        #[cfg(target_arch = "wasm32")]
        if !self.in_session {
            if let Some(gpu) = &mut self.gpu {
                let was_ready = self.menu3d.is_ready();
                if self.menu3d.frame(gpu, now) && !was_ready {
                    if let Some(v) = self.menu3d.vessel_name() {
                        self.gpu_canvas
                            .set_attribute("data-ose-menu-vessel", v)
                            .ok();
                    }
                }
                return;
            }
        }
        let Some(scene) = &self.scene else {
            return;
        };
        let cam = self.camera;
        let positions: Vec<_> = (0..scene.len()).map(|i| scene.position(i, ut)).collect();
        let followed = self
            .follow
            .and_then(|id| scene.vessel(id))
            .map(|v| scene.vessel_position(v, ut));
        let centre = followed.unwrap_or(positions[cam.focus]);
        #[cfg(target_arch = "wasm32")]
        if self.is_3d() {
            self.frame_3d(ut, &positions, centre);
            return;
        }
        // Exposed for tests and debugging: current scale and the star-space position of the focus body.
        self.canvas
            .set_attribute("data-ose-scale", &format!("{:.3e}", cam.m_per_px))
            .ok();
        self.canvas
            .set_attribute(
                "data-ose-focus",
                &format!(
                    "{} {:.3e} {:.3e}",
                    scene.bodies[cam.focus].id, centre.x, centre.y
                ),
            )
            .ok();

        // Orbits: each body's path around its parent's current position.
        ctx.set_line_width(1.0);
        for i in 0..scene.len() {
            let (Some(orbit), Some(p)) = (scene.orbit(i), scene.bodies[i].parent) else {
                continue;
            };
            let parent_pos = positions[p as usize];
            let r_px = orbit.semi_major_axis().abs() / cam.m_per_px;
            if r_px < 2.0 || r_px > 1.0e5 {
                continue;
            }
            let alpha = if i == cam.focus { 0.9 } else { 0.45 };
            ctx.set_stroke_style_str(&format!("rgba(120,150,210,{alpha})"));
            ctx.begin_path();
            let n = 180;
            for k in 0..=n {
                let nu = k as f64 / n as f64 * std::f64::consts::TAU;
                let (rel, _) = orbit.state_at_true_anomaly(nu);
                let (x, y) = cam.to_screen(parent_pos + rel, centre);
                if k == 0 {
                    ctx.move_to(x, y);
                } else {
                    ctx.line_to(x, y);
                }
            }
            ctx.stroke();
        }

        // Bodies and their labels.
        for (i, b) in scene.bodies.iter().enumerate() {
            // A moon whose whole orbit is a few pixels wide is hidden inside its parent at this zoom.
            let orbit_px = scene
                .orbit(i)
                .map(|o| o.semi_major_axis().abs() / cam.m_per_px)
                .unwrap_or(f64::INFINITY);
            let shown = orbit_px >= 6.0 || i == cam.focus;
            if let Some(el) = self.label_els.get(i) {
                if !shown {
                    el.style().set_property("visibility", "hidden").ok();
                }
            }
            if !shown {
                continue;
            }
            let (x, y) = cam.to_screen(positions[i], centre);
            let r_px = (b.radius_m / cam.m_per_px).max(if b.is_star { 6.0 } else { 3.0 });
            let colour = body_colour(&b.id, b.is_star);
            if let Some(soi) = b.soi_radius_m {
                let soi_px = soi / cam.m_per_px;
                if soi_px > r_px + 4.0 && soi_px < 1.0e5 {
                    ctx.set_stroke_style_str("rgba(90,110,150,0.25)");
                    ctx.begin_path();
                    ctx.arc(x, y, soi_px, 0.0, std::f64::consts::TAU).ok();
                    ctx.stroke();
                }
            }
            ctx.set_fill_style_str(colour);
            ctx.begin_path();
            ctx.arc(x, y, r_px.min(1.0e5), 0.0, std::f64::consts::TAU)
                .ok();
            ctx.fill();

            if let Some(el) = self.label_els.get(i) {
                let visible = x > -200.0 && x < w + 200.0 && y > -50.0 && y < h + 50.0;
                let style = el.style();
                style
                    .set_property("left", &format!("{:.1}px", x + r_px.min(40.0)))
                    .ok();
                style.set_property("top", &format!("{y:.1}px")).ok();
                style
                    .set_property("visibility", if visible { "visible" } else { "hidden" })
                    .ok();
                let cls = el.class_list();
                if i == cam.focus {
                    cls.add_1("focus").ok();
                } else {
                    cls.remove_1("focus").ok();
                }
            }
        }
        self.draw_vessels(ut, &positions, centre);
        self.draw_node(ut, &positions, centre);
        self.update_breadcrumbs();
    }

    /// The 3D map: planets from the exported GLBs at true scale, orbit lines, trajectories, and the DOM
    /// labels projected through the same camera. Kilometres, relative to the camera.
    #[cfg(target_arch = "wasm32")]
    /// One frame of the flight view; false when it cannot draw (no renderer, vessel unknown yet).
    #[cfg(target_arch = "wasm32")]
    fn frame_flight(&mut self, ut: f64) -> bool {
        let Some(id) = self.flying else { return false };
        let Some(scene) = &self.scene else {
            return false;
        };
        let Some(v) = scene.vessel(id) else {
            return false;
        };
        let target_dir = self.target_dir().map(|d| glam::DVec3::new(d.x, d.y, d.z));
        let Some(gpu) = self.gpu.as_mut() else {
            return false;
        };
        let rb = v.reference_body as usize;
        let Some(body) = scene.bodies.get(rb) else {
            return false;
        };
        let vp = scene.vessel_position(v, ut);
        let bp = scene.position(rb, ut);
        let dt = 0.5;
        let vv =
            (scene.vessel_position(v, ut + dt) - scene.vessel_position(v, ut - dt)) * (0.5 / dt);
        let bv = (scene.position(rb, ut + dt) - scene.position(rb, ut - dt)) * (0.5 / dt);
        let star = scene
            .root()
            .map(|i| scene.position(i, ut))
            .unwrap_or_default();
        let g = |d: ose_math::DVec3| glam::DVec3::new(d.x, d.y, d.z);
        // Other bodies (not the star, not the body flown over) for eclipses in the planet renderer.
        let root = scene.root();
        let others: Vec<(glam::DVec3, f64)> = scene
            .bodies
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != rb && Some(*i) != root)
            .map(|(i, b)| (g(scene.position(i, ut)), b.radius_m))
            .collect();
        let star_radius = root
            .and_then(|i| scene.bodies.get(i))
            .map(|b| b.radius_m)
            .unwrap_or(0.0);
        let catalog = &self.catalog;
        let is_dec = |pid: &str| catalog.get(pid).map(|p| p.is_decoupler).unwrap_or(false);
        let nearby_vessels: Vec<(glam::DVec3, flight3d::Stand)> = scene
            .vessels
            .iter()
            .filter(|o| o.id != id && o.reference_body == v.reference_body)
            .map(|o| (g(scene.vessel_position(o, ut)), stand_of(&self.roster, o)))
            .collect();
        let inp = flight3d::FlightInput {
            vessel: v,
            attitude: scene.vessel_attitude(id, now_ms()),
            nearby_vessels: &nearby_vessels,
            stand: stand_of(&self.roster, v),
            vessel_pos: g(vp),
            vessel_vel: g(vv),
            body_pos: g(bp),
            body_vel: g(bv),
            star_pos: g(star),
            body_id: &body.id,
            body_radius: body.radius_m,
            atmosphere_m: if body.has_atmosphere {
                body.atmosphere_depth_m
            } else {
                0.0
            },
            ut,
            craft: self.flight_craft.as_ref(),
            is_decoupler: &is_dec,
            maneuver: self
                .node
                .as_ref()
                .filter(|n| n.vessel == id)
                .map(|n| (n.prograde_mps, n.normal_mps, n.radial_mps)),
            target: target_dir,
            now_ms: now_ms(),
            others: &others,
            star_radius,
            spin: self.body_spins.get(rb).map(|b| flight3d::Spin {
                period_s: b.rotation_period_s,
                initial_deg: b.initial_rotation_deg,
            }),
        };
        let drawn = self.flight3d.frame(gpu, &inp);
        if drawn {
            if let Some(hud) = self.doc.query_selector("[data-ose=flight]").ok().flatten() {
                hud.set_attribute(
                    "data-ose-particles",
                    &gpu.engine_particle_count().to_string(),
                )
                .ok();
                hud.set_attribute(
                    "data-ose-engine-fx-layers",
                    &gpu.engine_fx_layers().to_string(),
                )
                .ok();
            }
            if let (Some(nav), Some(canvas)) = (
                self.flight3d.nav,
                self.doc
                    .get_element_by_id("fh-navball")
                    .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok()),
            ) {
                if self.navball.draw(&canvas, &nav) {
                    canvas.set_attribute("data-ose-navball", "drawn").ok();
                }
            }
        }
        drawn
    }

    #[cfg(target_arch = "wasm32")]
    fn frame_3d(&mut self, ut: f64, positions: &[ose_math::DVec3], centre: ose_math::DVec3) {
        use glam::{DQuat, DVec3 as GV};
        let planner = self.my_vessel().map(|(id, _)| id);
        let target = match self.target {
            Some(ose_net::TargetId::Vessel(id)) => Some(id),
            _ => None,
        };
        let Some(scene) = &self.scene else {
            return;
        };
        let to_km = |p: ose_math::DVec3| GV::new(p.x / 1000.0, p.z / 1000.0, -p.y / 1000.0); // Z-up -> Y-up
        let centre_km = to_km(centre);
        let dir = GV::new(
            self.orbit_pitch.cos() * self.orbit_yaw.sin(),
            self.orbit_pitch.sin(),
            self.orbit_pitch.cos() * self.orbit_yaw.cos(),
        );
        let cam_km = centre_km + dir * self.orbit_dist_km;
        let forward = -dir;
        let near = (self.orbit_dist_km * 1.0e-3).max(1.0e-3);
        let far = (self.orbit_dist_km * 2.0e4).max(1.0e4);
        let gpu = self.gpu.as_mut().unwrap();
        gpu.set_camera(forward, GV::Y, 50f64.to_radians(), near, far);

        // Ask for every body model once; draw the ones that have arrived.
        let star = scene.root();
        let star_km = star.map(|i| to_km(positions[i])).unwrap_or(GV::ZERO);
        let mut items: Vec<render::DrawItem> = Vec::new();
        let mut lines: Vec<render::LineItem> = Vec::new();
        for (i, b) in scene.bodies.iter().enumerate() {
            let name = format!("body:{}", b.id);
            gpu.request_model(&name, &format!("assets/bodies/{}.glb", b.id));
            let pos_km = to_km(positions[i]);
            if let Some(model) = gpu.model(&name) {
                let scale = (b.radius_m / 1000.0) / model.mesh_radius.max(1e-9);
                let light = if b.is_star { GV::Y } else { star_km - pos_km };
                items.push(render::DrawItem {
                    animation: None,
                    node_turns: Vec::new(),
                    model: name.clone(),
                    rel_pos_km: pos_km - cam_km,
                    rotation: DQuat::IDENTITY,
                    scale,
                    light_dir: light,
                    emissive: if b.is_star { 1.0 } else { 0.0 },
                    color: if b.is_star {
                        [1.0, 0.95, 0.75, 1.0]
                    } else {
                        [1.0; 4]
                    },
                    fill_dir: GV::ZERO,
                    fill: 0.0,
                    ambient: 0.0,
                    atmosphere: false,
                    mirror_z: true,
                    sun: 1.0,
                    glow: 0.0,
                });
            }
            if let (Some(orbit), Some(p)) = (scene.orbit(i), b.parent) {
                let n = 256;
                let pts: Vec<GV> = (0..=n)
                    .map(|k| {
                        let nu = k as f64 / n as f64 * std::f64::consts::TAU;
                        let (rel, _) = orbit.state_at_true_anomaly(nu);
                        to_km(rel)
                    })
                    .collect();
                let origin = to_km(positions[p as usize]) - cam_km;
                let focus = i == self.camera.focus;
                lines.push(render::LineItem {
                    key: format!("orbit:{}", b.id),
                    points_km: pts,
                    rel_origin_km: origin,
                    color: if focus {
                        [0.55, 0.65, 0.95, 0.9]
                    } else {
                        [0.45, 0.55, 0.8, 0.45]
                    },
                });
            }
        }
        // Vessel trajectories: one strip per patch, keyed by the prediction version.
        let mut traj: Vec<render::LineItem> = Vec::new();
        for v in &scene.vessels {
            let mine = v.owner.is_some() && v.owner == self.player_id;
            if let Some(patches) = scene.trajectories.get(&v.id) {
                for (pi, patch) in patches.iter().enumerate() {
                    let Some(body) = scene.bodies.get(patch.body as usize) else {
                        continue;
                    };
                    let orbit = scene::orbit_from(body.mu, &patch.orbit);
                    let end_ut = if patch.end_ut.is_finite() {
                        patch.end_ut
                    } else if orbit.period.is_finite() {
                        patch.start_ut + orbit.period
                    } else {
                        patch.start_ut + 6.0 * 3600.0
                    };
                    let soi = body.soi_radius_m.unwrap_or(f64::INFINITY);
                    let n = 240;
                    let mut pts = Vec::with_capacity(n + 1);
                    for s in 0..=n {
                        let t = patch.start_ut + (end_ut - patch.start_ut) * s as f64 / n as f64;
                        let rel = orbit.position_at(t);
                        if rel.length() > soi * 1.02 {
                            break;
                        }
                        pts.push(to_km(rel));
                    }
                    let colour = match patch.end {
                        PatchEnd::Final => [0.47, 0.86, 0.63, 0.95],
                        PatchEnd::Encounter => [0.94, 0.78, 0.35, 0.95],
                        PatchEnd::Escape => [0.94, 0.51, 0.35, 0.95],
                        PatchEnd::Collision => [0.94, 0.35, 0.35, 0.95],
                        PatchEnd::Maneuver => [0.59, 0.67, 1.0, 0.95],
                        #[allow(unreachable_patterns)]
                        _ => [0.8, 0.8, 0.8, 0.7],
                    };
                    let _ = mine;
                    traj.push(render::LineItem {
                        key: format!("traj:{}:{}:{}", v.id, pi, self.traj_version),
                        points_km: pts,
                        rel_origin_km: to_km(positions[patch.body as usize]) - cam_km,
                        color: colour,
                    });
                }
            }
        }
        lines.extend(traj);
        gpu.frame("assets/bodies/", &items, &lines, [0.024, 0.031, 0.059]);

        // Labels and markers through the 3D camera.
        let (w, h) = (self.camera.width, self.camera.height);
        let dpr = self.dpr;
        let project = |p: GV| gpu.project(p - cam_km).map(|(x, y)| (x / dpr, y / dpr));
        for (i, b) in scene.bodies.iter().enumerate() {
            let Some(el) = self.label_els.get(i) else {
                continue;
            };
            let style = el.style();
            let orbit_px = scene
                .orbit(i)
                .map(|o| o.semi_major_axis().abs() / 1000.0 / self.orbit_dist_km * h)
                .unwrap_or(f64::INFINITY);
            let shown = orbit_px >= 6.0 || i == self.camera.focus;
            match project(to_km(positions[i])) {
                Some((x, y))
                    if shown && x > -200.0 && x < w + 200.0 && y > -50.0 && y < h + 50.0 =>
                {
                    let r_px =
                        (b.radius_m / 1000.0 / self.orbit_dist_km * h * 0.6).clamp(3.0, 40.0);
                    style
                        .set_property("left", &format!("{:.1}px", x + r_px))
                        .ok();
                    style.set_property("top", &format!("{y:.1}px")).ok();
                    style.set_property("visibility", "visible").ok();
                }
                _ => {
                    style.set_property("visibility", "hidden").ok();
                }
            }
            let cls = el.class_list();
            if i == self.camera.focus {
                cls.add_1("focus").ok();
            } else {
                cls.remove_1("focus").ok();
            }
        }
        for v in &scene.vessels {
            if let Some((_, el)) = self.vessel_els.iter().find(|(id, _)| *id == v.id) {
                let style = el.style();
                match project(to_km(scene.vessel_position(v, ut))) {
                    Some((x, y)) => {
                        style
                            .set_property("left", &format!("{:.1}px", x + 8.0))
                            .ok();
                        style.set_property("top", &format!("{y:.1}px")).ok();
                        style.set_property("visibility", "visible").ok();
                    }
                    None => {
                        style.set_property("visibility", "hidden").ok();
                    }
                }
            }
        }
        // Pick points along the planning vessel's first patch (one orbit ahead at most).
        self.orbit_pick.clear();
        if let Some(v) = planner.and_then(|id| scene.vessel(id)) {
            let (bi, elements, start, end) =
                match scene.trajectories.get(&v.id).and_then(|p| p.first()) {
                    Some(p) => (
                        p.body as usize,
                        p.orbit,
                        p.start_ut.max(ut),
                        if p.end == PatchEnd::Maneuver {
                            f64::INFINITY
                        } else {
                            p.end_ut
                        },
                    ),
                    None => (v.reference_body as usize, v.orbit, ut, f64::INFINITY),
                };
            if let Some(body) = scene.bodies.get(bi) {
                let orbit = scene::orbit_from(body.mu, &elements);
                let span = if orbit.period.is_finite() {
                    orbit.period.min(end - start)
                } else {
                    (end - start).min(6.0 * 3600.0)
                };
                if span.is_finite() && span > 0.0 {
                    for k in 1..=360 {
                        let t = start + span * k as f64 / 360.0;
                        if let Some((x, y)) = project(to_km(positions[bi] + orbit.position_at(t))) {
                            self.orbit_pick.push((x, y, t));
                        }
                    }
                }
            }
        }
        // Closest approach to the target over the next orbit (the original's ClosestApproach event), when both
        // go around the same body: a marker on each orbit with the separation and the time.
        let approach = (|| {
            let me = scene.vessel(planner?)?;
            let t = scene.vessel(target?)?;
            let (bi, elements, end) = match scene.trajectories.get(&me.id).and_then(|p| p.first()) {
                Some(p) => (p.body as usize, p.orbit, p.end_ut),
                None => (me.reference_body as usize, me.orbit, f64::INFINITY),
            };
            if t.reference_body as usize != bi {
                return None;
            }
            let body = scene.bodies.get(bi)?;
            let mine = scene::orbit_from(body.mu, &elements);
            let theirs = scene::orbit_from(body.mu, &t.orbit);
            let span = if mine.period.is_finite() {
                mine.period.min(end - ut)
            } else {
                (end - ut).min(6.0 * 3600.0)
            };
            if !(span.is_finite() && span > 0.0) {
                return None;
            }
            let mut best = (f64::INFINITY, ut);
            for k in 0..=720 {
                let when = ut + span * k as f64 / 720.0;
                let d = (mine.position_at(when) - theirs.position_at(when)).length();
                if d < best.0 {
                    best = (d, when);
                }
            }
            let (d, when) = best;
            Some((
                to_km(positions[bi] + mine.position_at(when)),
                to_km(positions[bi] + theirs.position_at(when)),
                format!("Closest {} {}", hud::fmt_dist(d), hud::fmt_dt(when - ut)),
            ))
        })();
        for (k, id) in ["map-ca-me", "map-ca-target"].iter().enumerate() {
            let el = match self.doc.get_element_by_id(id) {
                Some(e) => e,
                None => {
                    let Ok(e) = self.doc.create_element("div") else {
                        continue;
                    };
                    e.set_id(id);
                    e.set_class_name("apsis target");
                    e.set_attribute("data-ose", "closest-approach").ok();
                    self.labels.append_child(&e).ok();
                    e
                }
            };
            let Some(el) = el.dyn_ref::<HtmlElement>() else {
                continue;
            };
            let style = el.style();
            let at = approach.as_ref().and_then(|(a, b, text)| {
                project(if k == 0 { *a } else { *b }).map(|xy| (xy, text))
            });
            match at {
                Some(((x, y), text)) => {
                    style.set_property("left", &format!("{x:.1}px")).ok();
                    style.set_property("top", &format!("{y:.1}px")).ok();
                    style.set_property("visibility", "visible").ok();
                    let text = if k == 0 {
                        text.clone()
                    } else {
                        "Target here".to_string()
                    };
                    if el.text_content().as_deref() != Some(text.as_str()) {
                        el.set_text_content(Some(&text));
                    }
                }
                None => {
                    style.set_property("visibility", "hidden").ok();
                }
            }
        }
        // Trajectory events (Map3DTrajectoryEvents): where the planning vessel enters or leaves a sphere of
        // influence or hits the ground, and the periapsis around each body it is captured by, with countdowns.
        let mut events: Vec<(GV, String)> = Vec::new();
        if let Some(patches) = planner.and_then(|id| scene.trajectories.get(&id)) {
            for (k, p) in patches.iter().enumerate() {
                let Some(body) = scene.bodies.get(p.body as usize) else {
                    continue;
                };
                let orbit = scene::orbit_from(body.mu, &p.orbit);
                let at = |t: f64| to_km(positions[p.body as usize] + orbit.position_at(t));
                if p.end_ut.is_finite() && p.end_ut > ut {
                    let when = hud::fmt_dt(p.end_ut - ut);
                    let text = match p.end {
                        PatchEnd::Encounter => patches
                            .get(k + 1)
                            .and_then(|n| scene.bodies.get(n.body as usize))
                            .map(|b| format!("{} encounter {when}", b.id)),
                        PatchEnd::Escape => Some(format!("{} escape {when}", body.id)),
                        PatchEnd::Collision => Some(format!("Impact {when}")),
                        _ => None,
                    };
                    if let Some(text) = text {
                        events.push((at(p.end_ut), text));
                    }
                }
                // Periapsis inside a later patch around another body (a flyby or capture).
                if k > 0 && Some(p.body) != patches.first().map(|f| f.body) {
                    let (_, (pe_alt, pe_dt)) = hud::apsides(&p.orbit, body.radius_m, body.mu, ut);
                    let pe_ut = ut + pe_dt;
                    if pe_dt.is_finite() && pe_ut >= p.start_ut && pe_ut <= p.end_ut {
                        events.push((
                            at(pe_ut),
                            format!(
                                "{} Pe {} {}",
                                body.id,
                                hud::fmt_dist(pe_alt),
                                hud::fmt_dt(pe_dt)
                            ),
                        ));
                    }
                }
            }
        }
        for k in 0..8 {
            let id = format!("map-ev-{k}");
            let el = match self.doc.get_element_by_id(&id) {
                Some(e) => e,
                None => {
                    let Ok(e) = self.doc.create_element("div") else {
                        continue;
                    };
                    e.set_id(&id);
                    e.set_class_name("apsis event");
                    e.set_attribute("data-ose", "trajectory-event").ok();
                    self.labels.append_child(&e).ok();
                    e
                }
            };
            let Some(el) = el.dyn_ref::<HtmlElement>() else {
                continue;
            };
            let style = el.style();
            match events
                .get(k)
                .and_then(|(p, t)| project(*p).map(|xy| (xy, t)))
            {
                Some(((x, y), text)) => {
                    style.set_property("left", &format!("{x:.1}px")).ok();
                    style.set_property("top", &format!("{y:.1}px")).ok();
                    style.set_property("visibility", "visible").ok();
                    if el.text_content().as_deref() != Some(text.as_str()) {
                        el.set_text_content(Some(text));
                    }
                }
                None => {
                    style.set_property("visibility", "hidden").ok();
                }
            }
        }
        // Apoapsis and periapsis markers on the followed vessel's current patch, with the altitude and the time
        // to each (the original's Map3DTrajectoryEvents Ap/Pe markers).
        let marked = self.flying.or(self.follow);
        for (id, nu, label) in [
            ("map-ap", std::f64::consts::PI, "Ap"),
            ("map-pe", 0.0, "Pe"),
        ] {
            let el = match self.doc.get_element_by_id(id) {
                Some(e) => e,
                None => {
                    let Ok(e) = self.doc.create_element("div") else {
                        continue;
                    };
                    e.set_id(id);
                    e.set_class_name("apsis");
                    e.set_attribute("data-ose", id).ok();
                    e.set_attribute("title", "Click to warp here").ok();
                    self.labels.append_child(&e).ok();
                    e
                }
            };
            let Some(el) = el.dyn_ref::<HtmlElement>() else {
                continue;
            };
            let at = marked
                .and_then(|vid| scene.vessel(vid))
                .and_then(|v| {
                    let patch = scene.trajectories.get(&v.id).and_then(|p| p.first());
                    let (bi, elements) = match patch {
                        Some(p) => (p.body as usize, p.orbit),
                        None => (v.reference_body as usize, v.orbit),
                    };
                    let body = scene.bodies.get(bi)?;
                    if nu > 0.0 && elements.eccentricity >= 1.0 {
                        return None;
                    }
                    let orbit = scene::orbit_from(body.mu, &elements);
                    let (rel, _) = orbit.state_at_true_anomaly(nu);
                    let (ap, pe) = hud::apsides(&elements, body.radius_m, body.mu, ut);
                    let (alt, t) = if nu > 0.0 { ap? } else { pe };
                    Some((positions[bi] + rel, alt, t))
                })
                .and_then(|(p, alt, t)| project(to_km(p)).map(|xy| (xy, alt, t)));
            let style = el.style();
            match at {
                Some(((x, y), alt, t)) => {
                    self.apsis_ut[usize::from(id == "map-pe")] = ut + t;
                    style.set_property("left", &format!("{x:.1}px")).ok();
                    style.set_property("top", &format!("{y:.1}px")).ok();
                    style.set_property("visibility", "visible").ok();
                    let text = format!("{label} {}  {}", hud::fmt_dist(alt), hud::fmt_dt(t));
                    if el.text_content().as_deref() != Some(text.as_str()) {
                        el.set_text_content(Some(&text));
                    }
                }
                None => {
                    style.set_property("visibility", "hidden").ok();
                }
            }
        }
        // The maneuver gizmo (MapManeuverGizmo): six handles around the node along prograde, retrograde,
        // normal, anti-normal, radial out and radial in, in the burn frame flight3d uses for the node marker.
        let gizmo = self.node.and_then(|node| {
            let v = scene.vessel(node.vessel)?;
            let rb = v.reference_body as usize;
            let orbit = scene::orbit_from(scene.bodies[rb].mu, &v.orbit);
            let (r, vel) = orbit.state_at(node.ut);
            let pro = to_km(vel).normalize_or_zero();
            let nrm = to_km(r.cross(vel)).normalize_or_zero();
            let rad = nrm.cross(pro);
            let at = to_km(positions[rb] + r);
            let (cx, cy) = project(at)?;
            let reach = self.orbit_dist_km * 0.03;
            let dirs = [pro, -pro, nrm, -nrm, rad, -rad].map(|d| {
                project(at + d * reach)
                    .map(|(x, y)| {
                        let (dx, dy) = (x - cx, y - cy);
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 1e-6 {
                            (dx / len, dy / len)
                        } else {
                            (0.0, 0.0)
                        }
                    })
                    .unwrap_or((0.0, 0.0))
            });
            Some((cx, cy, dirs))
        });
        const HANDLES: [(&str, &str); 6] = [
            (
                "prograde",
                "Prograde: drag outward to add speed along the orbit",
            ),
            ("retrograde", "Retrograde: drag outward to slow down"),
            ("normal", "Normal: tilt the orbit"),
            ("antinormal", "Anti-normal: tilt the orbit the other way"),
            ("radialout", "Radial out"),
            ("radialin", "Radial in"),
        ];
        for (k, (name, tip)) in HANDLES.iter().enumerate() {
            let id = format!("mnv-{name}");
            let el = match self.doc.get_element_by_id(&id) {
                Some(e) => e,
                None => {
                    let Ok(e) = self.doc.create_element("div") else {
                        continue;
                    };
                    e.set_id(&id);
                    e.set_class_name(&format!("mnv-h {name}"));
                    e.set_attribute("data-mnv", &k.to_string()).ok();
                    e.set_attribute("data-ose", "maneuver-handle").ok();
                    e.set_attribute("title", tip).ok();
                    self.labels.append_child(&e).ok();
                    e
                }
            };
            let Some(el) = el.dyn_ref::<HtmlElement>() else {
                continue;
            };
            let style = el.style();
            match gizmo {
                Some((cx, cy, dirs)) if dirs[k] != (0.0, 0.0) => {
                    self.mnv_dirs[k] = dirs[k];
                    let (x, y) = (cx + dirs[k].0 * 48.0, cy + dirs[k].1 * 48.0);
                    style.set_property("left", &format!("{x:.1}px")).ok();
                    style.set_property("top", &format!("{y:.1}px")).ok();
                    style.set_property("visibility", "visible").ok();
                }
                _ => {
                    style.set_property("visibility", "hidden").ok();
                }
            }
        }
        if let (Some(node), Some(el)) = (self.node, &self.node_el) {
            if let Some(v) = scene.vessel(node.vessel) {
                let rb = v.reference_body as usize;
                let rel = scene::orbit_from(scene.bodies[rb].mu, &v.orbit).position_at(node.ut);
                if let Some((x, y)) = project(to_km(positions[rb] + rel)) {
                    let style = el.style();
                    style.set_property("left", &format!("{x:.1}px")).ok();
                    style.set_property("top", &format!("{y:.1}px")).ok();
                }
                let remaining = node.ut - ut;
                let sign = if remaining < 0.0 { "+" } else { "-" };
                let r = remaining.abs();
                set_text(
                    &self.doc,
                    "node-dt-text",
                    &format!(
                        "T{sign}{}:{:02}",
                        (r / 60.0).floor() as u64,
                        (r % 60.0) as u64
                    ),
                );
            }
        }
        self.canvas
            .set_attribute(
                "data-ose-scale",
                &format!("{:.3e}", self.orbit_dist_km * 1000.0 / h),
            )
            .ok();
        self.canvas
            .set_attribute(
                "data-ose-focus",
                &format!(
                    "{} {:.3e} {:.3e}",
                    scene.bodies[self.camera.focus].id, centre.x, centre.y
                ),
            )
            .ok();
        self.update_breadcrumbs();
    }

    /// Vessels: predicted trajectory patches, then a marker and label per vessel.
    fn draw_vessels(&self, ut: f64, positions: &[ose_math::DVec3], centre: ose_math::DVec3) {
        let Some(scene) = &self.scene else {
            return;
        };
        let cam = self.camera;
        let ctx = &self.ctx;
        for v in &scene.vessels {
            let mine = v.owner.is_some() && v.owner == self.player_id;
            if let Some(patches) = scene.trajectories.get(&v.id) {
                for patch in patches {
                    let Some(body) = scene.bodies.get(patch.body as usize) else {
                        continue;
                    };
                    let orbit = scene::orbit_from(body.mu, &patch.orbit);
                    let origin = positions[patch.body as usize];
                    let end_ut = if patch.end_ut.is_finite() {
                        patch.end_ut
                    } else if orbit.period.is_finite() {
                        patch.start_ut + orbit.period
                    } else {
                        // Hyperbolic with no end: draw out to the body's SOI or a few hours.
                        patch.start_ut + 6.0 * 3600.0
                    };
                    let soi = body.soi_radius_m.unwrap_or(f64::INFINITY);
                    let n = 240;
                    ctx.set_line_width(if mine { 1.5 } else { 1.0 });
                    let colour = match patch.end {
                        PatchEnd::Final => "rgba(120,220,160,0.9)",
                        PatchEnd::Encounter => "rgba(240,200,90,0.9)",
                        PatchEnd::Escape => "rgba(240,130,90,0.9)",
                        PatchEnd::Collision => "rgba(240,90,90,0.9)",
                        PatchEnd::Maneuver => "rgba(150,170,255,0.9)",
                        #[allow(unreachable_patterns)]
                        _ => "rgba(200,200,200,0.7)",
                    };
                    ctx.set_stroke_style_str(colour);
                    ctx.begin_path();
                    let mut started = false;
                    for k in 0..=n {
                        let t = patch.start_ut + (end_ut - patch.start_ut) * k as f64 / n as f64;
                        let rel = orbit.position_at(t);
                        if rel.length() > soi * 1.02 {
                            break;
                        }
                        let (x, y) = cam.to_screen(origin + rel, centre);
                        if started {
                            ctx.line_to(x, y);
                        } else {
                            ctx.move_to(x, y);
                            started = true;
                        }
                    }
                    ctx.stroke();
                }
            }
            let p = scene.vessel_position(v, ut);
            let (x, y) = cam.to_screen(p, centre);
            ctx.set_fill_style_str(if mine { "#9ef0c0" } else { "#d0d8e8" });
            ctx.begin_path();
            ctx.move_to(x, y - 6.0);
            ctx.line_to(x + 5.0, y + 4.0);
            ctx.line_to(x - 5.0, y + 4.0);
            ctx.close_path();
            ctx.fill();
            if let Some((_, el)) = self.vessel_els.iter().find(|(id, _)| *id == v.id) {
                let style = el.style();
                style
                    .set_property("left", &format!("{:.1}px", x + 8.0))
                    .ok();
                style.set_property("top", &format!("{y:.1}px")).ok();
            }
        }
    }
}

fn body_colour(id: &str, is_star: bool) -> &'static str {
    if is_star {
        return "#ffd87a";
    }
    match id {
        "Kerbin" => "#4aa3ff",
        "Mun" => "#aab2bd",
        "Minmus" => "#b8e0d2",
        "Eve" => "#b06be0",
        "Gilly" => "#8a7a6a",
        "Duna" => "#e0683a",
        "Ike" => "#8e8e8e",
        "Jool" => "#6fd36a",
        "Laythe" => "#3f7fd0",
        "Vall" => "#c8d8e8",
        "Tylo" => "#d8d0c0",
        "Bop" => "#8c7a5a",
        "Pol" => "#d0c070",
        "Moho" => "#c08050",
        "Dres" => "#9a9a9a",
        "Eeloo" => "#e8eef5",
        _ => "#c0c8d8",
    }
}

/// Wire the label-click handler; called from `start` after the labels container exists.
fn install_label_clicks(app: &Shared) -> Result<(), JsValue> {
    let labels = app.borrow().labels.clone();
    install_focus_clicks(app, &labels)?;
    if let Some(crumbs) = app.borrow().doc.get_element_by_id("crumbs") {
        install_focus_clicks(app, &crumbs)?;
    }
    Ok(())
}

/// Clicking an element with `data-ose-index` focuses that body; `data-ose-vessel` follows that vessel.
fn install_focus_clicks(app: &Shared, target: &Element) -> Result<(), JsValue> {
    let a = app.clone();
    let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if let Some(i) = App::label_click_target(&ev) {
            a.borrow_mut().set_focus(i);
        } else if let Some(id) = App::vessel_click_target(&ev) {
            let mut app = a.borrow_mut();
            let rb = app
                .scene
                .as_ref()
                .and_then(|s| s.vessel(id))
                .map(|v| v.reference_body as usize);
            if let Some(rb) = rb {
                app.follow_vessel(id, rb);
            }
        }
    });
    target.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
    on_click.forget();
    let a = app.clone();
    let on_apsis = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let id = ev
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .map(|e| e.id())
            .unwrap_or_default();
        let k = match id.as_str() {
            "map-ap" => 0,
            "map-pe" => 1,
            _ => return,
        };
        let mut app = a.borrow_mut();
        let ut = app.apsis_ut[k];
        app.start_warp_to(ut, if k == 0 { "apoapsis" } else { "periapsis" });
    });
    target.add_event_listener_with_callback("click", on_apsis.as_ref().unchecked_ref())?;
    on_apsis.forget();
    let a = app.clone();
    let on_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if let Some(id) = App::vessel_click_target(&ev) {
            ev.prevent_default();
            a.borrow_mut().toggle_target(ose_net::TargetId::Vessel(id));
        } else if let Some(i) = App::label_click_target(&ev) {
            // A planet or moon (not the Sun) on the map.
            ev.prevent_default();
            let mut app = a.borrow_mut();
            let star = app
                .scene
                .as_ref()
                .and_then(|s| s.bodies.get(i))
                .is_none_or(|b| b.is_star);
            if !star && app.flying.is_some() {
                app.toggle_target(ose_net::TargetId::Body(i as u16));
            }
        }
    });
    target.add_event_listener_with_callback("contextmenu", on_menu.as_ref().unchecked_ref())?;
    on_menu.forget();
    Ok(())
}

/// Attach nodes of a catalog part as (position, orientation) in its own frame, for the part held in the VAB.
#[cfg(target_arch = "wasm32")]
fn vab_nodes(catalog: &vab::Catalog, id: &str) -> Vec<(glam::DVec3, glam::DVec3)> {
    catalog
        .get(id)
        .map(|s| {
            s.attach_nodes
                .iter()
                .map(|n| {
                    (
                        glam::DVec3::new(n.position.x, n.position.y, n.position.z),
                        glam::DVec3::new(n.orientation.x, n.orientation.y, n.orientation.z),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

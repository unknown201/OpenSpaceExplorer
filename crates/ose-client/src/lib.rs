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

pub mod audio;
#[cfg(target_arch = "wasm32")]
pub mod flight3d;
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
    keys_tick_ms: f64,
    controls_sent_ms: f64,
    controls_dirty: bool,
    drive_sent: (f32, f32, bool),
    /// Flight HUD: speed readout in orbit (true) or surface mode, brakes latched from the action bar.
    speed_orbit: bool,
    brakes_latched: bool,
    toasts: hud::Toasts,
    /// Vessels this client has staged at least once: their first stage is lit, so the next is the one after.
    staged: std::collections::HashSet<u64>,
    roll_sent: f32,
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
        dpr: window.device_pixel_ratio().max(1.0),
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
        vab3d: vab3d::VabView::new(),
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
        keys_tick_ms: 0.0,
        controls_sent_ms: 0.0,
        controls_dirty: false,
        drive_sent: (0.0, 0.0, false),
        speed_orbit: false,
        brakes_latched: false,
        toasts: hud::Toasts::default(),
        staged: Default::default(),
        roll_sent: 0.0,
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
        el.set_inner_html(&settings::input_html());
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
        "fh-missions-btn",
        Box::new(|a| {
            if let Some(el) = a.borrow().doc.get_element_by_id("fh-missions") {
                let open = el.class_list().toggle("open").unwrap_or(false);
                store("ose.tracker", if open { "open" } else { "closed" });
            }
        }),
    )?;
    if stored("ose.tracker").as_deref() == Some("open") {
        if let Some(el) = doc.get_element_by_id("fh-missions") {
            el.class_list().add_1("open").ok();
        }
    }
    wire(
        "fh-speed-mode",
        Box::new(|a| {
            let mut app = a.borrow_mut();
            app.speed_orbit = !app.speed_orbit;
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
    wire("fh-transmit", Box::new(|a| a.borrow().flight_science(1)))?;
    wire("fh-recover", Box::new(|a| a.borrow().flight_science(2)))?;
    wire("rnd-back", Box::new(|a| a.borrow().open_ksc()))?;
    wire("ksc-mission", Box::new(|a| a.borrow_mut().open_missions()))?;
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
    wire("vab-launch", Box::new(|a| a.borrow_mut().vab_save(true)))?;
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
                return;
            }
            #[cfg(target_arch = "wasm32")]
            if app.flight_active() && app.gpu.is_some() {
                app.flight3d.zoom(factor);
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
                app.drag = Some((nx, ny));
            }
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if app.flight_active() && app.gpu.is_some() {
            if let Some((x, y)) = app.drag {
                let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
                app.flight3d.orbit(nx - x, ny - y);
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
                    if let Err(e) = result {
                        set_text(&app.doc, "vab-report", &e);
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
            let code = ev.code();
            if code == "Escape" && settings_open(&app.doc) {
                show_settings(&app.doc, false);
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
                    "KeyZ" => {
                        app.set_throttle(1.0);
                        true
                    }
                    "KeyX" => {
                        app.set_throttle(0.0);
                        true
                    }
                    "Space" => {
                        // On EVA Space is the jetpack (held, flight_keys_tick); a Kerbal has nothing to stage.
                        if !ev.repeat() && !app.flying_eva() {
                            app.flight_stage();
                        }
                        true
                    }
                    "KeyT" => {
                        app.sas_hold();
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
                "Slash" if app.in_session => app.request_warp(1),
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
                    let w = kerbin_time::next_warp(app.time.warp);
                    app.request_warp(w);
                }
                "," | "<" => {
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
            a.borrow_mut().keys.up(&ev.code());
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
                        &format!("\u{394}v {:.0} m/s", s.delta_v_vacuum_mps),
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
                            self.start_flight(nv, None);
                        }
                        (Some("fh-flag"), _) => set_text(&self.doc, "fh-note", "flag planted"),
                        // Boarded: fly the craft the Kerbal climbed into.
                        (Some("fh-board"), _) if self.flying == Some(vessel) => {
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
                if let Some(scene) = &mut self.scene {
                    scene.vessels_ut = tick.seconds();
                }
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
                    scene.vessels = vessels;
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
            } => {
                self.vab_stages_readout = stages;
                set_text(
                    &self.doc,
                    "vab-dv",
                    &format!("\u{394}v {delta_v_vacuum_mps:.0} m/s"),
                );
                if self.vab_active() {
                    self.render_vab_view();
                }
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
        if let Some(m) = self.doc.get_element_by_id("menu") {
            m.class_list().remove_1("hidden").ok();
        }
        set_text(&self.doc, "menu-note", "");
        self.menu_screen("ksc");
    }

    // ---- Mission Control

    fn open_missions(&mut self) {
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
        for c in vab::categories(&self.catalog) {
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
                    p.category == self.vab_category
                } else {
                    p.display_title().to_lowercase().contains(&query)
                        || p.id.to_lowercase().contains(&query)
                }
            })
            .collect();
        parts.sort_by(|a, b| {
            (
                &a.size_category,
                a.mass_t
                    .partial_cmp(&b.mass_t)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
                .cmp(&(&b.size_category, std::cmp::Ordering::Equal))
                .then_with(|| a.display_title().cmp(b.display_title()))
        });
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
                "<li data-ose=\"vab-part\" data-ose-part=\"{id}\"{lock}>{title}<small>{cat} \u{b7} size {size} \u{b7} {mass:.2} t{extra}</small></li>",
                id = esc(&p.id),
                lock = if lock {
                    " class=\"locked\" title=\"not researched yet: the launch pad will refuse it\""
                } else {
                    ""
                },
                title = esc(p.display_title()),
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
        if let Err(e) = result {
            set_text(&self.doc, "vab-report", &e);
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
            t.style()
                .set_property("left", &format!("{}px", r.right() + 12.0))
                .ok();
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
        self.vab_turn = vab::turn_then(t, self.vab_turn);
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
        self.render_vab_view();
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
                // The server's readout for this stage: delta-v, sea-level TWR at its start, burn time.
                let stats = self
                    .vab_stages_readout
                    .iter()
                    .find(|r| r.stage_id as usize == n)
                    .map(|r| {
                        format!(
                            "<span class=\"sv\" title=\"vacuum delta-v, thrust-to-weight at Kerbin's sea level, burn time\">\u{394}v {:.0} m/s \u{b7} TWR {:.2} \u{b7} {:.0} s</span>",
                            r.delta_v_vacuum_mps, r.twr_asl, r.burn_time_s
                        )
                    })
                    .unwrap_or_default();
                html.push_str(&format!(
                    "<li><span class=\"st\">STAGE {}</span>{stats}{}</li>",
                    count - n,
                    parts.join("")
                ));
            }
            if html.is_empty() {
                html = "<li class=\"empty\">No stages yet: engines, decouplers and parachutes get one each.</li>".into();
            }
            list.set_inner_html(&html);
        }
        set_text(
            &self.doc,
            "vab-sym",
            &format!("\u{d7}{}", self.vab_symmetry),
        );
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
        }
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
                dv = c.delta_v_vacuum_mps,
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
             <h3>{name}</h3><div class=\"lp-at\">At Launchpad 1</div>{desc}\
             <div class=\"lp-report\"><b>Engineering Report</b><dl>\
             <dt>Parts</dt><dd>{parts}</dd><dt>Stages</dt><dd>{stages}</dd><dt>Mass</dt><dd>{wet:.1} t</dd>\
             <dt>Delta-v (vacuum)</dt><dd>{dv:.0} m/s</dd></dl>{warn}</div>\
             <div class=\"lp-actions\"><button data-ose=\"lp-edit\" data-ose-craft-edit=\"{id}\">Edit Vehicle</button>\
             <button data-ose=\"lp-orbit\" data-ose-craft-orbit=\"{id}\" title=\"skip the ascent: put it straight into orbit\">Orbit</button>\
             <button class=\"go\" data-ose=\"lp-launch\" data-ose-craft-launch=\"{id}\">Launch</button></div>",
            id = c.id,
            name = esc(&c.name),
            // Some stock crafts carry the original's untranslated placeholder.
            desc = if c.description.is_empty()
                || c.description == "This should be a localized string"
            {
                String::new()
            } else {
                format!("<p>{}</p>", esc(&c.description))
            },
            parts = c.parts,
            stages = c.stages,
            wet = c.wet_mass_t,
            dv = c.delta_v_vacuum_mps.max(0.0),
        ));
    }

    // ---- Tracking Station

    fn open_tracking(&mut self) {
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

    /// Roll a library craft out to launch pad 1 (the original's launch: landed, engines off).
    fn launch_craft(&mut self, id: u64) {
        set_text(&self.doc, "menu-note", "rolling out to the launch pad...");
        self.pending_launch = Some(id);
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::LaunchSavedOnPad { id });
        }
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
        self.flying = Some(vessel);
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
            "fh-eva" => ClientMessage::GoEva {
                vessel,
                kerbal: None,
            },
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
    fn update_crew_buttons(&self, f: &ose_net::FlightReadout) {
        let landed = matches!(f.situation, ose_net::FlightSituation::Landed);
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
        self.staged.insert(vessel);
        if let Some(ws) = &self.ws {
            send(ws, &ClientMessage::Stage { vessel });
        }
    }

    fn set_throttle(&mut self, t: f64) {
        self.throttle = t.clamp(0.0, 1.0);
        self.send_controls();
    }

    fn set_attitude(&mut self, a: ose_net::AttitudeMode) {
        self.attitude = a;
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

    /// A Kerbal on EVA, as the original's EVA keys: W walks ahead, S backs up, A/D turn, Space fires the
    /// jetpack straight up. The server walks a Kerbal with throttle and a level heading, and flies the jetpack
    /// when the attitude points steeply up.
    fn eva_keys_tick(&mut self, now: f64, dt: f64) {
        let k = &self.keys;
        let (walk, turn, jet) = (-k.pitch(), k.yaw(), k.is("Space"));
        let heading = match self.attitude {
            ose_net::AttitudeMode::PitchHeading { heading_deg, .. } => heading_deg,
            _ => self.facing_pitch_heading().map(|(_, h)| h).unwrap_or(90.0),
        };
        let heading = (heading + turn * 90.0 * dt).rem_euclid(360.0);
        let (pitch_deg, heading_deg, throttle) = if jet {
            (85.0, heading, 1.0)
        } else if walk < 0.0 {
            (0.0, (heading + 180.0).rem_euclid(360.0), 1.0)
        } else {
            (0.0, heading, walk.max(0.0))
        };
        let attitude = ose_net::AttitudeMode::PitchHeading {
            pitch_deg,
            // Backing up faces the walk direction for the server; the kept heading stays the Kerbal's own.
            heading_deg,
        };
        if walk < 0.0 {
            self.attitude = ose_net::AttitudeMode::PitchHeading {
                pitch_deg: 0.0,
                heading_deg: heading,
            };
        } else {
            self.attitude = attitude;
        }
        let changed = throttle != self.throttle || turn != 0.0 || jet || walk != 0.0;
        if changed || self.controls_dirty {
            self.throttle = throttle;
            self.controls_dirty = true;
        }
        if self.controls_dirty && now - self.controls_sent_ms >= 100.0 {
            self.controls_dirty = false;
            self.controls_sent_ms = now;
            if let (Some(vessel), Some(ws)) = (self.flying, &self.ws) {
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
        if self.flying_eva() {
            self.eva_keys_tick(now, dt);
            return;
        }
        let k = &self.keys;
        let (pitch_in, yaw_in, thr_in) = (k.pitch(), k.yaw(), k.throttle());
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
        if pitch_in != 0.0 || yaw_in != 0.0 {
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
        // At most ten control messages a second while keys are held, and one more when they let go.
        if self.controls_dirty && now - self.controls_sent_ms >= 100.0 {
            self.controls_dirty = false;
            self.controls_sent_ms = now;
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
            let html = hud::staging_html(craft, &self.catalog, next, &f.parachutes);
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
                "stage {} of {}",
                (f.stage + 1).min(f.stages.max(1)),
                f.stages.max(1)
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
            .unwrap_or_else(|| "Shared campaign".into());
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
        if self.in_session && self.vab_active() {
            self.vab_report_tick(now);
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
            if let Some(gpu) = &mut self.gpu {
                let was_ready = self.vab3d.is_ready();
                let free = self.editor.free_nodes(&self.catalog);
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
        let is_eng = |pid: &str| catalog.get(pid).map(|p| p.has_engine).unwrap_or(false);
        let nearby_vessels: Vec<(glam::DVec3, flight3d::Stand)> = scene
            .vessels
            .iter()
            .filter(|o| o.id != id && o.reference_body == v.reference_body)
            .map(|o| (g(scene.vessel_position(o, ut)), flight3d::Stand::of(o)))
            .collect();
        let inp = flight3d::FlightInput {
            vessel: v,
            nearby_vessels: &nearby_vessels,
            stand: flight3d::Stand::of(v),
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
            is_engine: &is_eng,
            maneuver: self
                .node
                .as_ref()
                .filter(|n| n.vessel == id)
                .map(|n| (n.prograde_mps, n.normal_mps, n.radial_mps)),
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

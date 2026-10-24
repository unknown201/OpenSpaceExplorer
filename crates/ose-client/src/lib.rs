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
pub mod kerbin_time;
#[cfg(target_arch = "wasm32")]
pub mod ksc3d;
#[cfg(target_arch = "wasm32")]
pub mod menu3d;
pub mod missions;
#[cfg(target_arch = "wasm32")]
pub mod navball;
#[cfg(target_arch = "wasm32")]
pub mod render;
pub mod rnd;
pub mod scene;
pub mod thumb;
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
    /// Music and sound effects, and the last stage seen in flight (vessel, stage, parts jettisoned) to play
    /// staging and decoupling sounds once.
    audio: audio::Audio,
    last_stage: Option<(u64, u32, usize)>,
    /// Controls as last sent.
    throttle: f64,
    attitude: ose_net::AttitudeMode,
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
        menu3d: menu3d::MenuBackdrop::new(),
        #[cfg(target_arch = "wasm32")]
        ksc3d: ksc3d::KscView::new(),
        #[cfg(target_arch = "wasm32")]
        vab3d: vab3d::VabView::new(),
        press: None,
        vab_press: None,
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
        audio: audio::Audio::new(stored("ose.sound").as_deref() == Some("off")),
        last_stage: None,
        throttle: 0.0,
        attitude: ose_net::AttitudeMode::Up,
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
            "sound-toggle" => return,
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
    if let Some(btn) = doc.get_element_by_id("sound-toggle") {
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
    Ok(())
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
    wire(
        "to-sessions",
        Box::new(|a| a.borrow_mut().open_sessions_screen()),
    )?;
    wire("resume", Box::new(|a| a.borrow().show_menu(false)))?;
    wire(
        "ksc-sessions",
        Box::new(|a| a.borrow_mut().open_sessions_screen()),
    )?;
    wire(
        "to-ksc",
        Box::new(|a| {
            a.borrow_mut().end_flight();
            a.borrow().open_ksc()
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
    for (id, mode) in [
        ("fh-att-up", ose_net::AttitudeMode::Up),
        ("fh-att-pro", ose_net::AttitudeMode::SurfacePrograde),
        ("fh-att-retro", ose_net::AttitudeMode::SurfaceRetrograde),
        ("fh-att-oprog", ose_net::AttitudeMode::OrbitPrograde),
        ("fh-att-normal", ose_net::AttitudeMode::Normal),
        ("fh-att-radial", ose_net::AttitudeMode::RadialOut),
    ] {
        wire(id, Box::new(move |a| a.borrow_mut().set_attitude(mode)))?;
    }
    wire("ksc-button", Box::new(|a| a.borrow().open_ksc()))?;
    wire("ksc-tracking", Box::new(|a| a.borrow().show_menu(false)))?;
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
        Box::new(|a| a.borrow_mut().vab_shift_stage(-1)),
    )?;
    wire(
        "vab-stage-down",
        Box::new(|a| a.borrow_mut().vab_shift_stage(1)),
    )?;
    wire("vab-save", Box::new(|a| a.borrow_mut().vab_save(false)))?;
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
    if let Some(list) = doc.get_element_by_id("crafts") {
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
            let factor = if ev.delta_y() > 0.0 { 1.25 } else { 0.8 };
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
        let on_down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
            if ev.button() == 0 {
                let mut app = a.borrow_mut();
                app.drag = Some((ev.client_x() as f64, ev.client_y() as f64));
                app.press = app.drag;
                app.canvas.class_list().add_1("dragging").ok();
                app.gpu_canvas.class_list().add_1("dragging").ok();
            }
        });
        target.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref())?;
        on_down.forget();
    }

    let a = app.clone();
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let mut app = a.borrow_mut();
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
        // Drop a held part: on its snapped node, or as the root of an empty craft; over the panels it is put
        // back.
        #[cfg(target_arch = "wasm32")]
        {
            let held = app.vab3d.held.take();
            let pressed = app.vab_press.take();
            if let Some((id, _)) = held {
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
                    let cat = std::mem::take(&mut app.catalog);
                    let result = match (cat.get(&id), snap) {
                        (Some(spec), Some((target, node))) => app
                            .editor
                            .add_part_on(spec, &cat, target, &node)
                            .map(|_| ()),
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
            let menu_open = app
                .doc
                .get_element_by_id("menu")
                .map(|m| !m.class_list().contains("hidden"))
                .unwrap_or(false);
            if app.flying.is_some() && !menu_open && !typing {
                // The original's flight keys: Z full throttle, X cut, Shift/Ctrl up and down, Space
                // stage, W/S pitch, A/D heading, T prograde hold, M map.
                let handled = match ev.key().as_str() {
                    "z" | "Z" => {
                        app.set_throttle(1.0);
                        true
                    }
                    "x" | "X" => {
                        app.set_throttle(0.0);
                        true
                    }
                    "Shift" => {
                        let t = app.throttle + 0.1;
                        app.set_throttle(t);
                        true
                    }
                    "Control" => {
                        let t = app.throttle - 0.1;
                        app.set_throttle(t);
                        true
                    }
                    " " => {
                        app.flight_stage();
                        true
                    }
                    "w" | "W" => {
                        app.nudge_attitude(-5.0, 0.0);
                        true
                    }
                    "s" | "S" => {
                        app.nudge_attitude(5.0, 0.0);
                        true
                    }
                    "a" | "A" => {
                        app.nudge_attitude(0.0, -5.0);
                        true
                    }
                    "d" | "D" => {
                        app.nudge_attitude(0.0, 5.0);
                        true
                    }
                    "t" | "T" => {
                        app.set_attitude(ose_net::AttitudeMode::SurfacePrograde);
                        true
                    }
                    "m" | "M" => {
                        app.map_mode = !app.map_mode;
                        true
                    }
                    _ => false,
                };
                if handled {
                    ev.prevent_default();
                    return;
                }
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
            "TrackingCenter" => self.show_menu(false),
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
                    self.craft_docs.remove(&id);
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
                        let altitude_m =
                            input_value(&self.doc, "launch-altitude").unwrap_or(100.0) * 1000.0;
                        set_text(&self.doc, "menu-note", "launching...");
                        if let Some(ws) = &self.ws {
                            send(ws, &ClientMessage::LaunchSavedCraft { id, altitude_m });
                        }
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
                    set_text(
                        &self.doc,
                        "menu-note",
                        &format!(
                            "launch refused: {}",
                            reason.unwrap_or_else(|| "unknown reason".into())
                        ),
                    );
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
            ServerMessage::ScienceReports {
                new,
                skipped,
                carried,
                reason,
                ..
            } => {
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
                    self.end_flight();
                    self.open_ksc();
                }
            }
            ServerMessage::BodyRotation { bodies } => {
                self.body_spins = bodies;
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
            ServerMessage::Vessels { vessels, .. } => {
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

    /// Centre the camera on a vessel: focus its reference body (so orbits draw around it) and follow.
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
        let cat = std::mem::take(&mut self.catalog);
        self.editor.remove_selected(&cat);
        self.catalog = cat;
        self.render_vab();
    }

    fn vab_clear(&mut self) {
        let name = self.editor.craft.name.clone();
        self.editor = vab::Editor::new(&name);
        self.vab_craft_id = None;
        set_text(&self.doc, "vab-report", "");
        self.render_vab();
    }

    fn vab_shift_stage(&mut self, delta: i32) {
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
            let mut html = String::new();
            for (n, stage) in self.editor.craft.stages.iter().enumerate() {
                let names: Vec<String> = stage
                    .iter()
                    .filter_map(|&i| self.editor.craft.parts.get(i))
                    .map(|p| {
                        self.catalog
                            .get(&p.part_id)
                            .map(|s| esc(s.display_title()))
                            .unwrap_or_else(|| esc(&p.part_id))
                    })
                    .collect();
                html.push_str(&format!("<li>stage {}: {}</li>", n + 1, names.join(", ")));
            }
            list.set_inner_html(&html);
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
            html.push_str(&format!(
                "<li data-ose=\"craft\" data-ose-id=\"{id}\" data-ose-dv=\"{dv:.0}\" data-ose-stock=\"{stock}\"><canvas class=\"thumb\" width=\"128\" height=\"168\" data-ose-thumb=\"{id}\"></canvas><div><div class=\"name\">{name}{badge}</div>\
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
        self.draw_thumbnails();
        for c in &self.crafts {
            self.mark_craft_research(c.id);
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
        let Ok(Some(el)) = self
            .doc
            .query_selector(&format!("canvas[data-ose-thumb=\"{id}\"]"))
        else {
            return;
        };
        let Ok(canvas) = el.dyn_into::<HtmlCanvasElement>() else {
            return;
        };
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
        if let Some(c) = craft_id {
            store(&format!("ose.flight.{vessel}"), &c.to_string());
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

    /// Pitch/heading nudge: switches to a pitch-and-heading hold from the current one.
    fn nudge_attitude(&mut self, dpitch: f64, dheading: f64) {
        let (p, h) = match self.attitude {
            ose_net::AttitudeMode::PitchHeading {
                pitch_deg,
                heading_deg,
            } => (pitch_deg, heading_deg),
            _ => (90.0, 90.0),
        };
        self.set_attitude(ose_net::AttitudeMode::PitchHeading {
            pitch_deg: (p + dpitch).clamp(-90.0, 90.0),
            heading_deg: (h + dheading).rem_euclid(360.0),
        });
    }

    fn show_sound_toggle(&self) {
        if let Some(el) = self.doc.get_element_by_id("sound-toggle") {
            el.set_text_content(Some(if self.audio.muted {
                "\u{266a} off"
            } else {
                "\u{266a} on"
            }));
            el.class_list()
                .toggle_with_force("off", self.audio.muted)
                .ok();
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
            Some((_, body, _)) if !menu_open => audio::Scene::Flight(body),
            _ => audio::Scene::Ksc,
        };
        self.audio.music(Some(audio::scene_music(scene)));
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
                 <button data-ose=\"session-join\" data-ose-session=\"{id}\">Join</button></li>",
                id = esc(&s.id),
                name = esc(&s.name),
                mode = mode_name(s.mode),
                ut = kerbin_time::format_ut(s.ut_s),
                players = s.players,
                pl = if s.players == 1 { "player" } else { "players" },
            ));
        }
        if html.is_empty() {
            html = "<li class=\"empty\">no sessions yet; create one below</li>".into();
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
    fn my_vessel(&self) -> Option<(u64, usize)> {
        let scene = self.scene.as_ref()?;
        scene
            .vessels
            .iter()
            .find(|v| v.owner.is_some() && v.owner == self.player_id)
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
        self.update_audio(now);
        if self.time.have_time {
            self.update_hud(ut);
        }
        self.update_flight_hud();
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
        if self.in_session && self.ksc_active() {
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
        let catalog = &self.catalog;
        let is_dec = |pid: &str| catalog.get(pid).map(|p| p.is_decoupler).unwrap_or(false);
        let is_eng = |pid: &str| catalog.get(pid).map(|p| p.has_engine).unwrap_or(false);
        let inp = flight3d::FlightInput {
            vessel: v,
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

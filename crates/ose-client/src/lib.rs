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

pub mod kerbin_time;
pub mod scene;

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
}

type Shared = Rc<RefCell<App>>;

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
        doc,
        canvas,
        ctx,
    }));

    app.borrow_mut().resize();
    install_input(&app)?;
    install_label_clicks(&app)?;
    install_menu(&app)?;
    animate(app);
    Ok(())
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

    let a = app.clone();
    let on_wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
        ev.prevent_default();
        let factor = if ev.delta_y() > 0.0 { 1.25 } else { 0.8 };
        a.borrow_mut()
            .camera
            .zoom_at(factor, ev.offset_x() as f64, ev.offset_y() as f64);
    });
    canvas.add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref())?;
    on_wheel.forget();

    let a = app.clone();
    let on_down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if ev.button() == 0 {
            let mut app = a.borrow_mut();
            app.drag = Some((ev.client_x() as f64, ev.client_y() as f64));
            app.canvas.class_list().add_1("dragging").ok();
        }
    });
    canvas.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref())?;
    on_down.forget();

    let a = app.clone();
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        let mut app = a.borrow_mut();
        if let Some((x, y)) = app.drag {
            let (nx, ny) = (ev.client_x() as f64, ev.client_y() as f64);
            app.camera.pan_px(nx - x, ny - y);
            app.drag = Some((nx, ny));
        }
    });
    window.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())?;
    on_move.forget();

    let a = app.clone();
    let on_up = Closure::<dyn FnMut(MouseEvent)>::new(move |_ev: MouseEvent| {
        let mut app = a.borrow_mut();
        app.drag = None;
        app.canvas.class_list().remove_1("dragging").ok();
    });
    window.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())?;
    on_up.forget();

    let a = app.clone();
    let on_key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let mut app = a.borrow_mut();
            match ev.key().as_str() {
                "f" | "F" => app.frame_focus(),
                "Escape" => {
                    let open = app
                        .doc
                        .get_element_by_id("menu")
                        .map(|m| !m.class_list().contains("hidden"))
                        .unwrap_or(false);
                    app.show_menu(!open);
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
                        self.show_menu(false);
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
        for id in ["name", "sessions", "paused"] {
            if let Some(el) = self.doc.get_element_by_id(&format!("screen-{id}")) {
                if id == which {
                    el.class_list().add_1("open").ok();
                } else {
                    el.class_list().remove_1("open").ok();
                }
            }
        }
        if let Some(card) = self.doc.get_element_by_id("menu-card") {
            if which == "sessions" {
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
        if self.time.have_time {
            self.update_hud(ut);
        }
        let dpr = self.dpr;
        let (w, h) = (self.camera.width, self.camera.height);
        let ctx = &self.ctx;
        ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0).ok();
        ctx.set_fill_style_str("#06080f");
        ctx.fill_rect(0.0, 0.0, w, h);

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

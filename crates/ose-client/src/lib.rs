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

use ose_net::{ClientMessage, ServerMessage, PROTOCOL_VERSION, WS_PATH};

const BODY_LABEL_CLASS: &str = "body";

struct App {
    doc: Document,
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    labels: Element,
    label_els: Vec<HtmlElement>,
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
    connect(&app)?;
    animate(app);
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
            send(
                &ws2,
                &ClientMessage::Hello {
                    protocol: PROTOCOL_VERSION,
                    token: None,
                    display_name: "browser".into(),
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
            a.set_status("disconnected; reload to reconnect");
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
                player_id, tick, ..
            } => {
                self.player_id = Some(player_id);
                self.time.update(tick.seconds(), 1, now_ms());
                self.set_status(&format!("player {player_id}"));
            }
            ServerMessage::Rejected { reason } => {
                self.set_status(&format!("rejected: {reason}"));
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
            ServerMessage::TimeState { tick, warp, .. } => {
                self.time.update(tick.seconds(), warp, now_ms());
            }
            ServerMessage::Snapshot { bodies, .. } => {
                if let Some(scene) = &mut self.scene {
                    scene.snapshot = bodies;
                }
            }
            ServerMessage::Pong { .. } => {}
            // Messages for features this client does not draw yet (vessels, trajectories) are ignored.
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
        let centre = positions[cam.focus];
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
    let a = app.clone();
    let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
        if let Some(i) = App::label_click_target(&ev) {
            a.borrow_mut().set_focus(i);
        }
    });
    labels.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
    on_click.forget();
    Ok(())
}

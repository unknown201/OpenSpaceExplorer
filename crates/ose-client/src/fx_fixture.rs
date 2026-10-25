//! Deterministic rendering fixture for pixel comparison against the game's original D3D bytecode.
use crate::{engine_fx_gpu::EngineDraw, render::Renderer};
use glam::{DMat4, DQuat, DVec3};
use std::cell::RefCell;
use wasm_bindgen::{prelude::*, JsCast};
thread_local! {static FIXTURE:RefCell<Option<Renderer>>=const{RefCell::new(None)};}

#[wasm_bindgen]
pub async fn fx_fixture_setup() -> Result<(), JsValue> {
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document
        .create_element("canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    canvas.set_id("fx-fixture");
    canvas.set_width(512);
    canvas.set_height(512);
    canvas.set_attribute(
        "style",
        "position:fixed;left:0;top:0;width:512px;height:512px;z-index:100000;background:black",
    )?;
    document.body().unwrap().append_child(&canvas)?;
    let mut renderer = Renderer::new(canvas)
        .await
        .ok_or_else(|| JsValue::from_str("WebGPU fixture unavailable"))?;
    renderer.enable_fx_capture();
    FIXTURE.with(|f| *f.borrow_mut() = Some(renderer));
    Ok(())
}

#[wasm_bindgen]
pub fn fx_fixture_draw(
    part: String,
    throttle: f64,
    pressure_kpa: f64,
    time: f64,
    center_y: f64,
    distance: f64,
    yaw: f64,
) -> Result<usize, JsValue> {
    FIXTURE.with(|f| {
        let mut state = f.borrow_mut();
        let gpu = state
            .as_mut()
            .ok_or_else(|| JsValue::from_str("fixture not set up"))?;
        let target = DVec3::new(0.0, center_y, 0.0);
        let camera = target + DVec3::new(yaw.sin(), 0.1, yaw.cos()).normalize() * distance;
        gpu.set_camera(target - camera, DVec3::Y, 45f64.to_radians(), 0.1, 1e6);
        let engine = EngineDraw {
            instance: 0,
            part,
            model: DMat4::from_rotation_translation(DQuat::IDENTITY, -camera),
            throttle,
            commanded: throttle,
            thrust_kn: 0.0,
            distance_from_spawn: 0.0,
            vertical_deviation: 1.0,
            pressure: crate::particles::pressure_multiplier(pressure_kpa),
            mode: 0,
            time,
            simulation_time: time,
        };
        gpu.frame_effects("assets/parts/", &[], &[], [0.0; 3], camera, &[engine]);
        Ok(gpu.engine_fx_layers())
    })
}

#[wasm_bindgen]
pub fn fx_fixture_capture() -> String {
    FIXTURE.with(|f| f.borrow().as_ref().unwrap().engine_fx_capture())
}

/// Two instances of the same production model in opposing deployment poses. Used to catch shared
/// vertex-buffer aliasing as well as missing clips, bones, or hierarchy transforms in browser tests.
#[wasm_bindgen]
pub fn rig_fixture_draw(part: String, fraction: f32) -> Result<bool, JsValue> {
    rig_fixture_instances(part, fraction, 3)
}

#[wasm_bindgen]
pub fn rig_fixture_instances(part: String, fraction: f32, instances: u8) -> Result<bool, JsValue> {
    FIXTURE.with(|f| {
        let mut state = f.borrow_mut();
        let gpu = state
            .as_mut()
            .ok_or_else(|| JsValue::from_str("fixture not set up"))?;
        gpu.set_fixture_grid(false);
        let name = format!("rig:{part}");
        gpu.request_model(&name, &format!("assets/parts/{part}.glb"));
        let mut items = Vec::new();
        let mut ready = false;
        if let Some(model) = gpu.model(&name) {
            let (lo, hi) = model.pose_bounds(model.rig.deployment_sample(0.0));
            let (lo2, hi2) = model.pose_bounds(model.rig.deployment_sample(1.0));
            let (lo, hi) = (lo.min(lo2).as_dvec3(), hi.max(hi2).as_dvec3());
            let center = (lo + hi) * 0.5;
            let size = hi - lo;
            let spacing = size.x.max(size.z).max(1.0) * 1.4;
            let distance = (spacing + size.length()).max(2.0) * 1.5;
            let camera = center + DVec3::new(0.2, 0.9, 1.0).normalize() * distance;
            gpu.set_camera(center - camera, DVec3::Y, 45f64.to_radians(), 0.05, 1e6);
            let model = gpu.model(&name).unwrap();
            for (i, (side, t)) in [(-0.5, fraction), (0.5, 1.0 - fraction)]
                .into_iter()
                .enumerate()
            {
                if instances & (1 << i) == 0 {
                    continue;
                }
                let mut item = crate::render::DrawItem::lit(
                    name.clone(),
                    DVec3::X * spacing * side - camera,
                    DQuat::IDENTITY,
                    1.0,
                    DVec3::new(1.0, 2.0, 2.0),
                );
                item.animation = model.rig.deployment_sample(t);
                item.ambient = 0.35;
                items.push(item);
            }
            ready = model.rig.deployment.is_some();
        }
        gpu.frame("assets/parts/", &items, &[], [0.025, 0.035, 0.06]);
        Ok(ready)
    })
}

#[wasm_bindgen]
pub async fn fx_fixture_pixels() -> Result<Vec<u8>, JsValue> {
    // Take the fixture out of the RefCell so no borrow is held across the GPU mapping await.
    let renderer = FIXTURE
        .with(|f| f.borrow_mut().take())
        .ok_or_else(|| JsValue::from_str("fixture unavailable"))?;
    let pixels = renderer
        .read_fx_pixels()
        .await
        .map_err(|e| JsValue::from_str(&e));
    FIXTURE.with(|f| *f.borrow_mut() = Some(renderer));
    pixels
}

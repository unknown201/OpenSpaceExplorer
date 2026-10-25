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

//! Deterministic rendering fixture for pixel comparison against the game's original D3D bytecode.
use crate::{engine_fx_gpu::EngineDraw, render::Renderer};
use glam::{DMat4, DQuat, DVec3};
use std::cell::RefCell;
use wasm_bindgen::{prelude::*, JsCast};
thread_local! {static FIXTURE:RefCell<Option<Renderer>>=const{RefCell::new(None)};}
thread_local! {static CRASH:RefCell<crate::particles::CrashFx>=RefCell::new(Default::default());}

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
    draw(
        part,
        throttle,
        pressure_kpa,
        time,
        (center_y, distance, yaw),
        0,
    )
}

#[wasm_bindgen]
pub fn fx_fixture_group(
    part: String,
    throttle: f64,
    pressure_kpa: f64,
    time: f64,
    mode: u32,
) -> Result<usize, JsValue> {
    draw(part, throttle, pressure_kpa, time, (0.0, 8.0, 0.4), mode)
}

fn draw(
    part: String,
    throttle: f64,
    pressure_kpa: f64,
    time: f64,
    view: (f64, f64, f64),
    mode: u32,
) -> Result<usize, JsValue> {
    let (center_y, distance, yaw) = view;
    FIXTURE.with(|f| {
        let mut state = f.borrow_mut();
        let gpu = state
            .as_mut()
            .ok_or_else(|| JsValue::from_str("fixture not set up"))?;
        let target = DVec3::new(0.0, center_y, 0.0);
        let camera = target + DVec3::new(yaw.sin(), 0.1, yaw.cos()).normalize() * distance;
        gpu.set_camera(target - camera, DVec3::Y, 45f64.to_radians(), 0.1, 1e6);
        let engine = EngineDraw {
            gimbal_poses: Vec::new(),
            gimbal: DQuat::IDENTITY,
            instance: if part.starts_with("fx_explosion_") {
                0x2000000000000000
            } else {
                0
            },
            part,
            model: DMat4::from_rotation_translation(DQuat::IDENTITY, -camera),
            throttle,
            commanded: throttle,
            vapor: None,
            reentry: None,
            thrust_kn: 0.0,
            distance_from_spawn: 0.0,
            vertical_deviation: 1.0,
            pressure: crate::particles::pressure_multiplier(pressure_kpa),
            atmosphere_density: pressure_kpa / 101.325 * 1.225,
            gravity_mps2: 9.81,
            relative_velocity: 30.0,
            mode,
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

/// Exercise the live-to-crashed latch and the production particle renderer together.
#[wasm_bindgen]
pub fn fx_fixture_crash(
    crashed: bool,
    fueled: bool,
    pressure_kpa: f64,
    time: f64,
) -> Result<usize, JsValue> {
    let hit = CRASH.with(|fx| fx.borrow_mut().update(crashed, fueled, time, [0.0; 3]));
    let part = hit
        .map(|h| crate::particles::explosion_asset(h.fueled, pressure_kpa > 0.01))
        .unwrap_or("engine_1v_methalox_swivel");
    fx_fixture_draw(
        part.into(),
        if hit.is_some() { 1.0 } else { 0.0 },
        pressure_kpa,
        time,
        2.0,
        35.0,
        0.4,
    )?;
    Ok(FIXTURE.with(|f| f.borrow().as_ref().unwrap().engine_particle_count()))
}

#[wasm_bindgen]
pub fn fx_fixture_particle_count() -> usize {
    FIXTURE.with(|f| f.borrow().as_ref().unwrap().engine_particle_count())
}

#[wasm_bindgen]
pub fn fx_fixture_ready(part: String) -> bool {
    FIXTURE.with(|f| f.borrow().as_ref().unwrap().fixture_fx_ready(&part))
}
#[wasm_bindgen]
pub fn fx_fixture_reset() {
    FIXTURE.with(|f| f.borrow_mut().as_mut().unwrap().fixture_fx_reset());
    CRASH.with(|c| *c.borrow_mut() = Default::default());
}

#[wasm_bindgen]
pub fn fx_fixture_vapor(time: f64, wet: bool) -> Result<usize, JsValue> {
    FIXTURE.with(|f| {
        let mut fixture = f.borrow_mut();
        let gpu = fixture
            .as_mut()
            .ok_or_else(|| JsValue::from_str("fixture not set up"))?;
        gpu.set_fixture_grid(false);
        let point = DVec3::X * (time * 80.0);
        let camera = point + DVec3::new(-12.0, 2.0, 35.0);
        gpu.set_camera(
            DVec3::new(0.0, -2.0, -35.0),
            DVec3::Y,
            45f64.to_radians(),
            0.1,
            1e6,
        );
        let engine = EngineDraw {
            gimbal_poses: Vec::new(),
            gimbal: DQuat::IDENTITY,
            instance: 0,
            part: "fx_vapor_vortices".into(),
            model: DMat4::from_translation(point - camera),
            throttle: 1.0,
            commanded: 0.0,
            pressure: 1.0,
            atmosphere_density: if wet { 1.0 } else { 0.0 },
            gravity_mps2: 9.81,
            relative_velocity: 80.0,
            mode: 0,
            time,
            simulation_time: time,
            thrust_kn: 0.0,
            distance_from_spawn: 0.0,
            vertical_deviation: 1.0,
            reentry: None,
            vapor: Some(crate::vapor::State {
                body: if wet { 0 } else { usize::MAX },
                altitude: 1000.0,
                temperature: 280.0,
                density: if wet { 1.0 } else { 0.0 },
                pressure: 101.0,
                dynamic_pressure: 100.0,
                lift: 100.0,
                coefficient: 1.0,
            }),
        };
        gpu.frame_effects("assets/parts/", &[], &[], [0.0; 3], camera, &[engine]);
        Ok(gpu.engine_particle_count())
    })
}

#[wasm_bindgen]
pub fn fx_fixture_reentry(
    part: String,
    speed: f64,
    density: f64,
    time: f64,
) -> Result<usize, JsValue> {
    FIXTURE.with(|f| {
        let mut fixture = f.borrow_mut();
        let gpu = fixture
            .as_mut()
            .ok_or_else(|| JsValue::from_str("fixture not set up"))?;
        gpu.set_fixture_grid(false);
        let camera = DVec3::new(4.0, -2.0, 8.0);
        let target = DVec3::new(0.0, -2.0, 0.0);
        gpu.set_camera(target - camera, DVec3::Y, 45f64.to_radians(), 0.1, 1e6);
        let engine = EngineDraw {
            gimbal_poses: Vec::new(),
            gimbal: DQuat::IDENTITY,
            instance: 0,
            part: format!("reentry:{part}"),
            model: DMat4::from_translation(-camera),
            throttle: 1.0,
            commanded: 0.0,
            pressure: 0.1,
            atmosphere_density: density,
            gravity_mps2: 9.81,
            relative_velocity: speed,
            mode: 0,
            time,
            simulation_time: time,
            thrust_kn: 0.0,
            distance_from_spawn: 0.0,
            vertical_deviation: 1.0,
            vapor: None,
            reentry: Some(crate::reentry::State {
                heading: DVec3::Y,
                density,
                altitude: 30000.0,
                speed,
                mach: speed / 300.0,
                gradient: 0,
            }),
        };
        gpu.frame_effects("assets/parts/", &[], &[], [0.0; 3], camera, &[engine]);
        Ok(gpu.engine_fx_layers())
    })
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
                if model.rig.gimbal.is_some() {
                    item.node_rotations = model.rig.gimbal_turns(DQuat::from_rotation_z(
                        (t as f64 * 2.0 - 1.0) * 5f64.to_radians(),
                    ));
                }
                item.ambient = 0.35;
                items.push(item);
            }
            ready = model.rig.deployment.is_some() || model.rig.gimbal.is_some();
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

#[wasm_bindgen]
pub async fn fx_fixture_occlusion_pixels() -> Result<Vec<u8>, JsValue> {
    let renderer = FIXTURE
        .with(|f| f.borrow_mut().take())
        .ok_or_else(|| JsValue::from_str("fixture unavailable"))?;
    let pixels = renderer
        .read_reentry_pixels()
        .await
        .map_err(|e| JsValue::from_str(&e));
    FIXTURE.with(|f| *f.borrow_mut() = Some(renderer));
    pixels
}

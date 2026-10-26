//! The main menu backdrop: the original game's `kspFlow.unity` scene, exported by
//! `tools/export_menu_scene.py` to `assets/menu/scene.json` and drawn with the WebGPU renderer while no
//! session is joined.
//!
//! What the scene holds (all in the glTF frame the part exports use): a fixed camera, the sun and fill
//! lights, Kerbin at scaled size turning about world Y, two cloud layers over it with their own spin, an
//! atmosphere shell, a starfield sphere around the camera, and three stock vessels, one of which (picked at
//! random, as the original does) spins slowly on its own axis in the foreground. Vessel parts are drawn
//! from the same `assets/parts/<id>.glb` files the editor uses.

use crate::render::{DrawItem, Renderer};
use glam::{DMat4, DQuat, DVec3};
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Deserialize, Clone, Debug)]
pub struct MenuScene {
    pub camera: CameraDef,
    #[serde(default)]
    pub lights: Vec<LightDef>,
    pub kerbin: KerbinDef,
    #[serde(default)]
    pub clouds: Option<CloudsDef>,
    #[serde(default)]
    pub atmosphere: Option<AtmosphereDef>,
    #[serde(default)]
    pub sky: Option<SkyDef>,
    #[serde(default)]
    pub vessels: Vec<VesselDef>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CameraDef {
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    pub fov_deg: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LightDef {
    pub name: String,
    pub to_light: [f64; 3],
    pub color: [f32; 3],
    pub intensity: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct KerbinDef {
    pub file: String,
    pub pos: [f64; 3],
    pub rot: [f64; 4],
    pub scale: [f64; 3],
    pub mesh_radius: f64,
    pub spin_deg_s: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CloudsDef {
    pub file: String,
    pub layers: Vec<CloudLayer>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CloudLayer {
    pub radius: f64,
    pub rot: [f64; 4],
    pub spin_deg_s: f64,
    #[serde(default = "one")]
    pub brightness: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AtmosphereDef {
    pub height: f64,
    #[serde(default = "one")]
    pub brightness: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SkyDef {
    pub file: String,
    pub mesh_radius: f64,
    pub rot: [f64; 4],
}

#[derive(Deserialize, Clone, Debug)]
pub struct VesselDef {
    pub name: String,
    pub pos: [f64; 3],
    pub rot: [f64; 4],
    pub scale: [f64; 3],
    pub spin_deg_s: f64,
    pub parts: Vec<PartPlacement>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PartPlacement {
    pub id: String,
    pub pos: [f64; 3],
    pub rot: [f64; 4],
    pub scale: [f64; 3],
    #[serde(default = "yes")]
    pub active: bool,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn v3(a: [f64; 3]) -> DVec3 {
    DVec3::new(a[0], a[1], a[2])
}

fn q4(a: [f64; 4]) -> DQuat {
    DQuat::from_xyzw(a[0], a[1], a[2], a[3]).normalize()
}

/// Loads the scene once and draws it each frame; `None` scene means not loaded (or no export on the server).
pub struct MenuBackdrop {
    scene: Option<MenuScene>,
    inbox: Rc<RefCell<Option<Result<MenuScene, String>>>>,
    requested: bool,
    failed: bool,
    start_ms: Option<f64>,
    /// Index of the vessel shown, chosen at random when the scene arrives.
    vessel: usize,
    required_models: Vec<String>,
}

impl Default for MenuBackdrop {
    fn default() -> Self {
        Self::new()
    }
}

impl MenuBackdrop {
    pub fn new() -> MenuBackdrop {
        MenuBackdrop {
            scene: None,
            inbox: Rc::new(RefCell::new(None)),
            requested: false,
            failed: false,
            start_ms: None,
            vessel: 0,
            required_models: Vec::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.scene.is_some()
    }

    /// Two real steps per required model: geometry uploaded, then initial material previews ready.
    /// Unknown before the scene description arrives; 100 means a complete textured frame was drawn.
    pub fn presentation_progress(&self, gpu: &Renderer) -> Option<u8> {
        self.scene.as_ref()?;
        let done: usize = self
            .required_models
            .iter()
            .map(|name| usize::from(gpu.has_model(name)) + usize::from(gpu.model_textured(name)))
            .sum();
        Some((done * 100 / (self.required_models.len() * 2).max(1)) as u8)
    }

    pub fn unavailable(&self) -> bool {
        self.failed
    }

    /// Name of the vessel on show (for the page's `data-ose-menu-vessel` hook).
    pub fn vessel_name(&self) -> Option<&str> {
        self.scene
            .as_ref()
            .and_then(|s| s.vessels.get(self.vessel))
            .map(|v| v.name.as_str())
    }

    fn start_fetch(&mut self) {
        self.requested = true;
        let inbox = self.inbox.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = match crate::render::fetch_bytes("assets/menu/scene.json").await {
                Ok(bytes) => serde_json::from_slice::<MenuScene>(&bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            *inbox.borrow_mut() = Some(r);
        });
    }

    /// Draw the backdrop. Returns false while nothing can be drawn yet (the caller clears the canvas).
    pub fn frame(&mut self, gpu: &mut Renderer, now_ms: f64) -> bool {
        if !self.requested {
            self.start_fetch();
        }
        if self.scene.is_none() {
            if let Some(r) = self.inbox.borrow_mut().take() {
                match r {
                    Ok(scene) => {
                        let n = scene.vessels.len().max(1);
                        self.vessel = ((js_sys::Math::random() * n as f64) as usize).min(n - 1);
                        self.required_models = vec!["menu:kerbin".into()];
                        if scene.sky.is_some() {
                            self.required_models.push("menu:sky".into());
                        }
                        if scene
                            .clouds
                            .as_ref()
                            .is_some_and(|c| !c.layers.is_empty() || scene.atmosphere.is_some())
                        {
                            self.required_models.push("menu:clouds".into());
                        }
                        if let Some(vessel) = scene.vessels.get(self.vessel) {
                            self.required_models.extend(
                                vessel
                                    .parts
                                    .iter()
                                    .filter(|p| p.active)
                                    .map(|p| format!("part:{}", p.id)),
                            );
                        }
                        self.required_models.sort();
                        self.required_models.dedup();
                        self.scene = Some(scene);
                        self.start_ms = Some(now_ms);
                    }
                    Err(e) => {
                        if !self.failed {
                            web_sys::console::log_1(&format!("menu scene: {e}").into());
                        }
                        self.failed = true;
                    }
                }
            }
        }
        let Some(scene) = &self.scene else {
            return false;
        };
        let t = (now_ms - self.start_ms.unwrap_or(now_ms)) / 1000.0;
        let cam_pos = v3(scene.camera.pos);
        gpu.set_camera(
            v3(scene.camera.forward),
            v3(scene.camera.up),
            scene.camera.fov_deg.to_radians(),
            0.5,
            60_000.0,
        );
        let light = |name: &str| scene.lights.iter().find(|l| l.name == name);
        let sun = light("Sun").map(|l| v3(l.to_light)).unwrap_or(DVec3::NEG_Z);
        let mut items: Vec<DrawItem> = Vec::new();

        // Sky: an inverted sphere around the camera, unlit.
        if let Some(sky) = &scene.sky {
            gpu.request_model("menu:sky", &format!("assets/{}", sky.file));
            if gpu.has_model("menu:sky") {
                let mut item = DrawItem::lit(
                    "menu:sky".into(),
                    DVec3::ZERO,
                    q4(sky.rot),
                    40_000.0 / sky.mesh_radius.max(1e-9),
                    sun,
                );
                item.emissive = 1.0;
                items.push(item);
            }
        }

        // Kerbin and its layers. World-space spin is a rotation about Y applied before the rest pose.
        let k = &scene.kerbin;
        gpu.request_model("menu:kerbin", &format!("assets/{}", k.file));
        let k_pos = v3(k.pos) - cam_pos;
        let k_spin = DQuat::from_rotation_y((k.spin_deg_s * t).to_radians());
        let k_rot = k_spin * q4(k.rot);
        let k_scale = k.scale[0];
        if gpu.has_model("menu:kerbin") {
            let mut item = DrawItem::lit("menu:kerbin".into(), k_pos, k_rot, k_scale, sun);
            item.ambient = 0.03;
            items.push(item);
        }
        if let Some(clouds) = &scene.clouds {
            gpu.request_model("menu:clouds", &format!("assets/{}", clouds.file));
            if gpu.has_model("menu:clouds") {
                for layer in &clouds.layers {
                    let spin = DQuat::from_rotation_y((layer.spin_deg_s * t).to_radians());
                    let rot = spin * k_rot * q4(layer.rot);
                    let mut item = DrawItem::lit(
                        "menu:clouds".into(),
                        k_pos,
                        rot,
                        k_scale * layer.radius,
                        sun,
                    );
                    let b = layer.brightness;
                    item.color = [b, b, b, 1.0];
                    item.ambient = 0.03;
                    items.push(item);
                }
                if let Some(atm) = &scene.atmosphere {
                    let mut item = DrawItem::lit(
                        "menu:clouds".into(),
                        k_pos,
                        k_rot,
                        k_scale * (1.0 + atm.height),
                        sun,
                    );
                    item.atmosphere = true;
                    item.color = [0.45, 0.65, 1.0, (0.9 * atm.brightness).min(1.0)];
                    items.push(item);
                }
            }
        }

        // The vessel: parts placed in the vessel's frame, the whole spinning about its local Y.
        if let Some(v) = scene.vessels.get(self.vessel) {
            let fill = light("ShipLight");
            let fill_dir = fill.map(|l| v3(l.to_light)).unwrap_or(DVec3::ZERO);
            let fill_strength = fill.map(|l| (l.intensity * 0.35).min(0.8)).unwrap_or(0.0);
            let spin = DQuat::from_rotation_y((v.spin_deg_s * t).to_radians());
            let v_rot = q4(v.rot) * spin;
            let v_scale = v.scale[0];
            let v_m = DMat4::from_scale_rotation_translation(
                DVec3::splat(v_scale),
                v_rot,
                v3(v.pos) - cam_pos,
            );
            for p in &v.parts {
                if !p.active {
                    continue;
                }
                let name = format!("part:{}", p.id);
                gpu.request_model(&name, &format!("assets/parts/{}.glb", p.id));
                if !gpu.has_model(&name) {
                    continue;
                }
                let mut item = DrawItem::lit(
                    name,
                    v_m.transform_point3(v3(p.pos)),
                    v_rot * q4(p.rot),
                    v_scale * p.scale[0],
                    sun,
                );
                item.fill_dir = fill_dir;
                item.fill = fill_strength;
                item.ambient = 0.14;
                items.push(item);
            }
        }
        gpu.frame("assets/menu/", &items, &[], [0.0, 0.0, 0.0]);
        true
    }
}

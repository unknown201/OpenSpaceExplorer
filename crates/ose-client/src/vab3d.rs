//! The Vehicle Assembly Building in 3D: the original's assembly hall (`OAB.prefab`, exported by
//! `tools/export_vab.py` to `assets/vab/vab.glb` and `scene.json`) with the craft under construction drawn
//! from the same `assets/parts/<id>.glb` meshes as everywhere else, at the build area centre.
//!
//! The camera orbits the craft (drag turns, wheel zooms, starting 20 m out as the original's
//! `initialCameraPivotDistance`); a click on a part selects it, the selected part is tinted.

use crate::render::{DrawItem, Renderer};
use glam::{DQuat, DVec3};
use ose_data::craft::Craft;
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Deserialize, Clone, Debug)]
pub struct VabScene {
    pub file: String,
    #[serde(default)]
    pub build_center: Option<[f64; 3]>,
    #[serde(default)]
    pub light: Option<LightDef>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LightDef {
    pub to_light: [f64; 3],
}

/// Unity-frame craft pose -> glTF frame (X mirrored), as the part exports are written.
fn to_gltf(p: ose_math::DVec3) -> DVec3 {
    DVec3::new(-p.x, p.y, p.z)
}

fn quat_to_gltf(q: ose_data::craft::Quat) -> DQuat {
    DQuat::from_xyzw(q.x, -q.y, -q.z, q.w).normalize()
}

pub struct VabView {
    scene: Option<VabScene>,
    inbox: Rc<RefCell<Option<Result<VabScene, String>>>>,
    requested: bool,
    failed: bool,
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    fov_rad: f64,
    target: DVec3,
    cam_pos: DVec3,
    /// Per drawn part: craft index and world-space box, for picking.
    boxes: Vec<(usize, DVec3, DVec3)>,
}

impl Default for VabView {
    fn default() -> Self {
        Self::new()
    }
}

impl VabView {
    pub fn new() -> VabView {
        VabView {
            scene: None,
            inbox: Rc::new(RefCell::new(None)),
            requested: false,
            failed: false,
            yaw: 0.6,
            pitch: 0.15,
            distance: 20.0,
            fov_rad: 60f64.to_radians(),
            target: DVec3::ZERO,
            cam_pos: DVec3::ZERO,
            boxes: Vec::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.scene.is_some()
    }

    fn start_fetch(&mut self) {
        self.requested = true;
        let inbox = self.inbox.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = match crate::render::fetch_bytes("assets/vab/scene.json").await {
                Ok(bytes) => serde_json::from_slice::<VabScene>(&bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            *inbox.borrow_mut() = Some(r);
        });
    }

    pub fn orbit(&mut self, dx_px: f64, dy_px: f64) {
        self.yaw -= dx_px * 0.005;
        self.pitch = (self.pitch + dy_px * 0.005).clamp(-1.2, 1.4);
    }

    pub fn zoom(&mut self, factor: f64) {
        self.distance = (self.distance * factor).clamp(2.0, 250.0);
    }

    fn centre(&self) -> DVec3 {
        self.scene
            .as_ref()
            .and_then(|s| s.build_center)
            .map(|c| DVec3::new(c[0], c[1], c[2]))
            .unwrap_or(DVec3::new(-3.0, 40.0, 44.0))
    }

    fn camera(&self) -> (DVec3, DVec3) {
        let dir = DVec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        (self.target + dir * self.distance, -dir)
    }

    /// Craft part under a pixel (ray against part boxes, nearest first).
    pub fn pick(&self, gpu: &Renderer, x_px: f64, y_px: f64) -> Option<usize> {
        let (pos, forward) = self.camera();
        let up = DVec3::Y;
        let aspect = gpu.width as f64 / gpu.height.max(1) as f64;
        let right = forward.cross(up).normalize();
        let cam_up = right.cross(forward).normalize();
        let ndc_x = x_px / gpu.width.max(1) as f64 * 2.0 - 1.0;
        let ndc_y = 1.0 - y_px / gpu.height.max(1) as f64 * 2.0;
        let t = (self.fov_rad * 0.5).tan();
        let dir = (forward + right * (ndc_x * t * aspect) + cam_up * (ndc_y * t)).normalize();
        let mut best: Option<(f64, usize)> = None;
        for (i, lo, hi) in &self.boxes {
            if let Some(d) = crate::ksc3d::ray_box(pos, dir, *lo, *hi) {
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, *i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Draw hall and craft. `selected` is tinted. Returns false while the hall is not loaded.
    pub fn frame(&mut self, gpu: &mut Renderer, craft: &Craft, selected: Option<usize>) -> bool {
        if !self.requested {
            self.start_fetch();
        }
        if self.scene.is_none() {
            if let Some(r) = self.inbox.borrow_mut().take() {
                match r {
                    Ok(s) => self.scene = Some(s),
                    Err(e) => {
                        if !self.failed {
                            web_sys::console::log_1(&format!("vab scene: {e}").into());
                        }
                        self.failed = true;
                    }
                }
            }
        }
        let Some(scene) = &self.scene else {
            return false;
        };
        let light = scene
            .light
            .as_ref()
            .map(|l| DVec3::new(l.to_light[0], l.to_light[1], l.to_light[2]))
            .unwrap_or(DVec3::new(0.3, 0.9, 0.3));
        let centre = self.centre();

        // Parts: craft poses (Unity frame) converted, the root at the build area centre.
        let poses = crate::thumb::poses(craft);
        let mut items: Vec<DrawItem> = Vec::new();
        self.boxes.clear();
        let mut lo_all = DVec3::splat(f64::INFINITY);
        let mut hi_all = DVec3::splat(f64::NEG_INFINITY);
        let mut placed: Vec<(usize, String, DVec3, DQuat)> = Vec::new();
        for (i, (p, (pos, q))) in craft.parts.iter().zip(poses.iter()).enumerate() {
            let name = format!("part:{}", p.part_id);
            gpu.request_model(&name, &format!("assets/parts/{}.glb", p.part_id));
            placed.push((i, name, centre + to_gltf(*pos), quat_to_gltf(*q)));
        }
        for (i, name, wpos, wrot) in &placed {
            let (lo, hi) = match gpu.model(name) {
                Some(m) => {
                    // World box from the model's bounds (8 corners through the pose).
                    let (bmin, bmax) = (m.bounds_min.as_dvec3(), m.bounds_max.as_dvec3());
                    let mut lo = DVec3::splat(f64::INFINITY);
                    let mut hi = DVec3::splat(f64::NEG_INFINITY);
                    for c in 0..8 {
                        let corner = DVec3::new(
                            if c & 1 == 0 { bmin.x } else { bmax.x },
                            if c & 2 == 0 { bmin.y } else { bmax.y },
                            if c & 4 == 0 { bmin.z } else { bmax.z },
                        );
                        let w = *wpos + *wrot * corner;
                        lo = lo.min(w);
                        hi = hi.max(w);
                    }
                    (lo, hi)
                }
                None => (*wpos - DVec3::splat(0.6), *wpos + DVec3::splat(0.6)),
            };
            lo_all = lo_all.min(lo);
            hi_all = hi_all.max(hi);
            self.boxes.push((*i, lo, hi));
        }
        // Orbit the craft's centre (the build centre while empty).
        self.target = if lo_all.is_finite() {
            (lo_all + hi_all) * 0.5
        } else {
            centre
        };
        let (cam_pos, forward) = self.camera();
        self.cam_pos = cam_pos;
        gpu.set_camera(forward, DVec3::Y, self.fov_rad, 0.1, 2000.0);

        gpu.request_model("vab", &format!("assets/{}", scene.file));
        if gpu.has_model("vab") {
            let mut hall = DrawItem::lit("vab".into(), -cam_pos, DQuat::IDENTITY, 1.0, light);
            hall.ambient = 0.55;
            items.push(hall);
        }
        for (i, name, wpos, wrot) in placed {
            if !gpu.has_model(&name) {
                continue;
            }
            let mut item = DrawItem::lit(name, wpos - cam_pos, wrot, 1.0, light);
            item.ambient = 0.45;
            item.fill_dir = -forward;
            item.fill = 0.35;
            if Some(i) == selected {
                item.color = [0.65, 1.0, 0.7, 1.0];
            }
            items.push(item);
        }
        gpu.frame("assets/vab/", &items, &[], [0.12, 0.13, 0.15]);
        true
    }
}

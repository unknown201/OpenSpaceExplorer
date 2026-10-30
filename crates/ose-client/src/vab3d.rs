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

/// A craft axis (0 X, 1 Y, 2 Z) in the glTF frame the hall is drawn in.
fn gizmo_axis(k: usize) -> DVec3 {
    let mut a = [0.0; 3];
    a[k] = 1.0;
    to_gltf(ose_math::DVec3::new(a[0], a[1], a[2]))
}

/// A gizmo drag's frame: pivot and axis tip on screen, size (metres), axis toward the camera.
pub type GizmoFrame = ((f64, f64), (f64, f64), f64, bool);

/// Distance (pixels) from `p` to the screen segment `a`-`b`.
fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 1e-9 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

pub struct VabView {
    /// The move and rotate tool's gizmo (the original's manipulation widget, tool 2) on the selected part:
    /// its pivot in the craft frame.
    pub gizmo: Option<ose_math::DVec3>,
    /// The gizmo handle under the cursor or in hand: 0..3 move along X/Y/Z, 3..6 turn about X/Y/Z.
    pub gizmo_hot: Option<usize>,
    /// Keep the camera where it is (during a gizmo drag) instead of following the craft's middle.
    pub hold_target: bool,
    /// The rest of the selected part's symmetry set, tinted with it.
    pub sym_lit: Vec<usize>,
    /// The rest of the hovered part's symmetry set, lit with it.
    pub sym_hover: Vec<usize>,
    /// While a part is held: how each part of a symmetry set maps onto the others (for the copies shown on
    /// a set when the held part snaps to one of its nodes).
    pub sym_of: std::collections::HashMap<usize, Vec<crate::vab::SymMap>>,
    /// Parts of the stage card the pointer is over, tinted in the hall (the original's stage highlight).
    pub stage_lit: Vec<usize>,
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
    /// A part being dragged from the picker: its id and attach nodes (position, direction; Unity frame), the
    /// cursor in device
    /// pixels, and the free node it snaps to (part index, node id) when close enough.
    pub held: Option<(String, Vec<(DVec3, DVec3)>)>,
    /// The workspace's loose assemblies (the original's other assemblies on the floor): their parts in the
    /// craft frame they came from, and where their first part stands in the hall (glTF frame).
    pub loose: Vec<(Vec<ose_data::craft::CraftPart>, DVec3)>,
    loose_boxes: Vec<(usize, DVec3, DVec3)>,
    /// Where the held part floats when it snaps to nothing (glTF frame), for dropping it there.
    pub held_at: Option<DVec3>,
    /// The builder's agency colours, for parts and crafts without their own.
    pub paint: Option<ose_data::craft::Paint>,
    pub cursor: (f64, f64),
    pub snap: Option<(usize, String)>,
    /// Surface attachment previews (craft frame poses, one per symmetry copy) for the held part, set by the
    /// screen each frame; drawn when no stack node is in reach.
    pub ghosts: Vec<(ose_math::DVec3, ose_data::craft::Quat)>,
    /// The held part's turn (W/A/S/D/Q/E) for the floating preview, and its yaw alone for a stack snap.
    /// Centre of mass and centre/direction of thrust (craft frame) from the server's StageReport, drawn
    /// when their toolbar toggles are on (the original's CoM and CoT markers).
    pub com: Option<ose_math::DVec3>,
    pub cot: Option<(ose_math::DVec3, ose_math::DVec3)>,
    /// Centre of lift (wings and control surfaces), drawn in blue.
    pub col: Option<ose_math::DVec3>,
    /// The part under the pointer, drawn lighter.
    pub hovered: Option<usize>,
    pub held_turn: ose_data::craft::Quat,
    /// The rotation of the part the held part snaps to (it takes it on).
    snap_turn: ose_data::craft::Quat,
    pub held_yaw: ose_data::craft::Quat,
    /// Vertical camera offset from the craft's centre, metres (arrow keys and middle drag, Home resets).
    pub pan_y: f64,
    /// Part icon mode (`?particons`, tools/export_part_icons.mjs): no hall, markers or tints, a plain
    /// backdrop, and the camera framing the craft to fill a 600 px square at the centre of the view.
    pub icon_mode: bool,
}

/// The part icons' backdrop (the picker tile colour, #141a26).
pub const ICON_BACKDROP: [f64; 3] = [0.078, 0.102, 0.149];

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
            held: None,
            cursor: (0.0, 0.0),
            snap: None,
            ghosts: Vec::new(),
            loose: Vec::new(),
            loose_boxes: Vec::new(),
            held_at: None,
            paint: None,
            pan_y: 0.0,
            com: None,
            cot: None,
            col: None,
            hovered: None,
            stage_lit: Vec::new(),
            gizmo: None,
            gizmo_hot: None,
            hold_target: false,
            sym_lit: Vec::new(),
            sym_hover: Vec::new(),
            sym_of: Default::default(),
            held_turn: crate::vab::NO_TURN,
            snap_turn: crate::vab::NO_TURN,
            held_yaw: crate::vab::NO_TURN,
            icon_mode: false,
        }
    }

    pub fn with_icon_mode(mut self, on: bool) -> VabView {
        self.icon_mode = on;
        self
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

    /// Move the camera up (positive) or down along the craft, in screen pixels of a drag.
    pub fn pan(&mut self, dy_px: f64) {
        self.pan_y += dy_px * self.distance * 0.002;
    }

    pub fn zoom(&mut self, factor: f64) {
        self.distance = (self.distance * factor).clamp(2.0, 250.0);
    }

    /// A hall point (glTF frame) in the craft's frame, and back: for loose assemblies saved in the craft.
    pub fn world_to_craft(&self, w: DVec3) -> ose_math::DVec3 {
        let d = w - self.centre();
        ose_math::DVec3::new(-d.x, d.y, d.z)
    }

    pub fn craft_to_world(&self, p: ose_math::DVec3) -> DVec3 {
        self.centre() + to_gltf(p)
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

    /// The gizmo's size (metres): about the same on screen at any zoom, in steps so its lines stay cached.
    fn gizmo_size(&self) -> f64 {
        ((self.distance * 0.2 * 8.0).round() / 8.0).max(0.25)
    }

    /// The gizmo handle under the cursor (within 12 px): 0..3 the X/Y/Z arrows, 3..6 the rings about them.
    pub fn gizmo_hit(&self, gpu: &Renderer) -> Option<usize> {
        let p = self.gizmo?;
        let at = self.centre() + to_gltf(p) - self.cam_pos;
        let l = self.gizmo_size();
        let mut best: Option<(f64, usize)> = None;
        let mut consider = |d: f64, h: usize| {
            if d < 12.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, h));
            }
        };
        for k in 0..3 {
            let a = gizmo_axis(k);
            if let (Some(s0), Some(s1)) = (gpu.project(at), gpu.project(at + a * l)) {
                consider(seg_dist(self.cursor, s0, s1), k);
            }
            let (u, v) = (gizmo_axis((k + 1) % 3), gizmo_axis((k + 2) % 3));
            let pts: Vec<Option<(f64, f64)>> = (0..=32)
                .map(|n| {
                    let t = n as f64 / 32.0 * std::f64::consts::TAU;
                    gpu.project(at + (u * t.cos() + v * t.sin()) * (l * 0.7))
                })
                .collect();
            for w in pts.windows(2) {
                if let (Some(a), Some(b)) = (w[0], w[1]) {
                    consider(seg_dist(self.cursor, a, b), 3 + k);
                }
            }
        }
        best.map(|(_, h)| h)
    }

    /// For a drag on gizmo handle `h`: the pivot on screen, the far end of its axis on screen, the gizmo's
    /// size in metres, and whether the axis points toward the camera.
    pub fn gizmo_frame(&self, gpu: &Renderer, h: usize) -> Option<GizmoFrame> {
        let p = self.gizmo?;
        let at = self.centre() + to_gltf(p) - self.cam_pos;
        let l = self.gizmo_size();
        let a = gizmo_axis(h % 3);
        Some((
            gpu.project(at)?,
            gpu.project(at + a * l)?,
            l,
            a.dot(-at) > 0.0,
        ))
    }

    /// The free node nearest the cursor on screen within 80 px: (distance, index into `free_nodes`). Uses
    /// the camera of the last frame, so it also works between frames (a drop).
    fn nearest_node(
        &self,
        gpu: &Renderer,
        free_nodes: &[(usize, String, ose_math::DVec3, ose_math::DVec3)],
    ) -> Option<(f64, usize)> {
        let centre = self.centre();
        let mut best: Option<(f64, usize)> = None;
        for (k, (_, _, p, _)) in free_nodes.iter().enumerate() {
            let w = centre + to_gltf(*p);
            if let Some((sx, sy)) = gpu.project(w - self.cam_pos) {
                let d = ((sx - self.cursor.0).powi(2) + (sy - self.cursor.1).powi(2)).sqrt();
                if d < 80.0 && best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, k));
                }
            }
        }
        best
    }

    /// The snap target under the cursor right now (part index, node id).
    pub fn snap_now(
        &self,
        gpu: &Renderer,
        free_nodes: &[(usize, String, ose_math::DVec3, ose_math::DVec3)],
    ) -> Option<(usize, String)> {
        self.nearest_node(gpu, free_nodes)
            .map(|(_, k)| (free_nodes[k].0, free_nodes[k].1.clone()))
    }

    /// Camera ray through the cursor (device pixels): origin and unit direction.
    fn cursor_ray(&self, gpu: &Renderer) -> (DVec3, DVec3) {
        let (pos, forward) = self.camera();
        let up = DVec3::Y;
        let aspect = gpu.width as f64 / gpu.height.max(1) as f64;
        let right = forward.cross(up).normalize();
        let cam_up = right.cross(forward).normalize();
        let ndc_x = self.cursor.0 / gpu.width.max(1) as f64 * 2.0 - 1.0;
        let ndc_y = 1.0 - self.cursor.1 / gpu.height.max(1) as f64 * 2.0;
        let t = (self.fov_rad * 0.5).tan();
        (
            pos,
            (forward + right * (ndc_x * t * aspect) + cam_up * (ndc_y * t)).normalize(),
        )
    }

    /// The craft part under the cursor and the point hit on it, in the craft frame (for surface attachment).
    pub fn surface_hit(&self, gpu: &Renderer) -> Option<(usize, ose_math::DVec3)> {
        let (ro, rd) = self.cursor_ray(gpu);
        let mut best: Option<(f64, usize)> = None;
        for (i, lo, hi) in &self.boxes {
            if let Some(d) = crate::ksc3d::ray_box(ro, rd, *lo, *hi) {
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, *i));
                }
            }
        }
        let (d, i) = best?;
        let w = ro + rd * d - self.centre();
        Some((i, ose_math::DVec3::new(-w.x, w.y, w.z)))
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

    /// The loose assembly under the cursor, if any.
    pub fn pick_loose(&self, gpu: &Renderer, x_px: f64, y_px: f64) -> Option<usize> {
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
        for (k, lo, hi) in &self.loose_boxes {
            if let Some(d) = crate::ksc3d::ray_box(pos, dir, *lo, *hi) {
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, *k));
                }
            }
        }
        best.map(|(_, k)| k)
    }

    /// Draw hall and craft. `selected` is tinted. Returns false while the hall is not loaded.
    pub fn frame(
        &mut self,
        gpu: &mut Renderer,
        craft: &Craft,
        selected: Option<usize>,
        free_nodes: &[(usize, String, ose_math::DVec3, ose_math::DVec3)],
    ) -> bool {
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
        self.target = if self.hold_target {
            // A gizmo drag: the camera stays put, so the part moves under the cursor alone.
            self.target
        } else if lo_all.is_finite() {
            let half = (hi_all.y - lo_all.y) * 0.5 + 2.0;
            self.pan_y = self.pan_y.clamp(-half, half);
            (lo_all + hi_all) * 0.5 + DVec3::Y * self.pan_y
        } else {
            centre
        };
        if self.icon_mode && lo_all.is_finite() {
            // The original's picker angle: a little above, turned to show the side.
            self.pitch = 0.35;
            self.yaw = 0.6;
            self.pan_y = 0.0;
            self.target = (lo_all + hi_all) * 0.5;
            self.distance = ((hi_all - lo_all).length() * 0.5).max(0.05) * 2.7;
        }
        let (cam_pos, forward) = self.camera();
        self.cam_pos = cam_pos;
        gpu.set_camera(forward, DVec3::Y, self.fov_rad, 0.1, 2000.0);

        gpu.request_model("vab", &format!("assets/{}", scene.file));
        if gpu.has_model("vab") && !self.icon_mode {
            let mut hall = DrawItem::lit("vab".into(), -cam_pos, DQuat::IDENTITY, 1.0, light);
            hall.ambient = 0.55;
            items.push(hall);
        }
        // Fairings stand as built over what sits on them, as in the original's VAB.
        if !self.icon_mode {
            gpu.request_model("fx:fairing_ring", "assets/fx/fairing_ring.glb");
            gpu.request_model("fx:fairing_nose", "assets/fx/fairing_nose.glb");
        }
        for (fi, fname, fpos, _) in placed.iter().filter(|p| p.1.starts_with("part:fairing_")) {
            if self.icon_mode
                || !gpu.has_model("fx:fairing_ring")
                || !gpu.has_model("fx:fairing_nose")
            {
                break;
            }
            let Some(base) = gpu.model(fname) else {
                continue;
            };
            let mut radius = (base.bounds_max - base.bounds_min).as_dvec3().max_element() * 0.5;
            let base_radius = radius;
            let bottom = fpos.y + 0.05;
            let mut top = bottom;
            for (j, lo, hi) in &self.boxes {
                let c = (*lo + *hi) * 0.5;
                let side = DVec3::new(c.x - fpos.x, 0.0, c.z - fpos.z).length();
                if j == fi || c.y < bottom || side > base_radius * 1.2 {
                    continue;
                }
                top = top.max(hi.y);
                let half = (hi.x - lo.x).max(hi.z - lo.z) * 0.5;
                radius = radius.max(side + half + 0.05);
            }
            if top - bottom < 0.1 {
                continue;
            }
            for (model, y) in crate::flight3d::fairing_shells(bottom, top, radius) {
                let at = DVec3::new(fpos.x, y, fpos.z);
                let mut it =
                    DrawItem::lit(model.into(), at - cam_pos, DQuat::IDENTITY, radius, light);
                it.ambient = 0.45;
                it.fill_dir = -forward;
                it.fill = 0.35;
                items.push(it);
            }
        }
        // Loose assemblies on the hall floor, a little dimmer than the craft being built.
        self.loose_boxes.clear();
        for (k, (parts, anchor)) in self.loose.iter().enumerate() {
            let Some(first) = parts.first() else { continue };
            let r0 = to_gltf(first.local_position);
            for p in parts {
                let name = format!("part:{}", p.part_id);
                gpu.request_model(&name, &format!("assets/parts/{}.glb", p.part_id));
                let wpos = *anchor + to_gltf(p.local_position) - r0;
                let wrot = quat_to_gltf(p.local_rotation);
                let Some(m) = gpu.model(&name) else { continue };
                let (bmin, bmax) = (m.bounds_min.as_dvec3(), m.bounds_max.as_dvec3());
                let mut lo = DVec3::splat(f64::INFINITY);
                let mut hi = DVec3::splat(f64::NEG_INFINITY);
                for c in 0..8 {
                    let corner = DVec3::new(
                        if c & 1 == 0 { bmin.x } else { bmax.x },
                        if c & 2 == 0 { bmin.y } else { bmax.y },
                        if c & 4 == 0 { bmin.z } else { bmax.z },
                    );
                    let w = wpos + wrot * corner;
                    lo = lo.min(w);
                    hi = hi.max(w);
                }
                self.loose_boxes.push((k, lo, hi));
                let mut item = DrawItem::lit(name, wpos - cam_pos, wrot, 1.0, light);
                item.ambient = 0.45;
                item.color = [0.85, 0.88, 0.95, 1.0];
                items.push(item);
            }
        }
        for (i, name, wpos, wrot) in placed {
            if !gpu.has_model(&name) {
                continue;
            }
            let mut item = DrawItem::lit(name, wpos - cam_pos, wrot, 1.0, light);
            item.ambient = 0.45;
            item.paint = crate::flight3d::part_paint(craft, i, self.paint);
            if crate::flight3d::bare_engine(craft, i) {
                item.hide_containing = Some("fairing");
            }
            item.fill_dir = -forward;
            item.fill = 0.35;
            if self.icon_mode {
            } else if Some(i) == selected || self.sym_lit.contains(&i) {
                item.color = [0.65, 1.0, 0.7, 1.0];
                item.highlight = 0.45;
            } else if self.stage_lit.contains(&i) {
                // the parts of the stage card under the pointer
                item.color = [0.7, 0.85, 1.0, 1.0];
                item.ambient = 0.6;
                item.highlight = 0.4;
            } else if (Some(i) == self.hovered || self.sym_hover.contains(&i))
                && self.held.is_none()
            {
                item.color = [1.0, 1.0, 0.75, 1.0];
                item.ambient = 0.6;
                item.highlight = 0.3;
            }
            items.push(item);
        }
        // The part held from the picker: snap to the free attach node nearest the cursor on screen (within
        // 80 px), else float under the cursor at the craft's depth.
        let mut lines = Vec::new();
        // CoM: a yellow ball of three rings; CoT: a magenta ring with the thrust line. Rings are wider than the
        // craft so they show outside the parts (lines are depth-tested).
        let half = if lo_all.is_finite() {
            ((hi_all.x - lo_all.x).max(hi_all.z - lo_all.z) * 0.5).max(0.4)
        } else {
            0.5
        };
        let ring = |key: String, at: DVec3, a: DVec3, b: DVec3, r: f64, color: [f32; 4]| {
            let pts = (0..=32)
                .map(|k| {
                    let t = k as f64 / 32.0 * std::f64::consts::TAU;
                    (a * t.cos() + b * t.sin()) * r
                })
                .collect();
            crate::render::LineItem {
                key,
                points_km: pts,
                rel_origin_km: at - cam_pos,
                color,
            }
        };
        if let Some(c) = self.com {
            let at = centre + to_gltf(c);
            for (k, (a, b)) in [
                (DVec3::X, DVec3::Y),
                (DVec3::Y, DVec3::Z),
                (DVec3::Z, DVec3::X),
            ]
            .into_iter()
            .enumerate()
            {
                lines.push(ring(
                    format!("vab-com:{k}"),
                    at,
                    a,
                    b,
                    half * 1.3,
                    [1.0, 0.85, 0.2, 1.0],
                ));
            }
        }
        if let Some(c) = self.col {
            let at = centre + to_gltf(c);
            for (k, (a, b)) in [(DVec3::X, DVec3::Y), (DVec3::Y, DVec3::Z)]
                .into_iter()
                .enumerate()
            {
                lines.push(ring(
                    format!("vab-col:{k}"),
                    at,
                    a,
                    b,
                    half * 1.45,
                    [0.35, 0.65, 1.0, 1.0],
                ));
            }
        }
        if let Some((c, d)) = self.cot {
            let at = centre + to_gltf(c);
            let dir = to_gltf(d).normalize_or_zero();
            if dir.length() > 0.5 {
                let side = dir.cross(DVec3::Y).normalize_or_zero();
                let side = if side.length() > 0.5 { side } else { DVec3::X };
                let up = dir.cross(side);
                lines.push(ring(
                    "vab-cot:ring".into(),
                    at,
                    side,
                    up,
                    half * 1.15,
                    [1.0, 0.35, 0.9, 1.0],
                ));
                lines.push(crate::render::LineItem {
                    key: "vab-cot:line".into(),
                    // Along the exhaust, as the original draws the thrust arrow.
                    points_km: vec![DVec3::ZERO, -dir * (half * 4.0 + 2.0)],
                    rel_origin_km: at - cam_pos,
                    color: [1.0, 0.35, 0.9, 1.0],
                });
            }
        }
        // The move and rotate gizmo: an arrow along each craft axis and a ring about it.
        if let Some(p) = self.gizmo.filter(|_| self.held.is_none()) {
            let at = centre + to_gltf(p);
            let l = self.gizmo_size();
            let size = (l * 100.0) as i64;
            let colors = [
                [1.0, 0.32, 0.3, 1.0],
                [0.4, 1.0, 0.45, 1.0],
                [0.35, 0.62, 1.0, 1.0],
            ];
            for (k, &base) in colors.iter().enumerate() {
                let d = gizmo_axis(k);
                let side = gizmo_axis((k + 1) % 3);
                let lit = |h: usize| {
                    if self.gizmo_hot == Some(h) {
                        [1.0, 0.95, 0.55, 1.0]
                    } else {
                        base
                    }
                };
                lines.push(crate::render::LineItem {
                    key: format!("overlay:vab-gz:m{k}:{size}"),
                    points_km: vec![
                        DVec3::ZERO,
                        d * l,
                        d * (l * 0.84) + side * (l * 0.06),
                        d * l,
                        d * (l * 0.84) - side * (l * 0.06),
                    ],
                    rel_origin_km: at - cam_pos,
                    color: lit(k),
                });
                lines.push(ring(
                    format!("overlay:vab-gz:r{k}:{size}"),
                    at,
                    gizmo_axis((k + 1) % 3),
                    gizmo_axis((k + 2) % 3),
                    l * 0.7,
                    lit(3 + k),
                ));
            }
        }
        self.snap = None;
        if let Some((id, nodes)) = self.held.clone() {
            let name = format!("part:{id}");
            gpu.request_model(&name, &format!("assets/parts/{id}.glb"));
            let best = self.nearest_node(gpu, free_nodes);
            // Every free attach node shows while a part is in hand (the original's node markers), drawn over
            // the craft; the one it would snap to stands out.
            let r = ((self.distance * 0.012 * 100.0).round() / 100.0).max(0.03);
            let rk = (r * 100.0) as i64;
            for (k, (_, _, p, _)) in free_nodes.iter().enumerate() {
                let hot = best.is_some_and(|(_, b)| b == k);
                let color = if hot {
                    [0.85, 1.0, 0.45, 1.0]
                } else {
                    [0.35, 1.0, 0.5, 0.75]
                };
                let at = centre + to_gltf(*p);
                for (n, (a, b)) in [
                    (DVec3::X, DVec3::Y),
                    (DVec3::Y, DVec3::Z),
                    (DVec3::Z, DVec3::X),
                ]
                .into_iter()
                .enumerate()
                {
                    lines.push(ring(
                        format!("overlay:vab-node:{n}:{rk}:{hot}"),
                        at,
                        a,
                        b,
                        if hot { r * 1.6 } else { r },
                        color,
                    ));
                }
            }
            let at = match best {
                Some((_, k)) => {
                    let (pi, nid, p, dir) = &free_nodes[k];
                    self.snap = Some((*pi, nid.clone()));
                    // Put the held part's node that faces the other way on the target node; the part takes
                    // the target's rotation (a plane's fuselage), so compare and place in that frame.
                    let pq = craft
                        .parts
                        .get(*pi)
                        .map(|p| p.local_rotation)
                        .unwrap_or(crate::vab::NO_TURN);
                    self.snap_turn = pq;
                    let turned = |v: DVec3| {
                        let r = crate::vab::rotate_by(pq, ose_math::DVec3::new(v.x, v.y, v.z));
                        DVec3::new(r.x, r.y, r.z)
                    };
                    let own = nodes
                        .iter()
                        .find(|(_, o)| turned(*o).dot(DVec3::new(dir.x, dir.y, dir.z)) < -0.5)
                        .map(|(np, _)| turned(*np))
                        .unwrap_or(DVec3::ZERO);
                    let wn = centre + to_gltf(*p);
                    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                        lines.push(crate::render::LineItem {
                            key: format!("vab-snap:{pi}:{nid}:{axis:?}"),
                            points_km: vec![-axis * 0.6, axis * 0.6],
                            rel_origin_km: wn - cam_pos,
                            color: [0.4, 1.0, 0.5, 1.0],
                        });
                    }
                    wn - DVec3::new(-own.x, own.y, own.z)
                }
                None => {
                    let (ro, rd) = self.cursor_ray(gpu);
                    ro + rd * self.distance
                }
            };
            self.held_at = Some(at);
            if gpu.has_model(&name) && self.snap.is_none() && !self.ghosts.is_empty() {
                for (p, q) in &self.ghosts {
                    let mut ghost = DrawItem::lit(
                        name.clone(),
                        centre + to_gltf(*p) - cam_pos,
                        quat_to_gltf(*q),
                        1.0,
                        light,
                    );
                    ghost.ambient = 0.6;
                    ghost.color = [0.7, 1.0, 0.75, 1.0];
                    items.push(ghost);
                }
            } else if gpu.has_model(&name) {
                let turn = if self.snap.is_some() {
                    crate::vab::turn_then(self.snap_turn, self.held_yaw)
                } else {
                    self.held_turn
                };
                // Snapped on a part of a symmetry set: the copies on the rest of the set.
                if let Some(maps) = self.snap.as_ref().and_then(|(pi, _)| self.sym_of.get(pi)) {
                    let pos = self.world_to_craft(at);
                    for m in maps {
                        let (p, q) = m.pose(pos, turn, ose_math::DVec3::new(0.0, 0.0, 1.0));
                        let mut copy = DrawItem::lit(
                            name.clone(),
                            centre + to_gltf(p) - cam_pos,
                            quat_to_gltf(q),
                            1.0,
                            light,
                        );
                        copy.ambient = 0.6;
                        copy.color = [0.7, 1.0, 0.75, 1.0];
                        items.push(copy);
                    }
                }
                let mut ghost = DrawItem::lit(name, at - cam_pos, quat_to_gltf(turn), 1.0, light);
                ghost.ambient = 0.6;
                ghost.color = if self.snap.is_some() {
                    [0.7, 1.0, 0.75, 1.0]
                } else {
                    [1.0, 0.7, 0.6, 1.0]
                };
                items.push(ghost);
            }
        }
        if self.icon_mode {
            lines.clear();
        }
        let clear = if self.icon_mode {
            ICON_BACKDROP
        } else {
            [0.12, 0.13, 0.15]
        };
        gpu.frame("assets/vab/", &items, &lines, clear);
        true
    }
}

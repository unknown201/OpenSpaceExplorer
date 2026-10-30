//! The Kerbal Space Center as a 3D scene: the original's `KSC.prefab` exported by `tools/export_ksc.py`
//! to `assets/ksc/ksc.glb` (every building, pad and road as instances of shared meshes) and
//! `assets/ksc/scene.json` (the camera the original starts from, the clickable facilities as boxes).
//!
//! The camera orbits the look-at point (drag turns, wheel zooms, as `KSCCameraTweakables` allows); a
//! facility under the pointer gets an outline box and a tooltip, and a click opens it.

use crate::render::{DrawItem, LineItem, Renderer};
use glam::{DQuat, DVec3};
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Deserialize, Clone, Debug)]
pub struct KscScene {
    pub file: String,
    pub camera: Option<KscCamera>,
    #[serde(default)]
    pub buildings: Vec<Building>,
    #[serde(default = "default_ground")]
    pub ground_y: f64,
    /// The original's vessel spawn points (`LaunchPad_spawn_01`..., `Runway_spawn_01`...), world positions.
    #[serde(default)]
    pub spawns: std::collections::HashMap<String, [f64; 3]>,
}

fn default_ground() -> f64 {
    -126.5
}

#[derive(Deserialize, Clone, Debug)]
pub struct KscCamera {
    pub origin: [f64; 3],
    pub look_at: [f64; 3],
    #[serde(default = "sixty")]
    pub fov_deg: f64,
}

fn sixty() -> f64 {
    60.0
}

#[derive(Deserialize, Clone, Debug)]
pub struct Building {
    pub facility: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Building {
    /// Display name as the original's facility menu writes it.
    pub fn label(&self) -> &'static str {
        match self.facility.as_str() {
            "VAB" => "Vehicle Assembly Building",
            "TrackingCenter" => "Tracking Station",
            "ResearchCenter" => "R&D Center",
            "TrainingCenter" => "Training Center",
            "AstronautComplex" => "Mission Control",
            "AirTraffic" => "Air Traffic Control",
            "Launchpad_1" => "Launchpad 1",
            "Launchpad_2" => "Launchpad 2",
            "Launchpad_3" => "Launchpad 3",
            "Launchpad_4" => "Launchpad 4",
            "Runway_1" => "Runway 1",
            "Runway_2" => "Runway 2",
            "Boat_Launch" => "Boat Launch",
            _ => "Facility",
        }
    }

    /// Whether the client has a screen for this facility yet.
    pub fn is_open(&self) -> bool {
        matches!(
            self.facility.as_str(),
            "VAB"
                | "ResearchCenter"
                | "AstronautComplex"
                | "TrackingCenter"
                | "Launchpad_1"
                | "Launchpad_2"
                | "Launchpad_3"
                | "Launchpad_4"
        )
    }
}

fn v3(a: [f64; 3]) -> DVec3 {
    DVec3::new(a[0], a[1], a[2])
}

/// The KSC view: scene, orbit camera and pointer state.
pub struct KscView {
    scene: Option<KscScene>,
    inbox: Rc<RefCell<Option<Result<KscScene, String>>>>,
    requested: bool,
    failed: bool,
    /// Orbit camera around `look_at`: yaw and pitch in radians, distance in metres.
    pub look_at: DVec3,
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    fov_rad: f64,
    /// Last camera pose used for a frame (position, forward, up) so picks match what is shown.
    cam_pos: DVec3,
    /// Facility index under the pointer.
    pub hovered: Option<usize>,
    /// Where the camera glides to (look-at point and distance): a facility, or back to the scene's own view.
    goal: Option<(DVec3, f64)>,
    home: Option<(DVec3, f64)>,
}

impl Default for KscView {
    fn default() -> Self {
        Self::new()
    }
}

impl KscView {
    pub fn new() -> KscView {
        KscView {
            scene: None,
            inbox: Rc::new(RefCell::new(None)),
            requested: false,
            failed: false,
            look_at: DVec3::ZERO,
            yaw: 0.0,
            pitch: 0.3,
            distance: 1000.0,
            fov_rad: 60f64.to_radians(),
            cam_pos: DVec3::ZERO,
            hovered: None,
            goal: None,
            home: None,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.scene.is_some()
    }

    pub fn buildings(&self) -> &[Building] {
        self.scene
            .as_ref()
            .map(|s| s.buildings.as_slice())
            .unwrap_or(&[])
    }

    fn start_fetch(&mut self) {
        self.requested = true;
        let inbox = self.inbox.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = match crate::render::fetch_bytes("assets/ksc/scene.json").await {
                Ok(bytes) => serde_json::from_slice::<KscScene>(&bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            *inbox.borrow_mut() = Some(r);
        });
    }

    fn take_inbox(&mut self) {
        if self.scene.is_some() {
            return;
        }
        if let Some(r) = self.inbox.borrow_mut().take() {
            match r {
                Ok(scene) => {
                    if let Some(c) = &scene.camera {
                        let origin = v3(c.origin);
                        self.look_at = v3(c.look_at);
                        let d = origin - self.look_at;
                        self.distance = d.length().max(1.0);
                        self.yaw = d.x.atan2(d.z);
                        self.pitch = (d.y / self.distance).clamp(-1.0, 1.0).asin();
                        self.fov_rad = c.fov_deg.to_radians();
                    }
                    self.home = Some((self.look_at, self.distance));
                    self.scene = Some(scene);
                }
                Err(e) => {
                    if !self.failed {
                        web_sys::console::log_1(&format!("ksc scene: {e}").into());
                    }
                    self.failed = true;
                }
            }
        }
    }

    /// Glide the camera to a facility (its id, as `Launchpad_1`), or back to the overview with `None`.
    pub fn focus(&mut self, facility: Option<&str>) {
        self.goal = match facility {
            None => self.home,
            Some(f) => self.buildings().iter().find(|b| b.facility == f).map(|b| {
                let (lo, hi) = (v3(b.min), v3(b.max));
                ((lo + hi) * 0.5, ((hi - lo).length() * 1.6).max(150.0))
            }),
        };
    }

    pub fn orbit(&mut self, dx_px: f64, dy_px: f64) {
        self.yaw -= dx_px * 0.005;
        self.pitch = (self.pitch + dy_px * 0.005).clamp(0.03, 1.5);
    }

    pub fn zoom(&mut self, factor: f64) {
        self.distance = (self.distance * factor).clamp(20.0, 20_000.0);
    }

    fn camera(&self) -> (DVec3, DVec3, DVec3) {
        let dir = DVec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        let pos = self.look_at + dir * self.distance;
        (pos, -dir, DVec3::Y)
    }

    /// The facility under a pixel (ray against the boxes, nearest first).
    pub fn pick(&self, gpu: &Renderer, x_px: f64, y_px: f64) -> Option<usize> {
        let scene = self.scene.as_ref()?;
        let (pos, forward, up) = self.camera();
        let aspect = gpu.width as f64 / gpu.height.max(1) as f64;
        let right = forward.cross(up).normalize();
        let cam_up = right.cross(forward).normalize();
        let ndc_x = x_px / gpu.width.max(1) as f64 * 2.0 - 1.0;
        let ndc_y = 1.0 - y_px / gpu.height.max(1) as f64 * 2.0;
        let t = (self.fov_rad * 0.5).tan();
        let dir = (forward + right * (ndc_x * t * aspect) + cam_up * (ndc_y * t)).normalize();
        let mut best: Option<(f64, usize)> = None;
        for (i, b) in scene.buildings.iter().enumerate() {
            if let Some(t) = ray_box(pos, dir, v3(b.min), v3(b.max)) {
                if best.map(|(bt, _)| t < bt).unwrap_or(true) {
                    best = Some((t, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Screen position of the top centre of a facility's box, for its tooltip.
    pub fn label_anchor(&self, gpu: &Renderer, i: usize) -> Option<(f64, f64)> {
        let b = self.scene.as_ref()?.buildings.get(i)?;
        let top = DVec3::new(
            (b.min[0] + b.max[0]) * 0.5,
            b.max[1],
            (b.min[2] + b.max[2]) * 0.5,
        );
        gpu.project(top - self.cam_pos)
    }

    /// Draw the campus. Returns false while the scene is not loaded yet.
    /// Draw the campus lit from `sun` (direction to the sun in the campus frame; `None` keeps a fixed
    /// afternoon light).
    pub fn frame(&mut self, gpu: &mut Renderer, sun: Option<DVec3>) -> bool {
        if !self.requested {
            self.start_fetch();
        }
        self.take_inbox();
        if self.scene.is_none() {
            return false;
        }
        if let Some((at, dist)) = self.goal {
            // About a second to settle at 60 fps.
            let k = 0.06;
            self.look_at += (at - self.look_at) * k;
            self.distance += (dist - self.distance) * k;
            if (at - self.look_at).length() < 0.5 && (dist - self.distance).abs() < 0.5 {
                self.goal = None;
            }
        }
        let Some(scene) = &self.scene else {
            return false;
        };
        let (pos, forward, up) = self.camera();
        self.cam_pos = pos;
        gpu.set_camera(forward, up, self.fov_rad, 1.0, 80_000.0);
        gpu.request_model("ksc", &format!("assets/{}", scene.file));
        // Day and night as the original's time-of-day: the sun's height sets the sky and the ambient light;
        // at night a dim moonlight from overhead keeps the campus readable.
        let sun = sun.unwrap_or(DVec3::new(0.45, 0.8, 0.35));
        let elev = sun.y.clamp(-1.0, 1.0);
        let day = ((elev + 0.08) / 0.3).clamp(0.0, 1.0);
        let light = if elev > -0.05 {
            sun
        } else {
            DVec3::new(0.2, 1.0, 0.1)
        };
        // Buildings, the tower and the dish cast the sun's shadows over the campus around the view.
        if elev > 0.0 {
            gpu.set_shadow(
                self.look_at - pos,
                (self.distance * 1.2).clamp(300.0, 3_000.0),
                1_500.0,
                sun,
            );
        }
        let mut items = Vec::new();
        if gpu.has_model("ksc") {
            let mut item = DrawItem::lit("ksc".into(), -pos, DQuat::IDENTITY, 1.0, light);
            item.ambient = (0.12 + 0.26 * day) as f32;
            // The campus lights (windows, floodlights: the original's emissive maps) come on at dusk.
            item.glow = (1.0 - day) as f32;
            if day < 1.0 {
                let k = (0.35 + 0.65 * day) as f32;
                item.color = [k * 0.85, k * 0.9, k, 1.0];
            }
            // The Tracking Station's big dish sweeps slowly back and forth in azimuth, about its own vertical.
            if let Some(dish) = gpu.model("ksc").and_then(|m| {
                m.rig
                    .nodes
                    .iter()
                    .find(|n| n.name == "ksc_campus_trackingcenter_dish")
            }) {
                let up = (dish.rest.rotation.inverse() * glam::Vec3::Y).normalize();
                let t = js_sys::Date::now() / 1000.0;
                let sweep = (0.9 * (t * 0.04).sin()) as f32;
                item.node_turns.push((
                    "ksc_campus_trackingcenter_dish",
                    glam::Quat::from_axis_angle(up, sweep),
                    glam::Vec3::ZERO,
                ));
            }
            items.push(item);
        }
        // Dawn and dusk tint the sky orange near the horizon, night is deep blue.
        let dusk = (1.0 - (elev.abs() / 0.25)).clamp(0.0, 1.0) * day.max(0.3);
        let sky = [
            0.04 + 0.49 * day + 0.25 * dusk,
            0.06 + 0.66 * day + 0.08 * dusk,
            0.14 + 0.78 * day - 0.10 * dusk,
        ];
        let mut lines = Vec::new();
        if let Some(i) = self.hovered {
            if let Some(b) = scene.buildings.get(i) {
                let (lo, hi) = (v3(b.min) - DVec3::splat(2.0), v3(b.max) + DVec3::splat(2.0));
                let c = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
                let strips = [
                    vec![
                        c(lo.x, lo.y, lo.z),
                        c(hi.x, lo.y, lo.z),
                        c(hi.x, lo.y, hi.z),
                        c(lo.x, lo.y, hi.z),
                        c(lo.x, lo.y, lo.z),
                    ],
                    vec![
                        c(lo.x, hi.y, lo.z),
                        c(hi.x, hi.y, lo.z),
                        c(hi.x, hi.y, hi.z),
                        c(lo.x, hi.y, hi.z),
                        c(lo.x, hi.y, lo.z),
                    ],
                    vec![c(lo.x, lo.y, lo.z), c(lo.x, hi.y, lo.z)],
                    vec![c(hi.x, lo.y, lo.z), c(hi.x, hi.y, lo.z)],
                    vec![c(hi.x, lo.y, hi.z), c(hi.x, hi.y, hi.z)],
                    vec![c(lo.x, lo.y, hi.z), c(lo.x, hi.y, hi.z)],
                ];
                for (k, pts) in strips.into_iter().enumerate() {
                    lines.push(LineItem {
                        key: format!("ksc-outline:{}:{k}", b.facility),
                        points_km: pts,
                        rel_origin_km: -pos,
                        color: [0.6, 0.95, 1.0, 0.9],
                    });
                }
            }
        }
        gpu.frame("assets/ksc/", &items, &lines, sky);
        true
    }
}

/// Slab test: distance along the ray to the box, if hit.
pub(crate) fn ray_box(origin: DVec3, dir: DVec3, lo: DVec3, hi: DVec3) -> Option<f64> {
    let mut tmin = 0.0f64;
    let mut tmax = f64::INFINITY;
    for axis in 0..3 {
        let (o, d, l, h) = (origin[axis], dir[axis], lo[axis], hi[axis]);
        if d.abs() < 1e-12 {
            if o < l || o > h {
                return None;
            }
            continue;
        }
        let (mut t1, mut t2) = ((l - o) / d, (h - o) / d);
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmin > tmax {
            return None;
        }
    }
    Some(tmin)
}

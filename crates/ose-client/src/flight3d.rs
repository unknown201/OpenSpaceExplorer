//! The flight view: the player's vessel in 3D near its reference body, after the original's flight scene.
//!
//! Frame: the body's own model frame (the exported scaled sphere, glTF Y-up, not rotated), in metres, with
//! everything else placed by body-fixed latitude, longitude and radius. That is the frame the original uses
//! near the surface (the ground holds still, the sky turns), and it places the KSC campus and the vessel on
//! the texture where they belong: the server's inertial position is turned into latitude and longitude with
//! the body rotation (`initial_rotation_deg + 2 pi t / rotation_period`, `ose_sim::flight::rotation_angle`).
//! The exported sphere's UVs put longitude `L` at azimuth `180 - L`, so east runs clockwise seen from the
//! north pole: the sphere is drawn mirrored along Z, which puts `L` at azimuth `180 + L` with east, north and
//! up right-handed. That is what the campus needs: its four pads fit the pad sites with a proper rotation
//! (yaw about -5 degrees, residual about 340 m) and not with a mirror (1.9 km), and the texture has the
//! KSC on its coast with the ocean to the east, as in the game.
//!
//! Drawn: the body sphere (slightly sunk so its facets never poke through the campus ground), the KSC campus
//! anchored on launch pad 1 at the server's pad site, the vessel from the craft's part meshes oriented by its
//! attitude hold, with jettisoned stages hidden; a chase camera orbits the vessel (drag, wheel).

use crate::render::{DrawItem, Renderer};
use glam::{DMat3, DQuat, DVec3};
use ose_data::craft::Craft;
use ose_net::{AttitudeMode, VesselState};
use std::collections::HashMap;

#[derive(Clone, serde::Deserialize)]
struct Nozzle {
    position: [f64; 3],
    direction: [f64; 3],
}

/// Launch pad 1 (`kerbin_KSCLaunchpad_spawn_01`, docs/reference-notes/terrain.md) and the other pads used to
/// orient the campus.
const KSC_PADS: [(f64, f64, f64); 4] = [
    (-0.6836, -74.8198, 115.2),
    (-0.4360, -75.0270, 111.0),
    (-0.6211, -75.1853, 115.8),
    (-0.4362, -75.3480, 132.0),
];

/// Visual exaggeration of the server's part flex: the real bends are a few milliradians, which reads as
/// nothing on screen; three times keeps the wobble subtle but visible.
pub const FLEX_EXAGGERATION: f64 = 3.0;

/// Snapshot interval the flex interpolation spans (the server streams vessels at 10 Hz).
const SNAPSHOT_MS: f64 = 100.0;

type FlexMap = HashMap<u16, ([f32; 3], [f32; 3])>;

/// Body rotation constants from the embedded system file (the server uses the same data).
#[derive(Clone, Copy, Debug)]
pub struct Spin {
    pub period_s: f64,
    pub initial_deg: f64,
}

pub fn body_spins() -> HashMap<String, Spin> {
    let text = include_str!("../../../assets/own/kerbol-system.json");
    let mut out = HashMap::new();
    if let Ok(serde_json::Value::Array(bodies)) = serde_json::from_str::<serde_json::Value>(text) {
        for b in bodies {
            if let Some(id) = b["id"].as_str() {
                out.insert(
                    id.to_string(),
                    Spin {
                        period_s: b["rotation_period_s"].as_f64().unwrap_or(0.0),
                        initial_deg: b["initial_rotation_deg"].as_f64().unwrap_or(0.0),
                    },
                );
            }
        }
    }
    out
}

impl Spin {
    pub fn angle(&self, ut: f64) -> f64 {
        let w = if self.period_s > 0.0 {
            std::f64::consts::TAU / self.period_s
        } else {
            0.0
        };
        self.initial_deg.to_radians() + w * ut
    }

    pub fn omega(&self) -> f64 {
        if self.period_s > 0.0 {
            std::f64::consts::TAU / self.period_s
        } else {
            0.0
        }
    }
}

/// Model-frame point for body-fixed latitude, longitude (degrees) and radius (m).
pub fn model_point(lat_deg: f64, lon_deg: f64, radius: f64) -> DVec3 {
    let lat = lat_deg.to_radians();
    let az = (180.0 + lon_deg).to_radians();
    radius * DVec3::new(lat.cos() * az.cos(), lat.sin(), -lat.cos() * az.sin())
}

/// Model-frame point of a body-centred inertial (server, Z-up) position at `ut`.
pub fn to_model(r: DVec3, spin: Spin, ut: f64) -> DVec3 {
    let m = r.length().max(1e-9);
    let lat = (r.z / m).clamp(-1.0, 1.0).asin().to_degrees();
    let lon = (r.y.atan2(r.x) - spin.angle(ut)).to_degrees();
    model_point(lat, lon, m)
}

/// Model-frame direction of an inertial direction `d` at inertial position `r` (through the point map, so a
/// mirrored mapping stays consistent).
pub fn dir_to_model(r: DVec3, d: DVec3, spin: Spin, ut: f64) -> DVec3 {
    let eps = (r.length() * 1e-6).max(1.0);
    (to_model(r + d.normalize_or_zero() * eps, spin, ut) - to_model(r, spin, ut))
        .normalize_or_zero()
}

/// Pointing direction (inertial, server frame) for an attitude hold.
pub fn attitude_dir(att: &AttitudeMode, r: DVec3, v_rel: DVec3, spin: Spin) -> DVec3 {
    let up = r.normalize_or_zero();
    let v_surf = v_rel - DVec3::new(0.0, 0.0, spin.omega()).cross(r);
    let or_up = |d: DVec3| if d.length() > 1e-3 { d.normalize() } else { up };
    match att {
        AttitudeMode::Up => up,
        AttitudeMode::SurfacePrograde => or_up(v_surf),
        AttitudeMode::SurfaceRetrograde => or_up(-v_surf),
        AttitudeMode::OrbitPrograde => or_up(v_rel),
        AttitudeMode::OrbitRetrograde => or_up(-v_rel),
        AttitudeMode::Normal => or_up(r.cross(v_rel)),
        AttitudeMode::AntiNormal => or_up(-r.cross(v_rel)),
        AttitudeMode::RadialOut => up,
        AttitudeMode::RadialIn => -up,
        AttitudeMode::PitchHeading {
            pitch_deg,
            heading_deg,
        } => {
            let east = DVec3::Z.cross(up).normalize_or_zero();
            let north = up.cross(east);
            let (p, h) = (pitch_deg.to_radians(), heading_deg.to_radians());
            (p.cos() * (h.cos() * north + h.sin() * east) + p.sin() * up).normalize_or_zero()
        }
        AttitudeMode::Fixed(d) => or_up(DVec3::new(d.x, d.y, d.z)),
    }
}

/// Parts gone after `fired` stages: decouplers in the fired stages and everything hanging below them.
/// Stages fire from the highest stage id down (`ose_data::staging`).
pub fn jettisoned(craft: &Craft, fired: usize, is_decoupler: &dyn Fn(&str) -> bool) -> Vec<bool> {
    let n = craft.parts.len();
    let mut gone = vec![false; n];
    let mut order: Vec<usize> = (0..craft.stages.len()).collect();
    order.sort_by(|a, b| b.cmp(a));
    order.retain(|&s| !craft.stages[s].is_empty());
    let mut roots = Vec::new();
    for &s in order.iter().take(fired) {
        for &pi in &craft.stages[s] {
            if pi < n && is_decoupler(&craft.parts[pi].part_id) {
                roots.push(pi);
            }
        }
    }
    // Mark each decoupler and its descendants.
    for root in roots {
        gone[root] = true;
        loop {
            let mut changed = false;
            for i in 0..n {
                if !gone[i] {
                    if let Some(p) = craft.parts[i].parent {
                        if p < n && gone[p] {
                            gone[i] = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    gone
}

pub struct FlightView {
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    spins: HashMap<String, Spin>,
    ksc: Option<(DVec3, DQuat)>,
    ksc_scene_req: bool,
    ksc_scene: std::rc::Rc<std::cell::RefCell<Option<crate::ksc3d::KscScene>>>,
    /// Part flex of the last two distinct snapshots and when the newer one arrived (ms), for interpolation.
    flex_prev: FlexMap,
    flex_cur: FlexMap,
    flex_since_ms: f64,
    /// Craft axes and local horizon of the last frame, for the navball.
    pub nav: Option<crate::navball::NavFrame>,
    fx_nozzles: std::rc::Rc<std::cell::RefCell<HashMap<String, Vec<Nozzle>>>>,
    fx_requested: bool,
    fx_context: Option<(u64, String)>,
    ground_radius: f64,
    launch_origin: Option<DVec3>,
}

impl Default for FlightView {
    fn default() -> Self {
        Self::new()
    }
}

/// What the flight view needs from the app for one frame.
pub struct FlightInput<'a> {
    pub vessel: &'a VesselState,
    /// Vessel and body star-space positions, body star-space velocity, at `ut`.
    pub vessel_pos: DVec3,
    pub vessel_vel: DVec3,
    pub body_pos: DVec3,
    pub body_vel: DVec3,
    pub star_pos: DVec3,
    pub body_id: &'a str,
    pub body_radius: f64,
    pub atmosphere_m: f64,
    pub ut: f64,
    pub craft: Option<&'a Craft>,
    pub is_decoupler: &'a dyn Fn(&str) -> bool,
    /// Body spin from the server's `BodyRotation` (protocol 9); the embedded system file otherwise.
    pub spin: Option<Spin>,
    /// Wall clock, ms (flex interpolation).
    pub now_ms: f64,
}

fn quat_from_basis(x: DVec3, y: DVec3, z: DVec3) -> DQuat {
    DQuat::from_mat3(&DMat3::from_cols(x, y, z)).normalize()
}

impl FlightView {
    pub fn new() -> FlightView {
        FlightView {
            yaw: 0.8,
            pitch: 0.25,
            distance: 35.0,
            spins: body_spins(),
            ksc: None,
            ksc_scene_req: false,
            ksc_scene: Default::default(),
            flex_prev: HashMap::new(),
            flex_cur: HashMap::new(),
            flex_since_ms: 0.0,
            nav: None,
            fx_nozzles: Default::default(),
            fx_requested: false,
            fx_context: None,
            ground_radius: 0.0,
            launch_origin: None,
        }
    }

    /// Take a new flex snapshot when it differs from the last one, and return each part's interpolated
    /// (rotation vector, offset) in the craft's Unity frame, exaggerated for display.
    fn flex_now(&mut self, flex: &[ose_net::PartFlex], now_ms: f64) -> FlexMap {
        let incoming: FlexMap = flex
            .iter()
            .map(|f| (f.part, (f.rotation, f.offset_m)))
            .collect();
        if incoming != self.flex_cur {
            self.flex_prev = std::mem::replace(&mut self.flex_cur, incoming);
            self.flex_since_ms = now_ms;
        }
        // Render one snapshot behind: blend from the previous to the current over one interval.
        let t = ((now_ms - self.flex_since_ms) / SNAPSHOT_MS).clamp(0.0, 1.0) as f32;
        let k = FLEX_EXAGGERATION as f32;
        let mut out = FlexMap::new();
        let zero = ([0.0f32; 3], [0.0f32; 3]);
        let keys: std::collections::HashSet<u16> = self
            .flex_prev
            .keys()
            .chain(self.flex_cur.keys())
            .copied()
            .collect();
        for part in keys {
            let a = self.flex_prev.get(&part).copied().unwrap_or(zero);
            let b = self.flex_cur.get(&part).copied().unwrap_or(zero);
            let lerp = |x: [f32; 3], y: [f32; 3]| {
                [
                    (x[0] + (y[0] - x[0]) * t) * k,
                    (x[1] + (y[1] - x[1]) * t) * k,
                    (x[2] + (y[2] - x[2]) * t) * k,
                ]
            };
            out.insert(part, (lerp(a.0, b.0), lerp(a.1, b.1)));
        }
        out
    }

    pub fn orbit(&mut self, dx: f64, dy: f64) {
        self.yaw -= dx * 0.005;
        self.pitch = (self.pitch + dy * 0.005).clamp(-1.45, 1.45);
    }

    pub fn zoom(&mut self, f: f64) {
        self.distance = (self.distance * f).clamp(4.0, 5.0e6);
    }

    /// Campus placement in the Kerbin model frame: launch pad 1's flame trench top on the pad site, turned
    /// so the other pads line up best.
    fn ksc_pose(&mut self, radius: f64) -> Option<(DVec3, DQuat)> {
        if let Some(p) = self.ksc {
            return Some(p);
        }
        if !self.ksc_scene_req {
            self.ksc_scene_req = true;
            let slot = self.ksc_scene.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(bytes) = crate::render::fetch_bytes("assets/ksc/scene.json").await {
                    if let Ok(s) = serde_json::from_slice::<crate::ksc3d::KscScene>(&bytes) {
                        *slot.borrow_mut() = Some(s);
                    }
                }
            });
        }
        let scene = self.ksc_scene.borrow().clone()?;
        let pad = |name: &str| {
            scene
                .buildings
                .iter()
                .find(|b| b.facility == name)
                .map(|b| {
                    DVec3::new(
                        (b.min[0] + b.max[0]) * 0.5,
                        b.max[1],
                        (b.min[2] + b.max[2]) * 0.5,
                    )
                })
        };
        // The spawn points are where the original stands a craft on each pad; the facility boxes are the
        // fallback for older exports.
        let local: Vec<Option<DVec3>> = (1..=4)
            .map(|k| {
                scene
                    .spawns
                    .get(&format!("LaunchPad_spawn_0{k}"))
                    .map(|p| DVec3::new(p[0], p[1], p[2]))
                    .or_else(|| pad(&format!("Launchpad_{k}")))
            })
            .collect();
        let l1 = local[0]?;
        let (lat, lon, alt) = KSC_PADS[0];
        let p1 = model_point(lat, lon, radius + alt);
        let up = p1.normalize();
        // Tangent basis at the pad: east-ish reference from the model's north pole.
        let north = (DVec3::Y - up * up.dot(DVec3::Y)).normalize_or_zero();
        let east = north.cross(up);
        // Best yaw: average the angle between local (campus x, z) and world (east, north) pad offsets.
        let (mut s, mut c) = (0.0, 0.0);
        for (k, lk) in local.iter().enumerate().skip(1) {
            let Some(lk) = lk else { continue };
            let (la, lo, al) = KSC_PADS[k];
            let w = model_point(la, lo, radius + al) - p1;
            let (we, wn) = (w.dot(east), w.dot(north));
            let d = *lk - l1;
            // Campus horizontal plane: x and -z (glTF, Y up) as a right-handed (e, n) pair.
            let (le, ln) = (d.x, -d.z);
            let a_w = wn.atan2(we);
            let a_l = ln.atan2(le);
            s += (a_w - a_l).sin();
            c += (a_w - a_l).cos();
        }
        let yaw = s.atan2(c);
        // Local axes: x -> rotate(east, yaw), -z -> rotate(north, yaw), y -> up.
        let ex = east * yaw.cos() + north * yaw.sin();
        let en = -east * yaw.sin() + north * yaw.cos();
        let rot = quat_from_basis(ex, up, -en);
        let pos = p1 - rot * l1;
        self.ksc = Some((pos, rot));
        self.ksc
    }

    /// Draw the flight scene; returns the vessel's altitude above the sphere for the HUD.
    pub fn frame(&mut self, gpu: &mut Renderer, inp: &FlightInput) -> bool {
        let spin = inp.spin.unwrap_or_else(|| {
            self.spins.get(inp.body_id).copied().unwrap_or(Spin {
                period_s: 0.0,
                initial_deg: 0.0,
            })
        });
        let r = inp.vessel_pos - inp.body_pos;
        let v_rel = inp.vessel_vel - inp.body_vel;
        let pos = to_model(r, spin, inp.ut);
        let up = pos.normalize_or_zero();
        let att = inp
            .vessel
            .flight
            .as_ref()
            .map(|f| f.attitude)
            .unwrap_or(AttitudeMode::Up);
        let landed = inp
            .vessel
            .flight
            .as_ref()
            .map(|f| matches!(f.situation, ose_net::FlightSituation::Landed))
            .unwrap_or(false);
        let dir_i = if landed {
            r.normalize_or_zero()
        } else {
            attitude_dir(&att, r, v_rel, spin)
        };
        // The server's actual facing and roll reference (protocol 9) when present; the hold otherwise.
        let g = |d: ose_math::DVec3| DVec3::new(d.x, d.y, d.z);
        let (facing, roll) = match &inp.vessel.flight {
            Some(f) if g(f.facing).length() > 0.5 => (g(f.facing), g(f.roll_ref)),
            _ => (dir_i, DVec3::ZERO),
        };
        let dir = dir_to_model(r, facing, spin, inp.ut);
        let dir = if dir.length() > 0.5 { dir } else { up };
        let roll_m = if roll.length() > 0.5 {
            dir_to_model(r, roll, spin, inp.ut)
        } else {
            DVec3::ZERO
        };
        // Craft +Y along the facing, craft +Z (a pod's windows) along the roll reference.
        let z = (roll_m - dir * roll_m.dot(dir)).normalize_or_zero();
        let vessel_rot = if z.length() > 0.5 {
            quat_from_basis(dir.cross(z), dir, z)
        } else {
            DQuat::from_rotation_arc(DVec3::Y, dir)
        };

        // Chase camera in the local horizon frame.
        let north = (DVec3::Y - up * up.dot(DVec3::Y)).normalize_or_zero();
        let east = north.cross(up);

        // Navball frame: craft nose, top (craft +Z) and right, local horizon, and the markers' directions.
        {
            let forward = vessel_rot * DVec3::Y;
            let top = vessel_rot * DVec3::Z;
            let alt_m = r.length() - inp.body_radius;
            let v_surf = v_rel - DVec3::new(0.0, 0.0, spin.omega()).cross(r);
            // The original switches the navball from surface to orbit speed high in the atmosphere.
            let v_nav = if alt_m < 36_000.0 { v_surf } else { v_rel };
            let moving = v_nav.length() > 0.5;
            let normal_i = r.cross(v_rel);
            let radial_i = r - v_rel.normalize_or_zero() * r.dot(v_rel.normalize_or_zero());
            self.nav = Some(crate::navball::NavFrame {
                forward,
                top,
                right: forward.cross(top),
                up,
                north,
                east,
                prograde: moving.then(|| dir_to_model(r, v_nav, spin, inp.ut)),
                normal: (normal_i.length() > 1.0).then(|| dir_to_model(r, normal_i, spin, inp.ut)),
                radial: (radial_i.length() > 1.0 && moving)
                    .then(|| dir_to_model(r, radial_i, spin, inp.ut)),
            });
        }
        let offs = self.pitch.cos() * (self.yaw.cos() * north + self.yaw.sin() * east)
            + self.pitch.sin() * up;
        let cam = pos + offs * self.distance;
        let alt = r.length() - inp.body_radius;
        let near = (self.distance * 0.02).clamp(0.3, 5_000.0);
        let far = (self.distance * 50.0).clamp(4.0e6, 5.0e8);
        gpu.set_camera(-offs, up, 60f64.to_radians(), near, far);

        let sun = dir_to_model(
            r,
            (inp.star_pos - inp.body_pos).normalize_or_zero(),
            spin,
            inp.ut,
        );
        let mut items: Vec<DrawItem> = Vec::new();
        let context = (inp.vessel.id, inp.body_id.to_string());
        if self.fx_context.as_ref() != Some(&context) {
            self.fx_context = Some(context);
            self.ground_radius = inp.body_radius;
            self.launch_origin = None;
        }
        if landed {
            self.ground_radius = pos.length();
            self.launch_origin = Some(pos);
        }
        if !self.fx_requested {
            self.fx_requested = true;
            let slot = self.fx_nozzles.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(bytes) = crate::render::fetch_bytes("assets/fx/engines.json").await {
                    if let Ok(nozzles) = serde_json::from_slice(&bytes) {
                        *slot.borrow_mut() = nozzles;
                    }
                }
            });
        }
        let mut engine_draws = Vec::new();

        // The body sphere, sunk by 1.5 km so its facets stay under the campus ground near the KSC.
        let body_model = format!("body:{}", inp.body_id);
        gpu.request_model(&body_model, &format!("assets/bodies/{}.glb", inp.body_id));
        if let Some(m) = gpu.model(&body_model) {
            let sunk = if alt < 40_000.0 { 1500.0 } else { 0.0 };
            let scale = (inp.body_radius - sunk) / m.mesh_radius.max(1e-9);
            let mut it = DrawItem::lit(body_model.clone(), -cam, DQuat::IDENTITY, scale, sun);
            it.ambient = 0.12;
            it.mirror_z = true;
            items.push(it);
        }
        // The KSC campus on Kerbin while low enough to matter.
        if inp.body_id == "Kerbin" && alt < 25_000.0 {
            if let Some((kpos, krot)) = self.ksc_pose(inp.body_radius) {
                gpu.request_model("ksc", "assets/ksc/ksc.glb");
                if gpu.has_model("ksc") {
                    let mut it = DrawItem::lit("ksc".into(), kpos - cam, krot, 1.0, sun);
                    it.ambient = 0.38;
                    items.push(it);
                }
            }
        }
        // The vessel.
        if let Some(craft) = inp.craft {
            let fired = inp
                .vessel
                .flight
                .as_ref()
                .map(|f| f.stage as usize)
                .unwrap_or(0);
            let gone = jettisoned(craft, fired, inp.is_decoupler);
            let flex = match &inp.vessel.flight {
                Some(f) => self.flex_now(&f.flex, inp.now_ms),
                None => FlexMap::new(),
            };
            let poses = crate::thumb::poses(craft);
            // Bottom anchor: the craft's lowest point (along its own +Y) sits at the vessel point.
            let mut min_y = f64::INFINITY;
            let mut parts = Vec::new();
            for (i, (p, (pp, q))) in craft.parts.iter().zip(poses.iter()).enumerate() {
                if gone.get(i).copied().unwrap_or(false) {
                    continue;
                }
                let name = format!("part:{}", p.part_id);
                gpu.request_model(&name, &format!("assets/parts/{}.glb", p.part_id));
                let mut lp = DVec3::new(-pp.x, pp.y, pp.z);
                let mut lq = DQuat::from_xyzw(q.x, -q.y, -q.z, q.w).normalize();
                // Wobble: rotate about the part's own origin, then shift, both given in the craft's Unity
                // frame; mirroring X turns a rotation vector (x, y, z) into (x, -y, -z).
                if let Some((rv, off)) = flex.get(&(i as u16)) {
                    let w = DVec3::new(rv[0] as f64, -(rv[1] as f64), -(rv[2] as f64));
                    if w.length() > 1e-9 {
                        lq = (DQuat::from_scaled_axis(w) * lq).normalize();
                    }
                    lp += DVec3::new(-(off[0] as f64), off[1] as f64, off[2] as f64);
                }
                if let Some(m) = gpu.model(&name) {
                    for c in 0..8 {
                        let (lo, hi) = (m.bounds_min.as_dvec3(), m.bounds_max.as_dvec3());
                        let corner = DVec3::new(
                            if c & 1 == 0 { lo.x } else { hi.x },
                            if c & 2 == 0 { lo.y } else { hi.y },
                            if c & 4 == 0 { lo.z } else { hi.z },
                        );
                        min_y = min_y.min((lp + lq * corner).y);
                    }
                }
                parts.push((i, name, lp, lq));
            }
            let lift = if min_y.is_finite() { -min_y } else { 0.0 };
            // Use the server's attached, fuel-fed engines, not just stage membership. Nozzles share
            // the exact flexed pose used by the part mesh (including radial/multi-nozzle engines).
            if let Some(f) = &inp.vessel.flight {
                let pressure = crate::particles::pressure_multiplier(f.static_pressure_kpa);
                let nozzles = self.fx_nozzles.borrow();
                let mut launch_smoke_added = false;
                for (pi, name, lp, lq) in &parts {
                    let part_id = &craft.parts[*pi].part_id;
                    if part_id.starts_with("engine_") || part_id.starts_with("booster_") {
                        engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                            instance: (inp.vessel.id << 16) | *pi as u64,
                            part: part_id.clone(),
                            model: glam::DMat4::from_rotation_translation(
                                vessel_rot * *lq,
                                pos - cam + vessel_rot * (*lp + DVec3::Y * lift),
                            ),
                            throttle: if f.firing_engines.contains(&(*pi as u16)) {
                                f.throttle
                            } else {
                                0.0
                            },
                            pressure,
                            mode: 0,
                            time: inp.now_ms * 0.001,
                            simulation_time: inp.ut,
                            commanded: f.throttle,
                            thrust_kn: f.thrust_kn,
                            distance_from_spawn: 0.0,
                            vertical_deviation: 1.0,
                        });
                    }
                    if !f.firing_engines.contains(&(*pi as u16)) || f.thrust_kn <= 0.0 {
                        continue;
                    }
                    let Some(m) = gpu.model(name) else {
                        continue;
                    };
                    let (lo, hi) = (m.bounds_min.as_dvec3(), m.bounds_max.as_dvec3());
                    let part_id = &craft.parts[*pi].part_id;
                    let fallback = [Nozzle {
                        position: [(lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5],
                        direction: [0.0, -1.0, 0.0],
                    }];
                    let jets = nozzles.get(part_id).map(Vec::as_slice).unwrap_or(&fallback);
                    let width = ((hi.x - lo.x).min(hi.z - lo.z) * 0.55
                        / (jets.len().max(1) as f64).sqrt())
                    .clamp(0.08, 12.0);
                    for (j, nozzle) in jets.iter().enumerate() {
                        let at = pos
                            + vessel_rot
                                * (*lp
                                    + *lq * DVec3::from_array(nozzle.position)
                                    + DVec3::Y * lift);
                        let direction = (vessel_rot * *lq * DVec3::from_array(nozzle.direction))
                            .normalize_or_zero();
                        let down = -direction.dot(up);
                        let height = (at.length() - self.ground_radius).max(0.0);
                        if pressure > 0.01 && down > 0.3 && height / down < width * 25.0 {
                            let ground = at + direction * (height / down);
                            let rotation = DQuat::from_rotation_arc(DVec3::Y, up);
                            engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                                instance: 0x8000000000000000
                                    | (inp.vessel.id << 24)
                                    | ((*pi as u64) << 8)
                                    | j as u64,
                                part: "fx_groundblast_atmosphere_01".into(),
                                model: glam::DMat4::from_rotation_translation(
                                    rotation,
                                    ground - cam,
                                ),
                                throttle: f.throttle,
                                commanded: f.throttle,
                                pressure,
                                mode: 0,
                                time: inp.now_ms * 0.001,
                                simulation_time: inp.ut,
                                thrust_kn: f.thrust_kn,
                                distance_from_spawn: 0.0,
                                vertical_deviation: down,
                            });
                            let distance = self
                                .launch_origin
                                .map(|origin| origin.distance(pos))
                                .unwrap_or(f64::INFINITY);
                            if inp.body_id == "Kerbin" && distance < 40.0 && !launch_smoke_added {
                                launch_smoke_added = true;
                                engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                                    instance: 0x4000000000000000 | inp.vessel.id,
                                    part: "fx_launchsmoke_default".into(),
                                    model: glam::DMat4::from_rotation_translation(
                                        rotation,
                                        self.launch_origin.unwrap().normalize()
                                            * self.ground_radius
                                            - cam,
                                    ),
                                    throttle: f.throttle,
                                    commanded: f.throttle,
                                    pressure,
                                    mode: 0,
                                    time: inp.now_ms * 0.001,
                                    simulation_time: inp.ut,
                                    thrust_kn: f.thrust_kn,
                                    distance_from_spawn: distance,
                                    vertical_deviation: down,
                                });
                            }
                        }
                    }
                }
            }
            for (_, name, lp, lq) in parts {
                if !gpu.has_model(&name) {
                    continue;
                }
                let wpos = pos + vessel_rot * (lp + DVec3::Y * lift);
                let mut it = DrawItem::lit(name, wpos - cam, vessel_rot * lq, 1.0, sun);
                it.ambient = 0.25;
                it.fill_dir = up;
                it.fill = 0.25;
                items.push(it);
            }
        } else {
            // Unknown craft (launched from another browser): a probe core as a stand-in.
            let name = "part:probe_0v_octagonal_electricity_1x1".to_string();
            gpu.request_model(&name, "assets/parts/probe_0v_octagonal_electricity_1x1.glb");
            if gpu.has_model(&name) {
                items.push(DrawItem::lit(name, pos - cam, vessel_rot, 1.0, sun));
            }
        }
        // Sky: blue low in the atmosphere, black above it.
        let t = if inp.atmosphere_m > 0.0 {
            (alt / inp.atmosphere_m).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let k = (1.0 - t).powf(2.5);
        let clear = [0.45 * k, 0.65 * k, 0.92 * k];
        gpu.frame_effects("assets/parts/", &items, &[], clear, cam, &engine_draws);
        true
    }
}

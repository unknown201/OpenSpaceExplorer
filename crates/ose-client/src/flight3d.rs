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
    #[serde(default)]
    mode: u32,
    #[serde(default)]
    rotate: bool,
}

/// Launch pad 1 (`kerbin_KSCLaunchpad_spawn_01`, docs/reference-notes/terrain.md) and the other pads used to
/// orient the campus.
const KSC_PADS: [(f64, f64, f64); 4] = [
    (-0.6836, -74.8198, 115.2),
    (-0.4360, -75.0270, 111.0),
    (-0.6211, -75.1853, 115.8),
    (-0.4362, -75.3480, 132.0),
];

/// Visual exaggeration of the server's part flex: with the server's realistic joint loads the bends are a
/// milliradian or two under thrust, which reads as nothing on screen; twelve times makes the wobble visible
/// and still subtle. FLEX_MAX_RAD and FLEX_MAX_M cap the result so a near-breaking joint doesn't fold a part.
pub const FLEX_EXAGGERATION: f64 = 12.0;
const FLEX_MAX_RAD: f32 = 0.12;
const FLEX_MAX_M: f32 = 0.4;

fn cap(v: [f32; 3], max: f32) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > max {
        let k = max / len;
        [v[0] * k, v[1] * k, v[2] * k]
    } else {
        v
    }
}

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

/// A model to draw: renderer name, file and scale.
type ModelRef = (&'static str, &'static str, f64);

/// A fairing shell from `bottom` up over a payload reaching `top` (heights along the craft's axis, m) at
/// `radius`: unit tube rings (fx:fairing_ring) stacked straight, then the rounded nose (fx:fairing_nose), each
/// drawn at scale `radius` from the height given.
pub fn fairing_shells(bottom: f64, top: f64, radius: f64) -> Vec<(&'static str, f64)> {
    // Straight rings up to the payload's top (less the nose's rounded shoulder), then the nose.
    let rings = ((top - bottom - 0.4 * radius) / radius).ceil().max(0.0) as usize;
    let mut shells: Vec<(&'static str, f64)> = (0..rings)
        .map(|k| ("fx:fairing_ring", bottom + k as f64 * radius))
        .collect();
    shells.push(("fx:fairing_nose", bottom + rings as f64 * radius));
    shells
}

/// The pose of an animated Kerbal model playing one of its clips (idle, walk, run, air), looping in page
/// time; `None` for any other model.
fn kerbal_sample(
    gpu: &Renderer,
    model: &str,
    clip: &str,
    now_ms: f64,
) -> Option<crate::animation::ClipSample> {
    if !model.starts_with("kerbal:anim") {
        return None;
    }
    let rig = &gpu.model(model)?.rig;
    let i = rig.clips.iter().position(|c| c.name == clip)?;
    let duration = rig.clips[i].duration.max(1e-3);
    Some(crate::animation::ClipSample {
        clip: i,
        time: ((now_ms / 1000.0) % duration as f64) as f32,
        looping: true,
    })
}

/// A control surface's hinge: the bone that turns and its axis in the original (Data_ControlSurface
/// CtrlSurfacePivotTransformName, CtrlTransformRotAxis; every stock surface has sign +1) where the export has it.
fn control_hinge(part_id: &str) -> Option<(&'static str, char)> {
    Some(match part_id {
        "controlsurface_1v_procedural_light"
        | "controlsurface_2v_procedural_heavy"
        | "controlsurface_3v_procedural_spaceplane" => ("bone_wing_pivot_ach", 'Y'),
        "gridfin_1v_radial" | "gridfin_2v_radial" => ("bone_rotator", 'Y'),
        // The wings' and stabilizers' ailerons hinge on bones of the original's procedural rig, which the
        // exports do not carry; their flaps stay put for now.
        _ => return None,
    })
}

/// The turn for a hinge deflected by `rad` about its Unity axis, in the exported (X-mirrored) frame:
/// mirroring X keeps a turn about X and reverses one about Y or Z.
fn hinge_turn(axis: char, rad: f32) -> glam::Quat {
    match axis {
        'X' => glam::Quat::from_rotation_x(rad),
        'Y' => glam::Quat::from_rotation_y(-rad),
        _ => glam::Quat::from_rotation_z(-rad),
    }
}

/// The original's most common Kerbal skin colour (looks SKINCOLOR BADA55).
pub const DEFAULT_SKIN: [u8; 3] = [0xBA, 0xDA, 0x55];

/// What stands in for a vessel the flight view has no craft for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stand {
    Craft,
    /// `round`: the Kerbal's looks are TYPE CIRCLE, which wears the female head and eyes; `skin` is their
    /// SKINCOLOR, which tints the original's grey head texture.
    Kerbal {
        round: bool,
        skin: [u8; 3],
    },
    Flag,
}

impl Stand {
    /// A Kerbal on EVA reads `eva` in its flight readout; a planted flag is named "<agency> flag".
    pub fn of(v: &ose_net::VesselState) -> Stand {
        if v.flight.as_ref().is_some_and(|f| f.eva) {
            Stand::Kerbal {
                round: false,
                skin: DEFAULT_SKIN,
            }
        } else if v.name.ends_with(" flag") {
            Stand::Flag
        } else {
            Stand::Craft
        }
    }

    /// The colour a model of this stand is drawn in: a Kerbal's head takes their skin colour (linear light).
    fn tint(self, model: &str) -> Option<[f32; 4]> {
        match self {
            Stand::Kerbal { skin, .. } if model.starts_with("kerbal:head") => {
                let c = |v: u8| crate::render::srgb_to_linear(v as f64 / 255.0) as f32;
                Some([c(skin[0]), c(skin[1]), c(skin[2]), 1.0])
            }
            _ => None,
        }
    }

    /// The models to draw (name, file, scale), requested from the renderer: the original's Kerbal (suit, head,
    /// eyes, helmet, tools/export_kerbal.py) and planted flag, with the procedural stand-ins while they load
    /// or when a server has no game assets.
    fn models(self, gpu: &mut Renderer) -> Vec<ModelRef> {
        // The animated Kerbal (tools/export_kerbal_anim.py): one skinned model with the head, eyes and helmet
        // on its bones and the original's EVA clips; the rigid pieces stand in while it loads.
        if let Stand::Kerbal { round, .. } = self {
            let anim: ModelRef = if round {
                ("kerbal:anim_f", "assets/kerbal/kerbal_f.glb", 1.0)
            } else {
                ("kerbal:anim_m", "assets/kerbal/kerbal_m.glb", 1.0)
            };
            gpu.request_model(anim.0, anim.1);
            if gpu.has_model(anim.0) {
                return vec![anim];
            }
        }
        let (real, stand_in): (&[ModelRef], ModelRef) = match self {
            Stand::Kerbal { round: false, .. } => (
                &[
                    ("kerbal:body", "assets/kerbal/body_spacesuit_01.glb", 1.0),
                    ("kerbal:head", "assets/kerbal/head_m_01.glb", 1.0),
                    ("kerbal:eyes", "assets/kerbal/eyes_m_01.glb", 1.0),
                    ("kerbal:helmet", "assets/kerbal/helm_spacesuit_01.glb", 1.0),
                ],
                ("fx:kerbal", "assets/fx/kerbal.glb", 1.0),
            ),
            Stand::Kerbal { round: true, .. } => (
                &[
                    ("kerbal:body", "assets/kerbal/body_spacesuit_01.glb", 1.0),
                    ("kerbal:head_f", "assets/kerbal/head_f_01.glb", 1.0),
                    ("kerbal:eyes_f", "assets/kerbal/eyes_f_01.glb", 1.0),
                    ("kerbal:helmet", "assets/kerbal/helm_spacesuit_01.glb", 1.0),
                ],
                ("fx:kerbal", "assets/fx/kerbal.glb", 1.0),
            ),
            Stand::Flag => (
                &[("kerbal:flag", "assets/kerbal/flag_portable.glb", 1.5)],
                ("fx:flag", "assets/fx/flag.glb", 1.0),
            ),
            Stand::Craft => (
                &[],
                (
                    "part:probe_0v_octagonal_electricity_1x1",
                    "assets/parts/probe_0v_octagonal_electricity_1x1.glb",
                    1.0,
                ),
            ),
        };
        for (name, file, _) in real {
            gpu.request_model(name, file);
        }
        if !real.is_empty() && real.iter().all(|(name, _, _)| gpu.has_model(name)) {
            return real.to_vec();
        }
        gpu.request_model(stand_in.0, stand_in.1);
        if gpu.has_model(stand_in.0) {
            vec![stand_in]
        } else {
            Vec::new()
        }
    }
}

/// Open canopy diameter for a chute part, m: the Mk16-XL main, the radial Mk16-R and the drogue.
fn canopy_diameter(part_id: &str) -> f64 {
    match part_id {
        "parachute_1v" => 12.0,
        "parachute_1v_radial" => 8.0,
        "parachute_0v_radial_drogue" => 5.0,
        _ => 8.0,
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
        // the server turns toward the burn; the readout's facing shows it
        AttitudeMode::Maneuver => or_up(v_rel),
        // SAS off holds nothing; the readout's facing shows where it points
        AttitudeMode::Free | AttitudeMode::Target | AttitudeMode::AntiTarget => or_up(v_rel),
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

/// The original's flight camera modes (`KSP.Sim/FlightCameraMode.cs`), cycled with V.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraMode {
    /// Free near the ground and in the air, orbital above the atmosphere.
    #[default]
    Auto,
    /// Turns with the local horizon: up is away from the body.
    Free,
    /// Up is the orbit normal, so the orbit stays level on screen.
    Orbital,
    /// Behind the vessel along its velocity.
    Chase,
    /// Turns with the vessel.
    Locked,
}

impl CameraMode {
    pub fn next(self) -> CameraMode {
        match self {
            CameraMode::Auto => CameraMode::Free,
            CameraMode::Free => CameraMode::Orbital,
            CameraMode::Orbital => CameraMode::Chase,
            CameraMode::Chase => CameraMode::Locked,
            CameraMode::Locked => CameraMode::Auto,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CameraMode::Auto => "Auto",
            CameraMode::Free => "Free",
            CameraMode::Orbital => "Orbital",
            CameraMode::Chase => "Chase",
            CameraMode::Locked => "Locked",
        }
    }
}

pub struct FlightView {
    pub mode: CameraMode,
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
    /// A walking Kerbal: last model-frame position and time (ms), smoothed speed (m/s) and stride phase.
    walk_last: Option<(DVec3, f64)>,
    walk_speed: f64,
    walk_phase: f64,
    deployments: HashMap<(u64, usize), crate::animation::Deployment>,
    gimbals: HashMap<(u64, usize), crate::gimbal::Driver>,
    /// Craft axes and local horizon of the last frame, for the navball.
    /// Height above the ground under the vessel (the altimeter's terrain mode), once the body's map is loaded.
    pub agl_m: Option<f64>,
    /// Part boxes relative to the camera (craft part index, min, max) and the camera's forward and up from
    /// the last frame, for picking a part under the pointer (the part window on right click).
    pick_boxes: Vec<(usize, DVec3, DVec3)>,
    pick_cam: (DVec3, DVec3),
    /// Terrain, ocean, sky and clouds of the body (planet.rs), created with the first frame.
    planet: Option<crate::planet::PlanetRenderer>,
    fx_nozzles: std::rc::Rc<std::cell::RefCell<HashMap<String, Vec<Nozzle>>>>,
    fx_requested: bool,
    fx_context: Option<(u64, String)>,
    ground_radius: f64,
    launch_origin: Option<DVec3>,
    crash_fx: crate::particles::CrashFx,
    effect_edges: crate::particles::EffectEdges,
    bursts: crate::particles::Bursts,
    last_surface_speed: f64,
    last_footstep: f64,
}

impl Default for FlightView {
    fn default() -> Self {
        Self::new()
    }
}

/// What the flight view needs from the app for one frame.
pub struct FlightInput<'a> {
    /// Smoothed inertial nose/top for presentation; the vessel readout remains authoritative.
    pub attitude: Option<(DVec3, DVec3)>,
    pub vessel: &'a VesselState,
    /// Vessel and body star-space positions, body star-space velocity, at `ut`.
    pub vessel_pos: DVec3,
    /// Other vessels' star-space positions and what they are, drawn when they are near.
    pub nearby_vessels: &'a [(DVec3, Stand)],
    /// What the flown vessel is when it has no craft: a Kerbal on EVA, a flag, or an unknown craft.
    pub stand: Stand,
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
    /// Planned maneuver for this vessel: prograde, normal and radial delta-v, m/s.
    pub maneuver: Option<(f64, f64, f64)>,
    /// Direction to the target vessel in the body's inertial frame, for the navball's target markers.
    pub target: Option<DVec3>,
    /// Body spin from the server's `BodyRotation` (protocol 9); the embedded system file otherwise.
    pub spin: Option<Spin>,
    /// Wall clock, ms (flex interpolation).
    pub now_ms: f64,
    /// Every other body that can hide the sun: star-space position and radius (m), and the star's radius.
    pub others: &'a [(DVec3, f64)],
    pub star_radius: f64,
}

fn quat_from_basis(x: DVec3, y: DVec3, z: DVec3) -> DQuat {
    DQuat::from_mat3(&DMat3::from_cols(x, y, z)).normalize()
}

impl FlightView {
    pub fn new() -> FlightView {
        FlightView {
            mode: CameraMode::Auto,
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
            walk_last: None,
            walk_speed: 0.0,
            walk_phase: 0.0,
            deployments: HashMap::new(),
            gimbals: HashMap::new(),
            agl_m: None,
            pick_boxes: Vec::new(),
            pick_cam: (DVec3::NEG_Z, DVec3::Y),
            planet: None,
            fx_nozzles: Default::default(),
            fx_requested: false,
            fx_context: None,
            ground_radius: 0.0,
            launch_origin: None,
            crash_fx: Default::default(),
            effect_edges: Default::default(),
            bursts: Default::default(),
            last_surface_speed: 0.0,
            last_footstep: 0.0,
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
            out.insert(
                part,
                (
                    cap(lerp(a.0, b.0), FLEX_MAX_RAD),
                    cap(lerp(a.1, b.1), FLEX_MAX_M),
                ),
            );
        }
        out
    }

    pub fn orbit(&mut self, dx: f64, dy: f64) {
        self.yaw -= dx * 0.005;
        self.pitch = (self.pitch + dy * 0.005).clamp(-1.45, 1.45);
    }

    /// The craft part under a pixel of the last frame (nearest box along the camera ray).
    pub fn pick(&self, gpu: &Renderer, x_px: f64, y_px: f64) -> Option<usize> {
        let (forward, up) = self.pick_cam;
        let right = forward.cross(up).normalize_or_zero();
        let cam_up = right.cross(forward).normalize_or_zero();
        let aspect = gpu.width as f64 / gpu.height.max(1) as f64;
        let ndc_x = x_px / gpu.width.max(1) as f64 * 2.0 - 1.0;
        let ndc_y = 1.0 - y_px / gpu.height.max(1) as f64 * 2.0;
        let t = (60f64.to_radians() * 0.5).tan();
        let dir = (forward + right * (ndc_x * t * aspect) + cam_up * (ndc_y * t)).normalize();
        let mut best: Option<(f64, usize)> = None;
        for (i, lo, hi) in &self.pick_boxes {
            if let Some(d) = crate::ksc3d::ray_box(DVec3::ZERO, dir, *lo, *hi) {
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, *i));
                }
            }
        }
        best.map(|(_, i)| i)
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

    /// Direction to the sun in the KSC campus frame (glTF, Y up) at `ut`, given the inertial direction from
    /// The campus footprint (all facility boxes, 300 m wider) as a scatter exclusion in the model frame.
    fn ksc_exclusion(&self, pos: DVec3, rot: DQuat) -> Option<crate::planet_scatter::Exclusion> {
        let scene = self.ksc_scene.borrow();
        let scene = scene.as_ref()?;
        let (mut x0, mut x1, mut z0, mut z1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for b in &scene.buildings {
            x0 = x0.min(b.min[0]);
            x1 = x1.max(b.max[0]);
            z0 = z0.min(b.min[2]);
            z1 = z1.max(b.max[2]);
        }
        if x0 > x1 {
            return None;
        }
        let m = 300.0;
        let ax = rot * DVec3::X;
        let az = rot * DVec3::Z;
        Some(crate::planet_scatter::Exclusion {
            origin: pos.to_array(),
            ax: ax.to_array(),
            az: az.to_array(),
            x: [x0 - m, x1 + m],
            z: [z0 - m, z1 + m],
        })
    }

    /// Kerbin to the sun. `None` until the campus placement is known.
    pub fn ksc_sun(
        &mut self,
        body_radius: f64,
        to_sun: DVec3,
        spin: Option<Spin>,
        ut: f64,
    ) -> Option<DVec3> {
        let spin = spin.or_else(|| self.spins.get("Kerbin").copied())?;
        let (_, krot) = self.ksc_pose(body_radius)?;
        // An inertial point on the pad: body-fixed latitude and longitude turned back by the spin.
        let (lat, lon, alt) = KSC_PADS[0];
        let ang = (lon.to_radians()) + spin.angle(ut);
        let (la, rr) = (lat.to_radians(), body_radius + alt);
        let r = DVec3::new(la.cos() * ang.cos(), la.cos() * ang.sin(), la.sin()) * rr;
        let d = dir_to_model(r, to_sun.normalize_or_zero(), spin, ut);
        Some((krot.inverse() * d).normalize_or_zero())
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
        self.agl_m = self
            .planet
            .as_ref()
            .and_then(|p| p.ground_m(inp.body_id, up))
            .map(|g| r.length() - inp.body_radius - g);
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
        let (facing, roll) = inp.attitude.unwrap_or_else(|| match &inp.vessel.flight {
            Some(f) if g(f.facing).length() > 0.5 => (g(f.facing), g(f.roll_ref)),
            _ => (dir_i, DVec3::ZERO),
        });
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

        // The camera's frame for the mode: (forward-ish, right-ish, up) axes the yaw and pitch turn around.
        let alt_m = r.length() - inp.body_radius;
        let mode = match self.mode {
            CameraMode::Auto if alt_m > inp.atmosphere_m.max(30_000.0) => CameraMode::Orbital,
            CameraMode::Auto => CameraMode::Free,
            m => m,
        };
        let orbit_up = dir_to_model(r, r.cross(v_rel), spin, inp.ut).normalize_or_zero();
        let v_surf_m = dir_to_model(
            r,
            v_rel - DVec3::new(0.0, 0.0, spin.omega()).cross(r),
            spin,
            inp.ut,
        )
        .normalize_or_zero();
        let frame = |b_up: DVec3, hint: DVec3| {
            let b_n = (hint - b_up * b_up.dot(hint)).normalize_or_zero();
            (b_up.length() > 0.5 && b_n.length() > 0.5).then(|| (b_n, b_up.cross(b_n), b_up))
        };
        let (b_n, b_e, b_u) = match mode {
            CameraMode::Orbital => frame(orbit_up, north),
            // Behind the vessel: the default yaw looks along the velocity.
            CameraMode::Chase => frame(up, -v_surf_m).or_else(|| frame(up, -dir)),
            CameraMode::Locked => frame(vessel_rot * DVec3::Y, vessel_rot * DVec3::Z),
            _ => None,
        }
        .unwrap_or((north, east, up));
        let cam_up = b_u;
        let offs = self.pitch.cos() * (self.yaw.cos() * b_n + self.yaw.sin() * b_e)
            + self.pitch.sin() * b_u;
        let cam = pos + offs * self.distance;
        let alt = r.length() - inp.body_radius;
        let near = (self.distance * 0.02).clamp(0.3, 5_000.0);
        let far = (self.distance * 50.0).clamp(4.0e6, 5.0e8);
        gpu.set_camera(-offs, cam_up, 60f64.to_radians(), near, far);
        self.pick_cam = (-offs.normalize_or_zero(), cam_up);
        self.pick_boxes.clear();

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
            self.crash_fx = Default::default();
            self.effect_edges = Default::default();
            self.bursts = Default::default();
            self.last_surface_speed = 0.0;
            self.last_footstep = inp.ut;
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
        if let Some(f) = &inp.vessel.flight {
            let crashed = f.situation == ose_net::FlightSituation::Crashed;
            // The source's surface-impact event is separate from its fuel explosion.
            if self.effect_edges.rising(0, 0, landed || crashed) && self.last_surface_speed > 1.0 {
                let environment = if f.atmosphere_density > 0.01 {
                    "atmosphere"
                } else {
                    "vacuum"
                };
                let surface =
                    if matches!(inp.body_id, "Kerbin" | "Laythe" | "Eve") && f.altitude_m < 1.0 {
                        "liquid"
                    } else {
                        "solid"
                    };
                self.bursts.queue(
                    format!("fx_surfaceImpact_{surface}_noFuel_{environment}"),
                    glam::DMat4::from_rotation_translation(
                        DQuat::from_rotation_arc(DVec3::Y, up),
                        pos + up,
                    ),
                    inp.ut,
                    self.last_surface_speed,
                );
            }
            if let Some(hit) = self.crash_fx.update_fuel(
                crashed,
                crate::particles::fuel_type(&f.resources, f.propellant_t),
                inp.ut,
                pos.to_array(),
            ) {
                let impact = DVec3::from_array(hit.position);
                engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                    gimbal: DQuat::IDENTITY,
                    instance: 0x2000000000000000 | inp.vessel.id,
                    part: if f.eva {
                        "fx_kerbal_poof_default".into()
                    } else {
                        format!(
                            "fx_explosion_{}_{}",
                            hit.fuel,
                            if f.atmosphere_density > 0.01 {
                                "atmosphere"
                            } else {
                                "vacuum"
                            }
                        )
                    },
                    model: glam::DMat4::from_rotation_translation(
                        DQuat::from_rotation_arc(DVec3::Y, impact.normalize_or_zero()),
                        impact + impact.normalize_or_zero() - cam,
                    ),
                    throttle: 1.0,
                    commanded: 1.0,
                    pressure: crate::particles::pressure_multiplier(f.static_pressure_kpa),
                    atmosphere_density: f.atmosphere_density,
                    gravity_mps2: f.gravity_mps2,
                    relative_velocity: f.surface_speed_mps,
                    mode: 0,
                    time: inp.now_ms * 0.001,
                    simulation_time: inp.ut,
                    vapor: None,
                    reentry: None,
                    thrust_kn: 0.0,
                    distance_from_spawn: 0.0,
                    vertical_deviation: 1.0,
                });
            }
        }

        // The body: the planet renderer's terrain, ocean, sky and clouds once its assets are in; until then the
        // scaled sphere, sunk by 1.5 km so its facets stay under the campus ground near the KSC.
        // Bodies that can hide the sun from here, in the model frame, biggest in the sky first.
        let mut occluders: Vec<(DVec3, f64)> = inp
            .others
            .iter()
            .map(|(p, rad)| (to_model(*p - inp.body_pos, spin, inp.ut), *rad))
            .collect();
        occluders.sort_by(|a, b| {
            let sa = b.1 / (b.0 - cam).length().max(1.0);
            let sb = a.1 / (a.0 - cam).length().max(1.0);
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        });
        occluders.truncate(4);
        let sun_radius_rad = (inp.star_radius / (inp.star_pos - inp.vessel_pos).length().max(1.0))
            .clamp(0.0, 1.0)
            .asin();
        let planet = self
            .planet
            .get_or_insert_with(|| crate::planet::PlanetRenderer::new(gpu));
        let planet_ready = planet.prepare(
            gpu,
            &crate::planet::PlanetView {
                body_id: inp.body_id,
                radius_m: inp.body_radius,
                cam,
                sun,
                time_s: inp.now_ms / 1000.0,
                occluders: &occluders,
                sun_radius_rad,
            },
        );
        let body_model = format!("body:{}", inp.body_id);
        if !planet_ready {
            gpu.request_model(&body_model, &format!("assets/bodies/{}.glb", inp.body_id));
        }
        if let Some(m) = gpu.model(&body_model).filter(|_| !planet_ready) {
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
                // No trees, rocks or grass on the campus, its runways and pads (the original draws the campus
                // over its own ground): the facilities' footprint, 300 m wider all round.
                if let Some(ex) = self.ksc_exclusion(kpos, krot) {
                    if let Some(p) = self.planet.as_mut() {
                        p.set_exclusions(vec![ex]);
                    }
                }
                gpu.request_model("ksc", "assets/ksc/ksc.glb");
                if gpu.has_model("ksc") {
                    let mut it = DrawItem::lit("ksc".into(), kpos - cam, krot, 1.0, sun);
                    it.ambient = 0.38;
                    items.push(it);
                }
            }
        }
        // Parts in space get more ambient: with only the sun, a craft between it and the camera (or in the
        // planet's shadow) went black against black, and players could not find their probes in orbit.
        let part_ambient = if alt > inp.atmosphere_m.max(20_000.0) {
            0.45
        } else {
            0.25
        };
        // How much of the star the craft sees: a moon or planet in front of it dims its parts as it does the
        // ground (planet.rs, the planet thread's eclipse test).
        let sun_share = crate::planet::eclipse_visible(pos, sun, &occluders, sun_radius_rad) as f32;
        // Air streams past opposite the surface velocity; open chutes trail into it.
        let trail = {
            let v_surf = v_rel - DVec3::new(0.0, 0.0, spin.omega()).cross(r);
            if v_surf.length() > 1.0 {
                dir_to_model(r, -v_surf, spin, inp.ut)
            } else {
                up
            }
        };
        let readout = inp.vessel.flight.as_ref();
        // The vessel.
        if let Some(craft) = inp.craft {
            let vessel_rot = vessel_rot * crate::orientation::mesh_to_vessel();
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
            // Simulation position is the wet centre of mass, including on the pad.
            let origin = readout
                .map(|f| crate::orientation::mesh_origin(f.centre_of_mass_m))
                .unwrap_or(DVec3::ZERO);
            let mut gimbal_rotations = HashMap::new();
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
                let mut animation = None;
                if let Some(model) = gpu.model(&name) {
                    if let Some((clip, _)) = model
                        .rig
                        .deployment
                        .filter(|_| model.rig.default_sample().is_some())
                    {
                        let deployed = inp
                            .vessel
                            .flight
                            .as_ref()
                            .and_then(|f| {
                                f.gear
                                    .iter()
                                    .chain(&f.panels)
                                    .find(|(p, _)| *p as usize == i)
                                    .map(|(_, out)| *out)
                            })
                            .unwrap_or(false);
                        let driver = self
                            .deployments
                            .entry((inp.vessel.id, i))
                            .or_insert_with(|| crate::animation::Deployment::new(deployed, inp.ut));
                        let fraction =
                            driver.update(deployed, inp.ut, model.rig.clips[clip].duration);
                        animation = model.rig.deployment_sample(fraction);
                    } else if p.part_id.starts_with("cargobay_") {
                        // The bays' controllers have several clips (open, close, the "min" variants);
                        // the open clip runs from shut to open.
                        let open_clip =
                            model.rig.clips.iter().position(|c| {
                                c.name.ends_with("_open") && !c.name.contains("_min")
                            });
                        if let Some(clip) = open_clip {
                            let open = readout
                                .and_then(|f| f.cargo_bays.iter().find(|b| b.0 as usize == i))
                                .is_some_and(|b| b.1);
                            let driver = self
                                .deployments
                                .entry((inp.vessel.id, i))
                                .or_insert_with(|| crate::animation::Deployment::new(open, inp.ut));
                            let duration = model.rig.clips[clip].duration;
                            let fraction = driver.update(open, inp.ut, duration);
                            animation = Some(crate::animation::ClipSample {
                                clip,
                                time: duration * fraction,
                                looping: false,
                            });
                        }
                    }
                }
                if let Some(definition) = gpu.model(&name).and_then(|m| m.rig.gimbal.as_ref()) {
                    let direction = readout
                        .and_then(|f| f.engine_gimbals.iter().find(|g| g.0 as usize == i))
                        .map(|(_, d)| DVec3::new(d.x, d.y, d.z))
                        .unwrap_or(DVec3::Y);
                    let driver = self
                        .gimbals
                        .entry((inp.vessel.id, i))
                        .or_insert_with(|| crate::gimbal::Driver::new(inp.ut));
                    gimbal_rotations.insert(
                        i,
                        driver.update(direction, inp.ut, definition.response_speed),
                    );
                }
                parts.push((i, name, lp, lq, animation));
            }
            if let Some(f) = readout {
                let suffix = if f.atmosphere_density > 0.01 {
                    "atm"
                } else {
                    "vacuum"
                };
                let fired_parts: Vec<_> = craft
                    .stages
                    .iter()
                    .rev()
                    .filter(|s| !s.is_empty())
                    .take(fired)
                    .flatten()
                    .copied()
                    .collect();
                for (i, p) in craft.parts.iter().enumerate() {
                    let (pp, q) = poses[i];
                    let matrix = glam::DMat4::from_rotation_translation(
                        vessel_rot * DQuat::from_xyzw(q.x, -q.y, -q.z, q.w).normalize(),
                        pos + vessel_rot * (DVec3::new(-pp.x, pp.y, pp.z) + origin),
                    );
                    let separated = gone.get(i).copied().unwrap_or(false);
                    if self.effect_edges.rising(1, i, separated) && (inp.is_decoupler)(&p.part_id) {
                        self.bursts.queue(
                            format!("fx_decouple_{suffix}"),
                            matrix,
                            inp.ut,
                            f.surface_speed_mps,
                        );
                    }
                    let fairing = f.fairings.iter().any(|x| x.0 as usize == i && x.1);
                    if self.effect_edges.rising(2, i, fairing) {
                        self.bursts.queue(
                            format!("fx_decouple_{suffix}"),
                            matrix,
                            inp.ut,
                            f.surface_speed_mps,
                        );
                    }
                    let chute = f.parachutes.iter().any(|x| {
                        x.0 as usize == i
                            && matches!(
                                x.1,
                                ose_net::ChuteState::SemiDeployed | ose_net::ChuteState::Deployed
                            )
                    });
                    if self.effect_edges.rising(3, i, chute) {
                        self.bursts.queue(
                            format!("fx_parachutedeploy_{suffix}"),
                            matrix,
                            inp.ut,
                            f.surface_speed_mps,
                        );
                    }
                    if self.effect_edges.rising(4, i, fired_parts.contains(&i))
                        && p.part_id.contains("launchclamp")
                    {
                        self.bursts.queue(
                            format!("fx_launchclamp_{suffix}"),
                            matrix,
                            inp.ut,
                            f.surface_speed_mps,
                        );
                    }
                    let docked = f
                        .docked
                        .iter()
                        .any(|(a, b)| *a as usize == i || *b as usize == i);
                    if self.effect_edges.rising(5, i, docked) {
                        self.bursts
                            .queue("fx_dock_00", matrix, inp.ut, f.surface_speed_mps);
                    }
                    if self.effect_edges.rising(6, i, !docked) {
                        self.bursts
                            .queue("fx_undock_00", matrix, inp.ut, f.surface_speed_mps);
                    }
                }
            }
            // Use the server's attached, fuel-fed engines, not just stage membership. Nozzles share
            // the exact flexed pose used by the part mesh (including radial/multi-nozzle engines).
            if let Some(f) = &inp.vessel.flight {
                let pressure = crate::particles::pressure_multiplier(f.static_pressure_kpa);
                let nozzles = self.fx_nozzles.borrow();
                let mut launch_smoke_added = false;
                for (pi, name, lp, lq, _) in &parts {
                    let part_id = &craft.parts[*pi].part_id;
                    if let Some((_, coefficient, lift_kn)) = f
                        .lifting_surfaces
                        .iter()
                        .find(|s| s.0 as usize == *pi)
                        .filter(|_| {
                            part_id.starts_with("wing_") || part_id.starts_with("controlsurface_")
                        })
                    {
                        if let Some(model) = gpu.model(name) {
                            // OSE's current wings use their authored static shape. Anchor the ribbon
                            // at that shape's outer trailing corner and use the exact rendered pose.
                            let (lo, hi) =
                                (model.bounds_min.as_dvec3(), model.bounds_max.as_dvec3());
                            let tip = DVec3::new(
                                if lo.x.abs() > hi.x.abs() { lo.x } else { hi.x },
                                (lo.y + hi.y) * 0.5,
                                hi.z,
                            );
                            engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                                gimbal: DQuat::IDENTITY,
                                instance: 0x0040000000000000 | (inp.vessel.id << 16) | *pi as u64,
                                part: "fx_vapor_vortices".into(),
                                model: glam::DMat4::from_rotation_translation(
                                    vessel_rot * *lq,
                                    pos - cam + vessel_rot * (*lp + origin + *lq * tip),
                                ),
                                throttle: if f.situation == ose_net::FlightSituation::Crashed {
                                    0.0
                                } else {
                                    1.0
                                },
                                commanded: 0.0,
                                pressure,
                                atmosphere_density: f.atmosphere_density,
                                gravity_mps2: f.gravity_mps2,
                                relative_velocity: f.surface_speed_mps,
                                mode: 0,
                                time: inp.now_ms * 0.001,
                                simulation_time: inp.ut,
                                thrust_kn: 0.0,
                                distance_from_spawn: 0.0,
                                vertical_deviation: 1.0,
                                reentry: None,
                                vapor: Some(crate::vapor::State {
                                    body: crate::vapor::BODIES
                                        .iter()
                                        .position(|body| *body == inp.body_id)
                                        .unwrap_or(usize::MAX),
                                    altitude: f.altitude_m,
                                    temperature: f.atmosphere_temperature_k,
                                    density: f.atmosphere_density,
                                    pressure: f.static_pressure_kpa,
                                    dynamic_pressure: f.dynamic_pressure_kpa,
                                    lift: *lift_kn as f64,
                                    coefficient: *coefficient as f64,
                                }),
                            });
                        }
                    }
                    let air_velocity = v_rel - DVec3::new(0.0, 0.0, spin.omega()).cross(r);
                    engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                        gimbal: DQuat::IDENTITY,
                        instance: 0x0080000000000000 | (inp.vessel.id << 16) | *pi as u64,
                        part: format!("reentry:{part_id}"),
                        model: glam::DMat4::from_rotation_translation(
                            vessel_rot * *lq,
                            pos - cam + vessel_rot * (*lp + origin),
                        ),
                        throttle: if f.situation == ose_net::FlightSituation::Crashed {
                            0.0
                        } else {
                            1.0
                        },
                        commanded: 0.0,
                        pressure,
                        atmosphere_density: f.atmosphere_density,
                        gravity_mps2: f.gravity_mps2,
                        relative_velocity: f.surface_speed_mps,
                        mode: 0,
                        time: inp.now_ms * 0.001,
                        simulation_time: inp.ut,
                        thrust_kn: 0.0,
                        distance_from_spawn: 0.0,
                        vertical_deviation: 1.0,
                        vapor: None,
                        reentry: Some(crate::reentry::State {
                            heading: dir_to_model(
                                r,
                                air_velocity.normalize_or_zero(),
                                spin,
                                inp.ut,
                            ),
                            density: f.atmosphere_density,
                            altitude: f.altitude_m,
                            speed: f.surface_speed_mps,
                            mach: f.mach_number,
                            gradient: match inp.body_id {
                                "Eve" => 1,
                                "Jool" => 2,
                                "Laythe" => 3,
                                _ => 0,
                            },
                        }),
                    });
                    if part_id.starts_with("wheel_") || part_id.starts_with("landinggear_") {
                        let deployed = f
                            .gear
                            .iter()
                            .find(|x| x.0 as usize == *pi)
                            .is_none_or(|x| x.1);
                        engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                            gimbal: DQuat::IDENTITY,
                            instance: 0x0200000000000000 | (inp.vessel.id << 16) | *pi as u64,
                            part: "fx_wheel_default".into(),
                            model: glam::DMat4::from_rotation_translation(
                                DQuat::from_rotation_arc(DVec3::Y, up),
                                pos - cam + vessel_rot * (*lp + origin),
                            ),
                            throttle: if landed
                                && deployed
                                && !f.brakes
                                && f.drive_speed_mps.abs() > 0.1
                            {
                                1.0
                            } else {
                                0.0
                            },
                            commanded: 0.0,
                            pressure,
                            atmosphere_density: f.atmosphere_density,
                            gravity_mps2: f.gravity_mps2,
                            relative_velocity: f.drive_speed_mps.abs() as f64,
                            mode: 0,
                            time: inp.now_ms * 0.001,
                            simulation_time: inp.ut,
                            vapor: None,
                            reentry: None,
                            thrust_kn: 0.0,
                            distance_from_spawn: 0.0,
                            vertical_deviation: 1.0,
                        });
                    }
                    if part_id.starts_with("rcs_") {
                        if let Some(jets) = nozzles.get(part_id) {
                            let pose = vessel_rot * *lq;
                            let translation =
                                pose.inverse() * dir_to_model(r, g(f.rcs_translate), spin, inp.ut);
                            let rotation =
                                pose.inverse() * dir_to_model(r, g(f.rcs_rotate), spin, inp.ut);
                            for nozzle in jets {
                                let throttle = crate::particles::rcs_nozzle_throttle(
                                    *lp + *lq * DVec3::from_array(nozzle.position),
                                    *lq * DVec3::from_array(nozzle.direction),
                                    *lq * translation,
                                    *lq * rotation,
                                    false,
                                    false,
                                );
                                engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                                    gimbal: DQuat::IDENTITY,
                                    instance: 0x0800000000000000
                                        | (inp.vessel.id << 24)
                                        | ((*pi as u64) << 8)
                                        | nozzle.mode as u64,
                                    part: part_id.clone(),
                                    model: glam::DMat4::from_rotation_translation(
                                        pose,
                                        pos - cam + vessel_rot * (*lp + origin),
                                    ),
                                    throttle,
                                    commanded: throttle,
                                    pressure,
                                    atmosphere_density: f.atmosphere_density,
                                    gravity_mps2: f.gravity_mps2,
                                    relative_velocity: f.surface_speed_mps,
                                    mode: nozzle.mode,
                                    time: inp.now_ms * 0.001,
                                    simulation_time: inp.ut,
                                    vapor: None,
                                    reentry: None,
                                    thrust_kn: f.rcs_thrust_kn as f64 * throttle,
                                    distance_from_spawn: 0.0,
                                    vertical_deviation: 1.0,
                                });
                            }
                        }
                    }
                    if part_id.starts_with("engine_") || part_id.starts_with("booster_") {
                        engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                            gimbal: gimbal_rotations.get(pi).copied().unwrap_or(DQuat::IDENTITY),
                            instance: (inp.vessel.id << 16) | *pi as u64,
                            part: part_id.clone(),
                            model: glam::DMat4::from_rotation_translation(
                                vessel_rot * *lq,
                                pos - cam + vessel_rot * (*lp + origin),
                            ),
                            throttle: crate::particles::exhaust_throttle(
                                f.firing_engines.contains(&(*pi as u16)),
                                f.thrust_kn,
                                f.throttle,
                                part_id.starts_with("booster_"),
                                f.situation == ose_net::FlightSituation::Crashed,
                            ),
                            pressure,
                            atmosphere_density: f.atmosphere_density,
                            gravity_mps2: f.gravity_mps2,
                            relative_velocity: f.surface_speed_mps,
                            mode: 0,
                            time: inp.now_ms * 0.001,
                            simulation_time: inp.ut,
                            commanded: f.throttle,
                            vapor: None,
                            reentry: None,
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
                        mode: 0,
                        rotate: false,
                    }];
                    let jets = nozzles.get(part_id).map(Vec::as_slice).unwrap_or(&fallback);
                    let width = ((hi.x - lo.x).min(hi.z - lo.z) * 0.55
                        / (jets.len().max(1) as f64).sqrt())
                    .clamp(0.08, 12.0);
                    let gimbal = gimbal_rotations.get(pi).copied().unwrap_or(DQuat::IDENTITY);
                    let pivot_world = m.rig.world_matrices(&m.rig.pose(None));
                    let pivots: Vec<_> = m
                        .rig
                        .gimbal
                        .as_ref()
                        .map(|definition| {
                            m.rig
                                .nodes
                                .iter()
                                .enumerate()
                                .filter(|(_, n)| definition.controls(&n.name))
                                .map(|(i, _)| pivot_world[i].w_axis.truncate().as_dvec3())
                                .collect()
                        })
                        .unwrap_or_default();
                    for (j, nozzle) in jets.iter().enumerate() {
                        let nozzle_position = DVec3::from_array(nozzle.position);
                        let pivot = pivots
                            .iter()
                            .min_by(|a, b| {
                                a.distance_squared(nozzle_position)
                                    .total_cmp(&b.distance_squared(nozzle_position))
                            })
                            .copied()
                            .unwrap_or(nozzle_position);
                        let nozzle_position = crate::gimbal::about_pivot(gimbal, pivot)
                            .transform_point3(nozzle_position);
                        let at = pos + vessel_rot * (*lp + *lq * nozzle_position + origin);
                        let direction =
                            (vessel_rot * *lq * gimbal * DVec3::from_array(nozzle.direction))
                                .normalize_or_zero();
                        let down = -direction.dot(up);
                        let height = (at.length() - self.ground_radius).max(0.0);
                        if pressure > 0.01 && down > 0.3 && height / down < width * 25.0 {
                            let ground = at + direction * (height / down);
                            let rotation = DQuat::from_rotation_arc(DVec3::Y, up);
                            engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                                gimbal: DQuat::IDENTITY,
                                instance: 0x8000000000000000
                                    | (inp.vessel.id << 24)
                                    | ((*pi as u64) << 8)
                                    | j as u64,
                                part: "fx_groundblast_atmosphere_01".into(),
                                model: glam::DMat4::from_rotation_translation(
                                    rotation,
                                    ground - cam,
                                ),
                                throttle: if part_id.starts_with("booster_") {
                                    1.0
                                } else {
                                    f.throttle
                                },
                                commanded: f.throttle,
                                pressure,
                                atmosphere_density: f.atmosphere_density,
                                gravity_mps2: f.gravity_mps2,
                                relative_velocity: f.surface_speed_mps,
                                mode: 0,
                                time: inp.now_ms * 0.001,
                                simulation_time: inp.ut,
                                vapor: None,
                                reentry: None,
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
                                    gimbal: DQuat::IDENTITY,
                                    instance: 0x4000000000000000 | inp.vessel.id,
                                    part: "fx_launchsmoke_default".into(),
                                    model: glam::DMat4::from_rotation_translation(
                                        rotation,
                                        self.launch_origin.unwrap().normalize()
                                            * self.ground_radius
                                            - cam,
                                    ),
                                    throttle: if part_id.starts_with("booster_") {
                                        1.0
                                    } else {
                                        f.throttle
                                    },
                                    commanded: f.throttle,
                                    pressure,
                                    atmosphere_density: f.atmosphere_density,
                                    gravity_mps2: f.gravity_mps2,
                                    relative_velocity: f.surface_speed_mps,
                                    mode: 0,
                                    time: inp.now_ms * 0.001,
                                    simulation_time: inp.ut,
                                    vapor: None,
                                    reentry: None,
                                    thrust_kn: f.thrust_kn,
                                    distance_from_spawn: distance,
                                    vertical_deviation: down,
                                });
                            }
                        }
                    }
                }
            }
            self.deployments
                .retain(|(v, i), _| *v == inp.vessel.id && parts.iter().any(|p| p.0 == *i));
            // The chute parts' own canopy meshes are skinned in their open pose; the canopy is drawn from the
            // chute's state instead (below).
            for name in [
                "part:parachute_1v",
                "part:parachute_1v_radial",
                "part:parachute_0v_radial_drogue",
            ] {
                gpu.hide_meshes(
                    name,
                    &[
                        "mesh_parachute_1v_m2_canopy",
                        "mesh_parachute_1v_m3_radial_canopy",
                        "mesh_parachute_0v_m1_drogue_canopy",
                    ],
                );
            }
            let chutes: HashMap<u16, (ose_net::ChuteState, f32)> = readout
                .map(|f| {
                    f.parachutes
                        .iter()
                        .map(|(i, s, o)| (*i, (*s, *o)))
                        .collect()
                })
                .unwrap_or_default();
            let heat: HashMap<u16, f32> = readout
                .map(|f| {
                    f.heat
                        .iter()
                        .map(|(i, t, max, _)| (*i, if *max > 0.0 { t / max } else { 0.0 }))
                        .collect()
                })
                .unwrap_or_default();
            for (fi, fname, flp, _, _) in &parts {
                if !craft.parts[*fi].part_id.starts_with("fairing_") {
                    continue;
                }
                let jettisoned = readout
                    .and_then(|f| f.fairings.iter().find(|x| x.0 as usize == *fi))
                    .is_some_and(|x| x.1);
                let Some(base) = gpu.model(fname) else {
                    continue;
                };
                if jettisoned {
                    continue;
                }
                let ext = (base.bounds_max - base.bounds_min).as_dvec3();
                let mut radius = ext.max_element() * 0.5;
                let bottom = flp.y + 0.05;
                // What it covers: the server's shielded parts above the base, else whatever stands on the
                // base within its width.
                let shielded = readout.map(|f| f.shielded.as_slice()).unwrap_or(&[]);
                let base_radius = radius;
                let covers = |j: usize, at: DVec3| {
                    if !shielded.is_empty() {
                        return shielded.contains(&(j as u16));
                    }
                    DVec3::new(at.x - flp.x, 0.0, at.z - flp.z).length() < base_radius * 1.2
                };
                let mut top = bottom;
                for (j, jname, jlp, jlq, _) in &parts {
                    if *j == *fi || !covers(*j, *jlp) || jlp.y < bottom - 0.05 {
                        continue;
                    }
                    let Some(m) = gpu.model(jname) else {
                        continue;
                    };
                    let (lo, hi) = (m.bounds_min.as_dvec3(), m.bounds_max.as_dvec3());
                    for c in 0..8 {
                        let corner = DVec3::new(
                            if c & 1 == 0 { lo.x } else { hi.x },
                            if c & 2 == 0 { lo.y } else { hi.y },
                            if c & 4 == 0 { lo.z } else { hi.z },
                        );
                        let w = *jlp + *jlq * corner;
                        top = top.max(w.y);
                        let side = DVec3::new(w.x - flp.x, 0.0, w.z - flp.z).length();
                        radius = radius.max(side + 0.05);
                    }
                }
                if top - bottom < 0.1 {
                    continue;
                }
                gpu.request_model("fx:fairing_ring", "assets/fx/fairing_ring.glb");
                gpu.request_model("fx:fairing_nose", "assets/fx/fairing_nose.glb");
                if !gpu.has_model("fx:fairing_ring") || !gpu.has_model("fx:fairing_nose") {
                    continue;
                }
                for (model, y) in fairing_shells(bottom, top, radius) {
                    let at = DVec3::new(flp.x, y, flp.z);
                    let wpos = pos + vessel_rot * (at + origin);
                    let mut it = DrawItem::lit(model.into(), wpos - cam, vessel_rot, radius, sun);
                    it.ambient = part_ambient;
                    it.fill_dir = up;
                    it.fill = 0.25;
                    it.sun = sun_share;
                    items.push(it);
                }
            }
            for (i, name, lp, lq, animation) in parts {
                if !gpu.has_model(&name) {
                    continue;
                }
                let wpos = pos + vessel_rot * (lp + origin);
                if let Some(m) = gpu.model(&name) {
                    let (bmin, bmax) = (m.bounds_min.as_dvec3(), m.bounds_max.as_dvec3());
                    let rot = vessel_rot * lq;
                    let mut lo = DVec3::splat(f64::INFINITY);
                    let mut hi = DVec3::splat(f64::NEG_INFINITY);
                    for c in 0..8 {
                        let corner = DVec3::new(
                            if c & 1 == 0 { bmin.x } else { bmax.x },
                            if c & 2 == 0 { bmin.y } else { bmax.y },
                            if c & 4 == 0 { bmin.z } else { bmax.z },
                        );
                        let w = wpos - cam + rot * corner;
                        lo = lo.min(w);
                        hi = hi.max(w);
                    }
                    self.pick_boxes.push((i, lo, hi));
                }
                let pid = craft.parts[i].part_id.as_str();
                let mut it = DrawItem::lit(name, wpos - cam, vessel_rot * lq, 1.0, sun);
                it.animation = animation;
                if let Some(rotation) = gimbal_rotations.get(&i) {
                    if let Some(model) = gpu.model(&it.model) {
                        it.node_rotations = model.rig.gimbal_turns(*rotation);
                    }
                }
                if let (Some((node, axis)), Some(f)) = (control_hinge(pid), readout) {
                    if let Some(d) = f.control_surfaces.iter().find(|c| c.0 as usize == i) {
                        it.node_turns.push((node, hinge_turn(axis, d.1)));
                    }
                }
                it.ambient = part_ambient;
                it.fill_dir = up;
                it.fill = 0.25;
                it.sun = sun_share;
                // Re-entry heat: a part past half its limit glows from dull red to orange-white.
                if let Some(frac) = heat.get(&(i as u16)) {
                    let g = ((frac - 0.5) / 0.5).clamp(0.0, 1.0);
                    if g > 0.0 {
                        it.color = [1.0, 1.0 - 0.55 * g, 1.0 - 0.85 * g, 1.0];
                        it.emissive = 0.6 * g;
                    }
                }
                items.push(it);
                if let Some((state, open)) = chutes.get(&(i as u16)) {
                    let open = match state {
                        ose_net::ChuteState::SemiDeployed => open.clamp(0.05, 1.0) * 0.35,
                        ose_net::ChuteState::Deployed => open.clamp(0.05, 1.0),
                        _ => 0.0,
                    };
                    if open > 0.0 {
                        gpu.request_model("fx:canopy", "assets/fx/canopy.glb");
                        if gpu.has_model("fx:canopy") {
                            let size = canopy_diameter(pid) * open as f64;
                            let at = wpos
                                + trail * (canopy_diameter(pid) * 0.8 * (0.4 + 0.6 * open as f64));
                            let rot = DQuat::from_rotation_arc(DVec3::Y, trail);
                            let mut c = DrawItem::lit("fx:canopy".into(), at - cam, rot, size, sun);
                            c.ambient = part_ambient.max(0.35);
                            c.sun = sun_share;
                            items.push(c);
                        }
                    }
                }
            }
        } else {
            // No craft document: a Kerbal on EVA, a flag, or a craft launched elsewhere (a probe core).
            // Kerbals and flags stand up from the ground (a Kerbal faces where it walks); a stand-in probe
            // keeps the vessel's attitude.
            let rot = match inp.stand {
                Stand::Craft => vessel_rot,
                Stand::Kerbal { .. } => {
                    let ahead = vessel_rot * DVec3::Y;
                    let level = (ahead - up * ahead.dot(up)).normalize_or_zero();
                    if level.length() > 0.5 {
                        quat_from_basis(up.cross(level), up, level)
                    } else {
                        DQuat::from_rotation_arc(DVec3::Y, up)
                    }
                }
                Stand::Flag => DQuat::from_rotation_arc(DVec3::Y, up),
            };
            // A walking Kerbal bobs and sways with its stride (the original animates a walk cycle; this
            // rigid model moves as a whole).
            let (mut at, mut rot) = (pos, rot);
            let mut kerbal_clip: Option<&str> = None;
            if matches!(inp.stand, Stand::Kerbal { .. }) {
                let landed = inp
                    .vessel
                    .flight
                    .as_ref()
                    .is_some_and(|f| f.situation == ose_net::FlightSituation::Landed);
                if let Some((p0, t0)) = self.walk_last {
                    let dt = ((inp.now_ms - t0) / 1000.0).max(1e-3);
                    let v = ((pos - p0).length() / dt).min(10.0);
                    self.walk_speed += (v - self.walk_speed) * (dt * 6.0).min(1.0);
                    self.walk_phase += self.walk_speed * dt * 5.0;
                }
                self.walk_last = Some((pos, inp.now_ms));
                kerbal_clip = Some(if !landed {
                    "air"
                } else if self.walk_speed > 1.6 {
                    "run"
                } else if self.walk_speed > 0.2 {
                    "walk"
                } else {
                    "idle"
                });
                // The rigid stand-in bobs with its stride; the animated Kerbal walks.
                let animated = gpu.has_model("kerbal:anim_m") || gpu.has_model("kerbal:anim_f");
                if landed && self.walk_speed > 0.2 && !animated {
                    let s = self.walk_phase.sin();
                    at += up * (0.035 * s.abs());
                    rot = (rot * DQuat::from_rotation_z(0.07 * s)).normalize();
                }
            }
            for (name, _, scale) in inp.stand.models(gpu) {
                let mut it = DrawItem::lit(name.to_string(), at - cam, rot, scale, sun);
                it.ambient = part_ambient;
                it.sun = sun_share;
                if let Some(key) = kerbal_clip {
                    it.animation = kerbal_sample(gpu, name, key, inp.now_ms);
                }
                if let Some(c) = inp.stand.tint(name) {
                    it.color = c;
                }
                items.push(it);
            }
            if let Some(f) = readout.filter(|f| f.eva) {
                let nozzles = self.fx_nozzles.borrow();
                let translation = rot.inverse() * dir_to_model(r, g(f.rcs_translate), spin, inp.ut);
                let rotation = rot.inverse() * dir_to_model(r, g(f.rcs_rotate), spin, inp.ut);
                if let Some(jets) = nozzles.get("eva_kerbal") {
                    for nozzle in jets {
                        let throttle = if landed {
                            0.0
                        } else {
                            crate::particles::rcs_nozzle_throttle(
                                DVec3::from_array(nozzle.position),
                                DVec3::from_array(nozzle.direction),
                                translation,
                                rotation,
                                nozzle.rotate,
                                true,
                            )
                        };
                        engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                            gimbal: DQuat::IDENTITY,
                            instance: 0x0400000000000000
                                | (inp.vessel.id << 8)
                                | nozzle.mode as u64,
                            part: "eva_kerbal".into(),
                            model: glam::DMat4::from_rotation_translation(rot, at - cam),
                            throttle,
                            commanded: throttle,
                            pressure: crate::particles::pressure_multiplier(f.static_pressure_kpa),
                            atmosphere_density: f.atmosphere_density,
                            gravity_mps2: f.gravity_mps2,
                            relative_velocity: f.surface_speed_mps,
                            mode: nozzle.mode,
                            time: inp.now_ms * 0.001,
                            simulation_time: inp.ut,
                            vapor: None,
                            reentry: None,
                            thrust_kn: 0.0,
                            distance_from_spawn: 0.0,
                            vertical_deviation: 1.0,
                        });
                    }
                }
                if landed && self.walk_speed > 0.2 && inp.ut - self.last_footstep > 0.45 {
                    self.last_footstep = inp.ut;
                    self.bursts.queue(
                        "fx_footstep_default",
                        glam::DMat4::from_rotation_translation(rot, pos),
                        inp.ut,
                        self.walk_speed,
                    );
                }
            }
        }
        // Other vessels within a few kilometres, as the original loads crafts around the active one.
        for (o, stand) in inp.nearby_vessels {
            let p = to_model(*o - inp.body_pos, spin, inp.ut);
            if (p - pos).length() > 5_000.0 {
                continue;
            }
            let rot = DQuat::from_rotation_arc(DVec3::Y, p.normalize_or_zero());
            for (name, _, scale) in stand.models(gpu) {
                let mut it = DrawItem::lit(name.to_string(), p - cam, rot, scale, sun);
                it.ambient = part_ambient;
                it.sun = sun_share;
                it.animation = kerbal_sample(gpu, name, "idle", inp.now_ms);
                if let Some(c) = stand.tint(name) {
                    it.color = c;
                }
                items.push(it);
            }
        }
        if let Some(mut fire) = engine_draws
            .iter()
            .find(|e| e.part == "fx_launchsmoke_default")
            .cloned()
        {
            fire.instance = 0x0100000000000000 | inp.vessel.id;
            fire.part = "fx_launchfire_default".into();
            engine_draws.push(fire);
        }
        self.bursts.expire(inp.ut);
        if let Some(f) = readout {
            for burst in &self.bursts.active {
                let mut model = burst.model;
                model.w_axis -= cam.extend(0.0);
                engine_draws.push(crate::engine_fx_gpu::EngineDraw {
                    gimbal: DQuat::IDENTITY,
                    instance: burst.instance,
                    part: burst.asset.clone(),
                    model,
                    throttle: 1.0,
                    commanded: 1.0,
                    pressure: crate::particles::pressure_multiplier(f.static_pressure_kpa),
                    atmosphere_density: f.atmosphere_density,
                    gravity_mps2: f.gravity_mps2,
                    relative_velocity: burst.relative_velocity,
                    mode: 0,
                    time: inp.now_ms * 0.001,
                    simulation_time: inp.ut,
                    vapor: None,
                    reentry: None,
                    thrust_kn: 0.0,
                    distance_from_spawn: 0.0,
                    vertical_deviation: 1.0,
                });
            }
            self.last_surface_speed = f.surface_speed_mps;
        }
        // Heated parts retain their surface glow above. The legacy flame.glb cone was not a
        // reentry effect: it also appeared after burnout and during low-speed unpowered descents.
        // Sky: blue low in the atmosphere, black above it.
        let t = if inp.atmosphere_m > 0.0 {
            (alt / inp.atmosphere_m).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let k = (1.0 - t).powf(2.5);
        let clear = [0.45 * k, 0.65 * k, 0.92 * k];
        if planet_ready {
            // The planet's terrain replaces the hub scene's flat 60 km ground plane.
            gpu.hide_meshes("ksc", &["ground"]);
            if let Some(p) = self.planet.as_mut() {
                items.extend(p.scatter_items(
                    gpu,
                    &crate::planet::PlanetView {
                        body_id: inp.body_id,
                        radius_m: inp.body_radius,
                        cam,
                        sun,
                        time_s: inp.now_ms / 1000.0,
                        occluders: &occluders,
                        sun_radius_rad,
                    },
                ));
            }
        }
        let extra = self
            .planet
            .as_ref()
            .filter(|_| planet_ready)
            .map(|p| p as &dyn crate::render::ExtraPass);
        gpu.frame_with_effects(
            "assets/parts/",
            &items,
            &[],
            clear,
            crate::render::FrameEffects {
                camera: cam,
                engines: &engine_draws,
                extra,
            },
        );
        true
    }
}

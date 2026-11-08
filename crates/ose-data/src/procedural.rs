//! KSP2's procedural wings, stabilizers, control surfaces and fins (`Module_ProceduralPart`).
//!
//! A procedural wing is a skinned mesh whose bones the player moves with sliders (`OffsetSliderSystem`):
//! each slider adds a position or scale offset to a few corner bones, Unity Position/Scale/Aim constraints
//! place the helper bones between them, and `SkinMeshToStaticGenerator` bakes the result into static meshes.
//! The part's numbers then come from a drag cube rendered from those meshes:
//!
//! - mass = `BasePartDensity` x the drag cube's volume estimate (`Module_ProceduralPart.SetMeshValues`,
//!   `Module_Drag.GetMeshVolume`);
//! - fixed lift coefficient = `MeshAreaLiftMultiplier` x the cube's area across the lift axis, less the
//!   moving part's own coefficient (`Module_ControlSurface.MeshDataChanged`, `GetMeshArea`);
//! - moving (control surface) coefficient = the same multiplier x the flap's area along its lift axis,
//!   measured in the flap pivot's own frame.
//!
//! This module replays all of that from the rigs exported by `tools/export_procedural_wings.py`
//! (`assets/own/procedural-wings.json`), including the 256-pixel silhouettes the original renders, so a
//! shape gives the same mass and lift on the server, in the editor and in the original.

use crate::flight_geometry::{PartFlight, SurfaceFlight};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// A procedural part's saved shape (`Data_ProceduralPart`): the selected mesh option (with or without a
/// moving control surface), slider values in 0..1 by slider name, and toggles. Sliders left out keep their
/// authored start value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WingShape {
    pub option: u32,
    pub sliders: Vec<(String, f64)>,
    pub toggles: Vec<(String, bool)>,
}

impl WingShape {
    pub fn slider(&self, name: &str) -> Option<f64> {
        self.sliders
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }

    pub fn set_slider(&mut self, name: &str, value: f64) {
        let value = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self
            .sliders
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(s) => s.1 = value,
            None => self.sliders.push((name.to_string(), value)),
        }
    }

    /// Finite values, known slider names and an existing option for `part_id`.
    pub fn valid_for(&self, part_id: &str) -> bool {
        let Some(rig) = rig(part_id) else {
            return false;
        };
        let Some(option) = rig.options.get(self.option as usize) else {
            return false;
        };
        self.sliders.len() <= 32
            && self.toggles.len() <= 8
            && self.sliders.iter().all(|(n, v)| {
                v.is_finite()
                    && (0.0..=1.0).contains(v)
                    && option.sliders.iter().any(|s| &s.name == n)
            })
    }
}

#[derive(Debug, Deserialize)]
pub struct Rigs {
    pub parts: BTreeMap<String, PartRig>,
}

#[derive(Debug, Deserialize)]
pub struct PartRig {
    /// The part definition's mass, t, and `BasePartDensity`, t per m^3 of the volume estimate.
    pub mass_t: f64,
    pub density: f64,
    #[serde(rename = "type")]
    pub kind: String,
    pub mesh_area_lift_multiplier: f64,
    /// `CtrlSurfacePivotTransformName`: the bone the moving surface hangs from.
    pub ctrl_pivot: String,
    /// `CtrlTransformRotAxis`: the pivot axis the moving surface turns about (X on wings and stabilizers, Y on
    /// control surfaces and fins).
    #[serde(default = "axis_x")]
    pub ctrl_rot_axis: String,
    pub options: Vec<OptionRig>,
}

/// One mesh option (`ProceduralMeshOption`), e.g. `wing_1v_procedural_light_1e` with a control surface.
#[derive(Debug, Deserialize)]
pub struct OptionRig {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub bones: Vec<Bone>,
    pub sliders: Vec<Slider>,
    pub toggles: Vec<Toggle>,
    pub constraints: Vec<Constraint>,
    /// Baked group name -> bone its static meshes hang from (-1: the static root).
    pub attach: BTreeMap<String, i32>,
    pub meshes: Vec<RigMesh>,
}

#[derive(Debug, Deserialize)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub p: [f64; 3],
    pub r: [f64; 4],
    pub s: [f64; 3],
}

#[derive(Debug, Deserialize)]
pub struct Slider {
    pub name: String,
    pub label: String,
    pub group: String,
    pub start: f64,
    pub limits: Vec<Limit>,
    pub targets: Vec<Target>,
    pub measure: Measure,
}

/// `DynamicSliderLimit`: a curve of the parent slider's value that narrows this slider's range about its
/// neutral value.
#[derive(Debug, Deserialize)]
pub struct Limit {
    pub parent: String,
    /// Keys: time, value, in tangent, out tangent (Unity `AnimationCurve`).
    pub curve: Vec<[f64; 4]>,
}

#[derive(Debug, Deserialize)]
pub struct Target {
    pub mode: String,
    pub bone: i32,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Deserialize)]
pub struct Measure {
    pub mode: String,
    pub from: i32,
    pub to: i32,
}

#[derive(Debug, Deserialize)]
pub struct Toggle {
    pub name: String,
    pub group: String,
    pub value: bool,
}

#[derive(Debug, Deserialize)]
pub struct Constraint {
    pub kind: String,
    pub bone: i32,
    pub sources: Vec<(i32, f64)>,
    #[serde(default)]
    pub offset: Option<[f64; 3]>,
    #[serde(default)]
    pub aim: Option<[f64; 3]>,
}

#[derive(Debug, Deserialize)]
pub struct RigMesh {
    pub name: String,
    /// `SkinMeshToStaticGenerator` group (`WING_CORE`, `CONTROLSURFACE`, ...).
    pub group: String,
    pub material: String,
    pub palette: Vec<i32>,
    /// Row-major 3x4 bind poses, one per palette entry.
    pub bindposes: Vec<[f64; 12]>,
    pub vertices: Vec<f32>,
    pub uvs: Vec<f32>,
    pub influences: Vec<Vec<(u16, f32)>>,
    pub triangles: Vec<u32>,
}

fn axis_x() -> String {
    "X".into()
}

static RIGS: OnceLock<Rigs> = OnceLock::new();

/// The exported rigs (parsed once).
pub fn rigs() -> &'static Rigs {
    RIGS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/own/procedural-wings.json"))
            .expect("assets/own/procedural-wings.json")
    })
}

pub fn rig(part_id: &str) -> Option<&'static PartRig> {
    rigs().parts.get(part_id)
}

pub fn is_procedural_wing(part_id: &str) -> bool {
    rig(part_id).is_some()
}

// -- small affine maths (Unity space, f64)

#[derive(Clone, Copy, Debug, PartialEq)]
struct Aff {
    /// Row-major linear part.
    m: [[f64; 3]; 3],
    t: [f64; 3],
}

impl Aff {
    const IDENTITY: Self = Self {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    fn mul(&self, o: &Self) -> Self {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.m[i][k] * o.m[k][j]).sum();
            }
        }
        Self {
            m,
            t: add(self.apply_dir(o.t), self.t),
        }
    }

    fn apply_dir(&self, v: [f64; 3]) -> [f64; 3] {
        [
            self.m[0][0] * v[0] + self.m[0][1] * v[1] + self.m[0][2] * v[2],
            self.m[1][0] * v[0] + self.m[1][1] * v[1] + self.m[1][2] * v[2],
            self.m[2][0] * v[0] + self.m[2][1] * v[1] + self.m[2][2] * v[2],
        ]
    }

    fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        add(self.apply_dir(v), self.t)
    }

    fn column(&self, j: usize) -> [f64; 3] {
        [self.m[0][j], self.m[1][j], self.m[2][j]]
    }

    /// Per-axis scale (Unity `lossyScale` for unsheared matrices).
    fn scale(&self) -> [f64; 3] {
        [
            len(self.column(0)),
            len(self.column(1)),
            len(self.column(2)),
        ]
    }

    /// Rotation with the scale divided out.
    fn rotation(&self) -> [[f64; 3]; 3] {
        let s = self.scale();
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = if s[j] > 1e-12 {
                    self.m[i][j] / s[j]
                } else {
                    0.0
                };
            }
        }
        r
    }

    fn inverse(&self) -> Self {
        let a = &self.m;
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if det.abs() < 1e-18 {
            return Self::IDENTITY;
        }
        let inv_det = 1.0 / det;
        let m = [
            [
                (a[1][1] * a[2][2] - a[1][2] * a[2][1]) * inv_det,
                (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * inv_det,
                (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * inv_det,
            ],
            [
                (a[1][2] * a[2][0] - a[1][0] * a[2][2]) * inv_det,
                (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * inv_det,
                (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * inv_det,
            ],
            [
                (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * inv_det,
                (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * inv_det,
                (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * inv_det,
            ],
        ];
        let inv = Self { m, t: [0.0; 3] };
        let t = inv.apply_dir(self.t);
        Self {
            m,
            t: [-t[0], -t[1], -t[2]],
        }
    }
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale3(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn mat_vec(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}
fn mat_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}
fn transpose(a: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[j][i];
        }
    }
    m
}

fn quat_mat(q: [f64; 4]) -> [[f64; 3]; 3] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let [x, y, z, w] = if n > 1e-12 {
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Rotation taking unit `from` onto unit `to` by the shortest arc.
fn arc(from: [f64; 3], to: [f64; 3]) -> [[f64; 3]; 3] {
    let axis = cross(from, to);
    let s = len(axis);
    let c = dot(from, to);
    if s < 1e-12 {
        if c > 0.0 {
            return Aff::IDENTITY.m;
        }
        // Half turn about any axis perpendicular to `from`.
        let p = if from[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let k = cross(from, p);
        let k = scale3(k, 1.0 / len(k));
        return [
            [
                2.0 * k[0] * k[0] - 1.0,
                2.0 * k[0] * k[1],
                2.0 * k[0] * k[2],
            ],
            [
                2.0 * k[1] * k[0],
                2.0 * k[1] * k[1] - 1.0,
                2.0 * k[1] * k[2],
            ],
            [
                2.0 * k[2] * k[0],
                2.0 * k[2] * k[1],
                2.0 * k[2] * k[2] - 1.0,
            ],
        ];
    }
    let k = scale3(axis, 1.0 / s);
    let kx = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let kk = mat_mul(&kx, &kx);
    let mut r = Aff::IDENTITY.m;
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] += s * kx[i][j] + (1.0 - c) * kk[i][j];
        }
    }
    r
}

/// Unity `AnimationCurve.Evaluate` (cubic Hermite between keys, clamped at the ends).
fn curve(keys: &[[f64; 4]], t: f64) -> f64 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return 1.0;
    };
    if t <= first[0] {
        return first[1];
    }
    if t >= last[0] {
        return last[1];
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a[0] && t <= b[0] {
            let dt = b[0] - a[0];
            if dt <= 0.0 {
                return b[1];
            }
            let s = (t - a[0]) / dt;
            let (s2, s3) = (s * s, s * s * s);
            return (2.0 * s3 - 3.0 * s2 + 1.0) * a[1]
                + (s3 - 2.0 * s2 + s) * dt * a[3]
                + (-2.0 * s3 + 3.0 * s2) * b[1]
                + (s3 - s2) * dt * b[2];
        }
    }
    last[1]
}

/// The procedural visual's turn under the part (`ProcedurallyEditablePart.DEFAULT_PART_VISUAL_ROTATION`,
/// Euler(0, 0, 90)): option frame (chord X, thickness Y, span Z) -> part frame (thickness -X, chord Y).
fn to_part(v: [f64; 3]) -> [f64; 3] {
    [-v[1], v[0], v[2]]
}

/// A baked mesh of a posed shape, in the part frame (Unity axes, metres).
#[derive(Clone, Debug, PartialEq)]
pub struct PosedMesh {
    pub name: String,
    pub group: String,
    pub material: String,
    pub positions: Vec<[f64; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Counter-clockwise from outside in Unity's left-handed frame (as authored, flipped when mirrored).
    pub triangles: Vec<u32>,
}

/// A drag cube as the original renders it (`DragCubeGenerator`): silhouette area per face (+X, -X, +Y, -Y,
/// +Z, -Z), and the bounds' centre and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cube {
    pub area: [f64; 6],
    pub center: [f64; 3],
    pub size: [f64; 3],
}

impl Cube {
    /// `Module_Drag.GetMeshVolume`: the box volume shrunk by how much of each face the shape fills.
    pub fn volume(&self) -> f64 {
        let [sx, sy, sz] = self.size;
        let mut volume = sx * sy * sz;
        let fx = self.area[0] / (sy * sz);
        let fy = self.area[2] / (sx * sz);
        let fz = self.area[4] / (sx * sy);
        let sum = fx + fy + fz;
        if sum.is_finite() && sum > 0.0 && sum < volume {
            volume *= (fx.min(fz) + 2.0 * (fx * fz)) / 3.0 * fy;
        }
        volume
    }

    /// `Module_Drag.GetCrossSectionalArea`.
    pub fn cross_section(&self, dir: [f64; 3]) -> f64 {
        const FACES: [[f64; 3]; 6] = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        FACES
            .iter()
            .zip(self.area)
            .map(|(f, a)| dot(*f, dir).max(0.0) * a)
            .sum()
    }
}

/// `DragRendererSettings.DRAG_RENDER_RESOLUTION`.
const DRAG_RESOLUTION: usize = 256;

/// Render `tris` the way the original's aero camera does: orthographic along each axis, a square view as
/// wide as the larger of the other two extents, 256 x 256 pixels, a pixel counted when its centre is covered.
pub fn drag_cube(points: &[[f64; 3]], tris: &[[u32; 3]]) -> Cube {
    if points.is_empty() {
        return Cube::default();
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let size = sub(hi, lo);
    let center = scale3(add(lo, hi), 0.5);
    let mut area = [0.0; 6];
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let side = size[u].max(size[v]);
        if side <= 0.0 {
            continue;
        }
        let px = side / DRAG_RESOLUTION as f64;
        let n = DRAG_RESOLUTION;
        let mut grid = vec![false; n * n];
        let to_px = |p: &[f64; 3]| {
            (
                (p[u] - center[u]) / px + n as f64 / 2.0,
                (p[v] - center[v]) / px + n as f64 / 2.0,
            )
        };
        for t in tris {
            let (Some(a), Some(b), Some(c)) = (
                points.get(t[0] as usize),
                points.get(t[1] as usize),
                points.get(t[2] as usize),
            ) else {
                continue;
            };
            let (a, b, c) = (to_px(a), to_px(b), to_px(c));
            let x0 = (a.0.min(b.0).min(c.0).floor().max(0.0)) as usize;
            let x1 = (a.0.max(b.0).max(c.0).ceil().min(n as f64)) as usize;
            let y0 = (a.1.min(b.1).min(c.1).floor().max(0.0)) as usize;
            let y1 = (a.1.max(b.1).max(c.1).ceil().min(n as f64)) as usize;
            let edge = |p: (f64, f64), q: (f64, f64), x: f64, y: f64| {
                (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)
            };
            for ix in x0..x1 {
                let x = ix as f64 + 0.5;
                for iy in y0..y1 {
                    let y = iy as f64 + 0.5;
                    let e0 = edge(a, b, x, y);
                    let e1 = edge(b, c, x, y);
                    let e2 = edge(c, a, x, y);
                    if (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0)
                        || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
                    {
                        grid[ix * n + iy] = true;
                    }
                }
            }
        }
        let covered = grid.iter().filter(|c| **c).count() as f64;
        area[2 * axis] = covered * px * px;
        area[2 * axis + 1] = covered * px * px;
    }
    Cube { area, center, size }
}

/// A shape evaluated: the baked meshes and the numbers the original derives from them.
#[derive(Clone, Debug, PartialEq)]
pub struct WingResult {
    pub meshes: Vec<PosedMesh>,
    /// Drag cube of every baked mesh, part frame.
    pub cube: Cube,
    pub mass_t: f64,
    /// `LiftingCtrlSurfaceArea` (the fixed part's `deflectionLiftCoeff`) and `CtrlSurfaceArea`.
    pub fixed_lift_coeff: f64,
    pub moving_lift_coeff: f64,
    /// The moving surface's pivot (`CtrlSurfacePivotOffset`), hinge (its local X) and lift axis (its local
    /// `CtrlTransformDir`), part frame; zero without a control surface.
    pub ctrl_pivot_m: [f64; 3],
    pub ctrl_hinge: [f64; 3],
    pub ctrl_normal: [f64; 3],
    /// The moving surface's centre (`CoPressureCtrlSurfaceOffset`).
    pub ctrl_center_m: [f64; 3],
    /// What the editor shows per slider: metres or degrees.
    pub measurements: Vec<(String, f64)>,
    /// Inertia per tonne about `cube.center`, as a uniform box of the cube's size (part frame).
    pub inertia_per_t: [[f64; 3]; 3],
}

/// The slider values that drive `option`, in the original's update order, with the dynamic limits applied
/// (`OffsetSliderSystem.UpdateAllOffsets`).
fn final_values(option: &OptionRig, shape: &WingShape) -> Vec<(usize, f64)> {
    let raw = |s: &Slider| shape.slider(&s.name).unwrap_or(s.start).clamp(0.0, 1.0);
    let by_name: BTreeMap<&str, usize> = option
        .sliders
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();
    // Parents before children (`BuildHierarchySortedSliderUpdateList`).
    let mut order: Vec<usize> = Vec::new();
    fn visit(
        i: usize,
        option: &OptionRig,
        by_name: &BTreeMap<&str, usize>,
        order: &mut Vec<usize>,
        depth: usize,
    ) {
        if order.contains(&i) || depth > 16 {
            return;
        }
        for l in &option.sliders[i].limits {
            if let Some(&p) = by_name.get(l.parent.as_str()) {
                visit(p, option, by_name, order, depth + 1);
            }
        }
        if !order.contains(&i) {
            order.push(i);
        }
    }
    for i in 0..option.sliders.len() {
        visit(i, option, &by_name, &mut order, 0);
    }
    order
        .into_iter()
        .map(|i| {
            let s = &option.sliders[i];
            let mut v = raw(s);
            if !s.limits.is_empty() {
                // `SetSliderToMatchInitialPosition`: neutral = |min| / |max - min| of the first target.
                let neutral = s.targets.first().map_or(0.0, |t| {
                    let d = sub(t.max, t.min);
                    let dd = dot(d, d);
                    if dd > 0.0 {
                        (dot(t.min, t.min) / dd).sqrt()
                    } else {
                        0.0
                    }
                });
                let mut lim = f64::MAX;
                for l in &s.limits {
                    if let Some(&p) = by_name.get(l.parent.as_str()) {
                        lim = lim.min(curve(&l.curve, raw(&option.sliders[p])));
                    }
                }
                if lim < f64::MAX {
                    v = v * lim + (neutral - neutral * lim);
                }
            }
            (i, v)
        })
        .collect()
}

/// Bone world matrices (option frame) after sliders and constraints.
fn pose(option: &OptionRig, shape: &WingShape) -> Vec<Aff> {
    let n = option.bones.len();
    let mut p: Vec<[f64; 3]> = option.bones.iter().map(|b| b.p).collect();
    let mut s: Vec<[f64; 3]> = option.bones.iter().map(|b| b.s).collect();
    let mut r: Vec<[[f64; 3]; 3]> = option.bones.iter().map(|b| quat_mat(b.r)).collect();
    // Offsets add up per bone and mode on top of the rest pose (`ApplyOffsets`).
    let mut pos_add = vec![[0.0; 3]; n];
    let mut scale_add = vec![[0.0; 3]; n];
    // `CONSTRAINT_POS` / `CONSTRAINT_SCALE` targets move the weights of a two-source constraint instead: the
    // first source gains the offset's x, the second loses it, each clamped to 0..100 (`ApplyOffsetByMode`).
    let mut pos_weight_add = vec![0.0; n];
    let mut scale_weight_add = vec![0.0; n];
    for (i, v) in final_values(option, shape) {
        for t in &option.sliders[i].targets {
            let Ok(b) = usize::try_from(t.bone) else {
                continue;
            };
            if b >= n {
                continue;
            }
            let off = add(t.min, scale3(sub(t.max, t.min), v));
            match t.mode.as_str() {
                "position" => pos_add[b] = add(pos_add[b], off),
                "scale" => scale_add[b] = add(scale_add[b], off),
                "constraint_pos" => pos_weight_add[b] += off[0],
                "constraint_scale" => scale_weight_add[b] += off[0],
                _ => {}
            }
        }
    }
    for b in 0..n {
        p[b] = add(p[b], pos_add[b]);
        s[b] = add(s[b], scale_add[b]);
    }
    let worlds = |p: &[[f64; 3]], r: &[[[f64; 3]; 3]], s: &[[f64; 3]]| {
        let mut w: Vec<Aff> = Vec::with_capacity(n);
        for (i, bone) in option.bones.iter().enumerate() {
            let mut m = [[0.0; 3]; 3];
            for (a, row) in m.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = r[i][a][c] * s[i][c];
                }
            }
            let local = Aff { m, t: p[i] };
            let world = match usize::try_from(bone.parent) {
                Ok(pi) if pi < i => w[pi].mul(&local),
                _ => local,
            };
            w.push(world);
        }
        w
    };
    // Unity re-evaluates constraints every frame; repeat until the chains (helper bones placed between
    // other helper bones) settle.
    for _ in 0..8 {
        let w = worlds(&p, &r, &s);
        for c in &option.constraints {
            let Ok(b) = usize::try_from(c.bone) else {
                continue;
            };
            if b >= n {
                continue;
            }
            let parent = usize::try_from(option.bones[b].parent)
                .ok()
                .filter(|pi| *pi < n)
                .map_or(Aff::IDENTITY, |pi| w[pi]);
            let shift = match c.kind.as_str() {
                "position" => pos_weight_add[b],
                "scale" => scale_weight_add[b],
                _ => 0.0,
            };
            let sources: Vec<(usize, f64)> = c
                .sources
                .iter()
                .enumerate()
                .filter_map(|(k, (i, wt))| {
                    let wt = if c.sources.len() == 2 && shift != 0.0 {
                        let sign = if k == 0 { 1.0 } else { -1.0 };
                        (wt + sign * shift).clamp(0.0, 100.0)
                    } else {
                        *wt
                    };
                    usize::try_from(*i).ok().filter(|i| *i < n).map(|i| (i, wt))
                })
                .collect();
            let total: f64 = sources.iter().map(|(_, wt)| wt).sum();
            if total <= 0.0 {
                continue;
            }
            match c.kind.as_str() {
                "position" => {
                    let mut at = [0.0; 3];
                    for (i, wt) in &sources {
                        at = add(at, scale3(w[*i].t, *wt / total));
                    }
                    at = add(at, c.offset.unwrap_or([0.0; 3]));
                    p[b] = parent.inverse().apply(at);
                }
                "scale" => {
                    let mut lossy = [0.0; 3];
                    for (i, wt) in &sources {
                        lossy = add(lossy, scale3(w[*i].scale(), *wt / total));
                    }
                    let off = c.offset.unwrap_or([1.0; 3]);
                    let ps = parent.scale();
                    for k in 0..3 {
                        s[b][k] = lossy[k] * off[k] / ps[k].max(1e-12);
                    }
                }
                "aim" => {
                    // Turn the bone by the shortest arc so its aim axis points at the source; the
                    // prefab's rest rotation is Unity's own solution, so the roll about the aim stays it.
                    let Some(&(src, _)) = sources.first() else {
                        continue;
                    };
                    let to = sub(w[src].t, w[b].t);
                    let l = len(to);
                    if l < 1e-9 {
                        continue;
                    }
                    let aim = c.aim.unwrap_or([1.0, 0.0, 0.0]);
                    let al = len(aim).max(1e-12);
                    let pr = parent.rotation();
                    let world_rot = mat_mul(&pr, &r[b]);
                    let current = mat_vec(&world_rot, scale3(aim, 1.0 / al));
                    let turn = arc(current, scale3(to, 1.0 / l));
                    r[b] = mat_mul(&transpose(&pr), &mat_mul(&turn, &world_rot));
                }
                _ => {}
            }
        }
    }
    worlds(&p, &r, &s)
}

/// Evaluate `shape` on `part_id`'s rig; `mirrored` for the second wing of a mirror pair (the original flips
/// the baked meshes across the thickness axis).
pub fn evaluate(part_id: &str, shape: &WingShape, mirrored: bool) -> Option<WingResult> {
    let rig = rig(part_id)?;
    let option = rig
        .options
        .get(shape.option as usize)
        .or_else(|| rig.options.last())?;
    let w = pose(option, shape);
    let flip = |v: [f64; 3]| if mirrored { [-v[0], v[1], v[2]] } else { v };
    let mut meshes = Vec::with_capacity(option.meshes.len());
    let mut all_points: Vec<[f64; 3]> = Vec::new();
    let mut all_tris: Vec<[u32; 3]> = Vec::new();
    let mut ctrl_points: Vec<[f64; 3]> = Vec::new();
    let mut ctrl_tris: Vec<[u32; 3]> = Vec::new();
    // The moving surface: the baked groups hung from the pivot bone (`CtrlSurfacePivotTransformName`) or
    // below it. Its frame as the original measures it at run time: the hinge along the pivot's
    // `CtrlTransformRotAxis`, the lift axis along the wing's thickness (the original's flap pivot carries its
    // `CtrlTransformDir` there, checked against the stock jet's wing and stabilizer in KSP2).
    let ctrl_bone = option
        .bones
        .iter()
        .position(|b| b.name == rig.ctrl_pivot)
        .filter(|b| *b < w.len());
    let under_pivot = |mut b: i32| {
        let mut hops = 0;
        while let (Ok(i), Some(pivot)) = (usize::try_from(b), ctrl_bone) {
            if i == pivot {
                return true;
            }
            b = option.bones.get(i).map_or(-1, |x| x.parent);
            hops += 1;
            if hops > 64 {
                break;
            }
        }
        false
    };
    let moving_groups: Vec<&str> = option
        .attach
        .iter()
        .filter(|(_, b)| under_pivot(**b))
        .map(|(g, _)| g.as_str())
        .collect();
    let ctrl_frame = ctrl_bone.map(|b| {
        let rot = w[b].rotation();
        let j = if rig.ctrl_rot_axis == "Y" { 1 } else { 0 };
        let hinge = [rot[0][j], rot[1][j], rot[2][j]];
        // The thickness axis: option -Y is the part's +X.
        let normal = [0.0, -1.0, 0.0];
        let along = dot(hinge, normal);
        let h = sub(hinge, scale3(normal, along));
        let l = len(h);
        let hinge = if l > 1e-9 {
            scale3(h, 1.0 / l)
        } else {
            [0.0, 0.0, 1.0]
        };
        let third = cross(normal, hinge);
        // columns: hinge, third, normal
        let frame = [
            [hinge[0], third[0], normal[0]],
            [hinge[1], third[1], normal[1]],
            [hinge[2], third[2], normal[2]],
        ];
        (w[b].t, frame)
    });
    for m in &option.meshes {
        let skin: Vec<Aff> = m
            .palette
            .iter()
            .zip(&m.bindposes)
            .map(|(b, bp)| {
                let bind = Aff {
                    m: [
                        [bp[0], bp[1], bp[2]],
                        [bp[4], bp[5], bp[6]],
                        [bp[8], bp[9], bp[10]],
                    ],
                    t: [bp[3], bp[7], bp[11]],
                };
                usize::try_from(*b)
                    .ok()
                    .filter(|b| *b < w.len())
                    .map_or(Aff::IDENTITY, |b| w[b].mul(&bind))
            })
            .collect();
        let count = m.vertices.len() / 3;
        let mut option_points = Vec::with_capacity(count);
        for vi in 0..count {
            let v = [
                m.vertices[3 * vi] as f64,
                m.vertices[3 * vi + 1] as f64,
                m.vertices[3 * vi + 2] as f64,
            ];
            let mut acc = [0.0; 3];
            let mut total = 0.0;
            for (slot, wt) in m.influences.get(vi).map(|v| v.as_slice()).unwrap_or(&[]) {
                if let Some(k) = skin.get(*slot as usize) {
                    acc = add(acc, scale3(k.apply(v), *wt as f64));
                    total += *wt as f64;
                }
            }
            option_points.push(if total > 0.0 {
                scale3(acc, 1.0 / total)
            } else {
                v
            });
        }
        let positions: Vec<[f64; 3]> = option_points.iter().map(|v| flip(to_part(*v))).collect();
        let mut triangles = m.triangles.clone();
        if mirrored {
            for t in triangles.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
        let base = all_points.len() as u32;
        all_points.extend_from_slice(&positions);
        all_tris.extend(
            triangles
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
        );
        if moving_groups.contains(&m.group.as_str()) {
            if let Some((origin, rot)) = ctrl_frame {
                // `GetMeshArea` on a copy of the pivot turned to the identity: the flap in the pivot's axes.
                let rt = transpose(&rot);
                let base = ctrl_points.len() as u32;
                ctrl_points.extend(option_points.iter().map(|v| mat_vec(&rt, sub(*v, origin))));
                ctrl_tris.extend(
                    m.triangles
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
                );
            }
        }
        let uvs = m
            .uvs
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| [c[0], c[1]])
            .collect::<Vec<_>>();
        meshes.push(PosedMesh {
            name: m.name.clone(),
            group: m.group.clone(),
            material: m.material.clone(),
            positions,
            uvs,
            triangles,
        });
    }
    let cube = drag_cube(&all_points, &all_tris);
    let k = rig.mesh_area_lift_multiplier;
    // Lift along the part's -X (`Data_LiftingSurface.transformDir` X, sign -1 on every procedural wing).
    let total_area = cube.cross_section([-1.0, 0.0, 0.0]);
    let (moving, pivot, hinge, normal, center) = match (ctrl_frame, ctrl_points.is_empty()) {
        (Some((origin, rot)), false) => {
            let flap = drag_cube(&ctrl_points, &ctrl_tris);
            // Along the lift axis (the frame's Z, built above).
            let area = flap.cross_section([0.0, 0.0, 1.0]);
            let axis = |j: usize| flip(to_part([rot[0][j], rot[1][j], rot[2][j]]));
            (
                k * area,
                flip(to_part(origin)),
                axis(0),
                axis(2),
                flip(to_part(add(origin, mat_vec(&rot, flap.center)))),
            )
        }
        _ => (0.0, [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3]),
    };
    let fixed = k * total_area - moving;
    let [sx, sy, sz] = cube.size;
    let inertia_per_t = [
        [(sy * sy + sz * sz) / 12.0, 0.0, 0.0],
        [0.0, (sx * sx + sz * sz) / 12.0, 0.0],
        [0.0, 0.0, (sx * sx + sy * sy) / 12.0],
    ];
    let measurements = option
        .sliders
        .iter()
        .map(|s| (s.name.clone(), measure(&s.measure, option, &w)))
        .collect();
    Some(WingResult {
        meshes,
        mass_t: rig.density * cube.volume(),
        fixed_lift_coeff: fixed,
        moving_lift_coeff: moving,
        ctrl_pivot_m: pivot,
        ctrl_hinge: hinge,
        ctrl_normal: normal,
        ctrl_center_m: center,
        measurements,
        inertia_per_t,
        cube,
    })
}

/// `SliderMeasurement.UpdateMeasurement` for the distance modes; the angle modes read the sweep of the
/// measured bone from the span axis (the original turns a helper bone with a LookAt constraint).
fn measure(m: &Measure, option: &OptionRig, w: &[Aff]) -> f64 {
    let index = |i: i32| usize::try_from(i).ok().filter(|i| *i < w.len());
    let world = |i: usize| w[i].t;
    // The axis modes compare `localPosition`s (both bones hang from the same parent bone).
    let local = |i: usize| {
        usize::try_from(option.bones[i].parent)
            .ok()
            .filter(|p| *p < w.len())
            .map_or(w[i].t, |p| w[p].inverse().apply(w[i].t))
    };
    match (m.mode.as_str(), index(m.from), index(m.to)) {
        ("distance", Some(a), Some(b)) => len(sub(world(a), world(b))),
        ("dist_x", Some(a), Some(b)) => (local(a)[0] - local(b)[0]).abs(),
        ("dist_y", Some(a), Some(b)) => (local(a)[1] - local(b)[1]).abs(),
        ("dist_z", Some(a), Some(b)) => (local(a)[2] - local(b)[2]).abs(),
        _ => 0.0,
    }
}

/// `evaluate`, remembered for the last few shapes asked for (the editor asks on every redraw).
pub fn evaluate_cached(
    part_id: &str,
    shape: &WingShape,
    mirrored: bool,
) -> Option<std::sync::Arc<WingResult>> {
    use std::sync::{Arc, Mutex};
    type Cache = Vec<((String, WingShape, bool), Arc<WingResult>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let key = (part_id.to_string(), shape.clone(), mirrored);
    if let Ok(c) = cache.lock() {
        if let Some((_, r)) = c.iter().find(|(k, _)| *k == key) {
            return Some(r.clone());
        }
    }
    let r = Arc::new(evaluate(part_id, shape, mirrored)?);
    if let Ok(mut c) = cache.lock() {
        if c.len() >= 32 {
            c.remove(0);
        }
        c.push((key, r.clone()));
    }
    Some(r)
}

/// The English names the editor shows (`PartModules/Procedural/Wings/*` in the original's table).
pub fn label(key: &str) -> String {
    let last = key.rsplit('/').next().unwrap_or(key);
    match last {
        "ControlLength" => "Length".into(),
        "ControlPosition" => "Position".into(),
        "ControlSpan" => "Span".into(),
        "ControlSurface" => "Control Surface".into(),
        "Root" => "Wing Root".into(),
        "Tip" => "Wing Tip".into(),
        _ => {
            let mut out = String::new();
            for (i, c) in last.chars().enumerate() {
                if i > 0 && c.is_uppercase() {
                    out.push(' ');
                }
                out.push(c);
            }
            out
        }
    }
}

/// The flight numbers of a shaped part, ready for `CraftPart::flight`: mass, centre of mass (the original
/// moves a control surface's centre of mass to its lift centre), inertia, lift coefficients and the moving
/// surface's pivot.
pub fn flight(part_id: &str, shape: &WingShape, mirrored: bool) -> Option<PartFlight> {
    let r = evaluate_cached(part_id, shape, mirrored)?;
    let v = |a: [f64; 3]| DVec3::new(a[0], a[1], a[2]);
    let t = r.inertia_per_t;
    Some(PartFlight {
        mirrored,
        part_id: part_id.to_string(),
        dry_mass_t: Some(r.mass_t.max(0.001)),
        mass_offset_m: Some(v(r.cube.center)),
        inertia_per_t: Some([v(t[0]), v(t[1]), v(t[2])]),
        surface: Some(SurfaceFlight {
            // `Module_ControlSurface.OnModuleFixedUpdate`: the fixed part lifts only above 0.1.
            fixed_lift_coeff: if r.fixed_lift_coeff > 0.1 {
                r.fixed_lift_coeff
            } else {
                0.0
            },
            moving_lift_coeff: r.moving_lift_coeff.max(0.0),
            pivot_m: v(r.ctrl_pivot_m),
            pressure_offset_m: v(r.ctrl_center_m),
            range_deg: 0.0,
            speed: 0.0,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(values: &[(&str, f64)]) -> WingShape {
        WingShape {
            option: 1,
            sliders: values.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            toggles: vec![],
        }
    }

    /// The stock jet's wing (`Stock04_Jet`): its lift area summed over the fixed and moving parts as the
    /// original measured it (1.95327 + 0.66619 = 2.61946 at the 0.25 multiplier).
    #[test]
    fn stock_jet_wing_total_lift_area_matches_the_original() {
        let s = shape(&[
            ("WING_SPAN", 0.2),
            ("ROOT_LENGTH", 0.33),
            ("TIP_LENGTH", 0.03),
            ("WING_ANGLE", 0.36),
            ("ROOT_THICKNESS", 0.1),
            ("TIP_THICKNESS", 0.04),
            ("TIP_ANGLE", 0.9),
            ("CONTROL_SPAN", 1.0),
            ("CONTROL_LENGTH", 0.5),
            ("CONTROL_POSITION", 1.0),
        ]);
        let r = evaluate("wing_1v_procedural_light", &s, false).unwrap();
        let total = r.fixed_lift_coeff + r.moving_lift_coeff;
        assert!((total - 2.61946).abs() < 0.01, "{total}");
        // KSP2 (tools/ksp2-wing-probe, reference/ksp2-wings): LiftingCtrlSurfaceArea 1.95327, CtrlSurfaceArea
        // 0.66619, flight dry mass 0.17855 t.
        assert!(
            (r.fixed_lift_coeff - 1.95327).abs() < 0.02,
            "{}",
            r.fixed_lift_coeff
        );
        assert!(
            (r.moving_lift_coeff - 0.66619).abs() < 0.01,
            "{}",
            r.moving_lift_coeff
        );
        assert!((r.mass_t - 0.17855).abs() < 0.002, "{}", r.mass_t);
    }

    #[test]
    fn stock_jet_stabilizer_total_lift_area_matches_the_original() {
        let s = shape(&[
            ("WING_SPAN", 0.08),
            ("ROOT_LENGTH", 0.1),
            ("TIP_LENGTH", 0.04),
            ("WING_ANGLE", 0.45),
            ("ROOT_THICKNESS", 0.05),
            ("TIP_THICKNESS", 0.02),
            ("TIP_ANGLE", 0.7033859),
            ("CONTROL_SPAN", 0.9999084),
            ("CONTROL_LENGTH", 0.415659338),
            ("CONTROL_POSITION", 0.677699149),
        ]);
        let r = evaluate("stabilizer_1v_procedural_light", &s, false).unwrap();
        let total = r.fixed_lift_coeff + r.moving_lift_coeff;
        assert!((total - 0.40812).abs() < 0.003, "{total}");
        // KSP2: 0.27433 fixed, 0.13379 moving, 0.03813 t.
        assert!(
            (r.fixed_lift_coeff - 0.27433).abs() < 0.002,
            "{}",
            r.fixed_lift_coeff
        );
        assert!(
            (r.moving_lift_coeff - 0.13379).abs() < 0.002,
            "{}",
            r.moving_lift_coeff
        );
        assert!((r.mass_t - 0.03813).abs() < 0.0005, "{}", r.mass_t);
    }

    /// Shapes reshaped through KSP2's own editor (the wing probe): control surface sliders move constraint
    /// weights, and the moving part's area follows.
    #[test]
    fn reshaped_wings_match_the_originals_editor() {
        let mut jet = shape(&[
            ("WING_SPAN", 0.2),
            ("ROOT_LENGTH", 0.33),
            ("TIP_LENGTH", 0.03),
            ("WING_ANGLE", 0.36),
            ("ROOT_THICKNESS", 0.1),
            ("TIP_THICKNESS", 0.04),
            ("TIP_ANGLE", 0.9),
            ("CONTROL_SPAN", 0.5),
            ("CONTROL_LENGTH", 0.8),
            ("CONTROL_POSITION", 0.3),
        ]);
        let r = evaluate("wing_1v_procedural_light", &jet, false).unwrap();
        assert!(
            (r.fixed_lift_coeff - 2.13668).abs() < 0.02,
            "{}",
            r.fixed_lift_coeff
        );
        assert!(
            (r.moving_lift_coeff - 0.48244).abs() < 0.01,
            "{}",
            r.moving_lift_coeff
        );
        jet.set_slider("CONTROL_SPAN", 1.0);
        jet.set_slider("CONTROL_LENGTH", 0.5);
        jet.set_slider("CONTROL_POSITION", 1.0);
        jet.set_slider("WING_SPAN", 0.6);
        let r = evaluate("wing_1v_procedural_light", &jet, false).unwrap();
        assert!(
            (r.fixed_lift_coeff - 5.64518).abs() < 0.08,
            "{}",
            r.fixed_lift_coeff
        );
        assert!(
            (r.moving_lift_coeff - 1.99763).abs() < 0.05,
            "{}",
            r.moving_lift_coeff
        );
        // the stock jet wing without its control surface (mesh option 0): flight mass 0.18388 t
        let mut plain = shape(&[
            ("WING_SPAN", 0.2),
            ("ROOT_LENGTH", 0.33),
            ("TIP_LENGTH", 0.03),
            ("WING_ANGLE", 0.36),
            ("ROOT_THICKNESS", 0.1),
            ("TIP_THICKNESS", 0.04),
            ("TIP_ANGLE", 0.9),
        ]);
        plain.option = 0;
        let r = evaluate("wing_1v_procedural_light", &plain, false).unwrap();
        assert_eq!(r.moving_lift_coeff, 0.0);
        assert!((r.mass_t - 0.18388).abs() < 0.002, "{}", r.mass_t);
    }

    /// The Jumping Flea's fins at their default shape weigh 0.0028657 t in the original's flight scene.
    #[test]
    fn default_fin_mass_matches_the_original() {
        let r = evaluate("fin_0v_procedural", &WingShape::default(), false).unwrap();
        assert!((r.mass_t - 0.0028657).abs() < 0.00002, "{}", r.mass_t);
    }

    /// `BasePartDensity` was set so the default shape weighs about the part's catalogue mass.
    #[test]
    fn default_shapes_weigh_about_their_catalogue_mass() {
        for (id, rig) in &rigs().parts {
            let option = rig.options.len() as u32 - 1;
            let r = evaluate(
                id,
                &WingShape {
                    option,
                    ..Default::default()
                },
                false,
            )
            .unwrap();
            let ratio = r.mass_t / rig.mass_t;
            assert!(
                (0.9..1.12).contains(&ratio),
                "{id}: {} vs {}",
                r.mass_t,
                rig.mass_t
            );
        }
    }

    #[test]
    fn longer_span_lifts_and_weighs_more_and_mirroring_keeps_the_numbers() {
        let small = evaluate(
            "wing_1v_procedural_light",
            &shape(&[("WING_SPAN", 0.1)]),
            false,
        )
        .unwrap();
        let big = evaluate(
            "wing_1v_procedural_light",
            &shape(&[("WING_SPAN", 0.6)]),
            false,
        )
        .unwrap();
        assert!(big.fixed_lift_coeff > small.fixed_lift_coeff * 1.5);
        assert!(big.mass_t > small.mass_t * 1.5);
        assert!(
            big.cube.size[2] > small.cube.size[2] * 1.5,
            "span runs along part Z"
        );
        let mirrored = evaluate(
            "wing_1v_procedural_light",
            &shape(&[("WING_SPAN", 0.6)]),
            true,
        )
        .unwrap();
        assert!((mirrored.mass_t - big.mass_t).abs() < 1e-9);
        assert!((mirrored.fixed_lift_coeff - big.fixed_lift_coeff).abs() < 1e-9);
        assert!((mirrored.cube.center[0] + big.cube.center[0]).abs() < 1e-9);
    }

    /// The editor's readouts (`SliderValueMeasurementStorage`): the span runs root to tip.
    #[test]
    fn span_measurement_is_the_wing_span() {
        let r = evaluate(
            "wing_1v_procedural_light",
            &shape(&[("WING_SPAN", 0.2)]),
            false,
        )
        .unwrap();
        let span = r
            .measurements
            .iter()
            .find(|(n, _)| n == "WING_SPAN")
            .unwrap()
            .1;
        assert!(
            (span - r.cube.size[2]).abs() < 0.3,
            "{span} vs {:?}",
            r.cube.size
        );
    }

    #[test]
    fn shapes_reject_unknown_sliders_and_out_of_range_values() {
        assert!(shape(&[("WING_SPAN", 0.5)]).valid_for("wing_1v_procedural_light"));
        assert!(!shape(&[("WING_SPAN", 1.5)]).valid_for("wing_1v_procedural_light"));
        assert!(!shape(&[("NOSE", 0.5)]).valid_for("wing_1v_procedural_light"));
        assert!(!shape(&[]).valid_for("engine_1v_methalox_reliant"));
    }
}

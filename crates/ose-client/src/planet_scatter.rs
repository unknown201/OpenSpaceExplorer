//! Surface scatter (rocks, boulders, plants, trees) near the camera, after the original's Vegetation Studio Pro
//! packages (`tools/export_scatter.py`, `assets/planets/<Body>/scatter.json`): each biome mask channel has a
//! package, each item a prefab placed on a grid of `spacing_m` with a density, a scale range and height,
//! steepness and noise rules, drawn within its render distance.
//!
//! The grid is laid on the cube face the camera is over (face coordinates from `planet_lod::face_ab`), so it is
//! fixed to the planet and the same rocks come back in the same places. Every cell's jitter, density test, scale
//! and turn come from a hash of the cell and the item. Plain f64, no GPU: tested natively.

use crate::planet_lod::{self, dot, length, normalize, scale, sub, V3};
use ose_data::terrain::TerrainMap;
use serde::Deserialize;

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScatterFile {
    pub id: String,
    #[serde(default)]
    pub sea_level: f64,
    pub packages: Vec<Package>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    pub name: String,
    pub channel: String,
    #[serde(default)]
    pub cutoff: f64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    pub name: String,
    pub model: String,
    pub kind: String,
    /// Largest size of the prop at its largest scale (m).
    #[serde(default = "one")]
    pub size_m: f64,
    pub spacing_m: f64,
    pub density: f64,
    pub scale: [f64; 2],
    #[serde(default)]
    pub y_offset: [f64; 2],
    #[serde(default)]
    pub seed: i64,
    pub render_distance_m: f64,
    pub height_rule: Option<[f64; 2]>,
    pub steepness_rule: Option<[f64; 2]>,
    /// (scale m, inverse)
    pub noise_density: Option<(f64, bool)>,
    /// (scale m, cutoff, inverse)
    pub noise_cutoff: Option<(f64, f64, bool)>,
    /// Grass: the patch texture, the dry and healthy tints and the size of the tint pattern (m).
    #[serde(default)]
    pub texture: Option<String>,
    #[serde(default)]
    pub tint: Option<[[f64; 3]; 2]>,
    #[serde(default)]
    pub tint_scale_m: Option<f64>,
    /// The original's Amplify impostor for drawing this prop far away.
    #[serde(default)]
    pub impostor: Option<Impostor>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Impostor {
    pub texture: String,
    pub frames: u32,
    #[serde(default)]
    pub hemi: bool,
    pub size_m: f64,
    /// Centre of the impostor's quad in the prop's own (Unity) frame.
    pub offset: [f64; 3],
    /// Colour scale that makes the atlas match the mesh's colour maps.
    #[serde(default = "ones")]
    pub colour_scale: [f64; 3],
}

fn ones() -> [f64; 3] {
    [1.0; 3]
}

/// One far prop drawn as its impostor, relative to the placement origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Far {
    pub pos: [f32; 3],
    /// Impostor quad size and centre height (m), already scaled.
    pub size: f32,
    pub lift: f32,
    /// Index into `ScatterFile::impostor_textures()`.
    pub layer: f32,
    pub frames: f32,
    pub colour: [f32; 3],
}

/// Impostors are drawn out to this distance (m), at most this many.
pub const FAR_RANGE_M: f64 = 2500.0;
pub const MAX_FAR: usize = 40_000;

/// One grass patch, relative to the placement origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grass {
    pub pos: [f32; 3],
    pub up: [f32; 3],
    pub scale: f32,
    pub yaw: f32,
    /// Index into `ScatterFile::grass_textures()`.
    pub layer: f32,
    /// 0 dry .. 1 healthy
    pub tint: f32,
}

/// Grass is drawn this far out at most (m), and at most this many patches.
pub const GRASS_RANGE_M: f64 = 55.0;
pub const MAX_GRASS: usize = 14_000;

/// A rectangle on the ground kept free of scatter (the KSC campus, its runway and pads): `origin` and the two
/// horizontal axes of its own frame (model frame, unit), and its extent along them (m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exclusion {
    pub origin: V3,
    pub ax: V3,
    pub az: V3,
    pub x: [f64; 2],
    pub z: [f64; 2],
}

impl Exclusion {
    /// 1 well inside the rectangle shrunk by `inset` m, falling to 0 over `falloff` m towards its edge.
    pub fn inner_weight(&self, p: V3, inset: f64, falloff: f64) -> f64 {
        let d = sub(p, self.origin);
        let (x, z) = (dot(d, self.ax), dot(d, self.az));
        let edge = (x - (self.x[0] + inset))
            .min((self.x[1] - inset) - x)
            .min(z - (self.z[0] + inset))
            .min((self.z[1] - inset) - z);
        (edge / falloff.max(1e-6)).clamp(0.0, 1.0)
    }

    pub fn contains(&self, p: V3) -> bool {
        let d = sub(p, self.origin);
        let (x, z) = (dot(d, self.ax), dot(d, self.az));
        x >= self.x[0] && x <= self.x[1] && z >= self.z[0] && z <= self.z[1]
    }
}

fn excluded(ex: &[Exclusion], p: V3) -> bool {
    ex.iter().any(|e| e.contains(p))
}

/// One placed prop.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// Index into `ScatterFile::models()`.
    pub model: usize,
    /// Base of the prop in the model frame (m).
    pub pos: V3,
    /// Surface normal.
    pub up: V3,
    pub yaw: f64,
    pub scale: f64,
}

/// Instances closer together than this many grid cells across are dropped: the renderer draws props one by one.
pub const MAX_CELLS_ACROSS: i64 = 26;
/// Most props of one item drawn at once (nearest first), and of all items.
pub const MAX_PER_ITEM: usize = 120;
pub const MAX_INSTANCES: usize = 900;
/// Props smaller than this are left to the ground texture.
pub const MIN_SIZE_M: f64 = 0.4;

fn hash(a: i64, b: i64, c: i64, d: i64) -> u64 {
    let mut h = 0x9e37_79b9_7f4a_7c15u64;
    for v in [a, b, c, d] {
        h ^= v as u64;
        h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h ^= h >> 31;
    }
    h
}

fn unit(h: u64, k: u32) -> f64 {
    ((h.rotate_left(k * 13) >> 11) as f64) / ((1u64 << 53) as f64)
}

/// Smooth value noise on the plane of the face coordinates, 0..1, period-free.
fn noise(x: f64, y: f64, seed: i64) -> f64 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let v = |i: f64, j: f64| unit(hash(i as i64, j as i64, seed, 77), 0);
    let a = v(xi, yi) + (v(xi + 1.0, yi) - v(xi, yi)) * ux;
    let b = v(xi, yi + 1.0) + (v(xi + 1.0, yi + 1.0) - v(xi, yi + 1.0)) * ux;
    a + (b - a) * uy
}

impl ScatterFile {
    /// Every item's model path, in package and item order (the `model` index of an instance).
    pub fn models(&self) -> Vec<&str> {
        self.packages
            .iter()
            .flat_map(|p| p.items.iter().map(|i| i.model.as_str()))
            .collect()
    }

    /// Props around `cam` (model frame, m) on a body of `radius` with terrain `map`.
    pub fn place(
        &self,
        map: &TerrainMap,
        radius: f64,
        cam: V3,
        exclude: &[Exclusion],
    ) -> Vec<Instance> {
        let mut out: Vec<(f64, Instance)> = Vec::new();
        let cam_dir = normalize(cam);
        let (face, ca, cb) = planet_lod::face_ab(cam_dir);
        let metres_per_unit = radius * std::f64::consts::FRAC_PI_4;
        let height = |d: V3| planet_lod::map_height(Some(map), d);
        // height above the ground under the camera
        let cam_alt = length(cam) - radius - height(cam_dir);
        let mut model = 0usize;
        for pk in &self.packages {
            let ch = match pk.channel.as_str() {
                "R" => 0,
                "G" => 1,
                "B" => 2,
                _ => 3,
            };
            for (item_i, item) in pk.items.iter().enumerate() {
                let this_model = model;
                model += 1;
                if item.kind == "grass" {
                    continue;
                }
                // items with an impostor hand over to it where `place_far` starts
                let range = Self::near_range(item);
                if cam_alt > range || item.size_m < MIN_SIZE_M {
                    continue;
                }
                let first = out.len();
                let step = item.spacing_m / metres_per_unit;
                let ci = (ca / step).round() as i64;
                let cj = (cb / step).round() as i64;
                let n = (range / item.spacing_m).ceil() as i64 + 1;
                let seed = item.seed + item_i as i64 * 1009 + ch as i64 * 7919;
                for i in ci - n..=ci + n {
                    for j in cj - n..=cj + n {
                        let h = hash(face as i64, i, j, seed);
                        if unit(h, 1) > item.density {
                            continue;
                        }
                        let a = (i as f64 + unit(h, 2) - 0.5) * step;
                        let b = (j as f64 + unit(h, 3) - 0.5) * step;
                        if a.abs() > 1.0 || b.abs() > 1.0 {
                            continue;
                        }
                        let d = planet_lod::dir_from_ab(face, a, b);
                        // distance along the ground first (cheap), then the rules
                        let ground_dist = dot(d, cam_dir).clamp(-1.0, 1.0).acos() * radius;
                        if ground_dist > range {
                            continue;
                        }
                        let (lat, lon) = planet_lod::lat_lon(d);
                        let w = map
                            .biome_weights(lat, lon)
                            .map(|w| {
                                let s = w[0] + w[1] + w[2] + w[3];
                                // as the shader: land with no biome weight takes the first biome
                                if s < 0.05 {
                                    let mut f = w;
                                    f[0] += 1.0 - s / 0.05;
                                    f[ch]
                                } else {
                                    w[ch] / s
                                }
                            })
                            .unwrap_or(if ch == 0 { 1.0 } else { 0.0 });
                        if w <= pk.cutoff || unit(h, 4) > w {
                            continue;
                        }
                        let hgt = height(d);
                        if map.has_ocean && hgt < self.sea_level {
                            continue;
                        }
                        if let Some([lo, hi]) = item.height_rule {
                            if hgt < lo || hgt > hi {
                                continue;
                            }
                        }
                        let (nx, ny) = (a * metres_per_unit, b * metres_per_unit);
                        if let Some((sc, inverse)) = item.noise_density {
                            let v = noise(nx / sc.max(1.0), ny / sc.max(1.0), seed);
                            let v = if inverse { 1.0 - v } else { v };
                            if unit(h, 5) > v {
                                continue;
                            }
                        }
                        if let Some((sc, cut, inverse)) = item.noise_cutoff {
                            let v = noise(nx / sc.max(1.0), ny / sc.max(1.0), seed + 1);
                            if (v < cut) != inverse {
                                continue;
                            }
                        }
                        // normal from the terrain around the point (one metre each way)
                        let e = 1.0 / radius;
                        let east = normalize(planet_lod::cross([0.0, 1.0, 0.0], d));
                        let north = planet_lod::cross(d, east);
                        let at = |o: V3| {
                            let dd = normalize(planet_lod::add(d, o));
                            scale(dd, radius + height(dd))
                        };
                        let pe = at(scale(east, e * 2.0));
                        let pn = at(scale(north, e * 2.0));
                        let p0 = scale(d, radius + hgt);
                        let mut up = normalize(planet_lod::cross(sub(pe, p0), sub(pn, p0)));
                        if dot(up, d) < 0.0 {
                            up = scale(up, -1.0);
                        }
                        if let Some([lo, hi]) = item.steepness_rule {
                            let slope = dot(up, d).clamp(-1.0, 1.0).acos().to_degrees();
                            if slope < lo || slope > hi {
                                continue;
                            }
                        }
                        let s = item.scale[0] + (item.scale[1] - item.scale[0]) * unit(h, 6);
                        let yo =
                            item.y_offset[0] + (item.y_offset[1] - item.y_offset[0]) * unit(h, 7);
                        let pos = scale(d, radius + hgt + yo * s);
                        if excluded(exclude, pos) {
                            continue;
                        }
                        let dist = length(sub(pos, cam));
                        out.push((
                            dist,
                            Instance {
                                model: this_model,
                                pos,
                                up,
                                yaw: unit(h, 8) * std::f64::consts::TAU,
                                scale: s,
                            },
                        ));
                    }
                }
                // nearest of this item only
                out[first..]
                    .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                out.truncate(first + MAX_PER_ITEM);
            }
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(MAX_INSTANCES);
        out.into_iter().map(|(_, i)| i).collect()
    }
}

impl ScatterFile {
    /// The impostor atlases, in order (the `layer` of a far prop).
    pub fn impostor_textures(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for i in self.packages.iter().flat_map(|p| &p.items) {
            if let Some(t) = i.impostor.as_ref().map(|m| m.texture.as_str()) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        out
    }

    /// Where an item's meshes stop and its impostors take over (m): its mesh range, or nearer when the
    /// per-item mesh cap runs out first.
    pub fn near_range(item: &Item) -> f64 {
        let range = item
            .render_distance_m
            .min(item.spacing_m * MAX_CELLS_ACROSS as f64 / 2.0)
            .min(item.size_m * 600.0);
        let cap = item.spacing_m
            * (MAX_PER_ITEM as f64 / (std::f64::consts::PI * item.density.max(0.05))).sqrt();
        range.min(cap * 0.95)
    }

    /// Far props (items with an impostor) between their mesh range and `FAR_RANGE_M`, positions relative to
    /// `cam`. Heights come from a coarse grid of terrain samples; the biome, height, noise and exclusion rules
    /// apply as for meshes.
    pub fn place_far(
        &self,
        map: &TerrainMap,
        radius: f64,
        cam: V3,
        exclude: &[Exclusion],
    ) -> Vec<Far> {
        let textures = self.impostor_textures();
        let cam_dir = normalize(cam);
        let height = |d: V3| planet_lod::map_height(Some(map), d);
        if textures.is_empty() || length(cam) - radius - height(cam_dir) > FAR_RANGE_M {
            return Vec::new();
        }
        let range = FAR_RANGE_M;
        let east = normalize(planet_lod::cross([0.0, 1.0, 0.0], cam_dir));
        let north = planet_lod::cross(cam_dir, east);
        const N: usize = 48;
        let step = 2.0 * range / N as f64;
        let mut grid = vec![0.0f64; (N + 1) * (N + 1)];
        for j in 0..=N {
            for i in 0..=N {
                let x = -range + i as f64 * step;
                let y = -range + j as f64 * step;
                let d = normalize(planet_lod::add(
                    scale(cam_dir, radius),
                    planet_lod::add(scale(east, x), scale(north, y)),
                ));
                grid[j * (N + 1) + i] = height(d);
            }
        }
        let grid_h = |x: f64, y: f64| {
            let fx = ((x + range) / step).clamp(0.0, N as f64 - 1e-9);
            let fy = ((y + range) / step).clamp(0.0, N as f64 - 1e-9);
            let (i, j) = (fx as usize, fy as usize);
            let (tx, ty) = (fx - i as f64, fy - j as f64);
            let g = |a: usize, b: usize| grid[b * (N + 1) + a];
            let h0 = g(i, j) + (g(i + 1, j) - g(i, j)) * tx;
            let h1 = g(i, j + 1) + (g(i + 1, j + 1) - g(i, j + 1)) * tx;
            h0 + (h1 - h0) * ty
        };
        let (face, ca, cb) = planet_lod::face_ab(cam_dir);
        let metres_per_unit = radius * std::f64::consts::FRAC_PI_4;
        let mut out = Vec::new();
        for pk in &self.packages {
            let ch = match pk.channel.as_str() {
                "R" => 0,
                "G" => 1,
                "B" => 2,
                _ => 3,
            };
            for (item_i, item) in pk.items.iter().enumerate() {
                let Some(imp) = &item.impostor else { continue };
                let layer = textures.iter().position(|t| *t == imp.texture).unwrap_or(0) as f32;
                let near = Self::near_range(item);
                let step_u = item.spacing_m / metres_per_unit;
                let n = (range / item.spacing_m).ceil() as i64 + 1;
                let (ic, jc) = ((ca / step_u).round() as i64, (cb / step_u).round() as i64);
                // the same seed as the meshes, so a prop keeps its place when it turns into an impostor
                let seed = item.seed + item_i as i64 * 1009 + ch as i64 * 7919;
                for i in ic - n..=ic + n {
                    for j in jc - n..=jc + n {
                        let h = hash(face as i64, i, j, seed);
                        if unit(h, 1) > item.density {
                            continue;
                        }
                        let a = (i as f64 + unit(h, 2) - 0.5) * step_u;
                        let b = (j as f64 + unit(h, 3) - 0.5) * step_u;
                        if a.abs() > 1.0 || b.abs() > 1.0 {
                            continue;
                        }
                        let d = planet_lod::dir_from_ab(face, a, b);
                        let rel = scale(sub(d, cam_dir), radius);
                        let (x, y) = (dot(rel, east), dot(rel, north));
                        let r2 = x * x + y * y;
                        if r2 > range * range || r2 < near * near {
                            continue;
                        }
                        let (lat, lon) = planet_lod::lat_lon(d);
                        let w = map
                            .biome_weights(lat, lon)
                            .map(|w| {
                                let s = w[0] + w[1] + w[2] + w[3];
                                if s < 0.05 {
                                    let mut f = w;
                                    f[0] += 1.0 - s / 0.05;
                                    f[ch]
                                } else {
                                    w[ch] / s
                                }
                            })
                            .unwrap_or(if ch == 0 { 1.0 } else { 0.0 });
                        if w <= pk.cutoff || unit(h, 4) > w {
                            continue;
                        }
                        let hgt = grid_h(x, y);
                        if map.has_ocean && hgt < self.sea_level {
                            continue;
                        }
                        if let Some([lo, hi]) = item.height_rule {
                            if hgt < lo || hgt > hi {
                                continue;
                            }
                        }
                        let (nx, ny) = (a * metres_per_unit, b * metres_per_unit);
                        if let Some((sc, inverse)) = item.noise_density {
                            let v = noise(nx / sc.max(1.0), ny / sc.max(1.0), seed);
                            let v = if inverse { 1.0 - v } else { v };
                            if unit(h, 5) > v {
                                continue;
                            }
                        }
                        if let Some((sc, cut, inverse)) = item.noise_cutoff {
                            let v = noise(nx / sc.max(1.0), ny / sc.max(1.0), seed + 1);
                            if (v < cut) != inverse {
                                continue;
                            }
                        }
                        let world = scale(d, radius + hgt);
                        if excluded(exclude, world) {
                            continue;
                        }
                        let pos = sub(world, cam);
                        let s = item.scale[0] + (item.scale[1] - item.scale[0]) * unit(h, 6);
                        out.push(Far {
                            pos: [pos[0] as f32, pos[1] as f32, pos[2] as f32],
                            size: (imp.size_m * s) as f32,
                            lift: (imp.offset[1] * s) as f32,
                            layer,
                            frames: imp.frames as f32,
                            colour: [
                                imp.colour_scale[0] as f32,
                                imp.colour_scale[1] as f32,
                                imp.colour_scale[2] as f32,
                            ],
                        });
                    }
                }
            }
        }
        if out.len() > MAX_FAR {
            out.sort_by(|p, q| {
                let dp = p.pos[0] * p.pos[0] + p.pos[1] * p.pos[1] + p.pos[2] * p.pos[2];
                let dq = q.pos[0] * q.pos[0] + q.pos[1] * q.pos[1] + q.pos[2] * q.pos[2];
                dp.partial_cmp(&dq).unwrap_or(std::cmp::Ordering::Equal)
            });
            out.truncate(MAX_FAR);
        }
        out
    }

    /// The grass items' textures, in order (the `layer` of a patch).
    pub fn grass_textures(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for i in self.packages.iter().flat_map(|p| &p.items) {
            if let Some(t) = i.texture.as_deref() {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        out
    }

    /// Grass patches within `GRASS_RANGE_M` of the camera's ground point, positions relative to `origin` (the
    /// camera position the placement is for). Heights come from a small grid of terrain samples around the
    /// camera, bilinearly interpolated: thousands of exact samples would cost too much per placement.
    pub fn place_grass(
        &self,
        map: &TerrainMap,
        radius: f64,
        cam: V3,
        exclude: &[Exclusion],
    ) -> Vec<Grass> {
        let textures = self.grass_textures();
        let cam_dir = normalize(cam);
        let height = |d: V3| planet_lod::map_height(Some(map), d);
        let ground = height(cam_dir);
        if length(cam) - radius - ground > GRASS_RANGE_M * 0.8 || textures.is_empty() {
            return Vec::new();
        }
        let range = GRASS_RANGE_M;
        // local tangent frame and height grid
        let east = normalize(planet_lod::cross([0.0, 1.0, 0.0], cam_dir));
        let north = planet_lod::cross(cam_dir, east);
        const N: usize = 24;
        let step = 2.0 * range / N as f64;
        let mut grid = vec![0.0f64; (N + 1) * (N + 1)];
        for j in 0..=N {
            for i in 0..=N {
                let x = -range + i as f64 * step;
                let y = -range + j as f64 * step;
                let d = normalize(planet_lod::add(
                    scale(cam_dir, radius),
                    planet_lod::add(scale(east, x), scale(north, y)),
                ));
                grid[j * (N + 1) + i] = height(d);
            }
        }
        let grid_h = |x: f64, y: f64| {
            let fx = ((x + range) / step).clamp(0.0, N as f64 - 1e-9);
            let fy = ((y + range) / step).clamp(0.0, N as f64 - 1e-9);
            let (i, j) = (fx as usize, fy as usize);
            let (tx, ty) = (fx - i as f64, fy - j as f64);
            let g = |a: usize, b: usize| grid[b * (N + 1) + a];
            let h0 = g(i, j) + (g(i + 1, j) - g(i, j)) * tx;
            let h1 = g(i, j + 1) + (g(i + 1, j + 1) - g(i, j + 1)) * tx;
            let h = h0 + (h1 - h0) * ty;
            // slope from the grid cell
            let dx = ((g(i + 1, j) - g(i, j)) + (g(i + 1, j + 1) - g(i, j + 1))) * 0.5 / step;
            let dy = ((g(i, j + 1) - g(i, j)) + (g(i + 1, j + 1) - g(i + 1, j))) * 0.5 / step;
            (h, dx, dy)
        };
        let (face, ca, cb) = planet_lod::face_ab(cam_dir);
        let metres_per_unit = radius * std::f64::consts::FRAC_PI_4;
        let mut out = Vec::new();
        for (ci, pk) in self.packages.iter().enumerate() {
            let ch = match pk.channel.as_str() {
                "R" => 0,
                "G" => 1,
                "B" => 2,
                _ => 3,
            };
            for (item_i, item) in pk.items.iter().enumerate() {
                let Some(tex) = item.texture.as_deref() else {
                    continue;
                };
                let layer = textures.iter().position(|t| *t == tex).unwrap_or(0) as f32;
                let spacing = item.spacing_m.max(0.8);
                let step_u = spacing / metres_per_unit;
                let n = (range / spacing).ceil() as i64 + 1;
                let (ic, jc) = ((ca / step_u).round() as i64, (cb / step_u).round() as i64);
                let seed = item.seed + item_i as i64 * 1009 + ci as i64 * 7919 + 31;
                let tint_scale = item.tint_scale_m.unwrap_or(100.0).max(1.0);
                for i in ic - n..=ic + n {
                    for j in jc - n..=jc + n {
                        let h = hash(face as i64, i, j, seed);
                        if unit(h, 1) > item.density {
                            continue;
                        }
                        let a = (i as f64 + unit(h, 2) - 0.5) * step_u;
                        let b = (j as f64 + unit(h, 3) - 0.5) * step_u;
                        if a.abs() > 1.0 || b.abs() > 1.0 {
                            continue;
                        }
                        let d = planet_lod::dir_from_ab(face, a, b);
                        let rel = scale(sub(d, cam_dir), radius);
                        let (x, y) = (dot(rel, east), dot(rel, north));
                        if x * x + y * y > range * range {
                            continue;
                        }
                        let (lat, lon) = planet_lod::lat_lon(d);
                        let w = map
                            .biome_weights(lat, lon)
                            .map(|w| {
                                let s = w[0] + w[1] + w[2] + w[3];
                                if s < 0.05 {
                                    let mut f = w;
                                    f[0] += 1.0 - s / 0.05;
                                    f[ch]
                                } else {
                                    w[ch] / s
                                }
                            })
                            .unwrap_or(if ch == 0 { 1.0 } else { 0.0 });
                        if w <= pk.cutoff || unit(h, 4) > w {
                            continue;
                        }
                        let (hgt, dx, dy) = grid_h(x, y);
                        if map.has_ocean && hgt < self.sea_level + 0.5 {
                            continue;
                        }
                        if let Some([lo, hi]) = item.height_rule {
                            if hgt < lo || hgt > hi {
                                continue;
                            }
                        }
                        let slope = (dx * dx + dy * dy).sqrt().atan().to_degrees();
                        if slope > 35.0 {
                            continue;
                        }
                        if let Some([lo, hi]) = item.steepness_rule {
                            if slope < lo || slope > hi {
                                continue;
                            }
                        }
                        let (nx, ny) = (a * metres_per_unit, b * metres_per_unit);
                        if let Some((sc, inverse)) = item.noise_density {
                            let v = noise(nx / sc.max(1.0), ny / sc.max(1.0), seed);
                            let v = if inverse { 1.0 - v } else { v };
                            if unit(h, 5) > v {
                                continue;
                            }
                        }
                        let up = normalize(planet_lod::add(
                            d,
                            planet_lod::add(scale(east, -dx), scale(north, -dy)),
                        ));
                        let world = scale(d, radius + hgt);
                        if excluded(exclude, world) {
                            continue;
                        }
                        let pos = sub(world, cam);
                        let s = item.scale[0] + (item.scale[1] - item.scale[0]) * unit(h, 6);
                        out.push(Grass {
                            pos: [pos[0] as f32, pos[1] as f32, pos[2] as f32],
                            up: [up[0] as f32, up[1] as f32, up[2] as f32],
                            scale: s as f32,
                            yaw: (unit(h, 8) * std::f64::consts::TAU) as f32,
                            layer,
                            tint: noise(nx / tint_scale, ny / tint_scale, seed + 5) as f32,
                        });
                    }
                }
            }
        }
        if out.len() > MAX_GRASS {
            out.sort_by(|p, q| {
                let dp = p.pos[0] * p.pos[0] + p.pos[1] * p.pos[1] + p.pos[2] * p.pos[2];
                let dq = q.pos[0] * q.pos[0] + q.pos[1] * q.pos[1] + q.pos[2] * q.pos[2];
                dp.partial_cmp(&dq).unwrap_or(std::cmp::Ordering::Equal)
            });
            out.truncate(MAX_GRASS);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_data::terrain::{BodyTerrain, MapRef};

    fn flat_map(radius: f64) -> TerrainMap {
        let blob = vec![0u8; 16 * 8 * 2];
        let entry = BodyTerrain {
            id: "T".into(),
            has_ocean: false,
            sites: vec![],
            map: Some(MapRef {
                offset: 0,
                width: 16,
                height: 8,
                scale_m: 1000.0,
                offset_m: 0.0,
                source: String::new(),
            }),
        };
        TerrainMap::from_entry(&entry, &blob, radius).unwrap()
    }

    fn file() -> ScatterFile {
        serde_json::from_str(
            r#"{"id":"T","packages":[{"name":"p","channel":"R","items":[
                {"name":"rock","model":"scatter/rock.glb","kind":"object","spacing_m":10,"density":0.5,
                 "scale":[1,2],"render_distance_m":100,"height_rule":null,"steepness_rule":null,
                 "noise_density":null,"noise_cutoff":null}]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn props_sit_on_the_ground_near_the_camera_and_stay_put() {
        let r = 200_000.0;
        let map = flat_map(r);
        let f = file();
        let cam = scale(planet_lod::direction(10.0, 20.0), r + 20.0);
        let a = f.place(&map, r, cam, &[]);
        // a 100 m disc of 10 m cells at half density: roughly 50 to 200 rocks
        assert!(a.len() > 40 && a.len() < 250, "{}", a.len());
        for i in &a {
            assert!((length(i.pos) - r).abs() < 1e-6);
            assert!(length(sub(i.pos, cam)) < 130.0);
            assert!(i.scale >= 1.0 && i.scale <= 2.0);
        }
        // moving the camera a little keeps the rocks that stay in range exactly where they were
        let cam2 = scale(planet_lod::direction(10.0, 20.0001), r + 20.0);
        let b = f.place(&map, r, cam2, &[]);
        let same = a
            .iter()
            .filter(|i| b.iter().any(|j| length(sub(i.pos, j.pos)) < 1e-6))
            .count();
        assert!(same as f64 > a.len() as f64 * 0.8, "{same} of {}", a.len());
        // too high above the ground: nothing
        let high = scale(planet_lod::direction(10.0, 20.0), r + 500.0);
        assert!(f.place(&map, r, high, &[]).is_empty());
    }

    #[test]
    fn grass_covers_the_ground_around_the_camera() {
        let r = 600_000.0;
        let map = flat_map(r);
        let f: ScatterFile = serde_json::from_str(
            r#"{"id":"T","packages":[{"name":"p","channel":"R","items":[
                {"name":"grass","model":"","texture":"textures/g.png","kind":"grass","spacing_m":1.4,
                 "density":1,"scale":[0.8,1.5],"render_distance_m":150,"height_rule":null,
                 "steepness_rule":null,"noise_density":null,"noise_cutoff":null,
                 "tint":[[1,1,1],[0.5,0.5,0.5]],"tint_scale_m":100}]}]}"#,
        )
        .unwrap();
        let cam = scale(planet_lod::direction(-0.7, -74.8), r + 2.0);
        let g = f.place_grass(&map, r, cam, &[]);
        // pi 55^2 / 1.4^2: about 4,800 patches
        assert!(g.len() > 3_500 && g.len() < 6_500, "{}", g.len());
        for p in &g {
            let d = (p.pos[0] * p.pos[0] + p.pos[1] * p.pos[1] + p.pos[2] * p.pos[2]).sqrt();
            assert!(d < 60.0);
        }
        // props skip grass items
        assert!(f.place(&map, r, cam, &[]).is_empty());
    }
}

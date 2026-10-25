//! The rest of the original's terrain height (`PQSJobUtil.HeightSample`, `DoDecals`, `DrawDecal`) on top of the
//! global map: tiled large and medium height layers weighted by the biome mask, and the PQS height decals. Read
//! from a per-body `TerrainPack` (JSON description plus one little-endian binary blob) written by
//! `tools/extract_terrain.py --packs`; see docs/reference-notes/terrain.md.
//!
//! Directions here are the original's body-fixed Unity frame: latitude/longitude `(lat, lon)` is
//! `(cos lat cos lon, sin lat, cos lat sin lon)` (`LatLon.GetRelSurfaceNVector`).

use crate::terrain::{flats_for, BodyTerrain, MapRef, Site, TerrainMap};
use serde::{Deserialize, Serialize};

/// A u16 grid (or a stack of `count` grids) in the pack blob.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridRef {
    pub offset: usize,
    pub width: usize,
    pub height: usize,
    #[serde(default = "one")]
    pub count: usize,
}

fn one() -> usize {
    1
}

/// One tiled height layer: a u16 map repeated `uv_scale` times around the body, `height_scale` m at full value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerRef {
    pub grid: GridRef,
    pub uv_scale: f64,
    pub height_scale: f64,
}

/// One placed height decal, as the original's `PQSDecalInstance` resolved through `GetInstanceInfo`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecalRef {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub rotation_deg: f64,
    pub scale_m: f64,
    /// Slice in the decal height (and alpha) stacks.
    pub index: usize,
    pub height_scale: f64,
    pub height_offset: f64,
    /// `FadeShape * HeightBlendMode`: shape -1 square, 1 circular; mode 1 subtract, 2 replace, 3 add.
    pub blend: i32,
    pub fade_strength: f64,
    pub vertical_offset: f64,
    pub use_alpha: bool,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainPack {
    pub version: u32,
    pub id: String,
    pub radius_m: f64,
    pub has_ocean: bool,
    pub sites: Vec<Site>,
    pub global: MapRef,
    /// The biome mask, RGBA8 (four bytes per texel, not u16).
    pub mask: Option<GridRef>,
    pub contrast: f64,
    /// R, G, B, A layers (missing ones `None`).
    pub large: Vec<Option<LayerRef>>,
    pub medium: Vec<Option<LayerRef>>,
    pub decal_heights: Option<GridRef>,
    pub decal_alpha: Option<GridRef>,
    pub decals: Vec<DecalRef>,
}

#[derive(Clone, Debug, PartialEq)]
struct Grid {
    w: usize,
    h: usize,
    count: usize,
    data: Vec<u16>,
}

impl Grid {
    fn read(r: &GridRef, blob: &[u8]) -> Option<Grid> {
        let n = r.width * r.height * r.count.max(1);
        let bytes = blob.get(r.offset..r.offset + n * 2)?;
        Some(Grid {
            w: r.width,
            h: r.height,
            count: r.count.max(1),
            data: bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect(),
        })
    }

    /// The original's `BiquadraticSample`: wraps both ways, texel centres at half texels, 0..1.
    fn biquad(&self, u: f64, v: f64, slice: usize) -> f64 {
        let (w, h) = (self.w as i64, self.h as i64);
        let off = slice.min(self.count - 1) * self.w * self.h;
        let u = u - u.floor();
        let v = v - v.floor();
        let fx = u * w as f64 - 0.5;
        let fy = v * h as f64 - 0.5;
        let x0 = fx.floor() as i64;
        let y0 = fy.floor() as i64;
        let tx = fx - x0 as f64;
        let ty = fy - y0 as f64;
        let at = |x: i64, y: i64| {
            let xi = x.rem_euclid(w) as usize;
            let yi = y.rem_euclid(h) as usize;
            self.data[off + yi * self.w + xi] as f64 / 65535.0
        };
        let quad = |a: f64, b: f64, c: f64, f: f64| {
            let p = a + (b - a) * ((f + 1.0) * 0.5);
            let q = b + (c - b) * (f * 0.5);
            p + (q - p) * f
        };
        let row = |y: i64| quad(at(x0 - 1, y), at(x0, y), at(x0 + 1, y), tx);
        quad(row(y0 - 1), row(y0), row(y0 + 1), ty)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Layer {
    grid: Grid,
    uv_scale: f64,
    height_scale: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct Decal {
    pos: [f64; 3],
    /// Rows of the inverse rotation (world to local).
    inv: [[f64; 3]; 3],
    scale: f64,
    cull2: f64,
    index: usize,
    height_scale: f64,
    height_offset: f64,
    blend: i32,
    fade_strength: f64,
    vertical_offset: f64,
    use_alpha: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    radius_m: f64,
    contrast: f64,
    mask: Option<(usize, usize, Vec<[u8; 4]>)>,
    large: Vec<Option<Layer>>,
    medium: Vec<Option<Layer>>,
    decal_heights: Option<Grid>,
    decal_alpha: Option<Grid>,
    decals: Vec<Decal>,
}

/// Unity direction of a latitude/longitude (degrees).
pub fn unity_dir(lat_deg: f64, lon_deg: f64) -> [f64; 3] {
    let (la, lo) = (lat_deg.to_radians(), lon_deg.to_radians());
    [la.cos() * lo.cos(), la.sin(), la.cos() * lo.sin()]
}

type Quat = [f64; 4]; // x, y, z, w

fn qmul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn qmat(q: Quat) -> [[f64; 3]; 3] {
    let [x, y, z, w] = q;
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

/// `transform.up = n; transform.Rotate(Vector3.up, rotation)`: the shortest rotation taking +Y to `n`, then a
/// turn about the local up axis.
fn decal_rotation(n: [f64; 3], rotation_deg: f64) -> [[f64; 3]; 3] {
    // from (0, 1, 0) to n: axis (n.z, 0, -n.x), angle acos(n.y)
    let d = n[1].clamp(-1.0, 1.0);
    let axis = [n[2], 0.0, -n[0]];
    let al = (axis[0] * axis[0] + axis[2] * axis[2]).sqrt();
    let from_to: Quat = if al < 1e-12 {
        if d > 0.0 {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0, 0.0]
        }
    } else {
        let half = d.acos() * 0.5;
        let s = half.sin() / al;
        [axis[0] * s, 0.0, axis[2] * s, half.cos()]
    };
    let h = rotation_deg.to_radians() * 0.5;
    let spin: Quat = [0.0, h.sin(), 0.0, h.cos()];
    qmat(qmul(from_to, spin))
}

impl Detail {
    fn read(p: &TerrainPack, blob: &[u8]) -> Detail {
        let layer = |l: &Option<LayerRef>| {
            l.as_ref().and_then(|l| {
                Some(Layer {
                    grid: Grid::read(&l.grid, blob)?,
                    uv_scale: l.uv_scale,
                    height_scale: l.height_scale,
                })
            })
        };
        let mut decals: Vec<&DecalRef> = p.decals.iter().collect();
        decals.sort_by_key(|d| d.sort_order);
        let decals = decals
            .into_iter()
            .map(|d| {
                let n = unity_dir(d.lat_deg, d.lon_deg);
                let r = decal_rotation(n, d.rotation_deg);
                let inv = [
                    [r[0][0], r[1][0], r[2][0]],
                    [r[0][1], r[1][1], r[2][1]],
                    [r[0][2], r[1][2], r[2][2]],
                ];
                Decal {
                    pos: [n[0] * p.radius_m, n[1] * p.radius_m, n[2] * p.radius_m],
                    inv,
                    scale: d.scale_m,
                    // the original: lossyScale.sqrMagnitude * 0.5 with a uniform scale
                    cull2: 3.0 * d.scale_m * d.scale_m * 0.5,
                    index: d.index,
                    height_scale: d.height_scale,
                    height_offset: d.height_offset,
                    blend: d.blend,
                    fade_strength: d.fade_strength,
                    vertical_offset: d.vertical_offset,
                    use_alpha: d.use_alpha,
                }
            })
            .collect();
        let mask = p.mask.as_ref().and_then(|m| {
            let bytes = blob.get(m.offset..m.offset + m.width * m.height * 4)?;
            Some((m.width, m.height, bytes.as_chunks::<4>().0.to_vec()))
        });
        Detail {
            radius_m: p.radius_m,
            contrast: p.contrast,
            mask,
            large: p.large.iter().map(layer).collect(),
            medium: p.medium.iter().map(layer).collect(),
            decal_heights: p.decal_heights.as_ref().and_then(|g| Grid::read(g, blob)),
            decal_alpha: p.decal_alpha.as_ref().and_then(|g| Grid::read(g, blob)),
            decals,
        }
    }

    /// The original's bilinear mask sample, wrapping.
    fn mask_at(&self, u: f64, v: f64) -> [f64; 4] {
        let Some((w, h, data)) = &self.mask else {
            return [0.0; 4];
        };
        let (w, h) = (*w, *h);
        let u = u - u.floor();
        let v = v - v.floor();
        let fx = (u * w as f64 - 0.5).max(0.0);
        let fy = (v * h as f64 - 0.5).max(0.0);
        let (x0, y0) = (fx as usize % w, fy as usize % h);
        let (tx, ty) = (fx - fx.floor(), fy - fy.floor());
        let (x1, y1) = ((x0 + 1) % w, (y0 + 1) % h);
        let c = |x: usize, y: usize, k: usize| data[y * w + x][k] as f64 / 255.0;
        let mut out = [0.0; 4];
        for (k, o) in out.iter_mut().enumerate() {
            *o = (c(x0, y0, k) * (1.0 - tx) + c(x1, y0, k) * tx) * (1.0 - ty)
                + (c(x0, y1, k) * (1.0 - tx) + c(x1, y1, k) * tx) * ty;
        }
        out
    }

    fn layers(
        &self,
        set: &[Option<Layer>],
        uv2: [f64; 2],
        top: [f64; 2],
        mask: [f64; 4],
        blend: f64,
    ) -> f64 {
        let mut sum = 0.0;
        for (k, l) in set.iter().enumerate().take(4) {
            let Some(l) = l else { continue };
            let y = l.grid.biquad(uv2[0] * l.uv_scale, uv2[1] * l.uv_scale, 0);
            let x = l.grid.biquad(top[0] * l.uv_scale, top[1] * l.uv_scale, 0);
            sum += (x + (y - x) * blend) * l.height_scale * mask[k];
        }
        sum
    }

    fn draw_decal(&self, d: &Decal, u: f64, v: f64, cur: f64) -> f64 {
        let Some(heights) = &self.decal_heights else {
            return cur;
        };
        let mut h = heights.biquad(u, v, d.index) - 0.5 + d.height_offset;
        let alpha = match (&self.decal_alpha, d.use_alpha) {
            (Some(a), true) => a.biquad(u, v, d.index),
            _ => 1.0,
        };
        let mut mode = d.blend.abs() - 2;
        let (x, y) = (u - 0.5, v - 0.5);
        let shape = if d.blend < 0 {
            (1.0 - 2.0 * x.abs().max(y.abs())).clamp(0.0, 1.0)
        } else {
            (1.0 - 2.0 * (x * x + y * y).sqrt()).clamp(0.0, 1.0)
        };
        let fade = (shape.powf(d.fade_strength) * 2.0).clamp(0.0, 1.0) * alpha;
        let mut cur = cur;
        if mode == 0 {
            cur *= 1.0 - fade;
            mode = 1;
        }
        h *= mode as f64;
        h *= fade;
        cur + h * d.height_scale + d.vertical_offset * fade
    }

    /// Height above the radius before the ocean offset, m.
    pub(crate) fn sample(&self, map: &TerrainMap, lat_deg: f64, lon_deg: f64) -> f64 {
        let global = map.global_m(lat_deg, lon_deg) - map.offset_m;
        let u = (lon_deg / 360.0 + 0.5).rem_euclid(1.0);
        let v = (lat_deg / 180.0 + 0.5).clamp(0.0, 1.0);
        let dir = unity_dir(lat_deg, lon_deg);
        let mut height = global;
        if self.mask.is_some() {
            let mut m = self.mask_at(u, v);
            let k = 1.0 / (m[0] + m[1] + m[2] + m[3]).max(0.001);
            for c in &mut m {
                *c *= k;
            }
            let blend = 1.0 - dir[1].abs().powf(self.contrast);
            let uv2 = [u * 2.0, v];
            let top = [dir[0] * 0.5, dir[2] * 0.5];
            height += self.layers(&self.large, uv2, top, m, blend);
            height += self.layers(&self.medium, uv2, top, m, blend);
        }
        let r = self.radius_m + height;
        let p = [dir[0] * r, dir[1] * r, dir[2] * r];
        let mut cur = height;
        for d in &self.decals {
            let dp = [p[0] - d.pos[0], p[1] - d.pos[1], p[2] - d.pos[2]];
            if dp[0] * dp[0] + dp[1] * dp[1] + dp[2] * dp[2] > d.cull2 {
                continue;
            }
            let l = |row: [f64; 3]| (row[0] * dp[0] + row[1] * dp[1] + row[2] * dp[2]) / d.scale;
            let (lx, ly, lz) = (l(d.inv[0]), l(d.inv[1]), l(d.inv[2]));
            let (du, dv) = (lx + 0.5, lz + 0.5);
            let clip = ly + 0.5 - 1.0 / d.scale.max(0.001);
            if (0.0..=1.0).contains(&du) && (0.0..=1.0).contains(&dv) && (0.0..=1.0).contains(&clip)
            {
                cur = self.draw_decal(d, du, dv, cur);
            }
        }
        cur
    }
}

impl TerrainMap {
    /// The biome mask's four weights at a latitude/longitude (bilinear, as the original samples it), or `None`
    /// without a pack mask. Not normalised.
    pub fn biome_weights(&self, lat_deg: f64, lon_deg: f64) -> Option<[f64; 4]> {
        let d = self.detail.as_ref()?;
        d.mask.as_ref()?;
        let u = (lon_deg / 360.0 + 0.5).rem_euclid(1.0);
        let v = (lat_deg / 180.0 + 0.5).clamp(0.0, 1.0);
        Some(d.mask_at(u, v))
    }

    /// A map with the pack's finer global map, tiled layers and decals; `None` if the blob is short.
    pub fn from_pack(pack: &TerrainPack, blob: &[u8]) -> Option<TerrainMap> {
        let entry = BodyTerrain {
            id: pack.id.clone(),
            has_ocean: pack.has_ocean,
            sites: pack.sites.clone(),
            map: Some(pack.global.clone()),
        };
        let mut map = TerrainMap::from_entry(&entry, blob, pack.radius_m)?;
        map.detail = Some(Box::new(Detail::read(pack, blob)));
        // Highest point: scan the whole function coarsely and keep a margin for what falls between samples.
        let mut hi = f64::MIN;
        for j in 0..=180 {
            for i in 0..720 {
                let lat = -90.0 + j as f64;
                let lon = -180.0 + i as f64 * 0.5;
                hi = hi.max(map.map_height_m(lat, lon));
            }
        }
        let flat_max = flats_for(&pack.sites)
            .iter()
            .map(|f| f.altitude_m)
            .fold(f64::MIN, f64::max);
        map.max_m = (hi + 250.0).max(flat_max);
        Some(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn pack(decals: Vec<DecalRef>) -> (TerrainPack, Vec<u8>) {
        // global map flat at 0; one decal slice 8 x 8 at full height
        let mut blob = u16s(&[0u16; 16 * 8]);
        let doff = blob.len();
        blob.extend(u16s(&[65535u16; 64]));
        let p = TerrainPack {
            version: 1,
            id: "T".into(),
            radius_m: 100_000.0,
            has_ocean: false,
            sites: vec![],
            global: MapRef {
                offset: 0,
                width: 16,
                height: 8,
                scale_m: 1000.0,
                offset_m: 0.0,
                source: String::new(),
            },
            mask: None,
            contrast: 5.0,
            large: vec![],
            medium: vec![],
            decal_heights: Some(GridRef {
                offset: doff,
                width: 8,
                height: 8,
                count: 1,
            }),
            decal_alpha: None,
            decals,
        };
        (p, blob)
    }

    fn decal(blend: i32) -> DecalRef {
        DecalRef {
            lat_deg: 10.0,
            lon_deg: 20.0,
            rotation_deg: 30.0,
            scale_m: 10_000.0,
            index: 0,
            height_scale: 200.0,
            height_offset: 0.0,
            blend,
            fade_strength: 0.0,
            vertical_offset: 50.0,
            use_alpha: false,
            sort_order: 0,
        }
    }

    #[test]
    fn decal_raises_ground_at_its_centre_only() {
        let (p, blob) = pack(vec![decal(3)]);
        let m = TerrainMap::from_pack(&p, &blob).unwrap();
        // centre: (1 - 0.5) * 1 * 200 + 50 with full fade
        let h = m.height_m(10.0, 20.0);
        assert!((h - 150.0).abs() < 1.0, "{h}");
        // 10 km away, outside the 10 km square
        let far = m.height_m(
            10.0,
            20.0 + 10_000.0 / (100_000.0 * 10f64.to_radians().cos()) * 57.2958,
        );
        assert!(far.abs() < 1e-6, "{far}");
        assert!(m.max_m >= 150.0);
    }

    #[test]
    fn replace_mode_overrides_the_map() {
        let (mut p, mut blob) = pack(vec![decal(2)]);
        // raise the global map to 400 m; a replace decal sets its own height in the middle
        for b in blob.iter_mut().take(16 * 8 * 2) {
            *b = 0xff;
        }
        p.global.scale_m = 400.0;
        let m = TerrainMap::from_pack(&p, &blob).unwrap();
        let h = m.height_m(10.0, 20.0);
        assert!((h - 150.0).abs() < 1.0, "{h}");
    }

    #[test]
    fn decal_rotation_puts_local_up_on_the_normal() {
        let n = unity_dir(-0.63, -75.5);
        let r = decal_rotation(n, 27.0);
        // the local +Y column is the normal
        for k in 0..3 {
            assert!((r[k][1] - n[k]).abs() < 1e-12);
        }
    }
}

#[cfg(test)]
mod real_data {
    use super::*;

    /// With the extracted Kerbin pack present (assets/web/terrain, not in git): the pads sit at their sites'
    /// altitudes, the ground under the KSC campus is near sea level (the campus model carries its own ground,
    /// as in the original) and the highest point is above the global map's
    /// 1.5 km (the body data's `MaxTerrainHeight`): the tiled layers and mountain decals add the rest.
    #[test]
    fn kerbin_pack_heights() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/terrain");
        let (Ok(text), Ok(blob)) = (
            std::fs::read_to_string(dir.join("Kerbin.json")),
            std::fs::read(dir.join("Kerbin.bin")),
        ) else {
            eprintln!("no Kerbin terrain pack, skipped");
            return;
        };
        let pack: TerrainPack = serde_json::from_str(&text).unwrap();
        let m = TerrainMap::from_pack(&pack, &blob).unwrap();
        for s in &pack.sites {
            let h = m.height_m(s.lat_deg, s.lon_deg);
            let raw = m.map_height_m(s.lat_deg, s.lon_deg);
            eprintln!(
                "{:32} site {:7.1}  terrain {:7.1}  without flats {:7.1}",
                s.name, s.altitude_m, h, raw
            );
            if s.name.to_ascii_lowercase().contains("launchpad") {
                assert!((h - s.altitude_m).abs() < 1.0);
            }
        }
        let ksc = pack
            .sites
            .iter()
            .find(|s| s.name == "kerbin_KSC_Object")
            .unwrap();
        for (dlat, dlon) in [
            (0.0, 0.0),
            (0.02, 0.0),
            (-0.02, 0.0),
            (0.0, 0.03),
            (0.0, -0.03),
        ] {
            let h = m.map_height_m(ksc.lat_deg + dlat, ksc.lon_deg + dlon);
            eprintln!("around KSC {dlat:+.2} {dlon:+.2}: {h:.1}");
            assert!(h.abs() < 400.0, "{h}");
        }
        eprintln!("max {:.0} m", m.max_m);
        assert!(m.max_m > 1_500.0 && m.max_m < 12_000.0);
    }
}


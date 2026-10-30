//! Terrain height from each body's global height map, plus flattened sites (launch pads, KSC).
//!
//! The maps come from the install via `tools/extract_terrain.py` into `assets/own/terrain/` (an index and one
//! concatenated binary). Each map is equirectangular, `u = lon / 360 + 0.5`, `v = lat / 180 + 0.5`, row 0 at
//! the south pole, column 0 at longitude -180, raw little-endian u16. Height above the body's nominal radius is
//! `value / 65535 * scale_m + offset_m`, matching the original's `GetSurfaceHeight` restricted to the global
//! map (see `docs/reference-notes/terrain.md`). Sampling is biquadratic over a 3x3 neighbourhood like the
//! original's `BiquadraticSample`, wrapping in longitude and clamping in latitude.
//!
//! A `TerrainPack` (`tools/extract_terrain.py --packs`, one per body, served from the container's assets and
//! never committed) adds the rest of the original's `PQSJobUtil.HeightSample`: a finer global map, the tiled
//! large and medium height layers weighted by the biome mask, and the PQS height decals (the KSC plateau,
//! mountain groups). `TerrainMap::from_pack` builds a map that samples all of it.
//!
//! On Kerbin the KSC campus model brings its own ground (`tools/export_campus_ground.py`): a plateau whose skirts
//! slope down well below sea level, drawn together with the terrain so that the higher of the two shows, as in
//! the original. `height_m` takes the higher of terrain and that overlay, so craft stand on what is drawn;
//! `terrain_m` is the terrain alone, for drawing it under the campus model.

use serde::{Deserialize, Serialize};

pub use crate::terrain_detail::{Detail, TerrainPack};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainIndex {
    pub version: u32,
    pub bodies: Vec<BodyTerrain>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyTerrain {
    pub id: String,
    pub has_ocean: bool,
    pub sites: Vec<Site>,
    pub map: Option<MapRef>,
}

/// A named surface location (launch pad, base anchor) at its authored altitude above the nominal radius.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Site {
    pub name: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub altitude_m: f64,
}

/// Where one map sits in the concatenated binary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapRef {
    pub offset: usize,
    pub width: usize,
    pub height: usize,
    pub scale_m: f64,
    pub offset_m: f64,
    #[serde(default)]
    pub source: String,
}

/// A disc of terrain held at one altitude, blended back into the map over `falloff_m`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flat {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub radius_m: f64,
    pub falloff_m: f64,
    pub altitude_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainMap {
    pub width: usize,
    pub height: usize,
    pub scale_m: f64,
    pub offset_m: f64,
    pub has_ocean: bool,
    /// Body radius the flats measure distance with, m.
    pub body_radius_m: f64,
    pub data: Vec<u16>,
    pub flats: Vec<Flat>,
    /// Highest point of the map (flats included), m above the nominal radius.
    pub max_m: f64,
    /// Tiled layers and decals from a terrain pack; `None` for the embedded global-only maps.
    pub detail: Option<Box<Detail>>,
    /// Ground a site's own model brings (the KSC campus on Kerbin); the higher of it and the terrain counts.
    pub overlay: Option<std::sync::Arc<Overlay>>,
}

/// A height field on a latitude/longitude rectangle (m above sea level, `None` where it has no ground).
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub lat_deg: [f64; 2],
    pub lon_deg: [f64; 2],
    pub width: usize,
    pub height: usize,
    /// decimetres, row 0 north; `i16::MIN` where empty
    pub data: Vec<i16>,
    /// `data` max-filtered over squares of 2^(k+1) - 1 cells either side, k = 0, 1, ... (`sample_max`)
    pub max_levels: Vec<Vec<i16>>,
}

/// Levels of `Overlay::max_levels` (the last reaches 63 cells, about 5 km on the KSC campus's).
const MAX_LEVELS: usize = 6;

#[derive(Deserialize)]
struct OverlayMeta {
    body: String,
    lat_deg: [f64; 2],
    lon_deg: [f64; 2],
    width: usize,
    height: usize,
}

impl Overlay {
    /// The KSC campus ground for `body`, embedded from `assets/own/terrain/ksc_ground.*`.
    pub fn for_body(body: &str) -> Option<Overlay> {
        let meta: OverlayMeta =
            serde_json::from_str(include_str!("../../../assets/own/terrain/ksc_ground.json"))
                .ok()?;
        if meta.body != body {
            return None;
        }
        let bytes = include_bytes!("../../../assets/own/terrain/ksc_ground.bin");
        let data: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        (data.len() == meta.width * meta.height).then(|| {
            let mut o = Overlay {
                lat_deg: meta.lat_deg,
                lon_deg: meta.lon_deg,
                width: meta.width,
                height: meta.height,
                data,
                max_levels: Vec::new(),
            };
            o.build_max_levels();
            o
        })
    }

    fn build_max_levels(&mut self) {
        let (w, h) = (self.width, self.height);
        let mut cur = self.data.clone();
        self.max_levels.clear();
        for k in 0..MAX_LEVELS {
            // 2^k cells either side, separably, on the previous level: the radii add up to 2^(k+1) - 1
            let r = 1usize << k;
            let mut tmp = vec![i16::MIN; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
                    tmp[y * w + x] = cur[y * w + a..=y * w + b].iter().copied().max().unwrap();
                }
            }
            let mut next = vec![i16::MIN; w * h];
            for y in 0..h {
                let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
                for x in 0..w {
                    next[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).max().unwrap();
                }
            }
            self.max_levels.push(next.clone());
            cur = next;
        }
    }

    /// The highest ground within about `reach_m` (bilinear over a max-filtered copy, so a coarse mesh through
    /// samples of it stays above the ground between them); `None` where there is none near.
    pub fn sample_max(
        &self,
        lat_deg: f64,
        lon_deg: f64,
        reach_m: f64,
        body_radius_m: f64,
    ) -> Option<f64> {
        let cell_m =
            (self.lat_deg[1] - self.lat_deg[0]).to_radians() * body_radius_m / self.height as f64;
        let cells = reach_m / cell_m.max(1e-6);
        if cells < 0.5 || self.max_levels.is_empty() {
            return self.sample(lat_deg, lon_deg);
        }
        // level k reaches 2^(k+1) - 1 cells
        let k = (0..self.max_levels.len())
            .find(|&k| ((1usize << (k + 1)) - 1) as f64 >= cells)
            .unwrap_or(self.max_levels.len() - 1);
        let reach = ((1usize << (k + 1)) - 1) as f64;
        let dlat = (self.lat_deg[1] - self.lat_deg[0]) / self.height as f64 * reach;
        let dlon = (self.lon_deg[1] - self.lon_deg[0]) / self.width as f64 * reach;
        let [la0, la1] = self.lat_deg;
        let [lo0, lo1] = self.lon_deg;
        if lat_deg < la0 - dlat
            || lat_deg > la1 + dlat
            || lon_deg < lo0 - dlon
            || lon_deg > lo1 + dlon
        {
            return None;
        }
        Self::bilinear(
            &self.max_levels[k],
            self.width,
            self.height,
            (lon_deg - lo0) / (lo1 - lo0) * self.width as f64 - 0.5,
            (la1 - lat_deg) / (la1 - la0) * self.height as f64 - 0.5,
        )
    }

    fn bilinear(data: &[i16], w: usize, h: usize, fx: f64, fy: f64) -> Option<f64> {
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (mut sum, mut wsum) = (0.0, 0.0);
        for (dx, dy, wt) in [
            (0, 0, (1.0 - tx) * (1.0 - ty)),
            (1, 0, tx * (1.0 - ty)),
            (0, 1, (1.0 - tx) * ty),
            (1, 1, tx * ty),
        ] {
            let x = (x0 as i64 + dx).clamp(0, w as i64 - 1) as usize;
            let y = (y0 as i64 + dy).clamp(0, h as i64 - 1) as usize;
            let v = data[y * w + x];
            if v != i16::MIN && wt > 0.0 {
                sum += wt * v as f64 * 0.1;
                wsum += wt;
            }
        }
        (wsum > 1e-9).then(|| sum / wsum)
    }

    /// Bilinear over the cells that have ground; `None` outside or where none of the four has any.
    pub fn sample(&self, lat_deg: f64, lon_deg: f64) -> Option<f64> {
        let [la0, la1] = self.lat_deg;
        let [lo0, lo1] = self.lon_deg;
        if lat_deg < la0 || lat_deg > la1 || lon_deg < lo0 || lon_deg > lo1 {
            return None;
        }
        Self::bilinear(
            &self.data,
            self.width,
            self.height,
            (lon_deg - lo0) / (lo1 - lo0) * self.width as f64 - 0.5,
            (la1 - lat_deg) / (la1 - la0) * self.height as f64 - 0.5,
        )
    }

    fn max_m(&self) -> f64 {
        self.data
            .iter()
            .filter(|&&v| v != i16::MIN)
            .map(|&v| v as f64 * 0.1)
            .fold(f64::MIN, f64::max)
    }
}

/// Flatten a disc around each launch pad, held at the pad's altitude. Other sites (base anchors such as
/// `kerbin_KSC_Object` at 221.7 m) are reference points, not ground: a disc there stood 100 m above the KSC
/// campus, so they get none. The ground around them comes from the map and, with a pack, the decals.
pub(crate) fn flats_for(sites: &[Site]) -> Vec<Flat> {
    sites
        .iter()
        .filter(|s| s.name.to_ascii_lowercase().contains("launchpad"))
        .map(|s| Flat {
            lat_deg: s.lat_deg,
            lon_deg: s.lon_deg,
            radius_m: 250.0,
            falloff_m: 800.0,
            altitude_m: s.altitude_m,
        })
        .collect()
}

impl TerrainMap {
    /// Build from an index entry and the binary blob; `None` for bodies without a map or with scale 0.
    pub fn from_entry(entry: &BodyTerrain, blob: &[u8], body_radius_m: f64) -> Option<TerrainMap> {
        let m = entry.map.as_ref()?;
        if m.scale_m <= 0.0 || m.width == 0 || m.height == 0 {
            return None;
        }
        let n = m.width * m.height;
        let bytes = blob.get(m.offset..m.offset + n * 2)?;
        let data: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let flats = flats_for(&entry.sites);
        let raw_max =
            data.iter().copied().max().unwrap_or(0) as f64 / 65535.0 * m.scale_m + m.offset_m;
        let flat_max = flats.iter().map(|f| f.altitude_m).fold(f64::MIN, f64::max);
        let overlay = Overlay::for_body(&entry.id).map(std::sync::Arc::new);
        let overlay_max = overlay.as_ref().map(|o| o.max_m()).unwrap_or(f64::MIN);
        Some(TerrainMap {
            width: m.width,
            height: m.height,
            scale_m: m.scale_m,
            offset_m: m.offset_m,
            has_ocean: entry.has_ocean,
            body_radius_m,
            data,
            flats,
            max_m: raw_max.max(flat_max).max(overlay_max),
            detail: None,
            overlay,
        })
    }

    fn texel(&self, x: i64, y: i64) -> f64 {
        let w = self.width as i64;
        let h = self.height as i64;
        let xi = x.rem_euclid(w) as usize;
        let yi = y.clamp(0, h - 1) as usize;
        self.data[yi * self.width + xi] as f64 / 65535.0
    }

    /// Height above the nominal radius at a body-fixed latitude/longitude without the pad flats, m: the global
    /// map, plus the tiled layers and decals when the map came from a pack.
    pub fn map_height_m(&self, lat_deg: f64, lon_deg: f64) -> f64 {
        match &self.detail {
            Some(d) => d.sample(self, lat_deg, lon_deg) + self.offset_m,
            None => self.global_m(lat_deg, lon_deg),
        }
    }

    /// The global map alone, m above the nominal radius.
    pub fn global_m(&self, lat_deg: f64, lon_deg: f64) -> f64 {
        let u = (lon_deg / 360.0 + 0.5).rem_euclid(1.0);
        let v = (lat_deg / 180.0 + 0.5).clamp(0.0, 1.0);
        let fx = u * self.width as f64 - 0.5;
        let fy = v * self.height as f64 - 0.5;
        let x0 = fx.floor() as i64;
        let y0 = fy.floor() as i64;
        let tx = fx - x0 as f64;
        let ty = fy - y0 as f64;
        // the original's Quadratic(a, b, c, f): lerp(lerp(a, b, (f + 1) / 2), lerp(b, c, f / 2), f)
        let quad = |a: f64, b: f64, c: f64, f: f64| {
            let p = a + (b - a) * ((f + 1.0) * 0.5);
            let q = b + (c - b) * (f * 0.5);
            p + (q - p) * f
        };
        let row = |y: i64| {
            quad(
                self.texel(x0 - 1, y),
                self.texel(x0, y),
                self.texel(x0 + 1, y),
                tx,
            )
        };
        let s = quad(row(y0 - 1), row(y0), row(y0 + 1), ty);
        s * self.scale_m + self.offset_m
    }

    fn distance_m(&self, lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
        let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
        let dlat = p2 - p1;
        let dlon = (lon2 - lon1).to_radians();
        let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
        2.0 * self.body_radius_m * a.sqrt().min(1.0).asin()
    }

    /// Ground height, m above the nominal radius: the terrain with its flattened sites, or a site model's own
    /// ground (the KSC campus) where that is higher.
    pub fn height_m(&self, lat_deg: f64, lon_deg: f64) -> f64 {
        let h = self.terrain_m(lat_deg, lon_deg);
        match self
            .overlay
            .as_ref()
            .and_then(|o| o.sample(lat_deg, lon_deg))
        {
            Some(o) => h.max(o),
            None => h,
        }
    }

    /// The terrain alone with its flattened sites (what the planet renderer draws), m above the nominal radius.
    pub fn terrain_m(&self, lat_deg: f64, lon_deg: f64) -> f64 {
        let mut h = self.map_height_m(lat_deg, lon_deg);
        for f in &self.flats {
            let d = self.distance_m(lat_deg, lon_deg, f.lat_deg, f.lon_deg);
            let w = if d <= f.radius_m {
                1.0
            } else if d >= f.radius_m + f.falloff_m {
                0.0
            } else {
                let t = (d - f.radius_m) / f.falloff_m;
                1.0 - t * t * (3.0 - 2.0 * t)
            };
            h += (f.altitude_m - h) * w;
        }
        h
    }

    /// What a vessel stands on: the terrain, or the sea surface (altitude 0) where the terrain is below it.
    pub fn ground_m(&self, lat_deg: f64, lon_deg: f64) -> f64 {
        let h = self.height_m(lat_deg, lon_deg);
        if self.has_ocean {
            h.max(0.0)
        } else {
            h
        }
    }

    pub fn site<'a>(entry: &'a BodyTerrain, name: &str) -> Option<&'a Site> {
        entry.sites.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(
        w: usize,
        h: usize,
        f: impl Fn(usize, usize) -> u16,
        sites: Vec<Site>,
        ocean: bool,
    ) -> TerrainMap {
        let mut blob = Vec::new();
        for y in 0..h {
            for x in 0..w {
                blob.extend_from_slice(&f(x, y).to_le_bytes());
            }
        }
        let entry = BodyTerrain {
            id: "T".into(),
            has_ocean: ocean,
            sites,
            map: Some(MapRef {
                offset: 0,
                width: w,
                height: h,
                scale_m: 1000.0,
                offset_m: -500.0,
                source: String::new(),
            }),
        };
        TerrainMap::from_entry(&entry, &blob, 100_000.0).unwrap()
    }

    #[test]
    fn constant_map_gives_constant_height_and_ocean_clamps() {
        let m = map(16, 8, |_, _| 0, vec![], true);
        assert!((m.map_height_m(10.0, 20.0) + 500.0).abs() < 1e-9);
        assert_eq!(
            m.ground_m(10.0, 20.0),
            0.0,
            "below sea level the ground is the sea"
        );
        let dry = map(16, 8, |_, _| 0, vec![], false);
        assert!((dry.ground_m(10.0, 20.0) + 500.0).abs() < 1e-9);
    }

    #[test]
    fn longitude_wraps_and_halves_differ() {
        // west half low, east half high
        let m = map(64, 32, |x, _| if x < 32 { 0 } else { 65535 }, vec![], false);
        assert!(m.map_height_m(0.0, -90.0) < -400.0, "west");
        assert!(m.map_height_m(0.0, 90.0) > 400.0, "east");
        let a = m.map_height_m(0.0, 179.9);
        let b = m.map_height_m(0.0, -180.1);
        assert!((a - b).abs() < 1e-6, "wraps at the antimeridian");
        // north row high, south rows low: latitude maps to rows with row 0 at the south
        let ns = map(16, 16, |_, y| if y >= 8 { 65535 } else { 0 }, vec![], false);
        assert!(ns.map_height_m(60.0, 0.0) > 400.0 && ns.map_height_m(-60.0, 0.0) < -400.0);
    }

    #[test]
    fn sites_flatten_to_their_altitude_and_blend_out() {
        let pad = Site {
            name: "x_Launchpad_1".into(),
            lat_deg: 0.0,
            lon_deg: 0.0,
            altitude_m: 120.0,
        };
        let m = map(64, 32, |_, _| 0, vec![pad], false);
        assert!((m.height_m(0.0, 0.0) - 120.0).abs() < 1e-9);
        // 2 km away (beyond 250 + 800 m) the map is back
        let deg = 2_000.0 / (std::f64::consts::PI / 180.0 * 100_000.0);
        let far = m.height_m(0.0, deg);
        assert!((far + 500.0).abs() < 1e-6, "far {far}");
        assert!(m.max_m >= 120.0);
    }

    #[test]
    fn kerbin_stands_on_the_campus_ground_where_it_is_above_the_terrain() {
        let o = Overlay::for_body("Kerbin").expect("embedded KSC ground");
        assert!(Overlay::for_body("Mun").is_none());
        // the plateau between the launch pad and the VAB is the campus height
        let pad = o.sample(-0.66, -74.80).unwrap();
        assert!((pad - 107.0).abs() < 5.0, "{pad}");
        // nothing far from the KSC
        assert!(o.sample(10.0, 10.0).is_none());
        // the ground is the higher of the two
        let entry = BodyTerrain {
            id: "Kerbin".into(),
            has_ocean: true,
            sites: vec![],
            map: Some(MapRef {
                offset: 0,
                width: 4,
                height: 2,
                scale_m: 1.0,
                offset_m: 0.0,
                source: String::new(),
            }),
        };
        let m = TerrainMap::from_entry(&entry, &[0u8; 16], 600_000.0).unwrap();
        assert!(m.terrain_m(-0.66, -74.80) < 1.0);
        assert!((m.height_m(-0.66, -74.80) - pad).abs() < 1e-9);
        assert!(m.max_m >= pad);
    }

    #[test]
    fn the_campus_grounds_highest_point_nearby_bounds_the_ground_between_samples() {
        let o = Overlay::for_body("Kerbin").expect("embedded KSC ground");
        let r: f64 = 600_000.0;
        // a grid of points around the campus edge: the maximum within a reach is at least the ground anywhere
        // within that reach (what keeps a coarse terrain mesh over the campus's skirts)
        let reach: f64 = 1500.0;
        let deg = (reach / r).to_degrees();
        for i in 0..12 {
            for j in 0..12 {
                let lat = -1.30 + i as f64 * 0.15;
                let lon = -76.0 + j as f64 * 0.17;
                let Some(top) = o.sample_max(lat, lon, reach, r) else {
                    continue;
                };
                for (dy, dx) in [(0.7, 0.0), (-0.7, 0.3), (0.2, -0.7), (-0.5, -0.5)] {
                    if let Some(g) = o.sample(lat + dy * deg, lon + dx * deg) {
                        assert!(top >= g - 0.05, "{lat} {lon}: {top} < {g}");
                    }
                }
                assert!(top >= o.sample(lat, lon).unwrap_or(f64::MIN));
            }
        }
        // no reach is the plain sample; far away there is nothing
        assert_eq!(o.sample_max(-0.66, -74.80, 0.0, r), o.sample(-0.66, -74.80));
        assert!(o.sample_max(10.0, 10.0, reach, r).is_none());
    }
}

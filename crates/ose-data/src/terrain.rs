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
        Some(TerrainMap {
            width: m.width,
            height: m.height,
            scale_m: m.scale_m,
            offset_m: m.offset_m,
            has_ocean: entry.has_ocean,
            body_radius_m,
            data,
            flats,
            max_m: raw_max.max(flat_max),
            detail: None,
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

    /// Terrain height including flattened sites, m above the nominal radius.
    pub fn height_m(&self, lat_deg: f64, lon_deg: f64) -> f64 {
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
}

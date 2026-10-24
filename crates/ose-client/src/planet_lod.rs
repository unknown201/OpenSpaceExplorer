//! Planet terrain level of detail, after the original's PQS (docs/reference-notes/planet-rendering.md): a cube
//! sphere whose six faces are quadtrees, subdivided by distance from the camera, and one mesh per quad with its
//! heights from `ose_data::terrain::TerrainMap`, the function the server lands vessels on.
//!
//! Frame: the flight view's body model frame (`flight3d::model_point`): metres, Y towards the north pole,
//! longitude `L` at azimuth `180 + L` measured from +X towards -Z. Everything here is plain f64 and has no GPU
//! dependency, so it is tested natively.

use ose_data::terrain::TerrainMap;

pub type V3 = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn normalize(a: V3) -> V3 {
    let l = length(a);
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        a
    }
}

/// Latitude and longitude (degrees) of a model-frame direction.
pub fn lat_lon(d: V3) -> (f64, f64) {
    let l = length(d).max(1e-300);
    let lat = (d[1] / l).clamp(-1.0, 1.0).asin().to_degrees();
    let az = (-d[2]).atan2(d[0]).to_degrees();
    let mut lon = az - 180.0;
    if lon < -180.0 {
        lon += 360.0;
    }
    (lat, lon)
}

/// Unit model-frame direction of a latitude and longitude (degrees), as `flight3d::model_point`.
pub fn direction(lat_deg: f64, lon_deg: f64) -> V3 {
    let lat = lat_deg.to_radians();
    let az = (180.0 + lon_deg).to_radians();
    [lat.cos() * az.cos(), lat.sin(), -lat.cos() * az.sin()]
}

/// Grid vertices along a quad edge (32 segments).
pub const GRID: usize = 33;
/// Deepest level, as the original's `maxLevel`.
pub const MAX_LEVEL: u8 = 12;
/// Shallowest level always drawn. The original keeps `minLevel 3` (384 quads); level 2 is enough here because
/// the height map has 7 km texels.
pub const MIN_LEVEL: u8 = 2;
/// The original subdivides below `radius * minDetailMultiplier (10) / 2^level`; we use 4 to keep the number of
/// quads a browser builds per second reasonable (about 1.6 quad edges instead of 6).
pub const DETAIL_MULTIPLIER: f64 = 2.5;

/// Cube face axes: (normal, s axis, t axis), right-handed so `s x t = normal` and triangles face outwards.
const FACES: [(V3, V3, V3); 6] = [
    ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
    ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
];

/// The cube face a direction falls on and its face coordinates `a`, `b` in [-1, 1] (the tangent-warped
/// parameters `QuadKey::dir_at` uses).
pub fn face_ab(d: V3) -> (u8, f64, f64) {
    let mut best = (0u8, f64::MIN);
    for (i, (n, _, _)) in FACES.iter().enumerate() {
        let c = dot(d, *n);
        if c > best.1 {
            best = (i as u8, c);
        }
    }
    let (n, sa, ta) = FACES[best.0 as usize];
    let dn = dot(d, n).max(1e-12);
    let a = (dot(d, sa) / dn).atan() / std::f64::consts::FRAC_PI_4;
    let b = (dot(d, ta) / dn).atan() / std::f64::consts::FRAC_PI_4;
    (best.0, a, b)
}

/// Unit direction of face coordinates (inverse of `face_ab`).
pub fn dir_from_ab(face: u8, a: f64, b: f64) -> V3 {
    let (n, sa, ta) = FACES[face as usize % 6];
    let wa = (a * std::f64::consts::FRAC_PI_4).tan();
    let wb = (b * std::f64::consts::FRAC_PI_4).tan();
    normalize(add(n, add(scale(sa, wa), scale(ta, wb))))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QuadKey {
    pub face: u8,
    pub level: u8,
    pub x: u32,
    pub y: u32,
}

impl QuadKey {
    pub fn roots() -> impl Iterator<Item = QuadKey> {
        (0..6u8).map(|face| QuadKey {
            face,
            level: 0,
            x: 0,
            y: 0,
        })
    }

    pub fn children(&self) -> [QuadKey; 4] {
        let c = |dx, dy| QuadKey {
            face: self.face,
            level: self.level + 1,
            x: self.x * 2 + dx,
            y: self.y * 2 + dy,
        };
        [c(0, 0), c(1, 0), c(0, 1), c(1, 1)]
    }

    /// Unit direction of face coordinates `(s, t)` in [0, 1] across this quad (tangent-warped so quads are
    /// about the same size on the sphere).
    pub fn dir_at(&self, s: f64, t: f64) -> V3 {
        let n = (1u64 << self.level) as f64;
        let a = ((self.x as f64 + s) / n) * 2.0 - 1.0;
        let b = ((self.y as f64 + t) / n) * 2.0 - 1.0;
        let wa = (a * std::f64::consts::FRAC_PI_4).tan();
        let wb = (b * std::f64::consts::FRAC_PI_4).tan();
        let (nrm, sa, ta) = FACES[self.face as usize];
        normalize(add(nrm, add(scale(sa, wa), scale(ta, wb))))
    }

    pub fn center_dir(&self) -> V3 {
        self.dir_at(0.5, 0.5)
    }

    /// Angular half size (radians) of the quad: centre to the farthest corner.
    pub fn angular_radius(&self) -> f64 {
        let c = self.center_dir();
        [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
            .iter()
            .map(|&(s, t)| dot(c, self.dir_at(s, t)).clamp(-1.0, 1.0).acos())
            .fold(0.0, f64::max)
    }
}

/// Quads to draw for a camera at `cam` (model frame, m) over a body of `radius` whose terrain lies between
/// `min_h` and `max_h` m. `ready(key)` says whether a quad's mesh exists; a quad that should split but whose four
/// children are not all ready is drawn itself, and its missing children are returned in `wanted` (coarsest,
/// then nearest first) so the caller builds them. Quads behind the horizon are skipped.
pub fn select(
    cam: V3,
    radius: f64,
    min_h: f64,
    max_h: f64,
    ready: &dyn Fn(&QuadKey) -> bool,
) -> (Vec<QuadKey>, Vec<(f64, QuadKey)>) {
    let mut draw = Vec::new();
    let mut wanted: Vec<(f64, QuadKey)> = Vec::new();
    let cam_r = length(cam).max(1.0);
    let cam_dir = scale(cam, 1.0 / cam_r);
    let low = radius + min_h.min(0.0);
    // Angle from the camera's nadir to its horizon on the lowest sphere, plus the angle that terrain as high as
    // max_h can still be seen beyond it.
    let horizon = if cam_r > low {
        (low / cam_r).acos() + (low / (radius + max_h.max(0.0)).max(low)).acos()
    } else {
        std::f64::consts::PI
    };
    let mut stack: Vec<QuadKey> = QuadKey::roots().collect();
    while let Some(q) = stack.pop() {
        let c = q.center_dir();
        let ang = q.angular_radius();
        let cang = dot(c, cam_dir).clamp(-1.0, 1.0).acos();
        if q.level >= MIN_LEVEL && cang - ang > horizon {
            continue;
        }
        if !ready(&q) {
            wanted.push((distance_to_quad(&q, cam, radius), q));
            continue;
        }
        let split = q.level < MIN_LEVEL
            || (q.level < MAX_LEVEL
                && distance_to_quad(&q, cam, radius)
                    < radius * DETAIL_MULTIPLIER / (1u64 << q.level) as f64);
        if split {
            let kids = q.children();
            let missing: Vec<QuadKey> = kids.iter().copied().filter(|k| !ready(k)).collect();
            if missing.is_empty() || q.level < MIN_LEVEL {
                stack.extend(kids);
                continue;
            }
            for k in missing {
                wanted.push((distance_to_quad(&k, cam, radius), k));
            }
        }
        draw.push(q);
    }
    // Coarse quads first (a missing coarse quad is a hole, a missing fine one only less detail), then nearest.
    wanted.sort_by(|a, b| {
        (a.1.level.max(MIN_LEVEL), a.0)
            .partial_cmp(&(b.1.level.max(MIN_LEVEL), b.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    wanted.dedup_by_key(|w| w.1);
    (draw, wanted)
}

/// The original's `gcDist`: distance along the surface from the camera's ground point to the quad's edge, plus
/// the camera's altitude.
pub fn distance_to_quad(q: &QuadKey, cam: V3, radius: f64) -> f64 {
    let cam_r = length(cam).max(1.0);
    let cang = dot(q.center_dir(), scale(cam, 1.0 / cam_r))
        .clamp(-1.0, 1.0)
        .acos();
    let surface = ((cang - q.angular_radius()).max(0.0)) * radius;
    surface + (cam_r - radius).abs()
}

/// One vertex: position relative to the quad's centre point (m), normal, height above the radius (m), and an
/// extra value (the terrain height under ocean vertices).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerrainVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub height: f32,
    pub extra: f32,
}

pub struct QuadMesh {
    /// Model-frame centre (on the sphere at the radius), m.
    pub center: V3,
    pub terrain: Vec<TerrainVertex>,
    /// Sea-level vertices when some terrain lies below sea level on an ocean world.
    pub ocean: Option<Vec<TerrainVertex>>,
    pub min_h: f64,
    pub max_h: f64,
}

/// Vertices per quad mesh: the grid plus a skirt along the four edges.
pub const VERTS: usize = GRID * GRID + 4 * GRID;

/// Indices of the grid alone (the skirts follow): the cloud shell draws only these.
pub const GRID_INDICES: u32 = ((GRID - 1) * (GRID - 1) * 6) as u32;

/// Triangle indices shared by every quad mesh (counter-clockwise seen from outside).
pub fn indices() -> Vec<u32> {
    let g = GRID as u32;
    let mut out = Vec::new();
    for y in 0..g - 1 {
        for x in 0..g - 1 {
            let i = y * g + x;
            out.extend_from_slice(&[i, i + 1, i + g + 1, i, i + g + 1, i + g]);
        }
    }
    // Skirts: each edge's grid vertices joined to a lowered copy. Edge order: bottom (y = 0), right (x = g - 1),
    // top (y = g - 1), left (x = 0), each walked so the outside stays on the right.
    let base = g * g;
    let edges: [Vec<u32>; 4] = [
        (0..g).collect(),
        (0..g).map(|y| y * g + g - 1).collect(),
        (0..g).rev().map(|x| (g - 1) * g + x).collect(),
        (0..g).rev().map(|y| y * g).collect(),
    ];
    for (e, idx) in edges.iter().enumerate() {
        let sk = base + e as u32 * g;
        for k in 0..(g - 1) as usize {
            let (a, b) = (idx[k], idx[k + 1]);
            let (sa, sb) = (sk + k as u32, sk + k as u32 + 1);
            // Two-sided enough: emit both windings so the skirt hides cracks from either side.
            out.extend_from_slice(&[a, sa, sb, a, sb, b, a, sb, sa, a, b, sb]);
        }
    }
    out
}

fn skirt_order(e: usize, k: usize) -> (usize, usize) {
    let g = GRID;
    match e {
        0 => (k, 0),
        1 => (g - 1, k),
        2 => (g - 1 - k, g - 1),
        _ => (0, g - 1 - k),
    }
}

/// Build a quad's mesh. `height(dir)` is the terrain height above `radius` for a unit model-frame direction.
pub fn build_quad(q: &QuadKey, radius: f64, ocean: bool, height: &dyn Fn(V3) -> f64) -> QuadMesh {
    let g = GRID;
    let n = g + 2; // one ring of extra samples for normals across edges
    let step = 1.0 / (g - 1) as f64;
    let mut pts = vec![[0.0; 3]; n * n];
    let mut hs = vec![0.0; n * n];
    let mut dirs = vec![[0.0; 3]; n * n];
    for j in 0..n {
        for i in 0..n {
            let s = (i as f64 - 1.0) * step;
            let t = (j as f64 - 1.0) * step;
            let d = q.dir_at(s, t);
            let h = height(d);
            dirs[j * n + i] = d;
            hs[j * n + i] = h;
            pts[j * n + i] = scale(d, radius + h);
        }
    }
    let center = scale(q.center_dir(), radius);
    let mut terrain = Vec::with_capacity(VERTS);
    let (mut min_h, mut max_h) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut normals = vec![[0.0f32; 3]; g * g];
    for y in 0..g {
        for x in 0..g {
            let (i, j) = (x + 1, y + 1);
            let p = pts[j * n + i];
            let ds = sub(pts[j * n + i + 1], pts[j * n + i - 1]);
            let dt = sub(pts[(j + 1) * n + i], pts[(j - 1) * n + i]);
            let mut nr = normalize(cross(ds, dt));
            if dot(nr, dirs[j * n + i]) < 0.0 {
                nr = scale(nr, -1.0);
            }
            let h = hs[j * n + i];
            min_h = min_h.min(h);
            max_h = max_h.max(h);
            let nf = [nr[0] as f32, nr[1] as f32, nr[2] as f32];
            normals[y * g + x] = nf;
            let rel = sub(p, center);
            terrain.push(TerrainVertex {
                pos: [rel[0] as f32, rel[1] as f32, rel[2] as f32],
                normal: nf,
                height: h as f32,
                extra: 0.0,
            });
        }
    }
    // Skirt depth: a fraction of the quad's size, enough to cover the step between neighbouring levels.
    let size = q.angular_radius() * radius;
    let skirt = (size * 0.06).max(20.0);
    for e in 0..4 {
        for k in 0..g {
            let (x, y) = skirt_order(e, k);
            let v = terrain[y * g + x];
            let d = dirs[(y + 1) * n + x + 1];
            let p = scale(d, radius + v.height as f64 - skirt);
            let rel = sub(p, center);
            terrain.push(TerrainVertex {
                pos: [rel[0] as f32, rel[1] as f32, rel[2] as f32],
                normal: v.normal,
                height: v.height - skirt as f32,
                extra: 0.0,
            });
        }
    }
    let ocean_mesh = (ocean && min_h < 0.0).then(|| {
        let mut out = Vec::with_capacity(VERTS);
        for y in 0..g {
            for x in 0..g {
                let d = dirs[(y + 1) * n + x + 1];
                let rel = sub(scale(d, radius), center);
                out.push(TerrainVertex {
                    pos: [rel[0] as f32, rel[1] as f32, rel[2] as f32],
                    normal: [d[0] as f32, d[1] as f32, d[2] as f32],
                    height: 0.0,
                    extra: hs[(y + 1) * n + x + 1] as f32,
                });
            }
        }
        for e in 0..4 {
            for k in 0..g {
                let (x, y) = skirt_order(e, k);
                let d = dirs[(y + 1) * n + x + 1];
                let rel = sub(scale(d, radius - skirt * 0.5), center);
                out.push(TerrainVertex {
                    pos: [rel[0] as f32, rel[1] as f32, rel[2] as f32],
                    normal: [d[0] as f32, d[1] as f32, d[2] as f32],
                    height: -skirt as f32 * 0.5,
                    extra: hs[(y + 1) * n + x + 1] as f32,
                });
            }
        }
        out
    });
    QuadMesh {
        center,
        terrain,
        ocean: ocean_mesh,
        min_h,
        max_h,
    }
}

/// Terrain height for a direction from a `TerrainMap` (no map: the sphere).
pub fn map_height(map: Option<&TerrainMap>, d: V3) -> f64 {
    match map {
        Some(m) => {
            let (lat, lon) = lat_lon(d);
            m.height_m(lat, lon)
        }
        None => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lat_lon_round_trip() {
        for &(lat, lon) in &[
            (0.0, 0.0),
            (-0.6836, -74.8198),
            (45.0, 170.0),
            (-60.0, -179.0),
        ] {
            let (a, b) = lat_lon(direction(lat, lon));
            assert!(
                (a - lat).abs() < 1e-9 && (b - lon).abs() < 1e-9,
                "{lat} {lon} -> {a} {b}"
            );
        }
    }

    #[test]
    fn face_coordinates_round_trip() {
        for &(lat, lon) in &[(0.0, 0.0), (44.0, 91.0), (-80.0, -170.0), (10.0, -74.8)] {
            let d = direction(lat, lon);
            let (f, a, b) = face_ab(d);
            assert!(a.abs() <= 1.0 + 1e-9 && b.abs() <= 1.0 + 1e-9);
            assert!(length(sub(dir_from_ab(f, a, b), d)) < 1e-12);
        }
    }

    #[test]
    fn faces_cover_the_sphere_and_face_outwards() {
        for k in QuadKey::roots() {
            let c = k.center_dir();
            let a = k.dir_at(0.0, 0.0);
            let b = k.dir_at(1.0, 0.0);
            let d = k.dir_at(0.0, 1.0);
            // s x t points outwards, so the grid's (i, i+1, i+g+1) triangles wind counter-clockwise from outside
            assert!(dot(cross(sub(b, a), sub(d, a)), c) > 0.0, "face {}", k.face);
        }
    }

    #[test]
    fn children_tile_their_parent() {
        let q = QuadKey {
            face: 2,
            level: 3,
            x: 5,
            y: 1,
        };
        let kids = q.children();
        let p = q.dir_at(0.5, 0.5);
        // the parent's centre is the shared corner of its four children
        assert!(length(sub(kids[0].dir_at(1.0, 1.0), p)) < 1e-12);
        assert!(length(sub(kids[3].dir_at(0.0, 0.0), p)) < 1e-12);
    }

    #[test]
    fn selection_refines_near_the_camera() {
        let r = 600_000.0;
        let cam = scale(direction(-0.68, -74.8), r + 100.0);
        let ready = |_: &QuadKey| true;
        let (draw, wanted) = select(cam, r, -1000.0, 6000.0, &ready);
        assert!(wanted.is_empty());
        let deepest = draw.iter().map(|q| q.level).max().unwrap();
        assert_eq!(deepest, MAX_LEVEL);
        let mut per = [0usize; 13];
        for q in &draw {
            per[q.level as usize] += 1;
        }
        assert!(draw.len() < 500, "{} quads {per:?}", draw.len());
        // from far away only the coarse levels remain, and only the side facing the camera
        let far = scale(direction(0.0, 0.0), r * 10.0);
        let (draw_far, _) = select(far, r, -1000.0, 6000.0, &ready);
        assert!(draw_far.iter().all(|q| q.level <= 3));
        assert!(draw_far.len() < 96);
    }

    #[test]
    fn missing_children_keep_the_parent() {
        let r = 600_000.0;
        let cam = scale(direction(10.0, 20.0), r + 1000.0);
        let ready = |q: &QuadKey| q.level <= MIN_LEVEL;
        let (draw, wanted) = select(cam, r, 0.0, 0.0, &ready);
        assert!(draw.iter().all(|q| q.level == MIN_LEVEL));
        assert!(!wanted.is_empty());
        assert!(wanted.iter().all(|w| w.1.level == MIN_LEVEL + 1));
    }

    #[test]
    fn quad_mesh_follows_the_height_function() {
        let q = QuadKey {
            face: 0,
            level: 4,
            x: 7,
            y: 9,
        };
        let r = 600_000.0;
        let m = build_quad(&q, r, true, &|d| d[1] * 5000.0 - 100.0);
        assert_eq!(m.terrain.len(), VERTS);
        for v in &m.terrain[..GRID * GRID] {
            let p = add(
                m.center,
                [v.pos[0] as f64, v.pos[1] as f64, v.pos[2] as f64],
            );
            let d = normalize(p);
            let h = d[1] * 5000.0 - 100.0;
            assert!((length(p) - r - h).abs() < 0.5);
            assert!((v.height as f64 - h).abs() < 0.01);
        }
        assert_eq!(m.ocean.as_ref().map(|o| o.len()), Some(VERTS));
        let idx = indices();
        assert!(idx.iter().all(|&i| (i as usize) < VERTS));
    }
}

//! How far the drawn ground (the LOD mesh, linear between grid vertices) is from the server's `height_m`, at
//! the quad levels a landed or low camera sees. Uses the real terrain packs (assets/web/terrain, not in git).
//! Usage: cargo run --release -p ose-client --example terrain_fidelity [Body] [radius_m]
use ose_client::planet_lod::{dir_from_ab, face_ab, lat_lon, GRID};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let body = args.get(1).map(String::as_str).unwrap_or("Kerbin");
    let r: f64 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600_000.0);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/terrain");
    let text = std::fs::read_to_string(dir.join(format!("{body}.json"))).expect("pack");
    let blob = std::fs::read(dir.join(format!("{body}.bin"))).expect("blob");
    let mut pack: ose_data::terrain::TerrainPack = serde_json::from_str(&text).unwrap();
    pack.radius_m = r;
    let map = ose_data::terrain::TerrainMap::from_pack(&pack, &blob).unwrap();
    let h = |d: [f64; 3]| {
        let (lat, lon) = lat_lon(d);
        map.height_m(lat, lon)
    };
    let mut seed = 12345u64;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for level in [12u8, 11, 10, 8] {
        let n = 1u64 << level;
        let cells = (n as f64) * (GRID - 1) as f64;
        let mut errs = Vec::new();
        for _ in 0..20000 {
            let d = dir_from_ab((rnd() * 6.0) as u8, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0);
            let (face, a, b) = face_ab(d);
            // grid coordinates across the face at this level's vertex spacing
            let gx = (a + 1.0) * 0.5 * cells;
            let gy = (b + 1.0) * 0.5 * cells;
            let (x0, y0) = (gx.floor(), gy.floor());
            let (fx, fy) = (gx - x0, gy - y0);
            let at = |x: f64, y: f64| {
                h(dir_from_ab(
                    face,
                    x / cells * 2.0 - 1.0,
                    y / cells * 2.0 - 1.0,
                ))
            };
            let (h00, h11) = (at(x0, y0), at(x0 + 1.0, y0 + 1.0));
            // the index order splits cells from (x, y) to (x + 1, y + 1)
            let mesh = if fx >= fy {
                let h10 = at(x0 + 1.0, y0);
                h00 + (h10 - h00) * fx + (h11 - h10) * fy
            } else {
                let h01 = at(x0, y0 + 1.0);
                h00 + (h11 - h01) * fx + (h01 - h00) * fy
            };
            errs.push((mesh - h(d)).abs());
        }
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f64| errs[((errs.len() - 1) as f64 * q) as usize];
        let spacing = std::f64::consts::FRAC_PI_2 * r / cells;
        println!(
            "{body} level {level:2} ({spacing:6.1} m vertices): median {:.2} m, p95 {:.2} m, p99 {:.2} m, max {:.2} m",
            p(0.5), p(0.95), p(0.99), p(1.0)
        );
    }
}

//! Where the terrain pack's launch pad flats are, against the campus model's pads. Samples the pack's
//! height on a grid around the KSC and prints, for pad 1's install site and for where pads 2-4 (which agree
//! with the campus layout) put the campus's pad 1, the terrain height there and the spread around it.
//! Needs assets/web/terrain (not in git). Usage: cargo run --release -p ose-client --example ksc_flats
fn main() {
    let r = 600_000.0;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web");
    let text = std::fs::read_to_string(root.join("terrain/Kerbin.json")).expect("pack");
    let blob = std::fs::read(root.join("terrain/Kerbin.bin")).expect("blob");
    let mut pack: ose_data::terrain::TerrainPack = serde_json::from_str(&text).unwrap();
    pack.radius_m = r;
    let map = ose_data::terrain::TerrainMap::from_pack(&pack, &blob).unwrap();
    let m_per_deg = r * std::f64::consts::PI / 180.0;
    let flat = |lat: f64, lon: f64| {
        // Height at the point and the spread over a 150 m disc: a pad flat reads as near zero spread.
        let h0 = map.height_m(lat, lon);
        let mut spread: f64 = 0.0;
        for k in 0..12 {
            let a = k as f64 * std::f64::consts::TAU / 12.0;
            let (dn, de) = (150.0 * a.sin(), 150.0 * a.cos());
            let h = map.height_m(
                lat + dn / m_per_deg,
                lon + de / (m_per_deg * lat.to_radians().cos()),
            );
            spread = spread.max((h - h0).abs());
        }
        (h0, spread)
    };
    let sites = [
        ("pad1 install site", -0.6836, -74.8198),
        ("pad2 install site", -0.4360, -75.0270),
        ("pad3 install site", -0.6211, -75.1853),
        ("pad4 install site", -0.4362, -75.3480),
    ];
    for (name, lat, lon) in sites {
        let (h, s) = flat(lat, lon);
        println!("{name:28} {lat:.4} {lon:.4}  height {h:7.1} m  spread(150 m) {s:6.2} m");
    }
    // Pad 1 where pads 2-4 put the campus: the campus offset (model -> site) from pads 2-4 is
    // (-430 E, -2036 N) relative to kerbin_KSC_Object, and the model's pad 1 is (10 E, 2104 N) from the model origin.
    let (ksc_lat, ksc_lon) = (-0.6274, -74.8305);
    let (e, n) = (10.0 - 430.0, 2104.0 - 2036.0);
    let lat = ksc_lat + n / m_per_deg;
    let lon = ksc_lon + e / (m_per_deg * ksc_lat.to_radians().cos());
    let (h, s) = flat(lat, lon);
    println!(
        "{:28} {lat:.4} {lon:.4}  height {h:7.1} m  spread(150 m) {s:6.2} m",
        "pad1 from the campus layout"
    );
    // A coarse search for the flattest spots within 2 km of each pad 1 candidate.
    let mut best = Vec::new();
    for i in -40..=40 {
        for j in -40..=40 {
            let la = -0.66 + i as f64 * 50.0 / m_per_deg;
            let lo = -74.80 + j as f64 * 50.0 / (m_per_deg * 0.66f64.to_radians().cos());
            let (h, s) = flat(la, lo);
            if h > 60.0 {
                best.push((s, la, lo, h));
            }
        }
    }
    best.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (s, la, lo, h) in best.iter().take(8) {
        println!("flat near pad 1: {la:.4} {lo:.4} height {h:6.1} spread {s:5.2}");
    }
}

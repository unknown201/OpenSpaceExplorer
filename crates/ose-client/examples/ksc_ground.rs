//! The KSC campus's ground against the server's terrain height: places the campus as flight3d's `ksc_pose`
//! does (pad 1's spawn on its site, yawed to fit the other pads) and compares the campus ground plane and spawn
//! points with `TerrainMap::height_m` under them. Needs assets/web/terrain and assets/web/ksc (not in git).
//! Usage: cargo run --release -p ose-client --example ksc_ground
use glam::{DMat3, DQuat, DVec3};

const PADS: [(f64, f64, f64); 4] = [
    (-0.6836, -74.8198, 115.2),
    (-0.4360, -75.0270, 111.0),
    (-0.6211, -75.1853, 115.8),
    (-0.4362, -75.3480, 132.0),
];

fn model_point(lat_deg: f64, lon_deg: f64, radius: f64) -> DVec3 {
    let lat = lat_deg.to_radians();
    let az = (180.0 + lon_deg).to_radians();
    radius * DVec3::new(lat.cos() * az.cos(), lat.sin(), -lat.cos() * az.sin())
}

fn main() {
    let r = 600_000.0;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web");
    let text = std::fs::read_to_string(root.join("terrain/Kerbin.json")).expect("pack");
    let blob = std::fs::read(root.join("terrain/Kerbin.bin")).expect("blob");
    let mut pack: ose_data::terrain::TerrainPack = serde_json::from_str(&text).unwrap();
    pack.radius_m = r;
    let map = ose_data::terrain::TerrainMap::from_pack(&pack, &blob).unwrap();
    let scene: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("ksc/scene.json")).expect("scene"))
            .unwrap();
    let spawn = |k: &str| {
        let p = &scene["spawns"][k];
        DVec3::new(
            p[0].as_f64().unwrap(),
            p[1].as_f64().unwrap(),
            p[2].as_f64().unwrap(),
        )
    };
    let local: Vec<DVec3> = (1..=4)
        .map(|k| spawn(&format!("LaunchPad_spawn_0{k}")))
        .collect();
    let l1 = local[0];
    let p1 = model_point(PADS[0].0, PADS[0].1, r + PADS[0].2);
    let up = p1.normalize();
    let north = (DVec3::Y - up * up.dot(DVec3::Y)).normalize_or_zero();
    let east = north.cross(up);
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for (k, lk) in local.iter().enumerate().skip(1) {
        let w = model_point(PADS[k].0, PADS[k].1, r + PADS[k].2) - p1;
        let d = *lk - l1;
        let a = w.dot(north).atan2(w.dot(east)) - (-d.z).atan2(d.x);
        s += a.sin();
        c += a.cos();
    }
    let yaw = s.atan2(c);
    let ex = east * yaw.cos() + north * yaw.sin();
    let en = -east * yaw.sin() + north * yaw.cos();
    let rot = DQuat::from_mat3(&DMat3::from_cols(ex, up, -en)).normalize();
    let pos = p1 - rot * l1;
    // (campus point) -> (its height above the radius, the server's terrain height under it)
    let at = |p: DVec3| {
        let w = pos + rot * p;
        let d = w.normalize();
        let lat = d.y.asin().to_degrees();
        let lon = (-d.z).atan2(d.x).to_degrees() - 180.0;
        let lon = (lon + 540.0).rem_euclid(360.0) - 180.0;
        (w.length() - r, map.height_m(lat, lon))
    };
    let ll = |p: DVec3| {
        let d = (pos + rot * p).normalize();
        let lon = ((-d.z).atan2(d.x).to_degrees() - 180.0 + 540.0).rem_euclid(360.0) - 180.0;
        (d.y.asin().to_degrees(), lon)
    };
    for (k, pd) in PADS.iter().enumerate() {
        let w = model_point(pd.0, pd.1, r + pd.2);
        let l = rot.inverse() * (w - pos);
        println!(
            "pad {} site in campus coordinates x {:.0} y {:.0} z {:.0}",
            k + 1,
            l.x,
            l.y,
            l.z
        );
    }
    for (k, l) in local.iter().enumerate() {
        println!("pad {} campus at {:?}, site {:?}", k + 1, ll(*l), PADS[k]);
    }
    for k in [
        "LaunchPad_spawn_01",
        "LaunchPad_spawn_02",
        "LaunchPad_spawn_03",
        "LaunchPad_spawn_04",
        "Runway_spawn_01",
        "Runway_spawn_02",
    ] {
        let (h, g) = at(spawn(k));
        println!(
            "{k:20} campus {h:7.1} m, server ground {g:7.1} m, server - campus {:+6.1} m",
            g - h
        );
    }
    // The campus ground plane over its footprint, every 250 m.
    let gy = scene["ground_y"].as_f64().unwrap();
    println!("ground plane (y {gy}): server - campus, m; rows z, columns x");
    let mut z = -4250.0;
    while z <= 500.0 {
        let mut line = format!("{z:6.0}");
        let mut x = -5500.0;
        while x <= 3500.0 {
            let (h, g) = at(DVec3::new(x, gy, z));
            line += &format!("{:+6.0}", g - h);
            x += 500.0;
        }
        println!("{line}");
        z += 250.0;
    }
}

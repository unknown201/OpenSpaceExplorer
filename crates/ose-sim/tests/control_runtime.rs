//! Numeric evidence from the running reference, with no fitted gain, pose or time offset.
use std::collections::HashMap;

use glam::{DMat3, DQuat};
use ose_math::DVec3;
use ose_sim::{
    flight::{self, Attitude, FlightSpec, Powered},
    structure::Structure,
    Universe,
};

#[test]
fn jumping_flea_live_physics_pulse_matches_the_original_strict_thresholds() {
    let csv =
        include_str!("../../../tools/ksp2-control-capture/fixtures/jumping-flea-interval.csv");
    let mut lines = csv.lines();
    let headers: Vec<_> = lines.next().unwrap().split(',').collect();
    let rows: Vec<HashMap<&str, f64>> = lines
        .map(|line| {
            headers
                .iter()
                .copied()
                .zip(line.split(',').map(|v| v.parse().unwrap()))
                .collect()
        })
        .collect();
    assert_eq!(rows.len(), 600);
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let crafts: Vec<ose_data::craft::Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let parts: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let craft = crafts.iter().find(|c| c.name == "Jumping Flea").unwrap();
    let report = ose_data::staging::delta_v_by_stage(craft, &parts, &resources);
    let initial = &rows[0];
    let q =
        |r: &HashMap<&str, f64>| DQuat::from_xyzw(-r["qx"], -r["qy"], r["qz"], r["qw"]).normalize();
    let vec = |v: glam::DVec3| DVec3::new(v.x, v.y, v.z);
    let g = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
    let position = DVec3::new(initial["px"], initial["py"], -initial["pz"]);
    let velocity = DVec3::new(initial["vx"], initial["vy"], -initial["vz"]);
    let mut u = Universe::with_bodies(&bodies);
    let body = u.body_entity("Kerbin").unwrap();
    let e = u.spawn_vessel("reference pulse", body, position, velocity);
    let mut p = Powered::new(
        FlightSpec::from_report(&report, &[], &[], 1.5),
        position,
        velocity,
        false,
    );
    p.ignited = false;
    p.throttle = initial["throttle"];
    p.attitude = Attitude::Free;
    p.facing = vec(q(initial) * glam::DVec3::Y);
    flight::set_vessel_top(&mut p, vec(q(initial) * glam::DVec3::Z));
    let w = DVec3::new(-initial["wx"], -initial["wy"], initial["wz"]);
    p.roll_rate = w.dot(p.facing);
    p.angvel = w - p.facing * p.roll_rate;
    u.set_powered(e, p);
    u.set_structure(e, Structure::build(craft, &parts, &resources));
    let (mut max_angle, mut max_rate, mut max_mass) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (i, row) in rows.iter().enumerate() {
        assert!((row["dt"] - 0.02).abs() < 1e-6);
        assert!((row["t"] - i as f64 * 0.02).abs() < 1e-6);
        assert_eq!(row["sas"], 0.0);
        if i > 0 {
            for k in ["wx", "wy", "wz"] {
                assert_eq!(row[k], rows[i - 1][format!("end_{k}").as_str()]);
            }
        }
        let p = u.powered(e).unwrap();
        let (x, y, z) = flight::vessel_basis(p);
        let pose = DQuat::from_mat3(&DMat3::from_cols(g(x), g(y), g(z))).normalize();
        max_angle = max_angle.max(2.0 * pose.dot(q(row)).abs().min(1.0).acos().to_degrees());
        let expected = DVec3::new(-row["wx"], -row["wy"], row["wz"]);
        max_rate = max_rate.max((p.angvel + p.facing * p.roll_rate - expected).length());
        max_mass = max_mass.max((p.mass_t - row["mass_t"]).abs());
        u.set_rotation(e, row["pitch"], row["yaw"], row["roll"]);
        u.step();
    }
    assert!(max_angle <= 0.5, "attitude error {max_angle} degrees");
    assert!(max_rate <= 0.01, "angular-rate error {max_rate} rad/s");
    assert!(max_mass <= 1e-6, "mass error {max_mass} t");
}

//! Reproducible 50 Hz vacuum input/release/reversal/SAS trace. CSV goes to stdout.
use ose_math::DVec3;
use ose_sim::{
    flight::{Attitude, FlightSpec, Powered},
    structure::Structure,
    Universe, TICKS_PER_SECOND,
};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| "Kerbal K1".into());
    let replay = args.get(2).map(|path| {
        let text = std::fs::read_to_string(path).expect("reference CSV");
        let mut lines = text.lines();
        let columns: Vec<_> = lines.next().expect("header").split(',').collect();
        lines
            .map(|line| {
                columns
                    .iter()
                    .cloned()
                    .zip(
                        line.split(',')
                            .map(|v| v.parse::<f64>().expect("numeric capture")),
                    )
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect::<std::collections::HashMap<_, _>>()
            })
            .collect::<Vec<_>>()
    });
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let crafts: Vec<ose_data::craft::Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let craft = crafts
        .iter()
        .find(|c| c.name == name)
        .expect("stock craft name");
    let report = ose_data::staging::delta_v_by_stage(craft, &defs, &res);
    let spec = FlightSpec::from_report(&report, &[], &[], 1.5);
    let mut u = Universe::with_bodies(&bodies);
    let body = u.body_entity("Kerbin").unwrap();
    let initial = replay.as_ref().map(|rows| &rows[0]);
    // Unity Z reflection: position/velocity are polar, angular velocity is axial.
    let r = initial
        .map(|r| DVec3::new(r["px"], r["py"], -r["pz"]))
        .unwrap_or(DVec3::new(700_000., 0., 0.));
    let v = initial
        .map(|r| DVec3::new(r["vx"], r["vy"], -r["vz"]))
        .unwrap_or(DVec3::new(0., 2246., 0.));
    let vessel = u.spawn_vessel(&name, body, r, v);
    let mut p = Powered::new(spec, r, v, false);
    p.facing = DVec3::new(0., 1., 0.);
    p.attitude = Attitude::Free;
    if let Some(row) = initial {
        assert_eq!(
            row["sas"], 0.,
            "replay currently supports SAS-off fixtures only"
        );
        p.ignited = false;
        p.throttle = row["throttle"];
        let q = glam::DQuat::from_xyzw(-row["qx"], -row["qy"], row["qz"], row["qw"]).normalize();
        let facing = q * glam::DVec3::Y;
        p.facing = DVec3::new(facing.x, facing.y, facing.z);
        let top = q * glam::DVec3::Z;
        ose_sim::flight::set_vessel_top(&mut p, DVec3::new(top.x, top.y, top.z));
        let w = DVec3::new(-row["wx"], -row["wy"], row["wz"]);
        p.roll_rate = w.dot(p.facing);
        p.angvel = w - p.facing * p.roll_rate;
    }
    u.set_powered(vessel, p);
    u.set_structure(vessel, Structure::build(craft, &defs, &res));
    if replay.is_some() { eprintln!("OSE initial mass properties: {:?}", u.structure(vessel).unwrap().mass_properties()); }
    println!("t,pitch,yaw,roll,throttle,qx,qy,qz,qw,wx,wy,wz,mass_t");
    let samples = replay
        .as_ref()
        .map(|rows| rows.len())
        .unwrap_or(12 * TICKS_PER_SECOND as usize);
    for step in 0..samples {
        let t = replay
            .as_ref()
            .map(|rows| rows[step]["t"])
            .unwrap_or(step as f64 / TICKS_PER_SECOND as f64);
        let pitch = if (1.0..2.0).contains(&t) {
            1.0
        } else if (3.0..4.0).contains(&t) {
            -1.0
        } else {
            0.0
        };
        let roll = if (5.0..6.0).contains(&t) { 1.0 } else { 0.0 };
        let (pitch, yaw, roll) = replay
            .as_ref()
            .map(|rows| {
                let r = &rows[step];
                (r["pitch"], r["yaw"], r["roll"])
            })
            .unwrap_or((pitch, 0., roll));
        if replay.is_none() && step == 7 * TICKS_PER_SECOND as usize {
            let f = u.powered(vessel).unwrap().facing;
            u.set_controls(vessel, 0., Attitude::Fixed(f));
        }
        u.set_rotation(vessel, pitch, yaw, roll);
        let p = u.powered(vessel).unwrap();
        let (x, y, z) = ose_sim::flight::vessel_basis(p);
        let vector = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
        let q = glam::DQuat::from_mat3(&glam::DMat3::from_cols(vector(x), vector(y), vector(z)));
        let w = p.angvel + p.facing * p.roll_rate;
        println!(
            "{t:.9},{pitch},{yaw},{roll},{},{},{},{},{},{},{},{},{}",
            p.throttle, q.x, q.y, q.z, q.w, w.x, w.y, w.z, p.mass_t
        );
        u.step();
    }
}

//! Reproducible 50 Hz vacuum input/release/reversal/SAS trace. CSV goes to stdout.
use ose_math::DVec3;
use ose_sim::{
    flight::{Attitude, FlightSpec, Powered},
    structure::Structure,
    Universe, TICKS_PER_SECOND,
};
fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Kerbal K1".into());
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
    let r = DVec3::new(700_000., 0., 0.);
    let v = DVec3::new(0., 2246., 0.);
    let vessel = u.spawn_vessel(&name, body, r, v);
    let mut p = Powered::new(spec, r, v, false);
    p.facing = DVec3::new(0., 1., 0.);
    p.attitude = Attitude::Free;
    u.set_powered(vessel, p);
    u.set_structure(vessel, Structure::build(craft, &defs, &res));
    println!("t,pitch,yaw,roll,throttle,qx,qy,qz,qw,wx,wy,wz,mass_t");
    for step in 0..(12 * TICKS_PER_SECOND) {
        let t = step as f64 / TICKS_PER_SECOND as f64;
        let pitch = if (1.0..2.0).contains(&t) {
            1.0
        } else if (3.0..4.0).contains(&t) {
            -1.0
        } else {
            0.0
        };
        let roll = if (5.0..6.0).contains(&t) { 1.0 } else { 0.0 };
        if step == 7 * TICKS_PER_SECOND {
            let f = u.powered(vessel).unwrap().facing;
            u.set_controls(vessel, 0., Attitude::Fixed(f));
        }
        u.set_rotation(vessel, pitch, 0., roll);
        let p = u.powered(vessel).unwrap();
        let (x, y, z) = ose_sim::flight::vessel_basis(p);
        let vector = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
        let q = glam::DQuat::from_mat3(&glam::DMat3::from_cols(vector(x), vector(y), vector(z)));
        let w = p.angvel + p.facing * p.roll_rate;
        println!(
            "{t:.9},{pitch},0,{roll},{},{},{},{},{},{},{},{},{}",
            p.throttle, q.x, q.y, q.z, q.w, w.x, w.y, w.z, p.mass_t
        );
        u.step();
    }
}

//! A craft whose root part strikes the ground harder than it takes: KSP2 destroys the root
//! (`PartBehavior.HandlePartDestruction`) and each part tree it held becomes a vessel of its own, carrying on
//! with the motion it had. Its Stock04_Jet nosing into the runway at 44 m/s lost the cockpit (tolerance 40),
//! nose gear and front tank, and the other nine parts slid on together without breaking again
//! (reference\ksp2-strike-tail-run2).
use std::f64::consts::TAU;

use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

fn kerbin() -> ose_data::CelestialBody {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut b = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    b.parent = None;
    b.orbit = None;
    b.soi_radius_m = None;
    b
}

#[test]
fn a_pod_landing_on_its_head_breaks_and_the_tank_it_held_falls_on_intact() {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-podtank.json"
    ))
    .unwrap();
    let c = ose_importer::craft::craft_from_value(&v).unwrap();
    let body = kerbin();
    let spin = TAU / body.rotation_period_s;
    let mut u = Universe::with_bodies(&[body]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let e = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    let st = Structure::build(&c, &defs, &res);
    let root = st.root;
    let tank_tolerance = (0..st.rigid.len())
        .filter(|&i| i != root)
        .map(|i| st.crash_tolerance[i])
        .fold(f64::INFINITY, f64::min);
    let pod_tolerance = st.crash_tolerance[root];
    u.set_structure(e, st);
    u.set_craft(e, c.clone());
    u.step();

    // upside down, the pod 0.5 m over the ground, coming down at 18 m/s: past the pod's 15, under the tank's 20
    let speed = 18.0;
    assert!(pod_tolerance < speed && speed < tank_tolerance);
    let mut p = u.powered(e).unwrap().clone();
    let up = p.r.normalized();
    let ground = p.r.length() - p.ground_clearance_m;
    p.facing = -up;
    let st = u.structure(e).unwrap();
    let f = flight::vessel_basis(&p);
    let centre = st.mass_properties().centre_m;
    let lowest = (0..st.rigid.len())
        .flat_map(|i| st.ground_points(i))
        .map(|q| {
            let q = q - centre;
            (f.0 * q.x + f.1 * q.y + f.2 * q.z).dot(up)
        })
        .fold(f64::INFINITY, f64::min);
    p.r = up * (ground + 0.5 - lowest);
    let ground_v = |r: DVec3| DVec3::new(0.0, 0.0, spin).cross(r);
    p.v = ground_v(p.r) - up * speed;
    p.landed = false;
    p.attitude = flight::Attitude::Free;
    p.angvel = DVec3::ZERO;
    u.set_powered(e, p);
    u.take_broken_off();

    let mut pieces = Vec::new();
    for _ in 0..(5 * TICKS_PER_SECOND) {
        u.step();
        pieces.extend(u.take_broken_off().into_iter().map(|(_, d)| d));
        if !pieces.is_empty() {
            break;
        }
    }
    assert!(
        u.powered(e).is_none_or(|p| p.crashed),
        "the pod broke, and the vessel with it"
    );
    assert_eq!(pieces.len(), 1, "the tank comes away as one piece");
    let tank = pieces[0];
    // it carries on down as it was going, not thrown about like a wreck
    let q = u.powered(tank).unwrap();
    let down = -(q.v - ground_v(q.r)).dot(up);
    let across = (q.v - ground_v(q.r) + up * down).length();
    assert!(
        down > 0.5 * speed && across < 1.0,
        "down {down:.2}, across {across:.2}"
    );
    let st = u.structure(tank).expect("built part by part");
    assert!(
        st.crash_tolerance.iter().all(|&t| t < 1.0e3),
        "it breaks like any part: {:?}",
        st.crash_tolerance
    );
    // it falls the pod's height and lands on its end, under its tolerance
    for _ in 0..(10 * TICKS_PER_SECOND) {
        u.step();
        assert!(u.take_broken_off().is_empty(), "nothing more breaks off");
    }
    let q = u.powered(tank).expect("the tank came through");
    assert!(!q.crashed, "the tank came through");
    assert!(
        (q.v - ground_v(q.r)).length() < 0.2,
        "and lies still: {:?}",
        q.v - ground_v(q.r)
    );
}

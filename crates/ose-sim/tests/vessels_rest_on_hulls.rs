//! Craft rest on other craft's hulls, as KSP2's colliders hold them (`Universe::crafts_find_decks`): a stage
//! dropped on the pad holds up what was above it, and a pod set down on a landed tank stands on its top (the
//! KSP2 "separate and land" captures: the pod lands on the dropped tank and stays on it).
use std::f64::consts::TAU;

use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

fn data() -> (
    Vec<ose_data::CelestialBody>,
    Vec<ose_data::PartDef>,
    Vec<ose_data::ResourceDef>,
) {
    (
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
    )
}

fn kerbin_universe() -> (Universe, Entity, f64) {
    let (bodies, defs, res) = data();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let spin = TAU / kerbin.rotation_period_s;
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs, res);
    let body = u.body_entity("Kerbin").unwrap();
    (u, body, spin)
}

fn place(u: &mut Universe, body: Entity, c: &ose_data::craft::Craft, lon_deg: f64) -> Entity {
    let (_, defs, res) = data();
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, lon_deg);
    u.set_structure(v, Structure::build(c, &defs, &res));
    u.set_craft(v, c.clone());
    v
}

fn importer(json: &str) -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

#[test]
fn a_pod_set_down_on_a_landed_tank_stands_on_its_top() {
    let (mut u, body, spin) = kerbin_universe();
    let podtank = importer(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-podtank.json"
    ));
    // the tank alone, landed on the pad
    let mut tank = podtank.clone();
    let mut t = tank.parts[1].clone();
    t.parent = None;
    t.own_node = None;
    t.parent_node = None;
    t.local_position = DVec3::ZERO;
    tank.parts = vec![t];
    tank.stages.clear();
    tank.name = "tank".into();
    let tv = place(&mut u, body, &tank, 0.0);
    let tank_top = {
        let p = u.powered(tv).unwrap();
        let st = u.structure(tv).unwrap();
        let (_, half) = st.part_box(0);
        p.r.length() - p.ground_clearance_m + 2.0 * half.y
    };
    // the pod alone, 2 m above the tank's top, coming down at 8 m/s (KSP2 sepland 8)
    let mut pod = podtank.clone();
    pod.parts.truncate(1);
    pod.stages.clear();
    pod.name = "pod".into();
    let pv = place(&mut u, body, &pod, 0.0);
    let mut p = u.powered(pv).unwrap().clone();
    let up = p.r.normalized();
    let ground = p.r.length() - p.ground_clearance_m;
    p.r = up * (tank_top + 2.0 + p.ground_clearance_m);
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) - up * 8.0;
    p.landed = false;
    u.set_powered(pv, p);
    let mut lowest = f64::INFINITY;
    for _ in 0..(5 * TICKS_PER_SECOND) {
        u.step();
        let p = u.powered(pv).unwrap();
        lowest = lowest.min(p.r.length() - p.ground_clearance_m);
    }
    let p = u.powered(pv).unwrap();
    let bottom = p.r.length() - p.ground_clearance_m;
    println!(
        "pod bottom {:.2} m above the pad (tank top {:.2}), lowest {:.2}; landed {} crashed {}",
        bottom - ground,
        tank_top - ground,
        lowest - ground,
        p.landed,
        p.crashed
    );
    assert!(!p.crashed, "8 m/s onto a 1.1 t tank: KSP2's pod survives");
    assert!(
        (bottom - tank_top).abs() < 0.15,
        "it stands on the tank's top, {:.2} m off",
        bottom - tank_top
    );
    assert!(lowest > tank_top - 0.3, "and never sinks into it");
    assert!(p.landed, "at rest");
}

#[test]
fn a_stage_dropped_on_the_pad_holds_up_what_sat_on_it() {
    let (mut u, body, _) = kerbin_universe();
    let crafts: Vec<ose_data::craft::Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
    let v = place(&mut u, body, k1, 0.0);
    assert!(u.set_controls(v, 0.0, flight::Attitude::Up));
    for _ in 0..TICKS_PER_SECOND {
        u.step();
    }
    let alt = |u: &Universe, e: Entity| u.powered(e).unwrap().r.length();
    // light the engines (throttle down), drop the boosters, then the lower stage, all on the pad
    assert!(u.stage(v).fired);
    for _ in 0..3 {
        u.step();
    }
    let mut dropped = Vec::new();
    for _ in 0..2 {
        let s = u.stage(v);
        dropped.extend(s.debris);
        for _ in 0..3 {
            u.step();
        }
    }
    let before = alt(&u, v);
    for _ in 0..(3 * TICKS_PER_SECOND) {
        u.step();
    }
    let after = alt(&u, v);
    let pu = u.powered(v).unwrap();
    let ground = flight::ground_radius(
        u.world.get::<ose_sim::bodies::Body>(body).unwrap(),
        None,
        pu.r,
        u.ut(),
    );
    let bottom = pu.r.length() - pu.ground_clearance_m - ground;
    let lower = *dropped.last().unwrap();
    let pl = u.powered(lower).unwrap();
    let top = u
        .structure(lower)
        .map(|st| {
            let c = st.mass_properties().centre_m;
            (0..st.rigid.len())
                .filter(|&i| st.rigid.present[i])
                .map(|i| {
                    let (centre, half) = st.part_box(i);
                    centre.y + half.y - c.y
                })
                .fold(f64::MIN, f64::max)
        })
        .unwrap_or(f64::NAN)
        + pl.r.length()
        - ground;
    println!(
        "upper: bottom {bottom:.2} m above the pad, landed {} parked-on {:?}; lower stage top {top:.2} m (landed {})",
        pu.landed,
        pu.deck.map(|d| d.radius_m - ground),
        pl.landed
    );
    println!(
        "upper stage moved {:.2} m once the lower one was dropped under it ({} pieces dropped)",
        after - before,
        dropped.len()
    );
    assert!(!dropped.is_empty(), "something was dropped");
    assert!(
        (bottom - top).abs() < 0.15 && pu.deck.is_some(),
        "the upper stage stands on the dropped one's top ({bottom:.2} vs {top:.2} m)"
    );
    assert!(
        after - before > -0.3,
        "the upper stage stays on the dropped one, not sinking {:.2} m through it",
        before - after
    );
}

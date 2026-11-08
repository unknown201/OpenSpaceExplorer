//! KSP2 parity for engine heat (gap T42): an engine firing into a part of its own craft blasts it
//! (`Module_Engine.EngineExhaustDamage`, `ThermalComponentJob`). Craft: the KSP2 pilot's tall stack
//! (`tools/ksp2-parity-pilot/craft-stack.json`: pod, six 1x4 tanks, a Swivel) with a 1x2 tank hung under the
//! Swivel's nozzle, as an upper stage lit before its lower stage is dropped.
//!
//! The Swivel's blast is 253,100 kW at full throttle on a part within a metre of its exhaust; the 1x2 tank
//! (0.125 t dry: thermal mass 62.5) heats at about 4,000 K/s and goes past its 1,000 K in about 0.2 s, while
//! the exhaust pushing on it cancels the engine's thrust, so the craft sits on the pad until the tank is gone.
use bevy_ecs::entity::Entity;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

fn defs() -> Vec<ose_data::PartDef> {
    serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap()
}

fn stack() -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-stack.json"
    ))
    .unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

/// The stack with a 1x2 tank hung right under the Swivel's nozzle.
fn stack_over_tank() -> (ose_data::craft::Craft, usize) {
    let mut c = stack();
    let defs = defs();
    let engine = c
        .parts
        .iter()
        .position(|p| p.part_id == "engine_1v_methalox_swivel")
        .unwrap();
    let cube = |id: &str| {
        defs.iter()
            .find(|d| d.id == id)
            .unwrap()
            .drag_cubes
            .first()
            .cloned()
            .unwrap()
    };
    let (e, t) = (
        cube("engine_1v_methalox_swivel"),
        cube("fueltank_1v_inline_methalox_1x2"),
    );
    let mut tank = c.parts[1].clone();
    tank.part_id = "fueltank_1v_inline_methalox_1x2".into();
    tank.guid = "under-the-nozzle".into();
    tank.parent = Some(engine);
    tank.own_node = None;
    tank.parent_node = None;
    tank.stage_index = c.parts[engine].stage_index;
    tank.local_position.y = c.parts[engine].local_position.y + e.center.y
        - e.size.y / 2.0
        - (t.center.y + t.size.y / 2.0);
    tank.resources.clear();
    c.parts.push(tank);
    let index = c.parts.len() - 1;
    (c, index)
}

fn on_pad(c: &ose_data::craft::Craft) -> (Universe, Entity) {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let defs = defs();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(c, &defs, &res));
    // as the server launches it: the craft rides along, so pieces that come off are built from it
    u.set_craft(v, c.clone());
    (u, v)
}

/// Light the Swivel at `throttle` holding attitude; run `seconds`. Returns the tick the part `watch` was
/// lost (if it was), its hottest temperature, and how far the craft climbed by then.
fn burn(
    c: &ose_data::craft::Craft,
    watch: usize,
    throttle: f64,
    seconds: f64,
) -> (Option<usize>, f64, f64) {
    let (mut u, v) = on_pad(c);
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, throttle, flight::Attitude::Fixed(facing)));
    let r0 = u.powered(v).unwrap().r.length();
    let (mut lost, mut hottest, mut climbed) = (None, 0.0f64, 0.0);
    for k in 1..=((seconds / DT).round() as usize) {
        u.step();
        let st = u.structure(v).unwrap();
        if lost.is_none() {
            if st.rigid.present[watch] {
                hottest = hottest.max(st.temperature_k[watch]);
                climbed = u.powered(v).unwrap().r.length() - r0;
            } else {
                lost = Some(k);
            }
        }
    }
    (lost, hottest, climbed)
}

#[test]
fn ksp2_an_engine_lit_into_its_own_lower_stage_blasts_it_apart() {
    let (c, tank) = stack_over_tank();
    let (lost, hottest, climbed) = burn(&c, tank, 1.0, 1.0);
    println!("full throttle: tank lost on tick {lost:?} at {hottest:.0} K, climbed {climbed:.3} m");
    // 280 K to 1,000 K at ~4,000 K/s: gone within a quarter second (plus a tick for heat to follow the blast)
    let lost = lost.expect("the tank under the nozzle burns up");
    assert!((8..=13).contains(&lost), "lost on tick {lost}");
    assert!(hottest > 900.0);
    // its push cancels the engine's thrust: the craft does not leave the pad while the tank is there
    assert!(climbed < 0.05, "climbed {climbed} m");

    // at a tenth of the throttle it takes ten times as long
    let (lost, _, _) = burn(&c, tank, 0.1, 4.0);
    let lost = lost.expect("burns up at 10% too");
    assert!((90..=125).contains(&lost), "lost on tick {lost} at 10%");
}

#[test]
fn ksp2_an_engine_firing_into_open_air_heats_nothing_on_its_own_craft() {
    let c = stack();
    let (mut u, v) = on_pad(&c);
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    for _ in 0..(3 * TICKS_PER_SECOND) {
        u.step();
    }
    let st = u.structure(v).unwrap();
    assert!(st.exhaust_flux_kw.iter().all(|&f| f == 0.0));
    assert_eq!(
        st.rigid.present.iter().filter(|&&p| p).count(),
        c.parts.len()
    );
    assert!(
        u.powered(v).unwrap().r.length() > 600_000.0 + 10.0,
        "it flies"
    );
}

/// The stack with a decoupler and a 1x2 tank under the Swivel, the decoupler staged after the engine.
fn stack_over_dropped_tank() -> (ose_data::craft::Craft, usize, usize) {
    let mut c = stack();
    let defs = defs();
    let engine = c
        .parts
        .iter()
        .position(|p| p.part_id == "engine_1v_methalox_swivel")
        .unwrap();
    let cube = |id: &str| {
        defs.iter()
            .find(|d| d.id == id)
            .unwrap()
            .drag_cubes
            .first()
            .cloned()
            .unwrap()
    };
    let below = |above: &ose_data::DragCube, y: f64, part: &ose_data::DragCube| {
        y + above.center.y - above.size.y / 2.0 - (part.center.y + part.size.y / 2.0)
    };
    let (e, dc, t) = (
        cube("engine_1v_methalox_swivel"),
        cube("decoupler_1v_inline"),
        cube("fueltank_1v_inline_methalox_1x2"),
    );
    let mut decoupler = c.parts[1].clone();
    decoupler.part_id = "decoupler_1v_inline".into();
    decoupler.guid = "drop".into();
    decoupler.parent = Some(engine);
    decoupler.own_node = Some("top".into());
    decoupler.parent_node = Some("bottom".into());
    decoupler.resources.clear();
    decoupler.local_position.y = below(&e, c.parts[engine].local_position.y, &dc);
    let mut tank = decoupler.clone();
    tank.part_id = "fueltank_1v_inline_methalox_1x2".into();
    tank.guid = "dropped-tank".into();
    tank.parent = Some(c.parts.len());
    tank.local_position.y = below(&dc, decoupler.local_position.y, &t);
    c.parts.push(decoupler);
    c.parts.push(tank);
    let (d, t) = (c.parts.len() - 2, c.parts.len() - 1);
    // the decoupler fires on the press after the engine's (the last stage listed fires first)
    c.stages.insert(0, vec![d]);
    (c, d, t)
}

#[test]
fn ksp2_a_stage_dropped_under_a_lit_engine_is_scorched_and_blown_away() {
    let (c, _, _) = stack_over_dropped_tank();
    let (mut u, v) = on_pad(&c);
    // coasting in vacuum, 100 km up
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    p.r = up * 700_000.0;
    let east = ose_math::DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
    p.v = east * (3.5316e12_f64 / 700_000.0).sqrt();
    p.landed = false;
    u.set_powered(v, p);
    // light the engine at zero throttle, then open it up and drop the tank on the same tick
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 0.0, flight::Attitude::Fixed(facing)));
    assert!(u.stage(v).fired);
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    let fired = u.stage(v);
    assert!(fired.fired);
    let debris = fired.debris.expect("the tank drops away");
    assert!(
        u.structure(debris).is_some(),
        "a stage dropped under engines at throttle is built to take the blast"
    );
    let mut hottest = 0.0f64;
    for _ in 0..(TICKS_PER_SECOND / 2) {
        u.step();
        if let Some(st) = u.structure(debris) {
            hottest = hottest.max(
                (0..st.rigid.len())
                    .filter(|&i| st.rigid.present[i])
                    .map(|i| st.temperature_k[i])
                    .fold(0.0, f64::max),
            );
        }
    }
    let (pv, pd) = (u.powered(v).unwrap(), u.powered(debris).unwrap());
    let apart = (pv.v - pd.v).dot(pv.facing);
    println!("dropped stage: hottest {hottest:.0} K, {apart:.1} m/s apart after 0.5 s");
    // the blast heats the dropped tank hard in the moment it is close...
    assert!(hottest > 600.0, "hottest {hottest:.0} K");
    // ...and throws the light empty tank away far faster than the upper stage's own 13 m/s^2 would part them
    assert!(apart > 20.0, "{apart:.1} m/s apart");
}

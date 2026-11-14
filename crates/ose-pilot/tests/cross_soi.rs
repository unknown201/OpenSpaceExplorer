//! Rendezvous v2 cross-SOI scenarios (R4). Setup places vessels in orbit or on the pad; the pilot
//! flies the rest with ordinary controls:
//! - T3: launch from KSC and dock at a station in Mun orbit.
//! - T5: a shuttle docked at an LKO station undocks, transfers to the Mun, captures, and docks at a
//!   station in Mun orbit.
//! - T6: the reverse: from a Mun-orbit station back to an LKO station.
use ose_data::craft::Craft;
use ose_pilot::{
    ascent::AscentParams,
    mission::{Mission, MissionParams},
    Status,
};

mod common;
use common::{fly, in_orbit, in_orbit_around, links, modules, on_pad, world, World};

fn print(m: &Mission) {
    for line in &m.log {
        println!("{line}");
    }
}

/// Dock `shuttle`'s aft port at `station`'s forward port (setup flown by the pilot), returning the
/// combined vessel.
fn dock_first(
    w: &mut World,
    station: bevy_ecs::entity::Entity,
    shuttle: bevy_ecs::entity::Entity,
) -> bevy_ecs::entity::Entity {
    let mut first = Mission::new(
        shuttle,
        MissionParams::new(station, station_port(w, station), "iss-unity-aft"),
    );
    let results = fly(w, std::slice::from_mut(&mut first), 3_600.0);
    print(&first);
    assert_eq!(results[0], Status::Done, "{:?}", first.log);
    first.vessel()
}

fn station_port(w: &World, station: bevy_ecs::entity::Entity) -> &'static str {
    let c = w.u.current_craft(station).unwrap();
    if c.parts.iter().any(|p| p.guid == "iss-zarya-forward") {
        "iss-zarya-forward"
    } else {
        "iss-zvezda-forward"
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t5_shuttle_leaves_an_leo_station_and_docks_at_a_mun_station() {
    let m = modules();
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    let leo = in_orbit(&mut w, &m.zvezda, 100_000.0, 0.0);
    let shuttle = in_orbit(&mut w, &m.unity, 100_000.0, -0.05);
    let mun_station = in_orbit_around(&mut w, mun, &m.zarya, 50_000.0, 0.0);
    let docked = dock_first(&mut w, leo, shuttle);

    let mut params = MissionParams::new(mun_station, "iss-zarya-forward", "iss-unity-forward");
    params.depart_from_port = Some("iss-unity-aft".into());
    let mut run = Mission::new(docked, params);
    let results = fly(&mut w, std::slice::from_mut(&mut run), 3.0 * 86_400.0);
    print(&run);
    assert_eq!(results[0], Status::Done, "{:?}", run.log);
    let at = w.u.current_craft(run.vessel()).unwrap();
    let has = |guid: &str| at.parts.iter().any(|p| p.guid == guid);
    assert!(has("iss-zarya-forward") && has("iss-unity-forward"));
    assert!(!has("iss-zvezda-forward"), "the LKO station stayed behind");
    assert_eq!(links(&w, run.vessel()), 1);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t6_shuttle_leaves_a_mun_station_and_docks_at_an_leo_station() {
    let m = modules();
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    let mun_station = in_orbit_around(&mut w, mun, &m.zarya, 50_000.0, 0.0);
    let shuttle = in_orbit_around(&mut w, mun, &m.unity, 50_000.0, -0.2);
    let leo = in_orbit(&mut w, &m.zvezda, 100_000.0, 0.0);
    let docked = dock_first(&mut w, mun_station, shuttle);

    let mut params = MissionParams::new(leo, "iss-zvezda-forward", "iss-unity-forward");
    params.depart_from_port = Some("iss-unity-aft".into());
    let mut run = Mission::new(docked, params);
    let results = fly(&mut w, std::slice::from_mut(&mut run), 3.0 * 86_400.0);
    print(&run);
    assert_eq!(results[0], Status::Done, "{:?}", run.log);
    let at = w.u.current_craft(run.vessel()).unwrap();
    let has = |guid: &str| at.parts.iter().any(|p| p.guid == guid);
    assert!(has("iss-zvezda-forward") && has("iss-unity-forward"));
    assert!(!has("iss-zarya-forward"), "the Mun station stayed behind");
    assert_eq!(links(&w, run.vessel()), 1);
}

#[derive(serde::Deserialize)]
struct Launchers {
    chaser: Craft,
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t3_launch_from_ksc_and_dock_at_a_mun_station() {
    let m = modules();
    let launchers: Launchers = serde_json::from_str(include_str!("data/t1-crafts.json")).unwrap();
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    let mun_station = in_orbit_around(&mut w, mun, &m.zarya, 50_000.0, 0.0);
    let rocket = on_pad(&mut w, &launchers.chaser);

    let mut params = MissionParams::new(mun_station, "iss-zarya-forward", "iss-unity-forward");
    params.ascent = Some(AscentParams {
        stage_in_orbit: false,
        ..AscentParams::default()
    });
    let mut run = Mission::new(rocket, params);
    let results = fly(&mut w, std::slice::from_mut(&mut run), 3.0 * 86_400.0);
    print(&run);
    assert_eq!(results[0], Status::Done, "{:?}", run.log);
    let at = w.u.current_craft(run.vessel()).unwrap();
    let has = |guid: &str| at.parts.iter().any(|p| p.guid == guid);
    assert!(has("iss-zarya-forward") && has("iss-unity-forward"));
    assert_eq!(links(&w, run.vessel()), 1);
}

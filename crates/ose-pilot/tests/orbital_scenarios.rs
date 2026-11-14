//! Rendezvous v2 orbital scenarios for the pilot (setup places vessels in orbit; the pilot then flies
//! everything with ordinary controls):
//! - T4: a shuttle docked at one station undocks, transfers to a depot at another altitude, and docks.
//! - T9: two craft go for the same port at the same time. No traffic control: one wins, the other
//!   fails cleanly, and nothing panics or gets stuck.
use bevy_ecs::entity::Entity;
use ose_data::craft::Craft;
use ose_pilot::{
    mission::{Catalog, Mission, MissionParams},
    Status,
};
use serde::Deserialize;

mod common;
use common::{in_orbit, spec, world, World};

#[derive(Deserialize)]
struct Modules {
    zarya: Craft,
    unity: Craft,
    zvezda: Craft,
}

fn modules() -> Modules {
    serde_json::from_str(include_str!("data/orbital-modules.json")).unwrap()
}

/// A copy of a craft whose part identities don't collide with the original's.
fn renamed(c: &Craft, suffix: &str) -> Craft {
    let mut c = c.clone();
    c.name = format!("{}{suffix}", c.name);
    for p in &mut c.parts {
        p.guid = format!("{}{suffix}", p.guid);
    }
    c
}

/// Fly the missions together until each one ends, or the time runs out.
fn fly(w: &mut World, missions: &mut [Mission], limit_s: f64) -> Vec<Status> {
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| spec(c, &defs, &res);
    let catalog = Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    let start = w.u.ut();
    let mut results = vec![Status::Running; missions.len()];
    while results.contains(&Status::Running) {
        for (m, result) in missions.iter_mut().zip(results.iter_mut()) {
            if *result == Status::Running {
                *result = m.update(&mut w.u, &catalog);
            }
        }
        assert!(
            w.u.ut() - start < limit_s,
            "missions still running after {limit_s} s: {:?}",
            missions
                .iter()
                .map(|m| (m.leg(), m.log.clone()))
                .collect::<Vec<_>>()
        );
        let warp = missions
            .iter()
            .zip(&results)
            .filter(|(_, r)| **r == Status::Running)
            .map(|(m, _)| m.suggested_warp(&w.u))
            .min()
            .unwrap_or(1);
        w.u.set_warp(warp);
        w.u.step();
        w.u.dock_latched(&defs, &res, &spec_for);
    }
    results
}

fn links(w: &World, e: Entity) -> usize {
    w.u.docked_links(e).len()
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t4_shuttle_undocks_from_one_station_and_docks_at_a_depot_at_another_altitude() {
    let m = modules();
    let mut w = world();
    let station = in_orbit(&mut w, &m.zarya, 100_000.0, 0.0);
    let shuttle = in_orbit(&mut w, &m.unity, 100_000.0, -0.05);
    let depot = in_orbit(&mut w, &m.zvezda, 250_000.0, 120.0);

    // Setup by the pilot itself: dock the shuttle at the station it will leave.
    let mut first = Mission::new(
        shuttle,
        MissionParams::new(station, "iss-zarya-forward", "iss-unity-aft"),
    );
    let results = fly(&mut w, std::slice::from_mut(&mut first), 3_600.0);
    assert_eq!(results[0], Status::Done, "{:?}", first.log);
    let docked = first.vessel();
    assert_eq!(links(&w, docked), 1);

    // The run: undock, back off, transfer up to the depot, dock at its forward port.
    let mut params = MissionParams::new(depot, "iss-zvezda-forward", "iss-unity-forward");
    params.depart_from_port = Some("iss-unity-aft".into());
    let mut run = Mission::new(docked, params);
    let results = fly(&mut w, std::slice::from_mut(&mut run), 6.0 * 3600.0);
    for line in &run.log {
        println!("{line}");
    }
    assert_eq!(results[0], Status::Done, "{:?}", run.log);
    let at_depot = w.u.current_craft(run.vessel()).unwrap();
    let has = |guid: &str| at_depot.parts.iter().any(|p| p.guid == guid);
    assert!(has("iss-zvezda-forward") && has("iss-unity-forward"));
    assert!(!has("iss-zarya-forward"), "the station stayed behind");
    assert_eq!(links(&w, run.vessel()), 1);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t9_two_craft_race_for_one_port_and_exactly_one_docks() {
    let m = modules();
    let mut w = world();
    let station = in_orbit(&mut w, &m.zarya, 100_000.0, 0.0);
    let behind = in_orbit(&mut w, &m.unity, 100_000.0, -0.05);
    let ahead = in_orbit(&mut w, &renamed(&m.unity, "-b"), 100_000.0, 0.05);
    let mut missions = [
        Mission::new(
            behind,
            MissionParams::new(station, "iss-zarya-forward", "iss-unity-aft"),
        ),
        Mission::new(
            ahead,
            MissionParams::new(station, "iss-zarya-forward", "iss-unity-aft-b"),
        ),
    ];
    let results = fly(&mut w, &mut missions, 3.0 * 3600.0);
    for (i, m) in missions.iter().enumerate() {
        println!("craft {i}: {:?} {:?}", results[i], m.log);
    }
    let docked = results.iter().filter(|r| **r == Status::Done).count();
    assert_eq!(docked, 1, "{results:?}");
    let winner = missions[results.iter().position(|r| *r == Status::Done).unwrap()].vessel();
    assert_eq!(links(&w, winner), 1);
}

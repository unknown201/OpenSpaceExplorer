//! Rendezvous v2 launch scenarios with launch windows (R5):
//! - T2: from the KSC pad to a station in a 20 degree inclined low orbit. A first flight of the same
//!   rocket measures its ascent profile; the run then waits on the pad for a plane and phase window.
//! - T7: lift off from the Mun's surface and dock at a station in Mun orbit, timed the same way.
use ose_pilot::{
    ascent::AscentParams,
    mission::{Mission, MissionParams},
    Status,
};

mod common;
use common::{fly, in_orbit_around, in_orbit_inclined, links, modules, on_pad, on_surface, world};
use ose_data::craft::Craft;
use ose_pilot::window::LaunchProfile;

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t7_lift_off_from_the_mun_and_dock_at_a_station_in_mun_orbit() {
    let m = modules();
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    let station = in_orbit_around(&mut w, mun, &m.zarya, 30_000.0, 0.0);
    let lander = on_surface(&mut w, mun, &m.unity, 0.0, 0.0);
    let profile = prove(|w| {
        let mun = w.u.body_entity("Mun").unwrap();
        let station = in_orbit_around(w, mun, &m.zarya, 30_000.0, 0.0);
        let lander = on_surface(w, mun, &m.unity, 0.0, 0.0);
        let mut params = MissionParams::new(station, "iss-zarya-forward", "iss-unity-forward");
        params.ascent = Some(AscentParams::airless(30_000.0));
        Mission::new(lander, params)
    });

    let mut params = MissionParams::new(station, "iss-zarya-forward", "iss-unity-forward");
    params.ascent = Some(AscentParams::airless(30_000.0));
    params.launch_profile = Some(profile);
    let mut run = Mission::new(lander, params);
    let results = fly(&mut w, std::slice::from_mut(&mut run), 86_400.0);
    for line in &run.log {
        println!("{line}");
    }
    assert_eq!(results[0], Status::Done, "{:?}", run.log);
    let at = w.u.current_craft(run.vessel()).unwrap();
    let has = |guid: &str| at.parts.iter().any(|p| p.guid == guid);
    assert!(has("iss-zarya-forward") && has("iss-unity-forward"));
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
fn t2_launch_window_from_ksc_to_a_station_in_an_inclined_orbit() {
    let m = modules();
    let launchers: Launchers = serde_json::from_str(include_str!("data/t1-crafts.json")).unwrap();
    let ascent = AscentParams {
        stage_in_orbit: true,
        ..AscentParams::default()
    };

    // Proving flight, in a world of its own: the same rocket flown into the station's plane.
    let profile = prove(|w| {
        let kerbin = w.kerbin;
        let station = in_orbit_inclined(w, kerbin, &m.zarya, 100_000.0, 0.0, 20.0);
        let rocket = on_pad(w, &launchers.chaser);
        let mut params = MissionParams::new(station, "iss-zarya-forward", "iss-unity-aft");
        params.ascent = Some(ascent.clone());
        Mission::new(rocket, params)
    });

    let mut w = world();
    let kerbin = w.kerbin;
    let station = in_orbit_inclined(&mut w, kerbin, &m.zarya, 100_000.0, 0.0, 20.0);
    let rocket = on_pad(&mut w, &launchers.chaser);
    let mut params = MissionParams::new(station, "iss-zarya-forward", "iss-unity-aft");
    params.ascent = Some(ascent);
    params.launch_profile = Some(profile);
    let mut run = Mission::new(rocket, params);
    let results = fly(&mut w, std::slice::from_mut(&mut run), 3.0 * 86_400.0);
    for line in &run.log {
        println!("{line}");
    }
    assert_eq!(results[0], Status::Done, "{:?}", run.log);
    assert_eq!(links(&w, run.vessel()), 1);
}

/// Fly a mission's ascent in a fresh world and return the measured profile, as a route's proving
/// flight would.
fn prove(setup: impl FnOnce(&mut common::World) -> Mission) -> LaunchProfile {
    let mut w = world();
    let mut m = setup(&mut w);
    while m.flown_profile().is_none() {
        assert_eq!(fly_ticks(&mut w, &mut m), Status::Running, "{:?}", m.log);
    }
    println!("proving flight: {:?}", m.log);
    m.flown_profile().unwrap()
}

/// A few hundred ticks of one mission (no docking checks needed before orbit).
fn fly_ticks(w: &mut common::World, m: &mut Mission) -> Status {
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| common::spec(c, &defs, &res);
    let catalog = ose_pilot::mission::Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    for _ in 0..200 {
        let s = m.update(&mut w.u, &catalog);
        if s != Status::Running || m.flown_profile().is_some() {
            return s;
        }
        w.u.set_warp(m.suggested_warp(&w.u));
        w.u.step();
    }
    Status::Running
}

//! Rendezvous v2 scenario T1: launch a station core from KSC, then launch a second craft and let the
//! pilot fly it from the pad to a docked joint on the station, with ordinary controls only.
use bevy_ecs::entity::Entity;
use ose_data::{craft::Craft, CelestialBody, PartDef, ResourceDef};
use ose_pilot::{
    ascent::{Ascent, AscentParams},
    mission::{Catalog, Leg, Mission, MissionParams},
    Status,
};
use ose_sim::{flight::Attitude, structure::Structure, Universe};
use serde::Deserialize;

mod common;
use common::spec;

#[derive(Deserialize)]
struct Fixture {
    core: Craft,
    chaser: Craft,
    incoming: String,
    target: String,
}

fn on_pad(
    u: &mut Universe,
    body: Entity,
    craft: &Craft,
    defs: &[PartDef],
    res: &[ResourceDef],
) -> Entity {
    let e =
        u.spawn_powered_on_surface(&craft.name, body, spec(craft, defs, res), -0.6836, -74.8198);
    u.set_craft(e, craft.clone());
    u.set_structure(e, Structure::build(craft, defs, res));
    e
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t1_pilot_flies_from_the_pad_to_a_docked_joint_on_a_station_in_low_orbit() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("data/t1-crafts.json")).expect("fixture");
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies: Vec<CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut u = Universe::with_bodies(&bodies);
    u.set_part_catalog(defs.clone(), resources.clone());
    let kerbin = u.body_entity("Kerbin").unwrap();
    let spec_for = |c: &Craft| spec(c, &defs, &resources);
    let catalog = Catalog {
        defs: &defs,
        resources: &resources,
        spec: &spec_for,
    };
    let in_orbit = AscentParams {
        stage_in_orbit: true,
        ..AscentParams::default()
    };

    // The station: an ordinary ascent flown by the same pilot.
    let station = on_pad(&mut u, kerbin, &fixture.core, &defs, &resources);
    let mut ascent = Ascent::new(in_orbit.clone());
    loop {
        match ascent.update(&mut u, station) {
            Status::Running => {}
            Status::Done => break,
            Status::Failed(why) => panic!("station ascent: {why}"),
        }
        u.set_warp(ascent.suggested_warp());
        u.step();
    }
    let station_parts = u.current_craft(station).unwrap().parts.len();
    let o = ose_pilot::orbit(&u, station).unwrap();
    let (_, _, radius) = ose_pilot::body(&u, station).unwrap();
    assert!(o.periapsis() > radius + 85_000.0, "station orbit {o:?}");

    // The delivery: pad to dock.
    let chaser = on_pad(&mut u, kerbin, &fixture.chaser, &defs, &resources);
    let mut params = MissionParams::new(station, &fixture.target, &fixture.incoming);
    params.ascent = Some(in_orbit);
    let mut mission = Mission::new(chaser, params);
    let started = u.ut();
    let mut held = false;
    let result = loop {
        let status = mission.update(&mut u, &catalog);
        if status != Status::Running {
            break status;
        }
        // A depot doesn't steer itself, but it holds its attitude like a parked station would.
        if mission.leg() == Leg::Approach && !held {
            u.wake(station);
            let facing = u.powered(station).unwrap().facing;
            u.set_controls(station, 0.0, Attitude::Fixed(facing));
            held = true;
        }
        assert!(
            u.ut() - started < 6.0 * 3600.0,
            "mission timed out: {:?}",
            mission.log
        );
        u.set_warp(mission.suggested_warp(&u));
        u.step();
        u.dock_latched(&defs, &resources, &|c| spec(c, &defs, &resources));
    };
    for line in &mission.log {
        println!("{line}");
    }
    assert_eq!(result, Status::Done, "{:?}", mission.log);

    // One vessel now, holding every part of both, with one new docked joint.
    let kept = [station, chaser]
        .into_iter()
        .find(|e| u.current_craft(*e).is_some())
        .expect("the docked vessel");
    let craft = u.current_craft(kept).unwrap();
    for guid in [&fixture.target, &fixture.incoming] {
        assert!(craft.parts.iter().any(|p| &p.guid == guid), "lost {guid}");
    }
    assert!(craft.parts.len() > station_parts);
    assert_eq!(u.docked_links(kept).len(), 1);
}

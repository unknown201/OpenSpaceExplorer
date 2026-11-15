//! Delivery routes step 1: record a proving flight flown by the pilot. A tanker launches from the KSC
//! pad, docks at a depot in low orbit, pumps monopropellant into the depot's empty tank, and the
//! player ends the recording there (a one-way route).
//!
//! Both crafts name their parts as the editor does (`p0`, `p1`, ...), so the names repeat between them and
//! docking renames the repeats, and the recorder is saved and loaded again on the way.
use bevy_ecs::entity::Entity;
use ose_data::craft::Craft;
use ose_pilot::{
    ascent::AscentParams,
    mission::{Catalog, Mission, MissionParams},
    Status,
};
use ose_routes::recording::{Event, Phase, Recorder, Recording, State};
use ose_routes::Routes;
use ose_sim::Universe;

#[path = "../../ose-pilot/tests/common/mod.rs"]
mod common;
use common::{in_orbit, modules, on_pad, spec, world};

/// Name the parts `p0`, `p1`, ... as the editor does, all but `keep` (the ports and tanks the test uses).
fn editor_names(c: &mut Craft, keep: &[&str]) {
    for (i, p) in c.parts.iter_mut().enumerate() {
        if !keep.contains(&p.guid.as_str()) {
            p.guid = format!("p{i}");
        }
    }
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
fn records_a_one_way_fuel_run_to_a_depot() {
    let m = modules();
    let launchers: Launchers =
        serde_json::from_str(include_str!("../../ose-pilot/tests/data/t1-crafts.json")).unwrap();
    let mut w = world();
    // The depot, with its monopropellant tank drained so the run has somewhere to pump.
    let mut zarya = m.zarya.clone();
    for p in zarya
        .parts
        .iter_mut()
        .filter(|p| p.guid == "iss-zarya-service-tank")
    {
        for r in &mut p.resources {
            r.stored_units = 0.0;
        }
    }
    editor_names(&mut zarya, &["iss-zarya-forward", "iss-zarya-service-tank"]);
    let mut chaser = launchers.chaser.clone();
    editor_names(&mut chaser, &["iss-unity-aft", "iss-unity-service-tank"]);
    let depot = in_orbit(&mut w, &zarya, 100_000.0, 0.0);
    let tanker = on_pad(&mut w, &chaser);
    // the campaign's routes book follows the depot as the server does
    let mut book = Routes::default();
    assert!(book.set_depot(&w.u, depot, 1, true).unwrap());
    let depot_parts = zarya.parts.len();

    let mut rec = Recorder::start(&w.u, tanker).expect("on the pad");
    let mut params = MissionParams::new(depot, "iss-zarya-forward", "iss-unity-aft");
    params.ascent = Some(AscentParams {
        stage_in_orbit: true,
        ..AscentParams::default()
    });
    let mut run = Mission::new(tanker, params);
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| spec(c, &defs, &res);
    let catalog = Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    let start = w.u.ut();
    let mut saved = false;
    loop {
        let s = run.update(&mut w.u, &catalog);
        book.follow_depots(&mut w.u);
        rec.observe(&mut w.u, &|_: &Universe, e: Entity| book.is_depot_vessel(e));
        if !saved && rec.phase() == Phase::Transfer {
            // a save and a restart on the way: the recorder finds its vessel again by its parts
            saved = true;
            rec = serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        }
        if s != Status::Running {
            assert_eq!(s, Status::Done, "{:?}", run.log);
            break;
        }
        assert!(w.u.ut() - start < 6.0 * 3600.0, "{:?}", run.log);
        w.u.set_warp(run.suggested_warp(&w.u));
        w.u.step();
        w.u.dock_latched(&defs, &res, &spec_for);
    }
    // the tick that docked
    w.u.step();
    book.follow_depots(&mut w.u);
    rec.observe(&mut w.u, &|_: &Universe, e: Entity| book.is_depot_vessel(e));
    assert_eq!(rec.phase(), Phase::Docked, "{:?}", rec.recording.markers);
    assert!(
        book.is_depot_vessel(rec.vessel()),
        "the docked vessel carries the depot"
    );
    assert_eq!(
        book.depots[0].parts.len(),
        depot_parts,
        "only the depot's own parts"
    );

    // The player pumps monopropellant from the tanker's tank into the depot's.
    let joined = rec.vessel();
    let craft = w.u.current_craft(joined).unwrap();
    let index = |guid: &str| craft.parts.iter().position(|p| p.guid == guid).unwrap();
    w.u.transfer_resource(
        joined,
        "MonoPropellant",
        &[index("iss-unity-service-tank")],
        &[index("iss-zarya-service-tank")],
    )
    .expect("transfer");
    for _ in 0..(60 * 50) {
        w.u.set_warp(1);
        w.u.step();
        book.follow_depots(&mut w.u);
        rec.observe(&mut w.u, &|_: &Universe, e: Entity| book.is_depot_vessel(e));
    }
    rec.finish_docked(&mut w.u).unwrap();
    let r: &Recording = &rec.recording;

    let burns = r.events().filter(|e| **e == Event::BurnStart).count();
    assert!((1..=12).contains(&burns), "{burns} burns");
    for m in &r.markers {
        println!("{:>8.0} {:?}", m.ut, m.event);
    }
    for p in &r.phases {
        println!("{:?} {:.0} s {:?}", p.phase, p.end_ut - p.start_ut, p.used);
    }
    println!(
        "delivered {:?}, {} samples",
        r.delivered,
        r.trajectory.len()
    );

    assert_eq!(r.state, State::OneWay);
    let order: Vec<&str> = r
        .events()
        .filter_map(|e| match e {
            Event::Liftoff => Some("liftoff"),
            Event::Orbit { .. } => Some("orbit"),
            Event::Arrived { .. } => Some("arrived"),
            Event::Docked { .. } => Some("docked"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["liftoff", "orbit", "arrived", "docked"]);
    assert_eq!(r.depot_port.as_deref(), Some("iss-zarya-forward"));
    assert_eq!(r.own_port.as_deref(), Some("iss-unity-aft"));
    assert!(
        r.events().any(|e| *e == Event::BurnStart),
        "transfer burns are marked"
    );
    let ascent = r
        .phases
        .iter()
        .find(|p| p.phase == Some(Phase::Ascent))
        .unwrap();
    assert!(
        ascent.used.values().sum::<f64>() > 100.0,
        "the ascent burned fuel: {:?}",
        ascent.used
    );
    let mono = r.delivered.get("MonoPropellant").copied().unwrap_or(0.0);
    assert!(
        mono > 1.0,
        "monopropellant reached the depot: {:?}",
        r.delivered
    );
    let docked = r
        .phases
        .iter()
        .find(|p| p.phase == Some(Phase::Docked))
        .unwrap();
    let gave = docked.used.get("MonoPropellant").copied().unwrap_or(0.0);
    assert!(
        (gave - mono).abs() < 0.05 * mono,
        "the tanker gave what the depot got: {gave} vs {mono}"
    );
    assert!(
        r.trajectory.iter().any(|s| s.rel.is_some()),
        "the approach is kept in the depot's frame"
    );
    // it survives the save
    let json = serde_json::to_string(r).unwrap();
    let back: Recording = serde_json::from_str(&json).unwrap();
    assert_eq!(back.markers.len(), r.markers.len());
    assert_eq!(back.trajectory.len(), r.trajectory.len());
    assert_eq!(back.state, r.state);
    assert_eq!(
        back.delivered.keys().collect::<Vec<_>>(),
        r.delivered.keys().collect::<Vec<_>>()
    );
}

//! Delivery routes, depot to depot (design 3.12): a shuttle leaves one station, flies to another in the same
//! orbit and pumps monopropellant into its tank, and the player ends the recording there. The shuttle route
//! made from it sends the same vessel back to the first station, fills it up there, and flies it over again
//! to deliver the same; the server restarts on the way.
use ose_data::craft::Craft;
use ose_pilot::{
    mission::{Catalog, Mission, MissionParams},
    Status,
};
use ose_routes::recording::{Phase, State};
use ose_routes::route::{Outcome, Pilots};
use ose_routes::shuttle::ShuttleLeg;
use ose_routes::Routes;

#[path = "../../ose-pilot/tests/common/mod.rs"]
mod common;
use common::{in_orbit, modules, spec, world, World};

fn set_tank(c: &mut Craft, guid: &str, units: f64) {
    for p in c.parts.iter_mut().filter(|p| p.guid == guid) {
        for r in &mut p.resources {
            r.stored_units = units;
        }
    }
}

fn index(w: &World, e: bevy_ecs::entity::Entity, guid: &str) -> usize {
    let c = w.u.current_craft(e).unwrap();
    c.parts.iter().position(|p| p.guid == guid).unwrap()
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn a_shuttle_carries_monopropellant_from_one_station_to_another() {
    let m = modules();
    let mut w = world();
    // The origin is full; the destination's tank has room for 1.5 units.
    let origin = in_orbit(&mut w, &m.zarya, 100_000.0, 0.0);
    let mut zvezda = m.zvezda.clone();
    set_tank(&mut zvezda, "iss-zvezda-service-tank", 3.0);
    let dest = in_orbit(&mut w, &zvezda, 100_000.0, 2.0);
    let shuttle = in_orbit(&mut w, &m.unity, 100_000.0, -0.05);
    w.u.rename_vessel(shuttle, "Shuttle");
    let mut book = Routes::default();
    assert!(book.set_depot(&w.u, origin, 1, true).unwrap());
    assert!(book.set_depot(&w.u, dest, 1, true).unwrap());
    let (origin_id, dest_id) = (book.depots[0].id, book.depots[1].id);

    // The proving flight starts next to the origin, in orbit.
    let proving = book.start(&w.u, shuttle, 1).expect("next to a depot");
    assert_eq!(book.provings[0].origin, Some(origin_id));
    assert_eq!(book.provings[0].recorder.phase(), Phase::Transfer);
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| spec(c, &defs, &res);
    let catalog = Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    let mut run = Mission::new(
        shuttle,
        MissionParams::new(dest, "iss-zvezda-forward", "iss-unity-forward"),
    );
    let start = w.u.ut();
    loop {
        let s = run.update(&mut w.u, &catalog);
        book.tick(&mut w.u);
        if s != Status::Running {
            assert_eq!(s, Status::Done, "{:?}", run.log);
            break;
        }
        assert!(w.u.ut() - start < 12.0 * 3600.0, "{:?}", run.log);
        w.u.set_warp(run.suggested_warp(&w.u));
        w.u.step();
        w.u.dock_latched(&defs, &res, &spec_for);
    }
    w.u.step();
    book.tick(&mut w.u);
    assert_eq!(book.provings[0].recorder.phase(), Phase::Docked);
    assert_eq!(
        book.provings[0].depot,
        Some(dest_id),
        "docked at the other one"
    );
    let joined = book.provings[0].recorder.vessel();
    w.u.transfer_resource(
        joined,
        "MonoPropellant",
        &[index(&w, joined, "iss-unity-service-tank")],
        &[index(&w, joined, "iss-zvezda-service-tank")],
    )
    .expect("transfer");
    for _ in 0..(60 * 50) {
        w.u.set_warp(1);
        w.u.step();
        book.tick(&mut w.u);
    }
    assert_eq!(book.finish(&mut w.u, 1), Ok(proving));
    let r = book.provings[0].recorder.recording.clone();
    assert_eq!(r.state, State::OneWay);
    assert_eq!(r.origin.as_deref(), Some(m.zarya.name.as_str()));
    let mono = r.delivered.get("MonoPropellant").copied().unwrap_or(0.0);
    assert!((mono - 1.5).abs() < 0.05, "{:?}", r.delivered);
    println!("used {:?}", Routes::used(&book.provings[0]));

    // The destination burns its monopropellant (the player takes it back into the shuttle), so there is
    // room for the next trip.
    w.u.transfer_resource(
        joined,
        "MonoPropellant",
        &[index(&w, joined, "iss-zvezda-service-tank")],
        &[index(&w, joined, "iss-unity-service-tank")],
    )
    .expect("take it back");
    for _ in 0..(30 * 50) {
        w.u.set_warp(1);
        w.u.step();
    }
    w.u.transfer_resource(joined, "MonoPropellant", &[], &[])
        .unwrap();
    let dv = book.depots[1].vessel().unwrap();
    let dest_mono = |w: &World, book: &Routes| {
        let dv = book.depots[1].vessel().unwrap();
        let c = w.u.current_craft(dv).unwrap();
        c.parts
            .iter()
            .filter(|p| p.guid == "iss-zvezda-service-tank")
            .flat_map(|p| p.resources.iter())
            .map(|r| r.stored_units)
            .sum::<f64>()
    };
    let before = dest_mono(&w, &book);
    assert!(before < 3.5, "room again: {before}");
    let _ = dv;

    let id = book.create_route(1, proving, "").expect("a shuttle");
    assert_eq!(book.shuttles.len(), 1);
    assert!(book.routes.is_empty());
    let mut pilots = Pilots::default();
    let mut restarted = false;
    let start = w.u.ut();
    while book.shuttles[0].trips.is_empty() {
        book.tick(&mut w.u);
        book.fly(&mut w.u, &mut pilots, &catalog);
        let s = &book.shuttles[0];
        assert!(s.paused.is_none(), "{} {:?}", s.status, s.trips);
        if !restarted
            && std::env::var("NORESTART").is_err()
            && s.leg == ShuttleLeg::Outbound
            && pilots.vessel(id).is_some()
        {
            // the server restarts while the filled-up shuttle is on its way
            restarted = true;
            println!("restart: {}", s.status);
            book = serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
            book.universe_replaced();
            pilots = Pilots::default();
            book.tick(&mut w.u);
            continue;
        }
        assert!(
            w.u.ut() - start < 2.0 * 86_400.0,
            "{}",
            book.shuttles[0].status
        );
        w.u.set_warp(pilots.suggested_warp(&w.u).unwrap_or(1));
        w.u.step();
        w.u.dock_latched(&defs, &res, &spec_for);
    }
    let s = &book.shuttles[0];
    println!("{:?} {}", s.trips, s.status);
    assert!(restarted, "it was restarted on the way");
    assert_eq!(s.trips[0].outcome, Outcome::Delivered, "{:?}", s.trips);
    let got = s.trips[0]
        .delivered
        .get("MonoPropellant")
        .copied()
        .unwrap_or(0.0);
    assert!((got - mono).abs() < 0.05 * mono, "{got} vs {mono}");
    assert!(
        s.origin_port.is_some(),
        "it docked at the origin on the way"
    );
    assert!(dest_mono(&w, &book) > before + 1.4);
}

#[test]
fn a_recording_in_orbit_starts_only_next_to_a_depot() {
    let m = modules();
    let mut w = world();
    let station = in_orbit(&mut w, &m.zarya, 100_000.0, 0.0);
    let near = in_orbit(&mut w, &m.unity, 100_000.0, -0.05);
    let far = in_orbit(&mut w, &m.zvezda, 100_000.0, 10.0);
    let mut book = Routes::default();
    // no depot yet: only the pad will do
    assert!(book.start(&w.u, near, 1).is_err());
    assert!(book.set_depot(&w.u, station, 1, true).unwrap());
    book.tick(&mut w.u);
    let err = book.start(&w.u, far, 1).unwrap_err();
    assert!(err.contains("next to the depot"), "{err}");
    book.start(&w.u, near, 1).expect("600 m from the depot");
    assert_eq!(book.provings[0].origin, Some(book.depots[0].id));
    assert_eq!(
        book.provings[0].recorder.recording.origin.as_deref(),
        Some(m.zarya.name.as_str())
    );
}

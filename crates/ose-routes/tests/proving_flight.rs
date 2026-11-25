//! Delivery routes, steps 1 and 2: record a proving flight flown by the pilot, then run the route. A tanker
//! launches from the KSC pad, docks at a depot in low orbit, pumps monopropellant into the depot's empty
//! tank, and the player ends the recording there (a one-way route). A route made from it launches a fresh
//! tanker at the next window, which docks and pumps the same again.
//!
//! Both crafts name their parts as the editor does (`p0`, `p1`, ...), so the names repeat between them and
//! docking renames the repeats, and the routes book is saved and loaded again on the way.
use ose_data::craft::Craft;
use ose_pilot::{
    ascent::AscentParams,
    mission::{Catalog, Leg, Mission, MissionParams},
    Status,
};
use ose_routes::recording::{Event, Phase, Recording, State};
use ose_routes::route::{Outcome, Pilots, StayedCommand, WhenDelivered};
use ose_routes::Routes;

#[path = "../../ose-pilot/tests/common/mod.rs"]
mod common;
use common::{in_orbit, modules, on_pad, spec, world, World};

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
fn records_a_one_way_fuel_run_to_a_depot_and_runs_it_as_a_route() {
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

    let proving = book.start(&w.u, tanker, 1).expect("on the pad");
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
        book.tick(&mut w.u);
        if !saved && book.provings[0].recorder.phase() == Phase::Transfer {
            // a save and a restart on the way: the recorder and the depot find their vessels again
            saved = true;
            book = serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
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
    book.tick(&mut w.u);
    let rec = &book.provings[0].recorder;
    assert_eq!(rec.phase(), Phase::Docked, "{:?}", rec.recording.markers);
    assert_eq!(book.provings[0].depot, Some(book.depots[0].id));
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
        book.tick(&mut w.u);
    }
    assert_eq!(book.finish(&mut w.u, 1), Ok(proving));
    let r: &Recording = &book.provings[0].recorder.recording.clone();

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

    // Step 2: a route from it. The player takes the monopropellant back and sends the tanker away, which
    // frees the port and leaves the depot's tank with room again.
    let route = book
        .create_route(1, proving, "")
        .expect("a one-way recording");
    // Each run flies its own rendezvous, and what its approach burns varies by a few percent of the
    // monopropellant aboard (0.16 to 0.28 units of about 4 between runs that differ only by a fraction of a
    // millimetre in how they first stand on the pad). A run that falls short pumps its tank dry and cannot back
    // away from the station again, so the runs here carry 95% of what the proving flight delivered: their
    // outcome and what they do next stay the same whichever way that spread falls.
    let cargo = 0.95 * mono;
    book.routes[0].cargo.insert("MonoPropellant".into(), cargo);
    let craft = w.u.current_craft(joined).unwrap();
    let index = |guid: &str| craft.parts.iter().position(|p| p.guid == guid).unwrap();
    w.u.transfer_resource(
        joined,
        "MonoPropellant",
        &[index("iss-zarya-service-tank")],
        &[index("iss-unity-service-tank")],
    )
    .expect("transfer back");
    for _ in 0..(60 * 50) {
        w.u.set_warp(1);
        w.u.step();
    }
    w.u.transfer_resource(joined, "MonoPropellant", &[], &[])
        .unwrap();
    w.u.set_warp(1);
    w.u.wake(joined);
    let other =
        w.u.undock(
            joined,
            Some(index("iss-zarya-forward")),
            &defs,
            &res,
            &spec_for,
        )
        .expect("undock");
    book.tick(&mut w.u);
    let depot_now = book.depots[0].vessel().unwrap();
    let away = if depot_now == other { joined } else { other };
    w.u.despawn_vessel(away);

    let mut pilots = Pilots::default();
    // the server restarts while the run is in orbit: the pilot is made again from the saved route
    let in_orbit = |p: &Pilots| {
        matches!(
            p.leg(route),
            Some(Leg::Plan | Leg::Transfer | Leg::SoiTransfer | Leg::Brake)
        )
    };
    drive_until(&mut w, &mut book, &mut pilots, &catalog, &|b, p| {
        !b.routes[0].runs.is_empty() || in_orbit(p)
    });
    assert!(book.routes[0].runs.is_empty(), "{:?}", book.routes[0].runs);
    restart(&mut w, &mut book, &mut pilots);
    book.fly(&mut w.u, &mut pilots, &catalog);
    assert!(book.routes[0].active.is_some(), "{:?}", book.routes[0].runs);
    assert!(
        in_orbit(&pilots) || pilots.leg(route) == Some(Leg::Approach),
        "{:?}",
        pilots.leg(route)
    );
    // a player takes control for a minute and hands it back: the pilot picks it up from there
    let run = pilots.vessel(route).expect("run vessel");
    pilots.set_manual([(run, 7)].into_iter().collect());
    for _ in 0..(60 * 50) {
        book.tick(&mut w.u);
        book.fly(&mut w.u, &mut pilots, &catalog);
        w.u.set_warp(1);
        w.u.step();
    }
    assert_eq!(book.routes[0].active.as_ref().unwrap().manual, Some(7));
    assert!(
        book.routes[0].status.contains("by hand"),
        "{}",
        book.routes[0].status
    );
    assert_eq!(
        pilots.suggested_warp(&w.u),
        None,
        "the player warps their own run"
    );
    pilots.set_manual(Default::default());
    drive(&mut w, &mut book, &mut pilots, &catalog, &|b| {
        !b.routes[0].runs.is_empty()
    });
    assert_eq!(book.routes[0].runs[0].flown_by, vec![7]);
    let r0 = &book.routes[0];
    println!("{:?} {}", r0.runs, r0.status);
    assert_eq!(r0.runs[0].outcome, Outcome::Delivered, "{:?}", r0.runs);
    let got = r0.runs[0]
        .delivered
        .get("MonoPropellant")
        .copied()
        .unwrap_or(0.0);
    assert!((got - cargo).abs() < 0.02 * cargo, "{got} vs {cargo}");
    // Stay: the run stays docked on the recorded port, so the next one waits for it
    book.set_when_delivered(route, WhenDelivered::Stay).unwrap();
    book.tick(&mut w.u);
    book.schedule(&mut w.u, &pilots);
    assert!(
        book.routes[0].status.contains("no free port"),
        "{}",
        book.routes[0].status
    );

    // Rotate: the next run launches, and once it is in orbit the first one undocks and heads home. The
    // player makes room in the depot's tank first, into the docked run's.
    book.set_when_delivered(route, WhenDelivered::Rotate)
        .unwrap();
    let dv = book.depots[0].vessel().unwrap();
    let craft = w.u.current_craft(dv).unwrap();
    let tank = |guid: &str| {
        craft
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.guid.starts_with(guid) || p.guid == guid)
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    };
    let depot_tank: Vec<usize> = tank("iss-zarya-service-tank");
    let run_tank: Vec<usize> = craft
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            !book.depots[0].parts.contains(&p.guid)
                && p.resources.iter().any(|r| r.resource == "MonoPropellant")
        })
        .map(|(i, _)| i)
        .collect();
    w.u.wake(dv);
    w.u.transfer_resource(dv, "MonoPropellant", &depot_tank, &run_tank)
        .expect("make room");
    for _ in 0..(60 * 50) {
        w.u.set_warp(1);
        w.u.step();
    }
    w.u.transfer_resource(dv, "MonoPropellant", &[], &[])
        .unwrap();
    let after = |b: &Routes| b.routes[0].runs[0].after.clone().unwrap_or_default();
    // and again while the first run flies home
    drive(&mut w, &mut book, &mut pilots, &catalog, &|b| {
        !["docked", "undocking to fly home"].contains(&after(b).as_str())
    });
    assert_eq!(after(&book), "flying home");
    assert_eq!(book.routes[0].runs[0].leaving, Some(true));
    restart(&mut w, &mut book, &mut pilots);
    drive(&mut w, &mut book, &mut pilots, &catalog, &|b| {
        b.routes[0].runs.len() == 2
            && !["docked", "undocking to fly home", "flying home"].contains(&after(b).as_str())
    });
    let r0 = &book.routes[0];
    println!("{:?} {}", r0.runs, r0.status);
    assert!(
        matches!(r0.runs[1].outcome, Outcome::Delivered),
        "{:?}",
        r0.runs
    );
    assert_ne!(after(&book), "docked", "the first run made way");
    assert_eq!(book.routes[0].stayed, Some(2));

    // cleaning up by hand: let the second run go
    book.stayed_command(&w.u, &mut pilots, route, StayedCommand::LetGo)
        .unwrap();
    assert_eq!(book.routes[0].stayed, None);
    book.pause_route(route, true).unwrap();
    let after2 = |b: &Routes| b.routes[0].runs[1].after.clone().unwrap_or_default();
    drive(&mut w, &mut book, &mut pilots, &catalog, &|b| {
        after2(b) != "undocking"
    });
    assert_eq!(after2(&book), "let go", "{:?}", book.routes[0].runs);
    assert_eq!(book.routes[0].runs[1].leaving, None);
}

/// What a server restart does to the routes: the book comes back from the save, every vessel is found
/// again by its parts or name, and the pilots start empty.
fn restart(w: &mut World, book: &mut Routes, pilots: &mut Pilots) {
    *book = serde_json::from_str(&serde_json::to_string(&*book).unwrap()).unwrap();
    book.universe_replaced();
    *pilots = Pilots::default();
    book.tick(&mut w.u);
}

/// Run the routes, launching what they ask for, until `done`.
fn drive(
    w: &mut World,
    book: &mut Routes,
    pilots: &mut Pilots,
    catalog: &Catalog,
    done: &dyn Fn(&Routes) -> bool,
) {
    drive_until(w, book, pilots, catalog, &|b, _| done(b));
}

fn drive_until(
    w: &mut World,
    book: &mut Routes,
    pilots: &mut Pilots,
    catalog: &Catalog,
    done: &dyn Fn(&Routes, &Pilots) -> bool,
) {
    let start = w.u.ut();
    let mut ticks = 0u64;
    while !done(book, pilots) {
        book.tick(&mut w.u);
        if ticks.is_multiple_of(50) {
            for req in book.schedule(&mut w.u, pilots) {
                let e = on_pad(w, &req.craft);
                w.u.rename_vessel(e, &req.vessel_name);
                book.launched(&w.u, pilots, req.route, e, &req.vessel_name)
                    .unwrap();
            }
        }
        book.fly(&mut w.u, pilots, catalog);
        assert!(
            w.u.ut() - start < 3.0 * 86_400.0,
            "{} {:?}",
            book.routes[0].status,
            book.routes[0].runs
        );
        let warp = match pilots.suggested_warp(&w.u) {
            Some(x) => x,
            None => match book.routes[0].next_launch {
                Some(t) if t - w.u.ut() > 120.0 => 50,
                _ => 1,
            },
        };
        w.u.set_warp(warp);
        w.u.step();
        w.u.dock_latched(&w.defs, &w.resources, catalog.spec);
        ticks += 1;
    }
}

//! Delivery routes to colonies (`docs/colonies-design.md`, step 2). A proving flight that ends landed at a
//! colony on the Mun makes a one-way colony route. Its run flies across to the Mun and lands within the
//! colony, where it stays and its tanks become the colony's stock.
//!
//! The run is put in low Kerbin orbit rather than on the pad (the climb is the ascent pilot's, tested with
//! the missions), and the server restarts at once, so the pilot is made again from the saved route: in
//! orbit, it flies the trip from there.
use ose_data::craft::Craft;
use ose_pilot::mission::Catalog;
use ose_routes::recording::State;
use ose_routes::route::{ColonySite, Endpoint, Outcome, Pilots, WhenDelivered};
use ose_routes::Routes;

#[path = "../../ose-pilot/tests/common/mod.rs"]
mod common;
use common::{in_orbit, on_surface, spec, world};

/// The Lesson 3.2 rocket (pod, tank, Terrier) with its tanks full.
fn lander() -> Craft {
    let all: Vec<Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let mut c = all
        .into_iter()
        .find(|c| c.name == "Lesson 3.2 rocket")
        .unwrap();
    for p in &mut c.parts {
        for r in &mut p.resources {
            r.stored_units = r.capacity_units;
        }
    }
    c
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn a_route_to_a_colony_on_the_mun_lands_its_runs_there() {
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    let colony = ColonySite {
        id: 7,
        name: "Mun Base".into(),
        body: "Mun".into(),
        lat_deg: 5.0,
        lon_deg: 60.0,
    };

    // The proving flight, as it ends: landed in the colony.
    let first = on_surface(&mut w, mun, &lander(), 5.0005, 60.0);
    let mut book = Routes::default();
    let proving = book.start(&w.u, first, 1).expect("landed");
    for _ in 0..200 {
        w.u.step();
        book.tick(&mut w.u);
    }
    assert_eq!(
        book.finish_at_colony(&mut w.u, 1, colony.clone()),
        Ok(proving)
    );
    let rec = &book.provings[0].recorder.recording;
    assert_eq!(rec.state, State::Landed);
    assert_eq!(rec.depot.as_deref(), Some("Mun Base"));
    assert!(rec.delivered.get("Oxidizer").copied().unwrap_or(0.0) > 0.1);
    assert!(!rec.delivered.contains_key("ElectricCharge"));

    let route = book
        .create_route(1, proving, "")
        .expect("a colony recording");
    let r = &book.routes[0];
    assert_eq!(r.endpoint, Endpoint::Colony(colony.clone()));
    assert_eq!(r.when_delivered, WhenDelivered::Stay);
    assert!(book
        .set_when_delivered(route, WhenDelivered::Rotate)
        .is_err());

    // The scheduler asks for a launch after the rollout; the run goes up.
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| spec(c, &defs, &res);
    let catalog = Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    let mut pilots = Pilots::default();
    book.fly(&mut w.u, &mut pilots, &catalog);
    let req = loop {
        if let Some(req) = book.schedule(&mut w.u, &pilots).pop() {
            break req;
        }
        w.u.set_warp(50);
        w.u.step();
    };
    let run = in_orbit(&mut w, &req.craft, 100_000.0, 0.0);
    w.u.rename_vessel(run, &req.vessel_name);
    book.launched(&w.u, &mut pilots, route, run, &req.vessel_name)
        .unwrap();
    assert!(book.routes[0].active.is_some());

    // The server restarts with the run in orbit: the pilot is made again from the saved route.
    book = serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
    book.universe_replaced();
    let mut pilots = Pilots::default();
    let start = w.u.ut();
    while book.routes[0].runs.is_empty() {
        book.tick(&mut w.u);
        book.fly(&mut w.u, &mut pilots, &catalog);
        assert!(
            w.u.ut() - start < 3.0 * 86_400.0,
            "{}",
            book.routes[0].status
        );
        w.u.set_warp(pilots.suggested_warp(&w.u).unwrap_or(1));
        w.u.step();
    }
    let r0 = &book.routes[0];
    println!("{:?} {}", r0.runs, r0.status);
    let x = &r0.runs[0];
    // it burned some of the cargo on the way: short, but landed
    assert!(
        matches!(x.outcome, Outcome::Delivered | Outcome::Short),
        "{:?}",
        x.outcome
    );
    assert_eq!(x.after.as_deref(), Some("landed at the colony"));
    assert!(x.delivered.get("Oxidizer").copied().unwrap_or(0.0) > 0.01);
    assert!(r0.active.is_none());
    let lander = w.u.powered(run).or_else(|| w.u.dormant(run)).unwrap();
    assert!(lander.landed && !lander.crashed);

    // Abandoning the colony pauses its routes.
    book.colony_abandoned(7);
    assert!(book.routes[0].paused.is_some());
}

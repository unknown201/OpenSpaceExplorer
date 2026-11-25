//! Orbital colonies (`docs/colonies-design.md`, step 7): a crewed station with a colony trunk in a stable
//! orbit founds a colony that goes where the station goes. The vessels within 300 m of it in orbit are its
//! stock and residents, and its dock, where new craft start out, is 400 m behind it on its orbit.
use ose_colonies::{dock, members, residents, stock, Colonies, PAD_DISTANCE_M, RADIUS_M};
use ose_data::craft::Craft;

#[path = "../../ose-pilot/tests/common/mod.rs"]
mod common;
use common::{in_orbit, modules, renamed, world};

/// The Unity module with a small colony trunk hung on its side, holding `metal` units.
fn with_trunk(c: &Craft, metal: f64) -> Craft {
    let mut c = c.clone();
    let mut trunk = c
        .parts
        .iter()
        .find(|p| p.guid == "iss-unity-battery")
        .unwrap()
        .clone();
    trunk.guid = "trunk".into();
    trunk.part_id = "transport_3v_colony_trunk_small".into();
    trunk.resources = vec![ose_data::craft::StoredResource {
        resource: "Metal".into(),
        stored_units: metal,
        capacity_units: 10_000.0,
    }];
    c.parts.push(trunk);
    c
}

fn crew(w: &mut common::World, e: bevy_ecs::entity::Entity, names: &[&str]) {
    let seats = w.u.crew_seats(e);
    w.u.set_crew(
        e,
        seats
            .iter()
            .zip(names)
            .map(|(&s, n)| (s, n.to_string()))
            .collect(),
    );
}

#[test]
fn a_station_in_orbit_founds_a_colony_that_moves_with_it() {
    let m = modules();
    let mut w = world();
    let r: f64 = 600_000.0 + 100_000.0;
    // one metre along the orbit is this many degrees of phase
    let deg_per_m = (1.0 / r).to_degrees();

    // in the air is no orbit for a colony
    let low = in_orbit(&mut w, &with_trunk(&m.unity, 1_000.0), 50_000.0, 90.0);
    crew(&mut w, low, &["Valentina Kerman"]);
    let mut book = Colonies::default();
    let err = book.found(&mut w.u, low, 7, "Too low").unwrap_err();
    assert!(err.contains("stable orbit"), "{err}");
    w.u.despawn_vessel(low);

    let station = in_orbit(&mut w, &with_trunk(&m.unity, 5_000.0), 100_000.0, 0.0);
    crew(&mut w, station, &["Jebediah Kerman", "Bill Kerman"]);
    // a tanker 100 m ahead belongs to it, one 12 km ahead does not
    let tanker = in_orbit(
        &mut w,
        &renamed(&m.unity, "t"),
        100_000.0,
        100.0 * deg_per_m,
    );
    crew(&mut w, tanker, &["Bob Kerman"]);
    let far = in_orbit(
        &mut w,
        &renamed(&m.unity, "f"),
        100_000.0,
        12_000.0 * deg_per_m,
    );
    for _ in 0..10 {
        w.u.step();
    }
    let id = book
        .found(&mut w.u, station, 7, "Gateway")
        .expect("founded");
    let colony = book.get(id).unwrap().clone();
    assert!(colony.orbital());
    assert_eq!(colony.body, "Kerbin");

    let at = members(&mut w.u, &colony);
    assert_eq!(at.first(), Some(&station), "the anchor comes first");
    assert!(at.contains(&tanker) && !at.contains(&far), "{at:?}");
    let held = stock(&mut w.u, &colony);
    let metal = held.iter().find(|s| s.resource == "Metal").unwrap().stored;
    assert!((metal - 5_000.0).abs() < 1e-6);
    let mono = held
        .iter()
        .find(|s| s.resource == "MonoPropellant")
        .unwrap()
        .stored;
    assert!((mono - 9.0).abs() < 1e-6, "two service tanks: {mono}");
    assert_eq!(residents(&mut w.u, &colony).len(), 3);

    // the dock: 400 m behind the station, moving with it, outside the colony
    let (around, dock_r, dock_v) = dock(&mut w.u, &colony).expect("in orbit");
    let (b, sr, sv, _) = ose_colonies::place(&w.u, station).unwrap();
    assert_eq!(around, b);
    assert!(((dock_r - sr).length() - PAD_DISTANCE_M).abs() < 1e-6);
    assert!((dock_r - sr).dot(sv) < 0.0, "behind");
    assert!((dock_v - sv).length() < 1e-9);
    const { assert!(PAD_DISTANCE_M > RADIUS_M) };

    // a second colony next to it, or on the same trunk, is refused
    let mut neighbour = renamed(&with_trunk(&m.unity, 0.0), "n");
    neighbour.name = "Neighbour".into();
    let next = in_orbit(&mut w, &neighbour, 100_000.0, -1_000.0 * deg_per_m);
    crew(&mut w, next, &["Gene Kerman"]);
    w.u.step();
    let err = book.found(&mut w.u, next, 7, "Neighbour").unwrap_err();
    assert!(err.contains("too close"), "{err}");
    let err = book.found(&mut w.u, station, 7, "Again").unwrap_err();
    assert!(err.contains("already anchors"), "{err}");

    // the colony goes where the station goes: an hour on, its members are still the same two
    for _ in 0..200 {
        w.u.set_warp(100);
        w.u.step();
    }
    book.track(&mut w.u);
    let colony = book.get(id).unwrap().clone();
    let at = members(&mut w.u, &colony);
    assert_eq!(at.first(), Some(&station));
    assert!(at.contains(&tanker), "{at:?}");

    // a copy of the station launched elsewhere carries the same trunk guid; the colony stays with its own
    let twin = in_orbit(&mut w, &with_trunk(&m.unity, 0.0), 100_000.0, 180.0);
    w.u.step();
    book.track(&mut w.u);
    let colony = book.get(id).unwrap().clone();
    assert_eq!(
        ose_colonies::anchor_vessel(&mut w.u, &colony),
        Some(station)
    );
    assert_ne!(twin, station);

    // the station gone, the colony has no vessels and no dock
    w.u.despawn_vessel(station);
    w.u.despawn_vessel(twin);
    assert!(members(&mut w.u, &colony).is_empty());
    assert!(dock(&mut w.u, &colony).is_none());
}

//! Colonies, step 1: a crewed lander with a colony trunk founds a colony on the Mun; a tanker landing
//! beside it adds to the stock; a craft launched from the colony's pad is paid for out of the colony's
//! tanks and crewed by the colony's Kerbals.
use ose_colonies::{bill, board, draw, members, pad, residents, stock, surface_distance, Colonies};
use ose_data::craft::Craft;

#[path = "../../ose-pilot/tests/common/mod.rs"]
mod common;
use common::{modules, on_surface, world};

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

fn held(s: &[ose_colonies::Stock], res: &str) -> f64 {
    s.iter()
        .find(|x| x.resource == res)
        .map(|x| x.stored)
        .unwrap_or(0.0)
}

#[test]
fn a_colony_on_the_mun_stocks_fuel_and_launches_from_its_pad() {
    let m = modules();
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    // enough Metal for two Unity launches and a half
    let dry_t = m.unity.mass(&w.defs, &w.resources).dry_t;
    let base = on_surface(
        &mut w,
        mun,
        &with_trunk(&m.unity, 2_500.0 * dry_t),
        1.0,
        20.0,
    );
    for _ in 0..50 {
        w.u.step();
    }
    let mut book = Colonies::default();

    // nobody aboard: no colony
    let crew = w.u.crew(base);
    w.u.set_crew(base, Vec::new());
    let err = book.found(&mut w.u, base, 7, "Armstrong").unwrap_err();
    assert!(err.contains("Kerbal"), "{err}");
    let seats = w.u.crew_seats(base);
    let crew = if crew.is_empty() {
        vec![
            (seats[0], "Jebediah Kerman".to_string()),
            (seats[1], "Bill Kerman".to_string()),
        ]
    } else {
        crew
    };
    w.u.set_crew(base, crew);
    let id = book.found(&mut w.u, base, 7, "Armstrong").expect("founded");
    assert_eq!(book.get(id).unwrap().body, "Mun");
    // a second colony next door is refused, and so is a vessel without a trunk
    let plain = on_surface(&mut w, mun, &m.unity, 1.0, 20.3);
    for _ in 0..20 {
        w.u.step();
    }
    assert!(book
        .found(&mut w.u, plain, 7, "Other")
        .unwrap_err()
        .contains("trunk"));
    let colony = book.get(id).unwrap().clone();

    // a tanker landed 100 m away belongs to the colony; the vessel 9 km away does not
    let r = 200_000.0;
    let (tlat, tlon) = ose_colonies::travel(r, (colony.lat_deg, colony.lon_deg), 0.0, 100.0);
    let tanker = on_surface(&mut w, mun, &m.unity, tlat, tlon);
    for _ in 0..20 {
        w.u.step();
    }
    let at = members(&mut w.u, &colony);
    assert!(
        at.contains(&base) && at.contains(&tanker) && !at.contains(&plain),
        "{at:?}"
    );
    let before = stock(&mut w.u, &colony);
    let mono = held(&before, "MonoPropellant");
    assert!((mono - 9.0).abs() < 1e-6, "two service tanks: {mono}");
    assert_eq!(
        residents(&mut w.u, &colony).len(),
        w.u.crew(base).len() + w.u.crew(tanker).len()
    );

    // the pad stands outside the colony
    let (plat, plon) = pad(&w.u, &colony).unwrap();
    let d = surface_distance(r, (colony.lat_deg, colony.lon_deg), (plat, plon));
    assert!(d > ose_colonies::RADIUS_M, "{d}");

    // launching a Unity from the pad costs 4.5 monopropellant and its dry mass in Metal, not its charge
    let bill = bill(&m.unity, dry_t);
    assert_eq!(bill.len(), 2, "{bill:?}");
    assert!((bill["Metal"] - 1000.0 * dry_t).abs() < 1e-6);
    let metal = held(&before, "Metal");
    draw(&mut w.u, &colony, &bill).expect("paid");
    let after = stock(&mut w.u, &colony);
    assert!((held(&after, "MonoPropellant") - (mono - 4.5)).abs() < 1e-6);
    assert!((held(&after, "Metal") - (metal - 1000.0 * dry_t)).abs() < 1e-6);
    assert!((held(&after, "ElectricCharge") - held(&before, "ElectricCharge")).abs() < 1e-6);
    let rocket = on_surface(&mut w, mun, &m.unity, plat, plon);
    w.u.set_crew(rocket, Vec::new());
    let living = residents(&mut w.u, &colony).len();
    let names = board(
        &mut w.u,
        &colony,
        rocket,
        Some(&["Bill Kerman".to_string()]),
    )
    .unwrap();
    assert_eq!(names, vec!["Bill Kerman".to_string()]);
    assert!(w.u.crew(base).iter().all(|(_, n)| n != "Bill Kerman"));
    assert_eq!(w.u.crew(rocket).len(), 1);
    // the rocket on the pad is not part of the colony: Bill has left it, and its tank is not stock
    assert_eq!(residents(&mut w.u, &colony).len(), living - 1);
    assert!(!members(&mut w.u, &colony).contains(&rocket));
    // a stranger cannot be picked
    let other = on_surface(&mut w, mun, &m.unity, plat, plon);
    assert!(board(
        &mut w.u,
        &colony,
        other,
        Some(&["Nobody Kerman".to_string()])
    )
    .is_err());

    // once more drains the colony dry and is then refused, taking nothing
    draw(&mut w.u, &colony, &bill).expect("second");
    let short = draw(&mut w.u, &colony, &bill).unwrap_err();
    assert!(
        short.contains("MonoPropellant") && short.contains("Metal"),
        "{short}"
    );

    // the book saves and loads
    let again: Colonies = serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
    let (a, b) = (&again.colonies[0], &book.colonies[0]);
    assert_eq!(
        (a.id, &a.name, &a.body, a.owner),
        (b.id, &b.name, &b.body, b.owner)
    );
    assert!((a.lat_deg - b.lat_deg).abs() < 1e-9 && (a.lon_deg - b.lon_deg).abs() < 1e-9);
    // the next colony takes a new id after a load
    let mut again = again;
    let far = on_surface(&mut w, mun, &with_trunk(&m.unity, 0.0), -30.0, 100.0);
    for _ in 0..20 {
        w.u.step();
    }
    let next = again
        .found(&mut w.u, far, 7, "Aldrin")
        .expect("second colony");
    assert_ne!(next, id);
    assert!(book.clone().abandon(id, 8).is_err());
    let mut b2 = book.clone();
    b2.abandon(id, 7).unwrap();
    assert!(b2.colonies.is_empty());
}

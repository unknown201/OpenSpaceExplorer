//! Colonies, routes to colonies: a powered landing at a site on the Mun from low orbit. The pilot lines
//! the orbit up with the site, lowers periapsis over it, brakes under closed-loop guidance and settles
//! within a colony's radius (300 m) of the site.
use ose_data::craft::Craft;
use ose_pilot::{
    landing::{Landing, LandingParams},
    Status,
};

mod common;
use common::{in_orbit_inclined, world};

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
fn lands_within_a_colony_radius_of_a_site_on_the_mun() {
    // (orbit inclination, site): a site just off the plane, and one well south and round the far side
    for (incl, lat, lon) in [(6.0, 3.0, 40.0), (15.0, -12.0, -150.0)] {
        land(incl, lat, lon);
    }
}

fn land(incl: f64, lat: f64, lon: f64) {
    let mut w = world();
    let mun = w.u.body_entity("Mun").unwrap();
    let e = in_orbit_inclined(&mut w, mun, &lander(), 25_000.0, 0.0, incl);
    let mut landing = Landing::new(LandingParams::at(lat, lon));
    let start = w.u.ut();
    let status = loop {
        let s = landing.update(&mut w.u, e);
        if s != Status::Running {
            break s;
        }
        assert!(w.u.ut() - start < 6.0 * 3600.0, "{:?}", landing.log);
        w.u.set_warp(landing.suggested_warp(&w.u, e));
        w.u.step();
    };
    for line in &landing.log {
        println!("{line}");
    }
    assert_eq!(status, Status::Done, "{:?}", landing.log);
    let miss = landing.miss_m.unwrap();
    assert!(miss < 300.0, "landed {miss:.0} m from the site");
    assert!(!w.u.powered(e).unwrap().crashed);
}

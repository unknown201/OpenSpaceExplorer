//! Rendezvous v2 scenario T8 (R6): a lifeboat docked at an LKO station undocks, backs off, deorbits
//! toward KSC, and comes down under parachutes, with ordinary controls. A first descent of the same
//! lifeboat (its proving flight) measures where the atmosphere stops it; the run then aims with that.
use ose_data::craft::Craft;
use ose_pilot::{
    depart::Depart,
    descent::{arc_deg, Descent, DescentParams},
    mission::{Catalog, Mission, MissionParams},
    Status,
};

mod common;
use common::{fly, in_orbit, modules, spec, world};

/// The Zvezda module with a heat shield in place of its aft docking port (it docks by the forward
/// one) and four radial parachutes on its body. Its heavy service tank is at the aft end too, so it
/// flies shield first, held surface-retrograde.
fn lifeboat(zvezda: &Craft) -> Craft {
    let mut c = zvezda.clone();
    c.name = "Lifeboat".into();
    let aft = c
        .parts
        .iter_mut()
        .find(|p| p.guid == "iss-zvezda-aft")
        .unwrap();
    aft.guid = "lifeboat-heatshield".into();
    aft.part_id = "heatshield_2v".into();
    aft.own_node = Some("top".into());
    // upright, its top node (0.065 m) under the service tank's bottom (y -2.350 m)
    aft.local_position = ose_math::DVec3::new(0.0, -2.415, 0.0);
    aft.local_rotation = ose_data::craft::Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
    aft.resources = vec![ose_data::craft::StoredResource {
        resource: "Ablator".into(),
        stored_units: 0.8,
        capacity_units: 0.8,
    }];
    let template = c
        .parts
        .iter()
        .find(|p| p.guid == "iss-zvezda-rcs-0")
        .unwrap()
        .clone();
    for (i, (x, z)) in [(1.3, 0.0), (-1.3, 0.0), (0.0, 1.3), (0.0, -1.3)]
        .iter()
        .enumerate()
    {
        let mut p = template.clone();
        p.guid = format!("lifeboat-chute-{i}");
        p.part_id = "parachute_1v_radial".into();
        p.local_position = ose_math::DVec3::new(*x, -0.9, *z);
        p.resources.clear();
        c.parts.push(p);
    }
    c
}

/// Dock a fresh lifeboat at a station, undock it, and bring it home. Returns the descent.
fn come_home(params: DescentParams) -> Descent {
    let m = modules();
    let mut w = world();
    let station = in_orbit(&mut w, &m.zarya, 100_000.0, 0.0);
    let boat = in_orbit(&mut w, &lifeboat(&m.zvezda), 100_000.0, -0.05);
    let mut dock = Mission::new(
        boat,
        MissionParams::new(station, "iss-zarya-forward", "iss-zvezda-forward"),
    );
    assert_eq!(
        fly(&mut w, std::slice::from_mut(&mut dock), 3_600.0)[0],
        Status::Done
    );
    let docked = dock.vessel();

    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| spec(c, &defs, &res);
    let catalog = Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    let mut depart = Depart::new("iss-zvezda-forward");
    loop {
        match depart.update(&mut w.u, docked, &catalog) {
            Status::Running => {}
            Status::Done => break,
            Status::Failed(why) => panic!("departure: {why}"),
        }
        w.u.set_warp(depart.suggested_warp());
        w.u.step();
    }
    let boat = depart.vessel.unwrap();
    let mut descent = Descent::new(params);
    let start = w.u.ut();
    let result = loop {
        let s = descent.update(&mut w.u, boat);
        if s != Status::Running {
            break s;
        }
        assert!(w.u.ut() - start < 6.0 * 3600.0, "{:?}", descent.log);
        w.u.set_warp(descent.suggested_warp(&w.u, boat));
        w.u.step();
    };
    for line in &descent.log {
        println!("{line}");
    }
    assert_eq!(result, Status::Done, "{:?}", descent.log);
    descent
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "a full flight takes minutes unoptimised; CI runs it with --release"
)]
fn t8_lifeboat_leaves_an_leo_station_and_lands_near_ksc() {
    let proving = come_home(DescentParams::ksc());
    let past = proving.flown_past_periapsis_deg().expect("measured");
    println!("proving descent came down {past:.2} deg past its vacuum periapsis");

    let run = come_home(DescentParams {
        past_periapsis_deg: past,
        ..DescentParams::ksc()
    });
    let at = run.landed_at.unwrap();
    let km = arc_deg(at, (-0.6836, -74.8198)).to_radians() * 600.0;
    assert!(km < 30.0, "came down {km:.0} km from KSC");
}

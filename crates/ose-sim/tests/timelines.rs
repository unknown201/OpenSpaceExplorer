use ose_sim::{Universe, Tick, flight::{Powered, FlightSpec}};

fn universe() -> Universe {
    let defs: Vec<ose_data::CelestialBody> = serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    Universe::with_bodies(&defs)
}

#[test]
fn docking_physics_does_not_limit_another_players_orbital_warp() {
    let mut u = universe();
    let body = u.body_entity("Kerbin").unwrap();
    let slow = u.spawn_vessel_in_circular_orbit("docking", body, 100_000.);
    let fast = u.spawn_vessel_in_circular_orbit("travelling", body, 200_000.);
    let (_, _, orbit) = u.vessel(slow).unwrap();
    let (r, v) = orbit.state_at(0.);
    u.set_powered(slow, Powered::new(FlightSpec { stages: vec![Default::default()], drag_area_m2: 0., drag_cd: 0. }, r, v, false));
    u.assign_timeline(slow, 1);
    u.assign_timeline(fast, 2);
    u.set_timeline_warp(1, 1);
    u.set_timeline_warp(2, 1000);
    for _ in 0..10 { u.step(); }
    assert_eq!(u.vessel_tick(slow), Tick(10));
    assert_eq!(u.vessel_tick(fast), Tick(10_000));
    assert_eq!(u.clock(2).warp, 1000);
    assert!(!u.same_time(slow, fast));
    assert!(u.distance_between(slow, fast).is_none());
    // Scheduling and waking use the traveller's time, not the shared clock.
    assert!(!u.schedule_burn(fast, ose_sim::vessels::ScheduledBurn {
        ut: 10., prograde_mps: 1., normal_mps: 0., radial_mps: 0.
    }));
}

#[test]
fn local_warp_stops_at_its_maneuver_without_advancing_the_other_clock() {
    let mut u = universe();
    let body = u.body_entity("Kerbin").unwrap();
    let slow = u.spawn_vessel_in_circular_orbit("slow", body, 100_000.);
    let fast = u.spawn_vessel_in_circular_orbit("fast", body, 200_000.);
    u.assign_timeline(slow, 1);
    u.assign_timeline(fast, 2);
    assert!(u.schedule_burn(fast, ose_sim::vessels::ScheduledBurn {
        ut: 1., prograde_mps: 10., normal_mps: 0., radial_mps: 0.
    }));
    u.set_timeline_warp(2, 1000);
    u.step();
    assert_eq!(u.vessel_tick(fast), Tick(ose_sim::TICKS_PER_SECOND));
    assert_eq!(u.vessel_tick(slow), Tick(1));
    assert!(u.scheduled_burn(fast).is_none());
}

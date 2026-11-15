use ose_sim::{
    flight::{FlightSpec, Powered},
    Tick, Universe,
};

fn universe() -> Universe {
    let defs: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
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
    u.set_powered(
        slow,
        Powered::new(
            FlightSpec {
                stages: vec![Default::default()],
                drag_area_m2: 0.,
                drag_cd: 0.,
            },
            r,
            v,
            false,
        ),
    );
    u.enable_timelines();
    let fast_time = u.split().unwrap();
    u.move_to_timeline(&[fast], fast_time);
    u.set_timeline_warp(0, 1);
    u.set_timeline_warp(fast_time, 1000);
    for _ in 0..10 {
        u.step();
    }
    assert_eq!(u.timeline_tick(0).unwrap(), Tick(10));
    assert_eq!(u.timeline_tick(fast_time).unwrap(), Tick(10_000));
    assert_eq!(u.timeline_clock(fast_time).unwrap().warp, 1000);
    assert_ne!(u.timeline_of(slow), u.timeline_of(fast));
    assert!(u.distance_between(slow, fast).is_none());
    u.enter(fast_time);
    // Scheduling uses the traveller's clock.
    assert!(!u.schedule_burn(
        fast,
        ose_sim::vessels::ScheduledBurn {
            ut: 10.,
            prograde_mps: 1.,
            normal_mps: 0.,
            radial_mps: 0.
        }
    ));
}

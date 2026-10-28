use ose_math::DVec3;
use ose_sim::{
    bodies::Body,
    dynamics::{Inertia, MassProperties},
    flight::{self, Attitude, FlightSpec, Powered, Propulsion, StageSpec},
    Universe,
};

fn kerbin() -> ose_data::CelestialBody {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut body = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    body.parent = None;
    body.orbit = None;
    body
}
fn spec(torque: f64) -> FlightSpec {
    FlightSpec {
        stages: vec![StageSpec {
            start_mass_t: 10.0,
            moi_tm2: 20.0,
            wheel_torque_knm: torque,
            ..Default::default()
        }],
        drag_area_m2: 0.0,
        drag_cd: 0.0,
    }
}
fn vacuum() -> Body {
    Body {
        id: "vacuum".into(),
        mu: 0.0,
        mass_kg: 0.0,
        radius_m: 1.0,
        soi_radius_m: None,
        rotation_period_s: 0.0,
        initial_rotation_deg: 0.0,
        is_star: false,
        ocean: None,
        site: None,
    }
}

#[test]
fn manual_axes_use_reference_torque_in_both_flight_paths() {
    for parts in [false, true] {
        for torque in [0.1, 10.0, 1000.0] {
            for dt in [0.01, 0.02, 0.08] {
                let mut p = Powered::new(
                    spec(torque),
                    DVec3::new(700000.0, 0.0, 0.0),
                    DVec3::ZERO,
                    false,
                );
                p.facing = DVec3::new(0.0, 1.0, 0.0);
                p.attitude = Attitude::Fixed(p.facing);
                p.rotation_input.x = 0.3;
                let engine = parts.then(|| Propulsion {
                    mass: MassProperties {
                        mass_t: 10.0,
                        inertia: Inertia {
                            x: DVec3::new(20.0, 0.0, 0.0),
                            y: DVec3::new(0.0, 20.0, 0.0),
                            z: DVec3::new(0.0, 0.0, 20.0),
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                });
                flight::integrate_with_propulsion(&mut p, &vacuum(), None, None, 0.0, dt, engine);
                assert!((p.angvel.length() - 0.3 * torque * dt / 20.0).abs() < 1e-9);
                assert!(p.sas.initialized);
                assert!(!p.manual_steering);
            }
        }
    }
}

#[test]
fn suborbital_coast_can_warp_and_stops_above_the_atmosphere() {
    let mut u = Universe::with_bodies(&[kerbin()]);
    let body = u.body_entity("Kerbin").unwrap();
    let r = DVec3::new(700_000.0, 0.0, 0.0);
    let v = DVec3::new(300.0, 1500.0, 0.0);
    let e = u.spawn_vessel("coast", body, r, v);
    let mut p = Powered::new(spec(10.0), r, v, false);
    p.coasting_since = Some(-20.0);
    u.set_powered(e, p);
    u.set_warp(100_000);
    assert_eq!(u.warp(), 100_000);
    assert!(u.dormant(e).is_some());
    u.step();
    let p = u.powered(e).expect("physics resumes before reentry");
    assert!(
        (p.r.length() - 671_000.0).abs() < 100.0,
        "radius {}",
        p.r.length()
    );
    assert_eq!(u.warp(), 1);
}

#[test]
fn rails_manual_controls_wake_without_throttle_and_keep_the_inertial_roll() {
    let mut u = Universe::with_bodies(&[kerbin()]);
    let body = u.body_entity("Kerbin").unwrap();
    let e = u.spawn_vessel_in_circular_orbit("craft", body, 100_000.0);
    let (_, _, orbit) = u.vessel(e).unwrap();
    let (r, v) = orbit.state_at(u.ut());
    let mut p = Powered::new(spec(10.0), r, v, false);
    p.attitude = Attitude::Fixed(DVec3::new(0.0, 1.0, 0.0));
    p.facing = DVec3::new(0.0, 1.0, 0.0);
    p.roll_rad = 0.7;
    let top = flight::vessel_top(&p);
    u.set_dormant(e, p);
    u.set_warp(1000);
    u.step();
    u.set_warp(1);
    assert!(u.set_rotation(e, 0.3, 0.0, 0.0));
    let p = u.powered(e).expect("manual control wakes a coast");
    assert_eq!(p.rotation_input.x, 0.3);
    assert!(flight::vessel_top(p).dot(top) > 1.0 - 1e-10);
    u.step();
    assert!(u.powered(e).unwrap().angvel.length() > 0.0);
    assert!(u.set_controls(e, 0.0, Attitude::Free));
    assert!(!u.powered(e).unwrap().manual_steering);
}

//! KSP2's SAS mode directions (`VesselAutopilot.SetAutopilotOrbit` / `SetAutopilotSurface`, the orbit-movement
//! frame of `TelemetryComponent.UpdateOrbitMovementTelemetry`):
//! - navball on orbit speed: prograde is the velocity, normal the orbit normal, and radial out the frame's right,
//!   square to the velocity in the orbit plane (not the local vertical, which it leaves by the flight-path angle)
//! - navball on surface speed: prograde is the surface velocity, normal and anti-normal the horizon's north and
//!   south, radial out and in straight up and down
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{bodies, flight, flight::Attitude, structure::Structure, Universe, TICKS_PER_SECOND};

fn angle_deg(a: DVec3, b: DVec3) -> f64 {
    a.normalized()
        .dot(b.normalized())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// A point on an eccentric orbit (e 0.3, 30 degrees past periapsis), climbing.
fn eccentric_state(mu: f64) -> (DVec3, DVec3) {
    let (a, e, nu) = (1_000_000.0f64, 0.3f64, 30f64.to_radians());
    let p = a * (1.0 - e * e);
    let r = p / (1.0 + e * nu.cos());
    let (s, c) = nu.sin_cos();
    // in the equatorial plane, orbit normal +Z
    let pos = DVec3::new(r * c, r * s, 0.0);
    let k = (mu / p).sqrt();
    let vel = DVec3::new(-k * s, k * (e + c), 0.0);
    (pos, vel)
}

#[test]
fn ksp2_orbit_radial_is_square_to_the_velocity_and_surface_modes_hold_the_horizon() {
    let mu = 3.5316e12;
    let (r, v) = eccentric_state(mu);
    let dir = |a| flight::attitude_direction(a, r, v, 0.0);
    let (up, east, north) = flight::surface_frame(r);
    let flight_path = 90.0 - angle_deg(r, v);
    println!("flight-path angle {flight_path:.2} deg");
    assert!(flight_path > 5.0, "a climbing point");
    // orbit speed
    let out = dir(Attitude::RadialOut);
    assert!(
        out.dot(v).abs() < 1e-9,
        "radial out is square to the velocity"
    );
    assert!(out.dot(r.cross(v)).abs() < 1e-3, "in the orbit plane");
    assert!(out.dot(up) > 0.0, "and outward");
    assert!(
        (angle_deg(out, up) - flight_path).abs() < 1e-6,
        "it leans off the vertical by the flight-path angle"
    );
    assert!((dir(Attitude::RadialIn) + out).length() < 1e-12);
    assert!(angle_deg(dir(Attitude::Normal), DVec3::new(0.0, 0.0, 1.0)) < 1e-5);
    assert!(angle_deg(dir(Attitude::OrbitPrograde), v) < 1e-5);
    // surface speed
    assert!(angle_deg(dir(Attitude::SurfaceNormal), north) < 1e-5);
    assert!(angle_deg(dir(Attitude::SurfaceAntiNormal), -north) < 1e-5);
    assert!(angle_deg(dir(Attitude::SurfaceRadialOut), up) < 1e-5);
    assert!(angle_deg(dir(Attitude::SurfaceRadialIn), -up) < 1e-5);
    assert!(dir(Attitude::SurfaceNormal).dot(east).abs() < 1e-12);
}

fn craft_on_eccentric_orbit() -> (Universe, Entity) {
    let mut kerbin = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
        "../../../assets/own/kerbol-system.json"
    ))
    .unwrap()
    .into_iter()
    .find(|b| b.id == "Kerbin")
    .unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let value: serde_json::Value = serde_json::from_str(
        include_str!("../../../tools/ksp2-parity-pilot/craft-rotation.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    let craft = ose_importer::craft::craft_from_value(&value).unwrap();
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
    let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface("orbiter", body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(&craft, &defs, &res));
    let (r, vel) = eccentric_state(mu);
    let mut p = u.powered(v).unwrap().clone();
    p.r = r;
    p.v = vel;
    p.landed = false;
    p.facing = vel.normalized();
    u.set_powered(v, p);
    (u, v)
}

#[test]
fn ksp2_sas_turns_the_craft_to_each_mode_and_holds_it() {
    for att in [
        Attitude::RadialOut,
        Attitude::RadialIn,
        Attitude::SurfaceNormal,
        Attitude::SurfaceAntiNormal,
        Attitude::SurfaceRadialOut,
        Attitude::SurfaceRadialIn,
    ] {
        let (mut u, v) = craft_on_eccentric_orbit();
        assert!(u.set_controls(v, 0.0, att));
        for _ in 0..(40 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).or(u.dormant(v)).unwrap();
        let omega = 0.0; // the test Kerbin's frame: surface modes use the craft's position only
        let target = flight::attitude_direction(att, p.r, p.v, omega);
        let off = angle_deg(p.facing, target);
        println!("{att:?}: {off:.2} deg off after 40 s");
        assert!(off < 1.0, "{att:?} held within 1 degree, off {off}");
    }
}

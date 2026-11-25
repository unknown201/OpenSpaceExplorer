//! KSP2 parity for a plane on the ground: the parity pilot's `strike taxi` run of Stock04_Jet on Runway_1
//! (tools/ksp2-parity-pilot/GroundStrike.cs): settle with the brakes on, full throttle to 25 m/s, coast 12 s,
//! brake to a stop, full throttle to 10 m/s, then full wheel steering while it coasts, and stop.
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

/// The stock jet launched on a flat Kerbin as the server launches a plane: shaped, laid level, heading 90.
fn on_runway() -> (Universe, bevy_ecs::entity::Entity, f64) {
    let value: serde_json::Value = serde_json::from_str(
        include_str!("../../../tools/ksp2-parity-pilot/craft-jet.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    let mut c = ose_importer::craft::craft_from_value(&value).unwrap();
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    c.apply_shapes(&defs);
    let c = ose_sim::structure::level_layout(&c).expect("built nose-forward");
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let spin = std::f64::consts::TAU / kerbin.rotation_period_s;
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_craft(v, c.clone());
    u.set_structure(v, Structure::build(&c, &defs, &res));
    assert!(u.set_level(v, 90.0));
    (u, v, spin)
}

#[derive(Debug)]
struct Row {
    phase: &'static str,
    speed: f64,
    heading_deg: f64,
}

/// Our run of the same inputs, a row a tick.
fn ose_taxi() -> Vec<Row> {
    let (mut u, v, spin) = on_runway();
    let mut rows = Vec::new();
    let mut t = 0.0;
    let record = |u: &Universe, phase: &'static str, rows: &mut Vec<Row>| {
        let p = u.powered(v).unwrap();
        let ground = DVec3::new(0.0, 0.0, spin).cross(p.r);
        rows.push(Row {
            phase,
            speed: (p.v - ground).length(),
            heading_deg: flight::heading_of(p.r, p.facing).to_degrees(),
        });
    };
    let speed = |u: &Universe| {
        let p = u.powered(v).unwrap();
        (p.v - DVec3::new(0.0, 0.0, spin).cross(p.r)).length()
    };
    macro_rules! run {
        ($phase:expr, $until:expr) => {{
            let start = t;
            loop {
                u.step();
                t += DT;
                record(&u, $phase, &mut rows);
                if ($until)(&u, t - start) {
                    break;
                }
            }
        }};
    }
    let hold = flight::Attitude::PitchHeading {
        pitch_deg: 0.0,
        heading_deg: 90.0,
    };
    u.set_drive(v, 0.0, 0.0, true);
    u.set_controls(v, 0.0, hold);
    u.stage(v);
    run!("settle", |_: &Universe, dt: f64| dt >= 5.0);
    u.set_drive(v, 0.0, 0.0, false);
    u.set_controls(v, 1.0, hold);
    run!("accel", |u: &Universe, dt: f64| speed(u) >= 25.0
        || dt >= 60.0);
    u.set_controls(v, 0.0, hold);
    run!("coast", |_: &Universe, dt: f64| dt >= 12.0);
    u.set_drive(v, 0.0, 0.0, true);
    run!("brake", |u: &Universe, dt: f64| speed(u) <= 0.2
        || dt >= 20.0);
    u.set_drive(v, 0.0, 0.0, false);
    u.set_controls(v, 1.0, hold);
    run!("taxi", |u: &Universe, dt: f64| speed(u) >= 10.0
        || dt >= 30.0);
    u.set_controls(v, 0.0, flight::Attitude::Free);
    u.set_drive(v, 0.0, 1.0, false);
    run!("steer", |_: &Universe, dt: f64| dt >= 10.0);
    u.set_drive(v, 0.0, 0.0, true);
    run!("stop", |_: &Universe, dt: f64| dt >= 8.0);
    rows
}

/// KSP2's run (reference\ksp2-strike-taxi, 2026-11-25): per phase, seconds, speed at the end (m/s), distance
/// (m) and the heading it turned (degrees).
const KSP2: [(&str, f64, f64, f64, f64); 6] = [
    ("accel", 13.8, 24.9, 105.6, 0.0),
    ("coast", 12.0, 33.2, 389.0, 0.2),
    ("brake", 3.88, 0.37, 76.8, 2.7),
    ("taxi", 9.6, 10.0, 29.5, 0.0),
    ("steer", 10.0, 14.9, 142.0, 151.0),
    ("stop", 8.0, 0.0, 21.7, 10.0),
];

#[test]
fn the_jet_speeds_up_coasts_brakes_and_turns_on_the_runway_as_in_ksp2() {
    let rows = ose_taxi();
    for (phase, secs, end, dist, turned) in KSP2 {
        let r: Vec<&Row> = rows.iter().filter(|r| r.phase == phase).collect();
        let ours_s = r.len() as f64 * DT;
        let ours_end = r.last().unwrap().speed;
        let ours_dist: f64 = r.iter().map(|x| x.speed * DT).sum();
        let ours_turn: f64 = r
            .windows(2)
            .map(|w| ((w[1].heading_deg - w[0].heading_deg + 540.0) % 360.0) - 180.0)
            .sum();
        println!(
            "{phase}: {ours_s:.2} s (KSP2 {secs}), ends {ours_end:.2} m/s ({end}), {ours_dist:.1} m ({dist}), turned {ours_turn:.1} deg ({turned})"
        );
        let near =
            |ours: f64, ksp2: f64, within: f64| (ours - ksp2).abs() <= within * ksp2.abs().max(5.0);
        // the phases that end on a speed take as long
        if phase != "coast" && phase != "steer" && phase != "stop" {
            assert!(near(ours_s, secs, 0.12), "{phase}: {ours_s:.2} s vs {secs}");
        }
        // the brakes stop it about 10% shorter than KSP2's: KSP2's nose leg bottoms out as the jet pitches
        // down, and the load it then carries through the leg grips nothing; ours has no suspension travel
        let within = if phase == "brake" || phase == "stop" {
            0.16
        } else {
            0.12
        };
        assert!(
            near(ours_dist, dist, within),
            "{phase}: {ours_dist:.1} m vs {dist}"
        );
        assert!(
            near(ours_end, end, 0.12),
            "{phase}: {ours_end:.2} m/s vs {end}"
        );
        // full steering turns it on its nose wheel alone (its main gear is built with steering off)
        if phase == "steer" {
            assert!(
                near(ours_turn, turned, 0.12),
                "turned {ours_turn:.1} vs {turned}"
            );
        }
    }
}

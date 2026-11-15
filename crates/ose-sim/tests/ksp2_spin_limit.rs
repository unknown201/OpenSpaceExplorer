//! KSP2 caps every part's spin at `PhysicsSettings.MaxAngularVelocity` (50 rad/s, read live from KSP2; PhysX
//! `Rigidbody.maxAngularVelocity`, set in `RigidbodyBehavior`). An attitude hold chasing a target that jumps
//! every tick keeps the reaction wheels pushing one way: in KSP2 the craft spins up to 50 rad/s and no faster.
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{bodies, flight, flight::Attitude, structure::Structure, Universe, TICKS_PER_SECOND};

/// The SAS test craft (pod, wheel, tanks) in a 100 km circular Kerbin orbit, nose along its velocity.
fn craft_in_orbit(craft: &ose_data::craft::Craft) -> (Universe, Entity) {
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
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
    let report = ose_data::staging::delta_v_by_stage(craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface("spinner", body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(craft, &defs, &res));
    u.set_craft(v, craft.clone());
    let r = 700_000.0;
    let mut p = u.powered(v).unwrap().clone();
    p.r = DVec3::new(r, 0.0, 0.0);
    p.v = DVec3::new(0.0, (mu / r).sqrt(), 0.0);
    p.landed = false;
    p.facing = p.v.normalized();
    u.set_powered(v, p);
    (u, v)
}

fn rotation_craft() -> ose_data::craft::Craft {
    let value: serde_json::Value = serde_json::from_str(
        include_str!("../../../tools/ksp2-parity-pilot/craft-rotation.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    ose_importer::craft::craft_from_value(&value).unwrap()
}

fn zvezda() -> ose_data::craft::Craft {
    serde_json::from_str(include_str!("data/zvezda-module.json")).unwrap()
}

#[test]
fn a_hold_chasing_a_target_that_flips_every_tick_never_spins_past_ksp2s_cap() {
    for (name, craft, reversed) in [
        ("rotation", rotation_craft(), false),
        ("zvezda", zvezda(), false),
        ("zvezda reversed", zvezda(), true),
    ] {
        println!("== {name}");
        let (mut u, v) = craft_in_orbit(&craft);
        if reversed {
            let mut p = u.powered(v).unwrap().clone();
            p.control_rotation = [0.0, 1.0, 0.0, 0.0];
            u.set_powered(v, p);
        }
        let start = u.powered(v).unwrap().clone();
        let target = DVec3::new(0.0, 0.0, 1.0);
        let mut fastest: f64 = 0.0;
        for k in 0..(300 * TICKS_PER_SECOND) {
            let dir = if k % 2 == 0 { target } else { -target };
            assert!(u.set_controls(v, 0.0, Attitude::Fixed(dir)));
            u.step();
            let p = u.powered(v).or(u.dormant(v)).unwrap();
            let spin = (p.angvel + p.facing * p.roll_rate).length();
            fastest = fastest.max(spin);
            if k % (30 * TICKS_PER_SECOND) == 0 {
                println!(
                    "t {:4} s: spin {spin:8.3} rad/s, speed {:8.1} m/s, height {:8.0} m",
                    k / TICKS_PER_SECOND,
                    p.v.length(),
                    p.r.length() - 600_000.0
                );
            }
        }
        let p = u.powered(v).or(u.dormant(v)).unwrap();
        println!("fastest spin {fastest:.3} rad/s");
        assert!(
            fastest <= flight::MAX_ANGULAR_VELOCITY + 1e-6,
            "spin {fastest:.1} rad/s past KSP2's cap"
        );
        // the craft is still in its orbit: spinning does not move it
        assert!(
            (p.v.length() - start.v.length()).abs() < 1.0,
            "speed {:.1} m/s (started {:.1})",
            p.v.length(),
            start.v.length()
        );
        assert!(
            (p.r.length() - start.r.length()).abs() < 1_000.0,
            "radius {:.0} m (started {:.0})",
            p.r.length(),
            start.r.length()
        );
    }
}

/// Started spinning far past the cap (600 rad/s, the runaway the delivery-routes pilot hit): the first tick
/// brings it down to KSP2's 50 rad/s, and the craft stays in its orbit (the runaway ended with one step
/// throwing the vessel out of Kerbin's sphere of influence).
#[test]
fn a_craft_spun_past_the_cap_is_held_to_it_and_stays_in_orbit() {
    for craft in [rotation_craft(), zvezda()] {
        let (mut u, v) = craft_in_orbit(&craft);
        let start = u.powered(v).unwrap().clone();
        let mut p = start.clone();
        let axis = (p.facing.cross(p.r.normalized()) + p.facing * 0.3).normalized();
        let w = axis * 600.0;
        p.roll_rate = w.dot(p.facing);
        p.angvel = w - p.facing * p.roll_rate;
        u.set_powered(v, p);
        assert!(u.set_controls(v, 0.0, Attitude::Free));
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
            let p = u.powered(v).or(u.dormant(v)).unwrap();
            let spin = (p.angvel + p.facing * p.roll_rate).length();
            assert!(
                spin <= flight::MAX_ANGULAR_VELOCITY + 1e-6,
                "{}: {spin:.1} rad/s",
                craft.name
            );
        }
        let p = u.powered(v).or(u.dormant(v)).unwrap();
        println!(
            "{}: speed {:.2} m/s (started {:.2}), radius {:.0} m",
            craft.name,
            p.v.length(),
            start.v.length(),
            p.r.length()
        );
        assert!(
            (p.v.length() - start.v.length()).abs() < 5.0,
            "{}",
            craft.name
        );
        assert!(
            (p.r.length() - start.r.length()).abs() < 1_000.0,
            "{}",
            craft.name
        );
    }
}

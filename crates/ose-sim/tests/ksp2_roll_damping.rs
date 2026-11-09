//! KSP2 parity for spin in air (gap T5/T13 roll): KSP2 sets each part's PhysX angular drag to
//! `PartData.angularDrag x dynamic pressure (atm) x AngularDragMultiplier (2)` (`RigidbodyBehavior`), and PhysX
//! damps each body's own spin by it. The pilot's `struts` craft (core tank and Swivel, two radial Swivel
//! boosters, `craft-struts.json`), staged on the pad with SAS holding and full throttle, gets full roll from 5 s
//! to 8 s. KSP2 (run 2026-11-08 21:12Z) levels off at 2.58 rad/s as the air thickens its damping and stops in
//! about 1.5 s after the input is released; without the damping OSE rolled up to 4.8 rad/s.
use bevy_ecs::entity::Entity;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

/// KSP2's roll rate, rad/s, at 6.0, 6.5, ... 9.5 s (roll input on from 5 to 8 s).
const KSP2_ROLL: [(f64, f64); 8] = [
    (6.0, 1.267),
    (6.5, 1.824),
    (7.0, 2.226),
    (7.5, 2.475),
    (8.0, 2.581),
    (8.5, 1.474),
    (9.0, 0.408),
    (9.5, 0.083),
];

fn on_pad() -> (Universe, Entity) {
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
        include_str!("../../../tools/ksp2-parity-pilot/craft-struts.json")
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
    let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&craft.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(&craft, &defs, &res));
    (u, v)
}

#[test]
fn ksp2_a_rolling_craft_levels_off_and_stops_as_ksp2s_does_in_air() {
    let (mut u, v) = on_pad();
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    let tick = |s: f64| (s * TICKS_PER_SECOND as f64).round() as usize;
    let mut worst: f64 = 0.0;
    for k in 1..=tick(9.5) {
        if k == tick(5.0) + 1 {
            u.set_rotation(v, 0.0, 0.0, 1.0);
        }
        if k == tick(8.0) + 1 {
            u.set_rotation(v, 0.0, 0.0, 0.0);
        }
        u.step();
        if let Some(&(t, ksp2)) = KSP2_ROLL.iter().find(|(t, _)| tick(*t) == k) {
            let ours = u.powered(v).unwrap().roll_rate.abs();
            println!("t {t:.1}: roll {ours:.3} rad/s (KSP2 {ksp2:.3})");
            worst = worst.max((ours - ksp2).abs());
        }
    }
    // within 0.25 rad/s everywhere (the first half second after release lags KSP2 by about one tick of SAS)
    assert!(worst < 0.25, "worst difference {worst:.3} rad/s");
    let p = u.powered(v).unwrap();
    assert!(
        p.roll_rate.abs() < 0.2 && p.angvel.length() < 0.2,
        "stopped and steady"
    );
}

#[test]
fn ksp2_angular_drag_is_zero_in_vacuum_and_follows_part_data() {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let drag = |id: &str| defs.iter().find(|d| d.id == id).unwrap().angular_drag;
    // KSP2 PartData: 2 on pods, tanks and engines, 1 on the radial decoupler and the strut
    assert_eq!(drag("pod_1v_conical_crew"), 2.0);
    assert_eq!(drag("fueltank_1v_inline_methalox_1x4"), 2.0);
    assert_eq!(drag("engine_1v_methalox_swivel"), 2.0);
    assert_eq!(drag("decoupler_1v_radial"), 1.0);
    assert_eq!(drag("strut_1v_extensible"), 1.0);
}

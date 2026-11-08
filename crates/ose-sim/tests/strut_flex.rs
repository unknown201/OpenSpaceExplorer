//! T39 struts: the KSP2 parity pilot's "struts" craft (`tools/ksp2-parity-pilot/craft-struts.json`): a core
//! tank and engine with two radial booster stacks, the +X one braced to the core by a strut, the -X one not.
//! Staged on the pad, SAS holding, full throttle 5 s, full roll 3 s, 3 s free, as the pilot flies it; each
//! booster engine's flex (offset from its rest pose relative to the pod) is traced.
use std::f64::consts::TAU;

use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

fn kerbin() -> ose_data::CelestialBody {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut b = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    b.parent = None;
    b.orbit = None;
    b.soi_radius_m = None;
    b
}

fn craft() -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(
        include_str!("../../../tools/ksp2-parity-pilot/craft-struts.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

fn on_pad(c: &ose_data::craft::Craft) -> (Universe, Entity) {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let body = kerbin();
    let _spin = TAU / body.rotation_period_s;
    let mut u = Universe::with_bodies(&[body]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(c, &defs, &res));
    (u, v)
}

/// Each booster engine's largest flex, m, in the thrust phase (0..5 s) and over the whole run (to `seconds`):
/// ((+X braced, -X unbraced) thrust, (+X, -X) all).
fn booster_flex(c: &ose_data::craft::Craft, seconds: f64) -> ((f64, f64), (f64, f64)) {
    let engines: Vec<usize> = (0..c.parts.len())
        .filter(|&i| c.parts[i].part_id == "engine_1v_methalox_swivel")
        .collect();
    let side = |i: usize| c.parts[i].local_position.x;
    let plus = *engines.iter().find(|&&i| side(i) > 0.5).unwrap();
    let minus = *engines.iter().find(|&&i| side(i) < -0.5).unwrap();
    let (mut u, v) = on_pad(c);
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    let ticks = |s: f64| (s * TICKS_PER_SECOND as f64).round() as usize;
    let (mut fp, mut fm) = (0.0f64, 0.0f64);
    let mut thrust = (0.0f64, 0.0f64);
    for k in 1..=ticks(seconds) {
        if k == ticks(5.0) + 1 {
            u.set_rotation(v, 0.0, 0.0, 1.0);
        }
        if k == ticks(8.0) + 1 {
            u.set_rotation(v, 0.0, 0.0, 0.0);
        }
        u.step();
        let s = u.structure(v).unwrap();
        let flex = s.rigid.flex(0.0, 0.0);
        let off = |part: usize| {
            flex.iter()
                .find(|f| f.part == part)
                .map(|f| f.offset_m)
                .unwrap_or(DVec3::ZERO)
                .length()
        };
        fp = fp.max(off(plus));
        fm = fm.max(off(minus));
        if k <= ticks(5.0) {
            thrust = (fp, fm);
        }
        if std::env::var("OSE_FLEX_PRINT").is_ok() && k % 25 == 0 {
            let pw = u.powered(v).unwrap();
            println!(
                "t {:.1}: braced {:.2} mm, unbraced {:.2} mm, angvel {:.3} roll {:.3}, alt {:.0}",
                k as f64 / TICKS_PER_SECOND as f64,
                off(plus) * 1e3,
                off(minus) * 1e3,
                pw.angvel.length(),
                pw.roll_rate,
                pw.r.length() - 600_000.0
            );
        }
    }
    (thrust, (fp, fm))
}

/// KSP2 (pilot scenario `struts`, run 2026-11-08): the booster engines move 0.046 mm (braced) and 0.042 mm
/// (unbraced) from rest relative to the pod under thrust, about 2 mm through the roll and after, and the strut
/// makes no visible difference: KSP2's Enhanced Joints tie every part to the heaviest one
/// (`struts::enhance`), so a radial attachment holds as rigidly as a stack (`OSE_FLEX_PRINT=1` prints the run).
const KSP2_THRUST_FLEX_M: (f64, f64) = (0.000046, 0.000042);
const KSP2_RUN_FLEX_M: (f64, f64) = (0.001925, 0.002073);

#[test]
fn braced_and_unbraced_boosters_hold_as_ksp2s_under_thrust() {
    let c = craft();
    assert_eq!(
        c.parts
            .iter()
            .filter(|p| p.part_id == "strut_1v_extensible")
            .count(),
        2,
        "one strut, two anchors"
    );
    let ((braced, loose), _) = booster_flex(&c, 5.0);
    println!(
        "thrust: booster engine flex braced {:.3} mm (KSP2 {:.3}), unbraced {:.3} mm (KSP2 {:.3})",
        braced * 1e3,
        KSP2_THRUST_FLEX_M.0 * 1e3,
        loose * 1e3,
        KSP2_THRUST_FLEX_M.1 * 1e3
    );
    // both boosters hold as KSP2's do under thrust: with KSP2's enhanced joints (`struts::enhance`) the
    // unbraced one is held by its reinforced joint to the heaviest part, so the strut makes no visible
    // difference, as in KSP2
    assert!(braced < 2.0 * KSP2_THRUST_FLEX_M.0, "braced {braced}");
    assert!(loose < 2.0 * KSP2_THRUST_FLEX_M.1, "unbraced {loose}");
}

/// The rest of the KSP2 comparison: the unbraced booster under thrust and both through the roll.
#[test]
fn unbraced_boosters_and_the_roll_match_ksp2() {
    let ((_, loose), (bp, lp)) = booster_flex(&craft(), 11.0);
    assert!(
        loose < 2.0 * KSP2_THRUST_FLEX_M.1,
        "unbraced under thrust {loose}"
    );
    assert!(bp < 2.0 * KSP2_RUN_FLEX_M.0, "braced through the run {bp}");
    assert!(
        lp < 2.0 * KSP2_RUN_FLEX_M.1,
        "unbraced through the run {lp}"
    );
}

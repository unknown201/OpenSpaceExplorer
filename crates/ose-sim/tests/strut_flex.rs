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

/// Each booster engine's largest flex during the run, m: (+X braced, -X unbraced).
fn booster_flex(c: &ose_data::craft::Craft) -> (f64, f64) {
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
    for k in 1..=ticks(11.0) {
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
    }
    (fp, fm)
}

#[test]
fn a_strut_holds_its_booster_stiffer_than_the_unbraced_one() {
    let c = craft();
    assert_eq!(
        c.parts
            .iter()
            .filter(|p| p.part_id == "strut_1v_extensible")
            .count(),
        2,
        "one strut, two anchors"
    );
    let (braced, loose) = booster_flex(&c);
    println!(
        "booster engine flex: braced {:.2} mm, unbraced {:.2} mm",
        braced * 1e3,
        loose * 1e3
    );
    assert!(braced < loose, "the strut stiffens its booster");
}

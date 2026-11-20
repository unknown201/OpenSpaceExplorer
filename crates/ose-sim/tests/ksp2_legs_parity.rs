//! KSP2 parity for landing legs on flat ground (gap T15).
//!
//! The KSP2 parity pilot's `legsrunway` run (2026-11-20): the run-1 craft with three landingleg_1v
//! (`tools/ksp2-parity-pilot/craft-legs.json`) on Runway_1, SAS on, legs deployed, 10 s standing, then 0.3 s of
//! full throttle, cut, and the drop back onto the legs (trimmed trace in `tests/data/ksp2-legs-runway`).
//!
//! KSP2's legs give 0.44 m of their 0.80 m under the craft, so it stands on its engine bell at the height it
//! stood before they came out. Back down at 8.2 m/s the bell takes the blow; PhysX pushes the bell back out
//! of the runway (up to +0.44 m, as the T14 drops do) and it is still within 0.75 s. Our contact stops the
//! hull at the surface, so it must never bounce higher than that. Every part survives in both.
use std::collections::HashMap;
use std::f64::consts::TAU;

use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

type Row = HashMap<String, String>;

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
        include_str!("../../../tools/ksp2-parity-pilot/craft-legs.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

/// The craft standing on flat ground at the equator, SAS holding it upright.
fn on_ground(c: &ose_data::craft::Craft) -> (Universe, Entity, f64) {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let body = kerbin();
    let spin = TAU / body.rotation_period_s;
    let mut u = Universe::with_bodies(&[body]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(c, &defs, &res));
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 0.0, flight::Attitude::Fixed(facing)));
    (u, v, spin)
}

fn csv(text: &str) -> Vec<Row> {
    let mut lines = text.lines();
    let head: Vec<String> = lines
        .next()
        .unwrap()
        .split(',')
        .map(str::to_string)
        .collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            head.iter()
                .cloned()
                .zip(l.split(',').map(str::to_string))
                .collect()
        })
        .collect()
}

fn num(row: &Row, k: &str) -> f64 {
    row[k].parse().unwrap()
}

fn vspeed(p: &flight::Powered, spin: f64) -> f64 {
    (p.v - DVec3::new(0.0, 0.0, spin).cross(p.r)).dot(p.r.normalized())
}

fn parts(u: &Universe, v: Entity) -> usize {
    u.structure(v)
        .map(|s| s.rigid.present.iter().filter(|&&x| x).count())
        .unwrap_or(0)
}

#[test]
fn ksp2_lander_stands_on_its_legs_and_comes_back_down_on_them_as_in_ksp2() {
    let rows = csv(include_str!("data/ksp2-legs-runway/legs-trace.csv"));
    let c = craft();
    let (mut u, v, spin) = on_ground(&c);
    for _ in 0..(1.0 / DT) as usize {
        u.step();
    }
    let before = u.powered(v).unwrap().r.length();
    assert_eq!(u.deploy_gear(v, true), Some(3));
    for _ in 0..(10.0 / DT) as usize {
        u.step();
    }
    // KSP2: standing on the legs at the height it stood before they came out (+1 mm)
    let rest = u.powered(v).unwrap().r.length();
    assert!(
        (rest - before).abs() < 0.02,
        "stands {:+.3} m from where it stood before the legs came out (KSP2 +0.001)",
        rest - before
    );
    assert!(u.powered(v).unwrap().landed);

    // KSP2's hop from the top of its climb (the engine's thrust tails off over the first ticks in the air)
    let k_rest = num(rows.iter().find(|r| num(r, "t") > 10.0).unwrap(), "alt_m");
    let flying = rows
        .iter()
        .position(|r| r["situation"] == "Flying")
        .unwrap();
    let start = (flying + 1..rows.len())
        .find(|&k| num(&rows[k], "vspeed") < num(&rows[k - 1], "vspeed"))
        .unwrap();
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    p.r = up * (rest + num(&rows[start], "alt_m") - k_rest);
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + up * num(&rows[start], "vspeed");
    p.landed = false;
    u.set_powered(v, p);

    // KSP2: struck at its fastest fall, bounced to its apex, and the last tick it moved more than 5 cm/s
    let t0 = num(&rows[start], "t");
    let hit = (start + 1..rows.len())
        .find(|&k| num(&rows[k], "vspeed") > num(&rows[k - 1], "vspeed") + 2.0)
        .unwrap();
    let k_impact = -rows[start..hit]
        .iter()
        .map(|r| num(r, "vspeed"))
        .fold(f64::INFINITY, f64::min);
    let k_apex = rows[hit..]
        .iter()
        .take_while(|r| num(r, "t") < num(&rows[hit], "t") + 1.0)
        .map(|r| num(r, "alt_m") - k_rest)
        .fold(f64::NEG_INFINITY, f64::max);
    let k_still = rows[hit..]
        .iter()
        // within the first second: the slow lean that tips it over (gap T15) starts after that
        .take_while(|r| num(r, "t") < num(&rows[hit], "t") + 1.0)
        .filter(|r| num(r, "vspeed").abs() > 0.05)
        .map(|r| num(r, "t"))
        .fold(0.0, f64::max)
        - num(&rows[hit], "t");

    let (mut impact, mut apex, mut still, mut struck) = (0.0f64, f64::NEG_INFINITY, 0.0, None);
    let mut last = num(&rows[start], "vspeed");
    for k in 1..=(4.0 / DT) as usize {
        u.step();
        let p = u.powered(v).unwrap();
        let vs = vspeed(p, spin);
        let t = k as f64 * DT;
        if struck.is_none() {
            impact = impact.max(-last);
        }
        match struck {
            None if vs > last + 2.0 => {
                struck = Some(t);
            }
            Some(at) => {
                apex = f64::max(apex, p.r.length() - rest);
                if vs.abs() > 0.05 {
                    still = t - at;
                }
            }
            None => {}
        }
        last = vs;
    }
    let p = u.powered(v).unwrap();
    println!(
        "touchdown at {:.2} s: struck at {impact:.2} m/s (KSP2 {k_impact:.2}), apex {apex:+.3} m (KSP2 {k_apex:+.3}), \
         still after {still:.2} s (KSP2 {k_still:.2}), KSP2 struck {:.2} s after the top",
        struck.unwrap_or(f64::NAN),
        num(&rows[hit], "t") - t0
    );
    // KSP2's feet reach 0.44 m below the bell (their contact depth at rest), ours 0.24 m (the leg's extended
    // drag cube), so its legs start braking about two ticks before the bell strikes and ours one
    assert!(
        (impact - k_impact).abs() < 0.4,
        "fastest fall {impact:.2} vs KSP2 {k_impact:.2} m/s"
    );
    assert_eq!(parts(&u, v), c.parts.len(), "a part broke");
    assert!(!p.crashed && p.landed, "landed at the end");
    assert!(
        apex <= k_apex + 0.02,
        "bounced to {apex:+.3} m, over KSP2's {k_apex:+.3}"
    );
    assert!(
        still <= k_still + 0.1,
        "moving for {still:.2} s, KSP2 {k_still:.2} s"
    );
    assert!(
        (p.r.length() - rest).abs() < 0.02,
        "rests {:+.3} m from where it stood",
        p.r.length() - rest
    );
}

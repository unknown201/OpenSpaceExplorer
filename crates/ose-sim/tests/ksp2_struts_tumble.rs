//! KSP2 parity for the tumble after a roll (gap T13): the pilot's `strutslong` run (2026-11-25), the struts craft
//! (core tank and Swivel, two radial Swivel boosters) staged on the pad, SAS on, full throttle, full roll from 5 to
//! 8 s and free to 16 s. KSP2's craft pitches over about the axis through its boosters from about 9 s (Mach 1, 60
//! kPa), its SAS pitch input saturating at 10.7 s, and is flying engine first by 12.5 s.
use std::collections::HashMap;

use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

type Row = HashMap<String, String>;

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

fn num(r: &Row, k: &str) -> f64 {
    r[k].parse().unwrap()
}

fn at(rows: &[Row], t: f64) -> &Row {
    rows.iter()
        .min_by(|a, b| (num(a, "t") - t).abs().total_cmp(&(num(b, "t") - t).abs()))
        .unwrap()
}

fn on_pad() -> (Universe, Entity, f64) {
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
    let spin = std::f64::consts::TAU / kerbin.rotation_period_s;
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
    (u, v, spin)
}

/// A KSP2 pod-frame (Unity) vector in the world, OSE's vessel frame being Unity's with Z mirrored.
fn from_pod(p: &flight::Powered, l: DVec3) -> DVec3 {
    let (bx, by, bz) = flight::vessel_basis(p);
    bx * l.x + by * l.y + bz * -l.z
}

/// A world vector in KSP2's pod frame.
fn to_pod(p: &flight::Powered, w: DVec3) -> DVec3 {
    let (bx, by, bz) = flight::vessel_basis(p);
    DVec3::new(w.dot(bx), w.dot(by), -w.dot(bz))
}

/// KSP2's spin in the pod frame (the trace's angular velocity is in the body's frame).
fn ksp2_spin(r: &Row) -> DVec3 {
    let q = DVec3::new(num(r, "qx"), num(r, "qy"), num(r, "qz"));
    let w = num(r, "qw");
    let v = DVec3::new(num(r, "wx"), num(r, "wy"), num(r, "wz"));
    // rotate by the inverse of the control transform's rotation
    let qi = -q;
    let t = qi.cross(v) * 2.0;
    v + t * w + qi.cross(t)
}

fn aoa_deg(air: DVec3) -> f64 {
    (air.y / air.length()).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Fly the struts craft as the pilot did (SAS holding, full throttle, full roll 5 to 8 s); `kick` adds KSP2's
/// onset spin about the pod's X at 5.2 s. Calls `each` every tick with the time and the angle of attack.
fn fly(kick: f64, until: f64, mut each: impl FnMut(f64, f64, &flight::Powered)) {
    let (mut u, v, spin) = on_pad();
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    let dt = 1.0 / TICKS_PER_SECOND as f64;
    for k in 1..=(until / dt).round() as usize {
        let t = k as f64 * dt;
        if (t - 5.0).abs() < dt / 2.0 {
            u.set_rotation(v, 0.0, 0.0, 1.0);
        }
        if (t - 8.0).abs() < dt / 2.0 {
            u.set_rotation(v, 0.0, 0.0, 0.0);
        }
        if (t - 5.2).abs() < dt / 2.0 && kick != 0.0 {
            let mut p = u.powered(v).unwrap().clone();
            p.angvel = p.angvel + from_pod(&p, DVec3::new(-kick, 0.0, 0.0));
            u.set_powered(v, p);
        }
        u.step();
        let p = u.powered(v).unwrap();
        let srf = p.v - DVec3::new(0.0, 0.0, spin).cross(p.r);
        each(t, aoa_deg(to_pod(p, srf)), p);
    }
}

/// From KSP2's state at 9.38 s (Mach 1.06, 3.5 degrees off the airflow, pitching at 0.075 rad/s), OSE's SAS
/// answers as KSP2's did (pitch 0.32, 0.47, 0.66, 0.91 and then full) and the craft pitches over the same way:
/// the aerodynamic moments, the gimbals and the SAS all match KSP2's through the tumble.
#[test]
fn ksp2_struts_craft_pitches_over_from_ksp2s_state_as_ksp2s_does() {
    let trace = csv(include_str!("data/ksp2-struts-tumble/trace.csv"));
    let air = csv(include_str!("data/ksp2-struts-tumble/pod-air.csv"));
    let t0 = 9.38;
    let (mut u, v, spin) = on_pad();
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    let dt = 1.0 / TICKS_PER_SECOND as f64;
    for _ in 0..(t0 / dt).round() as usize {
        u.step();
    }
    let (tr, ar) = (at(&trace, t0), at(&air, t0));
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    p.r = up * (600_000.0 + num(tr, "alt_m"));
    let srf = from_pod(&p, DVec3::new(num(ar, "ax"), num(ar, "ay"), num(ar, "az")));
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + srf;
    // the trace's spin in the pod frame, reflected into OSE's (an axial vector: X and Y flip, Z stays)
    let w = ksp2_spin(tr);
    let w = from_pod(&p, DVec3::new(-w.x, -w.y, w.z));
    p.roll_rate = w.dot(p.facing);
    p.angvel = w - p.facing * p.roll_rate;
    u.set_powered(v, p);
    assert!((u.powered(v).unwrap().mass_t - num(tr, "mass_t")).abs() < 0.05);
    let hold = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(hold)));
    for k in 1..=(2.6 / dt) as usize {
        let t = t0 + k as f64 * dt;
        u.step();
        if k % 13 != 0 {
            continue;
        }
        let p = u.powered(v).unwrap();
        let ours = aoa_deg(to_pod(p, p.v - DVec3::new(0.0, 0.0, spin).cross(p.r)));
        let ka = at(&air, t);
        let theirs = aoa_deg(DVec3::new(num(ka, "ax"), num(ka, "ay"), num(ka, "az")));
        println!("{t:5.2}: {ours:6.2} degrees off the airflow (KSP2 {theirs:6.2})");
        assert!(
            (ours - theirs).abs() < (0.08 * theirs).max(0.6),
            "{t:.2} s: {ours:.2} vs KSP2 {theirs:.2} degrees"
        );
    }
}

/// KSP2's tumble starts at the roll: its booster gimbals move a tick apart as the roll comes on (the first
/// engine's thrust point swings out while its thrust still points straight, the second's a tick later), a net
/// 0.9 kN m about the pod's X for 0.2 s that leaves it pitching at 0.010 rad/s. Rolling at 2.5 rad/s grows that
/// to 1.4 degrees off the airflow by 8 s (OSE's even gimbals leave 0.2), and past Mach 1 SAS cannot hold it.
/// Given the same onset spin OSE goes the same way: about 9 degrees at 10.1 s (KSP2 8.6) and over by 12.5 s.
#[test]
fn ksp2_roll_onset_spin_grows_into_the_same_tumble() {
    let mut at_10 = 0.0;
    let mut flipped = 0.0;
    fly(0.010, 12.6, |t, aoa, _| {
        if (t - 10.14).abs() < 0.01 {
            at_10 = aoa;
        }
        if aoa > 90.0 && flipped == 0.0 {
            flipped = t;
        }
    });
    println!("onset spin: {at_10:.2} degrees at 10.14 s (KSP2 8.63), over 90 at {flipped:.2} s (KSP2 12.15)");
    assert!(
        (5.0..13.0).contains(&at_10),
        "{at_10:.2} degrees at 10.14 s"
    );
    assert!(
        flipped > 11.8 && flipped < 12.6,
        "over 90 degrees at {flipped:.2} s"
    );
    // without it OSE's craft stays within a few degrees and SAS brings it back
    let mut worst: f64 = 0.0;
    fly(0.0, 12.6, |t, aoa, _| {
        if t > 9.0 {
            worst = worst.max(aoa);
        }
    });
    assert!(worst < 5.0, "{worst:.2} degrees");
}

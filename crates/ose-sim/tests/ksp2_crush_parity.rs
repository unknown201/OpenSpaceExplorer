//! KSP2 parity for how far a crash crushes through a craft (gap T14 follow-up): the pilot's drops of the tall
//! stack (pod, six 1x4 tanks, Swivel) at 10 to 45 m/s and of the pod on a 1x2 tank at 25 and 35 m/s
//! (`tools/ksp2-parity-pilot/run-drops2.ps1`; trimmed traces in `tests/data/ksp2-drops2`). Each layer breaks as
//! it reaches the ground; one struck well over its tolerance does not slow the rest, which strikes in turn, and
//! one struck barely over it stops the craft.
use std::collections::HashMap;
use std::f64::consts::TAU;

use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;
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

/// KSP2: the parts left after the drop (0: lost), and the times (s after the drop) the craft lost each part,
/// from its mass on the trace's own clock.
fn ksp2(trace: &str, parts: &str) -> (usize, Vec<f64>) {
    let rows = csv(trace);
    let start = rows
        .iter()
        .position(|r| r["situation"] != "PreLaunch")
        .unwrap();
    let t0 = num(&rows[start], "t");
    let mut drops = Vec::new();
    for w in rows[start..].windows(2) {
        if num(&w[0], "mass_t") - num(&w[1], "mass_t") > 0.01 {
            drops.push(num(&w[1], "t") - t0);
        }
    }
    let left = csv(parts).last().map(|r| num(r, "parts") as usize).unwrap();
    (left, drops)
}

/// OSE: the same craft 1 m above its resting height falling at `speed`; parts left after 3 s (0: lost) and the
/// times each part went.
fn ose(json: &str, speed: f64) -> (usize, Vec<f64>) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = ose_importer::craft::craft_from_value(&v).unwrap();
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let spin = TAU / kerbin.rotation_period_s;
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(&c, &defs, &res));
    u.set_craft(v, c.clone());
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    p.r = p.r + up;
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) - up * speed;
    p.landed = false;
    u.set_powered(v, p);
    let count = |u: &Universe| {
        if u.powered(v).is_none_or(|p| p.crashed) {
            return 0;
        }
        u.structure(v)
            .map(|s| s.rigid.present.iter().filter(|&&x| x).count())
            .unwrap_or(0)
    };
    let mut last = count(&u);
    let mut drops = Vec::new();
    for k in 1..=(3 * TICKS_PER_SECOND) {
        u.step();
        let n = count(&u);
        for _ in n..last {
            drops.push(k as f64 * DT);
        }
        last = n;
    }
    (last, drops)
}

#[test]
fn ksp2_crashes_crush_through_the_same_layers_at_the_same_pace() {
    let stack = include_str!("../../../tools/ksp2-parity-pilot/craft-stack.json");
    let podtank = include_str!("../../../tools/ksp2-parity-pilot/craft-drop-podtank.json");
    macro_rules! case {
        ($name:literal, $craft:expr, $speed:expr) => {
            (
                $name,
                $craft,
                $speed,
                include_str!(concat!("data/ksp2-drops2/", $name, "/drop-trace.csv")),
                include_str!(concat!("data/ksp2-drops2/", $name, "/drop-parts.csv")),
            )
        };
    }
    for (name, craft, speed, trace, parts) in [
        case!("stack-10", stack, 10.0),
        case!("stack-15", stack, 15.0),
        case!("stack-20", stack, 20.0),
        case!("stack-30", stack, 30.0),
        case!("stack-45", stack, 45.0),
        case!("podtank-25", podtank, 25.0),
        case!("podtank-35", podtank, 35.0),
    ] {
        let (k_left, k_drops) = ksp2(trace, parts);
        let (o_left, o_drops) = ose(craft, speed);
        println!("{name}: KSP2 {k_left} left, broke at {k_drops:.2?}; OSE {o_left} left, broke at {o_drops:.2?}");
        assert_eq!(o_left, k_left, "{name}: parts left");
        // every part KSP2 broke while the trace still ran, at its pace (the trace ends when the craft is lost)
        for (i, k) in k_drops.iter().enumerate() {
            let o = o_drops.get(i).copied().unwrap_or(f64::INFINITY);
            assert!(
                (o - k).abs() <= 0.041,
                "{name}: part {i} broke at {o:.2} vs KSP2 {k:.2} s"
            );
        }
    }
}

//! KSP2 parity for crash tolerance (gap T14) and stack rigidity (gap T13).
//!
//! Replays the KSP2 parity pilot's run 17 drops and run 18 tall stack through the sim with the same craft
//! (`tools/ksp2-parity-pilot/craft-*.json`) and compares against the KSP2 traces in `tests/data` (trimmed
//! copies of `reference/ksp2-run17-drops` and `reference/ksp2-run18-stack`).
//!
//! Drops: KSP2 teleported each craft upright about 1 m above its resting height on the pad with a set
//! vertical speed. We start from KSP2's first airborne tick (height above rest and vertical speed), step at
//! the same 50 Hz, and require the same outcome per part on the same tick: the Mk1 pod alone survives 6.8 and
//! 11.0 m/s and is lost on impact at 15.6 m/s (`crashTolerance` 15); the pod on a 1x2 tank (`crashTolerance`
//! 20) survives 6.8, 11.0 and 15.8 m/s and ends Landed.
//!
//! Stack: pod, six 1x4 tanks and a Swivel (15.9 t), SAS on, 5 s full throttle then 3 s full roll. KSP2 is
//! effectively rigid (engine 0.8 mm axial, under 0.3 mm lateral in the pod's frame, no breaks).
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

fn craft(json: &str) -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

/// The craft standing on flat ground at the equator, as built in the VAB; with Kerbin's spin rate.
fn on_pad(c: &ose_data::craft::Craft) -> (Universe, Entity, f64) {
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
    (u, v, spin)
}

/// Vertical speed over the ground, m/s.
fn vspeed(p: &flight::Powered, spin: f64) -> f64 {
    let ground = DVec3::new(0.0, 0.0, spin).cross(p.r);
    (p.v - ground).dot(p.r.normalized())
}

fn parts(u: &Universe, v: Entity) -> usize {
    u.structure(v)
        .map(|s| s.rigid.present.iter().filter(|&&x| x).count())
        .unwrap_or(0)
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

/// What happened to a dropped craft, counted in ticks from its first airborne tick.
#[derive(Debug)]
struct Drop {
    /// Speed down on the last tick before the ground stopped it, m/s.
    impact_mps: f64,
    /// Tick the vessel was lost, if it was.
    lost_tick: Option<usize>,
    /// Parts left at the end.
    parts_left: usize,
    landed_at_end: bool,
    /// Highest point after the impact, m above the resting height.
    apex_m: f64,
}

/// KSP2's drop, and where it started: height above rest, m, and vertical speed, m/s.
fn ksp2_drop(trace: &str, parts_csv: &str) -> (f64, f64, Drop) {
    let rows = csv(trace);
    let parts = csv(parts_csv);
    let rest = num(rows.last().unwrap(), "alt_m");
    let start = rows
        .iter()
        .position(|r| r["situation"] == "Flying")
        .expect("teleport tick");
    let (h0, v0) = (
        num(&rows[start], "alt_m") - rest,
        num(&rows[start], "vspeed"),
    );
    // the ground stops it on the first tick whose vertical speed jumps up
    let hit = (start + 1..rows.len())
        .find(|&k| num(&rows[k], "vspeed") - num(&rows[k - 1], "vspeed") > 2.0)
        .unwrap_or(rows.len());
    let t0 = num(&rows[start], "t");
    let lost_tick = parts
        .iter()
        .find(|r| r["parts"] == "0")
        .map(|r| ((num(r, "t") - t0) / DT).round() as usize);
    let apex_m = if lost_tick.is_some() {
        0.0
    } else {
        rows[hit..]
            .iter()
            .map(|r| num(r, "alt_m") - rest)
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let drop = Drop {
        impact_mps: -num(&rows[hit - 1], "vspeed"),
        lost_tick,
        parts_left: num(parts.last().unwrap(), "parts") as usize,
        landed_at_end: lost_tick.is_none() && rows.last().unwrap()["situation"] == "Landed",
        apex_m,
    };
    (h0, v0, drop)
}

fn ose_drop(c: &ose_data::craft::Craft, h0: f64, v0: f64, seconds: f64) -> Drop {
    let (mut u, v, spin) = on_pad(c);
    let mut p = u.powered(v).unwrap().clone();
    let rest = p.r.length();
    let up = p.r.normalized();
    p.r = p.r + up * h0;
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + up * v0;
    p.landed = false;
    u.set_powered(v, p);
    let mut d = Drop {
        impact_mps: 0.0,
        lost_tick: None,
        parts_left: parts(&u, v),
        landed_at_end: false,
        apex_m: 0.0,
    };
    let mut last_vs = v0;
    let mut hit = false;
    for k in 1..=((seconds / DT).round() as usize) {
        u.step();
        let lost = u.powered(v).is_none_or(|p| p.crashed);
        if lost {
            d.lost_tick.get_or_insert(k);
        }
        if !hit {
            let vs = u.powered(v).map(|p| vspeed(p, spin)).unwrap_or(0.0);
            if lost || vs - last_vs > 2.0 {
                hit = true;
                d.impact_mps = -last_vs;
            }
            last_vs = vs;
        }
        if hit && !lost {
            d.apex_m = d.apex_m.max(u.powered(v).unwrap().r.length() - rest);
        }
    }
    if d.lost_tick.is_some() {
        d.parts_left = 0;
    } else {
        d.parts_left = parts(&u, v);
        d.landed_at_end = u.powered(v).is_some_and(|p| p.landed);
    }
    d
}

#[test]
fn ksp2_drops_break_and_survive_the_same_parts_on_the_same_tick() {
    let pod = include_str!("../../../tools/ksp2-parity-pilot/craft-drop-pod.json");
    let podtank = include_str!("../../../tools/ksp2-parity-pilot/craft-drop-podtank.json");
    macro_rules! case {
        ($name:literal, $craft:expr) => {
            (
                $name,
                $craft,
                include_str!(concat!("data/ksp2-run17-drops/", $name, "/drop-trace.csv")),
                include_str!(concat!("data/ksp2-run17-drops/", $name, "/drop-parts.csv")),
            )
        };
    }
    for (name, json, trace, parts_csv) in [
        case!("pod-5", pod),
        case!("pod-10", pod),
        case!("pod-15", pod),
        case!("podtank-5", podtank),
        case!("podtank-10", podtank),
        case!("podtank-15", podtank),
    ] {
        let (h0, v0, ksp2) = ksp2_drop(trace, parts_csv);
        let ose = ose_drop(&craft(json), h0, v0, 3.0);
        println!("{name}: from {h0:.2} m at {v0:.2} m/s\n  KSP2 {ksp2:?}\n  OSE  {ose:?}");
        assert!(
            (ose.impact_mps - ksp2.impact_mps).abs() < 0.3,
            "{name}: struck at {:.2} vs KSP2 {:.2} m/s",
            ose.impact_mps,
            ksp2.impact_mps
        );
        match (ksp2.lost_tick, ose.lost_tick) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                assert!(a.abs_diff(b) <= 1, "{name}: lost on tick {b} vs KSP2 {a}")
            }
            (a, b) => panic!("{name}: lost on tick {b:?} vs KSP2 {a:?}"),
        }
        assert_eq!(ose.parts_left, ksp2.parts_left, "{name}: parts left");
        assert_eq!(
            ose.landed_at_end, ksp2.landed_at_end,
            "{name}: Landed at the end"
        );
        // KSP2's rebound is PhysX pushing the hull back out of the pad after it sank up to 30 cm into it within
        // one 20 ms tick, so it depends on where in the tick the craft arrived (apex 0 to 6 cm, and 64 cm once).
        // Our contact stops the hull at the surface; it must never bounce higher than KSP2's.
        assert!(
            ose.apex_m <= ksp2.apex_m.max(0.0) + 0.02,
            "{name}: rebound apex {:.3} vs KSP2 {:.3} m",
            ose.apex_m,
            ksp2.apex_m
        );
    }
}

#[test]
fn ksp2_tall_stack_stays_rigid_under_thrust_and_roll() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-stack.json"
    ));
    let eng = c
        .parts
        .iter()
        .position(|p| p.part_id == "engine_1v_methalox_swivel")
        .unwrap();
    let pod = c
        .parts
        .iter()
        .position(|p| p.part_id == "pod_1v_conical_crew")
        .unwrap();

    // KSP2: the engine's position in the pod's frame through thrust, roll and coast
    let kparts = csv(include_str!("data/ksp2-run18-stack/stack-parts.csv"));
    let engine: Vec<DVec3> = kparts
        .iter()
        .filter(|r| num(r, "index") as usize == eng)
        .map(|r| DVec3::new(num(r, "x"), num(r, "y"), num(r, "z")))
        .collect();
    let rest = engine[0];
    let axis = rest.normalized();
    let (k_axial, k_lateral) = engine
        .iter()
        .map(|p| *p - rest)
        .map(|d| (d.dot(axis).abs(), (d - axis * d.dot(axis)).length()))
        .fold((0.0f64, 0.0f64), |a, b| (a.0.max(b.0), a.1.max(b.1)));
    let k_parts = kparts
        .iter()
        .map(|r| num(r, "parts") as usize)
        .min()
        .unwrap();
    assert_eq!(k_parts, c.parts.len(), "KSP2 broke nothing");
    let ktrace = csv(include_str!("data/ksp2-run18-stack/stack-trace.csv"));
    let rate = |r: &Row| DVec3::new(num(r, "wx"), num(r, "wy"), num(r, "wz")).length();
    let k_rate = |t: f64| {
        rate(
            ktrace
                .iter()
                .min_by(|a, b| (num(a, "t") - t).abs().total_cmp(&(num(b, "t") - t).abs()))
                .unwrap(),
        )
    };
    let k_peak = ktrace.iter().map(rate).fold(0.0, f64::max);

    // OSE: staged on the pad, SAS holding, full throttle; full roll from 5 s to 8 s
    let (mut u, v, _) = on_pad(&c);
    assert!(u.stage(v).fired);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(facing)));
    let ticks = |s: f64| (s * TICKS_PER_SECOND as f64).round() as usize;
    let (mut axial, mut lateral, mut peak) = (0.0f64, 0.0f64, 0.0f64);
    let mut rates = Vec::new();
    for k in 1..=ticks(11.5) {
        if k == ticks(5.0) + 1 {
            u.set_rotation(v, 0.0, 0.0, 1.0);
        }
        if k == ticks(8.0) + 1 {
            u.set_rotation(v, 0.0, 0.0, 0.0);
        }
        u.step();
        assert_eq!(parts(&u, v), c.parts.len(), "a joint broke on tick {k}");
        let p = u.powered(v).unwrap();
        let s = u.structure(v).unwrap();
        // the engine's pose away from rest relative to the pod, from the joint bends (the flex the client draws)
        let a = (s.rigid.rest_position(eng) - s.rigid.rest_position(pod)).normalized();
        let off = s
            .rigid
            .flex(0.0, 0.0)
            .iter()
            .find(|f| f.part == eng)
            .map(|f| f.offset_m)
            .unwrap_or(DVec3::ZERO);
        axial = axial.max(off.dot(a).abs());
        lateral = lateral.max((off - a * off.dot(a)).length());
        let w = (p.angvel + p.facing * p.roll_rate).length();
        peak = peak.max(w);
        rates.push((k as f64 * DT, w));
    }
    println!(
        "engine flex axial {:.2} mm (KSP2 {:.2}), lateral {:.2} mm (KSP2 {:.2}); peak roll {peak:.3} (KSP2 {k_peak:.3}) rad/s",
        axial * 1e3,
        k_axial * 1e3,
        lateral * 1e3,
        k_lateral * 1e3
    );
    // near-zero flex, as KSP2's (visible wobble would be centimetres)
    assert!(axial < 2e-3 && lateral < 1e-3, "the stack flexed");
    assert!(
        (peak - k_peak).abs() < 0.05 * k_peak,
        "peak roll rate {peak:.3} vs KSP2 {k_peak:.3}"
    );
    // SAS takes the roll out after release at KSP2's pace
    for t in [9.0, 10.0, 11.0] {
        let w = rates
            .iter()
            .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
            .unwrap()
            .1;
        let k = k_rate(t);
        assert!(
            (w - k).abs() < 0.1 * k.max(0.1),
            "roll rate at {t} s: {w:.3} vs KSP2 {k:.3}"
        );
    }
}

//! KSP2 parity against the pilot's reentry and live centre-of-mass runs (`tools/ksp2-parity-pilot/
//! run-reentry-com.ps1`; trimmed traces in `tests/data/ksp2-reentry-heatshield` and `tests/data/ksp2-com-ascent`).
use std::collections::HashMap;
use std::f64::consts::TAU;

use bevy_ecs::entity::Entity;
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

fn craft(json: &str) -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

/// The craft on the pad at the equator of a real Kerbin (atmosphere and all), its craft riding along.
fn on_pad(c: &ose_data::craft::Craft) -> (Universe, Entity, f64, f64) {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let (spin, radius) = (TAU / kerbin.rotation_period_s, kerbin.radius_m);
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(c, &defs, &res));
    u.set_craft(v, c.clone());
    (u, v, spin, radius)
}

#[test]
fn ksp2_heat_shield_reentry_from_70_km_at_2300_mps() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-reentry.json"
    ));
    let trace = csv(include_str!(
        "data/ksp2-reentry-heatshield/reentry-trace.csv"
    ));
    let parts = csv(include_str!(
        "data/ksp2-reentry-heatshield/reentry-parts.csv"
    ));
    let start = trace
        .iter()
        .position(|r| r["situation"] != "PreLaunch")
        .unwrap();
    let t0 = num(&trace[start], "t");
    let (mut u, v, spin, radius) = on_pad(&c);
    let shield = c
        .parts
        .iter()
        .position(|p| p.part_id == "heatshield_1v")
        .unwrap();
    let pod = c
        .parts
        .iter()
        .position(|p| p.part_id == "pod_1v_conical_crew")
        .unwrap();
    // SAS on, upright, put where KSP2's teleport put it
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    p.r = up * (radius + num(&trace[start], "alt_m"));
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + up * num(&trace[start], "vspeed");
    p.landed = false;
    u.set_powered(v, p);
    let facing = u.powered(v).unwrap().facing;
    assert!(u.set_controls(v, 0.0, flight::Attitude::Fixed(facing)));
    let ablator0 = |u: &Universe| {
        let st = u.structure(v).unwrap();
        let r = st.net.resource("Ablator").unwrap();
        st.net
            .tanks
            .iter()
            .filter(|t| t.part == shield && t.resource == r)
            .map(|t| t.stored)
            .sum::<f64>()
    };
    let units0 = ablator0(&u);
    // KSP2 at time t since the teleport, from the trace (its per-part log is written once a frame, so its
    // clock runs behind; that file is used only for the ablator): the hottest part is the shield
    let at = |t: f64| {
        trace[start..]
            .iter()
            .min_by(|a, b| {
                (num(a, "t") - t0 - t)
                    .abs()
                    .total_cmp(&(num(b, "t") - t0 - t).abs())
            })
            .unwrap()
            .clone()
    };
    let (mut peak, mut ksp2_peak, mut pod_peak) = (0.0f64, 0.0f64, 0.0f64);
    let mut rows = Vec::new();
    for k in 1..=(52.0 / DT) as usize {
        u.step();
        let Some(p) = u.powered(v) else { break };
        if p.crashed || p.landed {
            break;
        }
        let st = u.structure(v).unwrap();
        let t = k as f64 * DT;
        let ts = st.temperature_k[shield];
        peak = peak.max(ts);
        pod_peak = pod_peak.max(st.temperature_k[pod]);
        if k % 250 == 0 {
            let ks = at(t);
            let alt = p.r.length() - radius;
            rows.push((t, alt, num(&ks, "alt_m"), ts, num(&ks, "temp_max_k")));
        }
    }
    for r in &trace[start..] {
        ksp2_peak = ksp2_peak.max(num(r, "temp_max_k"));
    }
    let used = units0 - ablator0(&u);
    let ksp2_used = 0.2
        - parts
            .iter()
            .filter(|r| r["part"] == "heatshield_1v")
            .map(|r| num(r, "ablator_units"))
            .fold(f64::INFINITY, f64::min);
    for r in &rows {
        println!(
            "t {:5.1}: alt {:7.0} (KSP2 {:7.0})  shield {:6.0} K (KSP2 {:6.0})",
            r.0, r.1, r.2, r.3, r.4
        );
        // the same fall and the same heating, second by second
        assert!(
            (r.1 - r.2).abs() < 0.02 * r.2.max(5000.0),
            "t {}: alt {:.0} vs {:.0}",
            r.0,
            r.1,
            r.2
        );
        assert!(
            (r.3 - r.4).abs() < 0.06 * r.4 + 10.0,
            "t {}: shield {:.0} vs {:.0} K",
            r.0,
            r.3,
            r.4
        );
    }
    println!(
        "shield peak {peak:.0} K (KSP2 {ksp2_peak:.0}), pod peak {pod_peak:.0} K, ablator used {used:.5} (KSP2 {ksp2_used:.5})"
    );
    assert!(
        (peak - ksp2_peak).abs() < 0.05 * ksp2_peak,
        "shield peak {peak:.0} vs {ksp2_peak:.0} K"
    );
    assert!(
        pod_peak < 300.0,
        "the pod stays cool behind its shield ({pod_peak:.0} K)"
    );
    assert!(
        (used - ksp2_used).abs() < 0.25 * ksp2_used,
        "ablator used {used:.5} vs KSP2 {ksp2_used:.5}"
    );
}

#[test]
fn ksp2_live_centre_of_mass_tracks_the_draining_tanks() {
    let c = craft(include_str!("data/ksp2-com-ascent/craft.json"));
    let trace = csv(include_str!("data/ksp2-com-ascent/com-trace.csv"));
    let (mut u, v, _, _) = on_pad(&c);
    assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
    u.stage(v);
    // KSP2's centre of mass (root frame, Unity) against its resource mass
    let ksp2_at = |res: f64| {
        let r = trace
            .iter()
            .min_by(|a, b| {
                (num(a, "resource_t") - res)
                    .abs()
                    .total_cmp(&(num(b, "resource_t") - res).abs())
            })
            .unwrap();
        (num(r, "resource_t"), num(r, "com_y"))
    };
    let mut worst = 0.0f64;
    for k in 0..(40 * TICKS_PER_SECOND) {
        let st = u.structure(v).unwrap();
        let res: f64 = st
            .net
            .tanks
            .iter()
            .map(|t| t.stored * st.net.unit_mass_t[t.resource])
            .sum();
        let ose_y = st.mass_properties().centre_m.y;
        let (kres, ky) = ksp2_at(res);
        if (kres - res).abs() < 0.01 {
            worst = worst.max((ose_y - ky).abs());
            if k % 250 == 0 {
                println!("resource {res:.3} t: CoM y {ose_y:.4} (KSP2 {ky:.4} at {kres:.3} t)");
            }
        }
        u.step();
    }
    println!("worst CoM gap {:.1} mm", worst * 1e3);
    assert!(worst < 0.02, "CoM off by {:.1} mm", worst * 1e3);
}

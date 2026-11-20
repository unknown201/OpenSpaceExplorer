//! KSP2 parity for a plane's control surfaces in flight (T10), against the parity pilot's `jetfly` run of the
//! stock jet (`tools/ksp2-parity-pilot/Jet.cs`, run 2026-11-20 with every airflow force and each control
//! surface's deflection logged; the pitch window trimmed into `tests/data/ksp2-jetfly-pitch`).
//!
//! With the same airflow and deflections OSE's lift and drag on every part matched KSP2's; what differed
//! was how far the elevons went. A KSP2 craft file gives `AuthorityLimiter` in degrees of the part's range
//! and the importer leaves it negative for the part definition to resolve, which only the stock-craft
//! importer did: a craft read at runtime lost its limit (and which axes each surface answers) and flew at
//! full range, twice KSP2's elevon throw on the stock jet.
use std::collections::HashMap;

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
    r[k].parse().unwrap_or(f64::NAN)
}

/// The stock jet as a KSP2 craft file reads at runtime, shaped as the server shapes it.
fn jet() -> (
    ose_data::craft::Craft,
    Vec<ose_data::PartDef>,
    Vec<ose_data::ResourceDef>,
) {
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
    (c, defs, res)
}

fn kerbin_with(c: &ose_data::craft::Craft) -> (Universe, bevy_ecs::entity::Entity, f64, f64) {
    let (_, defs, res) = jet();
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let (spin, radius) = (
        std::f64::consts::TAU / kerbin.rotation_period_s,
        kerbin.radius_m,
    );
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
fn a_ksp2_craft_files_surface_limits_survive_a_runtime_import() {
    let (c, _, _) = jet();
    let (mut u, v, _, _) = kerbin_with(&c);
    u.step();
    let p = u.powered(v).unwrap();
    for (i, part) in c.parts.iter().enumerate() {
        let cfg = p.part_control(i);
        if part.part_id.starts_with("wing") {
            // AuthorityLimiter 15 of the wing's 30 degrees; pitch and roll, not yaw
            assert!((cfg.surface_authority - 0.5).abs() < 1e-9, "{cfg:?}");
            assert_eq!(cfg.surface_axes, [true, true, false]);
        } else if part.part_id.starts_with("stabilizer") {
            // 15 of the stabilizer's 15: all of it, yaw only
            assert!((cfg.surface_authority - 1.0).abs() < 1e-9, "{cfg:?}");
            assert_eq!(cfg.surface_axes, [false, false, true]);
        }
    }
}

/// KSP2's control-transform axis `axis` at the row's attitude, as components along the surface velocity,
/// the vertical square to it, and the side.
fn ksp2_axis(r: &Row, axis: DVec3) -> DVec3 {
    let (x, y, z, w) = (num(r, "qx"), num(r, "qy"), num(r, "qz"), num(r, "qw"));
    let u = DVec3::new(x, y, z);
    let t = u.cross(axis) * 2.0;
    let v = axis + t * w + u.cross(t);
    let along = DVec3::new(num(r, "vx"), num(r, "vy"), num(r, "vz")).normalized();
    let down = DVec3::new(num(r, "gx"), num(r, "gy"), num(r, "gz")).normalized();
    let side = along.cross(-down).normalized();
    let up = side.cross(along);
    DVec3::new(v.dot(along), v.dot(up), v.dot(side))
}

/// From KSP2's state 0.5 s before the stock jet's pitch -0.5 (250 m/s, climbing at 6.4 degrees, SAS holding),
/// OSE flies the same input: the elevons go to the same -7.19 degrees and the nose follows KSP2's down to
/// -9 degrees and stops there (it went on to -19 before the limit was resolved).
#[test]
fn ksp2_jet_answers_a_half_pitch_input_as_in_ksp2() {
    let (c, _, _) = jet();
    let (mut u, v, spin, radius) = kerbin_with(&c);
    let trace = csv(include_str!("data/ksp2-jetfly-pitch/trace.csv"));
    let surfaces = csv(include_str!("data/ksp2-jetfly-pitch/surfaces.csv"));
    let t0 = trace
        .iter()
        .find(|r| r["phase"] == "pitch")
        .map(|r| num(r, "t"))
        .unwrap();
    let at = |rows: &[Row], t: f64| -> Row {
        rows.iter()
            .min_by(|a, b| (num(a, "t") - t).abs().total_cmp(&(num(b, "t") - t).abs()))
            .unwrap()
            .clone()
    };
    let start_t = t0 - 0.5;
    let (s0, tr0) = (at(&surfaces, start_t), at(&trace, start_t));
    // the same height, speed and climb, heading east; the craft turned so its control frame (+Y nose, its
    // up-facing axis on the runway as top) sits as KSP2's control transform did (+Y nose, -Z top)
    let mut p = u.powered(v).unwrap().clone();
    let upv = p.r.normalized();
    let east = DVec3::new(0.0, 0.0, 1.0).cross(upv).normalized();
    let (speed, vs) = (num(&tr0, "srf_speed"), num(&tr0, "vspeed"));
    let srf = upv * vs + east * (speed * speed - vs * vs).max(0.0).sqrt();
    let along = srf.normalized();
    let side = along.cross(upv).normalized();
    let up = side.cross(along);
    let map = |c: DVec3| (along * c.x + up * c.y + side * c.z).normalized();
    let nose = map(ksp2_axis(&s0, DVec3::new(0.0, 1.0, 0.0)));
    let top = map(ksp2_axis(&s0, DVec3::new(0.0, 0.0, -1.0)));
    let (cx, cy, cz) = flight::control_basis(&p);
    let ctop = [cx, -cx, cz, -cz]
        .into_iter()
        .max_by(|a, b| a.dot(upv).total_cmp(&b.dot(upv)))
        .unwrap();
    let frame = |a: DVec3, b: DVec3| {
        let b = (b - a * b.dot(a)).normalized();
        (a, b, a.cross(b))
    };
    let (a1, a2, a3) = frame(cy, ctop);
    let (b1, b2, b3) = frame(nose, top);
    let turn = |v: DVec3| b1 * v.dot(a1) + b2 * v.dot(a2) + b3 * v.dot(a3);
    let old_top = flight::vessel_top(&p);
    p.facing = turn(p.facing).normalized();
    p.r = upv * (radius + num(&tr0, "alt_m"));
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + srf;
    p.angvel = DVec3::ZERO;
    p.roll_rate = 0.0;
    p.landed = false;
    flight::set_vessel_top(&mut p, turn(old_top));
    u.set_powered(v, p);
    let hold = flight::control_basis(u.powered(v).unwrap()).1;
    assert!(u.set_controls(v, 0.0, flight::Attitude::Fixed(hold)));
    let pitch = |p: &flight::Powered| {
        flight::control_basis(p)
            .1
            .dot(p.r.normalized())
            .asin()
            .to_degrees()
    };
    let dt = 1.0 / TICKS_PER_SECOND as f64;
    for k in 1..=(1.9 / dt) as usize {
        let t = start_t + k as f64 * dt;
        let input = if (t0..t0 + 1.5).contains(&t) {
            -0.5
        } else {
            0.0
        };
        u.set_rotation(v, input, 0.0, 0.0);
        u.step();
        let rel = t - t0;
        if k % 10 == 0 && (0.2..=1.4).contains(&rel) {
            let p = u.powered(v).unwrap();
            let kt = at(&trace, t);
            let ks = at(&surfaces, t);
            let (ose_pitch, ksp2_pitch) = (pitch(p), num(&kt, "pitch_deg"));
            let deflection = -u.structure(v).unwrap().surface_angle(3).to_degrees();
            println!(
                "{rel:+5.2} s: pitch {ose_pitch:7.2} (KSP2 {ksp2_pitch:7.2}), elevon {deflection:6.2} (KSP2 {:6.2})",
                num(&ks, "deflection_deg")
            );
            assert!(
                (deflection - num(&ks, "deflection_deg")).abs() < 0.3,
                "{rel:.2} s: elevon {deflection:.2} vs KSP2 {:.2}",
                num(&ks, "deflection_deg")
            );
            assert!(
                (ose_pitch - ksp2_pitch).abs() < 1.6,
                "{rel:.2} s: pitch {ose_pitch:.2} vs KSP2 {ksp2_pitch:.2}"
            );
        }
    }
}

/// Put the jet where KSP2's was at `t` (height, speed, climb, heading east, control frame as KSP2's control
/// transform) and hold that attitude with SAS. Returns the time.
fn place_as_ksp2(
    u: &mut Universe,
    v: bevy_ecs::entity::Entity,
    spin: f64,
    radius: f64,
    surfaces: &[Row],
    trace: &[Row],
    t: f64,
) {
    let at = |rows: &[Row], t: f64| -> Row {
        rows.iter()
            .min_by(|a, b| (num(a, "t") - t).abs().total_cmp(&(num(b, "t") - t).abs()))
            .unwrap()
            .clone()
    };
    let (s0, tr0) = (at(surfaces, t), at(trace, t));
    let mut p = u.powered(v).unwrap().clone();
    let upv = p.r.normalized();
    let east = DVec3::new(0.0, 0.0, 1.0).cross(upv).normalized();
    let (speed, vs) = (num(&tr0, "srf_speed"), num(&tr0, "vspeed"));
    let srf = upv * vs + east * (speed * speed - vs * vs).max(0.0).sqrt();
    let along = srf.normalized();
    let side = along.cross(upv).normalized();
    let up = side.cross(along);
    let map = |c: DVec3| (along * c.x + up * c.y + side * c.z).normalized();
    let nose = map(ksp2_axis(&s0, DVec3::new(0.0, 1.0, 0.0)));
    let top = map(ksp2_axis(&s0, DVec3::new(0.0, 0.0, -1.0)));
    let (cx, cy, cz) = flight::control_basis(&p);
    let ctop = [cx, -cx, cz, -cz]
        .into_iter()
        .max_by(|a, b| a.dot(upv).total_cmp(&b.dot(upv)))
        .unwrap();
    let frame = |a: DVec3, b: DVec3| {
        let b = (b - a * b.dot(a)).normalized();
        (a, b, a.cross(b))
    };
    let (a1, a2, a3) = frame(cy, ctop);
    let (b1, b2, b3) = frame(nose, top);
    let turn = |v: DVec3| b1 * v.dot(a1) + b2 * v.dot(a2) + b3 * v.dot(a3);
    let old_top = flight::vessel_top(&p);
    p.facing = turn(p.facing).normalized();
    p.r = upv * (radius + num(&tr0, "alt_m"));
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + srf;
    p.angvel = DVec3::ZERO;
    p.roll_rate = 0.0;
    p.landed = false;
    flight::set_vessel_top(&mut p, turn(old_top));
    u.set_powered(v, p);
    let hold = flight::control_basis(u.powered(v).unwrap()).1;
    assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(hold)));
}

/// The climb after rotation, SAS holding: from KSP2's state at the start of the climb (68 m/s, nose 10.8
/// degrees up, full throttle) SAS trims the elevons as KSP2's does (within half a degree once settled) and the
/// jet settles a little flatter (about 4 degrees against KSP2's 6, a difference from the first seconds' SAS
/// transient; it held about 12 before the authority limit was resolved).
#[test]
fn ksp2_jet_climb_hold_trims_the_elevons_as_in_ksp2() {
    let (c, _, _) = jet();
    let (mut u, v, spin, radius) = kerbin_with(&c);
    let trace = csv(include_str!("data/ksp2-jetfly-climb/trace.csv"));
    let surfaces = csv(include_str!("data/ksp2-jetfly-climb/surfaces.csv"));
    let t0 = trace
        .iter()
        .find(|r| r["phase"] == "climb")
        .map(|r| num(r, "t"))
        .unwrap();
    // light the engine and let it spool up on the runway, brakes on, before the jump to KSP2's state
    assert!(u.set_controls(v, 1.0, flight::Attitude::Free));
    u.stage(v);
    for _ in 0..(20 * TICKS_PER_SECOND) {
        u.step();
    }
    place_as_ksp2(&mut u, v, spin, radius, &surfaces, &trace, t0);
    let at = |rows: &[Row], t: f64| -> Row {
        rows.iter()
            .min_by(|a, b| (num(a, "t") - t).abs().total_cmp(&(num(b, "t") - t).abs()))
            .unwrap()
            .clone()
    };
    let dt = 1.0 / TICKS_PER_SECOND as f64;
    for k in 1..=(19.0 / dt) as usize {
        let t = t0 + k as f64 * dt;
        u.step();
        if k % 50 == 0 {
            let p = u.powered(v).unwrap();
            let kt = at(&trace, t);
            let pitch = flight::control_basis(p)
                .1
                .dot(p.r.normalized())
                .asin()
                .to_degrees();
            let srf = p.v - DVec3::new(0.0, 0.0, spin).cross(p.r);
            let elevon = -u.structure(v).unwrap().surface_angle(3).to_degrees();
            if t - t0 >= 3.0 {
                let ksp2 = num(&at(&surfaces, t), "deflection_deg");
                assert!(
                    (elevon - ksp2).abs() < 0.5,
                    "{:.1} s: elevon {elevon:.2} vs KSP2 {ksp2:.2}",
                    t - t0
                );
                assert!(
                    (pitch - num(&kt, "pitch_deg")).abs() < 3.0,
                    "{:.1} s: pitch {pitch:.2} vs KSP2 {:.2}",
                    t - t0,
                    num(&kt, "pitch_deg")
                );
            }
            println!(
                "climb {:+5.1} s: pitch {:6.2} (KSP2 {:6.2}) speed {:6.1} ({:6.1}) vs {:6.1} ({:6.1}) elevon {:6.2} (KSP2 {:6.2})",
                t - t0,
                pitch,
                num(&kt, "pitch_deg"),
                srf.length(),
                num(&kt, "srf_speed"),
                srf.dot(p.r.normalized()),
                num(&kt, "vspeed"),
                elevon,
                num(&at(&surfaces, t), "deflection_deg"),
            );
        }
    }
}

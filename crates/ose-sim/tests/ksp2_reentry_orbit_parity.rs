//! KSP2 parity for reentry from orbit (T11): craft come down from an 80 x 30 km Kerbin orbit with SAS holding
//! retrograde (`tools/ksp2-parity-pilot/run-reentry-orbit.ps1`, `ReentryOrbit.cs`; trimmed by
//! `trim_reentry_orbit.py` into `tests/data/ksp2-reentryorbit-*`).
//!
//! Each test starts OSE where KSP2's craft passed 71 km on the way down (its height, vertical and horizontal
//! speed and part temperatures) and replays KSP2's attitude tick by tick, so what is compared is the physics
//! of the fall and not the two games' SAS: speed against altitude as the craft slows from orbital speed in
//! thickening air, every part's temperature, and the parts that burn up.
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
    r[k].parse().unwrap_or(f64::NAN)
}

fn craft(json: &str) -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut c = ose_importer::craft::craft_from_value(&v).unwrap();
    // as the server takes a craft at launch: shaped wings and fairings weigh what their shapes do
    let defs = catalog().0;
    c.apply_shapes(&defs);
    // the fairing craft was written by script, not built in KSP2's editor, so its fairing carried no panel
    // mass (`MassModifierAmount` 0 in the file; KSP2's craft weighed 1.0675 t, pod and fairing base, all the
    // way down)
    for p in &mut c.parts {
        if ose_data::fairing::def(&p.part_id).is_some() {
            if let (Some(f), Some(d)) = (&mut p.flight, defs.iter().find(|d| d.id == p.part_id)) {
                f.dry_mass_t = Some(d.mass_t);
            }
        }
    }
    c
}

/// One KSP2 run, trimmed.
struct Run {
    start: Row,
    attitude: Vec<Row>,
    parts: Vec<Row>,
    events: Vec<Row>,
}

macro_rules! run {
    ($dir:literal) => {
        Run {
            start: csv(include_str!(concat!("data/", $dir, "/start.csv"))).remove(0),
            attitude: csv(include_str!(concat!("data/", $dir, "/attitude.csv"))),
            parts: csv(include_str!(concat!("data/", $dir, "/parts.csv"))),
            events: csv(include_str!(concat!("data/", $dir, "/events.csv"))),
        }
    };
}

/// One second of a flight: altitude, surface speed and each part's temperature (craft order; `None` once gone),
/// and how much of it the airflow reached and the re-entry heat it took, kW (`None` where not logged).
#[derive(Clone, Debug)]
struct Sample {
    t: f64,
    alt: f64,
    speed: f64,
    temps: Vec<Option<f64>>,
    exposed: Vec<Option<f64>>,
    flux: Vec<Option<f64>>,
}

#[derive(Debug)]
struct Flight {
    samples: Vec<Sample>,
    /// (time, altitude, part index) of each part lost.
    lost: Vec<(f64, f64, usize)>,
    /// Hottest each part got, K.
    peak: Vec<f64>,
    /// Whether the root burned up (the whole craft lost).
    destroyed: bool,
}

/// KSP2's part order for a craft (`PartOwner.Parts`): depth first from the root, each part's surface-attached
/// children first (in craft order), then what hangs on its bottom node, then the rest.
fn ksp2_part_order(c: &ose_data::craft::Craft) -> Vec<usize> {
    fn visit(c: &ose_data::craft::Craft, i: usize, order: &mut Vec<usize>) {
        order.push(i);
        let rank = |j: usize| match c.parts[j].parent_node.as_deref() {
            None | Some("") => 0,
            Some(n) if n.starts_with("bottom") => 1,
            _ => 2,
        };
        let mut children: Vec<usize> = (0..c.parts.len())
            .filter(|&j| c.parts[j].parent == Some(i))
            .collect();
        children.sort_by_key(|&j| rank(j));
        for j in children {
            visit(c, j, order);
        }
    }
    let mut order = Vec::new();
    if let Some(root) = c.parts.iter().position(|p| p.parent.is_none()) {
        visit(c, root, &mut order);
    }
    order
}

/// KSP2's run as a `Flight`, times from the start row.
fn ksp2_flight(run: &Run, c: &ose_data::craft::Craft) -> Flight {
    let limits = &limits(c);
    let n = limits.len();
    let t0 = num(&run.start, "t");
    let names: Vec<&str> = run.events[0]["names"].split(' ').collect();
    assert_eq!(names.len(), n, "KSP2's craft has the craft file's parts");
    // KSP2 logs the parts still there in its own order (`PartOwner.Parts`): from the root, depth first,
    // each part's children latest attached first. `alive` maps its index to the craft's.
    let mut alive: Vec<usize> = ksp2_part_order(c);
    assert_eq!(
        alive
            .iter()
            .map(|&i| c.parts[i].part_id.as_str())
            .collect::<Vec<_>>(),
        names,
        "KSP2's part order"
    );
    let names: Vec<&str> = c.parts.iter().map(|p| p.part_id.as_str()).collect();
    let mut lost = Vec::new();
    let mut pending = run.events[1..]
        .iter()
        .filter(|e| !e["gone"].is_empty() && e["gone"] != "vessel");
    let mut next_event = pending.next();
    let mut samples: Vec<Sample> = Vec::new();
    let mut peak = vec![0.0f64; n];
    for r in &run.parts {
        let t = num(r, "t") - t0;
        while let Some(e) = next_event.filter(|e| num(e, "t") - t0 <= t) {
            for name in e["gone"].split(' ') {
                if let Some(k) = alive.iter().position(|&i| names[i] == name) {
                    lost.push((num(e, "t") - t0, num(e, "alt_m"), alive.remove(k)));
                }
            }
            next_event = pending.next();
        }
        if samples.last().map(|s| s.t) != Some(t) {
            samples.push(Sample {
                t,
                alt: num(r, "alt_m"),
                speed: num(r, "srf_speed"),
                temps: vec![None; n],
                exposed: vec![None; n],
                flux: vec![None; n],
            });
        }
        let idx: usize = r["index"].parse().unwrap();
        if let Some(&i) = alive.get(idx) {
            let temp = num(r, "temp_k");
            let s = samples.last_mut().unwrap();
            s.temps[i] = Some(temp);
            s.exposed[i] = r.get("exposed").map(|_| num(r, "exposed"));
            s.flux[i] = r.get("reentry_flux").map(|_| num(r, "reentry_flux"));
            if t >= 0.0 {
                peak[i] = peak[i].max(temp);
            }
        }
    }
    let last = run.events.last().unwrap();
    // the vessel gone with a part at its limit burned up; otherwise it hit the ground (the runs end there)
    let hot = samples.last().is_some_and(|s| {
        s.temps
            .iter()
            .zip(limits)
            .any(|(t, max)| t.is_some_and(|t| t >= 0.98 * max))
    });
    let destroyed = last["gone"] == "vessel" && hot;
    if destroyed {
        // the root burned up: everything left went with it
        let alt = samples.last().map_or(f64::NAN, |s| s.alt);
        for i in alive {
            lost.push((num(last, "t") - t0, alt, i));
        }
    }
    samples.retain(|s| s.t >= 0.0);
    Flight {
        samples,
        lost,
        peak,
        destroyed,
    }
}

/// OSE flying the same fall: Kerbin with its atmosphere, the craft put where KSP2's was at the start row
/// (height, vertical and horizontal surface speed, part temperatures) and turned each tick to KSP2's attitude.
fn ose_flight(c: &ose_data::craft::Craft, run: &Run, seconds: f64, replay: bool) -> Flight {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let (spin, radius) = (TAU / kerbin.rotation_period_s, kerbin.radius_m);
    let (defs, res) = catalog();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v: Entity = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    let n = c.parts.len();
    let ksp2 = ksp2_flight(run, c);
    let mut st = Structure::build(c, &defs, &res);
    for (i, t) in ksp2.samples[0].temps.iter().enumerate() {
        st.temperature_k[i] = t.unwrap();
    }
    u.set_structure(v, st);
    u.set_craft(v, c.clone());

    let start = &run.start;
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    let east = DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
    let vs = num(start, "vspeed");
    let hs = (num(start, "srf_speed").powi(2) - vs * vs).max(0.0).sqrt();
    p.r = up * (radius + num(start, "alt_m"));
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) + up * vs + east * hs;
    p.landed = false;
    if !replay {
        // left to the airflow: KSP2's attitude and turn rate at the start, then nothing holds it
        let v3 =
            |a: &str, b: &str, c: &str| DVec3::new(num(start, a), num(start, b), num(start, c));
        let (x, y, z, w) = (
            num(start, "qx"),
            num(start, "qy"),
            num(start, "qz"),
            num(start, "qw"),
        );
        let nose = DVec3::new(
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        );
        let k_along = v3("vx", "vy", "vz").normalized();
        let k_side = k_along
            .cross(-v3("gx", "gy", "gz").normalized())
            .normalized();
        let k_up = k_side.cross(k_along);
        let along = p.v.normalized();
        let side = along.cross(p.r.normalized()).normalized();
        let up = side.cross(along);
        let to_ose = |q: DVec3| along * q.dot(k_along) + up * q.dot(k_up) + side * q.dot(k_side);
        p.facing = to_ose(nose).normalized();
        let rate = to_ose(v3("wx", "wy", "wz"));
        p.roll_rate = rate.dot(p.facing);
        p.angvel = rate - p.facing * p.roll_rate;
    }
    u.set_powered(v, p);
    assert!(u.set_controls(v, 0.0, flight::Attitude::Free));

    // KSP2's nose as components along the orbital velocity, the vertical square to it, and the side
    let t0 = num(start, "t");
    let attitude: Vec<(f64, DVec3, Option<DVec3>)> = run
        .attitude
        .iter()
        .map(|r| {
            (
                num(r, "t") - t0,
                DVec3::new(num(r, "along"), num(r, "up"), num(r, "side")),
                r.contains_key("x_along")
                    .then(|| DVec3::new(num(r, "x_along"), num(r, "x_up"), num(r, "x_side"))),
            )
        })
        .collect();
    // KSP2's frame is left-handed: its side (along x up, worked out in its own axes) points the other way
    let side_sign = -1.0;
    let mut samples = Vec::new();
    let mut lost = Vec::new();
    let mut peak = vec![0.0f64; n];
    let mut was = vec![true; n];
    let mut destroyed = false;
    for k in 1..=(seconds / DT) as usize {
        if replay {
            let t = (k - 1) as f64 * DT;
            let (_, a, x) = attitude[attitude
                .partition_point(|x| x.0 < t - 1e-6)
                .min(attitude.len() - 1)];
            let mut p = u.powered(v).unwrap().clone();
            let along = p.v.normalized();
            let side = along.cross(p.r.normalized()).normalized() * side_sign;
            let up = side.cross(along) * side_sign;
            let world = |c: DVec3| along * c.x + up * c.y + side * c.z;
            let nose = world(a).normalized();
            p.facing = nose;
            if let Some(x) = x {
                // KSP2's control transform (its x and y axes, the nose) made the root's control frame here: the
                // vessel's own axes follow through the root's control rotation (z is mirrored, as with every
                // KSP2 rotation brought in)
                let cx = world(x);
                let cx = (cx - nose * cx.dot(nose)).normalized();
                let cz = cx.cross(nose);
                let q = p.control_rotation;
                let to_world = |e: DVec3| {
                    let l = ose_sim::aero::rotate_inverse(q, e);
                    cx * l.x + nose * l.y + cz * l.z
                };
                p.facing = to_world(DVec3::new(0.0, 1.0, 0.0)).normalized();
                flight::set_vessel_top(&mut p, to_world(DVec3::new(0.0, 0.0, 1.0)));
            }
            p.angvel = DVec3::ZERO;
            u.set_powered(v, p);
        }
        u.step();
        let t = k as f64 * DT;
        let p = u.powered(v).unwrap();
        let alt = p.r.length() - radius;
        if p.crashed {
            let st = u.structure(v).unwrap();
            // burned up (a part past its limit), not a crash into the ground
            destroyed = (0..n).any(|i| st.temperature_k[i] >= 0.98 * st.max_temp_k[i]);
            if !destroyed {
                break;
            }
            for (i, w) in was.iter().enumerate() {
                if *w {
                    lost.push((t, alt, i));
                }
            }
            break;
        }
        let st = u.structure(v).unwrap();
        let mut temps = vec![None; n];
        for i in 0..n {
            if st.rigid.present[i] {
                temps[i] = Some(st.temperature_k[i]);
                peak[i] = peak[i].max(st.temperature_k[i]);
            } else if was[i] {
                lost.push((t, alt, i));
            }
            was[i] = st.rigid.present[i];
        }
        if k % TICKS_PER_SECOND as usize == 0 {
            let srf = p.v - DVec3::new(0.0, 0.0, spin).cross(p.r);
            let here = |x: &[f64]| (0..n).map(|i| temps[i].map(|_| x[i])).collect();
            samples.push(Sample {
                t,
                alt,
                speed: srf.length(),
                exposed: here(&st.exposed),
                flux: here(&st.reentry_flux_kw),
                temps,
            });
        }
        if p.landed || p.splashed {
            break;
        }
    }
    Flight {
        samples,
        lost,
        peak,
        destroyed,
    }
}

fn catalog() -> (Vec<ose_data::PartDef>, Vec<ose_data::ResourceDef>) {
    (
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
    )
}

/// Each craft part's maximum temperature, K.
fn limits(c: &ose_data::craft::Craft) -> Vec<f64> {
    let (defs, _) = catalog();
    c.parts
        .iter()
        .map(|p| {
            defs.iter()
                .find(|d| d.id == p.part_id)
                .map_or(f64::MAX, |d| d.max_temp_k)
        })
        .collect()
}

/// The speed a flight had when it passed `alt` (within 30 s of `t`; a craft that skims back up passes a
/// height twice).
fn speed_at_height(f: &Flight, alt: f64, t: f64) -> Option<f64> {
    f.samples
        .windows(2)
        .filter(|w| (w[0].t - t).abs() < 30.0)
        .filter(|w| (w[0].alt - alt) * (w[1].alt - alt) <= 0.0 && w[0].alt != w[1].alt)
        .min_by(|a, b| (a[0].t - t).abs().total_cmp(&(b[0].t - t).abs()))
        .map(|w| {
            let s = (w[0].alt - alt) / (w[0].alt - w[1].alt);
            w[0].speed + s * (w[1].speed - w[0].speed)
        })
}

fn at(f: &Flight, t: f64) -> Option<&Sample> {
    f.samples
        .iter()
        .min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs()))
        .filter(|s| (s.t - t).abs() <= 1.0)
}

/// Fly OSE alongside KSP2's run and hold the two to each other every 10 s: the same height (within 2%, and
/// 400 m: a tumbling craft replayed from attitude sampled every 0.1 s moves it a few hundred metres), the
/// falling speed at each height (within 1%; below 21.5 km that takes KSP2's rotating physics frame,
/// `flight::rotating_frame_error`), every part's temperature (within 4% + 8 K), and the
/// same parts lost at about the same time.
fn compare(name: &str, c: &ose_data::craft::Craft, run: &Run) {
    compare_until(name, c, run, f64::INFINITY);
}

/// When KSP2's craft first swings more than 5 degrees off retrograde, s from the start row: SAS lost it and it
/// starts to swing and then tumble (KSP2's K1, K2 and the jet between 30 and 38 km).
fn tumbles_at(run: &Run) -> f64 {
    let t0 = num(&run.start, "t");
    run.attitude
        .iter()
        .map(|r| (num(r, "t") - t0, num(r, "along")))
        .find(|(t, along)| *t >= 0.0 && *along > -(5f64.to_radians().cos()))
        .map_or(f64::INFINITY, |(t, _)| t)
}

/// `compare` up to `until` s: the parts' heat and the fall are held while the craft holds retrograde. Past that
/// it tumbles, which the replay (attitude only, every 0.1 s) does not carry well enough to hold a part's
/// temperature or the speed to these tolerances (`docs/ksp2-side-by-side-gaps.md` T13), so peaks and losses
/// are not held.
fn compare_until(name: &str, c: &ose_data::craft::Craft, run: &Run, until: f64) {
    let n = c.parts.len();
    let ksp2 = ksp2_flight(run, c);
    let end = ksp2.samples.last().unwrap().t.min(until);
    // a craft KSP2 burned up may go a few seconds later here (the losses are held to 5 s below)
    let whole = until.is_infinite();
    let ose = ose_flight(
        c,
        run,
        if ksp2.destroyed && whole {
            end + 5.0
        } else {
            end
        },
        true,
    );
    println!("== {name}");
    let mut t = 10.0;
    while t <= end {
        let (Some(k), Some(o)) = (at(&ksp2, t), at(&ose, t)) else {
            break;
        };
        let temps = |s: &Sample| {
            s.temps
                .iter()
                .map(|t| t.map_or("   -".into(), |t| format!("{t:4.0}")))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!(
            "t {:4.0}: alt {:6.0} vs {:6.0}  speed {:6.1} vs {:6.1}  K [{}] vs [{}]",
            t,
            k.alt,
            o.alt,
            k.speed,
            o.speed,
            temps(k),
            temps(o)
        );
        assert!(
            (k.alt - o.alt).abs() < 0.02 * k.alt.max(20_000.0),
            "{name} t {t}: alt {:.0} vs KSP2 {:.0}",
            o.alt,
            k.alt
        );
        // the falling speed against height: OSE's speed where it passed KSP2's height (nearest in time)
        let at_height = speed_at_height(&ose, k.alt, t).unwrap_or(o.speed);
        assert!(
            (k.speed - at_height).abs() < 0.01 * k.speed.max(100.0),
            "{name} t {t}: {:.0} m at {at_height:.1} m/s, KSP2 {:.1} m/s",
            k.alt,
            k.speed
        );
        // Two of the same part side by side meet the air level with each other: which one KSP2 counts as
        // ahead, and so shadows the other, flips from tick to tick (two Swivels side by side swap 1.0 and 0.85).
        // Their exposure and heat are held as a set: each part's paired with the one of its kind that ranks
        // the same.
        // (KSP2 logs the heat a tick behind the exposure, so each is paired on its own.)
        let pairing = |ours: &Vec<Option<f64>>, theirs: &Vec<Option<f64>>, step: f64| {
            let mut pair: Vec<usize> = (0..n).collect();
            for i in 0..n {
                let kind: Vec<usize> = (0..n)
                    .filter(|&j| c.parts[j].part_id == c.parts[i].part_id)
                    .filter(|&j| ours[j].is_some() && theirs[j].is_some())
                    .collect();
                if kind.len() < 2 || kind[0] != i {
                    continue;
                }
                // ranked by value, a near tie kept in craft order
                let rank = |e: &Vec<Option<f64>>| {
                    let mut r = kind.clone();
                    r.sort_by_key(|&a| ((e[a].unwrap() / step).round() as i64, a));
                    r
                };
                for (o, k) in rank(ours).into_iter().zip(rank(theirs)) {
                    pair[o] = k;
                }
            }
            pair
        };
        let pair = pairing(&o.exposed, &k.exposed, 0.03);
        let pair_flux = pairing(&o.flux, &k.flux, 5.0);
        for i in 0..n {
            if let (Some(a), Some(b)) = (k.temps[i], o.temps[i]) {
                assert!(
                    (a - b).abs() < 0.04 * a + 8.0,
                    "{name} t {t}: part {i} {} at {b:.0} K, KSP2 {a:.0} K",
                    c.parts[i].part_id
                );
            }
            // where the run logged them: how much of the part the airflow reaches (its shadowing by the parts
            // ahead) and the re-entry heat it takes
            // (not a fuel line or strut: its outline is the line stretched between its anchors, which OSE does
            // not draw; it takes no heat either way)
            let compound = ["fuelline_", "strut_"]
                .iter()
                .any(|k| c.parts[i].part_id.starts_with(k));
            if let (Some(a), Some(b), false) = (k.exposed[pair[i]], o.exposed[i], compound) {
                assert!(
                    (a - b).abs() < 0.03,
                    "{name} t {t}: part {i} {} {b:.3} exposed, KSP2 {a:.3}",
                    c.parts[i].part_id
                );
            }
            // for a part well out in the air (one at the edge of a shadow is held by its exposure above): the runs
            // logged with it hold steady attitudes, the older swinging ones are held to their temperatures
            if let (Some(a), Some(b), Some(ka), Some(oa)) = (
                k.flux[pair_flux[i]],
                o.flux[i],
                k.exposed[pair[i]],
                o.exposed[i],
            ) {
                if ka.min(oa) < 0.1 {
                    continue;
                }
                assert!(
                    (a - b).abs() < 0.1 * a + 2.0,
                    "{name} t {t}: part {i} {} takes {b:.1} kW, KSP2 {a:.1} kW",
                    c.parts[i].part_id
                );
            }
        }
        t += 10.0;
    }
    if !whole {
        println!("{name}: tumbles at {until:.0} s; held to there");
        return;
    }
    for (i, p) in c.parts.iter().enumerate() {
        println!(
            "part {i} {}: peak {:.0} K (KSP2 {:.0} K)",
            p.part_id, ose.peak[i], ksp2.peak[i]
        );
        assert!(
            (ose.peak[i] - ksp2.peak[i]).abs() < 0.04 * ksp2.peak[i] + 8.0,
            "{name}: {} peak {:.0} vs KSP2 {:.0} K",
            p.part_id,
            ose.peak[i],
            ksp2.peak[i]
        );
    }
    println!("lost: OSE {:?}\n      KSP2 {:?}", ose.lost, ksp2.lost);
    assert_eq!(ose.destroyed, ksp2.destroyed, "{name}: craft destroyed");
    let set = |f: &Flight| {
        let mut v: Vec<usize> = f.lost.iter().map(|l| l.2).collect();
        v.sort();
        v
    };
    assert_eq!(set(&ose), set(&ksp2), "{name}: parts lost");
    for (o, k) in ose.lost.iter().zip(&ksp2.lost) {
        assert!(
            (o.0 - k.0).abs() < 5.0,
            "{name}: part {} lost at {:.1} s, KSP2 at {:.1} s",
            k.2,
            o.0,
            k.0
        );
    }
}

/// The bare Mk1 pod (no heat shield): it overheats past its 850 K and is lost at about 25 km in both games.
#[test]
fn ksp2_bare_pod_burns_up_reentering_from_orbit() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-pod.json"
    ));
    compare("bare pod", &c, &run!("ksp2-reentryorbit-pod"));
}

/// Pod, six 1x4 tanks and a Swivel, engine first: the Swivel takes the heat; the tanks and pod in its
/// shadow take none and drift toward the air's temperature (KSP2 counts a part 99% behind another as wholly
/// behind it). The heavy craft skims back up from about 30 km.
#[test]
fn ksp2_stack_reentry_shadowed_parts_warm_to_the_air() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-stack.json"
    ));
    compare("stack", &c, &run!("ksp2-reentryorbit-stack"));
}

/// The Mk1 pod over a 1.25 m heat shield: the shield takes the heat and ablates, the pod stays cool, and it
/// slows to its terminal speed in the lower air.
#[test]
fn ksp2_heat_shield_reentry_from_orbit() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-reentry.json"
    ));
    compare("pod + heat shield", &c, &run!("ksp2-reentryorbit-reentry"));
}

/// The bare pod with SAS off from 70.4 km, left to the airflow from KSP2's attitude and turn rate: it keeps
/// tumbling (KSP2 applies each tick's airflow forces one tick late, `Structure::airflow_one_tick_late`, which
/// feeds the swing instead of letting angular drag settle it), stays fast and burns up near 25 km. The tumble is
/// chaotic, so the two runs are held to its outcome and to the speed while the air is thin.
#[test]
fn ksp2_free_tumbling_pod_keeps_tumbling_and_burns_up() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-pod.json"
    ));
    let run = run!("ksp2-reentryorbit-podfree");
    let ksp2 = ksp2_flight(&run, &c);
    let ose = ose_flight(&c, &run, ksp2.samples.last().unwrap().t + 120.0, false);
    for s in ksp2.samples.iter().step_by(20) {
        let o = speed_at_height(&ose, s.alt, s.t);
        println!(
            "{:6.0} m: speed KSP2 {:7.1} OSE {:7.1}",
            s.alt,
            s.speed,
            o.unwrap_or(f64::NAN)
        );
        if s.alt > 30_000.0 && s.t > 2.0 {
            let o = o.expect("OSE passed this height");
            assert!(
                (o - s.speed).abs() < 0.02 * s.speed,
                "{:.0} m: {o:.1} vs KSP2 {:.1} m/s",
                s.alt,
                s.speed
            );
        }
    }
    println!("lost: OSE {:?}\n      KSP2 {:?}", ose.lost, ksp2.lost);
    assert!(ksp2.destroyed && ose.destroyed, "the pod burns up in both");
    let (o, k) = (ose.lost[0], ksp2.lost[0]);
    assert!(
        (o.1 - k.1).abs() < 2_000.0,
        "burns up at {:.0} m, KSP2 {:.0} m",
        o.1,
        k.1
    );
}

/// Complex craft (Josh 2026-11-25): the stock Kerbal K1 (38 parts: core, four radial Hammer boosters with nose
/// cones, fins, solar panels and radial batteries), the stock K2 (48 parts, 2.5 m core, Thumper boosters, radial
/// monopropellant tanks), the stock jet (wings, stabilizers, gear, intake) and the Mk1 pod under a closed fairing
/// nose, each brought down from orbit with SAS holding retrograde, part by part.
#[test]
fn ksp2_k1_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-k1.json"
    ));
    let run = run!("ksp2-reentryorbit-k1");
    compare_until("K1", &c, &run, tumbles_at(&run));
}

#[test]
fn ksp2_k2_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-k2.json"
    ));
    let run = run!("ksp2-reentryorbit-k2");
    compare_until("K2", &c, &run, tumbles_at(&run));
}

#[test]
fn ksp2_jet_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-jet.json"
    ));
    let run = run!("ksp2-reentryorbit-jet");
    compare_until("jet", &c, &run, tumbles_at(&run));
}

#[test]
fn ksp2_fairing_nose_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-fairing-nose.json"
    ));
    compare("fairing nose", &c, &run!("ksp2-reentryorbit-fairing"));
}

/// Three Swivels clustered side by side (craft-asparagus.json: a core and two radial boosters with fuel lines),
/// engines first: the three bells take the heat side by side and shadow what is over them.
#[test]
fn ksp2_clustered_engines_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-asparagus.json"
    ));
    let run = run!("ksp2-reentryorbit-asparagus");
    compare_until("clustered", &c, &run, tumbles_at(&run));
}

/// The same core with one booster on one side only (craft-asym.json): lopsided drag and shadows.
#[test]
fn ksp2_asymmetric_craft_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-asym.json"
    ));
    let run = run!("ksp2-reentryorbit-asym");
    compare_until("asymmetric", &c, &run, tumbles_at(&run));
}

/// KSP2's stock RC-Sparrow drone plane, tail first: fixed gear, a jet intake and shaped wings.
#[test]
fn ksp2_sparrow_plane_reentry_part_by_part() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-sparrow.json"
    ));
    let run = run!("ksp2-reentryorbit-sparrow");
    compare_until("sparrow", &c, &run, tumbles_at(&run));
}

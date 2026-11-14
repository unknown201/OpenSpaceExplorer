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
    ose_importer::craft::craft_from_value(&v).unwrap()
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

/// One second of a flight: altitude, surface speed and each part's temperature (craft order; `None` once gone).
#[derive(Clone, Debug)]
struct Sample {
    t: f64,
    alt: f64,
    speed: f64,
    temps: Vec<Option<f64>>,
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

/// KSP2's run as a `Flight`, times from the start row.
fn ksp2_flight(run: &Run, limits: &[f64]) -> Flight {
    let n = limits.len();
    let t0 = num(&run.start, "t");
    let names: Vec<&str> = run.events[0]["names"].split(' ').collect();
    assert_eq!(names.len(), n, "KSP2's craft has the craft file's parts");
    // KSP2 logs the parts still there in craft order: `alive` maps its index to the craft's
    let mut alive: Vec<usize> = (0..n).collect();
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
            });
        }
        let idx: usize = r["index"].parse().unwrap();
        if let Some(&i) = alive.get(idx) {
            let temp = num(r, "temp_k");
            samples.last_mut().unwrap().temps[i] = Some(temp);
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
    let ksp2 = ksp2_flight(run, &limits(c));
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
    let attitude: Vec<(f64, DVec3)> = run
        .attitude
        .iter()
        .map(|r| {
            (
                num(r, "t") - t0,
                DVec3::new(num(r, "along"), num(r, "up"), num(r, "side")),
            )
        })
        .collect();
    let mut samples = Vec::new();
    let mut lost = Vec::new();
    let mut peak = vec![0.0f64; n];
    let mut was = vec![true; n];
    let mut destroyed = false;
    for k in 1..=(seconds / DT) as usize {
        if replay {
            let t = (k - 1) as f64 * DT;
            let a = attitude[attitude
                .partition_point(|x| x.0 < t - 1e-6)
                .min(attitude.len() - 1)]
            .1;
            let mut p = u.powered(v).unwrap().clone();
            let along = p.v.normalized();
            let side = along.cross(p.r.normalized()).normalized();
            let up = side.cross(along);
            p.facing = (along * a.x + up * a.y + side * a.z).normalized();
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
            samples.push(Sample {
                t,
                alt,
                speed: srf.length(),
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
/// 400 m: a tumbling craft replayed from attitude sampled every 0.1 s, or a second's lag after the transonic
/// stretch, moves it a few hundred metres), the falling speed at each height (within 3%: from Mach 2 down to 1
/// OSE runs up to 2.5% slow, elsewhere within 0.5%), every part's temperature (within 4% + 8 K), and the
/// same parts lost at about the same time.
fn compare(name: &str, c: &ose_data::craft::Craft, run: &Run) {
    let n = c.parts.len();
    let ksp2 = ksp2_flight(run, &limits(c));
    let end = ksp2.samples.last().unwrap().t;
    let ose = ose_flight(c, run, end, true);
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
            (k.speed - at_height).abs() < 0.03 * k.speed.max(100.0),
            "{name} t {t}: {:.0} m at {at_height:.1} m/s, KSP2 {:.1} m/s",
            k.alt,
            k.speed
        );
        for i in 0..n {
            if let (Some(a), Some(b)) = (k.temps[i], o.temps[i]) {
                assert!(
                    (a - b).abs() < 0.04 * a + 8.0,
                    "{name} t {t}: part {i} {} at {b:.0} K, KSP2 {a:.0} K",
                    c.parts[i].part_id
                );
            }
        }
        t += 10.0;
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

/// Not matched yet: the bare pod with SAS off from 70.4 km, left to the airflow from KSP2's attitude and turn rate.
/// KSP2's keeps tumbling and burns up at 24.9 km; OSE's rights itself base first and survives (docs/ksp2-side-by-
/// side-gaps.md, orbital reentry delta). Prints the two side by side.
#[test]
#[ignore = "KSP2's free pod tumbles; OSE's rights itself"]
fn ksp2_free_tumbling_pod_reentry() {
    let c = craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-pod.json"
    ));
    let run = run!("ksp2-reentryorbit-podfree");
    let ksp2 = ksp2_flight(&run, &limits(&c));
    let ose = ose_flight(&c, &run, ksp2.samples.last().unwrap().t + 200.0, false);
    for s in ksp2.samples.iter().step_by(15) {
        let o = speed_at_height(&ose, s.alt, s.t);
        println!(
            "{:6.0} m: KSP2 {:7.1} m/s {:?}  OSE {:?}",
            s.alt,
            s.speed,
            s.temps[0].map(|t| t.round()),
            o.map(|x| x.round())
        );
    }
    println!(
        "KSP2 lost {:?} destroyed {}; OSE lost {:?} destroyed {}",
        ksp2.lost, ksp2.destroyed, ose.lost, ose.destroyed
    );
    println!(
        "OSE peak {:?}, last {:?}",
        ose.peak,
        ose.samples.last().map(|s| (s.t, s.alt, s.speed))
    );
}

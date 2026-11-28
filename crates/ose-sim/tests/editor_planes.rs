//! Planes built in the hangar's editor (`ose-client`'s `plane_build` test writes them to
//! data/editor-planes.json): a small jet like the stock one, a medium one with heavy swept wings and a small
//! one with long straight wings. Each must stand level on its three wheels, roll, rotate on the stick and
//! fly, as the stock jet does (`a_jet_rotates_on_the_stick_and_lifts_off_as_ksp2s`).
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

fn planes() -> Vec<ose_data::craft::Craft> {
    serde_json::from_str(include_str!("data/editor-planes.json")).unwrap()
}

/// The plane launched on a flat Kerbin as the server launches one: shaped, laid level, heading 90.
fn on_runway(c: &ose_data::craft::Craft) -> (Universe, bevy_ecs::entity::Entity, f64) {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut c = c.clone();
    c.apply_shapes(&defs);
    let c = ose_sim::structure::level_layout(&c).expect("built nose-forward");
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let spin = std::f64::consts::TAU / kerbin.rotation_period_s;
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_craft(v, c.clone());
    u.set_structure(v, Structure::build(&c, &defs, &res));
    assert!(u.set_level(v, 90.0));
    (u, v, spin)
}

fn pitch_deg(p: &flight::Powered) -> f64 {
    p.facing.dot(p.r.normalized()).asin().to_degrees()
}

#[test]
fn editor_planes_stand_level_on_three_wheels() {
    for c in planes() {
        let (mut u, v, _) = on_runway(&c);
        u.set_drive(v, 0.0, 0.0, true);
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).unwrap();
        let shares = p.drive.load_shares(1.0, 9.81);
        let pitch = pitch_deg(p);
        println!(
            "{}: {:.2} t, rests at {pitch:.2} deg, wheels {} carrying {shares:.3?}, clearance {:.2} m",
            c.name,
            p.mass_t,
            p.drive.wheels.len(),
            p.ground_clearance_m
        );
        assert!(p.landed && !p.crashed && !p.tipped, "{}: {p:?}", c.name);
        assert_eq!(p.drive.wheels.len(), 3, "{}", c.name);
        // nose wheel ahead of the mains, every wheel carrying weight
        assert!(shares.iter().all(|s| *s > 0.04), "{}: {shares:?}", c.name);
        assert!(pitch.abs() < 6.0, "{}: rests at {pitch:.2} deg", c.name);
    }
}

/// KSP2's `jetfly` runs of three of these planes (`run-planes.ps1`, reference\ksp2-plane-*, 2026-11-28),
/// flown as `JetScenario.Fly` flies them: SAS on, full throttle, full stick at 60 m/s until the nose is 10
/// degrees up (at most 10 s), then the stick let go. Per plane: seconds from the stick to leaving the
/// runway and the speed then (m/s); `None` where it never flies.
const KSP2_TAKEOFF: [(&str, Option<(f64, f64)>); 3] = [
    // mains 0.9 m behind the centre of mass: the stick cannot lift the nose against the weight ahead of
    // them, the wings push it down (-93 kN at 155 m/s), and it runs off the end of the runway
    ("Small", None),
    // the stick brings the nose up 4 degrees as the speed builds, and it flies off at 139 m/s
    ("Small swept", Some((10.14, 138.9))),
    // its long wings lift it almost at once
    ("Small straight", Some((0.5, 55.0))),
];

/// Our run of KSP2's takeoff: (seconds from the stick to leaving the runway, speed then, the highest nose
/// on the runway in degrees), or `None` if it never leaves within 60 s of the stick.
fn ose_takeoff(c: &ose_data::craft::Craft) -> Option<(f64, f64, f64)> {
    let (mut u, v, spin) = on_runway(c);
    u.step();
    let facing = u.powered(v).unwrap().facing;
    u.stage(v);
    u.set_controls(v, 1.0, flight::Attitude::Fixed(facing));
    let mut stick: Option<u64> = None;
    let mut held = true;
    let mut top = f64::MIN;
    for i in 0..(150 * TICKS_PER_SECOND) {
        u.step();
        let p = u.powered(v).unwrap().clone();
        assert!(!p.crashed, "{} crashed: {p:?}", c.name);
        let speed = (p.v - DVec3::new(0.0, 0.0, spin).cross(p.r)).length();
        let pitch = pitch_deg(&p);
        match stick {
            None if speed >= 60.0 => {
                stick = Some(i);
                u.set_rotation(v, 1.0, 0.0, 0.0);
            }
            Some(s) => {
                if held && (pitch >= 10.0 || i - s >= 10 * TICKS_PER_SECOND) {
                    held = false;
                    u.set_rotation(v, 0.0, 0.0, 0.0);
                    u.set_controls(v, 1.0, flight::Attitude::Fixed(p.facing));
                }
                if !p.landed {
                    return Some(((i - s) as f64 / TICKS_PER_SECOND as f64, speed, top));
                }
                top = top.max(pitch);
                if i - s > 60 * TICKS_PER_SECOND {
                    return None;
                }
            }
            None => {}
        }
    }
    None
}

/// A minute or two of runway each: run in release builds (`cargo test --release -p ose-sim --test
/// editor_planes`).
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn editor_planes_take_off_as_in_ksp2() {
    for (name, ksp2) in KSP2_TAKEOFF {
        let c = planes().into_iter().find(|c| c.name == name).unwrap();
        let ours = ose_takeoff(&c);
        println!("{name}: off the runway after (s, m/s, nose deg) {ours:?}, KSP2 {ksp2:?}");
        match (ours, ksp2) {
            (None, None) => {}
            (Some((s, v, _)), Some((ks, kv))) => {
                assert!(
                    (s - ks).abs() < 1.0,
                    "{name}: {s:.2} s after the stick (KSP2 {ks})"
                );
                assert!(
                    (v - kv).abs() < 0.05 * kv,
                    "{name}: at {v:.1} m/s (KSP2 {kv})"
                );
            }
            _ => panic!("{name}: {ours:?} against KSP2's {ksp2:?}"),
        }
    }
}

/// The medium plane (its heavy wing and Panther have no KSP2 craft to copy) rotates on the stick and flies.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn the_medium_plane_rotates_and_flies() {
    let c = planes()
        .into_iter()
        .find(|c| c.name == "Medium heavy wing")
        .unwrap();
    let (s, v, n) = ose_takeoff(&c).expect("leaves the runway");
    println!("Medium heavy wing: off the runway {s:.2} s after the stick at {v:.1} m/s, nose up to {n:.1} deg");
}

/// The KSP2 crafts of these planes (tools/ksp2-parity-pilot/make_craft_planes.py, flown by the parity pilot's
/// `jetfly <name>`) read back as the very planes the editor built.
#[test]
fn ksp2_crafts_of_the_editor_planes_read_back_the_same() {
    let same_turn = |a: ose_data::craft::Quat, b: ose_data::craft::Quat| {
        (a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w).abs() > 1.0 - 1e-9
    };
    let mirrored = |p: &ose_data::craft::CraftPart| p.flight.as_ref().is_some_and(|f| f.mirrored);
    for (name, text) in [
        (
            "Small",
            include_str!("../../../tools/ksp2-parity-pilot/craft-plane-small.json"),
        ),
        (
            "Small swept",
            include_str!("../../../tools/ksp2-parity-pilot/craft-plane-swept.json"),
        ),
        (
            "Small straight",
            include_str!("../../../tools/ksp2-parity-pilot/craft-plane-straight.json"),
        ),
    ] {
        let plane = planes().into_iter().find(|c| c.name == name).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        let back = ose_importer::craft::craft_from_value(&value).unwrap();
        assert_eq!(back.parts.len(), plane.parts.len(), "{name}");
        for (a, b) in plane.parts.iter().zip(&back.parts) {
            let what = format!("{name} {}", a.part_id);
            assert_eq!(a.part_id, b.part_id, "{what}");
            assert_eq!(a.parent, b.parent, "{what}");
            assert!(
                (a.local_position - b.local_position).length() < 1e-6,
                "{what}"
            );
            assert!(same_turn(a.local_rotation, b.local_rotation), "{what}");
            assert_eq!(mirrored(a), mirrored(b), "{what}");
            if let Some(s) = &a.shape {
                let t = b.shape.as_ref().expect("shaped");
                for (n, v) in &s.sliders {
                    assert!((t.slider(n).unwrap() - v).abs() < 1e-9, "{what} {n}");
                }
            }
        }
    }
}

/// What meets the ground: each wing of a pair touches down on its own baked shape, the second the first's
/// mirror image across the plane's middle and both outside the fuselage.
#[test]
fn editor_wing_pairs_meet_the_ground_as_mirror_images() {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    for c in planes() {
        let mut c = c.clone();
        c.apply_shapes(&defs);
        let c = ose_sim::structure::level_layout(&c).unwrap();
        let s = Structure::build(&c, &defs, &res);
        let root = c.parts.iter().position(|p| p.parent.is_none()).unwrap();
        let mid = s.ground_points(root);
        let centre = mid.iter().fold(DVec3::ZERO, |a, p| a + *p) * (1.0 / mid.len() as f64);
        let wings: Vec<usize> = (0..c.parts.len())
            .filter(|&i| c.parts[i].part_id.starts_with("wing_"))
            .collect();
        assert_eq!(wings.len(), 2, "{}", c.name);
        let (a, b) = (s.ground_points(wings[0]), s.ground_points(wings[1]));
        assert!(
            a.len() > 8 && a.len() == b.len(),
            "{}: hulls, not boxes",
            c.name
        );
        // the axis across the plane's middle: where the two wings lie apart
        let mean = |v: &[DVec3]| v.iter().fold(DVec3::ZERO, |s, p| s + *p) * (1.0 / v.len() as f64);
        let d = mean(&a) - mean(&b);
        let k = [d.x.abs(), d.y.abs(), d.z.abs()]
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .unwrap()
            .0;
        let at = |p: DVec3| [p.x, p.y, p.z][k];
        let flip = |p: DVec3| {
            let mut v = [p.x, p.y, p.z];
            v[k] = 2.0 * at(centre) - v[k];
            DVec3::new(v[0], v[1], v[2])
        };
        for p in &a {
            let q = flip(*p);
            let near = b.iter().map(|x| (*x - q).length()).fold(f64::MAX, f64::min);
            assert!(
                near < 1e-4,
                "{}: wing point {p:?} has no twin ({near} m)",
                c.name
            );
            assert!(
                (at(*p) - at(centre)).abs() > 0.6,
                "{}: wing inside the fuselage",
                c.name
            );
        }
    }
}

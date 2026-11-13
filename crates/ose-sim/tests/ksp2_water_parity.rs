//! KSP2 parity for water (gap T43): splashdown breakage, floating and settling, from the original's own model
//! (`PartBehavior.BuoyancyUpdate` / `CalculateLiquidDrag` with the loaded PhysicsSettings) on the KSP2 parity
//! pilot's craft (`tools/ksp2-parity-pilot/craft-drop-*.json`). The original has no flooding and no waves.
//! These formula fixtures remove Kerbin's orbital parent and use a synthetic radial reference axis.
//! The live geographic/frame comparison is documented in docs/reference-notes/water-live-parity.md.
use std::f64::consts::TAU;

use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{flight, structure::Structure, Universe, TICKS_PER_SECOND};

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

fn craft(json: &str) -> ose_data::craft::Craft {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    ose_importer::craft::craft_from_value(&v).unwrap()
}

fn pod() -> ose_data::craft::Craft {
    craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-pod.json"
    ))
}

fn podtank() -> ose_data::craft::Craft {
    craft(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-drop-podtank.json"
    ))
}

/// The craft held upright at the sea surface of an all-ocean Kerbin (sea floor 200 m down), with Kerbin's spin.
fn at_sea(c: &ose_data::craft::Craft) -> (Universe, Entity, f64, f64) {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let spin = TAU / kerbin.rotation_period_s;
    let radius = kerbin.radius_m;
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    u.world
        .entity_mut(body)
        .insert(flight::Terrain(std::sync::Arc::new(
            ose_data::terrain::TerrainMap {
                width: 2,
                height: 2,
                scale_m: 0.0,
                offset_m: -200.0,
                has_ocean: true,
                body_radius_m: radius,
                data: vec![0; 4],
                max_m: -200.0,
                detail: None,
                overlay: None,
                flats: vec![],
            },
        )));
    let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(c, &defs, &res));
    (u, v, spin, radius)
}

/// The lowest point of the craft's bounds below its centre of mass, m (upright).
fn keel_depth(u: &Universe, v: Entity) -> f64 {
    let st = u.structure(v).unwrap();
    let c = st.mass_properties().centre_m;
    (0..st.rigid.len())
        .filter(|&i| st.rigid.present[i])
        .map(|i| {
            let (centre, half) = st.part_box(i);
            c.y - (centre.y - half.y)
        })
        .fold(0.0, f64::max)
}

struct Splash {
    lost_tick: Option<usize>,
    parts_left: usize,
    /// How far the keel sits under the water at the end, m.
    draft_m: f64,
    /// Vertical speed at the end, m/s, and the last time it was over 0.05 m/s.
    vspeed: f64,
    settled_s: f64,
    splashed: bool,
}

/// Let the craft go with its keel `h0` m above the water, moving `down` m/s downward and `east` m/s east.
fn splash(c: &ose_data::craft::Craft, h0: f64, down: f64, east: f64, seconds: f64) -> Splash {
    let (mut u, v, spin, radius) = at_sea(c);
    let keel = keel_depth(&u, v);
    let mut p = u.powered(v).unwrap().clone();
    let up = p.r.normalized();
    let east_dir = DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
    p.r = up * (radius + keel + h0);
    p.v = DVec3::new(0.0, 0.0, spin).cross(p.r) - up * down + east_dir * east;
    p.landed = false;
    p.splashed = false;
    u.set_powered(v, p);
    let n0 = u
        .structure(v)
        .unwrap()
        .rigid
        .present
        .iter()
        .filter(|&&x| x)
        .count();
    let mut out = Splash {
        lost_tick: None,
        parts_left: n0,
        draft_m: 0.0,
        vspeed: 0.0,
        settled_s: 0.0,
        splashed: false,
    };
    for k in 1..=((seconds / DT).round() as usize) {
        u.step();
        let Some(p) = u.powered(v).filter(|p| !p.crashed) else {
            out.lost_tick.get_or_insert(k);
            continue;
        };
        let vs = (p.v - DVec3::new(0.0, 0.0, spin).cross(p.r)).dot(p.r.normalized());
        if vs.abs() > 0.05 {
            out.settled_s = k as f64 * DT;
        }
        out.vspeed = vs;
        out.splashed = p.splashed;
        out.draft_m = radius + keel_depth(&u, v) - p.r.length();
        out.parts_left = u
            .structure(v)
            .map(|s| s.rigid.present.iter().filter(|&&x| x).count())
            .unwrap_or(0);
    }
    if out.lost_tick.is_some() {
        out.parts_left = 0;
    }
    out
}

#[test]
fn ksp2_splashdowns_break_pods_over_one_point_two_times_their_tolerance() {
    // Mk1 pod, crashTolerance 15: an aligned reference axis breaks above 18 m/s (1.2 x 15).
    for (down, survives) in [(10.0, true), (17.0, true), (19.5, false)] {
        // keel 2 cm above the water: it enters at the set speed
        let s = splash(&pod(), 0.02, down, 0.0, 2.0);
        assert_eq!(
            s.lost_tick.is_none(),
            survives,
            "pod into the sea at {down} m/s"
        );
        if !survives {
            assert!(s.lost_tick.unwrap() <= 2, "lost on entry");
        }
    }
    // skimming: a tenth of the speed counts, so 100 m/s nearly level is survived only below 180 m/s
    let skim = splash(&pod(), 0.02, 2.0, 100.0, 1.0);
    assert!(skim.lost_tick.is_none(), "a 100 m/s skim at 2 m/s down");
    // A tank under the pod does not shield it: each part is judged on the vessel's speed as it gets wet, and
    // the water barely slows the craft in the 0.1 s before the pod goes in (the 1x2 tank, crashTolerance 20,
    // would itself take 24 m/s).
    let tank = splash(&podtank(), 0.02, 17.0, 0.0, 2.0);
    assert_eq!(tank.parts_left, 2, "pod on a tank at 17 m/s");
    let hard = splash(&podtank(), 0.02, 22.0, 0.0, 2.0);
    assert!(
        hard.lost_tick.is_some(),
        "pod on a tank at 22 m/s loses the pod"
    );
}

#[test]
fn ksp2_pods_float_at_the_draft_their_buoyancy_gives_and_settle() {
    // Mk1 pod: 0.67 m^3 x 1.2 x buoyancy 3 = 2.41 t of water fully under; at 0.99 t it floats with 41% of its
    // 0.96 m height wet: 0.40 m
    let s = splash(&pod(), 1.0, 5.0, 0.0, 15.0);
    assert!(s.lost_tick.is_none() && s.splashed);
    assert!((s.draft_m - 0.395).abs() < 0.03, "draft {:.3} m", s.draft_m);
    assert!(s.vspeed.abs() < 0.05);
    // The measured 3 m / 5 m/s native drop remains above 0.05 m/s until 7.52 s.
    // The previous six-second bound was an unmeasured inference from the damping constant.
    assert!(s.settled_s < 8.0, "still bobbing at {:.1} s", s.settled_s);
}

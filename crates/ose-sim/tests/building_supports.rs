use ose_data::craft::{Craft, Quat};
use ose_data::{PartDef, ResourceDef};
use ose_math::DVec3;
use ose_sim::{
    flight::{FlightSpec, Powered},
    rigid::{RigidVessel, Turning},
    structure::Structure,
    Universe,
};

fn assets() -> (Craft, Vec<PartDef>, Vec<ResourceDef>) {
    let crafts: Vec<Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    (
        crafts.into_iter().find(|c| c.name == "Kerbal K1").unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
    )
}

fn add_support(c: &mut Craft, id: &str, parent: usize) -> usize {
    let mut p = c.parts[parent].clone();
    p.guid = format!("support-{}", c.parts.len());
    p.part_id = id.into();
    p.parent = Some(parent);
    p.own_node = Some("srfAttach".into());
    p.parent_node = None;
    p.local_position = p.local_position + DVec3::new(0.5, 1.0, 0.0);
    p.local_rotation = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
    p.resources.clear();
    p.fuel_line_target = None;
    c.parts.push(p);
    c.parts.len() - 1
}

#[test]
fn strut_braces_the_loaded_path_and_releases_after_separation() {
    let (mut c, defs, res) = assets();
    let i = add_support(&mut c, "strut_1v_extensible", 21);
    let target = c.parts.iter().position(|p| p.parent.is_none()).unwrap();
    let far = add_support(&mut c, "strut_1v_extensible", target);
    c.parts[far].local_position = c.parts[target].local_position;
    c.parts[i].fuel_line_target = Some(far);
    let mut braced = RigidVessel::build(&c, &defs, &res);
    c.parts[i].fuel_line_target = None;
    let mut bare = RigidVessel::build(&c, &defs, &res);
    let turning = Turning {
        torques: vec![
            (21, DVec3::new(100.0, 0.0, 0.0)),
            (target, DVec3::new(-100.0, 0.0, 0.0)),
        ],
        ..Default::default()
    };
    for _ in 0..100 {
        assert!(bare
            .step_turning(0.02, DVec3::ZERO, &[], Some(&turning))
            .is_empty());
        assert!(braced
            .step_turning(0.02, DVec3::ZERO, &[], Some(&turning))
            .is_empty());
    }
    let bend = |v: &RigidVessel| {
        v.flex(0.0, 0.0)
            .iter()
            .find(|p| p.part == 21)
            .unwrap()
            .offset_m
            .length()
    };
    println!("bare {} braced {}", bend(&bare), bend(&braced));
    assert!(bend(&bare) > 0.0001, "fixture must flex: {}", bend(&bare));
    assert!(
        bend(&braced) < bend(&bare) * 0.25,
        "bare {}, braced {}",
        bend(&bare),
        bend(&braced)
    );
    assert!(braced.braces[0].active);
    let decoupler = c.parts[21].parent.unwrap();
    braced.remove_parts(&[decoupler]);
    assert!(
        !braced.braces[0].active,
        "brace must not hold separated stages together"
    );
    assert!(braced.groups().len() > 1);
}

#[test]
fn physicsless_truss_rides_parent_but_full_beams_have_flexible_joints() {
    let (mut c, defs, res) = assets();
    let tiny = add_support(&mut c, "truss_0v_square_1x1_0-31m", 0);
    let beam = add_support(&mut c, "ibeam_1v_1x5", 0);
    let v = RigidVessel::build(&c, &defs, &res);
    let joint = |i| v.links.iter().find(|l| l.child == i).unwrap();
    assert!(joint(tiny).limits.rigid);
    assert!(joint(tiny).limits.break_force_kn.is_infinite());
    assert!(!joint(beam).limits.rigid);
    assert!(joint(beam).limits.break_force_kn.is_finite());
    assert_eq!(v.dry_masses_t[tiny], 0.001);
    assert_eq!(v.dry_masses_t[beam], 0.176);
}

#[test]
fn compacting_and_reloading_a_craft_preserves_only_valid_braces() {
    let (mut c, defs, res) = assets();
    let i = add_support(&mut c, "strut_1v_extensible", 21);
    let far = add_support(&mut c, "strut_1v_extensible", 0);
    c.parts[i].fuel_line_target = Some(far);
    let keep: Vec<_> = (0..c.parts.len()).filter(|&j| j != 1).collect();
    let (compact, map) = ose_sim::docking::subset(&c, &keep);
    let index = map[i].unwrap();
    assert_eq!(
        compact.parts[index].fuel_line_target.unwrap(),
        map[far].unwrap()
    );
    let json = serde_json::to_string(&compact).unwrap();
    let restored: Craft = serde_json::from_str(&json).unwrap();
    assert_eq!(
        restored.parts[index].fuel_line_target,
        compact.parts[index].fuel_line_target
    );
    let v = RigidVessel::build(&restored, &defs, &res);
    assert_eq!(v.braces.len(), 1);
    let keep: Vec<_> = (0..c.parts.len()).filter(|&j| j != far).collect();
    let (separated, map) = ose_sim::docking::subset(&c, &keep);
    assert!(separated.parts[map[i].unwrap()].fuel_line_target.is_none());
}

#[test]
fn clamps_hold_burning_engines_then_release_on_their_stage() {
    let (mut c, defs, res) = assets();
    let clamp = add_support(&mut c, "launchclamp_3v_ground", 0);
    // Clamps release one stage after engine ignition.
    c.stages.push(vec![clamp]);
    let engine_stage = c.stages.len() - 2;
    c.stages.swap(engine_stage, engine_stage + 1);
    let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
    let spec = FlightSpec::from_report(&report, &[], &[], 1.0);
    let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
        "../../../assets/own/kerbol-system.json"
    ))
    .unwrap();
    let mut u = Universe::with_bodies(&bodies);
    let body = u.body_entity("Kerbin").unwrap();
    let r = DVec3::new(600010.0, 0.0, 0.0);
    let e = u.spawn_vessel("clamp test", body, r, DVec3::ZERO);
    let mut p = Powered::new(spec, r, DVec3::ZERO, true);
    p.throttle = 1.0;
    u.set_powered(e, p);
    u.set_structure(e, Structure::build(&c, &defs, &res));
    assert!(
        u.structure(e).unwrap().rigid.present[clamp],
        "prelaunch keeps clamp"
    );
    let pad_radius = u.powered(e).unwrap().r.length();
    assert!(u.stage(e).fired);
    let fuel = u.powered(e).unwrap().propellant_t;
    for _ in 0..50 {
        u.step();
    }
    let p = u.powered(e).unwrap();
    assert!(
        p.landed && p.ground_anchored,
        "thrust must not lift through clamp"
    );
    assert!(p.propellant_t < fuel, "held engines still burn fuel");
    assert!(
        (p.r.length() - pad_radius).abs() < 1e-5,
        "clamps preserve pad height"
    );
    let serialized = serde_json::to_string(p).unwrap();
    let restored: Powered = serde_json::from_str(&serialized).unwrap();
    u.set_powered(e, restored);
    // Rebuilding the structure during restore must not seat a clamped craft at its newly
    // fuel-shifted mass-centre height.
    u.set_structure(e, Structure::build(&c, &defs, &res));
    assert!((u.powered(e).unwrap().r.length() - pad_radius).abs() < 1e-5);
    let l = u
        .structure(e)
        .unwrap()
        .rigid
        .links
        .iter()
        .find(|l| l.child == clamp)
        .unwrap();
    assert!(l.limits.rigid && l.limits.break_force_kn.is_infinite());
    assert!(u.stage(e).fired);
    assert!(!u.structure(e).unwrap().rigid.present[clamp]);
    for _ in 0..50 {
        u.step();
    }
    assert!(!u.powered(e).unwrap().ground_anchored);
    assert!(!u.powered(e).unwrap().landed, "released rocket lifts off");
}

#[test]
fn uploaded_struts_obey_the_shipped_length_and_parent_limits() {
    let (mut c, defs, _) = assets();
    let first = add_support(&mut c, "strut_1v_extensible", 21);
    let far = add_support(&mut c, "strut_1v_extensible", 0);
    c.parts[first].fuel_line_target = Some(far);
    for (length, valid) in [(0.009, false), (0.01, true), (10.0, true), (10.01, false)] {
        c.parts[far].local_position = c.parts[first].local_position + DVec3::new(length, 0.0, 0.0);
        assert_eq!(c.strut_error(&defs).is_none(), valid, "length {length}");
    }
    c.parts[first].fuel_line_target = Some(usize::MAX);
    assert!(c.strut_error(&defs).is_some());
    c.parts[first].fuel_line_target = Some(far);
    c.parts[far].parent = c.parts[first].parent;
    assert!(c.strut_error(&defs).is_some());
}

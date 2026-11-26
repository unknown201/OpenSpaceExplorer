use ose_data::{
    craft::{Craft, CraftPart, Quat, StoredResource},
    PartDef, ResourceDef,
};
use ose_math::DVec3;
use ose_sim::{
    flight::{FlightSpec, Powered},
    structure::Structure,
};

fn fixture() -> (Structure, Powered) {
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let ids = [
        "pod_1v_conical_crew",
        "solararray_1v_1x6",
        "solararray_1v_shielded_1x6",
        "antenna_0v_16",
        "antenna_0v_16s",
        "landingleg_0v",
        "light_0v_spot",
    ];
    let parts = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let def = defs.iter().find(|d| d.id == *id).unwrap();
            CraftPart {
                shroud_shields: None,
                guid: format!("control-{i}"),
                part_id: (*id).into(),
                parent: (i > 0).then_some(0),
                own_node: None,
                parent_node: None,
                local_position: DVec3::new(i as f64, 0., 0.),
                local_rotation: Quat {
                    w: 1.,
                    ..Default::default()
                },
                stage_index: 0,
                fuel_crossfeed: true,
                resources: def
                    .resource_containers
                    .iter()
                    .map(|r| StoredResource {
                        resource: r.resource.clone(),
                        stored_units: r.capacity_units,
                        capacity_units: r.capacity_units,
                    })
                    .collect(),
                fuel_line_target: None,
                colors: None,
                flight: None,
                actions: vec![],
                symmetry: 0,
                shape: None,
                fairing: None,
                chute: None,
                control: None,
            }
        })
        .collect();
    (
        Structure::build(
            &Craft {
                name: "controls".into(),
                description: String::new(),
                colors: None,
                loose: vec![],
                parts,
                stages: vec![],
            },
            &defs,
            &res,
        ),
        Powered::new(
            FlightSpec {
                stages: vec![],
                drag_area_m2: 0.,
                drag_cd: 0.,
            },
            DVec3::new(700_000., 0., 0.),
            DVec3::new(0., 2000., 0.),
            false,
        ),
    )
}

#[test]
fn panel_permissions_antennas_and_gear_stay_independent() {
    let (mut s, _) = fixture();
    let gear = s.gear_out.clone();
    assert_eq!(s.deploy_panels(true), 2);
    assert_eq!(s.deploy_panels(false), 1, "OX-4L cannot retract");
    assert!(s.deployed[1]);
    assert!(!s.deployed[2]);
    assert!(!s.deployed[3]);
    assert_eq!(s.deploy_antennas(true), 1, "fixed 16S is excluded");
    assert!(s.deployed[3]);
    assert!(!s.deployed[4]);
    assert_eq!(s.deploy_antennas(true), 0);
    assert_eq!(s.deploy_antennas(false), 1);
    assert_eq!(s.gear_out, gear);
    let panels = s.deployed.clone();
    assert_eq!(s.deploy_gear(!gear[5]), 1);
    assert_eq!(s.deployed, panels);
    s.rigid.present[3] = false;
    assert_eq!(s.deploy_antennas(true), 0, "detached antenna cannot move");
    s.shielded[2] = true;
    assert_eq!(s.deploy_panels(true), 0, "shielding blocks extension");
}

#[test]
fn lamps_draw_authored_charge_and_go_dark_without_power() {
    let (mut s, mut p) = fixture();
    let ec = s.net.resource("ElectricCharge").unwrap();
    s.electric_services(&mut p, 0., 1.);
    assert!(!s.light_powered[6]);
    let stored = |s: &Structure| {
        s.net
            .tanks
            .iter()
            .filter(|t| t.resource == ec)
            .map(|t| t.stored)
            .sum::<f64>()
    };
    let before = stored(&s);
    p.groups.lights = true;
    s.electric_services(&mut p, 0., 1.);
    assert!(s.light_powered[6]);
    assert!((before - stored(&s) - 0.005).abs() < 1e-8);
    for t in &mut s.net.tanks {
        if t.resource == ec {
            t.stored = 0.;
        }
    }
    s.electric_services(&mut p, 0., 1.);
    assert!(!s.light_powered[6]);
    assert!(p.groups.lights, "power loss preserves the selected switch");
    s.net
        .tanks
        .iter_mut()
        .find(|t| t.resource == ec)
        .unwrap()
        .stored = 1.;
    s.electric_services(&mut p, 0., 1.);
    assert!(s.light_powered[6]);
}

#[test]
fn coasting_vessels_can_switch_lights_without_waking_physics() {
    let (st, p) = fixture();
    let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
        "../../../assets/own/kerbol-system.json"
    ))
    .unwrap();
    let mut u = ose_sim::Universe::with_bodies(&bodies);
    let e = u.spawn_vessel_in_circular_orbit(
        "coasting controls",
        u.body_entity("Kerbin").unwrap(),
        100_000.,
    );
    u.set_dormant(e, p);
    u.set_structure(e, st);
    assert_eq!(
        u.action_group(e, ose_sim::ActionGroup::Lights, None),
        Some(true)
    );
    assert!(u.powered(e).is_none());
    for _ in 0..20 {
        u.step();
    }
    assert!(u.dormant(e).unwrap().groups.lights);
    assert!(u.structure(e).unwrap().light_powered[6]);
    assert_eq!(
        u.action_group(e, ose_sim::ActionGroup::Lights, None),
        Some(false)
    );
    for _ in 0..20 {
        u.step();
    }
    assert!(!u.structure(e).unwrap().light_powered[6]);
}

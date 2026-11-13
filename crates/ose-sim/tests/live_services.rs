//! Production service paths checked against the bounded KSP2 runtime observations in services-live.json.
use ose_data::{
    craft::{Craft, CraftPart, Quat, StoredResource},
    science::{ExperimentDef, ExperimentKind, ResearchReport, ScienceCatalog},
    PartDef, ResourceDef,
};
use ose_math::DVec3;
use ose_sim::{
    commnet::{GroundStation, KSC_RANGE_M},
    flight::{FlightSpec, StageSpec},
    science::Transmission,
    structure::Structure,
    Universe,
};

fn fixture(size: f64) -> (Universe, bevy_ecs::entity::Entity) {
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
        "../../../assets/own/kerbol-system.json"
    ))
    .unwrap();
    let mut u = Universe::with_bodies(&bodies);
    let body = u.body_entity("Kerbin").unwrap();
    u.set_ground_station(GroundStation {
        body,
        lat_deg: 0.,
        lon_deg: 0.,
        altitude_m: 0.,
        range_m: KSC_RANGE_M,
    });
    let craft = Craft {
        name: "Live services".into(),
        description: String::new(),
        colors: None,
        loose: vec![],
        stages: vec![vec![]],
        parts: ["pod_1v_conical_crew", "antenna_0v_16"]
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let d = defs.iter().find(|d| d.id == *id).unwrap();
                CraftPart {
                    guid: format!("service-{i}"),
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
                    fuel_line_target: None,
                    resources: d
                        .resource_containers
                        .iter()
                        .map(|r| StoredResource {
                            resource: r.resource.clone(),
                            stored_units: r.capacity_units,
                            capacity_units: r.capacity_units,
                        })
                        .collect(),
                    flight: None,
                    colors: None,
                    actions: vec![],
                    symmetry: 0,
                    shape: None,
                    fairing: None,
                    chute: None,
                }
            })
            .collect(),
    };
    let e = u.spawn_powered_on_surface(
        "Live services",
        body,
        FlightSpec {
            stages: vec![StageSpec {
                start_mass_t: 1.,
                ..Default::default()
            }],
            drag_area_m2: 0.,
            drag_cd: 0.,
        },
        0.,
        0.01,
    );
    u.set_craft(e, craft.clone());
    u.set_structure(e, Structure::build(&craft, &defs, &resources));
    u.deploy_antennas(e, true);
    u.set_controls(e, 0., ose_sim::flight::Attitude::Free);
    u.set_science(
        ScienceCatalog {
            experiments: vec![ExperimentDef {
                id: "MeasuredPacket".into(),
                display_name_key: String::new(),
                kind: ExperimentKind::Both,
                valid: vec![],
                data_value: 8.,
                sample_value: 18.,
                transmission_size: size,
                requires_eva: false,
            }],
            bodies: vec![],
        },
        vec![],
    );
    u.set_science_carried(e, vec![report(false), report(true)]);
    u.update_commnet();
    (u, e)
}
fn report(sample: bool) -> ResearchReport {
    ResearchReport {
        experiment: "MeasuredPacket".into(),
        sample,
        location: "Kerbin_Landed".into(),
        value: if sample { 18. } else { 8. },
    }
}
fn charge(u: &Universe, e: bevy_ecs::entity::Entity) -> f64 {
    let s = u.structure(e).unwrap();
    let ec = s.net.resource("ElectricCharge").unwrap();
    s.net
        .tanks
        .iter()
        .filter(|t| t.resource == ec)
        .map(|t| t.stored)
        .sum()
}
fn fill(u: &mut Universe, e: bevy_ecs::entity::Entity, value: f64) {
    let mut s = u.world.get_mut::<Structure>(e).unwrap();
    let ec = s.net.resource("ElectricCharge").unwrap();
    for t in &mut s.net.tanks {
        if t.resource == ec {
            t.stored = value.min(t.capacity);
        }
    }
}
fn step(u: &mut Universe, n: usize) {
    for _ in 0..n {
        u.step();
    }
}

#[test]
fn native_single_packet_duration_charge_and_sample_retention() {
    let evidence: serde_json::Value =
        serde_json::from_str(include_str!("data/services-live.json")).unwrap();
    let (mut u, e) = fixture(5.);
    let before = charge(&u, e);
    assert_eq!(u.start_transmission(e), Ok(1));
    step(&mut u, 99);
    assert!(u.take_transmitted().is_empty());
    u.step();
    let sent = u.take_transmitted();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].1.sample);
    assert!(
        (u.ut()
            - evidence["transmitter"]["completion_sim_s"]
                .as_f64()
                .unwrap())
        .abs()
            <= 0.021
    );
    assert!(
        (before - charge(&u, e) - evidence["transmitter"]["ec_consumed"].as_f64().unwrap()).abs()
            < 1e-6
    );
    assert_eq!(u.science_carried(e), vec![report(true)]);
    assert!(
        u.start_transmission(e).is_err(),
        "samples never enter the transmitter"
    );
}

#[test]
fn native_power_stop_requires_restart_and_keeps_partial_report() {
    let (mut u, e) = fixture(50.);
    u.start_transmission(e).unwrap();
    step(&mut u, 50);
    fill(&mut u, e, 0.);
    u.step();
    let stopped = u.transmission(e).unwrap();
    assert_eq!(stopped.3, Some("out of electric charge"));
    let progress = stopped.1;
    assert_eq!(u.science_carried(e).len(), 2);
    fill(&mut u, e, 50.);
    step(&mut u, 100);
    assert!(u.take_transmitted().is_empty());
    assert_eq!(
        u.transmission(e).unwrap().1,
        progress,
        "power restoration is not a restart"
    );
    assert!(
        (charge(&u, e) - 50.).abs() < 1e-6,
        "stopped transmitter draws no EC"
    );
    u.start_transmission(e).unwrap();
    assert!(
        (u.transmission(e).unwrap().1 - 0.1).abs() < 1e-12,
        "explicit native restart rounds partial packets up"
    );
    step(&mut u, 20);
    assert!(u.transmission(e).unwrap().1 > progress);
}

#[test]
fn periodic_link_loss_and_reconnection_do_not_implicitly_restart() {
    let (mut u, e) = fixture(500.);
    u.start_transmission(e).unwrap();
    u.step();
    for connected in [false, true] {
        let start = u.tick().0;
        {
            let mut st = u.world.get_mut::<Structure>(e).unwrap();
            for d in &mut st.electric {
                if d.comm_range_m > 0. {
                    d.comm_range_m = if connected { 2e8 } else { 1. };
                }
            }
        }
        while u.comm_link(e).connected != connected && u.tick().0 - start < 151 {
            u.step();
        }
        assert_eq!(u.comm_link(e).connected, connected);
        assert_eq!(
            u.tick().0 - start,
            150,
            "same phase in the measured three-second graph cycle"
        );
    }
    let done = u.transmission(e).unwrap().1;
    fill(&mut u, e, 50.);
    step(&mut u, 100);
    assert_eq!(u.transmission(e).unwrap().1, done);
    assert!(u.take_transmitted().is_empty());
    assert_eq!(u.world.get::<Transmission>(e).unwrap().stopped.len(), 1);
}

#[test]
fn retracted_active_antenna_stops_even_with_other_antenna_available() {
    let (mut u, e) = fixture(50.);
    u.start_transmission(e).unwrap();
    step(&mut u, 10);
    u.deploy_antennas(e, false);
    u.step();
    assert_eq!(u.transmission(e).unwrap().3, Some("no working antenna"));
    let before = charge(&u, e);
    u.deploy_antennas(e, true);
    u.set_controls(e, 0., ose_sim::flight::Attitude::Free);
    step(&mut u, 10);
    assert!((before - charge(&u, e)).abs() < 1e-6);
}

#[test]
fn recovery_before_publication_includes_data_once_and_cancels_delivery() {
    let (mut u, e) = fixture(5.);
    u.start_transmission(e).unwrap();
    step(&mut u, 100);
    let reports = u.take_science(e);
    assert_eq!(reports.len(), 2);
    assert!(u.take_transmitted().is_empty());
    assert!(u.transmission(e).is_none());
    assert!(u.take_science(e).is_empty());
}

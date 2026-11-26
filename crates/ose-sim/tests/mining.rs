//! Mining (`docs/colonies-design.md`, in-situ resources): a drill switched on digs Ore in proportion to the
//! ore in the ground where its vessel stands (`ose_data::abundance`), for electric charge, and a refinery
//! makes Ore into methane and oxidizer.
use bevy_ecs::entity::Entity;
use ose_data::{
    abundance,
    colony::{DRILL, ORE, REFINERY},
    craft::{Craft, CraftPart, Quat, StoredResource},
    PartDef, ResourceDef,
};
use ose_math::DVec3;
use ose_sim::{
    flight::{FlightSpec, StageSpec},
    structure::Structure,
    Universe,
};

fn craft(defs: &[PartDef], parts: &[&str]) -> Craft {
    Craft {
        name: "Miner".into(),
        description: String::new(),
        colors: None,
        loose: vec![],
        stages: vec![vec![]],
        parts: parts
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let d = defs.iter().find(|d| d.id == *id).unwrap();
                CraftPart {
                    shroud_shields: None,
                    guid: format!("miner-{i}"),
                    part_id: (*id).into(),
                    parent: i.checked_sub(1),
                    own_node: None,
                    parent_node: None,
                    local_position: DVec3::new(i as f64 * 2.0, 0., 0.),
                    local_rotation: Quat {
                        w: 1.,
                        ..Default::default()
                    },
                    stage_index: 0,
                    fuel_crossfeed: true,
                    fuel_line_target: None,
                    // a methalox tank holds methane and oxidizer, as a saved craft has it
                    resources: d
                        .resource_containers
                        .iter()
                        .flat_map(|r| {
                            match r.resource.as_str() {
                                "Methalox" => vec![("Methane", 0.2), ("Oxidizer", 0.8)],
                                id => vec![(id, 1.0)],
                            }
                            .into_iter()
                            .map(|(id, share)| StoredResource {
                                resource: id.into(),
                                stored_units: r.initial_units * share,
                                capacity_units: r.capacity_units * share,
                            })
                        })
                        .collect(),
                    flight: None,
                    colors: None,
                    actions: vec![],
                    symmetry: 0,
                    shape: None,
                    fairing: None,
                    chute: None,
                    control: None,
                }
            })
            .collect(),
    }
}

fn universe() -> (Universe, Vec<PartDef>, Vec<ResourceDef>) {
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
        "../../../assets/own/kerbol-system.json"
    ))
    .unwrap();
    (Universe::with_bodies(&bodies), defs, resources)
}

fn land(
    u: &mut Universe,
    defs: &[PartDef],
    resources: &[ResourceDef],
    parts: &[&str],
    body: &str,
    lat: f64,
    lon: f64,
) -> Entity {
    let c = craft(defs, parts);
    let b = u.body_entity(body).unwrap();
    let e = u.spawn_powered_on_surface(
        "Miner",
        b,
        FlightSpec {
            stages: vec![StageSpec {
                start_mass_t: 6.,
                ..Default::default()
            }],
            drag_area_m2: 0.,
            drag_cd: 0.,
        },
        lat,
        lon,
    );
    u.set_craft(e, c.clone());
    u.set_structure(e, Structure::build(&c, defs, resources));
    e
}

fn units(u: &Universe, e: Entity, resource: &str) -> f64 {
    let s = u.structure(e).unwrap();
    let Some(id) = s.net.resource(resource) else {
        return 0.0;
    };
    s.net
        .tanks
        .iter()
        .filter(|t| t.resource == id)
        .map(|t| t.stored)
        .sum()
}

fn set(u: &mut Universe, e: Entity, resource: &str, value: f64) {
    let mut s = u.world.get_mut::<Structure>(e).unwrap();
    let id = s.net.resource(resource).unwrap();
    for t in &mut s.net.tanks {
        if t.resource == id {
            t.stored = value.min(t.capacity);
        }
    }
}

/// The richest of a few places on a body, and one with none.
fn rich_and_barren(body: &str) -> ((f64, f64), (f64, f64)) {
    let places: Vec<(f64, f64)> = (0..2000)
        .map(|i| {
            (
                -60.0 + (i % 40) as f64 * 3.0,
                -180.0 + (i / 40) as f64 * 7.2,
            )
        })
        .collect();
    let at = |p: &(f64, f64)| abundance::ore(body, p.0, p.1);
    let rich = *places
        .iter()
        .max_by(|a, b| at(a).total_cmp(&at(b)))
        .unwrap();
    let barren = *places.iter().find(|p| at(p) == 0.0).unwrap();
    (rich, barren)
}

#[test]
fn a_landed_drill_digs_ore_by_the_ground_it_stands_on() {
    let (mut u, defs, resources) = universe();
    let ((lat, lon), (blat, blon)) = rich_and_barren("Mun");
    let share = abundance::ore("Mun", lat, lon);
    assert!(share > 0.5, "richest ground {share}");
    // a fission reactor (50 EC/s) runs the drill (15 EC/s)
    let miner = ["pod_1v_conical_crew", DRILL, "generator_1v_fission_reactor"];
    let rich = land(&mut u, &defs, &resources, &miner, "Mun", lat, lon);
    let barren = land(&mut u, &defs, &resources, &miner, "Mun", blat, blon);
    for e in [rich, barren] {
        assert_eq!(u.set_converters(e, true), Some(2));
    }
    let start = u.ut();
    for _ in 0..500 {
        u.step();
    }
    let dt = u.ut() - start;
    assert!(u.powered(rich).unwrap().landed);
    // 0.5 Ore a second on the richest ground, scaled by the share where it stands
    let dug = units(&u, rich, ORE);
    let expected = 0.5 * share * dt;
    assert!(
        (dug - expected).abs() < 0.05 * expected,
        "dug {dug:.2} Ore in {dt:.1} s, expected {expected:.2}"
    );
    assert_eq!(units(&u, barren, ORE), 0.0);

    // through time warp it keeps digging at the same rate, and stops when its hold is full
    set(&mut u, rich, ORE, 0.0);
    u.set_warp(1000);
    let start = u.ut();
    while u.ut() - start < 100.0 {
        u.step();
    }
    let dt = u.ut() - start;
    assert!(u.warp() > 4 && u.powered(rich).unwrap().landed);
    let dug = units(&u, rich, ORE);
    let expected = 0.5 * share * dt;
    assert!(
        (dug - expected).abs() < 0.05 * expected,
        "at warp: dug {dug:.2} Ore in {dt:.1} s, expected {expected:.2}"
    );
    while u.ut() - start < 600.0 {
        u.step();
    }
    assert!((units(&u, rich, ORE) - 100.0).abs() < 1e-6);
    u.set_warp(1);

    // without charge it stops
    set(&mut u, rich, ORE, 0.0);
    assert!(u.part_action(rich, 2, "converter", false));
    set(&mut u, rich, "ElectricCharge", 0.0);
    for _ in 0..100 {
        u.step();
    }
    assert!(units(&u, rich, ORE) < 1e-6);
}

#[test]
fn the_refinery_makes_ore_into_methalox() {
    let (mut u, defs, resources) = universe();
    let e = land(
        &mut u,
        &defs,
        &resources,
        &[
            "pod_1v_conical_crew",
            REFINERY,
            "fueltank_1v_inline_methalox_1x4",
        ],
        "Mun",
        0.0,
        0.0,
    );
    set(&mut u, e, ORE, 1000.0);
    set(&mut u, e, "Methane", 0.0);
    set(&mut u, e, "Oxidizer", 0.0);
    assert_eq!(u.set_converters(e, true), Some(1));
    let start = u.ut();
    for _ in 0..300 {
        set(&mut u, e, "ElectricCharge", 1e9);
        u.step();
    }
    let dt = u.ut() - start;
    let used = 1000.0 - units(&u, e, ORE);
    assert!(
        (used - 2.0 * dt).abs() < 0.05 * 2.0 * dt,
        "used {used:.1} Ore in {dt:.1} s"
    );
    let (ch4, ox) = (units(&u, e, "Methane"), units(&u, e, "Oxidizer"));
    assert!(
        (ox / ch4 - 4.0).abs() < 0.01,
        "{ch4} methane, {ox} oxidizer"
    );
    // nine tenths of the Ore's mass comes out as fuel
    assert!((ch4 + ox - 0.9 * used * 0.01).abs() < 1e-3);
}

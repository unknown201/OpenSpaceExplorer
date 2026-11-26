//! Runtime measurement of production electrical, CommNet and flag APIs; no reference values supplied.
use ose_data::{
    craft::{Craft, CraftPart, Quat, StoredResource},
    PartDef, ResourceDef,
};
use ose_math::DVec3;
use ose_sim::{
    bodies,
    commnet::{GroundStation, KSC_RANGE_M},
    flight::{FlightSpec, Powered},
    structure::Structure,
    Universe,
};
use serde_json::json;

fn charge(s: &Structure) -> f64 {
    let ec = s.net.resource("ElectricCharge").unwrap();
    s.net
        .tanks
        .iter()
        .filter(|t| t.resource == ec)
        .map(|t| t.stored)
        .sum()
}
fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or(".cache/services-parity".into());
    std::fs::create_dir_all(&out).unwrap();
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let ids = [
        "pod_1v_conical_crew",
        "fueltank_1v_inline_methalox_1x2",
        "solararray_0v_flat",
        "solararray_1v_shielded_1x6",
        "antenna_0v_16",
    ];
    let positions = [
        [0., 0., 0.],
        [0., -1.2775, 0.],
        [1., -1., 0.],
        [-1., -1., 0.],
        [0., -1., 1.],
    ];
    let craft = Craft {
        colors: None,
        loose: vec![],
        name: "Services validation".into(),
        description: "Matched part list; controlled runtime fixture".into(),
        stages: vec![vec![]],
        parts: ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let d = defs.iter().find(|d| d.id == *id).unwrap();
                CraftPart {
                    shroud_shields: None,
                    guid: format!("services-{i}"),
                    part_id: (*id).into(),
                    parent: (i > 0).then_some(if i == 1 { 0 } else { 1 }),
                    own_node: None,
                    parent_node: None,
                    local_position: DVec3::new(positions[i][0], positions[i][1], positions[i][2]),
                    local_rotation: Quat {
                        w: 1.,
                        ..Default::default()
                    },
                    stage_index: 0,
                    fuel_crossfeed: true,
                    fuel_line_target: None,
                    colors: None,
                    flight: None,
                    actions: vec![],
                    symmetry: 0,
                    shape: None,
                    fairing: None,
                    chute: None,
                    control: None,
                    resources: d
                        .resource_containers
                        .iter()
                        .map(|r| StoredResource {
                            resource: r.resource.clone(),
                            stored_units: r.capacity_units * 0.2,
                            capacity_units: r.capacity_units,
                        })
                        .collect(),
                }
            })
            .collect(),
    };
    std::fs::write(
        format!("{out}/ose-craft.json"),
        serde_json::to_vec_pretty(&craft).unwrap(),
    )
    .unwrap();
    let mut rows = vec![];
    for panel in [2, 3] {
        for angle in [0_f64, 90., 180.] {
            for sun in [0., 1.] {
                let mut c = craft.clone();
                let a = angle.to_radians() / 2.;
                c.parts[panel].local_rotation = Quat {
                    x: 0.,
                    y: a.sin(),
                    z: 0.,
                    w: a.cos(),
                };
                let mut s = Structure::build(&c, &defs, &res);
                // Isolate each real producer without substituting its output/rate.
                s.rigid.present[if panel == 2 { 3 } else { 2 }] = false;
                s.deploy_panels(true);
                let mut p = Powered::new(
                    FlightSpec {
                        stages: vec![],
                        drag_area_m2: 0.,
                        drag_cd: 0.,
                    },
                    DVec3::new(700000., 0., 0.),
                    DVec3::ZERO,
                    false,
                );
                let before = charge(&s);
                for _ in 0..50 {
                    s.electric_services(&mut p, sun, 0.02);
                }
                rows.push(json!({"case":"solar", "part":ids[panel], "angle_deg":angle,"sun":sun,"duration_s":1.,"ec_before":before,"ec_after":charge(&s),"delta_ec":charge(&s)-before}));
            }
        }
    }
    let mut u = Universe::with_bodies(&bodies);
    let body = u.body_entity("Kerbin").unwrap();
    u.set_ground_station(GroundStation {
        body,
        lat_deg: -0.6274,
        lon_deg: -74.8305,
        altitude_m: 221.7,
        range_m: KSC_RANGE_M,
    });
    let e = u.spawn_vessel_in_circular_orbit("link probe", body, 100000.);
    u.set_structure(e, Structure::build(&craft, &defs, &res));
    u.update_commnet();
    rows.push(
        json!({"case":"link_initial","tick":u.tick().0,"connected":u.comm_link(e).connected}),
    );
    for trial in 0..3 {
        for range in [1., 2.0e8] {
            let before = u.tick().0;
            {
                let mut st = u.world.get_mut::<Structure>(e).unwrap();
                for x in &mut st.electric {
                    if x.comm_range_m > 0. {
                        x.comm_range_m = range;
                    }
                }
            }
            let wanted = range > 1.;
            while u.comm_link(e).connected != wanted && u.tick().0 - before <= 51 {
                u.step();
            }
            assert_eq!(
                u.comm_link(e).connected,
                wanted,
                "Link transition was not observed"
            );
            rows.push(json!({"case":"link_transition","trial":trial,"range_m":range,"trigger_tick":before,"observed_tick":u.tick().0,"connected":u.comm_link(e).connected,"latency_sim_s":(u.tick().0-before) as f64/ose_sim::TICKS_PER_SECOND as f64}));
            for _ in 0..17 {
                u.step();
            }
        }
    }
    let planet = u.world.get::<bodies::Body>(body).unwrap();
    let radius = planet.radius_m + 100000.;
    let to_star = -bodies::star_space_state(&u.world, body, u.ut())
        .0
        .normalized();
    for (name, r) in [("lit", to_star * radius), ("eclipse", -to_star * radius)] {
        rows.push(json!({"case":"body_shading","side":name,"sun":u.sunlight(body,r,u.ut())}));
    }
    let home = u.spawn_powered_on_surface(
        "flag pod",
        body,
        FlightSpec {
            stages: vec![],
            drag_area_m2: 0.,
            drag_cd: 0.,
        },
        -0.1,
        -74.499,
    );
    u.set_structure(home, Structure::build(&craft, &defs, &res));
    let eva = u.go_eva(home, None).unwrap();
    // Controlled grounded fixture: the free simulation API's surface placement, no camera/UI claim.
    let mut p = u.powered(eva).unwrap().clone();
    p.landed = true;
    u.set_powered(eva, p);
    for trial in 0..2 {
        let flag = u.plant_flag(eva, "Services flag").unwrap();
        rows.push(json!({"case":"flag_placement","trial":trial,"distance_m":u.distance_between(eva,flag),"planter":u.flag_planter(flag),"tick":u.tick().0}));
    }
    std::fs::write(
        format!("{out}/ose-sim.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    println!(
        "{} runtime measurements written to {out}/ose-sim.json",
        rows.len()
    );
}

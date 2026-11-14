//! Rendezvous v2 scenario T1: launch a station core from KSC, then launch a second craft and let the
//! pilot fly it from the pad to a docked joint on the station, with ordinary controls only.
use bevy_ecs::entity::Entity;
use ose_data::{craft::Craft, CelestialBody, PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use ose_pilot::{
    ascent::{Ascent, AscentParams},
    mission::{Catalog, Leg, Mission, MissionParams},
    Status,
};
use ose_sim::{
    flight::{Attitude, FlightSpec, StageInertia},
    structure::Structure,
    Universe,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    core: Craft,
    chaser: Craft,
    incoming: String,
    target: String,
}

/// The flight spec the ISS assembly test pilot builds for a craft (staging, inertia, sea-level Isp).
fn spec(c: &Craft, defs: &[PartDef], resources: &[ResourceDef]) -> FlightSpec {
    let report = ose_data::staging::delta_v_by_stage(c, defs, resources);
    let def = |id: &str| defs.iter().find(|d| d.id == id).unwrap();
    let mass = |p: &ose_data::craft::CraftPart| {
        def(&p.part_id).mass_t
            + p.resources
                .iter()
                .map(|r| {
                    resources
                        .iter()
                        .find(|d| d.id.eq_ignore_ascii_case(&r.resource))
                        .map(|d| match d.kind {
                            ResourceKind::Basic {
                                mass_per_unit_t, ..
                            } => mass_per_unit_t * r.stored_units,
                            _ => 0.0,
                        })
                        .unwrap_or(0.0)
                })
                .sum::<f64>()
    };
    let inertia = report
        .stages
        .iter()
        .map(|st| {
            let total = st
                .alive
                .iter()
                .map(|i| mass(&c.parts[*i]))
                .sum::<f64>()
                .max(0.001);
            let centre = st.alive.iter().fold(DVec3::ZERO, |v, i| {
                v + c.parts[*i].local_position * mass(&c.parts[*i])
            }) * total.recip();
            let (mut ix, mut iz, mut wheel, mut gimbal) = (0.0_f64, 0.0_f64, 0.0, 0.0);
            for i in &st.alive {
                let p = &c.parts[*i];
                let d = def(&p.part_id);
                let m = mass(p);
                let r = p.local_position - centre;
                let size = d
                    .bounds
                    .map(|b| b.size_m)
                    .unwrap_or(DVec3::new(1.0, 1.0, 1.0));
                ix += m * (r.y * r.y + r.z * r.z + (size.y * size.y + size.z * size.z) / 12.0);
                iz += m * (r.x * r.x + r.y * r.y + (size.x * size.x + size.y * size.y) / 12.0);
                wheel += d.reaction_wheel_torque_knm;
            }
            for i in &st.engines {
                let p = &c.parts[*i];
                if let Some(e) = def(&p.part_id).engines.first() {
                    gimbal += e.max_thrust_kn
                        * e.gimbal_range_deg.unwrap_or(0.0).to_radians().sin()
                        * (p.local_position.y - centre.y).abs();
                }
            }
            StageInertia {
                moi_tm2: ix.max(iz),
                wheel_torque_knm: wheel,
                gimbal_torque_knm: gimbal,
            }
        })
        .collect::<Vec<_>>();
    let asl = report
        .stages
        .iter()
        .map(|st| {
            let (mut n, mut d) = (0.0, 0.0);
            for i in &st.engines {
                for e in &def(&c.parts[*i].part_id).engines {
                    n += e.max_thrust_kn * e.isp_curve.evaluate(1.0);
                    d += e.max_thrust_kn;
                }
            }
            if d > 0.0 {
                n / d
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    let area = c
        .parts
        .iter()
        .filter_map(|p| {
            def(&p.part_id)
                .bounds
                .map(|b| b.size_m.x * b.size_m.z * std::f64::consts::FRAC_PI_4)
        })
        .fold(0.1, f64::max);
    FlightSpec::from_report(&report, &asl, &inertia, area)
}

fn on_pad(
    u: &mut Universe,
    body: Entity,
    craft: &Craft,
    defs: &[PartDef],
    res: &[ResourceDef],
) -> Entity {
    let e =
        u.spawn_powered_on_surface(&craft.name, body, spec(craft, defs, res), -0.6836, -74.8198);
    u.set_craft(e, craft.clone());
    u.set_structure(e, Structure::build(craft, defs, res));
    e
}

#[test]
fn t1_pilot_flies_from_the_pad_to_a_docked_joint_on_a_station_in_low_orbit() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("data/t1-crafts.json")).expect("fixture");
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies: Vec<CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut u = Universe::with_bodies(&bodies);
    u.set_part_catalog(defs.clone(), resources.clone());
    let kerbin = u.body_entity("Kerbin").unwrap();
    let catalog = Catalog {
        defs: &defs,
        resources: &resources,
    };
    let in_orbit = AscentParams {
        stage_in_orbit: true,
        ..AscentParams::default()
    };

    // The station: an ordinary ascent flown by the same pilot.
    let station = on_pad(&mut u, kerbin, &fixture.core, &defs, &resources);
    let mut ascent = Ascent::new(in_orbit.clone());
    loop {
        match ascent.update(&mut u, station) {
            Status::Running => {}
            Status::Done => break,
            Status::Failed(why) => panic!("station ascent: {why}"),
        }
        u.set_warp(ascent.suggested_warp());
        u.step();
    }
    let station_parts = u.current_craft(station).unwrap().parts.len();
    let o = ose_pilot::orbit(&u, station).unwrap();
    let (_, _, radius) = ose_pilot::body(&u, station).unwrap();
    assert!(o.periapsis() > radius + 85_000.0, "station orbit {o:?}");

    // The delivery: pad to dock.
    let chaser = on_pad(&mut u, kerbin, &fixture.chaser, &defs, &resources);
    let mut params = MissionParams::new(station, &fixture.target, &fixture.incoming);
    params.ascent = Some(in_orbit);
    let mut mission = Mission::new(params);
    let started = u.ut();
    let mut held = false;
    let result = loop {
        let status = mission.update(&mut u, chaser, &catalog);
        if status != Status::Running {
            break status;
        }
        // A depot doesn't steer itself, but it holds its attitude like a parked station would.
        if mission.leg() == Leg::Approach && !held {
            u.wake(station);
            let facing = u.powered(station).unwrap().facing;
            u.set_controls(station, 0.0, Attitude::Fixed(facing));
            held = true;
        }
        assert!(
            u.ut() - started < 6.0 * 3600.0,
            "mission timed out: {:?}",
            mission.log
        );
        u.set_warp(mission.suggested_warp(&u, chaser));
        u.step();
        u.dock_latched(&defs, &resources, &|c| spec(c, &defs, &resources));
    };
    for line in &mission.log {
        println!("{line}");
    }
    assert_eq!(result, Status::Done, "{:?}", mission.log);

    // One vessel now, holding every part of both, with one new docked joint.
    let kept = [station, chaser]
        .into_iter()
        .find(|e| u.current_craft(*e).is_some())
        .expect("the docked vessel");
    let craft = u.current_craft(kept).unwrap();
    for guid in [&fixture.target, &fixture.incoming] {
        assert!(craft.parts.iter().any(|p| &p.guid == guid), "lost {guid}");
    }
    assert!(craft.parts.len() > station_parts);
    assert_eq!(u.docked_links(kept).len(), 1);
}

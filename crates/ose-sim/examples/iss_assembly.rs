//! Repeatable ISS replica validation using ordinary launch, burn and RCS controls. No scheduled
//! impulsive burns, teleporting deliveries or direct craft merging. Generated saves open in ose-server.
use bevy_ecs::entity::Entity;
use ose_data::{craft::Craft, PartDef, ResourceDef, ResourceKind};
use ose_math::{rendezvous, DVec3, Orbit};
use ose_sim::{
    bodies,
    flight::{self, Attitude, FlightSpec, StageInertia},
    structure::Structure,
    Universe,
};
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path};

#[derive(Deserialize)]
struct Module {
    name: String,
    payload: Craft,
    launcher: Craft,
    incoming: String,
}
#[derive(Deserialize)]
struct Delivery {
    module: String,
    target: String,
}
#[derive(Deserialize)]
struct Blueprints {
    modules: Vec<Module>,
    deliveries: Vec<Delivery>,
}

fn spec(c: &Craft, defs: &[PartDef], resources: &[ResourceDef]) -> FlightSpec {
    let report = ose_data::staging::delta_v_by_stage(c, defs, resources);
    let mass = |p: &ose_data::craft::CraftPart| {
        let dry = defs.iter().find(|d| d.id == p.part_id).unwrap().mass_t;
        dry + p
            .resources
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
                let d = defs.iter().find(|d| d.id == p.part_id).unwrap();
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
                let d = defs.iter().find(|d| d.id == p.part_id).unwrap();
                if let Some(e) = d.engines.first() {
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
                for e in &defs
                    .iter()
                    .find(|d| d.id == c.parts[*i].part_id)
                    .unwrap()
                    .engines
                {
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
            defs.iter()
                .find(|d| d.id == p.part_id)?
                .bounds
                .map(|b| b.size_m.x * b.size_m.z * std::f64::consts::FRAC_PI_4)
        })
        .fold(0.1, f64::max);
    FlightSpec::from_report(&report, &asl, &inertia, area)
}
fn state(u: &Universe, e: Entity) -> (DVec3, DVec3) {
    u.powered(e).map(|p| (p.r, p.v)).unwrap_or_else(|| {
        u.world
            .get::<bodies::OnRails>(e)
            .unwrap()
            .0
            .state_at(u.ut())
    })
}
fn orbit(u: &Universe, e: Entity, mu: f64) -> Orbit {
    let (r, v) = state(u, e);
    Orbit::from_state_vectors(mu, r, v, u.ut())
}
fn advance(u: &mut Universe, seconds: f64) {
    let until = u.ut() + seconds;
    while u.ut() < until - 0.005 {
        let ticks = ((until - u.ut()) * 50.0).round() as u32;
        u.set_warp(ticks.clamp(1, 1000));
        u.step();
    }
    u.set_warp(1);
}
fn saved(u: &Universe, entries: &[(Entity, u64)], crafts: &Blueprints, out: &Path, name: &str) {
    let vessels=entries.iter().filter(|(e,_)|u.vessel(*e).is_some()).map(|(e,id)|{
        let o=u.world.get::<bodies::OnRails>(*e).unwrap().0;
        json!({"id":id,"name":u.vessel(*e).unwrap().0.name,"owner":1,"body":"Kerbin","elements":o.elements,
            "burn":null,"flight":u.powered(*e),"dormant":u.dormant(*e),"parts":u.saved_parts(*e),
            "craft":u.current_craft(*e),"crew":u.crew(*e),"docked":u.docked_links(*e)})
    }).collect::<Vec<_>>();
    let library = crafts
        .modules
        .iter()
        .enumerate()
        .map(|(i, m)| json!({"id":1000+i,"owner":1,"craft":m.launcher}))
        .collect::<Vec<_>>();
    let save = json!({"version":2,"tick":u.tick(),"warp":1,"vessels":vessels,"crafts":library,
        "campaign":{"id":"shared","name":"ISS rendezvous validation","started_tick":0,"members":[],"mode":"Sandbox","difficulty":[]}});
    fs::write(out.join(name), serde_json::to_string_pretty(&save).unwrap()).unwrap();
}
fn launch(
    u: &mut Universe,
    body: Entity,
    module: &Module,
    defs: &[PartDef],
    resources: &[ResourceDef],
    mu: f64,
) -> Entity {
    let vehicle = &module.launcher;
    let e = u.spawn_powered_on_surface(
        &vehicle.name,
        body,
        spec(vehicle, defs, resources),
        -0.6836,
        -74.8198,
    );
    u.set_craft(e, vehicle.clone());
    u.set_structure(e, Structure::build(vehicle, defs, resources));
    u.stage(e);
    let start = u.ut();
    let mut coasting = false;
    let radius = u.world.get::<bodies::Body>(body).unwrap().radius_m;
    println!(
        "LAUNCH {} · {} parts · mass {:.1} t",
        module.name,
        vehicle.parts.len(),
        u.powered(e).unwrap().mass_t
    );
    for iteration in 0..150_000 {
        u.wake(e);
        let p = u.powered(e).unwrap().clone();
        assert!(!p.crashed, "{} crashed", module.name);
        let o = orbit(u, e, mu);
        let alt = p.r.length() - radius;
        if o.eccentricity() < 1.0 && o.periapsis() > radius + 85_000.0 {
            u.set_controls(e, 0.0, Attitude::OrbitPrograde);
            println!(
                "ORBIT {} · {:.0} × {:.0} km · T+{:.0}s",
                module.name,
                (o.periapsis() - radius) / 1000.0,
                (o.apoapsis() - radius) / 1000.0,
                u.ut() - start
            );
            // Release the launch stage; payload propulsion is a normal staged pair of radial engines.
            u.stage(e);
            u.set_controls(e, 0.0, Attitude::OrbitPrograde);
            u.deploy_panels(e, true);
            let arrived = u.current_craft(e).unwrap();
            let missing = module
                .payload
                .parts
                .iter()
                .filter(|p| !arrived.parts.iter().any(|a| a.guid == p.guid))
                .map(|p| p.guid.clone())
                .collect::<Vec<_>>();
            assert!(
                missing.is_empty(),
                "payload parts lost during launch: {missing:?}"
            );
            let facing = u.powered(e).unwrap().facing;
            u.set_controls(e, 0.0, Attitude::Fixed(facing));
            return e;
        }
        assert!(
            u.ut() - start < 1500.0,
            "{} failed to reach orbit: alt {alt:.0}, pe {:.0}, ap {:.0}",
            module.name,
            o.periapsis() - radius,
            o.apoapsis() - radius
        );
        let apo = o.apoapsis() - radius;
        if !coasting && o.eccentricity() < 1.0 && apo > 140_000.0 {
            coasting = true;
        }
        let (throttle, attitude) = if !coasting {
            let pitch = if alt < 1_000.0 {
                90.0
            } else {
                (90.0 * (1.0 - (alt - 1_000.0) / 44_000.0)).clamp(0.0, 90.0)
            };
            (
                if alt < 20_000.0 {
                    (p.mass_t * 9.81 * 1.5 / 4250.0).clamp(0.3, 1.0)
                } else {
                    1.0
                },
                Attitude::PitchHeading {
                    pitch_deg: pitch,
                    heading_deg: 90.0,
                },
            )
        } else {
            let time = o.time_to_true_anomaly(std::f64::consts::PI, u.ut());
            let circular = (mu / p.r.length()).sqrt();
            let needed = (circular - p.v.length()).max(0.0);
            let accel = p
                .spec
                .stages
                .get(p.stage)
                .map(|s| s.thrust_vac_kn)
                .unwrap_or(0.0)
                / p.mass_t;
            (
                if time < needed / accel.max(0.01) * 0.65 + 8.0 || time > o.period - 20.0 {
                    1.0
                } else {
                    0.0
                },
                Attitude::OrbitPrograde,
            )
        };
        if p.propellant_t < 0.01 {
            panic!(
                "{} launch propellant exhausted at {alt:.0} m, pe {:.0}, ap {:.0}",
                module.name,
                o.periapsis() - radius,
                o.apoapsis() - radius
            );
        }
        if iteration % 5 == 0 {
            u.set_controls(e, throttle, attitude);
        }
        u.set_warp(4);
        u.step();
    }
    unreachable!()
}
fn burn(u: &mut Universe, e: Entity, dv: DVec3, mu: f64) {
    u.wake(e);
    let direction = dv.normalized();
    u.set_controls(e, 0.0, Attitude::Fixed(direction));
    u.wake(e);
    let mut left = dv;
    let start = u.ut();
    while left.dot(direction) > 0.25 {
        u.wake(e);
        assert!(u.ut() - start < 500.0, "burn took too long: {:?}", left);
        let p = u.powered(e).unwrap().clone();
        let at = u.ut();
        let accel = p
            .spec
            .stages
            .get(p.stage)
            .map(|s| s.thrust_vac_kn)
            .unwrap_or(0.0)
            / p.mass_t;
        assert!(
            accel > 0.0 && p.propellant_t > 0.0,
            "no propulsion for burn"
        );
        let aligned = p.facing.dot(direction) > 0.999;
        let throttle = if aligned {
            (left.dot(direction) / accel / 0.5).clamp(0.0, 1.0)
        } else {
            0.0
        };
        u.set_controls(e, throttle, Attitude::Fixed(direction));
        u.set_warp(1);
        u.step();
        let after = state(u, e);
        left = left - rendezvous::delivered_delta_v(mu, p.r, p.v, after.0, after.1, u.ut() - at);
    }
    println!("BURN TIME {:.1}s", u.ut() - start);
    u.set_controls(e, 0.0, Attitude::Fixed(direction));
    u.set_warp(1);
    println!(
        "BURN delivered {:.2} m/s  -  residual {:.3} m/s",
        dv.length(),
        left.length()
    );
}

fn align(u: &mut Universe, e: Entity, direction: DVec3) {
    let started = u.ut();
    loop {
        u.wake(e);
        u.set_controls(e, 0.0, Attitude::Fixed(direction));
        let p = u.powered(e).unwrap();
        if p.facing.dot(direction) > 0.999 && p.angvel.length() < 0.001 && p.roll_rate.abs() < 0.001
        {
            break;
        }
        assert!(u.ut() - started < 600.0, "could not align for the burn");
        u.set_warp(4);
        u.step();
    }
}
fn port_pose(u: &Universe, e: Entity, guid: &str, defs: &[PartDef]) -> (DVec3, DVec3) {
    let craft = u.current_craft(e).unwrap();
    let port = ose_sim::docking::free_ports(&craft, defs)
        .into_iter()
        .find(|p| craft.parts[p.part].guid == guid)
        .unwrap_or_else(|| panic!("port {guid} is not free"));
    let p = u.powered(e).or_else(|| u.dormant(e)).unwrap();
    let centre = u.structure(e).unwrap().rigid.rest_centre();
    let st = u.structure(e).unwrap();
    let r = state(u, e).0;
    let position = r + st.offset_inertial(
        p.facing,
        DVec3::new(port.position.x, port.position.y, -port.position.z) - centre,
    );
    (position, st.axis_inertial(p.facing, port.axis))
}
#[allow(clippy::too_many_arguments)] // Test pilot needs the live catalog and destination's authored port.
fn dock(
    u: &mut Universe,
    station: Entity,
    chaser: Entity,
    incoming: &str,
    target: &str,
    defs: &[PartDef],
    resources: &[ResourceDef],
    mu: f64,
) -> Entity {
    u.set_target(chaser, Some(station));
    u.wake(station);
    u.wake(chaser);
    let facing = u.powered(station).unwrap().facing;
    u.set_controls(station, 0.0, Attitude::Fixed(facing));
    u.action_group(chaser, ose_sim::ActionGroup::Rcs, Some(true));
    let mut final_leg = false;
    let started = u.ut();
    let mut last_log = started;
    let mut reported_contact = false;
    let mut pulses = [0.0_f64; 3];
    for _ in 0..200_000 {
        u.wake(station);
        u.wake(chaser);
        let (a, axis) = port_pose(u, station, target, defs);
        let (b, own_axis) = port_pose(u, chaser, incoming, defs);
        let ps = u.powered(station).unwrap().clone();
        let pc = u.powered(chaser).unwrap().clone();
        let stage = a + axis * 120.0;
        let goal = if final_leg { a + axis * 0.15 } else { stage };
        let error = goal - b;
        let station_spin = ps.angvel + ps.facing * ps.roll_rate;
        let own_spin = pc.angvel + pc.facing * pc.roll_rate;
        let rel_v = pc.v + own_spin.cross(b - pc.r) - ps.v - station_spin.cross(goal - ps.r);
        let distance = error.length();
        if !final_leg && distance < 15.0 && rel_v.length() < 0.3 {
            final_leg = true;
            println!(
                "STAGING POINT reached · {:.1} m from port",
                (a - b).length()
            );
        }
        let top = flight::vessel_top(&pc);
        let right = pc.facing.cross(top);
        let turn = glam::DQuat::from_rotation_arc(
            glam::DVec3::new(own_axis.x, own_axis.y, own_axis.z),
            -glam::DVec3::new(axis.x, axis.y, axis.z),
        );
        let face = turn * glam::DVec3::new(pc.facing.x, pc.facing.y, pc.facing.z);
        u.set_controls(
            chaser,
            0.0,
            Attitude::Fixed(DVec3::new(face.x, face.y, face.z)),
        );
        let accel = pc.rcs.thrust_kn / pc.mass_t;
        assert!(accel > 0.001, "RCS unavailable");
        let max_speed = if final_leg {
            if distance < 5.0 {
                0.12
            } else {
                0.3
            }
        } else {
            (distance * 0.03).clamp(0.4, 15.0)
        };
        let desired = error.normalized() * (distance * 0.12).min(max_speed);
        let differential =
            pc.r * (-mu / pc.r.length().powi(3)) - ps.r * (-mu / ps.r.length().powi(3));
        let command = ((desired - rel_v) * 0.6 - differential) * (1.0 / accel);
        let command = if command.length() > 0.5 {
            command.normalized() * 0.5
        } else {
            command
        };
        // The real RCS mixer has a 0.05 input deadband. Pulse the precision-sized inputs like
        // a player tapping translation keys, rather than silently requesting unusable tiny thrust.
        let axes = [command.dot(right), command.dot(pc.facing), command.dot(top)];
        let axes: [f64; 3] = std::array::from_fn(|i| {
            if axes[i].abs() >= 0.05 {
                return axes[i];
            }
            pulses[i] += axes[i];
            if pulses[i].abs() >= 0.1 {
                let pulse = pulses[i].signum() * 0.1;
                pulses[i] -= pulse;
                pulse
            } else {
                0.0
            }
        });
        u.set_translate(chaser, DVec3::new(axes[0], axes[1], axes[2]));
        u.set_warp(if final_leg { 4 } else { 10 });
        u.step();
        let contacts = u.take_contacts();
        let face_delta = b - a;
        if !reported_contact && !contacts.is_empty() {
            for hit in contacts.iter().take(3) {
                let a = &u.structure(hit.a).unwrap().part_ids[hit.part_a];
                let b = &u.structure(hit.b).unwrap().part_ids[hit.part_b];
                let ga = &u
                    .world
                    .get::<ose_sim::docking::CraftDoc>(hit.a)
                    .unwrap()
                    .0
                    .parts[hit.part_a]
                    .guid;
                let gb = &u
                    .world
                    .get::<ose_sim::docking::CraftDoc>(hit.b)
                    .unwrap()
                    .0
                    .parts[hit.part_b]
                    .guid;
                println!(
                    "CONTACT {a} ({ga}) / {b} ({gb}) at port range {:.2} m, alignment {:.1} deg",
                    a_port_distance(u, station, chaser, incoming, target, defs),
                    (-axis.dot(own_axis)).clamp(-1.0, 1.0).acos().to_degrees()
                );
                let axial = face_delta.dot(axis);
                println!(
                    "PRE-CONTACT axial {axial:.3}, lateral {:.3}, range {:.3}",
                    (face_delta - axis * axial).length(),
                    face_delta.length()
                );
            }
            reported_contact = true;
        }
        let joined = u.dock_latched(defs, resources, &|c| spec(c, defs, resources));
        if let Some((kept, _)) = joined.first() {
            u.set_translate(*kept, DVec3::ZERO);
            u.set_controls(*kept, 0.0, Attitude::Fixed(ps.facing));
            u.set_warp(1);
            return *kept;
        }
        assert!(
            u.ut() - started < 2500.0,
            "docking timed out: {:.1} m, velocity {:?}",
            (a - b).length(),
            rel_v
        );
        if u.ut() - last_log > 100.0 {
            println!(
                "APPROACH range {:.1} m · speed {:.2} m/s · final {final_leg} · staging error {:.1}m · axis error {:.1}deg",
                (a - b).length(),
                rel_v.length(),
                distance,
                (-axis.dot(own_axis)).clamp(-1.0,1.0).acos().to_degrees()
            );
            last_log = u.ut();
        }
    }
    panic!("docking did not capture")
}
fn a_port_distance(
    u: &Universe,
    station: Entity,
    chaser: Entity,
    incoming: &str,
    target: &str,
    defs: &[PartDef],
) -> f64 {
    (port_pose(u, station, target, defs).0 - port_pose(u, chaser, incoming, defs).0).length()
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let out = Path::new(
        args.get(1)
            .expect("output directory generated by tools/build_iss_test_crafts.py"),
    );
    let limit = args
        .get(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let blueprints: Blueprints =
        serde_json::from_str(&fs::read_to_string(out.join("crafts.json")).unwrap()).unwrap();
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let body = bodies.iter().find(|b| b.id == "Kerbin").unwrap();
    let mu = body.mu;
    let radius = body.radius_m;
    let mut u = Universe::with_bodies(&bodies);
    u.set_part_catalog(defs.clone(), resources.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let approach_resume = args.iter().any(|s| s == "--approach");
    let station_resume = args.iter().any(|s| s == "--resume-station");
    let resume = args.iter().any(|s| s == "--resume") || approach_resume || station_resume;
    let mut restored = Vec::new();
    if resume {
        let checkpoint: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(out.join(if approach_resume {
                "approach-flight.json"
            } else if station_resume {
                "assembled-station.json"
            } else {
                "intercept-flight.json"
            }))
            .unwrap(),
        )
        .unwrap();
        u.set_tick(serde_json::from_value(checkpoint["tick"].clone()).unwrap());
        for v in checkpoint["vessels"].as_array().unwrap() {
            let e = u.spawn_vessel_with_elements(
                v["name"].as_str().unwrap(),
                body,
                serde_json::from_value(v["elements"].clone()).unwrap(),
            );
            if !v["flight"].is_null() {
                u.set_powered(e, serde_json::from_value(v["flight"].clone()).unwrap());
            } else {
                u.set_dormant(e, serde_json::from_value(v["dormant"].clone()).unwrap());
            }
            u.restore_parts(
                e,
                &serde_json::from_value(v["parts"].clone()).unwrap(),
                &defs,
                &resources,
            );
            u.set_docked(e, serde_json::from_value(v["docked"].clone()).unwrap());
            restored.push(e);
        }
        println!("RESUMED actual flight checkpoint at UT {:.0}", u.ut());
    }
    let mut station = if resume {
        restored[0]
    } else {
        launch(&mut u, body, &blueprints.modules[0], &defs, &resources, mu)
    };
    let resume_index = u.docked_links(station).len();
    if !resume {
        saved(&u, &[(station, 1)], &blueprints, out, "core-in-orbit.json");
    }
    for (index, delivery) in blueprints
        .deliveries
        .iter()
        .take(limit.saturating_sub(1))
        .enumerate()
        .skip(resume_index)
    {
        let module = blueprints
            .modules
            .iter()
            .find(|m| m.name == delivery.module)
            .unwrap();
        let chaser = if resume && !station_resume && index == resume_index {
            restored[1]
        } else {
            launch(&mut u, body, module, &defs, &resources, mu)
        };
        u.set_target(chaser, Some(station));
        u.action_group(chaser, ose_sim::ActionGroup::Rcs, Some(false));
        if !(approach_resume && index == resume_index) {
            saved(
                &u,
                &[(station, 1), (chaser, 2)],
                &blueprints,
                out,
                "intercept-flight.json",
            );
            let mut encounter = None;
            for correction in 0..12 {
                u.wake(chaser);
                let p = u.powered(chaser).unwrap();
                let accel = p.spec.stages[p.stage].thrust_vac_kn / p.mass_t;
                let budget = ose_data::staging::delta_v_by_stage(
                    &u.current_craft(chaser).unwrap(),
                    &defs,
                    &resources,
                )
                .total_vacuum_mps
                    * 0.9;
                let adjusted = encounter.and_then(|arrival| {
                    rendezvous::correction(
                        orbit(&u, chaser, mu),
                        orbit(&u, station, mu),
                        u.ut() + 360.0,
                        arrival,
                        radius + 71_000.0,
                        budget,
                    )
                });
                let plan = adjusted
                    .or_else(|| {
                        rendezvous::plan(
                            orbit(&u, chaser, mu),
                            orbit(&u, station, mu),
                            u.ut() + 360.0,
                            radius + 71_000.0,
                            budget,
                        )
                    })
                    .expect("reachable fuel-budgeted transfer");
                encounter = Some(plan.arrival_ut);
                println!("PLAN {} correction {}  -  departure in {:.1}s  -  burn {:.1} + braking {:.1} m/s", module.name, correction, plan.departure_ut-u.ut(), plan.delta_v.length(), plan.arrival_delta_v.length());
                align(&mut u, chaser, plan.delta_v.normalized());
                let wait =
                    (plan.departure_ut - u.ut() - plan.delta_v.length() / accel * 0.5).max(0.0);
                advance(&mut u, wait);
                println!(
                    "BURN START error {:.2}s",
                    u.ut() + plan.delta_v.length() / accel * 0.5 - plan.departure_ut
                );
                burn(&mut u, chaser, plan.delta_v, mu);
                let miss = (orbit(&u, chaser, mu).state_at(plan.arrival_ut).0
                    - orbit(&u, station, mu).state_at(plan.arrival_ut).0)
                    .length();
                println!("PREDICTED PASS {miss:.1} m");
                if miss > 1_500.0 && correction < 11 {
                    continue;
                }
                let p = u.powered(chaser).unwrap();
                let accel = p.spec.stages[p.stage].thrust_vac_kn / p.mass_t;
                let arrival_dv = orbit(&u, station, mu).state_at(plan.arrival_ut).1
                    - orbit(&u, chaser, mu).state_at(plan.arrival_ut).1;
                align(&mut u, chaser, arrival_dv.normalized());
                let wait =
                    (plan.arrival_ut - u.ut() - plan.arrival_delta_v.length() / accel * 0.5 - 10.0)
                        .max(0.0);
                advance(&mut u, wait);
                burn(&mut u, chaser, arrival_dv, mu);
                let range = (state(&u, station).0 - state(&u, chaser).0).length();
                println!("BRAKED RANGE {range:.1} m");
                if range < 8_000.0 {
                    break;
                }
                encounter = None;
                assert!(correction < 11, "encounter remained outside approach range");
            }
        }
        saved(
            &u,
            &[(station, 1), (chaser, 2)],
            &blueprints,
            out,
            "approach-flight.json",
        );
        station = dock(
            &mut u,
            station,
            chaser,
            &module.incoming,
            &delivery.target,
            &defs,
            &resources,
            mu,
        );
        let c = u.current_craft(station).unwrap();
        let guids = c
            .parts
            .iter()
            .map(|p| &p.guid)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(guids.len(), c.parts.len(), "part identities stay unique");
        for completed in blueprints.modules.iter().take(index + 2) {
            for part in &completed.payload.parts {
                assert!(
                    guids.contains(&part.guid),
                    "{} lost payload part {} during assembly",
                    completed.name,
                    part.guid
                );
            }
        }
        assert_eq!(u.docked_links(station).len(), index + 1);
        println!(
            "ATTACHED {} · {} parts · {} docked joints",
            module.name,
            c.parts.len(),
            u.docked_links(station).len()
        );
        saved(
            &u,
            &[(station, 1)],
            &blueprints,
            out,
            "assembled-station.json",
        );
    }
    u.deploy_panels(station, true);
    saved(
        &u,
        &[(station, 1)],
        &blueprints,
        out,
        "assembled-station.json",
    );
    let report = json!({"pass":u.docked_links(station).len()==blueprints.deliveries.len(),
        "modules":u.docked_links(station).len()+1,"docked_joints":u.docked_links(station).len(),
        "parts":u.current_craft(station).unwrap().parts.len(),"ut_s":u.ut(),
        "method":"Surface launches, finite burns, RCS translation and authoritative docking capture; no teleport or direct merge"});
    fs::write(
        out.join("assembly-report.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "PASS · ISS replica: {} parts, {} independently launched deliveries · UT {:.0}s",
        u.current_craft(station).unwrap().parts.len(),
        u.docked_links(station).len(),
        u.ut()
    );
}

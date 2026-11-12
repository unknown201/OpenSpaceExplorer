//! Replay actual native-scene inputs through OSE without changing production physics.
use ose_data::{craft::Quat, PartDef, ResourceDef};
use ose_math::DVec3;
use ose_sim::rigid::{to_vessel, PartForce, RigidVessel};
use serde_json::{json, Value};
use std::{env, fs, io::Write, path::Path};

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap()
}
fn vec(v: &Value) -> DVec3 {
    DVec3::new(num(&v[0]), num(&v[1]), num(&v[2]))
}
fn rotate(q: [f64; 4], v: DVec3) -> DVec3 {
    let u = DVec3::new(q[0], q[1], q[2]);
    v + u.cross(v) * (2.0 * q[3]) + u.cross(u.cross(v)) * 2.0
}
fn read(p: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let run = Path::new(
        args.get(1)
            .expect("run folder containing inputs/ and native/"),
    );
    let output = run.join(args.get(2).map(String::as_str).unwrap_or("ose"));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let defs: Vec<PartDef> =
        serde_json::from_value(read(repo.join("assets/own/parts-catalog.json"))).unwrap();
    let res: Vec<ResourceDef> =
        serde_json::from_value(read(repo.join("assets/own/resources.json"))).unwrap();
    let mut summary = Vec::new();
    fs::create_dir_all(&output).unwrap();
    for entry in fs::read_dir(run.join("native")).unwrap() {
        let path = entry.unwrap().path();
        let Some(file) = path.file_name().unwrap().to_str() else {
            continue;
        };
        let Some(name) = file.strip_suffix("-specimen.json") else {
            continue;
        };
        let s = read(&path);
        let native_bodies = s["bodies"].as_array().unwrap();
        let raw = read(run.join("inputs").join(format!("{name}.json")));
        let mut craft = ose_importer::craft::craft_from_value(&raw).unwrap();
        // The OAB regenerates GUIDs when successive test craft share template part GUIDs.
        // Match those parts by definition and unique rest placement in the root frame, rejecting
        // deviations above 5 cm rather than silently guessing among repeated tanks.
        let root = craft.parts.iter().position(|p| p.parent.is_none()).unwrap();
        let rp = craft.parts[root].local_position;
        let rq = craft.parts[root].local_rotation;
        let rq = if rq.x * rq.x + rq.y * rq.y + rq.z * rq.z + rq.w * rq.w < 1e-12 {
            [0., 0., 0., 1.]
        } else {
            [-rq.x, -rq.y, -rq.z, rq.w]
        };
        let mut mapping = Vec::new();
        let mut mapping_error: f64 = 0.0;
        for b in native_bodies {
            let exact = craft
                .parts
                .iter()
                .position(|p| p.guid.eq_ignore_ascii_case(b["guid"].as_str().unwrap()));
            let i = exact.unwrap_or_else(|| {
                craft
                    .parts
                    .iter()
                    .enumerate()
                    .filter(|(i, p)| {
                        !mapping.contains(i) && p.part_id == b["part"].as_str().unwrap()
                    })
                    .min_by(|(_, a), (_, c)| {
                        (rotate(rq, a.local_position - rp) - vec(&b["position"]))
                            .length_squared()
                            .total_cmp(
                                &(rotate(rq, c.local_position - rp) - vec(&b["position"]))
                                    .length_squared(),
                            )
                    })
                    .expect("No unique definition/placement match")
                    .0
            });
            assert!(!mapping.contains(&i), "Duplicate part mapping {name}");
            let error =
                (rotate(rq, craft.parts[i].local_position - rp) - vec(&b["position"])).length();
            assert!(
                error < 0.05,
                "Rest placement mismatch {name} part {i}: {error}m"
            );
            mapping_error = mapping_error.max(error);
            mapping.push(i);
        }
        assert!(
            mapping.len() <= craft.parts.len(),
            "Part count mismatch {name}"
        );
        for (b, &i) in native_bodies.iter().zip(&mapping) {
            let p = &mut craft.parts[i];
            p.local_position = vec(&b["position"]);
            let q = &b["rotation"];
            p.local_rotation = Quat {
                x: num(&q[0]),
                y: num(&q[1]),
                z: num(&q[2]),
                w: num(&q[3]),
            };
        }
        let text =
            fs::read_to_string(run.join("native").join(format!("{name}-inputs.csv"))).unwrap();
        type Input = (usize, PartForce);
        let mut cases: Vec<(String, Vec<Input>)> = Vec::new();
        for line in text.lines().skip(1) {
            let cols: Vec<_> = line.split(',').collect();
            if cases.last().is_none_or(|c| c.0 != cols[0]) {
                cases.push((cols[0].to_string(), Vec::new()));
            }
            let v = |k: usize| {
                DVec3::new(
                    cols[k].parse().unwrap(),
                    cols[k + 1].parse().unwrap(),
                    cols[k + 2].parse().unwrap(),
                )
            };
            let part: usize = cols[2].parse().unwrap();
            cases.last_mut().unwrap().1.push((
                cols[1].parse().unwrap(),
                PartForce {
                    part: mapping[part],
                    force_kn: to_vessel(v(3)),
                    offset_m: to_vessel(v(6)),
                },
            ));
        }
        for mode in ["original", "mass-matched", "runtime-inertia"] {
            let mut used_defs = defs.clone();
            let mut used_craft = craft.clone();
            if mode == "runtime-inertia" {
                for (b, &i) in native_bodies.iter().zip(&mapping) {
                    let p = &mut used_craft.parts[i];
                    let original = used_defs.iter().find(|d| d.id == p.part_id).unwrap();
                    let mut d = p.flight_definition(original);
                    p.flight = None;
                    let q = &b["inertiaRotation"];
                    let q = [num(&q[0]), num(&q[1]), num(&q[2]), num(&q[3])];
                    let axes = [
                        rotate(q, DVec3::new(1., 0., 0.)),
                        rotate(q, DVec3::new(0., 1., 0.)),
                        rotate(q, DVec3::new(0., 0., 1.)),
                    ];
                    let tensor = vec(&b["inertia"]) * (1.0 / num(&b["mass"]));
                    let mul = |v: DVec3| {
                        axes[0] * (tensor.x * axes[0].dot(v))
                            + axes[1] * (tensor.y * axes[1].dot(v))
                            + axes[2] * (tensor.z * axes[2].dot(v))
                    };
                    d.flight_geometry.inertia_per_t = Some([
                        mul(DVec3::new(1., 0., 0.)),
                        mul(DVec3::new(0., 1., 0.)),
                        mul(DVec3::new(0., 0., 1.)),
                    ]);
                    d.flight_geometry.mass_offset_m = vec(&b["com"]);
                    d.mass_offset_m = vec(&b["com"]);
                    d.flight_geometry.inertia_mode = 0;
                    d.id = format!("{}-validation-{i}", d.id);
                    p.part_id = d.id.clone();
                    used_defs.push(d);
                }
            }
            let mut w = fs::File::create(output.join(format!("{name}-{mode}.csv"))).unwrap();
            writeln!(w,"case,tick,t_s,child,parent,intact,force_kn,torque_knm,bend_rad,break_force_kn,break_torque_knm,stiffness,constraint_force_kn,constraint_torque_knm").unwrap();
            for (id, inputs) in &cases {
                let mut rv = RigidVessel::build(&used_craft, &used_defs, &res);
                let native_mass: f64 = native_bodies.iter().map(|b| num(&b["mass"])).sum();
                let ose_mass: f64 = rv.masses_t.iter().sum();
                if mode != "original" {
                    let mut masses = rv.masses_t.clone();
                    for (b, &i) in native_bodies.iter().zip(&mapping) {
                        masses[i] = num(&b["mass"]);
                    }
                    rv.set_part_masses(&masses);
                }
                let mut first_break = None;
                let last_tick = inputs.last().unwrap().0;
                for tick in 0..=last_tick {
                    let forces: Vec<_> =
                        inputs.iter().filter(|r| r.0 == tick).map(|r| r.1).collect();
                    assert!(forces.len() == 2 || forces.len() == 4);
                    let broken = rv.step(0.02, DVec3::ZERO, &forces);
                    for l in &rv.links {
                        #[cfg(feature = "constraint-reactions")]
                        let reactions = (l.constraint_force_kn, l.constraint_torque_knm);
                        #[cfg(not(feature = "constraint-reactions"))]
                        let reactions = (l.force_kn, l.torque_knm);
                        writeln!(
                            w,
                            "{id},{tick},{},{},{},{},{},{},{},{},{},{},{},{}",
                            (tick + 1) as f64 * 0.02,
                            l.child,
                            l.parent,
                            l.intact() as u8,
                            l.force_kn,
                            l.torque_knm,
                            l.bend.length(),
                            l.limits.break_force_kn,
                            l.limits.break_torque_knm,
                            l.limits.stiffness,
                            reactions.0,
                            reactions.1
                        )
                        .unwrap();
                    }
                    if !broken.is_empty() {
                        first_break = Some(
                            json!({"tick":tick,"joints":broken.iter().map(|b|json!({"child":b.child,"parent":b.parent,"force_kn":b.force_kn,"torque_knm":b.torque_knm})).collect::<Vec<_>>()}),
                        );
                        break;
                    }
                }
                #[cfg(feature = "constraint-reactions")]
                let extra = json!(rv
                    .reinforcements
                    .iter()
                    .map(|b| json!({"source":b.source,"target":b.target,"active":b.active}))
                    .collect::<Vec<_>>());
                #[cfg(not(feature = "constraint-reactions"))]
                let extra = Value::Null;
                summary.push(json!({"craft":name,"mode":mode,"case":id,"ose_initial_mass_t":ose_mass,"ose_effective_mass_t":rv.masses_t.iter().sum::<f64>(),"native_mass_t":native_mass,"first_break":first_break,"last_native_tick":last_tick,"links":rv.links.iter().map(|l|json!({"child":l.child,"parent":l.parent,"force_limit":l.limits.break_force_kn,"torque_limit":l.limits.break_torque_knm})).collect::<Vec<_>>(),"native_to_ose":mapping,"mapping_max_error_m":mapping_error,"reinforcements":extra,"braces":rv.braces.iter().map(|b|json!({"source":b.source,"target":b.target,"active":b.active})).collect::<Vec<_>>()}));
            }
        }
        println!("Replayed {name}: {} cases x 3 modes", cases.len());
    }
    fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    println!("Wrote {} case results", summary.len());
}

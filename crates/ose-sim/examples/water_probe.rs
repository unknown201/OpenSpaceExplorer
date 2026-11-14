//! Production water/flight trajectory capture, using the identical native OAB input craft.
use ose_data::{CelestialBody, PartDef, ResourceDef};
use ose_math::DVec3;
use ose_sim::{
    bodies::Body,
    flight::{self, Atmo, EngineEnvironment, FlightSpec, Powered, StageSpec},
    structure::{Airflow, Structure},
};
use serde_json::{json, Value};
use std::io::Write;

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or(".cache/water-parity".into());
    let craft_value: Value =
        serde_json::from_slice(&std::fs::read(format!("{root}/craft.json")).unwrap()).unwrap();
    let craft = ose_importer::craft::craft_from_value(&craft_value).unwrap();
    let native_events: Vec<Value> = serde_json::Deserializer::from_slice(
        &std::fs::read(format!("{root}/native/events.jsonl"))
            .expect("complete native capture required"),
    )
    .into_iter::<Value>()
    .collect::<Result<_, _>>()
    .unwrap();
    assert_eq!(craft.parts.len(), 1);
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let bodies: Vec<CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let d = bodies.iter().find(|b| b.id == "Kerbin").unwrap();
    let body = Body {
        id: d.id.clone(),
        mu: d.mu,
        mass_kg: d.mass_kg,
        radius_m: d.radius_m,
        soi_radius_m: d.soi_radius_m,
        rotation_period_s: d.rotation_period_s,
        initial_rotation_deg: d.initial_rotation_deg,
        is_star: false,
        ocean: d.ocean.clone(),
        site: None,
    };
    let a = d.atmosphere.as_ref().unwrap();
    let atmo = Atmo {
        depth_m: a.depth_m,
        oxygen: a.contains_oxygen,
        pressure_kpa: a.pressure_curve.clone(),
        temperature_k: a.temperature_curve.clone(),
        molar_mass: a.molar_mass_kg_per_mol,
    };
    let terrain = ose_data::terrain::TerrainMap {
        width: 2,
        height: 2,
        scale_m: 0.,
        offset_m: -1000.,
        has_ocean: true,
        body_radius_m: body.radius_m,
        data: vec![0; 4],
        flats: vec![],
        max_m: -1000.,
        detail: None,
        overlay: None,
    };
    let spin = DVec3::new(0., 0., std::f64::consts::TAU / body.rotation_period_s);
    let mut rows = std::io::BufWriter::new(
        std::fs::File::create(format!("{root}/ose-samples.jsonl")).unwrap(),
    );
    let mut events = vec![];
    for (name, height, vy, vx, bank, duration) in [
        ("drop", 3., -5., 0., 0., 45.),
        ("bob", 0., 0., 0., 0., 45.),
        ("bank", 0., 0., 0., 25., 45.),
        ("drag", 0., 0., 5., 0., 20.),
        ("vertical_safe", 1., -16., 0., 0., 6.),
        ("vertical_destroy", 1., -22., 0., 0., 3.),
        ("horizontal_safe", 0., 0., 100., 0., 3.),
        ("horizontal_destroy", 0., 0., 220., 0., 3.),
    ] {
        let start_ut = native_events
            .iter()
            .find(|e| e["action"] == "release" && e["scenario"] == name)
            .and_then(|e| e["ut"].as_f64())
            .expect("matched native release time");
        // The OAB automatically seats one Kerbal in the native pod. Use the normal crew API,
        // including its suit/jetpack mass, rather than injecting a measured total mass.
        let mut u = ose_sim::Universe::with_bodies(&bodies);
        let home = u.body_entity("Kerbin").unwrap();
        let body_orbit = u.world.get::<ose_sim::bodies::OnRails>(home).unwrap().0;
        let e = u.spawn_vessel_in_circular_orbit("water fixture", home, 100000.);
        u.set_structure(e, Structure::build(&craft, &defs, &resources));
        u.set_crew(e, vec![(0, "water-test-kerbal".into())]);
        let mut s = u.world.entity_mut(e).take::<Structure>().unwrap();
        let mass = s.mass_properties();
        let mut p = Powered::new(
            FlightSpec {
                stages: vec![StageSpec::default()],
                drag_area_m2: 0.,
                drag_cd: 0.,
            },
            flight::surface_state_at(&body, 0., -60., height, start_ut).0,
            DVec3::ZERO,
            false,
        );
        let bank: f64 = bank;
        let (up, east, _north) = flight::surface_frame(p.r);
        let right = -east; // the native surface teleporter's root +X
        p.facing = right * (-bank.to_radians().sin()) + up * bank.to_radians().cos();
        p.attitude = flight::Attitude::Free;
        p.controlled = false;
        s.sync(&mut p);
        let (x, y, z) = flight::vessel_basis(&p);
        p.r = p.r + x * mass.centre_m.x + y * mass.centre_m.y + z * mass.centre_m.z;
        p.v = spin.cross(p.r) + right * vx + up * vy;
        p.angvel = spin;
        let definition = defs
            .iter()
            .find(|d| d.id == craft.parts[0].part_id)
            .unwrap();
        events.push(json!({"scenario":name,"action":"release","ut":start_ut,"latitude_deg":0.,"longitude_deg":-60.,"crew_count":1,"input":{"height":height,"vy":vy,"vx":vx,"bank":bank,"duration":duration},"mass_t":mass.mass_t,"com":mass.centre_m,"volume_m3":s.displacement_m3[0],"buoyancy":s.buoyancy[0],"buoyancy_center_m":definition.buoyancy_center_m}));
        for tick in 0..=(duration / 0.02) as usize {
            let t = tick as f64 * 0.02;
            let up = p.r.normalized();
            let v = p.v - spin.cross(p.r);
            let (x, y, z) = flight::vessel_basis(&p);
            let origin = p.r - x * mass.centre_m.x - y * mass.centre_m.y - z * mass.centre_m.z;
            writeln!(rows,"{}",json!({"scenario":name,"t":t,"alive":!p.crashed,"com_alt_m":p.r.length()-body.radius_m,"origin_alt_m":origin.length()-body.radius_m,"vertical_mps":v.dot(up),"horizontal_mps":(v-up*v.dot(up)).length(),"bank_deg":p.facing.dot(up).clamp(-1.,1.).acos().to_degrees(),"splashed":p.splashed})).unwrap();
            if p.crashed || tick == (duration / 0.02) as usize {
                continue;
            }
            let env = EngineEnvironment::at(&p, &body, Some(&atmo));
            let mut engine = s.propulsion(&p, env, 0.02);
            engine.water_reference = Some(ose_sim::water::ReferenceFrame::at(
                &body,
                body_orbit.position_at(start_ut + t),
                start_ut + t,
            ));
            let flow = Airflow::at(&p, &body, Some(&atmo));
            let aero = s.aerodynamics_local(p.stage, flow);
            p.lift_kn = aero.lift_kn;
            p.aero_torque_knm = aero.torque_knm;
            p.aero_drag_kn = Some(aero.drag_kn);
            s.configure_actuators(&p, env, &mut engine);
            s.configure_surface(&mut p, &body, Some(&terrain), t, 0.02, &mut engine);
            let before = p.clone();
            let mut out = flight::integrate_with_propulsion(
                &mut p,
                &body,
                Some(&atmo),
                Some(&terrain),
                t,
                0.02,
                Some(engine),
            );
            s.resolve_surface(&before, &mut p, &body, Some(&terrain), t, 0.02, &mut out);
            if out.crashed {
                assert!(s.contact_damage(&out.contact_damage, &p).is_none());
                p.crashed = true;
            }
        }
    }
    std::fs::write(
        format!("{root}/ose-events.json"),
        serde_json::to_vec_pretty(&events).unwrap(),
    )
    .unwrap();
    println!("Captured eight production water trajectories in {root}");
}

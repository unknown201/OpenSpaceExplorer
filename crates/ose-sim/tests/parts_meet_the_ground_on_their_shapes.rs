//! Each part meets the ground on its own shape, as KSP2's colliders hold it (Josh: "when a rocket or plane lands
//! its individual parts collide and react properly"). KSP2's Stock04_Jet (`craft-jet.json`, the parity pilot's
//! strike runs, reference\ksp2-strike-*) collides on each procedural wing's and tail's baked collider meshes
//! (`col_wing_*_staticCollision`, `col_stabilizer_staticCollision`): their furthest points are 4.489 m from the
//! wings' roots (a 3.735 m span) and 1.599 m from the tails' (1.331 m). Our wings and tails were 10 cm boxes at their roots (the catalog's start-shape
//! bounds), so a wingtip went through the runway.
use ose_sim::structure::Structure;

fn data() -> (Vec<ose_data::PartDef>, Vec<ose_data::ResourceDef>) {
    (
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
    )
}

#[test]
fn a_planes_wings_and_tails_meet_the_ground_on_their_shaped_span() {
    let (defs, res) = data();
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-jet.json"
    ))
    .unwrap();
    let c = ose_importer::craft::craft_from_value(&v).unwrap();
    let st = Structure::build(&c, &defs, &res);
    let mut seen = 0;
    for (i, part) in c.parts.iter().enumerate() {
        // KSP2's collider meshes, furthest point from the part's root (collider-points.csv)
        let ksp2_m = match part.part_id.as_str() {
            "wing_1v_procedural_light" => 4.489,
            "stabilizer_1v_procedural_light" => 1.599,
            _ => continue,
        };
        let root = st.rigid.rest_position(i);
        let reach = st
            .ground_points(i)
            .iter()
            .map(|q| (*q - root).length())
            .fold(0.0, f64::max);
        println!("{} {i}: reaches {reach:.3} m (KSP2 {ksp2_m})", part.part_id);
        assert!(
            (reach - ksp2_m).abs() < 0.02,
            "{} {i}: reaches {reach:.3} m, KSP2's collider {ksp2_m} m",
            part.part_id
        );
        seen += 1;
    }
    assert_eq!(seen, 4, "two wings and two tails");
}

#[test]
fn the_jet_scrapes_its_engine_on_the_runway_at_ksp2s_rotation_angle() {
    // KSP2's Stock04_Jet, rotated hard on its main gear as it slowed (strike tail run 6), first touched the
    // runway with its Wheesley's collider at 18.0 degrees nose up, the mains loaded: a scrape at 33.8 m/s along
    // the ground that broke the engine (tolerance 10). Ours: the angle at which the engine's hull comes down to
    // the main gear's lowest point, with the suspension fully compressed.
    let (defs, res) = data();
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/ksp2-parity-pilot/craft-jet.json"
    ))
    .unwrap();
    let c = ose_importer::craft::craft_from_value(&v).unwrap();
    let st = Structure::build(&c, &defs, &res);
    let parts = &c.parts;
    let find = |id: &'static str| (0..parts.len()).filter(move |&i| parts[i].part_id == id);
    let mains: Vec<usize> = find("landinggear_1v").collect();
    let engine = find("engine_1v_turbofan_methane_wheesley").next().unwrap();
    let travel = st.gear_defs[mains[0]].as_ref().unwrap().suspension_m;
    // the craft's nose is -z and its gear hangs down -y; nose up by `deg`, the tail swings down
    let lowest = |i: usize, deg: f64| {
        let t = f64::to_radians(deg);
        let down = ose_math::DVec3::new(0.0, -t.cos(), t.sin());
        st.ground_points(i)
            .iter()
            .map(|q| q.dot(down))
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let scrapes = |deg: f64| {
        let gear = mains
            .iter()
            .map(|&i| lowest(i, deg))
            .fold(f64::NEG_INFINITY, f64::max);
        lowest(engine, deg) >= gear - travel
    };
    let angle = (0..400)
        .map(|k| k as f64 * 0.1)
        .find(|&d| scrapes(d))
        .expect("the engine meets the runway");
    println!("engine meets the runway at {angle:.1} degrees (KSP2 18.0)");
    assert!((angle - 18.0).abs() < 1.0, "{angle:.1} degrees");
}

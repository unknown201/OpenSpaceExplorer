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

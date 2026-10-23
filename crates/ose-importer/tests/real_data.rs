//! Runs the importer over the real extracted data when it is present (`assets/ksp2/data`, gitignored).
//! Skips silently on machines without the KSP2 install so CI stays green.

use std::path::PathBuf;

fn data_dir() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ksp2/data");
    p.join("GalaxyDefinition_Default.json")
        .exists()
        .then_some(p)
}

#[test]
fn imports_the_whole_kerbol_system_and_all_parts() {
    let Some(dir) = data_dir() else {
        eprintln!("skipping: no extracted KSP2 data");
        return;
    };
    let mut cov = ose_importer::Coverage::default();
    let game = ose_importer::import_dir(&dir, &mut cov).expect("import");

    assert_eq!(game.bodies.len(), 22, "bodies");
    let kerbin = game
        .bodies
        .iter()
        .find(|b| b.id == "Kerbin")
        .expect("Kerbin");
    assert_eq!(kerbin.parent.as_deref(), Some("Kerbol"));
    assert!(
        (kerbin.mu - 3.5316e12).abs() / 3.5316e12 < 1e-3,
        "Kerbin mu {}",
        kerbin.mu
    );
    let soi = kerbin.soi_radius_m.expect("Kerbin SOI");
    assert!(
        (soi - 84_159_286.0).abs() / 84_159_286.0 < 0.01,
        "Kerbin SOI {soi}"
    );
    assert!(
        kerbin
            .atmosphere
            .as_ref()
            .unwrap()
            .pressure_curve
            .keys
            .len()
            > 2
    );
    let kerbol = game.bodies.iter().find(|b| b.id == "Kerbol").unwrap();
    assert!(kerbol.is_star && kerbol.parent.is_none() && kerbol.soi_radius_m.is_none());
    for b in &game.bodies {
        if let Some(p) = &b.parent {
            assert!(
                game.bodies.iter().any(|x| &x.id == p),
                "{} parent {} missing",
                b.id,
                p
            );
        }
    }

    assert!(game.resources.len() >= 14, "resources");
    assert!(game.resources.iter().any(|r| r.id == "Methalox"));

    assert_eq!(game.parts.len(), 400, "parts");
    let terrier = game
        .parts
        .iter()
        .find(|p| p.id == "engine_1v_methalox_terrier")
        .expect("terrier");
    assert_eq!(terrier.engines.len(), 1);
    assert_eq!(terrier.engines[0].max_thrust_kn, 60.0);
    assert_eq!(terrier.engines[0].isp_curve.evaluate(0.0), 335.0);
    assert_eq!(terrier.engines[0].gimbal_range_deg, Some(4.0));
    assert_eq!(terrier.mass_t, 0.5);
    assert!(!terrier.drag_cubes.is_empty());
    let engines: usize = game.parts.iter().map(|p| p.engines.len()).sum();
    assert!(engines >= 36, "engine modes {engines}");

    // Stock craft: Kerbal K2 is a 48-part tree whose dry+wet mass matches the file's own metadata (111.69 t).
    let k2 = dir.join("stockassemblies/Stock Kerbal-K2.json");
    let text = std::fs::read_to_string(&k2).expect("K2 file");
    let v: serde_json::Value = serde_json::from_str(&text).expect("K2 json");
    let craft = ose_importer::craft::craft_from_value(&v).expect("craft");
    assert_eq!(craft.parts.len(), 48);
    assert!(craft.is_tree());
    assert_eq!(craft.parts[0].parent, None);
    assert!(craft.stages.len() >= 5, "stages {:?}", craft.stages.len());
    let m = craft.mass(&game.parts, &game.resources);
    assert!(m.unknown_parts.is_empty(), "{:?}", m.unknown_parts);
    let meta_mass = v["Metadata"]["Mass"].as_f64().unwrap();
    assert!(
        (m.wet_t() - meta_mass).abs() / meta_mass < 0.02,
        "wet {} vs metadata {meta_mass}",
        m.wet_t()
    );
    assert!(cov.warnings.is_empty(), "{:?}", cov.warnings);
}

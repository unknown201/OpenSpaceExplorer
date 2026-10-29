//! Imports data from the extracted KSP2 JSON (`tools/extract_bundles.py`) into our own schema.
//!
//! ```text
//! cargo run -p ose-importer -- --data assets/ksp2/data --out assets/ksp2/converted
//! ```
//! Writes `game_data.json` and `coverage.txt` (unmapped source fields) into `--out`.

use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "ose-importer")]
struct Args {
    /// Authored thrust transforms exported by tools/export_physics_geometry.py.
    #[arg(long, default_value = "assets/own/physics-geometry.json")]
    physics_geometry: PathBuf,
    #[arg(long, default_value = "assets/own/rcs-geometry.json")]
    rcs_geometry: PathBuf,
    #[arg(long, default_value = "assets/own/lifting-geometry.json")]
    lifting_geometry: PathBuf,
    /// Extracted data directory (output of tools/extract_bundles.py).
    #[arg(long, default_value = "assets/ksp2/data")]
    data: PathBuf,
    /// Where to write the converted data.
    #[arg(long, default_value = "assets/ksp2/converted")]
    out: PathBuf,
    /// Where to write the slim, committed server assets (bodies, parts with their first drag cube and no raw
    /// modules, resources, stock craft). Skipped when empty.
    #[arg(long, default_value = "assets/own")]
    own_out: PathBuf,
    /// The client's exported localisation table (`assets/web/loc/en.json`): parts without a title of their
    /// own take `Parts/Title/<id>` from it. Skipped when missing.
    #[arg(long, default_value = "assets/web/loc/en.json")]
    loc: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut cov = ose_importer::Coverage::default();
    let mut game = ose_importer::import_dir(&args.data, &mut cov)
        .with_context(|| format!("importing {}", args.data.display()))?;
    if args.physics_geometry.exists() {
        let geometry: std::collections::BTreeMap<String, Vec<Vec<ose_data::ThrustTransform>>> =
            serde_json::from_str(&std::fs::read_to_string(&args.physics_geometry)?)?;
        ose_importer::apply_engine_geometry(&mut game.parts, &geometry)
            .map_err(anyhow::Error::msg)?;
    }
    if args.rcs_geometry.exists() {
        let geometry: std::collections::BTreeMap<String, Vec<ose_data::ThrustTransform>> =
            serde_json::from_str(&std::fs::read_to_string(&args.rcs_geometry)?)?;
        ose_importer::apply_rcs_geometry(&mut game.parts, &geometry).map_err(anyhow::Error::msg)?;
    }
    if args.lifting_geometry.exists() {
        let geometry: std::collections::BTreeMap<String, ose_importer::LiftingGeometry> =
            serde_json::from_str(&std::fs::read_to_string(&args.lifting_geometry)?)?;
        ose_importer::apply_lifting_geometry(&mut game.parts, &geometry)
            .map_err(anyhow::Error::msg)?;
    }
    if let Ok(text) = std::fs::read_to_string(&args.loc) {
        let loc: std::collections::HashMap<String, String> = serde_json::from_str(&text)?;
        let mut titled = 0;
        for p in &mut game.parts {
            if p.title_key.is_empty() || p.title_key == p.id {
                if let Some(t) = loc.get(&format!("Parts/Title/{}", p.id)) {
                    // the exported table lost its curly quotes (U+FFFD)
                    p.title_key = t.replace('\u{FFFD}', "\"");
                    titled += 1;
                }
            }
        }
        println!("titles from {}: {titled}", args.loc.display());
    }
    let geometry_path = args.own_out.join("flight-geometry.json");
    if geometry_path.exists() {
        let geometry: std::collections::HashMap<String, ose_data::flight_geometry::FlightGeometry> =
            serde_json::from_str(&std::fs::read_to_string(geometry_path)?)?;
        for part in &mut game.parts {
            if let Some(g) = geometry.get(&part.id) {
                part.flight_geometry = g.clone();
            }
        }
    }
    std::fs::create_dir_all(&args.out)?;
    std::fs::write(
        args.out.join("game_data.json"),
        serde_json::to_string_pretty(&game)?,
    )?;
    std::fs::write(args.out.join("coverage.txt"), cov.report())?;
    // stock craft, as our Craft schema
    let stock_dir = args.data.join("stockassemblies");
    let mut crafts = Vec::new();
    if stock_dir.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&stock_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f)?;
            let v: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => json5::from_str(&text)?,
            };
            match ose_importer::craft::craft_from_value(&v) {
                Ok(mut c) => {
                    ose_importer::craft::apply_stock_flight_geometry(&mut c, text.as_bytes())?;
                    crafts.push(c)
                }
                Err(e) => println!("warning: stock craft {}: {e}", f.display()),
            }
        }
        // the tutorial lessons' rockets (their saves hold vessels in the assembly schema), after the stock
        // crafts so those keep their ids
        let tutorials = args.data.join("tutorials");
        if tutorials.is_dir() {
            let mut saves: Vec<_> = std::fs::read_dir(&tutorials)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("Tutorial") && n.ends_with("_Save.json"))
                })
                .collect();
            saves.sort();
            for f in saves {
                let stem = f.file_stem().and_then(|n| n.to_str()).unwrap_or("");
                let lesson = stem
                    .trim_start_matches("Tutorial")
                    .trim_end_matches("_Save")
                    .to_string();
                let text = std::fs::read_to_string(&f)?;
                let v: serde_json::Value =
                    match serde_json::from_str(&text).or_else(|_| json5::from_str(&text)) {
                        Ok(v) => v,
                        Err(e) => {
                            println!("warning: {}: {e}", f.display());
                            continue;
                        }
                    };
                let vessels: Vec<&serde_json::Value> = v["Vessels"]
                    .as_array()
                    .map(|a| a.iter().filter(|x| x["IsDebris"] != true).collect())
                    .unwrap_or_default();
                for (k, vessel) in vessels.iter().enumerate() {
                    let name = if vessels.len() == 1 {
                        format!("Lesson {lesson} rocket")
                    } else {
                        format!("Lesson {lesson} rocket {}", k + 1)
                    };
                    let wrapped = serde_json::json!({
                        "Metadata": { "Name": name },
                        "Assemblies": [ { "Assembly": vessel } ],
                    });
                    match ose_importer::craft::craft_from_value(&wrapped) {
                        Ok(c) => crafts.push(c),
                        Err(e) => println!("warning: {}: {e}", f.display()),
                    }
                }
            }
        }
        std::fs::write(
            args.out.join("stock-craft.json"),
            serde_json::to_string(&crafts)?,
        )?;
    }
    if !args.own_out.as_os_str().is_empty() {
        std::fs::create_dir_all(&args.own_out)?;
        let bodies: Vec<_> = game
            .bodies
            .iter()
            .filter(|b| !b.id.starts_with("Asteroid"))
            .collect();
        std::fs::write(
            args.own_out.join("kerbol-system.json"),
            serde_json::to_string_pretty(&bodies)?,
        )?;
        let slim: Vec<ose_data::PartDef> = game
            .parts
            .iter()
            .map(|p| ose_data::PartDef {
                // the first cube is the part's default shape (the sim's drag); the rest are deployed states
                drag_cubes: p.drag_cubes.iter().take(1).cloned().collect(),
                raw_modules: Default::default(),
                ..p.clone()
            })
            .collect();
        std::fs::write(
            args.own_out.join("parts-catalog.json"),
            serde_json::to_string(&slim)?,
        )?;
        let (science, regions) = ose_importer::science::import_science(&args.data.join("science"))?;
        let tech = ose_importer::science::import_tech_tree(&args.data.join("technodedata"))?;
        std::fs::create_dir_all(args.own_out.join("science"))?;
        std::fs::write(
            args.own_out.join("science/index.json"),
            serde_json::to_string_pretty(&science)?,
        )?;
        std::fs::write(args.own_out.join("science/regions.bin"), &regions)?;
        let missions = ose_importer::missions::import_missions(&args.data)?;
        println!(
            "missions: {} ({} playable)",
            missions.len(),
            missions.iter().filter(|m| m.playable()).count()
        );
        std::fs::write(
            args.own_out.join("missions.json"),
            serde_json::to_string_pretty(&missions)?,
        )?;
        std::fs::write(
            args.own_out.join("tech-tree.json"),
            serde_json::to_string_pretty(&tech)?,
        )?;
        println!(
            "science: {} experiments, {} bodies, {} KB of region maps; tech tree: {} nodes",
            science.experiments.len(),
            science.bodies.len(),
            regions.len() / 1024,
            tech.len()
        );
        let kerbals = ose_importer::import_kerbals(&args.data.join("kerbal_variety_text"))?;
        println!("kerbals: {}", kerbals.len());
        std::fs::write(
            args.own_out.join("kerbals.json"),
            serde_json::to_string_pretty(&kerbals)?,
        )?;
        std::fs::write(
            args.own_out.join("resources.json"),
            serde_json::to_string_pretty(&game.resources)?,
        )?;
        std::fs::write(
            args.own_out.join("stock-craft.json"),
            serde_json::to_string(&crafts)?,
        )?;
    }
    println!(
        "bodies={} resources={} parts={} engines={} stock_craft={} -> {}",
        game.bodies.len(),
        game.resources.len(),
        game.parts.len(),
        game.parts.iter().map(|p| p.engines.len()).sum::<usize>(),
        crafts.len(),
        args.out.display()
    );
    for w in &cov.warnings {
        println!("warning: {w}");
    }
    Ok(())
}

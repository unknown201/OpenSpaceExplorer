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
                Ok(c) => crafts.push(c),
                Err(e) => println!("warning: stock craft {}: {e}", f.display()),
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

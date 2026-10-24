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
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut cov = ose_importer::Coverage::default();
    let game = ose_importer::import_dir(&args.data, &mut cov)
        .with_context(|| format!("importing {}", args.data.display()))?;
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

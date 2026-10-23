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
    println!(
        "bodies={} resources={} parts={} engines={} -> {}",
        game.bodies.len(),
        game.resources.len(),
        game.parts.len(),
        game.parts.iter().map(|p| p.engines.len()).sum::<usize>(),
        args.out.display()
    );
    for w in &cov.warnings {
        println!("warning: {w}");
    }
    Ok(())
}

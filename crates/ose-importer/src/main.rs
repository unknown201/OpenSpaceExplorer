//! Imports data and placeholder assets from the local KSP2 install into `assets/ksp2/`.
//!
//! Phase 0: reads the Addressables catalog and reports what it finds. Extraction of bundle contents is done by
//! `tools/extract_bundles.py` (UnityPy), which this binary will drive in Phase 1.

use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Args {
    /// Root of the KSP2 install.
    #[arg(
        long,
        default_value = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"
    )]
    install: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let catalog = args
        .install
        .join("KSP2_x64_Data/StreamingAssets/aa/catalog.json");
    let text = std::fs::read_to_string(&catalog)
        .with_context(|| format!("reading {}", catalog.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let ids = v["m_InternalIds"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("catalog: {} internal ids", ids);
    Ok(())
}

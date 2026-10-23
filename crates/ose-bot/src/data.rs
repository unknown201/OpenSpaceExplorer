//! Game data the bot needs: celestial bodies and stock craft.
//!
//! The converted KSP2 data (`assets/ksp2/converted/game_data.json`) is gitignored, so on machines without
//! it the bot falls back to a built-in three-body Kerbol system (Kerbol, Kerbin, Mun) with the original's
//! constants, which is all the Mun run needs.

use anyhow::{Context, Result};
use ose_data::{CelestialBody, GameData, Orbit as OrbitDef, BIG_G, G0};
use std::path::{Path, PathBuf};

/// Where the body data came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodySource {
    /// `game_data.json` written by `ose-importer`.
    Converted(PathBuf),
    /// The built-in Kerbol/Kerbin/Mun fixture.
    Builtin,
}

/// Repository root (the Cargo workspace), found from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    // Not canonicalised: on Windows that would add a `\?\` prefix to every path in the report.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Default path of the converted game data.
pub fn default_game_data_path() -> PathBuf {
    repo_root().join("assets/ksp2/converted/game_data.json")
}

/// Default directory of the extracted stock assemblies.
pub fn default_stock_dir() -> PathBuf {
    repo_root().join("assets/ksp2/data/stockassemblies")
}

/// Load the bodies from `path` when it exists, otherwise the built-in fixture.
pub fn load_bodies(path: &Path) -> Result<(Vec<CelestialBody>, BodySource)> {
    if path.exists() {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let game: GameData =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        Ok((game.bodies, BodySource::Converted(path.to_path_buf())))
    } else {
        Ok((builtin_kerbol(), BodySource::Builtin))
    }
}

fn body(
    id: &str,
    parent: Option<&str>,
    radius: f64,
    g: f64,
    soi: Option<f64>,
    rotation_period_s: f64,
    orbit: Option<(f64, f64)>,
) -> CelestialBody {
    let mu = g * G0 * radius * radius;
    CelestialBody {
        id: id.into(),
        display_name: id.into(),
        parent: parent.map(|s| s.into()),
        is_star: parent.is_none(),
        is_home_world: id == "Kerbin",
        radius_m: radius,
        gravity_asl: g,
        mu,
        mass_kg: mu / BIG_G,
        soi_radius_m: soi,
        rotation_period_s,
        initial_rotation_deg: 0.0,
        is_tidally_locked: false,
        has_solid_surface: true,
        ocean: None,
        orbit: orbit.map(|(a, m0)| OrbitDef {
            semi_major_axis_m: a,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            longitude_of_ascending_node_deg: 0.0,
            argument_of_periapsis_deg: 0.0,
            mean_anomaly_at_epoch_rad: m0,
            epoch_s: 0.0,
        }),
        atmosphere: None,
        star_luminosity: None,
    }
}

/// Kerbol, Kerbin and the Mun with the values `ose-importer` derives from the install.
// Kerbin's mean anomaly at epoch really is authored as 3.14 in `GalaxyDefinition_Default`, not pi.
#[allow(clippy::approx_constant)]
pub fn builtin_kerbol() -> Vec<CelestialBody> {
    vec![
        body(
            "Kerbol",
            None,
            261_600_000.0,
            1.746_846_561,
            None,
            432_000.0,
            None,
        ),
        body(
            "Kerbin",
            Some("Kerbol"),
            600_000.0,
            1.000_341_605,
            Some(84_159_286.48),
            21_549.425,
            Some((13_599_840_256.0, 3.14)),
        ),
        body(
            "Mun",
            Some("Kerbin"),
            200_000.0,
            0.166_056_700,
            Some(2_429_559.12),
            138_984.38,
            Some((12_000_000.0, 1.7)),
        ),
    ]
}

/// What the bot reads out of a KSP2 workspace/stock assembly file.
#[derive(Clone, Debug, PartialEq)]
pub struct StockCraft {
    pub file: PathBuf,
    pub name: String,
    pub mass_t: f64,
    pub stages: u64,
    /// Part definition ids in the order the file lists them.
    pub part_names: Vec<String>,
}

/// Parse one stock assembly JSON (`{Metadata: {...}, Assemblies: [{Assembly: {parts: [...]}}]}`).
pub fn parse_stock_craft(path: &Path) -> Result<StockCraft> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let meta = &v["Metadata"];
    let name = meta["VehicleName"]
        .as_str()
        .or_else(|| meta["Name"].as_str())
        .context("craft has no name")?
        .to_string();
    let mut part_names = Vec::new();
    for asm in v["Assemblies"].as_array().into_iter().flatten() {
        for p in asm["Assembly"]["parts"].as_array().into_iter().flatten() {
            if let Some(n) = p["partName"].as_str() {
                part_names.push(n.to_string());
            }
        }
    }
    anyhow::ensure!(!part_names.is_empty(), "craft lists no parts");
    Ok(StockCraft {
        file: path.to_path_buf(),
        name,
        mass_t: meta["Mass"].as_f64().unwrap_or(0.0),
        stages: meta["Stages"].as_u64().unwrap_or(0),
        part_names,
    })
}

/// Pick the first craft in `dir` whose file stem matches one of `preferred`, else the first `.json` file.
/// `Ok(None)` when the directory is absent or empty.
pub fn find_stock_craft(dir: &Path, preferred: &[&str]) -> Result<Option<StockCraft>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let pick = preferred
        .iter()
        .find_map(|want| {
            files.iter().find(|f| {
                f.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(want))
            })
        })
        .or(files.first());
    match pick {
        Some(p) => parse_stock_craft(p).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_fixture_matches_the_originals_constants() {
        let bodies = builtin_kerbol();
        let kerbin = bodies.iter().find(|b| b.id == "Kerbin").unwrap();
        assert!(
            (kerbin.mu - 3.5316e12).abs() / 3.5316e12 < 1e-4,
            "{}",
            kerbin.mu
        );
        let mun = bodies.iter().find(|b| b.id == "Mun").unwrap();
        assert!((mun.mu - 6.5138e10).abs() / 6.5138e10 < 1e-4, "{}", mun.mu);
        assert_eq!(mun.parent.as_deref(), Some("Kerbin"));
    }

    #[test]
    fn missing_game_data_falls_back_to_builtin() {
        let (bodies, src) = load_bodies(Path::new("Z:/definitely/not/here.json")).unwrap();
        assert_eq!(src, BodySource::Builtin);
        assert_eq!(bodies.len(), 3);
    }

    #[test]
    fn parses_a_stock_assembly_shape() {
        let dir = std::env::temp_dir().join(format!("ose-bot-craft-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Stock Kerbal-K2.json");
        std::fs::write(
            &f,
            r#"{"Metadata":{"Name":"Stock Kerbal-K2","VehicleName":"Kerbal K-2","Mass":30.5,"Stages":4},
                "Assemblies":[{"Assembly":{"parts":[{"partName":"pod_1v_conical_crew"},{"partName":"engine_1v_methalox_terrier"}]}}]}"#,
        )
        .unwrap();
        let c = find_stock_craft(&dir, &["Stock Kerbal-K2"])
            .unwrap()
            .unwrap();
        assert_eq!(c.name, "Kerbal K-2");
        assert_eq!(c.part_names.len(), 2);
        assert_eq!(c.stages, 4);
        std::fs::remove_dir_all(&dir).ok();
    }
}

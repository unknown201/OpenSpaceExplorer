//! Science, regions and the tech tree from the extracted data (`tools/extract_science.py` writes
//! `assets/ksp2/data/science/<label>/`, `tools/extract_bundles.py` writes `technodedata/`).

use crate::{b, f, read_json, s, vec3};
use ose_data::science::{
    BodyScience, Discoverable, ExperimentDef, ExperimentKind, RegionDef, RegionMapRef,
    ScienceCatalog, ScienceSituation, SituationData, TechNode, ValidLocation,
};
use serde_json::Value;
use std::path::Path;

/// Downsampled region map size: one texel is about 7 km on Kerbin.
pub const MAP_W: usize = 512;
pub const MAP_H: usize = 256;

fn files(dir: &Path) -> Vec<std::path::PathBuf> {
    crate::json_files(dir).unwrap_or_default()
}

fn data(v: &Value) -> &Value {
    if v["data"].is_object() {
        &v["data"]
    } else {
        v
    }
}

pub fn experiment(v: &Value) -> ExperimentDef {
    let d = data(v);
    let kind = match s(d, "ExperimentType").as_str() {
        "SampleType" => ExperimentKind::Sample,
        "Both" => ExperimentKind::Both,
        _ => ExperimentKind::Data,
    };
    let valid = d["ValidLocations"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| ValidLocation {
                    body: s(l, "BodyName"),
                    situation: ScienceSituation::from_name(&s(l, "ScienceSituation")),
                    region: s(l, "ScienceRegion"),
                    requires_region: b(l, "RequiresRegion"),
                })
                .collect()
        })
        .unwrap_or_default();
    ExperimentDef {
        id: s(d, "ExperimentID"),
        display_name_key: s(d, "DisplayName"),
        kind,
        valid,
        data_value: f(d, "DataValue"),
        sample_value: f(d, "SampleValue"),
        transmission_size: f(d, "TransmissionSize"),
        requires_eva: b(d, "RequiresEVA"),
    }
}

fn body_regions(v: &Value) -> BodyScience {
    let sd = &v["SituationData"];
    BodyScience {
        body: s(v, "BodyName"),
        situations: SituationData {
            high_orbit_max_m: f(sd, "HighOrbitMaxAltitude"),
            // the original spells these "Altutude"
            low_orbit_max_m: f(sd, "LowOrbitMaxAltutude").max(f(sd, "LowOrbitMaxAltitude")),
            atmosphere_max_m: f(sd, "AtmosphereMaxAltutude").max(f(sd, "AtmosphereMaxAltitude")),
            body_scalar: f(sd, "CelestialBodyScalar"),
            high_orbit_scalar: f(sd, "HighOrbitScalar"),
            low_orbit_scalar: f(sd, "LowOrbitScalar"),
            atmosphere_scalar: f(sd, "AtmosphereScalar"),
            splashed_scalar: f(sd, "SplashedScalar"),
            landed_scalar: f(sd, "LandedScalar"),
        },
        regions: v["Regions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| RegionDef {
                        id: s(r, "Id"),
                        map_id: r["MapId"].as_u64().unwrap_or(255) as u8,
                        atmosphere_scalar: f(r, "AtmosphereScalar"),
                        splashed_scalar: f(r, "SplashedScalar"),
                        landed_scalar: f(r, "LandedScalar"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        discoverables: Vec::new(),
        map: None,
    }
}

/// Nearest-neighbour downsample of a baked `Width x Height` map (row-major, row 0 at the south pole).
pub fn downsample(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; MAP_W * MAP_H];
    for y in 0..MAP_H {
        let sy = ((y as f64 + 0.5) / MAP_H as f64 * h as f64) as usize;
        for x in 0..MAP_W {
            let sx = ((x as f64 + 0.5) / MAP_W as f64 * w as f64) as usize;
            out[y * MAP_W + x] = src[sy.min(h - 1) * w + sx.min(w - 1)];
        }
    }
    out
}

/// The science catalog and the concatenated downsampled region maps.
pub fn import_science(science_dir: &Path) -> anyhow::Result<(ScienceCatalog, Vec<u8>)> {
    let mut cat = ScienceCatalog::default();
    for p in files(&science_dir.join("scienceExperiment")) {
        cat.experiments.push(experiment(&read_json(&p)?));
    }
    let regions_dir = science_dir.join("science_region");
    for p in files(&regions_dir) {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with("_science_regions.json") {
            continue;
        }
        cat.bodies.push(body_regions(&read_json(&p)?));
    }
    for p in files(&science_dir.join("science_region_discoverables")) {
        let v = read_json(&p)?;
        let body = s(&v, "BodyName");
        let Some(b) = cat.bodies.iter_mut().find(|b| b.body == body) else {
            continue;
        };
        if let Some(a) = v["Discoverables"].as_array() {
            for d in a {
                b.discoverables.push(Discoverable {
                    region: s(d, "ScienceRegionId"),
                    position: vec3(&d["Position"]),
                    radius_m: f(d, "Radius"),
                });
            }
        }
    }
    let mut blob = Vec::new();
    for p in files(&science_dir.join("science_region_map")) {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with("_baked_map.json") {
            continue;
        }
        let v = read_json(&p)?;
        let body = s(&v, "BodyName");
        let (w, h) = (
            v["Width"].as_u64().unwrap_or(0) as usize,
            v["Height"].as_u64().unwrap_or(0) as usize,
        );
        let Some(src) = v["MapData"].as_array() else {
            continue;
        };
        if w == 0 || h == 0 || src.len() < w * h {
            continue;
        }
        let src: Vec<u8> = src.iter().map(|x| x.as_u64().unwrap_or(0) as u8).collect();
        let Some(b) = cat.bodies.iter_mut().find(|b| b.body == body) else {
            continue;
        };
        b.map = Some(RegionMapRef {
            offset: blob.len(),
            width: MAP_W,
            height: MAP_H,
        });
        blob.extend(downsample(&src, w, h));
    }
    cat.experiments.sort_by(|a, b| a.id.cmp(&b.id));
    cat.bodies.sort_by(|a, b| a.body.cmp(&b.body));
    Ok((cat, blob))
}

pub fn tech_node(v: &Value) -> TechNode {
    let d = data(v);
    let strings = |k: &str| {
        d[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    TechNode {
        id: s(d, "ID"),
        name_key: s(d, "NameLocKey"),
        description_key: s(d, "DescriptionLocKey"),
        icon: s(d, "IconID"),
        required_science: d["RequiredSciencePoints"].as_u64().unwrap_or(0) as u32,
        parts: strings("UnlockedPartsIDs"),
        requires: strings("RequiredTechNodeIDs"),
        hidden_by: s(d, "HiddenByNodeID"),
        tier: d["TierToUnlock"].as_u64().unwrap_or(0) as u32,
        position: (
            f(&d["TechTreePosition"], "x"),
            f(&d["TechTreePosition"], "y"),
        ),
    }
}

pub fn import_tech_tree(dir: &Path) -> anyhow::Result<Vec<TechNode>> {
    let mut nodes = Vec::new();
    for p in files(dir) {
        nodes.push(tech_node(&read_json(&p)?));
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(nodes)
}

/// Experiments of a part's `Data_ScienceExperiment` entries.
pub fn part_experiments(m: &Value, out: &mut Vec<ose_data::science::PartExperiment>) {
    let Some(entries) = m["ModuleData"].as_array() else {
        return;
    };
    for entry in entries {
        if entry["Name"].as_str() != Some("Data_ScienceExperiment") {
            continue;
        }
        if let Some(list) = entry["DataObject"]["Experiments"].as_array() {
            for e in list {
                let ec = e["ResourcesCost"]
                    .as_array()
                    .map(|c| {
                        c.iter()
                            .filter(|r| r["ResourceName"].as_str() == Some("ElectricCharge"))
                            .map(|r| f(r, "Rate"))
                            .sum()
                    })
                    .unwrap_or(0.0);
                out.push(ose_data::science::PartExperiment {
                    experiment: s(e, "ExperimentDefinitionID"),
                    crew_required: e["CrewRequired"].as_u64().unwrap_or(0) as u32,
                    time_to_complete_s: f(e, "TimeToComplete"),
                    electric_rate: ec,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsampling_keeps_hemispheres_and_longitude_halves() {
        // 8 x 4 source: west half region 1, east half region 2; top (north) row region 3
        let (w, h) = (8usize, 4usize);
        let mut src = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                src[y * w + x] = if y == h - 1 {
                    3
                } else if x < w / 2 {
                    1
                } else {
                    2
                };
            }
        }
        let d = downsample(&src, w, h);
        assert_eq!(d[MAP_W / 4], 1);
        assert_eq!(d[3 * MAP_W / 4], 2);
        assert_eq!(d[(MAP_H - 1) * MAP_W + 5], 3);
    }
}

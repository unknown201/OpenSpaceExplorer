//! Converts a KSP2 workspace / stock assembly file into an `ose_data::craft::Craft`.
//!
//! Shape: `{ Metadata: {...}, Assemblies: [{ Assembly: { parts: [...], stagingState: {...}, ... } }] }`.
//! Each part carries `partState.attachNodeStates[]` naming the part attached at each node; the tree is built
//! by walking from the first listed part (the root pod).

use ose_data::craft::{Craft, CraftPart, Quat, StoredResource};
use ose_math::DVec3;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

fn f(v: &Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or(0.0)
}
fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}
fn guid(v: &Value) -> String {
    v["Guid"].as_str().unwrap_or("").to_string()
}
const NIL: &str = "00000000-0000-0000-0000-000000000000";

/// A part's paint (`Data_Color` Base and Accent), sRGB bytes.
fn part_colors(v: &Value) -> Option<ose_data::craft::Paint> {
    fn find(v: &Value) -> Option<&Value> {
        match v {
            Value::Object(m) => {
                if m.get("Name").and_then(|n| n.as_str()) == Some("Data_Color") {
                    return m.get("DataObject");
                }
                m.values().find_map(find)
            }
            Value::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    let d = find(v)?;
    let rgb = |k: &str| -> Option<[u8; 3]> {
        let c = &d[k]["storedValue"];
        let b = |x: &str| Some((c[x].as_f64()?.clamp(0.0, 1.0) * 255.0).round() as u8);
        Some([b("r")?, b("g")?, b("b")?])
    };
    Some((rgb("Base")?, rgb("Accent")?))
}

/// The part at the far end of a two-anchor part (fuel line, strut): `Data_CompoundPart.OtherEndAnchor`.
fn compound_other_end(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => {
            if m.get("Name").and_then(|n| n.as_str()) == Some("Data_CompoundPart") {
                let g = guid(&m.get("DataObject")?["OtherEndAnchor"]);
                return (!g.is_empty() && g != NIL).then_some(g);
            }
            m.values().find_map(compound_other_end)
        }
        Value::Array(a) => a.iter().find_map(compound_other_end),
        _ => None,
    }
}

pub fn craft_from_value(v: &Value) -> anyhow::Result<Craft> {
    let meta = &v["Metadata"];
    let name = meta["VehicleName"]
        .as_str()
        .or_else(|| meta["Name"].as_str())
        .unwrap_or("Unnamed")
        .to_string();
    let description = s(meta, "Description");
    let asm = v["Assemblies"]
        .as_array()
        .and_then(|a| a.first())
        .map(|a| &a["Assembly"])
        .ok_or_else(|| anyhow::anyhow!("craft has no assembly"))?;
    let raw: Vec<&Value> = asm["parts"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    anyhow::ensure!(!raw.is_empty(), "craft lists no parts");

    // index by guid
    let guids: Vec<String> = raw.iter().map(|p| guid(&p["PartGuid"])).collect();
    let index: HashMap<&str, usize> = guids
        .iter()
        .enumerate()
        .map(|(i, g)| (g.as_str(), i))
        .collect();

    // adjacency: (node on this part, other part index, node on the other part)
    let mut links: Vec<Vec<(String, usize, String)>> = vec![Vec::new(); raw.len()];
    for (i, p) in raw.iter().enumerate() {
        for n in p["partState"]["attachNodeStates"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let other = guid(&n["AttachedPartGuid"]);
            if other.is_empty() || other == NIL {
                continue;
            }
            if let Some(&j) = index.get(other.as_str()) {
                // links are listed on one side only (e.g. a surface-attached child names its parent, the
                // parent does not name the child), so record both directions
                let own = s(n, "nodeId");
                let theirs = s(n, "attachedPartNodeID");
                links[i].push((own.clone(), j, theirs.clone()));
                if !links[j].iter().any(|(_, back, _)| *back == i) {
                    links[j].push((theirs, i, own));
                }
            }
        }
    }

    // BFS from the first part to orient the tree
    let mut parent: Vec<Option<(usize, String, String)>> = vec![None; raw.len()];
    let mut seen = vec![false; raw.len()];
    let mut q = VecDeque::new();
    seen[0] = true;
    q.push_back(0usize);
    while let Some(i) = q.pop_front() {
        for (own_node, j, other_node) in &links[i] {
            if !seen[*j] {
                seen[*j] = true;
                // the child's own node is the node on j that points back at i, if listed
                let child_node = links[*j]
                    .iter()
                    .find(|(_, back, _)| *back == i)
                    .map(|(n, _, _)| n.clone())
                    .unwrap_or_else(|| other_node.clone());
                parent[*j] = Some((i, child_node, own_node.clone()));
                q.push_back(*j);
            }
        }
    }

    let mut parts = Vec::with_capacity(raw.len());
    for (i, p) in raw.iter().enumerate() {
        let ps = &p["partState"];
        let lp = &ps["localPosition"];
        let lr = &ps["localRotation"];
        let resources = ps["resources"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, r)| StoredResource {
                        resource: r["name"].as_str().unwrap_or(k).to_string(),
                        stored_units: f(r, "storedUnits"),
                        capacity_units: f(r, "capacityUnits"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (par, own_node, parent_node) = match &parent[i] {
            Some((pi, own, pn)) => (Some(*pi), Some(own.clone()), Some(pn.clone())),
            None => (None, None, None),
        };
        parts.push(CraftPart {
            flight: ps["isMirrored"].as_bool().unwrap_or(false).then(|| {
                ose_data::flight_geometry::PartFlight {
                    part_id: s(p, "partName"),
                    mirrored: true,
                    ..Default::default()
                }
            }),
            colors: part_colors(p),
            guid: guids[i].clone(),
            part_id: s(p, "partName"),
            parent: par,
            own_node,
            parent_node,
            local_position: DVec3::new(f(lp, "x"), f(lp, "y"), f(lp, "z")),
            local_rotation: Quat {
                x: f(lr, "x"),
                y: f(lr, "y"),
                z: f(lr, "z"),
                w: f(lr, "w"),
            },
            stage_index: ps["StageIndex"].as_i64().unwrap_or(-1) as i32,
            fuel_crossfeed: ps["FuelCrossfeed"].as_bool().unwrap_or(true),
            resources,
            // a fuel line (not a strut) feeds the part at its other end
            fuel_line_target: if s(p, "partName").starts_with("fuelline") {
                compound_other_end(p).and_then(|g| index.get(g.as_str()).copied())
            } else {
                None
            },
        });
    }
    // parts not reached from the root become roots of their own; report rather than hide
    let unreached: Vec<&str> = seen
        .iter()
        .enumerate()
        .filter(|(_, s)| !**s)
        .map(|(i, _)| guids[i].as_str())
        .collect();
    anyhow::ensure!(
        unreached.is_empty(),
        "{} parts not attached to the root: {unreached:?}",
        unreached.len()
    );

    let mut stages: Vec<(i64, Vec<usize>)> = Vec::new();
    for st in asm["stagingState"]["availableStages"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let id = st["StageID"].as_i64().unwrap_or(0);
        let members = st["PartIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| index.get(guid(g).as_str()).copied())
            .collect();
        stages.push((id, members));
    }
    stages.sort_by_key(|(id, _)| *id);
    // the craft's paint is the one most parts wear; parts keep theirs only where it differs
    let mut counts: Vec<(ose_data::craft::Paint, usize)> = Vec::new();
    for c in parts.iter().filter_map(|p| p.colors) {
        match counts.iter_mut().find(|x| x.0 == c) {
            Some(x) => x.1 += 1,
            None => counts.push((c, 1)),
        }
    }
    let colors = counts.iter().max_by_key(|x| x.1).map(|x| x.0);
    for p in &mut parts {
        if p.colors == colors {
            p.colors = None;
        }
    }
    Ok(Craft {
        colors,
        loose: Vec::new(),
        name,
        description,
        parts,
        stages: stages.into_iter().map(|(_, m)| m).collect(),
    })
}

/// Reapply curated measurements only to the exact stock assembly they were captured from.
pub fn apply_stock_flight_geometry(craft: &mut Craft, source: &[u8]) -> anyhow::Result<()> {
    let values: Value = serde_json::from_str(include_str!(
        "../../../assets/own/stock-flight-geometry.json"
    ))?;
    let Some(entry) = values.get(&craft.name) else {
        return Ok(());
    };
    let hash = source.iter().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    });
    anyhow::ensure!(
        entry["source_fnv1a64"].as_str() == Some(&format!("{hash:016x}")),
        "stock flight geometry requires a fresh capture for {}",
        craft.name
    );
    for row in entry["parts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("flight geometry parts"))?
    {
        let part = craft
            .parts
            .iter_mut()
            .find(|p| {
                p.guid == row["guid"].as_str().unwrap_or("")
                    && p.part_id == row["part_id"].as_str().unwrap_or("")
            })
            .ok_or_else(|| anyhow::anyhow!("captured part does not match stock craft"))?;
        let flight: ose_data::flight_geometry::PartFlight =
            serde_json::from_value(row["flight"].clone())?;
        anyhow::ensure!(flight.valid(), "invalid captured part geometry");
        part.flight = Some(flight);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stock_measurements_require_the_original_assembly_bytes() {
        let mut craft = Craft {
            name: "Jumping Flea".into(),
            ..Default::default()
        };
        assert!(apply_stock_flight_geometry(&mut craft, b"modified stock assembly").is_err());
        craft.name = "Custom craft".into();
        assert!(apply_stock_flight_geometry(&mut craft, b"custom").is_ok());
    }

    #[test]
    fn two_part_craft_builds_a_tree_with_resources_and_stages() {
        let v = json!({
            "Metadata": {"VehicleName": "Tiny", "Description": "d"},
            "Assemblies": [{"Assembly": {
                "parts": [
                    {"partName": "pod", "PartGuid": {"Guid": "a"}, "partState": {
                        "attachNodeStates": [{"nodeId": "bottom", "AttachedPartGuid": {"Guid": "b"}, "attachedPartNodeID": "top"}],
                        "resources": {"ElectricCharge": {"name": "ElectricCharge", "storedUnits": 150.0, "capacityUnits": 150.0}},
                        "localPosition": {"x": 0, "y": 1, "z": 0}, "localRotation": {"x": 0, "y": 0, "z": 0, "w": 1},
                        "StageIndex": -1, "FuelCrossfeed": true}},
                    {"partName": "engine", "PartGuid": {"Guid": "b"}, "partState": {
                        "attachNodeStates": [{"nodeId": "top", "AttachedPartGuid": {"Guid": "a"}, "attachedPartNodeID": "bottom"}],
                        "resources": null,
                        "localPosition": {"x": 0, "y": 0, "z": 0}, "localRotation": {"x": 0, "y": 0, "z": 0, "w": 1},
                        "StageIndex": 0, "FuelCrossfeed": true}}
                ],
                "stagingState": {"availableStages": [{"PartIds": [{"Guid": "b"}], "StageID": 0}]}
            }}]
        });
        let c = craft_from_value(&v).unwrap();
        assert_eq!(c.name, "Tiny");
        assert_eq!(c.parts.len(), 2);
        assert!(c.is_tree());
        assert_eq!(c.parts[1].parent, Some(0));
        assert_eq!(c.parts[1].own_node.as_deref(), Some("top"));
        assert_eq!(c.parts[1].parent_node.as_deref(), Some("bottom"));
        assert_eq!(c.parts[0].resources[0].stored_units, 150.0);
        assert_eq!(c.stages, vec![vec![1]]);
    }

    #[test]
    fn a_fuel_line_points_at_the_part_it_feeds_and_a_strut_does_not() {
        let at = |id: &str, to: &str, node: &str, other: &str| json!({"nodeId": node, "AttachedPartGuid": {"Guid": to}, "attachedPartNodeID": other, "_": id});
        let compound = |to: &str| {
            json!([{"ModuleData": [{"Name": "Data_CompoundPart",
                "DataObject": {"OtherEndAnchor": {"Guid": to, "DebugName": "x"}}}]}])
        };
        let part = |name: &str,
                    g: &str,
                    nodes: Vec<serde_json::Value>,
                    modules: serde_json::Value| {
            json!({"partName": name, "PartGuid": {"Guid": g}, "partState": {
                "attachNodeStates": nodes, "resources": null,
                "localPosition": {"x": 0, "y": 0, "z": 0}, "localRotation": {"x": 0, "y": 0, "z": 0, "w": 1},
                "StageIndex": -1, "FuelCrossfeed": true, "PartModulesState": modules}})
        };
        let v = json!({
            "Metadata": {"VehicleName": "Lined"},
            "Assemblies": [{"Assembly": {"parts": [
                part("tank", "a", vec![at("a", "b", "bottom", "top"), at("a", "c", "srfAttach", "srfAttach"),
                    at("a", "d", "srfAttach2", "srfAttach")], json!(null)),
                part("tank", "b", vec![at("b", "a", "top", "bottom")], json!(null)),
                part("fuelline_0v_external", "c", vec![at("c", "a", "srfAttach", "srfAttach")], compound("b")),
                part("strut_1v_extensible", "d", vec![at("d", "a", "srfAttach", "srfAttach2")], compound("b")),
            ]}}]
        });
        let c = craft_from_value(&v).unwrap();
        assert!(c.is_tree());
        assert_eq!(c.parts[2].parent, Some(0));
        assert_eq!(c.parts[2].fuel_line_target, Some(1));
        assert_eq!(c.parts[3].fuel_line_target, None);
    }
}

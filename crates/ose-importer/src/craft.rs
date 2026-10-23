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
    Ok(Craft {
        name,
        description,
        parts,
        stages: stages.into_iter().map(|(_, m)| m).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}

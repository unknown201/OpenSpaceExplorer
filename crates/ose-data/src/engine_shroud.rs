//! KSP2's engine shrouds: the fairing round an engine's bell that shows once a part hangs from the engine's
//! bottom node (`Module_Fairing` on an engine, `Data_Fairing.IsShroud`).
//!
//! The original builds the shroud procedurally rather than drawing a mesh from the engine's prefab
//! (`Module_Fairing.CheckForNeededShroud` -> `GenerateConnectingCrossSection`): two cross sections down the
//! part's -Y axis, the top ring at `FairingStartHeight` with `BaseRadius` and the bottom ring
//! `CrossSectionHeightMax` lower with half the target diameter. The target is the attached node's size class
//! plus one (`Mathf.Floor(node.Radius) + 1`), raised to `MinAutoFairingTargetRadius` when smaller than the
//! engine's own node and capped at `MaxAutoFairingTargetRadius` when larger, looked up in
//! `ObjectAssemblyPartTracker.AssemblySizeTypeDiameters` (`Module_Fairing.CalculateTargetAttachmentDiameter`).
//!
//! Definitions come from `assets/own/engine-shrouds.json` (tools/export_engine_shrouds.py).

use crate::craft::Craft;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// `ObjectAssemblyPartTracker.AssemblySizeTypeDiameters`, m.
pub const SIZE_DIAMETERS: [f64; 11] = [
    0.3125, 0.625, 1.25, 2.5, 3.75, 5.0, 10.0, 20.0, 40.0, 60.0, 80.0,
];

/// An engine's shroud definition (`Data_Fairing` fields).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ShroudDef {
    /// The floating node the shroud reaches down to (`FloatingAttachNodeTag`).
    pub node: String,
    /// That node's size class.
    pub node_size: i32,
    /// Part-frame height of the shroud's top ring (`FairingStartHeight`), m.
    pub start_height: f64,
    /// How far the wall runs down from there (`CrossSectionHeightMax`), m.
    pub height: f64,
    /// The top ring's radius (`BaseRadius`), m.
    pub base_radius: f64,
    pub side_count: u32,
    /// `MinAutoFairingTargetRadius` / `MaxAutoFairingTargetRadius` (size indices, -1 for none).
    pub min_target: i32,
    pub max_target: i32,
}

#[derive(Deserialize)]
struct File {
    shrouds: BTreeMap<String, ShroudDef>,
    stack_node_sizes: BTreeMap<String, BTreeMap<String, i32>>,
}

fn file() -> &'static File {
    static FILE: OnceLock<File> = OnceLock::new();
    FILE.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/own/engine-shrouds.json"))
            .expect("assets/own/engine-shrouds.json")
    })
}

pub fn def(part_id: &str) -> Option<&'static ShroudDef> {
    file().shrouds.get(part_id)
}

/// The size class of stack node `node` on part `part_id`.
pub fn node_size(part_id: &str, node: &str) -> Option<i32> {
    file().stack_node_sizes.get(part_id)?.get(node).copied()
}

impl ShroudDef {
    /// The bottom ring's radius, m, with a node of size class `other` attached
    /// (`Module_Fairing.CalculateTargetAttachmentDiameter`).
    pub fn target_radius(&self, other: Option<i32>) -> f64 {
        let own = self.node_size + 1;
        let mut value = other.map_or(own, |s| s + 1);
        value = value.clamp(0, SIZE_DIAMETERS.len() as i32 - 1);
        if self.max_target > 0 && value > own {
            value = value.min(self.max_target);
        } else if self.min_target > 0 && value < own {
            value = value.max(self.min_target);
        }
        SIZE_DIAMETERS[value as usize] * 0.5
    }

    /// The wall as (part-frame height, radius) rings from the bottom up.
    pub fn rings(&self, target_radius: f64) -> [(f64, f64); 2] {
        [
            (self.start_height - self.height, target_radius),
            (self.start_height, self.base_radius),
        ]
    }
}

/// Part `i`'s shroud when it is an engine with a part on its shroud node: its definition and the bottom
/// ring's radius. The part on the node can hang from the engine (a child on that node) or carry it (the
/// engine attached to its parent by that node).
pub fn shroud_for(craft: &Craft, i: usize) -> Option<(&'static ShroudDef, f64)> {
    let p = craft.parts.get(i)?;
    let d = def(&p.part_id)?;
    let other = if p.own_node.as_deref() == Some(d.node.as_str()) {
        let parent = &craft.parts[p.parent?];
        Some(
            p.parent_node
                .as_deref()
                .and_then(|n| node_size(&parent.part_id, n)),
        )
    } else {
        craft
            .parts
            .iter()
            .find(|q| q.parent == Some(i) && q.parent_node.as_deref() == Some(d.node.as_str()))
            .map(|q| q.own_node.as_deref().and_then(|n| node_size(&q.part_id, n)))
    }?;
    Some((d, d.target_radius(other)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::craft::CraftPart;

    fn part(id: &str, parent: Option<usize>, own: &str, on: &str) -> CraftPart {
        CraftPart {
            part_id: id.into(),
            parent,
            own_node: (!own.is_empty()).then(|| own.into()),
            parent_node: (!on.is_empty()).then(|| on.into()),
            ..Default::default()
        }
    }

    fn craft(parts: Vec<CraftPart>) -> Craft {
        Craft {
            parts,
            ..Default::default()
        }
    }

    #[test]
    fn swivel_shroud_matches_the_sections_ksp2_saves() {
        // Stock12_StartRocket saves the Swivel's shroud as heights 0.122 and 1.802 down from the model origin
        // (y 0.802), radius 0.625: part-frame rings at y 0.680 and -1.000.
        let c = craft(vec![
            part("fueltank_1v_inline_methalox_1x2", None, "", ""),
            part("engine_1v_methalox_swivel", Some(0), "top", "bottom"),
            part("decoupler_1v_inline", Some(1), "top", "bottom"),
        ]);
        let (d, r) = shroud_for(&c, 1).expect("a part hangs from the bottom node");
        let [lo, hi] = d.rings(r);
        assert!((hi.0 - (0.802 - 0.122)).abs() < 1e-3, "top ring {hi:?}");
        assert!((lo.0 - (0.802 - 1.802)).abs() < 1e-3, "bottom ring {lo:?}");
        assert_eq!((lo.1, hi.1), (0.625, 0.625));
        // The shroud spans the part between its stack nodes, not below them (the prefab's static
        // Fairing_Swivel mesh reached down to y -1.87).
        let bottom = node_size("engine_1v_methalox_swivel", "bottom");
        assert_eq!(bottom, Some(1));
        assert!(lo.0 >= -1.0 - 1e-3 && hi.0 <= 0.802);
    }

    #[test]
    fn no_shroud_without_a_part_on_the_bottom_node() {
        let c = craft(vec![
            part("fueltank_1v_inline_methalox_1x2", None, "", ""),
            part("engine_1v_methalox_swivel", Some(0), "top", "bottom"),
        ]);
        assert!(shroud_for(&c, 1).is_none());
        assert!(shroud_for(&c, 0).is_none(), "a tank has no shroud");
    }

    #[test]
    fn engine_carried_by_its_bottom_node_has_a_shroud() {
        let c = craft(vec![
            part("fueltank_2v_inline_methalox_1x2", None, "", ""),
            part("engine_1v_methalox_reliant", Some(0), "bottom", "top"),
        ]);
        let (_, r) = shroud_for(&c, 1).unwrap();
        // a 2v node below a 1v engine: the shroud flares out to the 2.5 m tank
        assert_eq!(r, 1.25);
    }

    #[test]
    fn target_radius_follows_calculate_target_attachment_diameter() {
        let swivel = def("engine_1v_methalox_swivel").unwrap();
        // smaller node below: raised to MinAutoFairingTargetRadius (index 2, 1.25 m)
        assert_eq!(swivel.target_radius(Some(0)), 0.625);
        assert_eq!(swivel.target_radius(Some(1)), 0.625);
        assert_eq!(swivel.target_radius(Some(3)), 1.875);
        // the Ant has no limits: it shrinks to a 0v node and flares to a 1v one
        let ant = def("engine_0v_methalox_ant").unwrap();
        assert_eq!(ant.target_radius(Some(0)), 0.3125);
        assert_eq!(ant.target_radius(Some(1)), 0.625);
    }

    #[test]
    fn every_shroud_runs_down_to_its_node() {
        // KSP2's shrouds end at the engine's bottom node (within a few cm), never past it.
        let catalog: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        assert_eq!(file().shrouds.len(), 19);
        for (id, d) in &file().shrouds {
            let p = catalog.iter().find(|p| p["id"] == id.as_str()).unwrap();
            let node = p["attach_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["id"] == d.node.as_str())
                .unwrap();
            let y = node["position"]["y"].as_f64().unwrap();
            let bottom = d.start_height - d.height;
            assert!(
                (bottom - y).abs() < 0.03,
                "{id}: shroud ends at {bottom}, node at {y}"
            );
        }
    }
}

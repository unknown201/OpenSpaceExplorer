//! KSP2's part shrouds: the fairing round an engine's bell, or round a heat shield's back, that shows once a
//! part hangs from its bottom node (`Module_Fairing` with `Data_Fairing.IsShroud`).
//!
//! The original builds the shroud procedurally rather than drawing a mesh from the engine's prefab
//! (`Module_Fairing.CheckForNeededShroud` -> `GenerateConnectingCrossSection`): two cross sections down the
//! part's -Y axis, the top ring at `FairingStartHeight` with `BaseRadius` and the bottom ring
//! `CrossSectionHeightMax` lower with half the target diameter. The target is the attached node's size class
//! plus one (`Mathf.Floor(node.Radius) + 1`), raised to `MinAutoFairingTargetRadius` when smaller than the
//! engine's own node and capped at `MaxAutoFairingTargetRadius` when larger, looked up in
//! `ObjectAssemblyPartTracker.AssemblySizeTypeDiameters` (`Module_Fairing.CalculateTargetAttachmentDiameter`).
//!
//! Procedural tubes and engine mounts have a wall of the same kind (`WallDef`): from `FairingStartHeight` out
//! along `LocalUpAxis` to their floating node `Length` away, built once a part sits on that node and flaring to
//! its size. A KSP2 VAB capture (reference/ksp2-procparts) shows the sections as part-frame heights along the
//! axis: tube_1v_procedural 0.1:0.625 to 1.25:1.25 under a 2.5 m tank, enginemount_1v_procedural 0.029:0.625 to
//! 1.25:0.625 down to a 1.25 m decoupler, and no wall with nothing on the node.
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

/// A procedural tube's or engine mount's wall (`Data_Fairing` with a floating node, not a shroud).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WallDef {
    /// The floating node at the wall's far end (`FloatingAttachNodeTag`).
    pub node: String,
    pub node_size: i32,
    /// `LocalUpAxis.y`: +1 for a tube (the wall runs up), -1 for an engine mount (down).
    pub up: f64,
    /// Part-frame height where the wall starts (`FairingStartHeight`), m.
    pub start_height: f64,
    /// `LengthEditDefault`: the wall ends `length` along the axis, at the floating node, m.
    pub length: f64,
    pub base_radius: f64,
    pub side_count: u32,
    pub min_target: i32,
    pub max_target: i32,
}

/// A wall to draw: (part-frame height, radius) rings from the bottom up, and its side count.
#[derive(Clone, Debug, PartialEq)]
pub struct Wall {
    pub rings: [(f64, f64); 2],
    pub side_count: u32,
}

#[derive(Deserialize)]
struct File {
    shrouds: BTreeMap<String, ShroudDef>,
    #[serde(default)]
    floating_walls: BTreeMap<String, WallDef>,
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

pub fn wall_def(part_id: &str) -> Option<&'static WallDef> {
    file().floating_walls.get(part_id)
}

/// The size class of stack node `node` on part `part_id`.
pub fn node_size(part_id: &str, node: &str) -> Option<i32> {
    file().stack_node_sizes.get(part_id)?.get(node).copied()
}

impl ShroudDef {
    /// The bottom ring's radius, m, with a node of size class `other` attached
    /// (`Module_Fairing.CalculateTargetAttachmentDiameter`).
    pub fn target_radius(&self, other: Option<i32>) -> f64 {
        target_radius(self.node_size, self.min_target, self.max_target, other)
    }

    /// The wall as (part-frame height, radius) rings from the bottom up.
    pub fn rings(&self, target_radius: f64) -> [(f64, f64); 2] {
        [
            (self.start_height - self.height, target_radius),
            (self.start_height, self.base_radius),
        ]
    }
}

/// `Module_Fairing.CalculateTargetAttachmentDiameter` / 2: the wall's far ring for a part with node size class
/// `other` on the node (of size class `node_size`), held within `MinAutoFairingTargetRadius` and
/// `MaxAutoFairingTargetRadius` (size indices; -1 for none).
pub fn target_radius(node_size: i32, min_target: i32, max_target: i32, other: Option<i32>) -> f64 {
    let own = node_size + 1;
    let mut value = other.map_or(own, |s| s + 1);
    value = value.clamp(0, SIZE_DIAMETERS.len() as i32 - 1);
    if max_target > 0 && value > own {
        value = value.min(max_target);
    } else if min_target > 0 && value < own {
        value = value.max(min_target);
    }
    SIZE_DIAMETERS[value as usize] * 0.5
}

/// The size class of the part on part `i`'s node `node`, when one is there: a child hanging from that node, or
/// the parent part `i` itself hangs from by it. `Some(None)` when that part's node has no known size.
fn on_node(craft: &Craft, i: usize, node: &str) -> Option<Option<i32>> {
    let p = craft.parts.get(i)?;
    if p.own_node.as_deref() == Some(node) {
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
            .find(|q| q.parent == Some(i) && q.parent_node.as_deref() == Some(node))
            .map(|q| q.own_node.as_deref().and_then(|n| node_size(&q.part_id, n)))
    }
}

/// Part `i`'s procedural wall, an engine's or heat shield's shroud or a tube's or engine mount's wall, when
/// a part sits on its node.
pub fn wall_for(craft: &Craft, i: usize) -> Option<Wall> {
    if let Some((d, r)) = shroud_for(craft, i) {
        let [lo, hi] = d.rings(r);
        return Some(Wall {
            rings: [lo, hi],
            side_count: d.side_count,
        });
    }
    let d = wall_def(&craft.parts.get(i)?.part_id)?;
    let other = on_node(craft, i, &d.node)?;
    let target = target_radius(d.node_size, d.min_target, d.max_target, other);
    let start = (d.start_height, d.base_radius);
    let end = (d.up * d.length, target);
    Some(Wall {
        rings: if end.0 < start.0 {
            [end, start]
        } else {
            [start, end]
        },
        side_count: d.side_count,
    })
}

/// Part `i`'s shroud when it is an engine with a part on its shroud node: its definition and the bottom
/// ring's radius. The part on the node can hang from the engine (a child on that node) or carry it (the
/// engine attached to its parent by that node).
pub fn shroud_for(craft: &Craft, i: usize) -> Option<(&'static ShroudDef, f64)> {
    let d = def(&craft.parts.get(i)?.part_id)?;
    let other = on_node(craft, i, &d.node)?;
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
    fn heat_shield_shrouds_down_to_a_decoupler_below() {
        // heatshield_1v: Data_Fairing IsShroud, start 0.052, 0.1597 down to its bottom node at -0.1077, and
        // MinAutoFairingTargetRadius 2 keeps a smaller part below from pinching it in.
        let c = craft(vec![
            part("pod_1v_conical_crew", None, "", ""),
            part("heatshield_1v", Some(0), "top", "bottom"),
            part("decoupler_1v_inline", Some(1), "top", "bottom"),
        ]);
        let (d, r) = shroud_for(&c, 1).expect("a decoupler hangs from the shield");
        let [lo, hi] = d.rings(r);
        assert!((hi.0 - 0.052).abs() < 1e-6 && (lo.0 + 0.1077).abs() < 1e-4);
        assert_eq!((lo.1, hi.1), (0.625, 0.625));
        assert_eq!(d.target_radius(Some(0)), 0.625);
        let bare = craft(vec![
            part("pod_1v_conical_crew", None, "", ""),
            part("heatshield_1v", Some(0), "top", "bottom"),
        ]);
        assert!(shroud_for(&bare, 1).is_none());
    }

    #[test]
    fn tube_wall_matches_ksp2_vab_capture() {
        // reference/ksp2-procparts: a tube on the pod has no wall until a part sits on its floating node; with
        // a 2.5 m tank there the wall runs 0.1:0.625 to 1.25:1.25, with a 1.25 m tank to 1.25:0.625.
        let bare = craft(vec![
            part("pod_1v_conical_crew", None, "", ""),
            part("tube_1v_procedural", Some(0), "bottom", "top"),
        ]);
        assert!(wall_for(&bare, 1).is_none());
        let wide = craft(vec![
            part("pod_1v_conical_crew", None, "", ""),
            part("tube_1v_procedural", Some(0), "bottom", "top"),
            part(
                "fueltank_2v_inline_methalox_1x2",
                Some(1),
                "bottom",
                "dynamic_floating",
            ),
        ]);
        let w = wall_for(&wide, 1).unwrap();
        assert_eq!(w.rings, [(0.1, 0.625), (1.25, 1.25)]);
        assert_eq!(w.side_count, 24);
        // hung from a tank by its floating node: the wall flares to that tank
        let hung = craft(vec![
            part("fueltank_1v_inline_methalox_1x2", None, "", ""),
            part("tube_1v_procedural", Some(0), "dynamic_floating", "bottom"),
        ]);
        assert_eq!(
            wall_for(&hung, 1).unwrap().rings,
            [(0.1, 0.625), (1.25, 0.625)]
        );
    }

    #[test]
    fn engine_mount_wall_runs_down_to_its_floating_node() {
        // reference/ksp2-procparts: enginemount_1v_procedural with a decoupler on its floating node, panels
        // from y -1.25 to -0.029 at 0.625 m; the node at y -1.25 in the parts catalog.
        let c = craft(vec![
            part("fueltank_1v_inline_methalox_1x2", None, "", ""),
            part("enginemount_1v_procedural", Some(0), "top", "bottom"),
            part("decoupler_1v_inline", Some(1), "top", "dynamic_floating"),
        ]);
        let w = wall_for(&c, 1).unwrap();
        assert_eq!(w.rings, [(-1.25, 0.625), (-0.029, 0.625)]);
        let catalog: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        for (id, d) in &file().floating_walls {
            let p = catalog.iter().find(|p| p["id"] == id.as_str()).unwrap();
            let n = p["attach_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["id"] == d.node.as_str())
                .unwrap_or_else(|| panic!("{id} has no {} node", d.node));
            assert_eq!(
                n["position"]["y"].as_f64().unwrap(),
                d.up * d.length,
                "{id}"
            );
            assert_eq!(n["orientation"]["y"].as_f64().unwrap(), d.up, "{id}");
        }
        assert_eq!(file().floating_walls.len(), 9);
    }

    #[test]
    fn every_shroud_runs_down_to_its_node() {
        // KSP2's shrouds end at the engine's bottom node (within a few cm), never past it.
        let catalog: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        assert_eq!(file().shrouds.len(), 24);
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

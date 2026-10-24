//! A craft: a tree of part instances with attachment, placement, staging and stored resources.
//!
//! Imported from KSP2 workspace/stock assembly files by `ose-importer`, uploaded by clients, spawned by the
//! server. Geometry is in the craft's own frame (metres, Unity Y-up as authored; converted by the renderer).

use crate::{PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CraftPart {
    /// Stable id within the craft (the original part GUID when imported).
    pub guid: String,
    /// `PartDef::id`.
    pub part_id: String,
    /// Index of the parent part in `Craft::parts`; `None` for the root.
    pub parent: Option<usize>,
    /// Attach node on this part used for the parent link, and the node on the parent.
    pub own_node: Option<String>,
    pub parent_node: Option<String>,
    pub local_position: DVec3,
    pub local_rotation: Quat,
    pub stage_index: i32,
    pub fuel_crossfeed: bool,
    /// Stored resource units by resource id.
    pub resources: Vec<StoredResource>,
    /// A fuel line (compound part) hanging on its parent tank feeds this part (index in `Craft::parts`).
    // (no skip_serializing_if: crafts also travel in postcard, which needs every field)
    #[serde(default)]
    pub fuel_line_target: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredResource {
    pub resource: String,
    pub stored_units: f64,
    pub capacity_units: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Craft {
    pub name: String,
    pub description: String,
    pub parts: Vec<CraftPart>,
    /// Stages in activation order (stage 0 first); each lists part indices activated by that stage.
    pub stages: Vec<Vec<usize>>,
}

/// Mass summary of a craft against a part and resource catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct MassSummary {
    pub dry_t: f64,
    pub resources_t: f64,
    pub unknown_parts: Vec<String>,
}

impl MassSummary {
    pub fn wet_t(&self) -> f64 {
        self.dry_t + self.resources_t
    }
}

/// Vacuum delta-v estimate for a craft: Tsiolkovsky over the whole craft using the mass-weighted best
/// vacuum Isp of its engines, with every stored propellant counted as burnable. A per-stage model comes
/// with the resource flow graph; this is the number for a first readout.
pub fn delta_v_vacuum_mps(craft: &Craft, parts: &[PartDef], resources: &[ResourceDef]) -> f64 {
    let m = craft.mass(parts, resources);
    if m.resources_t <= 0.0 || m.wet_t() <= 0.0 {
        return 0.0;
    }
    let by_id: HashMap<&str, &PartDef> = parts.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut best_isp: f64 = 0.0;
    for p in &craft.parts {
        if let Some(def) = by_id.get(p.part_id.as_str()) {
            for e in &def.engines {
                best_isp = best_isp.max(e.isp_curve.evaluate(0.0));
            }
        }
    }
    if best_isp <= 0.0 {
        return 0.0;
    }
    best_isp * crate::G0 * (m.wet_t() / m.dry_t).ln()
}

impl Craft {
    /// Children of part `i`.
    pub fn children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.parts
            .iter()
            .enumerate()
            .filter(move |(_, p)| p.parent == Some(i))
            .map(|(j, _)| j)
    }

    /// True when every part's parent index is valid and exactly one part is the root.
    pub fn is_tree(&self) -> bool {
        let roots = self.parts.iter().filter(|p| p.parent.is_none()).count();
        roots == 1
            && self
                .parts
                .iter()
                .all(|p| p.parent.is_none_or(|i| i < self.parts.len()))
    }

    pub fn mass(&self, parts: &[PartDef], resources: &[ResourceDef]) -> MassSummary {
        let by_id: HashMap<&str, &PartDef> = parts.iter().map(|p| (p.id.as_str(), p)).collect();
        let res_mass: HashMap<&str, f64> = resources
            .iter()
            .filter_map(|r| match &r.kind {
                ResourceKind::Basic {
                    mass_per_unit_t, ..
                } => Some((r.id.as_str(), *mass_per_unit_t)),
                ResourceKind::Recipe { .. } => None,
            })
            .collect();
        let mut dry = 0.0;
        let mut wet = 0.0;
        let mut unknown = Vec::new();
        for p in &self.parts {
            match by_id.get(p.part_id.as_str()) {
                Some(def) => dry += def.mass_t,
                None => unknown.push(p.part_id.clone()),
            }
            for r in &p.resources {
                wet += r.stored_units * res_mass.get(r.resource.as_str()).copied().unwrap_or(0.0);
            }
        }
        MassSummary {
            dry_t: dry,
            resources_t: wet,
            unknown_parts: unknown,
        }
    }
}

//! OSE's own colony content (`docs/colonies-design.md`): KSP2 never shipped colonies, so its data has no
//! colony research and no building material. The importer adds these to what it converts, and a test keeps
//! `assets/own/` in step with them.
//!
//! - **Metal**, the building material: shipped from Kerbin in the colony trunks, like fuel in a tank. A
//!   craft launched from a colony pays its dry mass in Metal out of the colony's stock.
//! - **Colony research**: two nodes after the 3.75 m payload node, unlocking the small then the large trunk.
use crate::science::TechNode;
use crate::{FlowMode, PartDef, ResourceContainer, ResourceDef, ResourceKind};

/// The building material's resource id.
pub const METAL: &str = "Metal";
/// Tonnes per unit of Metal: one unit is a kilogram, so a part's dry mass in kg is its price.
pub const METAL_UNIT_T: f64 = 0.001;
/// The trunks and the Metal each holds (units).
pub const TRUNKS: [(&str, f64); 2] = [
    ("transport_3v_colony_trunk_small", 10_000.0),
    ("transport_3v_colony_trunk_large", 40_000.0),
];
/// One node of the colony research path.
pub struct ColonyNode {
    pub id: &'static str,
    pub name: &'static str,
    /// What it is for.
    pub description: &'static str,
    pub science: u32,
    pub requires: &'static str,
    pub position: (f64, f64),
    /// The trunk it unlocks.
    pub trunk: &'static str,
}

/// The colony research path, after the 3.75 m payload node.
pub const NODES: [ColonyNode; 2] = [
    ColonyNode {
        id: "tNode_ose_colonies_01",
        name: "Colony Foundations",
        description: "Found colonies on other worlds with a crewed colony trunk, and launch craft from their pads on fuel and Metal shipped from Kerbin.",
        science: 2900,
        requires: "tNode_3v_payload_01",
        position: (3780.0, 310.0),
        trunk: "transport_3v_colony_trunk_small",
    },
    ColonyNode {
        id: "tNode_ose_colonies_02",
        name: "Colony Logistics",
        description: "A larger colony trunk that ships forty tonnes of Metal at a time.",
        science: 4500,
        requires: "tNode_ose_colonies_01",
        position: (3960.0, 310.0),
        trunk: "transport_3v_colony_trunk_large",
    },
];

/// Localization keys of OSE's own nodes, with their English text.
pub fn node_name_key(id: &str) -> String {
    format!("OSE/TechNodes/Names/{id}")
}
pub fn node_description_key(id: &str) -> String {
    format!("OSE/TechNodes/Descriptions/{id}")
}

/// The English text for one of OSE's own localization keys.
pub fn own_text(key: &str) -> Option<&'static str> {
    if key == "Resource/DisplayName/Metal" {
        return Some("Metal");
    }
    NODES.iter().find_map(|n| {
        if key == node_name_key(n.id) {
            Some(n.name)
        } else if key == node_description_key(n.id) {
            Some(n.description)
        } else {
            None
        }
    })
}

pub fn metal() -> ResourceDef {
    ResourceDef {
        id: METAL.into(),
        display_name_key: "Resource/DisplayName/Metal".into(),
        abbreviation_key: "Resource/Abbreviation/Metal".into(),
        kind: ResourceKind::Basic {
            mass_per_unit_t: METAL_UNIT_T,
            volume_per_unit: 0.2,
            specific_heat_capacity_per_unit: 0.45,
            flow_mode: FlowMode::AllVessel,
            pumpable: true,
            cost_per_unit: 0.01,
            non_stageable: false,
        },
    }
}

/// Add Metal, the trunks' Metal holds and the colony research nodes. Running it twice changes nothing.
pub fn add_colony_content(
    parts: &mut [PartDef],
    resources: &mut Vec<ResourceDef>,
    tech: &mut Vec<TechNode>,
) {
    if !resources.iter().any(|r| r.id == METAL) {
        resources.push(metal());
    }
    for (id, units) in TRUNKS {
        if let Some(p) = parts.iter_mut().find(|p| p.id == id) {
            p.resource_containers.retain(|c| c.resource != METAL);
            p.resource_containers.push(ResourceContainer {
                resource: METAL.into(),
                capacity_units: units,
                initial_units: units,
            });
        }
    }
    for n in &mut *tech {
        n.parts.retain(|p| !TRUNKS.iter().any(|t| t.0 == p));
    }
    for n in &NODES {
        tech.retain(|t| t.id != n.id);
        tech.push(TechNode {
            id: n.id.into(),
            name_key: node_name_key(n.id),
            description_key: node_description_key(n.id),
            icon: "ICO-RDCenter-Payload-24x".into(),
            required_science: n.science,
            parts: vec![n.trunk.into()],
            requires: vec![n.requires.into()],
            hidden_by: String::new(),
            tier: 0,
            position: n.position,
        });
    }
    tech.sort_by(|a, b| a.id.cmp(&b.id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assets_carry_the_colony_content() {
        let parts: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let resources: Vec<ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let tech: Vec<TechNode> =
            serde_json::from_str(include_str!("../../../assets/own/tech-tree.json")).unwrap();
        let (mut p, mut r, mut t) = (parts.clone(), resources.clone(), tech.clone());
        add_colony_content(&mut p, &mut r, &mut t);
        assert!(p == parts, "parts-catalog.json lacks the trunks' Metal");
        assert!(r == resources, "resources.json lacks Metal");
        assert!(t == tech, "tech-tree.json lacks the colony nodes");
        // every prerequisite exists, and each trunk is unlocked by exactly one node
        for n in &t {
            for q in &n.requires {
                assert!(t.iter().any(|x| &x.id == q), "{} needs {q}", n.id);
            }
        }
        for (trunk, _) in TRUNKS {
            assert_eq!(
                t.iter()
                    .filter(|n| n.parts.iter().any(|p| p == trunk))
                    .count(),
                1
            );
        }
        assert_eq!(
            own_text("OSE/TechNodes/Names/tNode_ose_colonies_01"),
            Some("Colony Foundations")
        );
    }
}

//! OSE's own colony content (`docs/colonies-design.md`): KSP2 never shipped colonies, so its data has no
//! colony research and no building material. The importer adds these to what it converts, and a test keeps
//! `assets/own/` in step with them.
//!
//! - **Metal**, the building material: shipped from Kerbin in the colony trunks, like fuel in a tank. A
//!   craft launched from a colony pays its dry mass in Metal out of the colony's stock.
//! - **Colony research**: two nodes after the 3.75 m payload node, unlocking the small then the large trunk,
//!   and a third for mining.
//! - **Mining** (in-situ resources): **Ore** dug out of the ground by a drill, where the ground holds some
//!   (`crate::abundance`), and an ore refinery that makes it into methalox. KSP2 planned these and shipped
//!   their models without any working modules, so OSE gives two of its unused parts the jobs: the deep
//!   resource scanner becomes the drill and the methalox factory the refinery ([`MINING`]).
use crate::science::TechNode;
use crate::{Converter, FlowMode, PartDef, ResourceContainer, ResourceDef, ResourceKind};

/// The building material's resource id.
pub const METAL: &str = "Metal";
/// Tonnes per unit of Metal: one unit is a kilogram, so a part's dry mass in kg is its price.
pub const METAL_UNIT_T: f64 = 0.001;
/// The mined resource's id.
pub const ORE: &str = "Ore";
/// Tonnes per unit of Ore.
pub const ORE_UNIT_T: f64 = 0.01;
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
    /// The parts it unlocks.
    pub parts: &'static [&'static str],
}

/// The colony research path, after the 3.75 m payload node.
pub const NODES: [ColonyNode; 3] = [
    ColonyNode {
        id: "tNode_ose_colonies_01",
        name: "Colony Foundations",
        description: "Found colonies on other worlds with a crewed colony trunk, and launch craft from their pads on fuel and Metal shipped from Kerbin.",
        science: 2900,
        requires: "tNode_3v_payload_01",
        position: (3780.0, 310.0),
        parts: &["transport_3v_colony_trunk_small"],
    },
    ColonyNode {
        id: "tNode_ose_colonies_02",
        name: "Colony Logistics",
        description: "A larger colony trunk that ships forty tonnes of Metal at a time.",
        science: 4500,
        requires: "tNode_ose_colonies_01",
        position: (3960.0, 310.0),
        parts: &["transport_3v_colony_trunk_large"],
    },
    ColonyNode {
        id: "tNode_ose_colonies_03",
        name: "Colony Mining",
        description: "Drill Ore out of the ground on other worlds and refine it into methalox, so a colony fuels its own launches.",
        science: 4500,
        requires: "tNode_ose_colonies_01",
        position: (3960.0, 470.0),
        parts: &[DRILL, REFINERY],
    },
];

/// The drill: KSP2's deep resource scanner (a radially mounted probe that reaches down into the ground).
pub const DRILL: &str = "scanner_1v_resources_deep";
/// The ore refinery: KSP2's methalox factory.
pub const REFINERY: &str = "factory_2v_methalox-monoprop";

/// A mining part as OSE makes it: its name, dry mass, what it holds and what it converts.
pub struct MiningPart {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub mass_t: f64,
    /// Ore it holds (units).
    pub ore_units: f64,
    pub converter: fn() -> Converter,
}

/// The drill digs 0.5 Ore (5 kg) a second on the richest ground for 15 EC/s; the refinery makes 2 Ore a
/// second into 18 kg of methalox, a fifth methane and four fifths oxidizer by mass, for 30 EC/s.
pub const MINING: [MiningPart; 2] = [
    MiningPart {
        id: DRILL,
        title: "Ore Drill",
        category: "Utility",
        mass_t: 1.25,
        ore_units: 100.0,
        converter: || Converter {
            formula: "OseDrillOre".into(),
            inputs: vec![("ElectricCharge".into(), 15.0)],
            outputs: vec![(ORE.into(), 0.5)],
            conversion_rate: 1.0,
            harvests: true,
        },
    },
    MiningPart {
        id: REFINERY,
        title: "Ore Refinery",
        category: "Utility",
        mass_t: 4.25,
        ore_units: 1500.0,
        converter: || Converter {
            formula: "OseOre2Methalox".into(),
            inputs: vec![(ORE.into(), 2.0), ("ElectricCharge".into(), 30.0)],
            outputs: vec![("Methane".into(), 0.0036), ("Oxidizer".into(), 0.0144)],
            conversion_rate: 1.0,
            harvests: false,
        },
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
    match key {
        "Resource/DisplayName/Metal" => return Some("Metal"),
        "Resource/DisplayName/Ore" => return Some("Ore"),
        _ => {}
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

pub fn ore() -> ResourceDef {
    ResourceDef {
        id: ORE.into(),
        display_name_key: "Resource/DisplayName/Ore".into(),
        abbreviation_key: "Resource/Abbreviation/Ore".into(),
        kind: ResourceKind::Basic {
            mass_per_unit_t: ORE_UNIT_T,
            volume_per_unit: 5.0,
            specific_heat_capacity_per_unit: 1000.0,
            flow_mode: FlowMode::AllVessel,
            pumpable: true,
            cost_per_unit: 0.02,
            non_stageable: false,
        },
    }
}

/// Add Metal and Ore, the trunks' Metal holds, the mining parts and the colony research nodes. Running it
/// twice changes nothing.
pub fn add_colony_content(
    parts: &mut [PartDef],
    resources: &mut Vec<ResourceDef>,
    tech: &mut Vec<TechNode>,
) {
    for r in [metal(), ore()] {
        if !resources.iter().any(|x| x.id == r.id) {
            resources.push(r);
        }
    }
    for m in &MINING {
        if let Some(p) = parts.iter_mut().find(|p| p.id == m.id) {
            p.title_key = m.title.into();
            p.category = m.category.into();
            p.mass_t = m.mass_t;
            p.converter = Some((m.converter)());
            p.resource_containers.retain(|c| c.resource != ORE);
            p.resource_containers.push(ResourceContainer {
                resource: ORE.into(),
                capacity_units: m.ore_units,
                initial_units: 0.0,
            });
        }
    }
    let ours: Vec<&str> = NODES.iter().flat_map(|n| n.parts.iter().copied()).collect();
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
        n.parts.retain(|p| !ours.contains(&p.as_str()));
    }
    for n in &NODES {
        tech.retain(|t| t.id != n.id);
        tech.push(TechNode {
            id: n.id.into(),
            name_key: node_name_key(n.id),
            description_key: node_description_key(n.id),
            icon: "ICO-RDCenter-Payload-24x".into(),
            required_science: n.science,
            parts: n.parts.iter().map(|p| p.to_string()).collect(),
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
        assert!(
            p == parts,
            "parts-catalog.json lacks the trunks' Metal or the mining parts"
        );
        assert!(r == resources, "resources.json lacks Metal or Ore");
        assert!(t == tech, "tech-tree.json lacks the colony nodes");
        // every prerequisite exists, and each trunk is unlocked by exactly one node
        for n in &t {
            for q in &n.requires {
                assert!(t.iter().any(|x| &x.id == q), "{} needs {q}", n.id);
            }
        }
        for part in TRUNKS
            .iter()
            .map(|t| t.0)
            .chain(MINING.iter().map(|m| m.id))
        {
            assert!(p.iter().any(|x| x.id == part), "no part {part}");
            assert_eq!(
                t.iter()
                    .filter(|n| n.parts.iter().any(|p| p == part))
                    .count(),
                1
            );
        }
        let drill = p.iter().find(|x| x.id == DRILL).unwrap();
        assert!(drill.converter.as_ref().is_some_and(|c| c.harvests));
        assert_eq!(
            own_text("OSE/TechNodes/Names/tNode_ose_colonies_01"),
            Some("Colony Foundations")
        );
    }
}

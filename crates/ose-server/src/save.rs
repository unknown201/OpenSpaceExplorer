//! Universe save file: universal time, warp, and every vessel's orbit, owner and scheduled burn.
//!
//! Written as JSON periodically and on shutdown; loaded at startup when present. Bodies are not saved (they
//! come from the embedded catalog), only what players changed.

use ose_data::Tick;
use ose_math::Elements;
use ose_sim::vessels::ScheduledBurn;
use ose_sim::Universe;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SAVE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedVessel {
    pub id: u64,
    pub name: String,
    pub owner: Option<u64>,
    pub body: String,
    pub elements: Elements,
    pub burn: Option<SavedBurn>,
    #[serde(default)]
    pub flight: Option<ose_sim::flight::Powered>,
    /// Flight state kept while on rails (stage, propellant), restored as dormant.
    #[serde(default)]
    pub dormant: Option<ose_sim::flight::Powered>,
    /// Science reports the vessel carries.
    #[serde(default)]
    pub science: Vec<ose_data::science::ResearchReport>,
    /// Experiments still running.
    #[serde(default)]
    pub experiments: Vec<ose_sim::science::Run>,
    /// The vessel's parts as they are (lost parts left out, tanks as they were), its crew and docked joints,
    /// so a restored vessel gets its structure back.
    #[serde(default)]
    pub craft: Option<ose_data::craft::Craft>,
    #[serde(default)]
    pub crew: Vec<(usize, String)>,
    #[serde(default)]
    pub docked: Vec<(usize, usize)>,
    /// A Kerbal on EVA (their name) or a planted flag (who planted it).
    #[serde(default)]
    pub eva: Option<String>,
    #[serde(default)]
    pub flag: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedBurn {
    pub ut: f64,
    pub prograde_mps: f64,
    pub normal_mps: f64,
    pub radial_mps: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampaignMember {
    pub player_id: u64,
    pub display_name: String,
    pub joined_tick: Tick,
    #[serde(default)]
    pub agency: Option<ose_net::Agency>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Campaign {
    /// The campaign's id in the session list (the first campaign of a server is "shared").
    #[serde(default = "shared_id")]
    pub id: String,
    pub name: String,
    pub started_tick: Tick,
    pub members: Vec<CampaignMember>,
    #[serde(default)]
    pub owner: Option<u64>,
    #[serde(default = "default_mode")]
    pub mode: ose_net::SessionMode,
    #[serde(default)]
    pub difficulty: Vec<ose_net::DifficultyOption>,
    #[serde(default)]
    pub last_played_unix: u64,
    /// The agency's science, tech unlocks and missions.
    #[serde(default)]
    pub science: crate::science::ScienceProgress,
    /// The astronauts the campaign has hired, in order, with when they joined (universal time, s).
    #[serde(default)]
    pub roster: Vec<(String, f64)>,
    /// The campaign's quicksave (F5).
    #[serde(default)]
    pub quicksave: Option<Box<SaveFile>>,
}

fn shared_id() -> String {
    crate::SHARED_SESSION_ID.into()
}

fn default_mode() -> ose_net::SessionMode {
    ose_net::SessionMode::Exploration
}

impl Default for Campaign {
    fn default() -> Self {
        Campaign {
            id: shared_id(),
            name: "Shared campaign".into(),
            started_tick: Tick(0),
            members: Vec::new(),
            owner: None,
            mode: default_mode(),
            difficulty: Vec::new(),
            last_played_unix: 0,
            science: Default::default(),
            roster: Vec::new(),
            quicksave: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCraft {
    pub id: u64,
    pub owner: u64,
    pub craft: ose_data::craft::Craft,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveFile {
    pub version: u32,
    pub tick: Tick,
    pub warp: u32,
    pub vessels: Vec<SavedVessel>,
    #[serde(default)]
    pub campaign: Campaign,
    #[serde(default)]
    pub crafts: Vec<SavedCraft>,
    /// The campaigns not being played, each as its own save.
    #[serde(default)]
    pub shelf: Vec<ShelvedCampaign>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShelvedCampaign {
    pub id: String,
    pub save: SaveFile,
}

/// Vessel registry entry as the server keeps it: entity, vessel id, owner.
pub type VesselEntry = (bevy_ecs::entity::Entity, u64, Option<u64>);

pub fn capture(
    u: &Universe,
    vessels: &[VesselEntry],
    campaign: &Campaign,
    crafts: &[SavedCraft],
) -> SaveFile {
    let saved = vessels
        .iter()
        .filter_map(|&(e, id, owner)| {
            let (v, body, orbit) = u.vessel(e)?;
            Some(SavedVessel {
                id,
                name: v.name.clone(),
                owner,
                body: u.body_id(body)?.to_string(),
                elements: orbit.elements,
                flight: u.powered(e).cloned(),
                dormant: u.dormant(e).cloned(),
                science: u.science_carried(e),
                experiments: u.running_experiments(e),
                craft: u.current_craft(e),
                crew: u.crew(e),
                docked: u.docked_links(e),
                eva: u.eva_kerbal(e).map(|k| k.name.clone()),
                flag: u.flag_planter(e),
                burn: u.scheduled_burn(e).map(|b| SavedBurn {
                    ut: b.ut,
                    prograde_mps: b.prograde_mps,
                    normal_mps: b.normal_mps,
                    radial_mps: b.radial_mps,
                }),
            })
        })
        .collect();
    SaveFile {
        version: SAVE_VERSION,
        tick: u.tick(),
        warp: u.warp(),
        vessels: saved,
        campaign: campaign.clone(),
        crafts: crafts.to_vec(),
        shelf: Vec::new(),
    }
}

/// Restore a save into a fresh universe (bodies already spawned). Returns the vessel registry.
pub fn restore(
    u: &mut Universe,
    save: &SaveFile,
    parts: &[ose_data::PartDef],
    resources: &[ose_data::ResourceDef],
) -> Vec<VesselEntry> {
    u.set_tick(save.tick);
    u.set_warp(save.warp);
    let mut registry = Vec::new();
    for v in &save.vessels {
        let Some(body) = u.body_entity(&v.body) else {
            tracing::warn!(vessel = v.id, body = %v.body, "saved vessel's body is unknown; dropped");
            continue;
        };
        let e = u.spawn_vessel_with_elements(&v.name, body, v.elements);
        if let Some(f) = &v.flight {
            u.set_powered(e, f.clone());
        } else if let Some(d) = &v.dormant {
            u.set_dormant(e, d.clone());
        }
        if !v.science.is_empty() {
            u.set_science_carried(e, v.science.clone());
        }
        if !v.experiments.is_empty() {
            u.set_running_experiments(e, v.experiments.clone());
        }
        if let Some(craft) = &v.craft {
            u.set_craft(e, craft.clone());
            u.set_crew(e, v.crew.clone());
            u.set_structure(
                e,
                ose_sim::structure::Structure::build(craft, parts, resources),
            );
            u.set_docked(e, v.docked.clone());
        }
        if let Some(name) = &v.eva {
            u.restore_eva_kerbal(e, name);
        }
        if let Some(by) = &v.flag {
            u.restore_flag(e, by);
        }
        if let Some(b) = v.burn {
            u.schedule_burn(
                e,
                ScheduledBurn {
                    ut: b.ut,
                    prograde_mps: b.prograde_mps,
                    normal_mps: b.normal_mps,
                    radial_mps: b.radial_mps,
                },
            );
        }
        registry.push((e, v.id, v.owner));
    }
    registry
}

pub fn write(path: &Path, save: &SaveFile) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(save)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read(path: &Path) -> anyhow::Result<Option<SaveFile>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let save: SaveFile = serde_json::from_str(&text)?;
    anyhow::ensure!(
        save.version == SAVE_VERSION,
        "save version {} not supported (server writes {SAVE_VERSION})",
        save.version
    );
    Ok(Some(save))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_survives_saves_and_legacy_campaigns_remain_unowned() {
        let campaign = Campaign {
            owner: Some(42),
            ..Default::default()
        };
        let json = serde_json::to_value(&campaign).unwrap();
        assert_eq!(
            serde_json::from_value::<Campaign>(json.clone())
                .unwrap()
                .owner,
            Some(42)
        );
        let mut legacy = json;
        legacy.as_object_mut().unwrap().remove("owner");
        assert_eq!(
            serde_json::from_value::<Campaign>(legacy).unwrap().owner,
            None
        );
    }
}

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
    /// Keep debris cleanup working after a server restart.
    #[serde(default)]
    pub debris: bool,
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
    pub deployed_part_controls: Vec<(String, bool, bool)>,
    #[serde(default)]
    pub crew: Vec<(usize, String)>,
    #[serde(default)]
    pub docked: Vec<(usize, usize)>,
    /// A Kerbal on EVA (their name) or a planted flag (who planted it).
    #[serde(default)]
    pub eva: Option<String>,
    /// Stable registry id of a supporting hatch vessel, and its saved craft part index.
    #[serde(default)]
    pub eva_attachment: Option<(u64, usize)>,
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
    /// Invitation credential, independent of the private account login code.
    #[serde(default)]
    pub share_code: Option<String>,
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
            share_code: None,
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
    #[serde(default)]
    pub timelines: std::collections::BTreeMap<u64, ose_sim::timelines::TimelineClock>,
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
                debris: u.world.get::<ose_sim::vessels::Debris>(e).is_some(),
                body: u.body_id(body)?.to_string(),
                elements: orbit.elements,
                flight: u.powered(e).cloned(),
                dormant: u.dormant(e).cloned(),
                science: u.science_carried(e),
                experiments: u.running_experiments(e),
                craft: u.current_craft(e),
                deployed_part_controls: u.deployed_part_controls(e),
                crew: u.crew(e),
                docked: u.docked_links(e),
                eva: u.eva_kerbal(e).map(|k| k.name.clone()),
                eva_attachment: u.eva_attachment(e).and_then(|a| {
                    let id = vessels.iter().find(|v| v.0 == a.vessel)?.1;
                    // current_craft compacts lost parts on save.
                    let st = u.structure(a.vessel)?;
                    let part = st.rigid.present.iter().take(a.part).filter(|p| **p).count();
                    Some((id, part))
                }),
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
        timelines: u.timelines.clone(),
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
    u.timelines = save.timelines.clone();
    u.set_science_credited(save.campaign.science.submitted.iter().map(|s| s.report.clone()));
    restore_vessels(u, &save.vessels, parts, resources)
}

pub fn restore_vessels(u: &mut Universe, vessels: &[SavedVessel], parts: &[ose_data::PartDef], resources: &[ose_data::ResourceDef]) -> Vec<VesselEntry> {
    let mut registry = Vec::new();
    for v in vessels {
        // Legacy saves did not persist the marker; detached point masses used this name.
        let debris = v.debris || (v.craft.is_none() && v.name.ends_with(" debris"));
        if debris && v.flight.as_ref().or(v.dormant.as_ref()).is_some_and(|p| p.crashed || p.landed) {
            continue;
        }
        let Some(body) = u.body_entity(&v.body) else {
            tracing::warn!(vessel = v.id, body = %v.body, "saved vessel's body is unknown; dropped");
            continue;
        };
        let e = u.spawn_vessel_with_elements(&v.name, body, v.elements);
        if debris {
            u.world.entity_mut(e).insert(ose_sim::vessels::Debris);
        }
        if let Some(owner) = v.owner { u.assign_timeline(e, owner); }
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
        if let Some(craft) = v.craft.as_ref().filter(|c| !c.parts.is_empty()) {
            u.set_craft(e, craft.clone());
            u.set_crew(e, v.crew.clone());
            u.set_structure(
                e,
                ose_sim::structure::Structure::build(craft, parts, resources),
            );
            u.restore_deployed_part_controls(e, &v.deployed_part_controls);
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
    for v in vessels {
        if let Some((target, part)) = v.eva_attachment {
            let e = registry.iter().find(|r| r.1 == v.id).map(|r| r.0);
            let parent = registry.iter().find(|r| r.1 == target).map(|r| r.0);
            if let (Some(e), Some(parent)) = (e, parent) { u.restore_eva_attachment(e, parent, part); }
        }
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
    fn debris_marker_survives_save_and_surface_debris_is_dropped_on_restore() {
        let bodies: Vec<ose_data::CelestialBody> = serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut u = Universe::with_bodies(&bodies);
        let body = u.body_entity("Kerbin").unwrap();
        let e = u.spawn_vessel_in_circular_orbit("stage debris", body, 100_000.0);
        u.world.entity_mut(e).insert(ose_sim::vessels::Debris);
        let (r, v) = u.vessel(e).unwrap().2.state_at(u.ut());
        let p = ose_sim::flight::Powered::new(ose_sim::flight::FlightSpec {
            stages: vec![], drag_area_m2: 1.0, drag_cd: 0.2,
        }, r, v, false);
        u.set_powered(e, p.clone());
        let file = capture(&u, &[(e, 7, Some(1))], &Campaign::default(), &[]);
        let mut file: SaveFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        let mut restored = Universe::with_bodies(&bodies);
        let entries = restore(&mut restored, &file, &[], &[]);
        assert_eq!(entries.len(), 1);
        assert!(restored.world.get::<ose_sim::vessels::Debris>(entries[0].0).is_some());
        file.vessels[0].flight.as_mut().unwrap().landed = true;
        let mut restored = Universe::with_bodies(&bodies);
        assert!(restore(&mut restored, &file, &[], &[]).is_empty());
        // A pre-change save has no debris field; recover its legacy point-mass marker too.
        let mut legacy = serde_json::to_value(&file).unwrap();
        legacy["vessels"][0].as_object_mut().unwrap().remove("debris");
        let legacy: SaveFile = serde_json::from_value(legacy).unwrap();
        assert!(restore(&mut restored, &legacy, &[], &[]).is_empty());
        // Ordinary landed craft still restore.
        file.vessels[0].debris = false;
        file.vessels[0].name = "lander".into();
        assert_eq!(restore(&mut restored, &file, &[], &[]).len(), 1);
    }

    #[test]
    fn deployment_and_light_switch_survive_json_save_restore() {
        let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let defs = serde_json::from_str::<Vec<ose_data::PartDef>>(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res = serde_json::from_str::<Vec<ose_data::ResourceDef>>(include_str!("../../../assets/own/resources.json")).unwrap();
        let crafts = serde_json::from_str::<Vec<ose_data::craft::Craft>>(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let mut craft = crafts.iter().find(|c| c.name == "Kerbal K2").unwrap().clone();
        for id in ["antenna_0v_16", "landingleg_0v", "light_0v_spot"] {
            let mut part = craft.parts[0].clone();
            part.guid = format!("saved-{id}"); part.part_id = id.into(); part.parent = Some(0);
            part.resources.clear(); craft.parts.push(part);
        }
        let mut u = Universe::with_bodies(&bodies);
        let e = u.spawn_vessel_in_circular_orbit("saved controls", u.body_entity("Kerbin").unwrap(), 100_000.);
        let (r, v) = u.vessel(e).unwrap().2.state_at(u.ut());
        let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
        u.set_dormant(e, ose_sim::flight::Powered::new(ose_sim::flight::FlightSpec::from_report(&report, &[], &[], 1.), r, v, false));
        u.set_craft(e, craft.clone());
        u.set_structure(e, ose_sim::structure::Structure::build(&craft, &defs, &res));
        assert!(u.deploy_panels(e, true).unwrap() > 0);
        assert!(u.deploy_antennas(e, true).unwrap() > 0);
        assert!(u.deploy_gear(e, true).unwrap() > 0);
        assert_eq!(u.action_group(e, ose_sim::ActionGroup::Lights, Some(true)), Some(true));
        let controls = u.deployed_part_controls(e);
        let saved = capture(&u, &[(e, 42, Some(1))], &Campaign::default(), &[]);
        let saved: SaveFile = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let mut restored = Universe::with_bodies(&bodies);
        let entry = restore(&mut restored, &saved, &defs, &res)[0];
        assert_eq!(restored.deployed_part_controls(entry.0), controls);
        assert!(restored.dormant(entry.0).unwrap().groups.lights);
        assert!(restored.panels_deployed(entry.0));
        assert_eq!(restored.deploy_panels(entry.0, true), Some(0));
        assert_eq!(restored.deploy_antennas(entry.0, true), Some(0));
        assert_eq!(restored.deploy_gear(entry.0, true), Some(0));
    }

    #[test]
    fn an_eva_handhold_survives_save_restore_with_new_entity_ids() {
        let bodies = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let defs = serde_json::from_str::<Vec<ose_data::PartDef>>(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res = serde_json::from_str::<Vec<ose_data::ResourceDef>>(include_str!("../../../assets/own/resources.json")).unwrap();
        let crafts = serde_json::from_str::<Vec<ose_data::craft::Craft>>(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let craft = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut u = Universe::with_bodies(&bodies);
        let e = u.spawn_vessel_in_circular_orbit("EVA pod", u.body_entity("Kerbin").unwrap(), 100_000.0);
        let (r, v) = u.vessel(e).unwrap().2.state_at(u.ut());
        let report = ose_data::staging::delta_v_by_stage(craft, &defs, &res);
        u.set_powered(e, ose_sim::flight::Powered::new(ose_sim::flight::FlightSpec::from_report(&report, &[], &[], 1.0), r, v, false));
        u.set_craft(e, craft.clone());
        u.set_structure(e, ose_sim::structure::Structure::build(craft, &defs, &res));
        let jeb = u.go_eva(e, None).unwrap();
        // Put the Kerbal first so restoration must resolve a forward vessel reference.
        let saved = capture(&u, &[(jeb, 71, Some(1)), (e, 42, Some(1))], &Campaign::default(), &[]);
        let saved: SaveFile = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(saved.vessels[0].eva_attachment, Some((42, 0)));
        let mut restored = Universe::with_bodies(&bodies);
        let entries = restore(&mut restored, &saved, &defs, &res);
        let jeb = entries.iter().find(|v| v.1 == 71).unwrap().0;
        let pod = entries.iter().find(|v| v.1 == 42).unwrap().0;
        assert_eq!(restored.eva_attachment(jeb).unwrap().vessel, pod);
        for _ in 0..100 { restored.step(); }
        assert!(restored.eva_attachment(jeb).is_some());
        restored.board(jeb, pod).unwrap();
    }

    #[test]
    fn part_settings_and_control_point_survive_json_save_restore() {
        use ose_data::flight_geometry::PartControl;
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let craft = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let mut u = Universe::with_bodies(&bodies);
        let e = u.spawn_vessel_in_circular_orbit(
            "configured K1",
            u.body_entity("Kerbin").unwrap(),
            100_000.,
        );
        let (r, v) = u.vessel(e).unwrap().2.state_at(u.ut());
        let report = ose_data::staging::delta_v_by_stage(craft, &defs, &res);
        u.set_powered(
            e,
            ose_sim::flight::Powered::new(
                ose_sim::flight::FlightSpec::from_report(&report, &[], &[], 1.),
                r,
                v,
                false,
            ),
        );
        u.set_craft(e, craft.clone());
        u.set_structure(e, ose_sim::structure::Structure::build(craft, &defs, &res));
        let command = (0..craft.parts.len())
            .find(|i| u.structure(e).unwrap().can_control_from(*i))
            .unwrap() as u16;
        let settings = PartControl {
            wheel_authority: 0.25,
            gimbal_limiter: 0.4,
            surface_invert: true,
            rcs_axes: [false, true, true, false, true, true],
            ..Default::default()
        };
        assert!(u.configure_part(e, command, settings));
        assert!(!u.configure_part(e, u16::MAX, settings));
        assert!(!u.configure_part(
            e,
            command,
            PartControl {
                wheel_authority: f64::NAN,
                ..settings
            }
        ));
        let before = u.powered(e).unwrap().clone();
        assert!(u.control_from(e, command));
        assert!(u.set_precision(e, true));
        assert!(u.set_raw_rotation(e, ose_math::DVec3::new(1., -1., 0.5)));
        assert_eq!(u.powered(e).unwrap().r, before.r);
        assert_eq!(u.powered(e).unwrap().v, before.v);
        let saved = capture(&u, &[(e, 42, Some(1))], &Campaign::default(), &[]);
        let saved: SaveFile =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let mut restored = Universe::with_bodies(&bodies);
        let entry = restore(&mut restored, &saved, &defs, &res)[0];
        let p = restored.powered(entry.0).unwrap();
        assert_eq!(p.part_control(command as usize), settings);
        assert_eq!(p.control_point, Some(command));
        assert!(p.rcs.precision);
        assert_eq!(
            p.raw_rotation_input,
            Some(ose_math::DVec3::new(1., -1., 0.5))
        );
        assert_eq!(p.control_rotation, u.powered(e).unwrap().control_rotation);
    }

    #[test]
    fn empty_saved_craft_keeps_flight_state_without_building_an_empty_structure() {
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut u = Universe::with_bodies(&bodies);
        let body = u.body_entity("Kerbin").unwrap();
        let e = u.spawn_vessel_in_circular_orbit("legacy probe", body, 100_000.0);
        let (_, _, orbit) = u.vessel(e).unwrap();
        let (r, v) = orbit.state_at(u.ut());
        u.set_powered(
            e,
            ose_sim::flight::Powered::new(
                ose_sim::flight::FlightSpec {
                    stages: vec![],
                    drag_area_m2: 0.0,
                    drag_cd: 0.0,
                },
                r,
                v,
                false,
            ),
        );
        u.set_craft(
            e,
            ose_data::craft::Craft {
                name: "legacy probe".into(),
                description: String::new(),
                parts: vec![],
                stages: vec![],
            },
        );
        let file = capture(&u, &[(e, 42, Some(1))], &Campaign::default(), &[]);
        let mut restored = Universe::with_bodies(&bodies);
        let entries = restore(&mut restored, &file, &[], &[]);
        assert_eq!(entries.len(), 1);
        assert!(restored.powered(entries[0].0).is_some());
        assert!(restored.structure(entries[0].0).is_none());
    }

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

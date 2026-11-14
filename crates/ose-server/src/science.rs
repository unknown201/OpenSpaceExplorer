//! The campaign's science, tech tree and missions on the server (rules in `docs/reference-notes/science.md`
//! and `missions.md`). One shared agency: every member's reports, unlocks and missions count for all.

use ose_data::missions::{AgencyFacts, MissionDef, MissionProgress};
use ose_data::science::{ResearchReport, TechNode};
use ose_net::{AgencyEvent, DifficultyValue, MissionInfo, ReportInfo, ServerMessage, SessionMode};
use serde::{Deserialize, Serialize};

use crate::save::Campaign;
use crate::Shared;

/// Static definitions.
pub struct ScienceData {
    pub tech: Vec<TechNode>,
    pub missions: Vec<MissionDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmittedReport {
    pub report: ResearchReport,
    pub by: Option<u64>,
    pub tick: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedMission {
    pub id: String,
    pub progress: MissionProgress,
    /// When the current stage began (its events count from here).
    pub stage_started_tick: u64,
    /// Taken on at Mission Control (missions only progress once accepted; older saves were all accepted).
    #[serde(default = "yes")]
    pub accepted: bool,
    /// Complete and handed in at Mission Control: its science reward is paid.
    #[serde(default)]
    pub turned_in: bool,
}

fn yes() -> bool {
    true
}

/// The agency's science progress, saved with the campaign.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScienceProgress {
    pub submitted: Vec<SubmittedReport>,
    pub unlocked: Vec<String>,
    pub missions: Vec<SavedMission>,
    /// Bumped on every change so connected players get a fresh `ScienceState`.
    #[serde(default)]
    pub rev: u64,
}

pub fn report_info(r: &ResearchReport) -> ReportInfo {
    ReportInfo {
        key: r.key(),
        experiment: r.experiment.clone(),
        sample: r.sample,
        location: r.location.clone(),
        value: r.value,
    }
}

fn option(c: &Campaign, id: &str) -> f64 {
    c.difficulty
        .iter()
        .find(|d| d.id == id)
        .and_then(|d| match d.value {
            DifficultyValue::Number(x) => Some(x),
            _ => None,
        })
        .unwrap_or(1.0)
}

impl ScienceData {
    pub fn node(&self, id: &str) -> Option<&TechNode> {
        self.tech.iter().find(|n| n.id == id)
    }

    /// Unlocked nodes, the free starting ones included.
    pub fn unlocked<'a>(&'a self, p: &'a ScienceProgress) -> Vec<&'a TechNode> {
        self.tech
            .iter()
            .filter(|n| n.always_unlocked() || p.unlocked.contains(&n.id))
            .collect()
    }

    /// Points earned: submitted reports rounded, times `ScienceRewards`, plus passed mission stages' rewards
    /// times `MissionRewards` (`UpdateSciencePointCapacity`).
    pub fn capacity(&self, c: &Campaign) -> u32 {
        let p = &c.science;
        let reports: f64 = p
            .submitted
            .iter()
            .map(|r| r.report.value.round())
            .sum::<f64>()
            * option(c, "ScienceRewards");
        // missions pay when turned in (`UpdateSciencePointCapacity`: `missionSaveData.TurnedIn`)
        let missions: f64 = p
            .missions
            .iter()
            .filter(|m| m.turned_in)
            .filter_map(|m| {
                let def = self.missions.iter().find(|d| d.id == m.id)?;
                Some(def.science_reward())
            })
            .sum::<f64>()
            * option(c, "MissionRewards");
        (reports + missions).max(0.0) as u32
    }

    pub fn allocated(&self, p: &ScienceProgress) -> u32 {
        self.unlocked(p).iter().map(|n| n.required_science).sum()
    }

    /// Whether a mission is offered now: active from the start, or activated by a mission the agency has
    /// completed (the original's `ActivateMissionAction` chain).
    pub fn available(&self, def: &MissionDef, p: &ScienceProgress) -> bool {
        if !def.playable() {
            return false;
        }
        def.active_at_start
            || self.missions.iter().any(|prev| {
                prev.activates.iter().any(|a| a == &def.id)
                    && p.missions
                        .iter()
                        .any(|m| m.id == prev.id && m.progress.complete)
            })
    }

    pub fn missions_info(&self, p: &ScienceProgress) -> Vec<MissionInfo> {
        self.missions
            .iter()
            // offered now, or already taken on (an older save may hold missions offered before the chain)
            .filter(|d| {
                self.available(d, p) || p.missions.iter().any(|m| m.id == d.id && m.accepted)
            })
            .map(|d| {
                let prog = p
                    .missions
                    .iter()
                    .find(|m| m.id == d.id)
                    .map(|m| m.progress.clone())
                    .unwrap_or_default();
                let saved = p.missions.iter().find(|m| m.id == d.id);
                MissionInfo {
                    accepted: saved.map(|m| m.accepted).unwrap_or(d.active_at_start),
                    turned_in: saved.is_some_and(|m| m.turned_in),
                    id: d.id.clone(),
                    name_key: d.name_key.clone(),
                    description_key: d.description_key.clone(),
                    primary: d.primary,
                    granter: d.granter.clone(),
                    stage: prog.stage as u32,
                    stages: d.stages.len() as u32,
                    objective_key: if prog.complete {
                        String::new()
                    } else {
                        d.stages
                            .get(prog.stage)
                            .map(|s| s.objective_key.clone())
                            .unwrap_or_default()
                    },
                    complete: prog.complete,
                    science_reward: d.science_reward(),
                }
            })
            .collect()
    }

    /// Take on an offered mission.
    pub fn accept(&self, c: &mut Campaign, id: &str, now: u64) -> Result<(), String> {
        let def = self
            .missions
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("no mission {id}"))?;
        if !self.available(def, &c.science) {
            return Err("not offered yet".into());
        }
        match c.science.missions.iter_mut().find(|m| m.id == id) {
            Some(m) if m.accepted => return Err("already accepted".into()),
            Some(m) => {
                m.accepted = true;
                m.stage_started_tick = now;
            }
            None => c.science.missions.push(SavedMission {
                id: id.to_string(),
                progress: MissionProgress::default(),
                stage_started_tick: now,
                accepted: true,
                turned_in: false,
            }),
        }
        c.science.rev += 1;
        Ok(())
    }

    /// Hand in a completed mission for its science reward.
    pub fn turn_in(&self, c: &mut Campaign, id: &str) -> Result<f64, String> {
        let def = self
            .missions
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("no mission {id}"))?;
        let Some(m) = c
            .science
            .missions
            .iter_mut()
            .find(|m| m.id == id && m.accepted)
        else {
            // the first mission is taken on from the start, before it is tracked
            return Err(if def.active_at_start {
                "not complete yet"
            } else {
                "not accepted"
            }
            .into());
        };
        if !m.progress.complete {
            return Err("not complete yet".into());
        }
        if m.turned_in {
            return Err("already turned in".into());
        }
        m.turned_in = true;
        c.science.rev += 1;
        Ok(def.science_reward() * option(c, "MissionRewards"))
    }

    pub fn state(&self, c: &Campaign) -> ServerMessage {
        ServerMessage::ScienceState {
            tech_tree: c.mode == SessionMode::Exploration,
            capacity: self.capacity(c),
            allocated: self.allocated(&c.science),
            unlocked: self
                .unlocked(&c.science)
                .iter()
                .map(|n| n.id.clone())
                .collect(),
            submitted: c
                .science
                .submitted
                .iter()
                .map(|r| report_info(&r.report))
                .collect(),
            missions: self.missions_info(&c.science),
        }
    }

    /// Research a node: every prerequisite unlocked and enough free points.
    pub fn unlock(&self, c: &mut Campaign, id: &str) -> Result<(), String> {
        if c.mode != SessionMode::Exploration {
            return Err("sandbox has everything unlocked".into());
        }
        let node = self.node(id).ok_or_else(|| format!("no tech node {id}"))?;
        let have = self.unlocked(&c.science);
        if have.iter().any(|n| n.id == id) {
            return Err("already unlocked".into());
        }
        if let Some(missing) = node
            .requires
            .iter()
            .find(|r| !have.iter().any(|n| &n.id == *r))
        {
            return Err(format!("needs {missing} first"));
        }
        let free = self.capacity(c).saturating_sub(self.allocated(&c.science));
        if node.required_science > free {
            return Err(format!(
                "needs {} science points, {free} free",
                node.required_science
            ));
        }
        c.science.unlocked.push(id.to_string());
        c.science.rev += 1;
        Ok(())
    }

    /// Parts of a craft the campaign has not researched (always empty in sandbox): every part no researched
    /// node unlocks, parts no node unlocks at all included (the colony trunks, scanners and service bays the
    /// original's tree leaves out are not in its campaign's VAB either).
    pub fn locked_parts(&self, c: &Campaign, craft: &ose_data::craft::Craft) -> Vec<String> {
        if c.mode != SessionMode::Exploration {
            return Vec::new();
        }
        let have = self.unlocked(&c.science);
        let mut out: Vec<String> = craft
            .parts
            .iter()
            .map(|p| p.part_id.clone())
            .filter(|id| !have.iter().any(|n| n.parts.contains(id)))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Submit reports for the agency: a report it already has scores nothing (`GetPotentialReportValueBase`).
/// Returns them with the value each actually earned, and the total.
pub fn submit(
    c: &mut Campaign,
    reports: Vec<ResearchReport>,
    by: u64,
    tick: u64,
) -> (Vec<ReportInfo>, f64) {
    let mut out = Vec::new();
    let mut earned = 0.0;
    let mult = option(c, "ScienceRewards");
    for r in reports {
        let mut info = report_info(&r);
        if c.science
            .submitted
            .iter()
            .any(|s| s.report.key() == r.key())
        {
            info.value = 0.0;
        } else {
            earned += r.value.round() * mult;
            c.science.submitted.push(SubmittedReport {
                report: r,
                by: Some(by),
                tick,
            });
        }
        out.push(info);
    }
    if earned > 0.0 {
        c.science.rev += 1;
    }
    (out, earned)
}

/// Tell everyone else playing this campaign what `by` (or the agency, `None`) just did for it. The one who did
/// it already has their own answer; the `ScienceState` with the change reaches everyone on its own.
pub fn tell_agency(s: &Shared, by: Option<u64>, event: AgencyEvent) {
    let name = by.map(|p| crate::display_name_for(s, p));
    // the players connected to this campaign's room
    let players: Vec<u64> = s
        .warp_requests
        .lock()
        .expect("warp lock")
        .keys()
        .copied()
        .filter(|&p| Some(p) != by)
        .collect();
    let mut q = s.pending.lock().expect("pending lock");
    for p in players {
        q.push((
            p,
            ServerMessage::AgencyNews {
                by: name.clone(),
                event: event.clone(),
            },
        ));
    }
}

/// Whether `player` is a member of the campaign: only members spend its science and take on its missions.
pub fn member(c: &Campaign, player: u64) -> bool {
    c.members.iter().any(|m| m.player_id == player)
}

/// Hand finished and interrupted experiments to their vessel owners' connections.
pub fn deliver_experiment_events(s: &Shared) {
    let ev = s
        .universe
        .lock()
        .expect("sim lock")
        .take_experiment_events();
    if ev.finished.is_empty() && ev.interrupted.is_empty() {
        return;
    }
    let vessels = s.vessels.lock().expect("vessels lock").clone();
    let owner_of = |e| {
        vessels
            .iter()
            .find(|v| v.0 == e)
            .and_then(|v| v.2.map(|o| (v.1, o)))
    };
    let mut pending = s.pending.lock().expect("pending lock");
    for (e, run, reports) in ev.finished {
        if let Some((vessel, owner)) = owner_of(e) {
            pending.push((
                owner,
                ServerMessage::ExperimentFinished {
                    vessel,
                    part: run.part as u16,
                    experiment: run.experiment,
                    reports: reports.iter().map(report_info).collect(),
                    interrupted: false,
                },
            ));
        }
    }
    for (e, run) in ev.interrupted {
        if let Some((vessel, owner)) = owner_of(e) {
            pending.push((
                owner,
                ServerMessage::ExperimentFinished {
                    vessel,
                    part: run.part as u16,
                    experiment: run.experiment,
                    reports: vec![],
                    interrupted: true,
                },
            ));
        }
    }
    // a player who is not connected does not collect them forever
    let n = pending.len();
    if n > 256 {
        pending.drain(..n - 256);
    }
}

/// Advance every offered mission against the agency's vessels; true when anything changed.
pub fn evaluate_missions(s: &Shared) -> bool {
    let u = s.universe.lock().expect("sim lock");
    let vessels = s.vessels.lock().expect("vessels lock").clone();
    let mut c = s.campaign.lock().expect("campaign lock");
    let members: Vec<u64> = c.members.iter().map(|m| m.player_id).collect();
    let now = u.tick().0;
    let last = u.last_destroyed();
    let agency = AgencyFacts {
        scored_reports: c.science.submitted.iter().map(|r| r.report.key()).collect(),
        last_destroyed_max_ground_altitude_m: last.max_ground_altitude_m,
        last_destroyed_hit: last.body,
    };
    let mut changed = false;
    let mut completed = Vec::new();
    for def in &s.science.missions {
        if !s.science.available(def, &c.science) {
            continue;
        }
        let idx = match c.science.missions.iter().position(|m| m.id == def.id) {
            Some(i) => i,
            // the campaign starts with the first mission already taken on (`state: Active`)
            None if def.active_at_start => {
                c.science.missions.push(SavedMission {
                    id: def.id.clone(),
                    progress: MissionProgress::default(),
                    stage_started_tick: now,
                    accepted: true,
                    turned_in: false,
                });
                changed = true;
                c.science.missions.len() - 1
            }
            None => continue,
        };
        if !c.science.missions[idx].accepted || c.science.missions[idx].progress.complete {
            continue;
        }
        let since = c.science.missions[idx].stage_started_tick;
        let facts: Vec<(u64, ose_data::missions::VesselFacts)> = vessels
            .iter()
            .filter(|(_, _, owner)| owner.is_some_and(|o| members.contains(&o)))
            .filter(|(e, _, _)| u.world.get::<ose_sim::vessels::Debris>(*e).is_none())
            .filter_map(|&(e, id, _)| Some((id, u.mission_facts(e, since)?)))
            .collect();
        if facts.is_empty() && agency.last_destroyed_max_ground_altitude_m <= 0.0 {
            continue;
        }
        let m = &mut c.science.missions[idx];
        let (_, moved) = m.progress.advance(def, &facts, &agency);
        if moved {
            m.stage_started_tick = now;
            changed = true;
            if m.progress.complete {
                completed.push(def.id.clone());
            }
            tracing::info!(mission = %def.id, stage = m.progress.stage, complete = m.progress.complete, "mission advanced");
        }
    }
    if changed {
        c.science.rev += 1;
    }
    drop((c, vessels, u));
    for mission in completed {
        tell_agency(s, None, AgencyEvent::MissionCompleted { mission });
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> ScienceData {
        ScienceData {
            tech: serde_json::from_str(include_str!("../../../assets/own/tech-tree.json")).unwrap(),
            missions: serde_json::from_str(include_str!("../../../assets/own/missions.json"))
                .unwrap(),
        }
    }

    fn report(key_loc: &str, value: f64) -> ResearchReport {
        ResearchReport {
            experiment: "CrewObservation".into(),
            sample: false,
            location: key_loc.into(),
            value,
        }
    }

    #[test]
    fn reports_score_once_and_buy_tech() {
        let d = data();
        let mut c = Campaign::default();
        assert_eq!(c.mode, SessionMode::Exploration);
        assert_eq!(d.capacity(&c), 0);
        let starting = d.unlocked(&c.science).len();
        assert!(starting > 0, "free starting nodes");
        let (r, earned) = submit(&mut c, vec![report("Kerbin_LowOrbit", 24.4)], 1, 10);
        assert_eq!((r[0].value, earned), (24.4, 24.0));
        let (r, earned) = submit(&mut c, vec![report("Kerbin_LowOrbit", 24.4)], 2, 20);
        assert_eq!(
            (r[0].value, earned),
            (0.0, 0.0),
            "the same report scores nothing twice"
        );
        // the cheapest node whose prerequisites are free
        let have = d.unlocked(&c.science);
        let next = d
            .tech
            .iter()
            .filter(|n| !have.iter().any(|h| h.id == n.id))
            .filter(|n| n.requires.iter().all(|r| have.iter().any(|h| &h.id == r)))
            .min_by_key(|n| n.required_science)
            .unwrap()
            .clone();
        if next.required_science > 24 {
            assert!(d.unlock(&mut c, &next.id).is_err(), "not enough points yet");
            submit(
                &mut c,
                vec![report("Mun_HighOrbit", next.required_science as f64)],
                1,
                30,
            );
        }
        assert!(d.unlock(&mut c, &next.id).is_ok());
        assert_eq!(d.unlock(&mut c, &next.id), Err("already unlocked".into()));
        assert_eq!(d.allocated(&c.science), next.required_science);
        // its parts are no longer locked
        let craft = ose_data::craft::Craft {
            colors: None,
            loose: Vec::new(),
            name: "x".into(),
            description: String::new(),
            parts: vec![ose_data::craft::CraftPart {
                shape: None,
                fairing: None,
                chute: None,
                control: None,
                symmetry: 0,
                actions: Vec::new(),
                flight: None,
                colors: None,
                guid: "0".into(),
                part_id: next.parts[0].clone(),
                parent: None,
                own_node: None,
                parent_node: None,
                local_position: ose_math::DVec3::ZERO,
                local_rotation: Default::default(),
                stage_index: 0,
                fuel_crossfeed: true,
                resources: vec![],
                fuel_line_target: None,
            }],
            stages: vec![],
        };
        assert!(d.locked_parts(&c, &craft).is_empty());
        // a part no node unlocks is not offered in a campaign either
        let mut stray = craft.clone();
        stray.parts[0].part_id = "servicebay_1v".into();
        assert_eq!(
            d.locked_parts(&c, &stray),
            vec!["servicebay_1v".to_string()]
        );
    }

    #[test]
    fn primaries_open_in_order() {
        let d = data();
        let mut c = Campaign::default();
        let open = |c: &Campaign| -> Vec<String> {
            d.missions_info(&c.science)
                .into_iter()
                .filter(|m| m.primary)
                .map(|m| m.id)
                .collect()
        };
        assert_eq!(open(&c), vec!["Main_Kerbin_01".to_string()]);
        // nothing else is offered until the chain reaches it
        assert_eq!(
            d.missions_info(&c.science)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["Main_Kerbin_01"]
        );
        c.science.missions.push(SavedMission {
            id: "Main_Kerbin_01".into(),
            progress: MissionProgress {
                stage: 3,
                complete: true,
                latched: vec![],
            },
            stage_started_tick: 0,
            accepted: true,
            turned_in: false,
        });
        assert!(open(&c).contains(&"Main_Kerbin_02".to_string()));
        // complete but not handed in: nothing paid yet
        assert_eq!(d.capacity(&c), 0);
        assert_eq!(d.turn_in(&mut c, "Main_Kerbin_01"), Ok(25.0));
        assert_eq!(d.capacity(&c), 25);
        assert!(d.turn_in(&mut c, "Main_Kerbin_01").is_err(), "only once");
        // the next one has to be accepted before it counts
        assert!(d.turn_in(&mut c, "Main_Kerbin_02").is_err());
        assert!(d.accept(&mut c, "Main_Kerbin_02", 10).is_ok());
        assert!(d.accept(&mut c, "Main_Kerbin_02", 10).is_err());
        assert!(
            d.accept(&mut c, "Main_Kerbin_03", 10).is_err(),
            "not offered yet"
        );
        // completing the second opens the third and its secondary
        c.science
            .missions
            .iter_mut()
            .find(|m| m.id == "Main_Kerbin_02")
            .unwrap()
            .progress
            .complete = true;
        let ids: Vec<String> = d
            .missions_info(&c.science)
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert!(ids.contains(&"Main_Kerbin_03".to_string()));
        assert!(ids.contains(&"KSP2Mission_Secondary_Kerbin_Buoyancy".to_string()));
        assert!(!ids.contains(&"Main_MunOrBust_04".to_string()));
    }
}

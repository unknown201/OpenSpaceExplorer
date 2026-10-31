//! Missions as the original defines them (`explorationmissions` and `missioncontrol` assets, see
//! `docs/reference-notes/missions.md`): ordered stages, each passed when its condition tree holds for one of
//! the agency's vessels, with science-point rewards.
//!
//! A condition is a tree of AND / OR / XOR / NOT sets over property checks (a vessel value against a number,
//! count or text) and events (a vessel landed, a report was taken). A property check with
//! `RequireCurrentValue = false` latches once met; events count from the moment their stage became current.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Equal,
    Greater,
    Lesser,
}

impl Op {
    pub fn from_name(s: &str) -> Op {
        match s {
            "GREATER" => Op::Greater,
            "LESSER" => Op::Lesser,
            _ => Op::Equal,
        }
    }

    fn num(self, a: f64, b: f64) -> bool {
        match self {
            Op::Equal => a == b,
            Op::Greater => a > b,
            Op::Lesser => a < b,
        }
    }
}

/// What a property check watches (the original's property watchers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Watch {
    /// The body whose sphere of influence the vessel is in (text).
    Soi,
    /// Altitude above the ground / sea level, m.
    GroundAltitude,
    SeaLevelAltitude,
    /// Highest altitude above the ground this flight, m.
    MaxGroundAltitude,
    PeriapsisAltitude,
    ApoapsisAltitude,
    OrbitSpeed,
    /// Vessel mass, t.
    Mass,
    CrewCount,
    WheelCount,
    /// Has a part with this id (input).
    HasPart,
    /// Has a part of this family (input, like "0010-Probe").
    HasPartInGroup,
    /// A part with this id has been activated (staged).
    PartActivated,
    /// Carries a report with this key (input).
    HasStoredReport,
    /// The agency has submitted a report with this key (input).
    HasScoredReport,
    /// The vessel's research location id (text, like "Kerbin_Landed_KerbinWater").
    ResearchLocation,
    /// The predicted trajectory enters this body's sphere of influence (input).
    EscapePredictedSoi,
    /// Has landed on this body (input).
    LandedOnBody,
    IsEva,
    /// The last destroyed vessel's highest altitude, and whether it hit this body (input).
    LastDestroyedMaxGroundAltitude,
    DestroyedByCollision,
    Other(String),
}

impl Watch {
    pub fn from_type_name(t: &str) -> Watch {
        match t {
            "VesselSOIPropertyWatcher" => Watch::Soi,
            "VesselGroundLevelAltitudePropertyWatcher" => Watch::GroundAltitude,
            "VesselSeaLevelAltitudePropertyWatcher" => Watch::SeaLevelAltitude,
            "MaxGroundAltitudeReached" => Watch::MaxGroundAltitude,
            "VehiclePeriapsisAltitudePropertyWatcher" => Watch::PeriapsisAltitude,
            "VehicleApoapsisAltitudePropertyWatcher" => Watch::ApoapsisAltitude,
            "VesselOrbitSpeedPropertyWatcher" => Watch::OrbitSpeed,
            "VesselMass" => Watch::Mass,
            "CrewCount" => Watch::CrewCount,
            "WheelCount" => Watch::WheelCount,
            "HasPart" => Watch::HasPart,
            "HasPartInGroup" => Watch::HasPartInGroup,
            "PartActivated" => Watch::PartActivated,
            "HasStoredResearchReport" => Watch::HasStoredReport,
            "HasScoredResearchReport" => Watch::HasScoredReport,
            "VesselResearchLocation" => Watch::ResearchLocation,
            "EscapeTrajectorySOIPredictedPropertyWatcher" => Watch::EscapePredictedSoi,
            "VesselHasLandedOnCBPropertyWatcher" => Watch::LandedOnBody,
            "IsActiveVesselEva" => Watch::IsEva,
            "LastDestroyedVesselMaxGroundAltitudeReached" => Watch::LastDestroyedMaxGroundAltitude,
            "IsDestroyedByCbCollision" => Watch::DestroyedByCollision,
            other => Watch::Other(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissionEvent {
    LandedOnGround,
    LandedOnWater,
    LandedAfterFlight,
    FlagPlanted,
    VesselDestroyed,
    ReportAcquired,
    Other(String),
}

impl MissionEvent {
    pub fn from_type_name(t: &str) -> MissionEvent {
        match t {
            "VesselLandedGroundAtRestMessage" => MissionEvent::LandedOnGround,
            "VesselLandedWaterAtRestMessage" => MissionEvent::LandedOnWater,
            "VesselLandedAfterFlight" => MissionEvent::LandedAfterFlight,
            "FlagPlantedMessage" => MissionEvent::FlagPlanted,
            "VesselDestroyedMessage" => MissionEvent::VesselDestroyed,
            "ResearchReportAcquiredMessage" => MissionEvent::ReportAcquired,
            other => MissionEvent::Other(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    OneOf(Vec<Condition>),
    Not(Vec<Condition>),
    Property {
        watch: Watch,
        op: Op,
        value: f64,
        count: i64,
        text: String,
        /// The `Inputstring` the watcher takes (a part id, a report key, a body).
        input: String,
        /// Whether the input-style watcher must be true (`TestWatchedBool`).
        expect: bool,
        /// Stays true once met (`RequireCurrentValue = false`).
        latch: bool,
    },
    Event(MissionEvent),
}

/// What a vessel looks like to the mission checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VesselFacts {
    pub soi: String,
    pub ground_altitude_m: f64,
    pub sea_level_altitude_m: f64,
    pub max_ground_altitude_m: f64,
    pub periapsis_altitude_m: f64,
    pub apoapsis_altitude_m: f64,
    pub orbit_speed_mps: f64,
    pub mass_t: f64,
    pub crew: i64,
    pub wheels: i64,
    pub parts: Vec<String>,
    pub part_families: Vec<String>,
    pub activated_parts: Vec<String>,
    pub stored_reports: Vec<String>,
    pub research_location: String,
    pub predicted_sois: Vec<String>,
    pub landed_on: Vec<String>,
    pub eva: bool,
    /// Events since the current stage started.
    pub events: Vec<MissionEvent>,
}

/// Agency-wide facts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgencyFacts {
    pub scored_reports: Vec<String>,
    pub last_destroyed_max_ground_altitude_m: f64,
    pub last_destroyed_hit: String,
}

impl Condition {
    /// Evaluate for one vessel; `latched` holds the latched property checks by their position in the tree.
    pub fn holds(
        &self,
        v: &VesselFacts,
        a: &AgencyFacts,
        latched: &mut Vec<bool>,
        at: &mut usize,
    ) -> bool {
        match self {
            Condition::All(c) => {
                let mut ok = true;
                for x in c {
                    ok &= x.holds(v, a, latched, at);
                }
                ok
            }
            Condition::Any(c) => {
                let mut ok = false;
                for x in c {
                    ok |= x.holds(v, a, latched, at);
                }
                ok
            }
            Condition::OneOf(c) => {
                let mut n = 0;
                for x in c {
                    n += usize::from(x.holds(v, a, latched, at));
                }
                n == 1
            }
            Condition::Not(c) => {
                let mut any = false;
                for x in c {
                    any |= x.holds(v, a, latched, at);
                }
                !any
            }
            Condition::Event(e) => v.events.contains(e),
            Condition::Property {
                watch,
                op,
                value,
                count,
                text,
                input,
                expect,
                latch,
            } => {
                let i = *at;
                *at += 1;
                if latched.len() <= i {
                    latched.resize(i + 1, false);
                }
                if *latch && latched[i] {
                    return true;
                }
                let has = |list: &[String], s: &str| list.iter().any(|x| x.eq_ignore_ascii_case(s));
                fn wanted<'a>(input: &'a str, t: &'a str) -> &'a str {
                    if input.is_empty() {
                        t
                    } else {
                        input
                    }
                }
                let met = match watch {
                    Watch::Soi => v.soi.eq_ignore_ascii_case(text),
                    Watch::GroundAltitude => op.num(v.ground_altitude_m, *value),
                    Watch::SeaLevelAltitude => op.num(v.sea_level_altitude_m, *value),
                    Watch::MaxGroundAltitude => op.num(v.max_ground_altitude_m, *value),
                    Watch::PeriapsisAltitude => op.num(v.periapsis_altitude_m, *value),
                    Watch::ApoapsisAltitude => op.num(v.apoapsis_altitude_m, *value),
                    Watch::OrbitSpeed => op.num(v.orbit_speed_mps, *value),
                    Watch::Mass => op.num(v.mass_t, *value),
                    Watch::CrewCount => op.num(v.crew as f64, *count as f64),
                    Watch::WheelCount => op.num(v.wheels as f64, *count as f64),
                    Watch::HasPart => has(&v.parts, wanted(input, text)) == *expect,
                    Watch::HasPartInGroup => has(&v.part_families, wanted(input, text)) == *expect,
                    Watch::PartActivated => has(&v.activated_parts, wanted(input, text)) == *expect,
                    Watch::HasStoredReport => {
                        has(&v.stored_reports, wanted(input, text)) == *expect
                    }
                    Watch::HasScoredReport => {
                        has(&a.scored_reports, wanted(input, text)) == *expect
                    }
                    Watch::ResearchLocation => v.research_location.eq_ignore_ascii_case(text),
                    Watch::EscapePredictedSoi => {
                        has(&v.predicted_sois, wanted(input, text)) == *expect
                    }
                    Watch::LandedOnBody => has(&v.landed_on, wanted(input, text)) == *expect,
                    Watch::IsEva => v.eva == (wanted(input, "true") == "true"),
                    Watch::LastDestroyedMaxGroundAltitude => {
                        op.num(a.last_destroyed_max_ground_altitude_m, *value)
                    }
                    Watch::DestroyedByCollision => {
                        a.last_destroyed_hit
                            .eq_ignore_ascii_case(wanted(input, text))
                            == *expect
                    }
                    Watch::Other(_) => false,
                };
                if met && *latch {
                    latched[i] = true;
                }
                met
            }
        }
    }

    /// Whether the tree uses only checks we can evaluate.
    pub fn supported(&self) -> bool {
        match self {
            Condition::All(c) | Condition::Any(c) | Condition::OneOf(c) | Condition::Not(c) => {
                c.iter().all(Condition::supported)
            }
            Condition::Event(e) => !matches!(e, MissionEvent::Other(_)),
            Condition::Property { watch, .. } => !matches!(watch, Watch::Other(_)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionStage {
    pub id: i64,
    pub objective_key: String,
    /// `None` for the closing stage, which passes as soon as it is reached.
    pub condition: Option<Condition>,
    pub science_reward: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionDef {
    pub id: String,
    pub name_key: String,
    pub description_key: String,
    pub primary: bool,
    /// Who offers it (`MissionGranterKey`); empty for missions the game starts itself.
    pub granter: String,
    /// Active from the start of a campaign (`state: Active`).
    pub active_at_start: bool,
    pub stages: Vec<MissionStage>,
}

impl MissionDef {
    pub fn science_reward(&self) -> f64 {
        self.stages.iter().map(|s| s.science_reward).sum()
    }

    /// Offered to players: an agency mission with a granter whose checks we can evaluate (the tutorial
    /// helpers and scripted activators are left out).
    pub fn playable(&self) -> bool {
        !self.granter.is_empty()
            && self
                .stages
                .iter()
                .all(|s| s.condition.as_ref().is_none_or(Condition::supported))
    }
}

/// Progress through one mission.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MissionProgress {
    pub stage: usize,
    pub complete: bool,
    /// Latched property checks of the current stage, per vessel id.
    #[serde(default)]
    pub latched: Vec<(u64, Vec<bool>)>,
}

impl MissionProgress {
    /// Pass every stage whose condition one of `vessels` meets, in order; returns the science earned.
    /// `vessels` are (vessel id, facts) and facts' events count from when the current stage began.
    pub fn advance(
        &mut self,
        def: &MissionDef,
        vessels: &[(u64, VesselFacts)],
        agency: &AgencyFacts,
    ) -> (f64, bool) {
        let mut earned = 0.0;
        let mut advanced = false;
        while !self.complete && self.stage < def.stages.len() {
            let st = &def.stages[self.stage];
            let passed = match &st.condition {
                None => true,
                Some(c) => vessels.iter().any(|(id, facts)| {
                    let slot = match self.latched.iter().position(|(v, _)| v == id) {
                        Some(i) => i,
                        None => {
                            self.latched.push((*id, Vec::new()));
                            self.latched.len() - 1
                        }
                    };
                    let mut at = 0;
                    c.holds(facts, agency, &mut self.latched[slot].1, &mut at)
                }),
            };
            if !passed {
                break;
            }
            earned += st.science_reward;
            advanced = true;
            self.stage += 1;
            self.latched.clear();
            if self.stage >= def.stages.len() {
                self.complete = true;
            }
        }
        (earned, advanced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(watch: Watch, op: Op, value: f64, latch: bool) -> Condition {
        Condition::Property {
            watch,
            op,
            value,
            count: 0,
            text: String::new(),
            input: String::new(),
            expect: true,
            latch,
        }
    }

    fn first_flight() -> MissionDef {
        // shaped like Main_Kerbin_01: above 10 km over Kerbin, then land after flying that high
        MissionDef {
            id: "Main_Kerbin_01".into(),
            name_key: String::new(),
            description_key: String::new(),
            primary: true,
            granter: "MissionGranterKSC".into(),
            active_at_start: true,
            stages: vec![
                MissionStage {
                    id: 0,
                    objective_key: String::new(),
                    condition: Some(Condition::All(vec![
                        Condition::Property {
                            watch: Watch::Soi,
                            op: Op::Equal,
                            value: 0.0,
                            count: 0,
                            text: "Kerbin".into(),
                            input: String::new(),
                            expect: true,
                            latch: false,
                        },
                        prop(Watch::GroundAltitude, Op::Greater, 10_000.0, false),
                    ])),
                    science_reward: 0.0,
                },
                MissionStage {
                    id: 17,
                    objective_key: String::new(),
                    condition: Some(Condition::All(vec![
                        prop(Watch::MaxGroundAltitude, Op::Greater, 10_000.0, true),
                        Condition::Event(MissionEvent::LandedAfterFlight),
                    ])),
                    science_reward: 25.0,
                },
                MissionStage {
                    id: 15,
                    objective_key: String::new(),
                    condition: None,
                    science_reward: 0.0,
                },
            ],
        }
    }

    #[test]
    fn stages_pass_in_order_and_pay_their_reward() {
        let def = first_flight();
        let mut p = MissionProgress::default();
        let agency = AgencyFacts::default();
        let mut v = VesselFacts {
            soi: "Kerbin".into(),
            ground_altitude_m: 500.0,
            ..Default::default()
        };
        assert_eq!(p.advance(&def, &[(1, v.clone())], &agency), (0.0, false));
        v.ground_altitude_m = 12_000.0;
        v.max_ground_altitude_m = 12_000.0;
        let (earned, moved) = p.advance(&def, &[(1, v.clone())], &agency);
        assert!(moved && earned == 0.0 && p.stage == 1);
        // back on the ground: the landing event finishes it, and the closing stage passes at once
        v.ground_altitude_m = 0.0;
        v.events = vec![MissionEvent::LandedAfterFlight];
        let (earned, _) = p.advance(&def, &[(1, v)], &agency);
        assert_eq!(earned, 25.0);
        assert!(p.complete);
    }

    #[test]
    fn a_latched_check_stays_met_and_a_current_one_does_not() {
        let c = Condition::All(vec![
            prop(Watch::GroundAltitude, Op::Greater, 100.0, true),
            prop(Watch::Mass, Op::Greater, 5.0, false),
        ]);
        let a = AgencyFacts::default();
        let mut latched = Vec::new();
        let high = VesselFacts {
            ground_altitude_m: 200.0,
            mass_t: 1.0,
            ..Default::default()
        };
        let mut at = 0;
        assert!(!c.holds(&high, &a, &mut latched, &mut at));
        let low_heavy = VesselFacts {
            ground_altitude_m: 0.0,
            mass_t: 10.0,
            ..Default::default()
        };
        let mut at = 0;
        assert!(
            c.holds(&low_heavy, &a, &mut latched, &mut at),
            "altitude latched earlier"
        );
    }

    /// Make `v` and `a` meet `c` (the first branch of an Any/OneOf; a Not is left to the defaults).
    fn satisfy(c: &Condition, v: &mut VesselFacts, a: &mut AgencyFacts) {
        let num = |op: Op, value: f64| match op {
            Op::Greater => value + 1.0,
            Op::Lesser => value - 1.0,
            Op::Equal => value,
        };
        match c {
            Condition::All(cs) => cs.iter().for_each(|c| satisfy(c, v, a)),
            Condition::Any(cs) | Condition::OneOf(cs) => {
                if let Some(c) = cs.first() {
                    satisfy(c, v, a);
                }
            }
            Condition::Not(_) => {}
            Condition::Event(e) => v.events.push(e.clone()),
            Condition::Property {
                watch,
                op,
                value,
                count,
                text,
                input,
                expect,
                ..
            } => {
                let wanted = if input.is_empty() { text } else { input }.clone();
                let put = |list: &mut Vec<String>| {
                    if *expect {
                        list.push(wanted.clone());
                    }
                };
                match watch {
                    Watch::Soi => v.soi = text.clone(),
                    Watch::GroundAltitude => v.ground_altitude_m = num(*op, *value),
                    Watch::SeaLevelAltitude => v.sea_level_altitude_m = num(*op, *value),
                    Watch::MaxGroundAltitude => v.max_ground_altitude_m = num(*op, *value),
                    Watch::PeriapsisAltitude => v.periapsis_altitude_m = num(*op, *value),
                    Watch::ApoapsisAltitude => v.apoapsis_altitude_m = num(*op, *value),
                    Watch::OrbitSpeed => v.orbit_speed_mps = num(*op, *value),
                    Watch::Mass => v.mass_t = num(*op, *value),
                    Watch::CrewCount => v.crew = num(*op, *count as f64) as i64,
                    Watch::WheelCount => v.wheels = num(*op, *count as f64) as i64,
                    Watch::HasPart => put(&mut v.parts),
                    Watch::HasPartInGroup => put(&mut v.part_families),
                    Watch::PartActivated => put(&mut v.activated_parts),
                    Watch::HasStoredReport => put(&mut v.stored_reports),
                    Watch::HasScoredReport => put(&mut a.scored_reports),
                    Watch::ResearchLocation => v.research_location = text.clone(),
                    Watch::EscapePredictedSoi => put(&mut v.predicted_sois),
                    Watch::LandedOnBody => put(&mut v.landed_on),
                    Watch::IsEva => {
                        v.eva = if input.is_empty() {
                            true
                        } else {
                            input == "true"
                        }
                    }
                    Watch::LastDestroyedMaxGroundAltitude => {
                        a.last_destroyed_max_ground_altitude_m = num(*op, *value)
                    }
                    Watch::DestroyedByCollision => {
                        if *expect {
                            a.last_destroyed_hit = wanted.clone();
                        }
                    }
                    Watch::Other(_) => {}
                }
            }
        }
    }

    #[test]
    fn every_stage_of_every_offered_mission_can_be_met() {
        let missions: Vec<MissionDef> =
            serde_json::from_str(include_str!("../../../assets/own/missions.json")).unwrap();
        let playable: Vec<&MissionDef> = missions.iter().filter(|m| m.playable()).collect();
        assert!(playable.len() >= 30, "{} playable", playable.len());
        for m in playable {
            let mut progress = MissionProgress::default();
            let mut earned = 0.0;
            for (k, stage) in m.stages.iter().enumerate() {
                assert_eq!(progress.stage, k, "{}", m.id);
                let mut v = VesselFacts::default();
                let mut a = AgencyFacts::default();
                if let Some(c) = &stage.condition {
                    satisfy(c, &mut v, &mut a);
                }
                let (got, moved) = progress.advance(m, &[(1, v)], &a);
                earned += got;
                assert!(
                    moved,
                    "{} stage {} ({}) cannot be met",
                    m.id, k, stage.objective_key
                );
                if progress.complete {
                    break;
                }
            }
            assert!(
                progress.complete,
                "{} stuck at stage {}",
                m.id, progress.stage
            );
            assert_eq!(earned, m.science_reward(), "{}", m.id);
        }
    }

    #[test]
    fn landing_objectives_count_a_splashdown_wherever_the_original_does() {
        // the return-home stages ask for VesselLandedAfterFlight, which the original sends for Landed and
        // Splashed alike; the water-only ones ask for VesselLandedWaterAtRestMessage
        let missions: Vec<MissionDef> =
            serde_json::from_str(include_str!("../../../assets/own/missions.json")).unwrap();
        let first = missions.iter().find(|m| m.id == "Main_Kerbin_01").unwrap();
        for down in [MissionEvent::LandedOnGround, MissionEvent::LandedOnWater] {
            let mut progress = MissionProgress {
                stage: 1,
                ..Default::default()
            };
            let v = VesselFacts {
                soi: "Kerbin".into(),
                max_ground_altitude_m: 12_000.0,
                events: vec![down.clone(), MissionEvent::LandedAfterFlight],
                ..Default::default()
            };
            progress.advance(first, &[(1, v)], &AgencyFacts::default());
            assert!(progress.complete, "{down:?}");
        }
    }
}

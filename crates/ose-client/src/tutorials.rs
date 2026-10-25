//! Kerbal Space Academy lessons (`data/tutorials`, the original's Training Center lessons): the groups and
//! lessons from assets/ui/tutorials.json (tools/export_tutorials.py), listed in the Training Center's Lessons
//! tab; a lesson plays PAIGE's messages stage by stage in the first-time hint box (`ftue.rs`). A stage moves
//! on when its condition holds (`eval`): Continue, or something done in the game (launch, stage, reach an
//! altitude, open the map, pick a part ...). Conditions the client cannot watch let Continue through.

use crate::ftue::Message;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Academy {
    pub groups: Vec<Group>,
    pub lessons: Vec<Lesson>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Group {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Lesson {
    pub id: String,
    pub group: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub stages: Vec<Stage>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Stage {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub cond: Option<Cond>,
}

/// A stage's condition as exported: an event, a watched property against a value, or all / any of several.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Cond {
    Event {
        e: String,
    },
    Prop {
        p: String,
        op: String,
        #[serde(default)]
        num: f64,
        #[serde(default)]
        str: String,
        #[serde(default)]
        int: i64,
    },
    All {
        and: Vec<Cond>,
    },
    Any {
        or: Vec<Cond>,
    },
}

/// What a lesson can watch: the events seen since the stage began and the state of the game now.
#[derive(Clone, Debug, Default)]
pub struct World {
    /// Event names as the original's messages (`StageActivatedMessage`, `MapViewEnteredMessage` ...),
    /// `GoToNextTutorialStep` for Continue.
    pub events: HashSet<String>,
    pub altitude_sea_m: Option<f64>,
    pub altitude_ground_m: Option<f64>,
    pub surface_speed_mps: Option<f64>,
    pub throttle: Option<f64>,
    /// Stage propellant left, percent.
    pub stage_fuel_pct: Option<f64>,
    pub apoapsis_m: Option<f64>,
    pub periapsis_m: Option<f64>,
    /// The original's SAS mode numbers (0 stability, 1 prograde, 2 retrograde ... 9 maneuver).
    pub sas_mode: Option<i64>,
    pub map: bool,
    /// The VAB's last part placed and the part in hand or selected.
    pub last_placed: Option<String>,
    pub last_selected: Option<String>,
}

fn compare(op: &str, have: f64, want: f64) -> bool {
    match op {
        "GREATER" => have > want,
        "LESSER" => have < want,
        _ => (have - want).abs() <= 1e-3 * want.abs().max(1.0),
    }
}

/// Whether `c` holds in `w`: `None` when it watches something the client cannot see.
pub fn eval(c: &Cond, w: &World) -> Option<bool> {
    match c {
        Cond::Event { e } => {
            const KNOWN: &[&str] = &[
                "GoToNextTutorialStep",
                "StageActivatedMessage",
                "MapViewEnteredMessage",
                "ManeuverCreatedMessage",
                "OrbitClickedMessage",
                "VesselLaunchedMessage",
                "LaunchSequenceInitiatedMessage",
                "VesselCreatedMessage",
                "VABLaunchButtonPressedMessage",
                "PartPlacedOnMainAssemblyMessage",
                "ParachuteDeployedMessage",
                "ParachuteSemiDeployedMessage",
                "TimeWarpToLocationInitializedMessage",
                "TimeWarpToLocationCompletedMessage",
            ];
            if w.events.contains(e) {
                Some(true)
            } else if KNOWN.contains(&e.as_str()) {
                Some(false)
            } else {
                None
            }
        }
        Cond::Prop {
            p,
            op,
            num,
            str,
            int,
        } => {
            let n = |v: Option<f64>| v.map(|v| compare(op, v, *num));
            match p.as_str() {
                "VesselSeaLevelAltitudePropertyWatcher" => n(w.altitude_sea_m),
                "VesselGroundLevelAltitudePropertyWatcher" => n(w.altitude_ground_m),
                "VesselSurfaceSpeedPropertyWatcher" => n(w.surface_speed_mps),
                "Throttle" => n(w.throttle),
                "StageFuelLevelPropertyWatcher" => n(w.stage_fuel_pct),
                "VehicleApoapsisAltitudePropertyWatcher" => n(w.apoapsis_m),
                "VehiclePeriapsisAltitudePropertyWatcher" => n(w.periapsis_m),
                "SASMode" => w.sas_mode.map(|m| m == *int),
                "MapEnabled" => Some(w.map),
                "OABLastPartPlacedPropertyWatcher" => {
                    Some(w.last_placed.as_deref() == Some(str.as_str()))
                }
                "OABLastPartSelectedPropertyWatcher" => {
                    Some(w.last_selected.as_deref() == Some(str.as_str()))
                }
                _ => None,
            }
        }
        Cond::All { and } => {
            let r: Vec<Option<bool>> = and.iter().map(|c| eval(c, w)).collect();
            if r.contains(&Some(false)) {
                Some(false)
            } else if r.iter().all(|x| *x == Some(true)) {
                Some(true)
            } else {
                None
            }
        }
        Cond::Any { or } => {
            let r: Vec<Option<bool>> = or.iter().map(|c| eval(c, w)).collect();
            if r.contains(&Some(true)) {
                Some(true)
            } else if r.iter().all(|x| *x == Some(false)) {
                Some(false)
            } else {
                None
            }
        }
    }
}

impl Stage {
    /// Whether the stage is over: its condition holds, or the player pressed Continue (`continued`) and the
    /// condition is none, Continue itself, or one the client cannot watch.
    pub fn done(&self, w: &World, continued: bool) -> bool {
        let Some(c) = &self.cond else {
            return continued;
        };
        let mut w2 = w.clone();
        if continued {
            w2.events.insert("GoToNextTutorialStep".into());
        }
        match eval(c, &w2) {
            Some(v) => v,
            None => continued,
        }
    }
}

pub fn parse(text: &str) -> Option<Academy> {
    serde_json::from_str(text).ok()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn loc_or<'a>(loc: &'a HashMap<String, String>, key: &str, fallback: &'a str) -> &'a str {
    loc.get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
}

impl Group {
    pub fn title<'a>(&'a self, loc: &'a HashMap<String, String>) -> &'a str {
        loc_or(
            loc,
            &format!("Tutorials/Titles/{}", self.title),
            &self.title,
        )
    }
}

impl Lesson {
    pub fn title<'a>(&'a self, loc: &'a HashMap<String, String>) -> &'a str {
        loc_or(
            loc,
            &format!("Tutorials/Lessons/Titles/{}/{}", self.group, self.name),
            &self.name,
        )
    }
    pub fn description<'a>(&'a self, loc: &'a HashMap<String, String>) -> &'a str {
        loc_or(
            loc,
            &format!("Tutorials/Lessons/Descriptions/{}", self.id),
            &self.description,
        )
    }
    /// Every message of the lesson, stage after stage.
    pub fn messages(&self) -> Vec<Message> {
        self.stages
            .iter()
            .flat_map(|s| s.messages.clone())
            .collect()
    }
}

/// The Lessons tab: each group's title with its lessons; `selected` is highlighted.
pub fn list_html(a: &Academy, loc: &HashMap<String, String>, selected: Option<&str>) -> String {
    let mut html = String::new();
    for g in &a.groups {
        html.push_str(&format!(
            "<li class=\"tl-group\">{}</li>",
            esc(g.title(loc))
        ));
        for l in a.lessons.iter().filter(|l| l.group == g.id) {
            html.push_str(&format!(
                "<li class=\"tl-lesson{sel}\" data-ose=\"lesson\" data-lesson=\"{id}\">{title}</li>",
                sel = if selected == Some(l.id.as_str()) { " selected" } else { "" },
                id = esc(&l.id),
                title = esc(l.title(loc)),
            ));
        }
    }
    html
}

/// The selected lesson's page: title, description, how many messages and the Start Lesson button.
pub fn info_html(l: &Lesson, loc: &HashMap<String, String>) -> String {
    let n: usize = l.stages.iter().map(|s| s.messages.len()).sum();
    format!(
        "<h3>{title}</h3><p>{desc}</p><p class=\"tl-meta\">{stages} steps \u{b7} {n} messages from PAIGE</p>\
         <button id=\"tl-start\" data-ose=\"lesson-start\" data-lesson=\"{id}\">{start}</button>\
         <p class=\"tl-meta\">PAIGE walks you through the lesson; try each step in the game as you go.</p>",
        title = esc(l.title(loc)),
        desc = esc(l.description(loc)),
        stages = l.stages.len(),
        id = esc(&l.id),
        start = esc(loc_or(loc, "Tutorials/Lessons/StartLesson", "Start Lesson")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lessons_list_under_their_groups_with_localized_titles() {
        let a = parse(
            r#"{"groups":[{"id":"Tutorial1","title":"Space is the Place","order":1}],
                "lessons":[{"id":"Tutorial 1.1","group":"Tutorial1","name":"Launch a Test Rocket","description":"d",
                  "stages":[{"messages":[{"key":"Tutorials/a","image":"PAIGE_emote_02_happy_0001"}]},
                            {"messages":[{"key":"Tutorials/b","image":""}]}]}]}"#,
        )
        .unwrap();
        let mut loc = HashMap::new();
        loc.insert(
            "Tutorials/Titles/Space is the Place".to_string(),
            "Space Is the Place".to_string(),
        );
        let html = list_html(&a, &loc, Some("Tutorial 1.1"));
        assert!(html.contains("Space Is the Place"));
        assert!(html.contains("tl-lesson selected"));
        assert!(html.contains("Launch a Test Rocket"));
        assert_eq!(a.lessons[0].messages().len(), 2);
        assert!(info_html(&a.lessons[0], &loc).contains("Start Lesson"));
    }

    #[test]
    fn stages_wait_for_what_they_watch() {
        let st: Stage = serde_json::from_str(
            r#"{"messages":[],"cond":{"or":[{"e":"VesselLaunchedMessage"},
                {"p":"VesselSeaLevelAltitudePropertyWatcher","op":"GREATER","num":200.0,"str":"","int":0}]}}"#,
        )
        .unwrap();
        let mut w = World {
            altitude_sea_m: Some(80.0),
            ..Default::default()
        };
        // Continue alone does not launch the rocket.
        assert!(!st.done(&w, true));
        w.altitude_sea_m = Some(250.0);
        assert!(st.done(&w, false));
        w.altitude_sea_m = Some(80.0);
        w.events.insert("VesselLaunchedMessage".into());
        assert!(st.done(&w, false));
        // Continue moves on, or anything Continue is one way through.
        let c: Stage =
            serde_json::from_str(r#"{"messages":[],"cond":{"e":"GoToNextTutorialStep"}}"#).unwrap();
        assert!(!c.done(&World::default(), false));
        assert!(c.done(&World::default(), true));
        // A camera heading the client does not watch lets Continue through.
        let cam: Stage = serde_json::from_str(
            r#"{"messages":[],"cond":{"and":[{"e":"CameraRotatedMessage"},
                {"p":"CameraGimbalHeadingPropertyWatcher","op":"GREATER","num":20.0}]}}"#,
        )
        .unwrap();
        let mut w = World::default();
        w.events.insert("CameraRotatedMessage".into());
        assert!(!cam.done(&w, false));
        assert!(cam.done(&w, true));
        // SAS prograde.
        let sas: Stage = serde_json::from_str(
            r#"{"messages":[],"cond":{"p":"SASMode","op":"EQUAL","num":0.0,"str":"","int":1}}"#,
        )
        .unwrap();
        let w = World {
            sas_mode: Some(1),
            ..Default::default()
        };
        assert!(sas.done(&w, false));
    }
}

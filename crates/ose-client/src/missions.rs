//! Mission Control: the original's missions (`assets/own/missions.json`, imported by the server thread) as the
//! original's mission log: tabs for primary and secondary missions on the left, and the selected mission's
//! briefing on the right (title, granter, description, objectives in order, science reward). Text comes from
//! the original's English localization.
//!
//! Mission progress is not in the protocol yet: missions marked active at the start of a campaign show as
//! active and the rest as not yet offered.

use crate::rnd::text;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Clone, Debug)]
pub struct Mission {
    pub id: String,
    #[serde(default)]
    pub name_key: String,
    #[serde(default)]
    pub description_key: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub granter: String,
    #[serde(default)]
    pub active_at_start: bool,
    #[serde(default)]
    pub stages: Vec<Stage>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Stage {
    #[serde(default)]
    pub objective_key: String,
    #[serde(default)]
    pub science_reward: f64,
}

pub fn all() -> Vec<Mission> {
    let mut m: Vec<Mission> =
        serde_json::from_str(include_str!("../../../assets/own/missions.json")).unwrap_or_default();
    // Active ones first, then by name, as the log lists them.
    m.sort_by(|a, b| {
        b.active_at_start
            .cmp(&a.active_at_start)
            .then(a.id.cmp(&b.id))
    });
    m
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Granter display name: the localization's granter name when present, else the id without its prefix.
fn granter(loc: &HashMap<String, String>, id: &str) -> String {
    let short = id
        .trim_start_matches("MIssionGranter")
        .trim_start_matches("MissionGranter");
    for key in [
        format!("Missions/MissionGranter/Name/{short}"),
        format!("Missions/MissionGranter/{short}"),
    ] {
        if let Some(v) = loc.get(&key) {
            return v.clone();
        }
    }
    short.to_string()
}

pub fn reward(m: &Mission) -> f64 {
    m.stages.iter().map(|s| s.science_reward.max(0.0)).sum()
}

/// The mission log for one tab (primary or secondary). With the server's progress, offered missions are
/// listed (active first, completed after); without it, every mission.
pub fn render_list(
    missions: &[Mission],
    loc: &HashMap<String, String>,
    primary: bool,
    selected: Option<&str>,
    progress: Option<&[ose_net::MissionInfo]>,
    completed: bool,
) -> String {
    let mut html = String::new();
    let turned_in =
        |m: &Mission| progress.is_some_and(|p| p.iter().any(|i| i.id == m.id && i.turned_in));
    // The Completed tab holds the missions handed in, primary and secondary; the others leave them out.
    let mut rows: Vec<&Mission> = missions
        .iter()
        .filter(|m| {
            if completed {
                turned_in(m)
            } else {
                m.primary == primary && !turned_in(m)
            }
        })
        .collect();
    if let Some(p) = progress {
        rows.retain(|m| p.iter().any(|i| i.id == m.id));
        rows.sort_by_key(|m| {
            p.iter()
                .find(|i| i.id == m.id)
                .map(|i| i.complete)
                .unwrap_or(false)
        });
    }
    for m in rows {
        let sel = if selected == Some(m.id.as_str()) {
            " selected"
        } else {
            ""
        };
        let info = progress.and_then(|p| p.iter().find(|i| i.id == m.id));
        let state = match info {
            Some(i) if i.turned_in => "complete",
            Some(i) if i.complete => "ready",
            Some(i) if !i.accepted => "offered",
            Some(_) => "active",
            None if m.active_at_start => "active",
            None => "offered-later",
        };
        html.push_str(&format!(
            "<li class=\"mc-mission {state}{sel}\" data-ose=\"mc-mission\" data-ose-mission=\"{id}\">\
             <span class=\"name\">{name}</span><span class=\"reward\">{reward:.0}</span></li>",
            id = esc(&m.id),
            name = esc(&text(loc, &m.name_key)),
            reward = reward(m),
        ));
    }
    if html.is_empty() {
        html.push_str("<li class=\"mc-empty\">no missions</li>");
    }
    html
}

/// The briefing panel of one mission, with the current objective marked when the server sent progress.
pub fn render_briefing(
    m: &Mission,
    loc: &HashMap<String, String>,
    info: Option<&ose_net::MissionInfo>,
) -> String {
    let mut objectives = String::new();
    for (k, s) in m
        .stages
        .iter()
        .filter(|s| !s.objective_key.is_empty())
        .enumerate()
    {
        let cls = match info {
            Some(i) if i.complete || (k as u32) < i.stage => " class=\"done\"",
            Some(i) if k as u32 == i.stage => " class=\"current\"",
            _ => "",
        };
        objectives.push_str(&format!(
            "<li{cls}>{}</li>",
            esc(&text(loc, &s.objective_key))
        ));
    }
    format!(
        "<div class=\"mc-kind\">{kind}</div><h3>{name}</h3>\
         <div class=\"mc-granter\">{granter_head} <b>{granter}</b><span>{granter_desc}</span></div>\
         <div class=\"mc-sub\">{desc_head}</div><p>{desc}</p>\
         <div class=\"mc-sub\">{obj_head}</div><ol class=\"mc-objectives\">{objectives}</ol>\
         <div class=\"mc-reward\">{reward_head} <b>{reward:.0}</b> {science}</div>\
         <div class=\"mc-state\">{state}</div>{action}<div id=\"mc-note\" data-ose=\"mc-note\"></div>",
        kind = esc(&text(
            loc,
            if m.primary {
                "Missions/MissionControl/Primary"
            } else {
                "Missions/MissionControl/Secondary"
            }
        )),
        name = esc(&text(loc, &m.name_key)),
        granter_head = esc(&text(loc, "Missions/MissionControl/Popover/GranterHeader")),
        granter = esc(&granter(loc, &m.granter)),
        granter_desc = esc(loc
            .get(&format!(
                "Missions/MissionGranter/Description/{}",
                m.granter
                    .trim_start_matches("MIssionGranter")
                    .trim_start_matches("MissionGranter")
            ))
            .map(|s| s.as_str())
            .unwrap_or("")),
        desc_head = esc(&text(
            loc,
            "Missions/MissionControl/Popover/MissionDescriptionHeader"
        )),
        desc = esc(&text(loc, &m.description_key)),
        obj_head = esc(&text(loc, "Missions/MissionControl/Popover/Objectives")),
        reward_head = esc(&text(loc, "Missions/MissionControl/Rewards/Reward")),
        science = esc(&text(loc, "Missions/MissionControl/Rewards/Science")),
        reward = reward(m),
        // The original's mission log: take on an offered mission, hand in a finished one for its reward.
        action = match info {
            Some(i) if i.complete && !i.turned_in => format!(
                "<button class=\"mc-turnin\" data-ose=\"mc-turnin\" data-ose-mission-turnin=\"{}\">Turn in ({:.0} science)</button>",
                esc(&m.id),
                reward(m)
            ),
            Some(i) if !i.accepted && !i.complete => format!(
                "<button class=\"mc-accept\" data-ose=\"mc-accept\" data-ose-mission-accept=\"{}\">Accept mission</button>",
                esc(&m.id)
            ),
            _ => String::new(),
        },
        state = match info {
            Some(i) if i.turned_in => "Complete: reward paid".to_string(),
            Some(i) if i.complete => "Complete: turn it in at Mission Control".to_string(),
            Some(i) if !i.accepted => "Offered: accept it to start".to_string(),
            Some(i) => format!(
                "Active: stage {} of {}",
                (i.stage + 1).min(i.stages.max(1)),
                i.stages.max(1)
            ),
            None if m.active_at_start => "Active".to_string(),
            None => "Offered later in the campaign".to_string(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_missions_load_with_objectives() {
        let m = all();
        assert!(m.len() >= 50, "{} missions", m.len());
        assert!(m.iter().any(|x| x.primary) && m.iter().any(|x| !x.primary));
        assert!(m.iter().any(|x| x.active_at_start));
        let loc = HashMap::new();
        let list = render_list(&m, &loc, true, None, None, false);
        assert!(list.contains("data-ose=\"mc-mission\""));
        let b = render_briefing(&m[0], &loc, None);
        assert!(b.contains("<h3>"));
    }
}

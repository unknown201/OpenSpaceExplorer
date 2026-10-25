//! Kerbal Space Academy lessons (`data/tutorials`, the original's Training Center lessons): the groups and
//! lessons from assets/ui/tutorials.json (tools/export_tutorials.py), listed in the Training Center's Lessons
//! tab; a lesson plays PAIGE's messages stage by stage in the first-time hint box (`ftue.rs`). The lessons'
//! conditions are not checked yet: the player moves on with Continue.

use crate::ftue::Message;
use serde::Deserialize;
use std::collections::HashMap;

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
}

//! The Research and Development Center: the original's tech tree (`assets/own/tech-tree.json`, imported
//! from the game's data by the server thread) laid out on its own canvas as the original's R&D screen does:
//! nodes at their authored positions with connectors to their prerequisites, tier gates splitting the canvas,
//! and an info panel with the node's name, description, science cost and the parts it unlocks. Text comes from
//! the original's English localization (`assets/loc/en.json`, `tools/export_localization.py`), falling back to
//! the term itself.
//!
//! Research state is not in the protocol yet: the start node counts as researched and the rest as locked or
//! available by their prerequisites, until the server sends the player's tech.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Deserialize, Clone, Debug)]
pub struct TechNode {
    pub id: String,
    #[serde(default)]
    pub name_key: String,
    #[serde(default)]
    pub description_key: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub required_science: f64,
    #[serde(default)]
    pub parts: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub position: [f64; 2],
}

/// Node box size on the canvas, px (the original's nodes are about this wide).
pub const NODE_W: f64 = 128.0;
pub const NODE_H: f64 = 54.0;
/// Margin around the authored positions.
const PAD: f64 = 60.0;

pub fn tree() -> Vec<TechNode> {
    serde_json::from_str(include_str!("../../../assets/own/tech-tree.json")).unwrap_or_default()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// English text for a localization term, or a readable form of the term's last segment.
pub fn text<'a>(loc: &'a HashMap<String, String>, key: &'a str) -> std::borrow::Cow<'a, str> {
    match loc.get(key) {
        Some(v) => std::borrow::Cow::Borrowed(v.as_str()),
        None => {
            let last = key.rsplit('/').next().unwrap_or(key);
            std::borrow::Cow::Owned(last.replace(['_', '-'], " "))
        }
    }
}

/// Node state for display.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Researched,
    Available,
    Locked,
}

pub fn state(node: &TechNode, researched: &HashSet<String>) -> State {
    if researched.contains(&node.id) {
        State::Researched
    } else if node.requires.iter().all(|r| researched.contains(r)) {
        State::Available
    } else {
        State::Locked
    }
}

/// The tree canvas as HTML: an SVG layer of connectors under absolutely positioned node buttons.
pub fn render_tree(
    nodes: &[TechNode],
    loc: &HashMap<String, String>,
    researched: &HashSet<String>,
    selected: Option<&str>,
) -> String {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for n in nodes {
        minx = minx.min(n.position[0]);
        miny = miny.min(n.position[1]);
        maxx = maxx.max(n.position[0]);
        maxy = maxy.max(n.position[1]);
    }
    if nodes.is_empty() {
        return "<div class=\"rd-empty\">no tech tree data</div>".into();
    }
    let ox = PAD - minx;
    let oy = PAD - miny;
    let w = maxx - minx + NODE_W + 2.0 * PAD;
    let h = maxy - miny + NODE_H + 2.0 * PAD;
    let by_id: HashMap<&str, &TechNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let centre = |n: &TechNode| {
        (
            n.position[0] + ox + NODE_W * 0.5,
            n.position[1] + oy + NODE_H * 0.5,
        )
    };
    let mut svg = format!(
        "<svg class=\"rd-lines\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">"
    );
    for n in nodes {
        for r in &n.requires {
            let Some(p) = by_id.get(r.as_str()) else {
                continue;
            };
            let (x1, y1) = centre(p);
            let (x2, y2) = centre(n);
            let mx = (x1 + x2) * 0.5;
            let cls = match state(n, researched) {
                State::Researched => "done",
                State::Available => "open",
                State::Locked => "",
            };
            svg.push_str(&format!(
                "<path class=\"{cls}\" d=\"M{x1:.0},{y1:.0} C{mx:.0},{y1:.0} {mx:.0},{y2:.0} {x2:.0},{y2:.0}\"/>"
            ));
        }
    }
    svg.push_str("</svg>");
    let mut html =
        format!("<div class=\"rd-canvas\" style=\"width:{w:.0}px;height:{h:.0}px\">{svg}");
    for n in nodes {
        let st = state(n, researched);
        let cls = match st {
            State::Researched => "researched",
            State::Available => "available",
            State::Locked => "locked",
        };
        let sel = if selected == Some(n.id.as_str()) {
            " selected"
        } else {
            ""
        };
        html.push_str(&format!(
            "<button class=\"rd-node {cls}{sel}\" data-ose=\"rd-node\" data-ose-node=\"{id}\" \
             style=\"left:{x:.0}px;top:{y:.0}px\"><img src=\"assets/ui/tech/{icon}.png\" alt=\"\" />\
             <span class=\"name\">{name}</span><span class=\"cost\">{cost:.0}</span></button>",
            id = esc(&n.id),
            x = n.position[0] + ox,
            y = n.position[1] + oy,
            icon = esc(&n.icon),
            name = esc(&text(loc, &n.name_key)),
            cost = n.required_science,
        ));
    }
    html.push_str("</div>");
    html
}

/// The info panel for one node: name, description, cost, unlocked parts (by their catalog titles).
pub fn render_info(
    node: &TechNode,
    loc: &HashMap<String, String>,
    researched: &HashSet<String>,
    part_title: &dyn Fn(&str) -> String,
) -> String {
    let st = state(node, researched);
    let status = match st {
        State::Researched => "Researched",
        State::Available => "Available to research",
        State::Locked => "Requires earlier technology",
    };
    let mut parts = String::new();
    for p in &node.parts {
        parts.push_str(&format!(
            "<li data-ose-part=\"{id}\">{title}</li>",
            id = esc(p),
            title = esc(&part_title(p))
        ));
    }
    format!(
        "<h3>{name}</h3><div class=\"rd-status {cls}\">{status}</div><p>{desc}</p>\
         <div class=\"rd-cost\">Science cost <b>{cost:.0}</b></div>\
         <div class=\"rd-sub\">Parts acquired</div><ul class=\"rd-parts\">{parts}</ul>",
        name = esc(&text(loc, &node.name_key)),
        desc = esc(&text(loc, &node.description_key)),
        cost = node.required_science,
        cls = match st {
            State::Researched => "researched",
            State::Available => "available",
            State::Locked => "locked",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_tree_has_a_root_and_reachable_nodes() {
        let t = tree();
        assert!(t.len() > 50, "tech tree has {} nodes", t.len());
        let ids: HashSet<&str> = t.iter().map(|n| n.id.as_str()).collect();
        let roots: Vec<&TechNode> = t.iter().filter(|n| n.requires.is_empty()).collect();
        assert_eq!(roots.len(), 1);
        for n in &t {
            for r in &n.requires {
                assert!(ids.contains(r.as_str()), "{} requires unknown {}", n.id, r);
            }
        }
        let mut done = HashSet::new();
        done.insert(roots[0].id.clone());
        let open = t
            .iter()
            .filter(|n| state(n, &done) == State::Available)
            .count();
        assert!(open > 0);
        let html = render_tree(&t, &HashMap::new(), &done, None);
        assert_eq!(html.matches("data-ose=\"rd-node\"").count(), t.len());
    }
}

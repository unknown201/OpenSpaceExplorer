//! First-time hints (`data/firsttimeuserexperience/FTUE_*.json`, the original's FTUE): PAIGE's message boxes
//! on the first visit to each screen and the first time something happens, from assets/ui/ftue.json
//! (tools/export_ftue.py) with the text from the localization table.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize)]
pub struct Message {
    pub key: String,
    #[serde(default)]
    pub image: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Script {
    #[serde(default)]
    pub name: String,
    pub messages: Vec<Message>,
}

pub type Scripts = HashMap<String, Script>;

pub fn parse(text: &str) -> Option<Scripts> {
    serde_json::from_str(text).ok()
}

/// The original's TextMeshPro rich text as HTML: `<color=#hex>` spans and bold/italic kept, `<cspace>` and
/// anything else dropped, plain text escaped, newlines as breaks, and the table's broken apostrophes mended.
pub fn rich_text(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(i) = rest.find('<') {
        out.push_str(&escape(&rest[..i]));
        let Some(j) = rest[i..].find('>') else {
            out.push_str(&escape(&rest[i..]));
            rest = "";
            break;
        };
        let tag = &rest[i + 1..i + j];
        let lower = tag.to_ascii_lowercase();
        if let Some(c) = lower.strip_prefix("color=") {
            let c: String = c
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '#')
                .collect();
            out.push_str(&format!("<span style=\"color:{c}\">"));
        } else {
            match lower.as_str() {
                "/color" => out.push_str("</span>"),
                "b" | "/b" | "i" | "/i" => out.push_str(&format!("<{lower}>")),
                "br" => out.push_str("<br>"),
                _ => {}
            }
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(&escape(rest));
    out.trim().replace('\n', "<br>")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\u{fffd}', "\u{2019}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_text_becomes_html() {
        let t = "Welcome to the <b><color=#FFA114><cspace=-.1>VAB</cspace></color></b>! It\u{fffd}s here.\n";
        assert_eq!(
            rich_text(t),
            "Welcome to the <b><span style=\"color:#ffa114\">VAB</span></b>! It\u{2019}s here."
        );
        assert_eq!(rich_text("a < b"), "a &lt; b");
        let s = parse(r#"{"FTUE_VAB":{"name":"x","messages":[{"key":"FTUE/a","image":"PAIGE"}]}}"#)
            .unwrap();
        assert_eq!(s["FTUE_VAB"].messages[0].key, "FTUE/a");
    }
}

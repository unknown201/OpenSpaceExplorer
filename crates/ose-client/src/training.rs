//! The Training Center: the campaign's astronauts (`KerbalRoster`, protocol 32) as a roster on the left, by
//! availability, and the selected Kerbal's file on the right: where they are, when they enrolled, and the
//! original's four personality traits. KSP2 has no experience levels or professions, so neither is shown.
//!
//! Also the launch dialog's crew choice: the seats a craft has and who sits in each.

use ose_net::{KerbalInfo, KerbalStatus};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Where a Kerbal is, in words; `vessel_name` names a vessel id.
pub fn status_text(k: &KerbalInfo, vessel_name: &dyn Fn(u64) -> Option<String>) -> String {
    match k.status {
        KerbalStatus::Available => "Available".into(),
        KerbalStatus::Assigned { vessel } => match vessel_name(vessel) {
            Some(n) => format!("Aboard {n}"),
            None => "Assigned".into(),
        },
        KerbalStatus::Eva { vessel } => match vessel_name(vessel) {
            Some(n) => format!("On EVA ({n})"),
            None => "On EVA".into(),
        },
        KerbalStatus::Missing => "Missing".into(),
    }
}

fn status_class(k: &KerbalInfo) -> &'static str {
    match k.status {
        KerbalStatus::Available => "available",
        KerbalStatus::Assigned { .. } | KerbalStatus::Eva { .. } => "assigned",
        KerbalStatus::Missing => "missing",
    }
}

/// One of the original's looks keys (SKINCOLOR, HAIRSTYLE, ...).
fn look<'a>(k: &'a KerbalInfo, key: &str) -> Option<&'a str> {
    k.looks
        .iter()
        .find(|(a, _)| a == key)
        .map(|(_, b)| b.as_str())
}

/// A looks colour ("BADA55") as CSS, or `fallback`.
fn hex(k: &KerbalInfo, key: &str, fallback: &str) -> String {
    match look(k, key) {
        Some(h) if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) => format!("#{h}"),
        _ => fallback.to_string(),
    }
}

/// A Kerbal's face from the original's looks: skin and hair colour, the square or round head type, the suit
/// in the team colour (gold trim for veterans), and a grin as wide as their optimism.
pub fn portrait(k: &KerbalInfo) -> String {
    face(k, "tc-face")
}

pub fn face(k: &KerbalInfo, class: &str) -> String {
    let skin = hex(k, "SKINCOLOR", "#8fd16a");
    let hair = hex(k, "HAIRCOLOR", "#55483f");
    let suit = hex(k, "TEAMCOLOR2", "#e8ecf2");
    let team = hex(k, "TEAMCOLOR1", "#bf0000");
    let trim = if k.veteran { "#e6b84a" } else { team.as_str() };
    let round = look(k, "TYPE") == Some("CIRCLE");
    let head = if round {
        format!(
            "<ellipse cx=\"20\" cy=\"19\" rx=\"12\" ry=\"13\" fill=\"{skin}\" stroke=\"#0005\"/>"
        )
    } else {
        format!("<rect x=\"7\" y=\"6\" width=\"26\" height=\"26\" rx=\"10\" fill=\"{skin}\" stroke=\"#0005\"/>")
    };
    let hair_shape = if round {
        // A bun and a fringe.
        format!("<circle cx=\"20\" cy=\"5.5\" r=\"4\" fill=\"{hair}\"/><path d=\"M8.5 15 Q12 6 20 6 Q28 6 31.5 15 Q26 9.5 20 10 Q13 9.5 8.5 15Z\" fill=\"{hair}\"/>")
    } else {
        format!("<path d=\"M10 12 Q14 4 22 5 Q30 6 31 12 Q25 8.5 18 9.5 Q13 10 10 12Z\" fill=\"{hair}\"/>")
    };
    let grin = 6.0 + 8.0 * k.optimism as f64;
    format!(
        "<svg class=\"{class}\" viewBox=\"0 0 40 40\" aria-hidden=\"true\">\
         <path d=\"M4 40 Q20 29 36 40\" fill=\"{suit}\"/><path d=\"M11 35.5 Q20 31 29 35.5\" stroke=\"{trim}\" stroke-width=\"2\" fill=\"none\"/>\
         {head}{hair_shape}\
         <circle cx=\"15\" cy=\"18\" r=\"3.4\" fill=\"#fff\"/><circle cx=\"25\" cy=\"18\" r=\"3.4\" fill=\"#fff\"/>\
         <circle cx=\"15.6\" cy=\"18.4\" r=\"1.5\" fill=\"#222\"/><circle cx=\"24.4\" cy=\"18.4\" r=\"1.5\" fill=\"#222\"/>\
         <path d=\"M{x0:.1} 25.5 Q20 {y:.1} {x1:.1} 25.5\" stroke=\"#0009\" stroke-width=\"1.6\" fill=\"none\"/></svg>",
        x0 = 20.0 - grin / 2.0,
        x1 = 20.0 + grin / 2.0,
        y = 25.5 + grin / 2.5,
    )
}

/// Available first, then flying, then missing; veterans first within each, then by name.
pub fn sorted(kerbals: &[KerbalInfo]) -> Vec<&KerbalInfo> {
    let rank = |k: &KerbalInfo| match k.status {
        KerbalStatus::Available => 0,
        KerbalStatus::Assigned { .. } | KerbalStatus::Eva { .. } => 1,
        KerbalStatus::Missing => 2,
    };
    let mut v: Vec<&KerbalInfo> = kerbals.iter().collect();
    v.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(b.veteran.cmp(&a.veteran))
            .then(a.name.cmp(&b.name))
    });
    v
}

pub fn render_list(
    kerbals: &[KerbalInfo],
    selected: Option<&str>,
    vessel_name: &dyn Fn(u64) -> Option<String>,
) -> String {
    if kerbals.is_empty() {
        return "<li class=\"tc-empty\">No astronauts yet.</li>".into();
    }
    let mut html = String::new();
    for k in sorted(kerbals) {
        html.push_str(&format!(
            "<li class=\"tc-kerbal {cls}{sel}\" data-ose=\"tc-kerbal\" data-ose-kerbal=\"{name}\">{face}\
             <div><div class=\"name\">{name}{vet}</div><div class=\"where\">{status}</div></div></li>",
            cls = status_class(k),
            sel = if selected == Some(k.name.as_str()) { " selected" } else { "" },
            name = esc(&k.name),
            face = portrait(k),
            vet = if k.veteran { " <span class=\"tc-vet\">Veteran</span>" } else { "" },
            status = esc(&status_text(k, vessel_name)),
        ));
    }
    html
}

pub fn render_info(k: Option<&KerbalInfo>, vessel_name: &dyn Fn(u64) -> Option<String>) -> String {
    let Some(k) = k else {
        return "<div class=\"tc-none\">Select an astronaut</div>".into();
    };
    let bar = |label: &str, v: f32| {
        format!(
            "<dt>{label}</dt><dd><span class=\"tc-bar\"><i style=\"width:{:.0}%\"></i></span></dd>",
            (v.clamp(0.0, 1.0) * 100.0)
        )
    };
    format!(
        "<div class=\"tc-file\">{face}<div><h3 data-ose=\"tc-name\">{name}</h3><div class=\"tc-status {cls}\" data-ose=\"tc-status\">{status}</div>\
         <div class=\"tc-enrolled\">{vet}Enrolled {enrolled}</div></div></div>\
         <div class=\"tc-sub\">Personality</div><dl class=\"tc-traits\">{b}{s}{o}{c}</dl>",
        face = portrait(k),
        name = esc(&k.name),
        cls = status_class(k),
        status = esc(&status_text(k, vessel_name)),
        vet = if k.veteran { "Veteran. " } else { "" },
        enrolled = crate::kerbin_time::format_ut(k.enrolled_ut),
        b = bar("Bravery", k.bravery),
        s = bar("Stupidity", k.stupidity),
        o = bar("Optimism", k.optimism),
        c = bar("Constitution", k.constitution),
    )
}

/// The crew for `seats` seats: keep the current picks that are still free (or already in this crew), then
/// fill empty seats from the available roster in order, the way the server fills them for `crew: None`.
pub fn fill_crew(current: &[String], seats: usize, kerbals: &[KerbalInfo]) -> Vec<String> {
    let free = |n: &str| {
        kerbals
            .iter()
            .any(|k| k.name == n && k.status == KerbalStatus::Available)
    };
    let mut crew: Vec<String> = (0..seats)
        .map(|i| {
            current
                .get(i)
                .filter(|n| n.is_empty() || free(n))
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    // A name picked twice keeps only its first seat.
    for i in 0..crew.len() {
        if !crew[i].is_empty() && crew[..i].contains(&crew[i]) {
            crew[i].clear();
        }
    }
    if current.is_empty() {
        let taken = crew.clone();
        let mut pool = sorted(kerbals)
            .into_iter()
            .filter(|k| k.status == KerbalStatus::Available && !taken.contains(&k.name));
        for seat in crew.iter_mut().filter(|s| s.is_empty()) {
            match pool.next() {
                Some(k) => *seat = k.name.clone(),
                None => break,
            }
        }
    }
    crew
}

/// The launch dialog's crew rows: a choice per seat of the available Kerbals or an empty seat.
pub fn render_crew(crew: &[String], kerbals: &[KerbalInfo]) -> String {
    if crew.is_empty() {
        return String::new();
    }
    let mut html = String::from("<div class=\"lp-crew\" data-ose=\"lp-crew\"><b>Crew</b>");
    for (i, who) in crew.iter().enumerate() {
        let mut opts = format!(
            "<option value=\"\"{}>Empty seat</option>",
            if who.is_empty() { " selected" } else { "" }
        );
        for k in sorted(kerbals) {
            let taken = crew.iter().enumerate().any(|(j, n)| j != i && *n == k.name);
            if k.status != KerbalStatus::Available || taken {
                continue;
            }
            opts.push_str(&format!(
                "<option value=\"{n}\"{sel}>{n}{vet}</option>",
                n = esc(&k.name),
                sel = if *who == k.name { " selected" } else { "" },
                vet = if k.veteran { " (veteran)" } else { "" },
            ));
        }
        html.push_str(&format!(
            "<label>Seat {}<select data-ose=\"lp-seat\" data-ose-seat=\"{i}\">{opts}</select></label>",
            i + 1
        ));
    }
    html.push_str("</div>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kerbal(name: &str, status: KerbalStatus, veteran: bool) -> KerbalInfo {
        KerbalInfo {
            name: name.into(),
            status,
            veteran,
            enrolled_ut: 0.0,
            bravery: 0.5,
            stupidity: 0.5,
            optimism: 0.5,
            constitution: 0.5,
            looks: vec![],
        }
    }

    #[test]
    fn crew_fills_from_available_veterans_first_and_keeps_picks() {
        let roster = vec![
            kerbal("Bill", KerbalStatus::Available, false),
            kerbal("Jeb", KerbalStatus::Available, true),
            kerbal("Val", KerbalStatus::Assigned { vessel: 3 }, true),
            kerbal("Bob", KerbalStatus::Available, false),
        ];
        assert_eq!(fill_crew(&[], 2, &roster), vec!["Jeb", "Bill"]);
        // A pick stays; an empty seat chosen on purpose stays empty; a flying Kerbal is dropped.
        let picks = vec!["Bob".to_string(), String::new(), "Val".to_string()];
        assert_eq!(fill_crew(&picks, 3, &roster), vec!["Bob", "", ""]);
        let html = render_crew(&fill_crew(&[], 1, &roster), &roster);
        assert!(html.contains("Jeb") && !html.contains("Val"));
        let list = render_list(&roster, Some("Val"), &|_| Some("Kerbal K1".into()));
        assert!(list.contains("Aboard Kerbal K1") && list.contains("selected"));
    }
}

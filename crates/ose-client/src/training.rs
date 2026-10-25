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

/// A small Kerbal head: the green face, the suit's collar gold for veterans.
fn portrait(k: &KerbalInfo) -> String {
    let collar = if k.veteran { "#e6b84a" } else { "#e8ecf2" };
    let grin = 6.0 + 8.0 * k.optimism as f64;
    format!(
        "<svg class=\"tc-face\" viewBox=\"0 0 40 40\" aria-hidden=\"true\">\
         <path d=\"M6 40 Q20 30 34 40\" fill=\"{collar}\"/>\
         <ellipse cx=\"20\" cy=\"19\" rx=\"13\" ry=\"14\" fill=\"#8fd16a\" stroke=\"#4d7d34\"/>\
         <circle cx=\"15\" cy=\"16\" r=\"3.4\" fill=\"#fff\"/><circle cx=\"25\" cy=\"16\" r=\"3.4\" fill=\"#fff\"/>\
         <circle cx=\"15.6\" cy=\"16.4\" r=\"1.5\" fill=\"#222\"/><circle cx=\"24.4\" cy=\"16.4\" r=\"1.5\" fill=\"#222\"/>\
         <path d=\"M{x0:.1} 24 Q20 {y:.1} {x1:.1} 24\" stroke=\"#2d4a1e\" stroke-width=\"1.6\" fill=\"none\"/></svg>",
        x0 = 20.0 - grin / 2.0,
        x1 = 20.0 + grin / 2.0,
        y = 24.0 + grin / 2.5,
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

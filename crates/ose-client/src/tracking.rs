//! The Tracking Station: the original's list of every vessel in the campaign beside the map, each with its
//! body, situation and altitude, and for the selected one its orbit with Focus (the map follows it), Fly
//! (your own vessels) and Recover (your own, landed on Kerbin).

use ose_net::{FlightSituation, VesselState};

/// What the station shows for one vessel.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub name: String,
    pub mine: bool,
    pub body: String,
    pub situation: &'static str,
    pub altitude_m: f64,
    pub apoapsis_m: f64,
    pub periapsis_m: f64,
    /// Whose it is (multiplayer) and their colour, `#rrggbb`; empty for nobody's.
    pub owner: String,
    pub owner_rgb: String,
    /// What it is, for its map icon: "vessel", "eva" or "flag".
    pub kind: &'static str,
}

/// Situation as the original names it: the flight model's when the vessel is under it, otherwise from its
/// orbit (a periapsis inside the atmosphere or the ground is sub-orbital).
pub fn situation(
    v: &VesselState,
    radius_m: f64,
    atmosphere_m: f64,
    periapsis_m: f64,
) -> &'static str {
    match v.flight.as_ref().map(|f| f.situation) {
        Some(FlightSituation::Landed) => "Landed",
        Some(FlightSituation::Flying) => "Flying",
        Some(FlightSituation::SubOrbital) => "Sub-orbital",
        Some(FlightSituation::Orbit) => "Orbiting",
        Some(FlightSituation::Escaping) => "Escaping",
        Some(FlightSituation::Crashed) => "Crashed",
        None if periapsis_m < radius_m + atmosphere_m => "Sub-orbital",
        None => "Orbiting",
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn km(m: f64) -> String {
    if m.abs() >= 1.0e6 {
        format!("{:.0} km", m / 1000.0)
    } else if m.abs() >= 1.0e4 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{:.0} m", m)
    }
}

/// The vessel list: yours first, then by name; `mine_only` hides other players' vessels.
pub fn render_list(rows: &[Row], selected: Option<u64>, mine_only: bool) -> String {
    let mut rows: Vec<&Row> = rows.iter().filter(|r| r.mine || !mine_only).collect();
    rows.sort_by(|a, b| b.mine.cmp(&a.mine).then(a.name.cmp(&b.name)));
    let mut html = String::new();
    for r in rows {
        html.push_str(&format!(
            "<li class=\"ts-vessel{mine}{sel} {kind}\" data-ose=\"ts-vessel\" data-ose-id=\"{id}\">\
             <span class=\"name\">{name}</span><span class=\"meta\">{body} \u{b7} {sit} \u{b7} {alt}{owner}</span></li>",
            kind = r.kind,
            owner = if r.owner.is_empty() || r.mine {
                String::new()
            } else {
                format!(
                    " \u{b7} <i class=\"ts-owner-dot\" style=\"background:{}\"></i>{}",
                    esc(&r.owner_rgb),
                    esc(&r.owner)
                )
            },
            mine = if r.mine { " mine" } else { "" },
            sel = if selected == Some(r.id) { " selected" } else { "" },
            id = r.id,
            name = esc(&r.name),
            body = esc(&r.body),
            sit = r.situation,
            alt = km(r.altitude_m),
        ));
    }
    if html.is_empty() {
        html.push_str("<li class=\"ts-empty\">no vessels being tracked</li>");
    }
    html
}

/// The selected vessel's card.
pub fn render_info(r: &Row) -> String {
    let recover = r.mine && r.situation == "Landed" && r.body == "Kerbin";
    format!(
        "<h3>{name}</h3><div class=\"ts-owner\">{owner}</div><dl>\
         <dt>Body</dt><dd>{body}</dd><dt>Situation</dt><dd>{sit}</dd><dt>Altitude</dt><dd>{alt}</dd>\
         <dt>Apoapsis</dt><dd>{ap}</dd><dt>Periapsis</dt><dd>{pe}</dd></dl>\
         <div class=\"ts-actions\"><button data-ose=\"ts-focus\" data-ose-ts-focus=\"{id}\">Focus</button>\
         <button data-ose=\"ts-fly\" data-ose-ts-fly=\"{id}\"{fly}>Fly</button>\
         <button data-ose=\"ts-recover\" data-ose-ts-recover=\"{id}\"{rec}>Recover</button></div>",
        name = esc(&r.name),
        owner = if r.mine {
            "Your vessel".to_string()
        } else if r.owner.is_empty() {
            "Another agency's vessel".to_string()
        } else {
            format!("{}'s vessel", esc(&r.owner))
        },
        body = esc(&r.body),
        sit = r.situation,
        alt = km(r.altitude_m),
        ap = if r.apoapsis_m.is_finite() && r.apoapsis_m > 0.0 {
            km(r.apoapsis_m)
        } else {
            "escape".into()
        },
        pe = km(r.periapsis_m),
        id = r.id,
        fly = if r.mine { "" } else { " disabled title=\"not your vessel\"" },
        rec = if recover {
            ""
        } else {
            " disabled title=\"land your vessel on Kerbin to recover it\""
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64, name: &str, mine: bool, sit: &'static str) -> Row {
        Row {
            id,
            name: name.into(),
            mine,
            body: "Kerbin".into(),
            situation: sit,
            altitude_m: 80_000.0,
            apoapsis_m: 90_000.0,
            periapsis_m: 75_000.0,
            owner: String::new(),
            owner_rgb: String::new(),
            kind: "vessel",
        }
    }

    #[test]
    fn yours_come_first_and_only_yours_can_fly_or_recover() {
        let rows = vec![
            row(1, "Alpha", false, "Orbiting"),
            row(2, "Zulu", true, "Landed"),
        ];
        let html = render_list(&rows, Some(2), false);
        assert!(html.find("Zulu").unwrap() < html.find("Alpha").unwrap());
        assert!(!render_list(&rows, None, true).contains("Alpha"));
        let mine = render_info(&rows[1]);
        assert!(!mine.contains("data-ose-ts-fly=\"2\" disabled"));
        assert!(!mine.contains("data-ose-ts-recover=\"2\" disabled"));
        let theirs = render_info(&rows[0]);
        assert!(theirs.contains("data-ose-ts-fly=\"1\" disabled"));
        assert_eq!(km(1_500.0), "1500 m");
        assert_eq!(km(85_000.0), "85.0 km");
    }
}

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
    /// What it is, for its map icon and the station's filters: "vessel", "debris", "eva" or "flag".
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

/// The station's object filters (the original's Filters): kind, label.
pub const FILTERS: [(&str, &str); 4] = [
    ("vessel", "Vessels"),
    ("debris", "Debris"),
    ("eva", "Kerbals"),
    ("flag", "Flags"),
];

/// The filter toggles, lit for kinds shown, with how many of each there are.
pub fn render_filters(rows: &[Row], hidden: &[&str]) -> String {
    FILTERS
        .iter()
        .map(|(kind, label)| {
            format!(
                "<button class=\"ts-filter {kind}{on}\" data-ose=\"ts-filter\" data-ose-kind=\"{kind}\">{label} <span>{n}</span></button>",
                on = if hidden.contains(kind) { "" } else { " on" },
                n = rows.iter().filter(|r| r.kind == *kind).count(),
            )
        })
        .collect()
}

/// The vessel list: yours first, then by name; `mine_only` hides other players' vessels, `hidden` kinds
/// filtered out.
pub fn render_list(
    rows: &[Row],
    selected: Option<u64>,
    mine_only: bool,
    hidden: &[&str],
) -> String {
    let mut rows: Vec<&Row> = rows
        .iter()
        .filter(|r| (r.mine || !mine_only) && !hidden.contains(&r.kind))
        .collect();
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

/// A celestial body for the station's information panel (the original's Physical and Orbital
/// Characteristics).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BodyCard {
    pub name: String,
    pub synopsis: String,
    pub is_star: bool,
    pub radius_m: f64,
    pub mu: f64,
    pub soi_m: Option<f64>,
    /// Sidereal rotation period, s (0: does not spin).
    pub rotation_s: f64,
    /// Atmosphere height, surface pressure (atm) and whether it has oxygen.
    pub atmosphere: Option<(f64, f64, bool)>,
    /// Parent's name, its `mu`, and the orbit's semi-major axis (m), eccentricity and inclination (deg).
    pub orbit: Option<(String, f64, f64, f64, f64)>,
}

/// One row of the body list: catalog index, name, depth in the hierarchy (0 the star).
pub fn render_body_list(bodies: &[(usize, String, usize)], selected: Option<usize>) -> String {
    let mut html = String::from("<li class=\"ts-section\">Celestial Bodies</li>");
    for (i, name, depth) in bodies {
        html.push_str(&format!(
            "<li class=\"ts-body{sel}\" data-ose=\"ts-body\" data-ose-body=\"{i}\" style=\"padding-left:{pad}px\">\
             <span class=\"name\">{name}</span></li>",
            sel = if selected == Some(*i) { " selected" } else { "" },
            pad = 10 + 14 * depth,
            name = esc(name),
        ));
    }
    html
}

/// What the map's information panel shows for a vessel beyond its station row (the original's Flight
/// Situation, Orbital Characteristics and Manifest).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VesselExtra {
    pub speed_mps: f64,
    pub inclination_deg: f64,
    pub period_s: f64,
    /// Delta-v left in the stages to fire, where the vessel is now; `None` without a readout.
    pub delta_v_mps: Option<f64>,
    /// Electric charge stored and capacity.
    pub power: Option<(f64, f64)>,
    pub crew: Vec<String>,
}

/// The map's information panel for a vessel.
pub fn render_vessel_map_info(r: &Row, x: &VesselExtra) -> String {
    let row = |k: &str, v: String| format!("<dt>{k}</dt><dd>{v}</dd>");
    let mut sit = row("Situation", r.situation.into());
    sit.push_str(&row(
        "Trajectory Type",
        match r.situation {
            "Landed" | "Crashed" => "none",
            "Escaping" => "escape",
            "Sub-orbital" | "Flying" => "sub-orbital",
            _ => "orbit",
        }
        .into(),
    ));
    sit.push_str(&row("Velocity", format!("{:.0} m/s", x.speed_mps)));
    if let Some(dv) = x.delta_v_mps {
        sit.push_str(&row("Delta-V Remaining", format!("{dv:.0} m/s")));
    }
    if let Some((now, cap)) = x.power {
        sit.push_str(&row("Power Stored", format!("{now:.0} / {cap:.0}")));
    }
    let mut orb = row("Body", esc(&r.body));
    orb.push_str(&row("Altitude", km(r.altitude_m)));
    orb.push_str(&row(
        "Apoapsis",
        if r.apoapsis_m.is_finite() && r.apoapsis_m > 0.0 {
            km(r.apoapsis_m)
        } else {
            "escape".into()
        },
    ));
    orb.push_str(&row("Periapsis", km(r.periapsis_m)));
    orb.push_str(&row(
        "Inclination",
        format!("{:.2}\u{b0}", x.inclination_deg),
    ));
    if x.period_s.is_finite() && x.period_s > 0.0 {
        orb.push_str(&row("Period", duration(x.period_s)));
    }
    let crew = if x.crew.is_empty() {
        "<div class=\"ts-owner\">no crew</div>".to_string()
    } else {
        format!(
            "<dl>{}</dl>",
            x.crew
                .iter()
                .map(|c| row("Crew", esc(c)))
                .collect::<String>()
        )
    };
    format!(
        "<h3>{name}</h3><div class=\"ts-owner\">{owner}</div><h4>Flight Situation</h4><dl>{sit}</dl>\
         <h4>Orbital Characteristics</h4><dl>{orb}</dl><h4>Manifest</h4>{crew}",
        name = esc(&r.name),
        owner = if r.mine {
            "Your vessel".to_string()
        } else if r.owner.is_empty() {
            "Vessel".to_string()
        } else {
            format!("{}'s vessel", esc(&r.owner))
        },
    )
}

/// Gravitational constant, m^3 kg^-1 s^-2.
const BIG_G: f64 = 6.674_30e-11;

/// A duration in Kerbin days (6 h) and hours, as the original's info panel gives periods.
pub fn duration(s: f64) -> String {
    if !s.is_finite() || s <= 0.0 {
        return "--".into();
    }
    let m = (s / 60.0).round();
    if m >= 360.0 {
        let d = (m / 360.0).floor();
        format!("{d:.0}d {:.1}h", (m - d * 360.0) / 60.0)
    } else if m >= 60.0 {
        format!("{:.0}h {:.0}m", (m / 60.0).floor(), m % 60.0)
    } else {
        format!("{:.0}m {:.0}s", (s / 60.0).floor(), s % 60.0)
    }
}

fn sci(x: f64) -> String {
    let e = x.abs().log10().floor();
    format!("{:.3}e{e:.0}", x / 10f64.powf(e))
}

/// The selected body's card: synopsis, physical and orbital characteristics, and Focus.
pub fn render_body_info(b: &BodyCard, index: usize) -> String {
    let row = |k: &str, v: String| format!("<dt>{k}</dt><dd>{v}</dd>");
    let mass = b.mu / BIG_G;
    let r = b.radius_m;
    let mut phys = String::new();
    phys.push_str(&row("Radius", km(r)));
    phys.push_str(&row("Circumference", km(std::f64::consts::TAU * r)));
    phys.push_str(&row("Mass", format!("{} kg", sci(mass))));
    phys.push_str(&row(
        "Density",
        format!(
            "{:.0} kg/m^3",
            mass / (4.0 / 3.0 * std::f64::consts::PI * r * r * r)
        ),
    ));
    phys.push_str(&row("Gravity", format!("{:.2} m/s^2", b.mu / (r * r))));
    phys.push_str(&row(
        "Escape velocity",
        format!("{:.0} m/s", (2.0 * b.mu / r).sqrt()),
    ));
    phys.push_str(&row(
        "Sphere of Influence",
        b.soi_m.map(km).unwrap_or_else(|| "infinite".into()),
    ));
    if b.rotation_s > 0.0 {
        phys.push_str(&row("Sidereal day", duration(b.rotation_s)));
        phys.push_str(&row(
            "Rotation Velocity",
            format!("{:.1} m/s", std::f64::consts::TAU * r / b.rotation_s),
        ));
    } else if !b.is_star {
        phys.push_str(&row("Rotation", "tidally locked".into()));
    }
    match b.atmosphere {
        Some((h, p, o2)) => {
            phys.push_str(&row("Atmosphere", format!("{} high", km(h))));
            if p > 0.0 {
                phys.push_str(&row("Surface pressure", format!("{p:.2} atm")));
            }
            phys.push_str(&row("Oxygen Present", if o2 { "yes" } else { "no" }.into()));
        }
        None if !b.is_star => phys.push_str(&row("Atmosphere", "none".into())),
        None => {}
    }
    let mut orb = String::new();
    if let Some((parent, mu_p, a, e, inc)) = &b.orbit {
        let period = std::f64::consts::TAU * (a.powi(3) / mu_p).sqrt();
        orb.push_str(&row("Orbits", esc(parent)));
        orb.push_str(&row("Semi-Major Axis", km(*a)));
        orb.push_str(&row("Eccentricity", format!("{e:.3}")));
        orb.push_str(&row("Inclination", format!("{inc:.2}\u{b0}")));
        orb.push_str(&row("Sidereal Period", duration(period)));
        orb.push_str(&row("Velocity", format!("{:.0} m/s", (mu_p / a).sqrt())));
        if b.rotation_s > 0.0 && (period - b.rotation_s).abs() > 1.0 {
            let solar = 1.0 / (1.0 / b.rotation_s - 1.0 / period);
            phys.push_str(&row("Solar Day", duration(solar.abs())));
        }
    }
    format!(
        "<h3>{name}</h3><div class=\"ts-owner\">Celestial Body</div>{syn}\
         <h4>Physical Characteristics</h4><dl>{phys}</dl>{orb}\
         <div class=\"ts-actions\"><button data-ose=\"ts-body-focus\" data-ose-ts-body-focus=\"{index}\">Focus</button></div>",
        name = esc(&b.name),
        syn = if b.synopsis.is_empty() {
            String::new()
        } else {
            format!("<p class=\"ts-synopsis\">{}</p>", esc(&b.synopsis))
        },
        orb = if orb.is_empty() {
            String::new()
        } else {
            format!("<h4>Orbital Characteristics</h4><dl>{orb}</dl>")
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kerbin_card_reads_like_the_original() {
        let card = BodyCard {
            name: "Kerbin".into(),
            radius_m: 600_000.0,
            mu: 3.5316e12,
            soi_m: Some(84_159_286.0),
            rotation_s: 21_549.425,
            atmosphere: Some((70_000.0, 1.0, true)),
            orbit: Some(("Kerbol".into(), 1.172_332_8e18, 13_599_840_256.0, 0.0, 0.0)),
            ..Default::default()
        };
        let html = render_body_info(&card, 3);
        assert!(
            html.contains("<dt>Gravity</dt><dd>9.81 m/s^2</dd>"),
            "{html}"
        );
        assert!(
            html.contains("<dt>Escape velocity</dt><dd>3431 m/s</dd>"),
            "{html}"
        );
        assert!(html.contains("Oxygen Present</dt><dd>yes"));
        // Kerbin's year is 426 days and half an hour; its solar day 6 h.
        assert!(
            html.contains("<dt>Sidereal Period</dt><dd>426d 0.5h</dd>"),
            "{html}"
        );
        assert!(
            html.contains("<dt>Solar Day</dt><dd>1d 0.0h</dd>"),
            "{html}"
        );
        assert!(html.contains("data-ose-ts-body-focus=\"3\""));
        assert_eq!(duration(5400.0), "1h 30m");
    }

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
        let html = render_list(&rows, Some(2), false, &[]);
        assert!(html.find("Zulu").unwrap() < html.find("Alpha").unwrap());
        assert!(!render_list(&rows, None, true, &[]).contains("Alpha"));
        let mine = render_info(&rows[1]);
        assert!(!mine.contains("data-ose-ts-fly=\"2\" disabled"));
        assert!(!mine.contains("data-ose-ts-recover=\"2\" disabled"));
        let theirs = render_info(&rows[0]);
        assert!(theirs.contains("data-ose-ts-fly=\"1\" disabled"));
        assert_eq!(km(1_500.0), "1500 m");
        assert_eq!(km(85_000.0), "85.0 km");
    }
}

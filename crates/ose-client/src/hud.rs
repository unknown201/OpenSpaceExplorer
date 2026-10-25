//! Flight HUD instruments after the original's (`KSP.UI.Flight/*Instrument.cs`): the SAS mode buttons around
//! the navball, the speed mode toggle, the apoapsis/periapsis readout and the action bar with its state lights.

use ose_net::{AttitudeMode, ChuteState, FlightReadout, OrbitElements};
use std::f64::consts::{PI, TAU};
use web_sys::Document;

/// SAS buttons in the original's order (`NavballInstrument`), with the mode each selects. Prograde and
/// retrograde follow the speed mode (surface or orbit) as the original's navball does. Target, anti-target
/// and maneuver modes have no server counterpart yet and stay disabled.
pub const SAS_BUTTONS: &[(&str, &str, &str)] = &[
    (
        "fh-sas-stability",
        "Flight-ICO-StabilityAssist",
        "Stability assist: hold the current direction (T toggles SAS on and off)",
    ),
    (
        "fh-sas-prograde",
        "Flight-ICO-Prograde",
        "Prograde: point along the velocity (surface or orbit, as the speed readout)",
    ),
    (
        "fh-sas-retrograde",
        "Flight-ICO-Retrograde",
        "Retrograde: point against the velocity",
    ),
    (
        "fh-sas-normal",
        "Flight-ICO-Normal",
        "Normal: square to the orbit plane",
    ),
    ("fh-sas-antinormal", "Flight-ICO-AntiNormal", "Anti-normal"),
    (
        "fh-sas-radialin",
        "Flight-ICO-RadialIn",
        "Radial in: toward the body",
    ),
    (
        "fh-sas-radialout",
        "Flight-ICO-RadialOut",
        "Radial out: away from the body",
    ),
    (
        "fh-sas-target",
        "Flight-ICO-Target",
        "Target: point at the target (set one by right-clicking a vessel on the map)",
    ),
    (
        "fh-sas-antitarget",
        "Flight-ICO-AntiTarget",
        "Anti-target: point away from the target",
    ),
    (
        "fh-sas-maneuver",
        "Flight-ICO-Maneuver",
        "Maneuver: point along the planned burn",
    ),
];

/// The attitude a SAS button asks for; `None` for stability assist (the caller holds the current facing) and
/// the disabled modes.
pub fn sas_mode(id: &str, orbit_speed: bool) -> Option<AttitudeMode> {
    Some(match (id, orbit_speed) {
        ("fh-sas-prograde", false) => AttitudeMode::SurfacePrograde,
        ("fh-sas-prograde", true) => AttitudeMode::OrbitPrograde,
        ("fh-sas-retrograde", false) => AttitudeMode::SurfaceRetrograde,
        ("fh-sas-retrograde", true) => AttitudeMode::OrbitRetrograde,
        ("fh-sas-normal", _) => AttitudeMode::Normal,
        ("fh-sas-antinormal", _) => AttitudeMode::AntiNormal,
        ("fh-sas-radialin", _) => AttitudeMode::RadialIn,
        ("fh-sas-radialout", _) => AttitudeMode::RadialOut,
        ("fh-sas-maneuver", _) => AttitudeMode::Maneuver,
        _ => return None,
    })
}

/// Which SAS button is lit for an attitude target.
pub fn sas_lit(att: &AttitudeMode) -> &'static str {
    match att {
        AttitudeMode::SurfacePrograde | AttitudeMode::OrbitPrograde => "fh-sas-prograde",
        AttitudeMode::SurfaceRetrograde | AttitudeMode::OrbitRetrograde => "fh-sas-retrograde",
        AttitudeMode::Normal => "fh-sas-normal",
        AttitudeMode::AntiNormal => "fh-sas-antinormal",
        AttitudeMode::RadialIn => "fh-sas-radialin",
        AttitudeMode::RadialOut => "fh-sas-radialout",
        AttitudeMode::Maneuver => "fh-sas-maneuver",
        // SAS off: no mode button lit
        AttitudeMode::Free => "",
        AttitudeMode::Up | AttitudeMode::PitchHeading { .. } | AttitudeMode::Fixed(_) => {
            "fh-sas-stability"
        }
    }
}

/// Action bar buttons (`ActionBar.cs`): id, icon, tooltip, enabled.
pub const ACTIONS: &[(&str, &str, &str, bool)] = &[
    (
        "fh-act-gear",
        "ICO-Action-LandingGear-24x24",
        "Landing gear (G)",
        true,
    ),
    (
        "fh-act-brakes",
        "ICO-Action-Brakes-24x24",
        "Brakes (hold B, click to latch)",
        true,
    ),
    (
        "fh-act-lights",
        "ICO-Action-Lights-24x24",
        "Lights (U)",
        true,
    ),
    ("fh-act-rcs", "ICO-Action-Jetpack-24x24", "RCS (R)", true),
    (
        "fh-act-solar",
        "ICO-Action-Solar-24x24",
        "Solar panels",
        true,
    ),
    (
        "fh-act-chutes",
        "Staging-ICO-Chute-Stowed",
        "Arm the parachutes",
        true,
    ),
    (
        "fh-act-science",
        "ICO-AppBar-ResearchManager-24x24",
        "Run every experiment on board",
        true,
    ),
    (
        "fh-act-converters",
        "ICO-AppBar-ActionGroup-24x24",
        "Fuel cells and reactors on or off",
        true,
    ),
    (
        "fh-act-abort",
        "ICO-Action-Abort-24x24",
        "Abort: cut the throttle and arm the parachutes",
        true,
    ),
];

/// Altitude above the surface and seconds until the apsides: (apoapsis, periapsis), each `(altitude m,
/// seconds from now)`. No apoapsis on an escape trajectory; the periapsis time is negative once passed on one.
pub fn apsides(
    o: &OrbitElements,
    radius_m: f64,
    mu: f64,
    ut: f64,
) -> (Option<(f64, f64)>, (f64, f64)) {
    let (a, e) = (o.semi_major_axis_m, o.eccentricity);
    let pe_alt = a * (1.0 - e) - radius_m;
    let n = (mu / a.abs().powi(3)).sqrt();
    if !(n.is_finite() && n > 0.0) {
        return (None, (pe_alt, f64::NAN));
    }
    let m = o.mean_anomaly_at_epoch_rad + n * (ut - o.epoch_s);
    if e >= 1.0 || a <= 0.0 {
        return (None, (pe_alt, -m / n));
    }
    let m = m.rem_euclid(TAU);
    let t_pe = (TAU - m).rem_euclid(TAU) / n;
    let t_ap = (PI - m).rem_euclid(TAU) / n;
    (Some((a * (1.0 + e) - radius_m, t_ap)), (pe_alt, t_pe))
}

pub fn fmt_dist(m: f64) -> String {
    if !m.is_finite() {
        "--".into()
    } else if m.abs() >= 1.0e9 {
        format!("{:.2} Gm", m / 1.0e9)
    } else if m.abs() >= 1.0e6 {
        format!("{:.2} Mm", m / 1.0e6)
    } else if m.abs() >= 1.0e4 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{m:.0} m")
    }
}

/// Kerbin time: 6-hour days, as the original's countdowns (`T-` before, `T+` after).
pub fn fmt_dt(s: f64) -> String {
    if !s.is_finite() {
        return "--".into();
    }
    let sign = if s < 0.0 { "T+" } else { "T-" };
    let t = s.abs().round() as u64;
    let (d, h, m, sec) = (t / 21_600, t % 21_600 / 3600, t % 3600 / 60, t % 60);
    if d > 0 {
        format!("{sign}{d}d {h}h {m:02}m")
    } else if h > 0 {
        format!("{sign}{h}:{m:02}:{sec:02}")
    } else {
        format!("{sign}{m}:{sec:02}")
    }
}

/// Fill the SAS column and the action bar with their buttons (icons masked in the text colour, tooltips naming
/// the key). Clicks are handled by the caller on the containers `#fh-sas` and `#fh-actions`.
pub fn install(doc: &Document) {
    let fill = |container: &str, items: &mut dyn Iterator<Item = (&str, &str, &str, bool)>| {
        let Some(c) = doc.get_element_by_id(container) else {
            return;
        };
        let mut html = String::new();
        for (id, icon, tip, enabled) in items {
            html.push_str(&format!(
                "<button id=\"{id}\" data-ose=\"{id}\" title=\"{tip}\"{}><i style=\"--ico:url(assets/ui/hud/{icon}.png)\"></i></button>",
                if enabled { "" } else { " disabled" }
            ));
        }
        c.set_inner_html(&html);
    };
    fill(
        "fh-sas",
        &mut SAS_BUTTONS.iter().map(|(id, icon, tip)| {
            (
                *id,
                *icon,
                *tip,
                sas_mode(id, false).is_some() || *id == "fh-sas-stability",
            )
        }),
    );
    fill(
        "fh-actions",
        &mut ACTIONS
            .iter()
            .map(|(id, icon, tip, on)| (*id, *icon, *tip, *on)),
    );
}

fn set(doc: &Document, id: &str, text: &str) {
    if let Some(el) = doc.get_element_by_id(id) {
        if el.text_content().as_deref() != Some(text) {
            el.set_text_content(Some(text));
        }
    }
}

fn lit(doc: &Document, id: &str, on: bool) {
    if let Some(el) = doc.get_element_by_id(id) {
        el.class_list().toggle_with_force("on", on).ok();
    }
}

/// Everything the new instruments show, refreshed every frame from the vessel's readout.
pub struct HudState<'a> {
    pub f: &'a FlightReadout,
    pub attitude: &'a AttitudeMode,
    pub orbit_speed: bool,
    pub orbital_speed_mps: f64,
    pub apsides: (Option<(f64, f64)>, (f64, f64)),
    pub brakes_latched: bool,
}

pub fn update(doc: &Document, s: &HudState) {
    let f = s.f;
    set(
        doc,
        "fh-speed-mode",
        if s.orbit_speed { "ORBIT" } else { "SURFACE" },
    );
    if s.orbit_speed {
        set(doc, "fh-speed", &format!("{:.1} m/s", s.orbital_speed_mps));
    }
    if let Some(bar) = doc
        .get_element_by_id("fh-fuel-fill")
        .and_then(|e| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(e).ok())
    {
        bar.style()
            .set_property(
                "width",
                &format!("{:.0}%", (f.propellant_fraction * 100.0).clamp(0.0, 100.0)),
            )
            .ok();
    }
    set(
        doc,
        "fh-throttle-pct",
        &format!("{:.0}%", f.throttle * 100.0),
    );
    let (ap, (pe_alt, pe_t)) = s.apsides;
    match ap {
        Some((alt, t)) => {
            set(doc, "fh-ap", &fmt_dist(alt));
            set(doc, "fh-ap-t", &fmt_dt(t));
        }
        None => {
            set(doc, "fh-ap", "escape");
            set(doc, "fh-ap-t", "");
        }
    }
    set(doc, "fh-pe", &fmt_dist(pe_alt));
    set(doc, "fh-pe-t", &fmt_dt(pe_t));
    let current = sas_lit(s.attitude);
    for (id, _, _) in SAS_BUTTONS {
        lit(doc, id, *id == current);
    }
    lit(doc, "fh-act-gear", f.gear.iter().any(|(_, out)| *out));
    lit(doc, "fh-act-brakes", f.brakes || s.brakes_latched);
    // Aerodynamics: lift from wings and whether the burning stage is jets (thrust follows speed and air).
    let aero = match (f.lift_kn > 0.1, f.jet) {
        (true, true) => format!("Lift {:.0} kN \u{b7} jets", f.lift_kn),
        (true, false) => format!("Lift {:.0} kN", f.lift_kn),
        (false, true) => "jets".to_string(),
        (false, false) => String::new(),
    };
    set(doc, "fh-aero", &aero);
    lit(doc, "fh-act-solar", f.panels_deployed);
    lit(doc, "fh-act-lights", f.lights);
    lit(doc, "fh-act-rcs", f.rcs);
    lit(doc, "fh-act-abort", f.abort);
    lit(
        doc,
        "fh-act-converters",
        f.converters.iter().any(|(_, on, _)| *on),
    );
    if let Some(b) = doc.get_element_by_id("fh-act-converters") {
        if f.converters.is_empty() {
            b.set_attribute("disabled", "").ok();
        } else {
            b.remove_attribute("disabled").ok();
        }
    }
    // CommNet (the original's signal indicator in the flight cluster): linked to the KSC and over how many
    // relays, or no signal.
    if let Some(el) = doc.get_element_by_id("fh-comm") {
        let (text, cls) = if f.comm_connected {
            (
                if f.comm_hops <= 1 {
                    "Signal: KSC".to_string()
                } else {
                    format!("Signal: KSC via {} relays", f.comm_hops - 1)
                },
                "on",
            )
        } else if f.comm_range_m > 0.0 {
            ("No signal".to_string(), "off")
        } else {
            ("No antenna".to_string(), "off")
        };
        set(doc, "fh-comm-text", &text);
        el.class_list().toggle_with_force("on", cls == "on").ok();
    }
    lit(
        doc,
        "fh-act-chutes",
        f.parachutes
            .iter()
            .any(|(_, st, _)| !matches!(st, ChuteState::Stowed)),
    );
}

/// The flight staging stack (`FlightStagingStackInstrument`): one card per stage still to fire, the next one
/// at the bottom and lit, each with its parts' icons and parachute states.
pub fn staging_html(
    craft: &ose_data::craft::Craft,
    catalog: &crate::vab::Catalog,
    next_stage: usize,
    chutes: &[(u16, ChuteState, f32)],
) -> String {
    let mut html = String::new();
    let stages = &craft.stages;
    // The highest index fires first: firing step k is craft stage len - 1 - k. Cards run from the last to
    // fire (index 0) at the top to the next one at the bottom, numbered from the bottom up.
    let Some(next) = stages.len().checked_sub(next_stage + 1) else {
        return html;
    };
    for (n, stage) in stages.iter().enumerate().take(next + 1) {
        let mut icons = String::new();
        for &i in stage {
            let Some(p) = craft.parts.get(i) else {
                continue;
            };
            let spec = catalog.get(&p.part_id);
            let chute = chutes.iter().find(|(pi, _, _)| *pi as usize == i);
            let icon = match (spec, chute) {
                (_, Some((_, ChuteState::Armed, _))) => "Staging-ICO-Chute-Armed",
                (_, Some((_, ChuteState::SemiDeployed | ChuteState::Deployed, _))) => {
                    "Staging-ICO-Chute-Deployed"
                }
                (_, Some((_, ChuteState::Cut, _))) => "Staging-ICO-Chute-Cut",
                (_, Some(_)) => "Staging-ICO-Chute-Stowed",
                (Some(s), None) if s.has_engine => "Staging-Icon-Thruster",
                (Some(s), None) if s.is_decoupler => "Staging-Icon-Inline",
                (Some(s), None) if s.stage_type.contains("Parachute") => "Staging-ICO-Chute-Stowed",
                _ => "Staging-ICO-Structure01",
            };
            let title = spec.map(|s| s.display_title()).unwrap_or(&p.part_id);
            icons.push_str(&format!(
                "<i title=\"{}\" style=\"--ico:url(assets/ui/hud/{icon}.png)\"></i>",
                title.replace('"', "&quot;")
            ));
        }
        html.push_str(&format!(
            "<li class=\"{}\"><b>{}</b>{icons}</li>",
            if n == next { "next" } else { "" },
            stages.len() - n
        ));
    }
    html
}

/// FNV-1a of a string, to tell rendered HTML apart cheaply.
pub fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// The mission tracker window (`MissionTracker`): accepted missions not yet handed in, each with its current
/// objective and progress, or that it is ready to turn in at Mission Control.
pub fn tracker_html(
    missions: &[ose_net::MissionInfo],
    loc: &std::collections::HashMap<String, String>,
) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let mut rows: Vec<&ose_net::MissionInfo> = missions
        .iter()
        .filter(|m| m.accepted && !m.turned_in)
        .collect();
    rows.sort_by_key(|m| (!m.primary, m.complete));
    let mut html = String::new();
    for m in rows {
        let name = crate::rnd::text(loc, &m.name_key);
        let body = if m.complete {
            "<span class=\"done\">Complete: hand it in at Mission Control</span>".to_string()
        } else {
            format!(
                "{} <small>{}/{}</small>",
                esc(&crate::rnd::text(loc, &m.objective_key)),
                m.stage + 1,
                m.stages.max(1)
            )
        };
        html.push_str(&format!(
            "<li class=\"{}\"><b>{}</b><div>{body}</div></li>",
            if m.primary { "primary" } else { "secondary" },
            esc(&name)
        ));
    }
    if html.is_empty() {
        html = "<li class=\"empty\">No missions in progress. Take one on at Mission Control.</li>"
            .into();
    }
    html
}

/// The burn timer (`BurnTimerInstrument`): time to the node, its delta-v and the burn's length at full
/// throttle, starting half the burn before the node as the original's countdown does. `full_thrust_kn` 0
/// leaves the length unknown.
pub fn burn_text(to_node_s: f64, dv_mps: f64, mass_t: f64, full_thrust_kn: f64) -> String {
    let burn = if full_thrust_kn > 0.0 {
        Some(dv_mps * mass_t / full_thrust_kn)
    } else {
        None
    };
    let start = to_node_s - burn.unwrap_or(0.0) * 0.5;
    match burn {
        Some(b) => format!(
            "Burn {} \u{b7} \u{394}v {:.0} m/s \u{b7} {:.0} s at full throttle",
            fmt_dt(start),
            dv_mps,
            b
        ),
        None => format!(
            "Node {} \u{b7} \u{394}v {:.0} m/s \u{b7} throttle up to time the burn",
            fmt_dt(to_node_s),
            dv_mps
        ),
    }
}

/// The part window in flight (right click a part, the original's PAM): name, temperature, crew, parachute
/// and gear state, and the actions that apply to it (`data-act` names the action bar button they press).
pub fn flight_pam_html(
    title: &str,
    idx: usize,
    f: &FlightReadout,
    is_engine: bool,
    is_solar: bool,
) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let i = idx as u16;
    let mut rows = String::new();
    let mut acts = String::new();
    let row =
        |k: &str, v: String| format!("<div class=\"pam-row\"><span>{k}</span><b>{v}</b></div>");
    if let Some((_, t, max, kw)) = f.heat.iter().find(|h| h.0 == i) {
        rows.push_str(&row("Temperature", format!("{t:.0} / {max:.0} K")));
        if *kw > 0.5 {
            rows.push_str(&row("Re-entry heat", format!("{kw:.0} kW")));
        }
    }
    let crew: Vec<&str> = f
        .crew
        .iter()
        .filter(|c| c.0 == i)
        .map(|c| c.1.as_str())
        .collect();
    if !crew.is_empty() {
        rows.push_str(&row("Crew", esc(&crew.join(", "))));
    }
    if let Some((_, st, open)) = f.parachutes.iter().find(|c| c.0 == i) {
        rows.push_str(&row("Parachute", format!("{st:?} {:.0}%", open * 100.0)));
        if matches!(st, ChuteState::Stowed) {
            acts.push_str("<button data-act=\"fh-act-chutes\">Arm parachutes</button>");
        }
    }
    if let Some((_, out)) = f.gear.iter().find(|g| g.0 == i) {
        rows.push_str(&row("Gear", if *out { "out" } else { "stowed" }.into()));
        acts.push_str(&format!(
            "<button data-act=\"fh-act-gear\">{}</button>",
            if *out { "Retract gear" } else { "Deploy gear" }
        ));
    }
    if let Some((_, on, rate)) = f.converters.iter().find(|c| c.0 == i) {
        rows.push_str(&row(
            "Converter",
            if *on {
                format!("running {:.0}%", rate * 100.0)
            } else {
                "off".into()
            },
        ));
        acts.push_str(&format!(
            "<button data-act=\"fh-act-converters\">{}</button>",
            if *on { "Stop" } else { "Start" }
        ));
    }
    if is_solar {
        rows.push_str(&row(
            "Panels",
            if f.panels_deployed {
                "deployed"
            } else {
                "folded"
            }
            .into(),
        ));
        acts.push_str(&format!(
            "<button data-act=\"fh-act-solar\">{}</button>",
            if f.panels_deployed {
                "Retract panels"
            } else {
                "Extend panels"
            }
        ));
    }
    if is_engine {
        rows.push_str(&row(
            "Stage thrust",
            format!("{:.0} kN at {:.0}%", f.thrust_kn, f.throttle * 100.0),
        ));
    }
    if rows.is_empty() {
        rows.push_str("<div class=\"pam-sub\">Nothing to set on this part.</div>");
    }
    format!(
        "<div class=\"pam-head\"><b>{}</b><button id=\"fpam-close\" title=\"Close (Esc)\">&#10005;</button></div>{rows}<div class=\"pam-acts\">{acts}</div>",
        esc(title)
    )
}

/// Mission elapsed time as the original's vessel status shows it: `1d 02:03:04` (Kerbin days of 6 h).
pub fn fmt_met(s: f64) -> String {
    if !s.is_finite() || s < 0.0 {
        return "--".into();
    }
    let t = s.round() as u64;
    let (d, h, m, sec) = (t / 21_600, t % 21_600 / 3600, t % 3600 / 60, t % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}:{sec:02}")
    } else {
        format!("{h:02}:{m:02}:{sec:02}")
    }
}

/// The flight's records for the flight report (`FlightReportUIManager`): highest altitude, fastest speed,
/// furthest from home is left out (no ground track yet).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlightRecords {
    pub vessel: u64,
    pub name: String,
    pub launch_ut: f64,
    pub max_altitude_m: f64,
    pub max_speed_mps: f64,
    pub bodies: Vec<String>,
}

impl FlightRecords {
    pub fn note(&mut self, altitude_m: f64, speed_mps: f64, body: &str) {
        self.max_altitude_m = self.max_altitude_m.max(altitude_m);
        self.max_speed_mps = self.max_speed_mps.max(speed_mps);
        if !self.bodies.iter().any(|b| b == body) {
            self.bodies.push(body.to_string());
        }
    }
}

/// The flight report window: how the flight ended, its records and the science it brought home.
pub fn flight_report_html(r: &FlightRecords, ended: &str, ut: f64, science: Option<f64>) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let row = |k: &str, v: String| format!("<tr><td>{k}</td><td>{v}</td></tr>");
    let mut rows = String::new();
    if r.launch_ut > 0.0 {
        rows.push_str(&row("Mission time", fmt_met(ut - r.launch_ut)));
    }
    rows.push_str(&row("Highest altitude", fmt_dist(r.max_altitude_m)));
    rows.push_str(&row("Fastest speed", format!("{:.0} m/s", r.max_speed_mps)));
    if !r.bodies.is_empty() {
        rows.push_str(&row("Visited", esc(&r.bodies.join(", "))));
    }
    if let Some(s) = science {
        rows.push_str(&row("Science", format!("{s:.0}")));
    }
    format!(
        "<h3>Flight report</h3><div class=\"fr-name\">{}</div><div class=\"fr-ended\">{}</div><table>{rows}</table><button id=\"fr-close\">Continue</button>",
        esc(&r.name),
        esc(ended)
    )
}

/// What the flight events watch, as last seen: vessel, CommNet link, control, reference body, situation, and
/// whether the charge was low.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlightWatch {
    pub vessel: u64,
    pub comm: bool,
    pub control: bool,
    pub body: u16,
    pub situation: Option<ose_net::FlightSituation>,
    pub charge_low: bool,
}

/// The original's `NotificationEvents` the client can tell from two readouts of the same vessel: lost and
/// regained signal, lost control, out of charge, a new sphere of influence, reaching orbit, landing,
/// splashing down and crashing. Returns (text, tier) for each.
pub fn flight_events(
    before: &FlightWatch,
    now: &FlightWatch,
    body_name: &str,
) -> Vec<(String, &'static str)> {
    use ose_net::FlightSituation as S;
    let mut out = Vec::new();
    if before.vessel != now.vessel {
        return out;
    }
    if before.comm && !now.comm {
        out.push(("Lost connection to the KSC".to_string(), "alert"));
    } else if !before.comm && now.comm {
        out.push(("Connection to the KSC restored".to_string(), "passive"));
    }
    if before.control && !now.control {
        out.push((
            "No control: the probe has no charge or signal".to_string(),
            "alert",
        ));
    }
    if !before.charge_low && now.charge_low {
        out.push(("Electric charge is running out".to_string(), "alert"));
    }
    if before.body != now.body {
        out.push((
            format!("Entered {body_name}'s sphere of influence"),
            "passive",
        ));
    }
    if before.situation != now.situation {
        match now.situation {
            Some(S::Orbit) => out.push((format!("Stable orbit around {body_name}"), "triumph")),
            Some(S::Landed)
                if before.situation == Some(S::Flying)
                    || before.situation == Some(S::SubOrbital) =>
            {
                out.push((format!("Landed on {body_name}"), "triumph"))
            }
            Some(S::Crashed) => out.push(("The vessel crashed".to_string(), "alert")),
            Some(S::Escaping) => out.push((format!("Escaping {body_name}"), "passive")),
            _ => {}
        }
    }
    out
}

/// Notification toasts (`NotificationManager`): short messages stacked at the top right that fade after a few
/// seconds. Flight messages still land in `#fh-note` (the tests read it); `watch` turns each new one into a toast.
#[derive(Default)]
pub struct Toasts {
    seen: std::collections::HashMap<String, String>,
}

impl Toasts {
    /// Toast the text of element `id` when it changes to something new and non-empty.
    pub fn watch(&mut self, doc: &Document, id: &str, tier: &str) {
        let text = doc
            .get_element_by_id(id)
            .and_then(|e| e.text_content())
            .unwrap_or_default();
        let last = self.seen.entry(id.to_string()).or_default();
        if *last != text {
            *last = text.clone();
            if !text.trim().is_empty() {
                push(doc, &text, tier);
            }
        }
    }
}

/// Show one toast; `tier` is a class: `alert`, `passive` or `triumph`, as the original's tiers.
pub fn push(doc: &Document, text: &str, tier: &str) {
    let Some(stack) = doc.get_element_by_id("toasts") else {
        return;
    };
    let Ok(el) = doc.create_element("div") else {
        return;
    };
    el.set_class_name(&format!("toast {tier}"));
    el.set_attribute("data-ose", "toast").ok();
    el.set_text_content(Some(text));
    stack.append_child(&el).ok();
    // At most four at once; the oldest goes first.
    while stack.child_element_count() > 4 {
        if let Some(first) = stack.first_element_child() {
            first.remove();
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let gone = el.clone();
        let cb = wasm_bindgen::closure::Closure::once_into_js(move || gone.remove());
        if let Some(w) = web_sys::window() {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), 6000)
                .ok();
        }
    }
}

/// The crew portraits (`KerbalPortraits`, bottom right in flight): each Kerbal aboard with a name plate and
/// an EVA button that sends that Kerbal out.
/// `face` draws a Kerbal's portrait by name (from the roster); without one the generic Kerbal icon shows.
pub fn crew_html(
    crew: &[(u16, String)],
    can_eva: bool,
    face: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut html = String::new();
    for (_, name) in crew {
        let n = crate::esc(name);
        let eva = if can_eva {
            format!(
                "<button data-ose=\"crew-eva\" data-eva=\"{n}\" title=\"{n} climbs out\">EVA</button>"
            )
        } else {
            String::new()
        };
        html.push_str(&format!(
            "<div class=\"fh-kerbal\" data-ose=\"crew-portrait\"><div class=\"pic\">{pic}{eva}</div><div class=\"plate\" title=\"{n}\">{n}</div></div>",
            pic = face(name).unwrap_or_else(|| {
                "<i style=\"--ico:url(assets/ui/hud/WB-ICO-Kerbal.png)\"></i>".into()
            })
        ));
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orbit(a: f64, e: f64, m0: f64) -> OrbitElements {
        OrbitElements {
            semi_major_axis_m: a,
            eccentricity: e,
            inclination_deg: 0.0,
            longitude_of_ascending_node_deg: 0.0,
            argument_of_periapsis_deg: 0.0,
            mean_anomaly_at_epoch_rad: m0,
            epoch_s: 0.0,
        }
    }

    #[test]
    fn apsides_of_an_ellipse() {
        // Kerbin: radius 600 km, mu 3.5316e12. 100 x 80 km orbit, at periapsis now.
        let (r, mu) = (600_000.0, 3.5316e12);
        let a = r + 90_000.0;
        let e = 10_000.0 / a;
        let (ap, pe) = apsides(&orbit(a, e, 0.0), r, mu, 0.0);
        let (ap_alt, ap_t) = ap.unwrap();
        assert!((ap_alt - 100_000.0).abs() < 1.0);
        assert!((pe.0 - 80_000.0).abs() < 1.0);
        let period = TAU * (a.powi(3) / mu).sqrt();
        assert!((ap_t - period / 2.0).abs() < 1e-6);
        assert!(pe.1.abs() < 1e-6 || (pe.1 - period).abs() < 1e-6);
    }

    #[test]
    fn hyperbola_has_no_apoapsis() {
        let (ap, pe) = apsides(&orbit(-1.0e6, 1.5, 0.5), 600_000.0, 3.5316e12, 0.0);
        assert!(ap.is_none());
        assert!(pe.1 < 0.0);
    }

    #[test]
    fn burn_starts_half_a_burn_early() {
        // 100 m/s on 10 t with 50 kN: 20 s burn, starting 10 s before the node.
        let t = burn_text(70.0, 100.0, 10.0, 50.0);
        assert!(t.starts_with("Burn T-1:00"), "{t}");
        assert!(t.contains("20 s"));
        assert!(burn_text(70.0, 100.0, 10.0, 0.0).starts_with("Node T-1:10"));
    }

    #[test]
    fn flight_events_from_two_readouts() {
        use ose_net::FlightSituation as S;
        let a = FlightWatch {
            vessel: 1,
            comm: true,
            control: true,
            body: 1,
            situation: Some(S::SubOrbital),
            charge_low: false,
        };
        let b = FlightWatch {
            comm: false,
            situation: Some(S::Orbit),
            ..a.clone()
        };
        let ev = flight_events(&a, &b, "Kerbin");
        assert!(ev
            .iter()
            .any(|(t, k)| t == "Lost connection to the KSC" && *k == "alert"));
        assert!(ev.iter().any(|(t, _)| t == "Stable orbit around Kerbin"));
        // Another vessel: nothing (switching is not an event).
        let c = FlightWatch {
            vessel: 2,
            ..b.clone()
        };
        assert!(flight_events(&a, &c, "Kerbin").is_empty());
    }

    #[test]
    fn records_keep_the_highest_and_fastest() {
        let mut r = FlightRecords::default();
        r.note(1000.0, 200.0, "Kerbin");
        r.note(500.0, 900.0, "Kerbin");
        r.note(80_000.0, 2200.0, "Mun");
        assert_eq!(r.max_altitude_m, 80_000.0);
        assert_eq!(r.max_speed_mps, 2200.0);
        assert_eq!(r.bodies, vec!["Kerbin", "Mun"]);
        assert_eq!(fmt_met(3725.0), "01:02:05");
        assert_eq!(fmt_met(21_600.0 + 61.0), "1d 00:01:01");
        assert!(flight_report_html(&r, "Recovered", 100.0, Some(12.0)).contains("Fastest speed"));
    }

    #[test]
    fn countdowns_in_kerbin_days() {
        assert_eq!(fmt_dt(65.0), "T-1:05");
        assert_eq!(fmt_dt(3725.0), "T-1:02:05");
        assert_eq!(fmt_dt(21_600.0 + 7200.0 + 60.0), "T-1d 2h 01m");
        assert_eq!(fmt_dt(-5.0), "T+0:05");
    }

    #[test]
    fn sas_buttons_round_trip() {
        for (id, _, _) in SAS_BUTTONS {
            if let Some(m) = sas_mode(id, false) {
                assert_eq!(sas_lit(&m), *id);
            }
        }
    }
}

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
        "Stability assist: hold the current direction (T)",
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
        "Target (not available yet)",
    ),
    (
        "fh-sas-antitarget",
        "Flight-ICO-AntiTarget",
        "Anti-target (not available yet)",
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

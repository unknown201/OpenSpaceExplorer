//! Settings (`KSP.UI/SettingsMenuManager.cs`): the tabs this client can honour. Input lists the original's
//! default bindings and says which ones work here; Interface has the UI scale and the key hints; Audio the
//! sound switch. Values persist per browser in localStorage.

/// One row of the input list: key, action, works in this client.
pub type Binding = (&'static str, &'static str, bool);

pub const INPUT: &[(&str, &[Binding])] = &[
    (
        "Flight",
        &[
            ("W / S", "Pitch down / up (held)", true),
            ("A / D", "Yaw left / right (held)", true),
            ("Q / E", "Roll (held)", true),
            ("Caps Lock", "Precision controls", true),
            ("Shift / Ctrl", "Throttle up / down (held)", true),
            ("Z / X", "Full throttle / cut", true),
            ("Space", "Stage", true),
            (
                "T",
                "SAS on / off (off: the keys turn the craft directly and it keeps spinning)",
                true,
            ),
            ("R", "RCS", true),
            ("G", "Landing gear", true),
            ("B", "Brakes (held)", true),
            ("U", "Lights", true),
            ("1 - 0", "Custom action groups", true),
            ("W / S, A / D landed", "Wheel throttle and steering", true),
            (
                "J / L, I / K, H / N",
                "RCS translate right / left, up / down, fore / aft (with RCS on)",
                true,
            ),
            (
                "Delete",
                "Docking mode: W/A/S/D and Shift/Ctrl translate",
                true,
            ),
            (
                "W / S, A / D, Q / E (on EVA)",
                "Walk, turn, step sideways",
                true,
            ),
            (
                "Shift / Space (on EVA)",
                "Run / jump; in the air the jetpack up",
                true,
            ),
            ("Ctrl (on EVA)", "Jetpack down", true),
            ("F (on EVA)", "Board the craft within reach", true),
            ("M", "Map view", true),
            ("Arrow keys", "Turn the flight camera", true),
            (
                "Tab",
                "Mouselook: the flight camera follows the mouse",
                true,
            ),
            ("V", "Camera mode: auto, free, orbital, chase, locked", true),
            (
                "] / [",
                "Next / previous vessel: take control nearby in flight, follow on the map",
                true,
            ),
            (
                "Right click a vessel (map)",
                "Set or clear the target",
                true,
            ),
            ("Right mouse drag", "Look around", true),
            (
                "Right click a part",
                "Part window: temperature, crew, chutes, gear, panels",
                true,
            ),
            ("Mouse wheel", "Zoom", true),
            ("Home", "Reset the camera", true),
            ("F4", "Vessel labels", true),
        ],
    ),
    (
        "Vehicle Assembly Building",
        &[
            (
                "Left drag",
                "Pick up a part (with everything under it) and attach it",
                true,
            ),
            ("Right drag", "Orbit the camera", true),
            ("Mouse wheel / Shift", "Zoom / faster", true),
            ("Home", "Reset the camera", true),
            (
                "Up / Down arrows",
                "Move the camera up and down the craft",
                true,
            ),
            ("X / Shift+X", "Symmetry", true),
            ("C", "Angle snap", true),
            ("Delete / Backspace", "Remove the selected part", true),
            ("Ctrl+Z / Ctrl+Y", "Undo / redo", true),
            ("Ctrl+F", "Search parts", true),
            ("Ctrl+N / Ctrl+S", "New vehicle / save", true),
            (
                "Ctrl+C / Ctrl+V",
                "Copy the selected part and what hangs on it / paste (click to place, Esc drops)",
                true,
            ),
            (
                "W A S D Q E / Space",
                "Rotate the held part (90 degrees with snap, 15 without, Shift 5) / upright",
                true,
            ),
        ],
    ),
    (
        "Global",
        &[
            ("Esc", "Pause menu", true),
            (". / , / /", "Time warp up / down / stop", true),
            ("F2", "Hide the interface", true),
            ("Numpad 1", "Mute the game", true),
            ("Numpad 2", "Mute the music", true),
            ("F5 / F9", "Quicksave / quickload", true),
            ("M (at the KSC)", "Tracking Station", true),
        ],
    ),
];

pub fn input_html() -> String {
    let mut html = String::new();
    for (group, rows) in INPUT {
        html.push_str(&format!("<h4>{group}</h4><table>"));
        for (key, action, works) in rows.iter() {
            html.push_str(&format!(
                "<tr{}><td><kbd>{key}</kbd></td><td>{action}{}</td></tr>",
                if *works { "" } else { " class=\"off\"" },
                if *works {
                    ""
                } else {
                    " <small>(not yet)</small>"
                },
            ));
        }
        html.push_str("</table>");
    }
    html
}

/// Render resolution as a share of the screen's pixels from the stored percentage (50..100, default 100).
pub fn render_scale(stored: Option<&str>) -> f64 {
    stored
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(100.0)
        .clamp(50.0, 100.0)
        / 100.0
}

/// Planet detail (Settings > Graphics): 0 low, 1 medium, 2 high (the default) from the stored value.
pub fn planet_quality(stored: Option<&str>) -> u8 {
    match stored {
        Some("low") => 0,
        Some("medium") => 1,
        _ => 2,
    }
}

/// UI scale from the stored percentage (50..150, default 100).
pub fn ui_scale(stored: Option<&str>) -> f64 {
    stored
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(100.0)
        .clamp(50.0, 150.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_is_clamped_and_defaults() {
        assert_eq!(ui_scale(None), 100.0);
        assert_eq!(ui_scale(Some("300")), 150.0);
        assert_eq!(ui_scale(Some("x")), 100.0);
    }

    #[test]
    fn planet_quality_defaults_to_high() {
        assert_eq!(planet_quality(None), 2);
        assert_eq!(planet_quality(Some("low")), 0);
        assert_eq!(planet_quality(Some("medium")), 1);
        assert_eq!(planet_quality(Some("bogus")), 2);
    }

    #[test]
    fn input_list_names_every_group() {
        let html = input_html();
        assert!(html.contains("Vehicle Assembly Building"));
        assert!(html.contains("RCS translate"));
    }
}

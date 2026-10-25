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
            ("Q / E", "Roll", false),
            ("Caps Lock", "Precision controls", true),
            ("Shift / Ctrl", "Throttle up / down (held)", true),
            ("Z / X", "Full throttle / cut", true),
            ("Space", "Stage", true),
            ("T", "SAS: hold the current direction", true),
            ("R", "RCS", false),
            ("G", "Landing gear", true),
            ("B", "Brakes (held)", true),
            ("U", "Lights", false),
            ("1 - 0", "Action groups", false),
            ("W / S, A / D landed", "Wheel throttle and steering", true),
            ("I J K L H N", "Translate (RCS)", false),
            ("M", "Map view", true),
            ("V", "Camera mode", false),
            ("] / [", "Next / previous vessel", false),
            ("Right mouse drag", "Look around", true),
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
            ("Delete / Backspace", "Remove the selected part", true),
            ("Ctrl+Z / Ctrl+Y", "Undo / redo", true),
            ("Ctrl+F", "Search parts", true),
            ("Ctrl+N / Ctrl+S", "New vehicle / save", true),
            ("Ctrl+C / Ctrl+V", "Copy / paste", false),
            ("W A S D Q E", "Rotate the held part", false),
            ("X / Shift+X", "Symmetry", false),
            ("C", "Angle snap", false),
        ],
    ),
    (
        "Global",
        &[
            ("Esc", "Pause menu", true),
            (". / , / /", "Time warp up / down / stop", true),
            ("F2", "Hide the interface", true),
            ("F5 / F9", "Quicksave / quickload", false),
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
    fn input_list_names_every_group() {
        let html = input_html();
        assert!(html.contains("Vehicle Assembly Building"));
        assert!(html.contains("(not yet)"));
    }
}

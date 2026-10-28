//! Held-key state and the original's flight key bindings (`KSP.Input/GameInput.cs`, the embedded input asset).
//!
//! Keys are tracked by `KeyboardEvent.code` (the physical key), as the original binds keys by position. The
//! flight keys that act while held (rotation, throttle, wheels) are read every frame from [`Held`]; the ones
//! that act once (stage, gear, map) stay in the keydown handler.

use std::collections::HashSet;

/// Degrees per second the attitude target turns while a rotation key is held; CapsLock precision mode
/// scales inputs by 0.1 as `FlightInputHandler.inputSensitivityScale` does.
pub const TURN_RATE_DEG_S: f64 = 8.0;
/// Keyboard rotation authority in every SAS mode; precision mode reduces this by a further factor of ten.
pub const ROTATION_SCALE: f64 = 0.3;
pub const PRECISION_SCALE: f64 = 0.1;
/// Throttle change per second with Shift or Ctrl held (`FlightInputHandler.UpdateMainThrottle`: input x dt).
pub const THROTTLE_RATE: f64 = 1.0;

/// Continuous controls are small intents; their cadence is independent of world snapshot bandwidth.
pub const CONTROL_INTERVAL_MS: f64 = 50.0;

#[derive(Default)]
pub struct ControlCadence {
    next_send_ms: Option<f64>,
    axes: [f64; 3],
}

impl ControlCadence {
    /// Send starts, reversals and releases immediately, and coalesce intermediate held-key changes.
    pub fn due(&mut self, now_ms: f64, dirty: bool, axes: [f64; 3]) -> bool {
        let edge = axes != self.axes;
        self.axes = axes;
        let due = dirty && (edge || self.next_send_ms.is_none_or(|t| now_ms >= t));
        if due {
            // Keep the cadence anchored when a render frame lands just after the deadline. Scheduling
            // from that late frame every time turns 20 Hz into 15 Hz on a 60 Hz display.
            self.next_send_ms = Some(match self.next_send_ms {
                Some(deadline) if !edge && now_ms - deadline < CONTROL_INTERVAL_MS => {
                    deadline + CONTROL_INTERVAL_MS
                }
                _ => now_ms + CONTROL_INTERVAL_MS,
            });
        }
        due
    }

    pub fn sent(&mut self, now_ms: f64) {
        self.next_send_ms = Some(now_ms + CONTROL_INTERVAL_MS);
    }
}

#[derive(Default)]
pub struct Held {
    codes: HashSet<String>,
    /// CapsLock toggles precision mode (`TogglePrecisionMode`).
    pub precision: bool,
}

impl Held {
    pub fn down(&mut self, code: &str) {
        self.codes.insert(code.to_string());
    }
    pub fn up(&mut self, code: &str) {
        self.codes.remove(code);
    }
    /// Forget everything (window blur, a text field took focus): no key stays stuck down.
    pub fn clear(&mut self) {
        self.codes.clear();
    }
    pub fn is(&self, code: &str) -> bool {
        self.codes.contains(code)
    }
    fn axis(&self, neg: &[&str], pos: &[&str]) -> f64 {
        let n = neg.iter().any(|c| self.is(c)) as i32;
        let p = pos.iter().any(|c| self.is(c)) as i32;
        (p - n) as f64
    }
    /// The original's flight axes: pitch W (-1, nose down) / S, yaw A (-1) / D, roll Q (-1) / E.
    pub fn pitch(&self) -> f64 {
        self.axis(&["KeyW"], &["KeyS"])
    }
    pub fn yaw(&self) -> f64 {
        self.axis(&["KeyA"], &["KeyD"])
    }
    pub fn roll(&self) -> f64 {
        self.axis(&["KeyQ"], &["KeyE"])
    }
    /// ThrottleDelta: left Shift up, left Ctrl down (either side accepted).
    pub fn throttle(&self) -> f64 {
        self.axis(
            &["ControlLeft", "ControlRight"],
            &["ShiftLeft", "ShiftRight"],
        )
    }
    /// WheelThrottle W (+1) / S and WheelSteer A (-1) / D.
    pub fn wheel_throttle(&self) -> f64 {
        self.axis(&["KeyS"], &["KeyW"])
    }
    pub fn wheel_steer(&self) -> f64 {
        self.axis(&["KeyA"], &["KeyD"])
    }
    /// RCS translation (the original's TranslateX J/L, fore/aft H/N, up/down I/K): right, fore and top in
    /// the vessel's frame, as protocol 27's `Translate` takes them.
    pub fn translate(&self) -> (f64, f64, f64) {
        (
            self.axis(&["KeyJ"], &["KeyL"]),
            self.axis(&["KeyN"], &["KeyH"]),
            self.axis(&["KeyK"], &["KeyI"]),
        )
    }
    /// WheelBrakes is held, not toggled.
    pub fn brakes(&self) -> bool {
        self.is("KeyB")
    }
    pub fn rotation_scale(&self) -> f64 {
        ROTATION_SCALE * self.scale()
    }
    pub fn scale(&self) -> f64 {
        if self.precision {
            PRECISION_SCALE
        } else {
            1.0
        }
    }
}

/// Turn a pitch-and-heading target (degrees above the horizon, compass heading with 90 = east) by a pitch and
/// a yaw step (degrees), in the vessel's own sense: pitch tilts the nose along its heading toward or away from
/// the horizon, yaw swings it sideways about the nose's "top" (the way it would tilt next). Near vertical a yaw
/// tilts the rocket sideways rather than spinning an invisible heading, as a real yaw does. Positive yaw turns
/// right (clockwise seen from above), positive pitch raises the nose.
pub fn turn(pitch_deg: f64, heading_deg: f64, dpitch: f64, dyaw: f64) -> (f64, f64) {
    // East-north-up: heading h is the horizontal direction, up is +Z.
    let (p, h) = (pitch_deg.to_radians(), heading_deg.to_radians());
    let hor = [h.sin(), h.cos(), 0.0];
    let up = [0.0, 0.0, 1.0];
    let lin = |a: [f64; 3], b: [f64; 3], x: f64, y: f64| {
        [
            a[0] * x + b[0] * y,
            a[1] * x + b[1] * y,
            a[2] * x + b[2] * y,
        ]
    };
    // Pitch first, within the vertical plane of the heading; the plane may flip over the top.
    let p2 = p + dpitch.to_radians();
    let d = lin(hor, up, p2.cos(), p2.sin());
    let top = lin(hor, up, -p2.sin(), p2.cos());
    // top x d = -right, so a positive (rightward) yaw rotates d toward -(top x d) = right.
    let right = [
        -(top[1] * d[2] - top[2] * d[1]),
        -(top[2] * d[0] - top[0] * d[2]),
        -(top[0] * d[1] - top[1] * d[0]),
    ];
    let y = dyaw.to_radians();
    let d = lin(d, right, y.cos(), y.sin());
    let horiz = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let pitch = d[2].clamp(-1.0, 1.0).asin().to_degrees();
    let heading = if horiz < 1e-9 {
        heading_deg
    } else {
        d[0].atan2(d[1]).to_degrees()
    };
    (pitch, heading.rem_euclid(360.0))
}

/// Keys the player can rebind (Settings > Input, the original's `InputRebindingController`): the action's
/// name and its default physical key. The rest of the client handles the default keys; [`Remap`] turns a
/// pressed key into the default key of the action it is bound to.
pub const REBINDABLE: &[(&str, &str)] = &[
    ("Pitch down", "KeyW"),
    ("Pitch up", "KeyS"),
    ("Yaw left", "KeyA"),
    ("Yaw right", "KeyD"),
    ("Roll left", "KeyQ"),
    ("Roll right", "KeyE"),
    ("Throttle up", "ShiftLeft"),
    ("Throttle down", "ControlLeft"),
    ("Full throttle", "KeyZ"),
    ("Cut throttle", "KeyX"),
    ("Stage", "Space"),
    ("SAS on / off", "KeyT"),
    ("RCS", "KeyR"),
    ("Landing gear", "KeyG"),
    ("Brakes", "KeyB"),
    ("Lights", "KeyU"),
    ("Precision controls", "CapsLock"),
    ("Docking mode", "Delete"),
    ("Translate left", "KeyJ"),
    ("Translate right", "KeyL"),
    ("Translate up", "KeyI"),
    ("Translate down", "KeyK"),
    ("Translate forward", "KeyH"),
    ("Translate back", "KeyN"),
    ("Interact on EVA (board)", "KeyF"),
    ("Map", "KeyM"),
    ("Camera mode", "KeyV"),
    ("Quicksave", "F5"),
    ("Quickload", "F9"),
    ("Hide the interface", "F2"),
    ("Vessel labels", "F4"),
];

/// The player's rebinds: default key -> the key now used for it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Remap {
    pub custom: std::collections::HashMap<String, String>,
}

impl Remap {
    /// From the stored form `default=key;default=key`.
    pub fn parse(s: &str) -> Remap {
        let custom = s
            .split(';')
            .filter_map(|kv| kv.split_once('='))
            .filter(|(d, k)| !d.is_empty() && !k.is_empty())
            .map(|(d, k)| (d.to_string(), k.to_string()))
            .collect();
        Remap { custom }
    }

    pub fn to_stored(&self) -> String {
        let mut v: Vec<String> = self
            .custom
            .iter()
            .map(|(d, k)| format!("{d}={k}"))
            .collect();
        v.sort();
        v.join(";")
    }

    /// The key that now does what `default` does by default.
    pub fn key_for<'a>(&'a self, default: &'a str) -> &'a str {
        self.custom
            .get(default)
            .map(|s| s.as_str())
            .unwrap_or(default)
    }

    /// Bind the action whose default key is `default` to `key`. An action that had `key` before gives it up
    /// and goes back to its own default (or nothing, when that default is now taken).
    pub fn bind(&mut self, default: &str, key: &str) {
        let others: Vec<String> = self
            .custom
            .iter()
            .filter(|(d, k)| k.as_str() == key && d.as_str() != default)
            .map(|(d, _)| d.clone())
            .collect();
        for d in others {
            self.custom.remove(&d);
        }
        if key == default {
            self.custom.remove(default);
        } else {
            self.custom.insert(default.to_string(), key.to_string());
        }
    }

    /// The default key to act on for a pressed key: the action it is bound to, `None` when the key's own
    /// action was moved elsewhere and nothing else uses it, the key itself otherwise.
    pub fn translate(&self, code: &str) -> Option<String> {
        if let Some((d, _)) = self.custom.iter().find(|(_, k)| k.as_str() == code) {
            return Some(d.clone());
        }
        if self.custom.contains_key(code) {
            return None;
        }
        Some(code.to_string())
    }
}

/// A key code as the player reads it: `KeyW` -> `W`, `ShiftLeft` -> `Left Shift`, `Digit1` -> `1`.
pub fn key_name(code: &str) -> String {
    if let Some(k) = code.strip_prefix("Key") {
        return k.to_string();
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return d.to_string();
    }
    for side in ["Left", "Right"] {
        if let Some(k) = code.strip_suffix(side) {
            return format!("{side} {k}");
        }
    }
    match code {
        "Space" => "Space".into(),
        "CapsLock" => "Caps Lock".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebinding_moves_actions_between_keys() {
        let mut r = Remap::default();
        assert_eq!(r.translate("KeyW").as_deref(), Some("KeyW"));
        // Stage on P: P stages, Space does nothing.
        r.bind("Space", "KeyP");
        assert_eq!(r.translate("KeyP").as_deref(), Some("Space"));
        assert_eq!(r.translate("Space"), None);
        assert_eq!(r.key_for("Space"), "KeyP");
        // Gear on P takes it from staging, which goes back to Space.
        r.bind("KeyG", "KeyP");
        assert_eq!(r.translate("KeyP").as_deref(), Some("KeyG"));
        assert_eq!(r.translate("Space").as_deref(), Some("Space"));
        let back = Remap::parse(&r.to_stored());
        assert_eq!(back, r);
        r.bind("KeyG", "KeyG");
        assert!(r.custom.is_empty());
        assert_eq!(key_name("ShiftLeft"), "Left Shift");
        assert_eq!(key_name("KeyT"), "T");
    }

    #[test]
    fn controls_coalesce_but_edges_and_final_values_are_immediate() {
        let mut cadence = ControlCadence::default();
        assert!(cadence.due(0.0, true, [1.0, 0.0, 0.0]));
        assert!(!cadence.due(16.0, true, [1.0, 0.0, 0.0]));
        assert!(!cadence.due(33.0, true, [1.0, 0.0, 0.0]));
        assert!(cadence.due(50.0, true, [1.0, 0.0, 0.0]));
        assert!(cadence.due(60.0, true, [-1.0, 0.0, 0.0]));
        assert!(cadence.due(65.0, true, [0.0; 3]));
        assert!(!cadence.due(200.0, false, [0.0; 3]));
        // A pause does not create a catch-up burst.
        assert!(cadence.due(2000.0, true, [0.0, 1.0, 0.0]));
        assert!(!cadence.due(2001.0, true, [0.0, 1.0, 0.0]));
    }

    #[test]
    fn control_cadence_does_not_drift_with_render_frame_jitter() {
        let mut cadence = ControlCadence::default();
        let mut sent = 0;
        for frame in 0..120 {
            let now = frame as f64 * 1000.0 / 60.0 + (frame as f64).sin() * 0.2;
            sent += cadence.due(now, true, [1.0, 0.0, 0.0]) as usize;
        }
        assert_eq!(sent, 40);
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && ((a.1 - b.1 + 540.0).rem_euclid(360.0) - 180.0).abs() < 1e-6
    }

    #[test]
    fn pitch_moves_along_the_heading() {
        assert!(close(turn(90.0, 90.0, -10.0, 0.0), (80.0, 90.0)));
        assert!(close(turn(10.0, 45.0, 5.0, 0.0), (15.0, 45.0)));
    }

    #[test]
    fn yaw_on_the_horizon_turns_the_heading() {
        assert!(close(turn(0.0, 90.0, 0.0, 10.0), (0.0, 100.0)));
        assert!(close(turn(0.0, 90.0, 0.0, -10.0), (0.0, 80.0)));
    }

    #[test]
    fn yaw_while_vertical_tilts_sideways() {
        // Straight up with the heading east: a right yaw leans the nose south (right of east).
        let (p, h) = turn(90.0, 90.0, 0.0, 10.0);
        assert!((p - 80.0).abs() < 1e-6);
        assert!((h - 180.0).abs() < 1e-6);
    }

    #[test]
    fn rotation_is_gentler_in_normal_and_precision_modes_without_changing_other_controls() {
        let mut held = Held::default();
        held.down("KeyW");
        held.down("ShiftLeft");
        assert!((held.rotation_scale() - 0.3).abs() < 1e-10);
        assert_eq!(held.pitch(), -1.0);
        assert_eq!(held.throttle(), 1.0);
        assert_eq!(held.wheel_throttle(), 1.0);
        held.precision = true;
        assert!((held.rotation_scale() - 0.03).abs() < 1e-10);
        assert_eq!(held.scale(), PRECISION_SCALE);
    }

    #[test]
    fn axes_follow_held_keys() {
        let mut k = Held::default();
        k.down("KeyW");
        k.down("ShiftLeft");
        assert_eq!(k.pitch(), -1.0);
        assert_eq!(k.throttle(), 1.0);
        assert_eq!(k.wheel_throttle(), 1.0);
        k.down("KeyS");
        assert_eq!(k.pitch(), 0.0);
        k.clear();
        assert_eq!(k.throttle(), 0.0);
    }
}

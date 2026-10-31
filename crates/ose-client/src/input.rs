//! Held-key state and the original's flight key bindings (`KSP.Input/GameInput.cs`, the embedded input asset).
//!
//! Keys are tracked by `KeyboardEvent.code` (the physical key), as the original binds keys by position. The
//! flight keys that act while held (rotation, throttle, wheels) are read every frame from [`Held`]; the ones
//! that act once (stage, gear, map) stay in the keydown handler.

use ose_math::DVec3;
use std::collections::HashSet;

/// Degrees per second the attitude target turns while a rotation key is held; CapsLock precision mode
/// scales inputs by 0.1 as `FlightInputHandler.inputSensitivityScale` does.
pub const TURN_RATE_DEG_S: f64 = 8.0;
/// Manual torque and roll-rate authority. Keep short keyboard taps manageable with SAS off too.
pub const ROTATION_SCALE: f64 = 1.0;
/// Do not wind the SAS target far ahead of a heavy craft that cannot follow the requested turn rate.
const MAX_TARGET_LEAD_DEG: f64 = 5.0;
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
    trim: [f64; 3],
    /// CapsLock toggles precision mode (`TogglePrecisionMode`).
    pub precision: bool,
}

impl Held {
    /// FlightInputHandler: trim changes by 0.1/s and is added after precision scaling.
    pub fn flight_rotation(&mut self, dt: f64) -> DVec3 {
        self.flight_axes(dt, DVec3::ZERO).1
    }
    pub fn flight_axes(&mut self, dt: f64, additional: DVec3) -> (DVec3, DVec3) {
        let raw = self.rotation() + additional;
        let raw = DVec3::new(
            raw.x.clamp(-1., 1.),
            raw.y.clamp(-1., 1.),
            raw.z.clamp(-1., 1.),
        );
        let modifier = self.is("AltLeft") || self.is("AltRight");
        if modifier && self.is("KeyX") {
            self.trim = [0.0; 3];
        } else if modifier {
            for (t, u) in self.trim.iter_mut().zip([raw.x, raw.y, raw.z]) {
                *t = (*t + u * 0.1 * dt).clamp(-1.0, 1.0);
            }
        }
        let scale = if modifier { 0.0 } else { self.scale() };
        (
            if modifier { DVec3::ZERO } else { raw },
            DVec3::new(
                raw.x * scale + self.trim[0],
                raw.y * scale + self.trim[1],
                raw.z * scale + self.trim[2],
            ),
        )
    }
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
    /// KSP's pitch/yaw/roll intents, also used by SAS target steering. The simulation converts these
    /// into angular axes (+X pitch, -Z yaw, +Y roll) after mirroring Unity Z.
    pub fn rotation(&self) -> DVec3 {
        DVec3::new(self.pitch(), self.yaw(), self.roll())
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
    pub fn scale(&self) -> f64 {
        if self.precision {
            PRECISION_SCALE
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod reference_trim_tests {
    use super::*;
    #[test]
    fn raw_module_input_stays_separate_from_precision_and_analog_trim() {
        let mut keys = Held {
            precision: true,
            ..Default::default()
        };
        let (raw, effective) = keys.flight_axes(0.02, DVec3::new(1., 0., 0.));
        assert_eq!(raw.x, 1.);
        assert_eq!(effective.x, 0.1);
        keys.down("AltLeft");
        let (raw, effective) = keys.flight_axes(1., DVec3::new(1., 0., 0.));
        assert_eq!(raw, DVec3::ZERO);
        assert_eq!(effective.x, 0.1);
        keys.up("AltLeft");
        let (raw, effective) = keys.flight_axes(0.02, DVec3::ZERO);
        assert_eq!(raw, DVec3::ZERO);
        assert_eq!(effective.x, 0.1);
    }
    #[test]
    fn trim_rate_precision_and_reset_follow_reference() {
        let mut keys = Held::default();
        keys.down("AltLeft");
        keys.down("KeyS");
        assert_eq!(keys.flight_rotation(1.0).x, 0.1);
        keys.up("AltLeft");
        keys.up("KeyS");
        keys.precision = true;
        assert_eq!(keys.flight_rotation(0.02).x, 0.1);
        keys.down("KeyS");
        assert_eq!(keys.flight_rotation(0.02).x, 0.2);
        keys.down("AltLeft");
        keys.down("KeyX");
        assert_eq!(keys.flight_rotation(0.02), DVec3::ZERO);
    }
}

/// Advance a SAS target in the vessel's rolled inertial frame, including straight through vertical.
/// Inputs use the same pitch-toward-+Z / yaw-toward-+X convention as `SetRotation`.
pub fn steer_target(
    facing: DVec3,
    top: DVec3,
    target: DVec3,
    pitch: f64,
    yaw: f64,
    dt: f64,
) -> DVec3 {
    let facing = facing.normalized();
    let top = (top - facing * top.dot(facing)).normalized();
    let right = facing.cross(top);
    let command = right * pitch - top * yaw;
    let magnitude = command.length();
    if magnitude == 0.0 {
        return target;
    }
    // Normalize diagonals so two held axes do not turn faster than one.
    let axis = command * (1.0 / magnitude);
    let angle = TURN_RATE_DEG_S.to_radians() * magnitude.min(1.0) * dt.clamp(0.0, 0.1);
    let target = target.normalized();
    let turned = target * angle.cos()
        + axis.cross(target) * angle.sin()
        + axis * (axis.dot(target) * (1.0 - angle.cos()));
    let error_axis = facing.cross(turned);
    let error = error_axis.length().atan2(facing.dot(turned));
    let limit = MAX_TARGET_LEAD_DEG.to_radians();
    if error > limit {
        let tangent = (turned - facing * facing.dot(turned)).normalized();
        (facing * limit.cos() + tangent * limit.sin()).normalized()
    } else {
        turned.normalized()
    }
}

/// Whether E (Ctrl+E anywhere) sends a Kerbal out of the hatch. The original has no EVA key (its crew
/// portraits' EVA button does it) and binds E to roll right in flight, so plain E goes EVA only where rolling
/// means nothing: standing on the ground or afloat. Ctrl+E works in the air and in space as well.
pub fn eva_key(situation: ose_net::FlightSituation, crew: usize, on_eva: bool, ctrl: bool) -> bool {
    use ose_net::FlightSituation as S;
    crew > 0
        && !on_eva
        && situation != S::Crashed
        && (ctrl || matches!(situation, S::Landed | S::Splashed))
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

    #[test]
    fn sas_keys_follow_the_rolled_vessel_frame_on_launch_and_in_orbit() {
        for (facing, top) in [
            (DVec3::new(1.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 1.0)), // vertical at equator
            (DVec3::new(0.0, 1.0, 0.0), DVec3::new(0.0, 0.0, 1.0)), // level
            (DVec3::new(0.0, 1.0, 0.0), DVec3::new(1.0, 0.0, 0.0)), // banked 90 degrees
            (DVec3::new(0.0, 1.0, 0.0), DVec3::new(0.0, 0.0, -1.0)), // inverted
        ] {
            let right = facing.cross(top);
            for (key, expected) in [
                ("KeyW", -top),
                ("KeyS", top),
                ("KeyA", -right),
                ("KeyD", right),
            ] {
                let mut held = Held::default();
                held.down(key);
                let i = held.rotation();
                let target = steer_target(facing, top, facing, i.x, i.y, 0.1);
                assert!(target.dot(expected) > 0.0, "{key}: {facing:?}, {top:?}");
                assert!((facing.dot(target).acos().to_degrees() - 0.8).abs() < 1e-8);
            }
        }
    }

    #[test]
    fn sas_crosses_vertical_without_reversing_and_respects_precision() {
        let mut facing = DVec3::new(1.0, 0.0, 0.0);
        let mut top = DVec3::new(0.0, 0.0, 1.0);
        for _ in 0..250 {
            let next = steer_target(facing, top, facing, 1.0, 0.0, 0.1);
            assert!(next.dot(top) > 0.0);
            let right = facing.cross(top);
            top = right.cross(next).normalized();
            facing = next;
        }
        // 200 degrees of uninterrupted pitch, including both vertical poles.
        assert!((facing.x - 200_f64.to_radians().cos()).abs() < 1e-8);
        let precise = steer_target(facing, top, facing, PRECISION_SCALE, 0.0, 0.1);
        assert!((facing.dot(precise).acos().to_degrees() - 0.08).abs() < 1e-8);
    }

    #[test]
    fn sas_target_does_not_wind_up_and_release_keeps_the_final_target() {
        let facing = DVec3::new(0.0, 1.0, 0.0);
        let top = DVec3::new(0.0, 0.0, 1.0);
        let mut target = facing;
        for _ in 0..600 {
            target = steer_target(facing, top, target, 1.0, 1.0, 1.0 / 60.0);
            assert!(facing.dot(target).acos().to_degrees() <= 5.0 + 1e-8);
        }
        assert_eq!(steer_target(facing, top, target, 0.0, 0.0, 0.1), target);
        let a = steer_target(facing, top, facing, 1.0, 0.0, 0.1);
        let b = steer_target(facing, top, facing, 1.0, 1.0, 0.1);
        assert!((facing.dot(a) - facing.dot(b)).abs() < 1e-10);
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

    #[test]
    fn e_goes_on_eva_on_the_ground_and_in_the_water_and_ctrl_e_anywhere() {
        use ose_net::FlightSituation as S;
        assert!(eva_key(S::Landed, 1, false, false));
        assert!(eva_key(S::Splashed, 1, false, false));
        // in flight E rolls; Ctrl+E still opens the hatch (a space walk)
        assert!(!eva_key(S::Orbit, 1, false, false));
        assert!(eva_key(S::Orbit, 1, false, true));
        assert!(eva_key(S::SubOrbital, 2, false, true));
        // nobody aboard, already a Kerbal, a wreck
        assert!(!eva_key(S::Landed, 0, false, true));
        assert!(!eva_key(S::Landed, 1, true, false));
        assert!(!eva_key(S::Crashed, 1, false, true));
    }
}

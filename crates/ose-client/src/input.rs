//! Held-key state and the original's flight key bindings (`KSP.Input/GameInput.cs`, the embedded input asset).
//!
//! Keys are tracked by `KeyboardEvent.code` (the physical key), as the original binds keys by position. The
//! flight keys that act while held (rotation, throttle, wheels) are read every frame from [`Held`]; the ones
//! that act once (stage, gear, map) stay in the keydown handler.

use std::collections::HashSet;

/// Degrees per second the attitude target turns while a rotation key is held; CapsLock precision mode
/// scales inputs by 0.1 as `FlightInputHandler.inputSensitivityScale` does.
pub const TURN_RATE_DEG_S: f64 = 25.0;
pub const PRECISION_SCALE: f64 = 0.1;
/// Throttle change per second with Shift or Ctrl held (`FlightInputHandler.UpdateMainThrottle`: input x dt).
pub const THROTTLE_RATE: f64 = 1.0;

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

/// One `?` overlay line per binding the client honours, in the original's words (Settings > Input lists them).
pub const FLIGHT_KEYS: &[(&str, &str)] = &[
    ("W / S", "pitch down / up (held)"),
    ("A / D", "yaw left / right (held)"),
    ("Caps Lock", "precision controls"),
    ("Shift / Ctrl", "throttle up / down (held)"),
    ("Z / X", "full throttle / cut"),
    ("Space", "stage"),
    ("T", "SAS: hold the current direction"),
    ("G", "landing gear"),
    ("B", "brakes (held)"),
    ("W / S, A / D landed", "drive and steer on wheels"),
    ("M", "map view"),
    ("Home", "reset the camera"),
    ("Right mouse drag", "look around"),
    (". / , / /", "time warp up / down / stop"),
    ("F2", "hide the interface"),
    ("F4", "vessel labels"),
    ("Esc", "pause menu"),
];

#[cfg(test)]
mod tests {
    use super::*;

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

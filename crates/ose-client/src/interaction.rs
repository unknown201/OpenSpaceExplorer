//! Interaction gains shared by the browser handlers and the KSP2 parity probe.
//! These describe OSE's current behavior, not an assertion of KSP2 parity.

pub fn orbit_radians(pixels: f64) -> f64 {
    pixels * 0.005
}

pub fn wheel_factor(delta_y: f64, shift: bool) -> f64 {
    let factor: f64 = if delta_y > 0.0 { 1.25 } else { 0.8 };
    if shift {
        factor.powi(3)
    } else {
        factor
    }
}

pub fn rotation_step_degrees(snap: bool, fine: bool) -> f64 {
    if fine {
        5.0
    } else if snap {
        90.0
    } else {
        15.0
    }
}

pub fn maneuver_delta_mps(projected_pixels: f64, fine: bool) -> f64 {
    projected_pixels * if fine { 0.1 } else { 1.0 }
}

pub fn throttle_step(current: f64, axis: f64, dt: f64) -> f64 {
    (current + axis * crate::input::THROTTLE_RATE * dt).clamp(0.0, 1.0)
}

pub fn throttle_preset(full: bool) -> f64 {
    if full {
        1.0
    } else {
        0.0
    }
}

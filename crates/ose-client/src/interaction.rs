//! Interaction gains shared by the browser handlers and the KSP2 parity probe (`examples/interaction_probe.rs`,
//! `tools/check_interaction_parity.py`). Each follows the KSP2 0.2.2 code and `Global` configuration recorded in
//! `testdata/ksp2-interactions.json`; the fine/precise modifier is Alt everywhere, as KSP2 binds it
//! (`CameraFineMovement`, the VAB's `modifier`, the map's `altKeyModifier`).

/// KSP2's `CameraZoom` scroll processors per action map: Unity reports 120 per wheel notch, scaled by
/// `ScaleVector2(y=...)`; the map view's 0.1 only matters by its sign.
pub const FLIGHT_ZOOM_PER_NOTCH: f64 = 120.0 * 0.000_583_333_4;
pub const VAB_ZOOM_PER_NOTCH: f64 = 120.0 * 0.005_833_334;
pub const MAP_ZOOM_PER_NOTCH: f64 = 120.0 * 0.000_833_333_4;

/// Wheel notches in a DOM wheel event, positive away from the user (zoom in): 100 px or 3 lines a notch.
pub fn wheel_notches(delta_y: f64, delta_mode: u32) -> f64 {
    match delta_mode {
        1 => -delta_y / 3.0,
        2 => -delta_y,
        _ => -delta_y / 100.0,
    }
}

/// Flight camera orbit (ground and orbital basic rigs): degrees = mouse pixels x `orbitSensitivity` 4.5 x the
/// frame's real seconds, x `fineMovementModifier` 0.3 with Alt. Frame-rate dependent, as KSP2's is.
pub fn flight_orbit_degrees(pixels: f64, frame_s: f64, fine: bool) -> f64 {
    pixels * 4.5 * frame_s * if fine { 0.3 } else { 1.0 }
}

/// Flight camera zoom: distance x exp(-zoomAxis), exp(-0.3 zoomAxis) with Alt.
pub fn flight_zoom(distance: f64, axis: f64, fine: bool) -> f64 {
    distance * (-axis * if fine { 0.3 } else { 1.0 }).exp()
}

/// Map camera orbit: degrees per pixel = 2 x max(0.001, sensitivity x (85 - |pitch|) / 85), sensitivity
/// 0.05 (0.005 with Alt): slower towards the poles.
pub fn map_orbit_degrees(pixels: f64, pitch_deg: f64, fine: bool) -> f64 {
    let sensitivity = if fine { 0.005 } else { 0.05 };
    pixels * 2.0 * (sensitivity * (85.0 - pitch_deg.abs()) / 85.0).max(0.001)
}

/// Map zoom (exponential mode): the inverse map scale x exp(-0.3) in, exp(0.3) out, by the input's sign.
pub fn map_zoom(distance: f64, axis: f64) -> f64 {
    if axis == 0.0 {
        distance
    } else {
        distance * (-0.3 * axis.signum()).exp()
    }
}

/// VAB camera orbit: 0.004 radians per pixel, 0.001 with Alt.
pub fn vab_orbit_radians(pixels: f64, fine: bool) -> f64 {
    pixels * if fine { 0.001 } else { 0.004 }
}

/// VAB zoom: the pivot distance shrinks by zoomAxis x 3 m (Alt: 0.3 m) x 2 with Shift.
pub fn vab_zoom(distance: f64, axis: f64, fast: bool, fine: bool) -> f64 {
    distance - axis * if fine { 0.3 } else { 3.0 } * if fast { 2.0 } else { 1.0 }
}

/// Held-part rotation keys: 90 degrees a press with snap on or off, 5 with Shift (snap only steps the
/// gizmo, not the keys).
pub fn rotation_step_degrees(_snap: bool, fine: bool) -> f64 {
    if fine {
        5.0
    } else {
        90.0
    }
}

/// The CSS pixels to an inch KSP2's handle divides by (Unity's `Screen.dpi` at 100% scaling): DOM pixels
/// already include the display scale.
pub const MANEUVER_DPI: f64 = 96.0;

/// Maneuver handle: while held, each frame adds (projected pixels / (strength x DPI))^2 m/s, signed with the
/// displacement from where the drag started; strength 1, and no precision modifier.
pub fn maneuver_frame_mps(projected_pixels: f64, dpi: f64, strength: f64) -> f64 {
    let x = projected_pixels / (strength * dpi).max(1e-9);
    x * x * projected_pixels.signum()
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

/// A camera drag whose mouseup never reached the page: a mouse move reports no button held (`MouseEvent.buttons`)
/// while a drag is still open. The button was let go outside the window or while it lost focus.
pub fn drag_orphaned(buttons: u16, dragging: bool) -> bool {
    dragging && buttons == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_notches_follow_the_dom_modes() {
        assert_eq!(wheel_notches(-100.0, 0), 1.0);
        assert_eq!(wheel_notches(3.0, 1), -1.0);
        // One notch in each map, as KSP2 scales Unity's 120.
        assert!((flight_zoom(100.0, FLIGHT_ZOOM_PER_NOTCH, false) - 93.24).abs() < 0.01);
        assert!((vab_zoom(20.0, VAB_ZOOM_PER_NOTCH, false, false) - 17.9).abs() < 1e-6);
        assert!((map_zoom(1.0, MAP_ZOOM_PER_NOTCH) - 0.7408).abs() < 1e-4);
    }

    #[test]
    fn a_drag_released_outside_the_window_ends_on_the_next_move() {
        // left, right or middle still held: the drag goes on
        for held in [1, 2, 4, 3] {
            assert!(!drag_orphaned(held, true));
        }
        // nothing held while a drag is open: its mouseup was lost
        assert!(drag_orphaned(0, true));
        // plain hovering is not a drag
        assert!(!drag_orphaned(0, false));
    }
}

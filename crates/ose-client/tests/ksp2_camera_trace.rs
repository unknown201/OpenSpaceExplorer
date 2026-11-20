//! KSP2 parity for the flight camera's orbit and zoom (gaps T27, T28), from a KSP2 trace.
//!
//! The KSP2 parity pilot's `camera` run (`tools/ksp2-parity-pilot/run-camera.ps1`, 2026-11-20): the run-1 craft
//! on the pad, mouse look on and 10 px of mouse movement queued on the Input System's mouse every frame (then
//! with the fine-movement modifier held), then single wheel notches in and out (also fine). Each frame logs what
//! KSP2 read (`Mouse.SystemDelta`, the `CameraZoom` value) and the rig's heading and distance.
//! `tests/data/ksp2-camera` keeps that trace and the live `CameraTweakables` of every flight camera context.

use std::collections::HashMap;

use ose_client::{camera_modes, interaction};

type Row = HashMap<String, String>;

fn csv(text: &str) -> Vec<Row> {
    let mut lines = text.lines();
    let head: Vec<String> = lines
        .next()
        .unwrap()
        .split(',')
        .map(str::to_string)
        .collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            head.iter()
                .cloned()
                .zip(l.split(',').map(str::to_string))
                .collect()
        })
        .collect()
}

fn num(r: &Row, k: &str) -> f64 {
    r[k].parse().unwrap()
}

/// Each frame's turn is the mouse movement KSP2 read that frame x 4.5 x that frame's real seconds (x 0.3 fine):
/// a row's movement and seconds show in the next row's heading, with the modifier as held then.
#[test]
fn ksp2_flight_camera_turns_by_the_frames_mouse_movement_and_time() {
    let rows = csv(include_str!("data/ksp2-camera/camera-trace.csv"));
    let mut checked = 0;
    for w in rows.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if !a["phase"].starts_with("orbit") || num(a, "system_dx") == 0.0 {
            continue;
        }
        let turned = (num(b, "heading") - num(a, "heading") + 540.0).rem_euclid(360.0) - 180.0;
        let ours = interaction::flight_orbit_degrees(
            num(a, "system_dx"),
            num(a, "unscaled_dt"),
            b["fine"] == "1",
        );
        assert!(
            (turned - ours).abs() < 1e-3,
            "frame {}: KSP2 turned {turned:.5} deg, ours {ours:.5}",
            b["frame"]
        );
        checked += 1;
    }
    assert!(checked > 100, "only {checked} frames of mouse movement");
}

/// A wheel notch scales the distance in the frame after it by exp(-0.07), exp(-0.021) fine.
#[test]
fn ksp2_flight_camera_zooms_a_notch_at_a_time() {
    let rows = csv(include_str!("data/ksp2-camera/camera-trace.csv"));
    let mut notches = 0;
    for w in rows.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let zoom = num(a, "zoom_value");
        if zoom == 0.0 {
            continue;
        }
        let notch = zoom.signum();
        assert!((zoom.abs() - interaction::FLIGHT_ZOOM_PER_NOTCH).abs() < 1e-6);
        let ours = interaction::flight_zoom(
            num(a, "distance"),
            notch * interaction::FLIGHT_ZOOM_PER_NOTCH,
            a["fine"] == "1",
        );
        assert!(
            (num(b, "distance") - ours).abs() < 1e-4,
            "frame {}: KSP2 {:.5} m, ours {ours:.5}",
            b["frame"],
            num(b, "distance")
        );
        notches += 1;
    }
    assert_eq!(notches, 13, "5 in, 3 fine, 5 out");
}

/// The zoom and pitch limits are KSP2's live tweakables for each context.
#[test]
fn ksp2_flight_camera_limits_are_the_live_tweakables() {
    let rows = csv(include_str!("data/ksp2-camera/camera-tweakables.csv"));
    let field = |ctx: &str, f: &str| -> f64 {
        rows.iter()
            .find(|r| r["context"] == ctx && r["field"] == f)
            .map(|r| num(r, "value"))
            .unwrap()
    };
    for (ctx, stable, eva) in [
        ("Ground", false, false),
        ("Orbital", true, false),
        ("EVAGround", false, true),
        ("EVAOrbital", true, true),
    ] {
        let ours = camera_modes::flight_camera_limits(stable, eva);
        assert!(
            (ours.min_distance_m - field(ctx, "minDistance")).abs() < 1e-6,
            "{ctx}"
        );
        assert!(
            (ours.max_distance_m - field(ctx, "maxDistance")).abs() < 1e-6,
            "{ctx}"
        );
        let pitch = field(ctx, "maxPitch").min(89.5);
        assert!(
            (ours.max_pitch_rad.to_degrees() - pitch).abs() < 1e-9,
            "{ctx}"
        );
        assert_eq!(field(ctx, "orbitSensitivity"), 4.5, "{ctx}");
        assert_eq!(field(ctx, "scrollSensitivity"), 1.0, "{ctx}");
        assert!(
            (field(ctx, "fineMovementModifier") - 0.3).abs() < 1e-6,
            "{ctx}"
        );
    }
}

//! The original's flight camera modes and when each applies (KSP2 parity T26); the flight view frames them.

/// The original's flight camera modes (`KSP.Sim.CameraMode` as `UniverseCameraManager` registers them for
/// the flight rig), cycled with V (`SelectNextFlightCamera`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraMode {
    /// `FlightBody`: no frame provider, so the gimbal turns with the rig's anchor, the vessel's own frame.
    Body,
    /// `FlightAuto`: Horizon, switching to Celestial while the vessel is in a stable orbit
    /// (`VesselComponent.OnUpdateAutoCameraMode`).
    #[default]
    Auto,
    /// `FlightChase` (`ChaseFrameRotationProvider`): the vessel's frame, followed by a slerp at 3 per second.
    Chase,
    /// `FlightHorizon` (`HorizonFrameRotationProvider`): up is straight out from the body, north ahead.
    Horizon,
    /// `FlightCelestial` (`CelestialFrameRotationProvider`): the body's own axes, its north pole up.
    Celestial,
    /// `FlightCapture` (`CaptureFrameRotationProvider`): the body's axes too, on the ground camera's settings.
    Capture,
}

/// `UniverseCameraManager._allCameraModes`, the order V steps through.
pub const ALL_CAMERA_MODES: [CameraMode; 6] = [
    CameraMode::Body,
    CameraMode::Auto,
    CameraMode::Chase,
    CameraMode::Horizon,
    CameraMode::Celestial,
    CameraMode::Capture,
];
/// `UniverseCameraManager._surfaceEVACameraModes`: a Kerbal on EVA at or below its body's
/// `navballSwitchAltitudeLow` steps through only these.
pub const SURFACE_EVA_CAMERA_MODES: [CameraMode; 3] =
    [CameraMode::Auto, CameraMode::Horizon, CameraMode::Celestial];

/// `CelestialBodyData.navballSwitchAltitudeLow` from the original's body files (13 km where unset).
pub fn navball_switch_altitude_m(body_id: &str) -> f64 {
    match body_id {
        "Bop" => 3_575.0,
        "Dres" => 7_590.0,
        "Duna" => 17_600.0,
        "Eeloo" => 11_550.0,
        "Eve" => 38_500.0,
        "Gilly" => 715.0,
        "Ike" => 7_150.0,
        "Jool" => 330_000.0,
        "Kerbin" => 33_000.0,
        "Kerbol" => 14_388_000.0,
        "Laythe" => 27_500.0,
        "Minmus" => 3_300.0,
        "Moho" => 13_750.0,
        "Mun" => 11_000.0,
        "Pol" => 2_420.0,
        "Tylo" => 33_000.0,
        "Vall" => 16_500.0,
        _ => 13_000.0,
    }
}

/// `VesselComponent.IsStableOrbit`: elliptical, apoapsis inside the sphere of influence and periapsis above
/// the atmosphere (semi-major axis, radius, atmosphere depth and SOI radius in m).
pub fn is_stable_orbit(sma_m: f64, ecc: f64, radius_m: f64, atmosphere_m: f64, soi_m: f64) -> bool {
    ecc < 1.0 && sma_m * (1.0 + ecc) < soi_m && sma_m * (1.0 - ecc) - radius_m > atmosphere_m
}

impl CameraMode {
    /// `SelectNextFlightCamera`: the mode after this one in `list` (its first when this one is not in it).
    pub fn next_in(self, list: &[CameraMode]) -> CameraMode {
        let i = list.iter().position(|&m| m == self);
        list[i.map_or(0, |i| (i + 1) % list.len())]
    }

    pub fn next(self) -> CameraMode {
        self.next_in(&ALL_CAMERA_MODES)
    }

    pub fn name(self) -> &'static str {
        match self {
            CameraMode::Body => "Body",
            CameraMode::Auto => "Auto",
            CameraMode::Chase => "Chase",
            CameraMode::Horizon => "Horizon",
            CameraMode::Celestial => "Celestial",
            CameraMode::Capture => "Capture",
        }
    }
}

#[cfg(test)]
mod camera_mode_tests {
    use super::*;

    /// KSP2 T26 (`UniverseCameraManager`, `VesselComponent.IsStableOrbit`).
    #[test]
    fn ksp2_camera_modes_cycle_and_auto_switches_on_a_stable_orbit() {
        let mut m = CameraMode::default();
        let mut seen = vec![m];
        for _ in 0..6 {
            m = m.next();
            seen.push(m);
        }
        use CameraMode::*;
        assert_eq!(
            seen,
            vec![Auto, Chase, Horizon, Celestial, Capture, Body, Auto]
        );
        // on EVA near the ground only Auto, Horizon and Celestial; from Chase it starts over at Auto
        assert_eq!(Chase.next_in(&SURFACE_EVA_CAMERA_MODES), Auto);
        assert_eq!(Celestial.next_in(&SURFACE_EVA_CAMERA_MODES), Auto);
        assert_eq!(Auto.next_in(&SURFACE_EVA_CAMERA_MODES), Horizon);
        // Kerbin: 600 km radius, 70 km of air, 84,159 km SOI
        let stable = |pe_alt: f64, ap_alt: f64| {
            let (rp, ra) = (600_000.0 + pe_alt, 600_000.0 + ap_alt);
            let a = (rp + ra) / 2.0;
            is_stable_orbit(a, (ra - rp) / (ra + rp), 600_000.0, 70_000.0, 84_159_286.0)
        };
        assert!(stable(80_000.0, 100_000.0), "an 80 x 100 km orbit");
        assert!(
            !stable(60_000.0, 200_000.0),
            "periapsis in the air: still Horizon"
        );
        assert!(!stable(-500_000.0, 120_000.0), "a suborbital hop at 120 km");
        assert!(!stable(80_000.0, 90_000_000.0), "apoapsis beyond the SOI");
        assert_eq!(navball_switch_altitude_m("Kerbin"), 33_000.0);
        assert_eq!(navball_switch_altitude_m("Mun"), 11_000.0);
    }
}

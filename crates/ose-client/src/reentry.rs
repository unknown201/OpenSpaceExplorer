//! The source ReentryMesh data record and FXReentryContextualManager activation curves.
use crate::engine_fx::{curve, number};
use glam::{DQuat, DVec3};
use serde_json::Value;

#[derive(Clone, Copy, Default)]
pub struct State {
    pub heading: DVec3,
    pub density: f64,
    pub altitude: f64,
    pub speed: f64,
    pub mach: f64,
    pub gradient: u8,
}
pub fn intensity(settings: &Value, state: State) -> f64 {
    // The contextual manager gates on the calculated intensity, not a fixed air-density
    // threshold. Thin upper atmosphere can still produce plasma at orbital speeds.
    if !state.density.is_finite()
        || !state.speed.is_finite()
        || !state.mach.is_finite()
        || !state.altitude.is_finite()
        || state.density <= 0.0
        || state.speed <= 0.0
        || state.mach <= 1.0
    {
        return 0.0;
    }
    let flux = number(settings, "FluxScale", 500.0)
        * state.mach.powf(number(settings, "MachExponent", 3.0))
        * state
            .density
            .powf(number(settings, "DensityExponent", 0.53));
    let low = curve(&settings["LowDensityFluxCurve"], flux);
    let high = curve(&settings["HighDensityFluxCurve"], flux);
    let blend = curve(&settings["DensityCurve"], state.density).clamp(0.0, 1.0);
    let intensity = curve(&settings["AltitudeCurve"], state.altitude)
        * curve(&settings["VelocityCurve"], state.speed)
        * (low + (high - low) * blend);
    let minimum = number(settings, "ReentryMinimumIntensity", 0.05).clamp(0.0, 0.999);
    if intensity <= minimum {
        0.0
    } else {
        // FXReentryContextualManager.Update remaps the activation threshold to zero.
        ((intensity - minimum) / (1.0 - minimum)).clamp(0.0, 1.0)
    }
}

/// Hottest surviving part as a fraction of its destruction temperature. Remains meaningful
/// after leaving the atmosphere: stopping the incoming heat does not instantly cool the craft.
pub fn heat_fraction(heat: &[(u16, f32, f32, f32)]) -> f32 {
    heat.iter()
        .filter(|(_, temperature, limit, _)| {
            temperature.is_finite() && limit.is_finite() && *limit > 0.0
        })
        .map(|(_, temperature, limit, _)| (temperature / limit).clamp(0.0, 1.0))
        .fold(0.0, f32::max)
}

/// OSE warning thresholds; these are presentation tuning, not simulation damage limits.
pub fn thermal_level(fraction: f32, heating: bool) -> &'static str {
    if fraction >= 0.85 {
        "critical"
    } else if fraction >= 0.65 {
        "hot"
    } else if heating {
        "heating"
    } else {
        "off"
    }
}
pub fn vessel_data(settings: &Value, state: State, intensity: f64) -> [f32; 40] {
    let mut data = [0.0; 40];
    data[..3].copy_from_slice(&state.heading.normalize_or_zero().as_vec3().to_array());
    let matrix = glam::DMat3::from_quat(DQuat::from_rotation_arc(
        state.heading.normalize_or_zero(),
        DVec3::NEG_Y,
    ));
    data[3..12].copy_from_slice(&matrix.as_mat3().to_cols_array());
    for (i, name) in [
        "FresnelSoftenCurve",
        "MeshDistanceCurve",
        "ShiftWithNoiseCurve",
        "WiggleWithNoiseCurve",
        "BulgeAtTransitionCurve",
        "TailBreakupIntensity",
        "FrictionSpreadCurve",
        "FrictionOffsetLowCurve",
        "FrictionOffsetHighCurve",
        "Noise1ScaleCurve",
        "Noise1LevelMinCurve",
        "Noise1LevelMaxCurve",
        "Noise1RampPowerCurve",
        "Noise2ScaleCurve",
        "Noise2LevelMinCurve",
        "Noise2LevelMaxCurve",
        "Noise2RampPowerCurve",
        "UvScaleCurve",
        "VelocityAnimationScaleCurve",
        "IntensityCurve",
    ]
    .iter()
    .enumerate()
    {
        data[12 + i] = curve(&settings[*name], intensity) as f32;
    }
    for (i, name) in [
        "SoftParticleFade",
        "SoftFresnelScale",
        "SoftFresnelExponent",
        "OcclusionSpreadMin",
        "OcclusionSpreadMax",
        "OcclusionSpreadMaxDist",
        "OcclusionAnimationJitter",
    ]
    .iter()
    .enumerate()
    {
        data[32 + i] = number(settings, name, 1.0) as f32;
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    fn activation_settings() -> Value {
        // Constant flux curves isolate the velocity/density activation path from the shader.
        serde_json::json!({
            "ReentryMinAtmosphereActivationThreshold": 0.001,
            "ReentryMinimumIntensity": 0.05,
            "LowDensityFluxCurve": {"m_Curve":[{"time":0,"value":1}]},
            "HighDensityFluxCurve": {"m_Curve":[{"time":0,"value":1}]},
            "AltitudeCurve": {"m_Curve":[{"time":0,"value":1}]},
            "VelocityCurve": {"m_Curve":[{"time":0,"value":0},{"time":3000,"value":1}]}
        })
    }
    #[test]
    fn orbital_speed_in_thin_air_still_produces_plasma() {
        let settings = activation_settings();
        let mut state = State {
            density: 0.0005,
            speed: 3000.0,
            mach: 10.0,
            altitude: 50000.0,
            ..Default::default()
        };
        assert_eq!(intensity(&settings, state), 1.0);
        state.density = 0.0;
        assert_eq!(intensity(&settings, state), 0.0);
        state.density = 1.0;
        state.speed = 40.0;
        state.mach = 0.12;
        assert_eq!(intensity(&settings, state), 0.0);
    }
    #[test]
    fn onset_is_remapped_and_bad_telemetry_cannot_reach_the_gpu() {
        let mut settings = activation_settings();
        settings["VelocityCurve"] = serde_json::json!({"m_Curve":[{"time":0,"value":0.525}]});
        let state = State {
            density: 0.01,
            speed: 2000.0,
            mach: 7.0,
            altitude: 30000.0,
            ..Default::default()
        };
        assert!((intensity(&settings, state) - 0.5).abs() < 1e-9);
        assert_eq!(
            intensity(
                &settings,
                State {
                    density: f64::NAN,
                    ..state
                }
            ),
            0.0
        );
        assert_eq!(
            intensity(
                &settings,
                State {
                    mach: f64::INFINITY,
                    ..state
                }
            ),
            0.0
        );
        settings["VelocityCurve"] = serde_json::json!({"m_Curve":[{"time":0,"value":0.05}]});
        assert_eq!(intensity(&settings, state), 0.0);
    }
    #[test]
    fn heat_feedback_tracks_the_hottest_part_even_without_incoming_flux() {
        assert_eq!(heat_fraction(&[]), 0.0);
        assert_eq!(
            heat_fraction(&[
                (0, 900.0, 1000.0, 0.0),
                (1, 400.0, 2000.0, 50.0),
                (2, 500.0, 0.0, 0.0)
            ]),
            0.9
        );
        assert_eq!(heat_fraction(&[(0, f32::NAN, 1000.0, 0.0)]), 0.0);
        assert_eq!(thermal_level(0.9, false), "critical");
        assert_eq!(thermal_level(0.7, false), "hot");
        assert_eq!(thermal_level(0.4, true), "heating");
        assert_eq!(thermal_level(0.4, false), "off");
    }
    #[test]
    fn empty_air_and_slow_unpowered_flight_do_not_create_plasma() {
        let settings = serde_json::json!({"ReentryMinAtmosphereActivationThreshold":0.001});
        assert_eq!(
            intensity(
                &settings,
                State {
                    density: 0.0,
                    speed: 3000.0,
                    mach: 10.0,
                    ..Default::default()
                }
            ),
            0.0
        );
        assert_eq!(
            intensity(
                &settings,
                State {
                    density: 1.0,
                    speed: 0.0,
                    ..Default::default()
                }
            ),
            0.0
        );
    }
    #[test]
    fn structured_buffer_matches_the_original_unaligned_float3_layout() {
        let settings = serde_json::json!({"IntensityCurve":{"m_Curve":[{"time":0,"value":5}]},"MeshDistanceCurve":{"m_Curve":[{"time":0,"value":75}]}});
        let data = vessel_data(
            &settings,
            State {
                heading: DVec3::Y,
                ..Default::default()
            },
            0.5,
        );
        assert_eq!(&data[..3], &[0.0, 1.0, 0.0]);
        assert_eq!(data[13], 75.0);
        assert_eq!(data[31], 5.0);
        assert!(data.iter().all(|v| v.is_finite()));
    }
}

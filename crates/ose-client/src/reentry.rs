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
    if state.density < number(settings, "ReentryMinAtmosphereActivationThreshold", 0.001)
        || state.speed <= 0.0
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
    if intensity < number(settings, "ReentryMinimumIntensity", 0.05) {
        0.0
    } else {
        intensity.clamp(0.0, 1.0)
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

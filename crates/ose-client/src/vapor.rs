//! VesselVaporVFXManager's humidity/dew-point activation for wingtip condensation.
use crate::engine_fx::{curve, number};
use serde_json::Value;
pub const BODIES: [&str; 5] = ["Kerbin", "Eve", "Laythe", "Jool", "Duna"];
#[derive(Clone, Copy, Default)]
pub struct State {
    pub body: usize,
    pub altitude: f64,
    pub temperature: f64,
    pub density: f64,
    pub pressure: f64,
    pub dynamic_pressure: f64,
    pub lift: f64,
    pub coefficient: f64,
}
pub fn intensity(settings: &Value, state: State) -> f64 {
    if state.coefficient <= 0.0
        || state.pressure <= 0.0
        || state.temperature <= 30.11
        || state.density < number(settings, "VacuumThresholdDensity", 0.01)
    {
        return 0.0;
    }
    let Some(body) = BODIES.get(state.body) else {
        return 0.0;
    };
    let humidity_curve = &settings["HumidityCurves"][*body];
    if humidity_curve.is_null() {
        return 0.0;
    }
    let humidity = curve(humidity_curve, state.altitude);
    if humidity < number(settings, "VaporHumidityThresholdPercentage", 30.0) {
        return 0.0;
    }
    let rh = (humidity / 100.0).clamp(1e-6, 1.0);
    let log = rh.ln();
    let temperature = state.temperature;
    let dewpoint =
        30.11 - 4283.58 * (temperature - 30.11) / (temperature * log - 30.11 * log - 4283.58);
    let pressure = state.pressure * 1000.0;
    let fraction = (pressure - state.lift / state.coefficient * 1000.0 / pressure)
        .clamp(0.0, 1.0)
        .min(
            1.0 - (state.dynamic_pressure
                / number(settings, "VaporVorticesDynamicPressureClamp", 2000.0))
            .clamp(
                0.0,
                number(settings, "VaporVorticesDynamicPressureMaxIntensity", 0.9),
            ),
        );
    let gamma = number(settings, "VaporVorticesGammaParameter", 1.4);
    let vortex_temperature = fraction.max(0.0).powf((gamma - 1.0) / gamma) * temperature;
    if vortex_temperature > dewpoint {
        return 0.0;
    }
    let threshold = number(settings, "VaporVorticesTemperatureThreshold", 0.0);
    ((dewpoint - vortex_temperature - threshold)
        / (number(settings, "VaporVorticesTemperatureMax", 30.0) - threshold).max(1e-6))
    .clamp(0.0, 1.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dry_air_vacuum_and_unloaded_wings_do_not_condense() {
        let settings =
            serde_json::json!({"HumidityCurves":{"Kerbin":{"m_Curve":[{"time":0,"value":10}]}}});
        assert_eq!(
            intensity(
                &settings,
                State {
                    coefficient: 1.0,
                    pressure: 101.0,
                    temperature: 290.0,
                    density: 1.0,
                    ..Default::default()
                }
            ),
            0.0
        );
        assert_eq!(intensity(&settings, State::default()), 0.0);
    }
    #[test]
    fn humid_air_condenses_under_aerodynamic_pressure_drop() {
        let settings = serde_json::json!({"HumidityCurves":{"Kerbin":{"m_Curve":[{"time":0,"value":90}]}},"VaporVorticesDynamicPressureClamp":20,"VaporVorticesTemperatureMax":30});
        assert!(
            intensity(
                &settings,
                State {
                    coefficient: 1.0,
                    pressure: 101.0,
                    temperature: 290.0,
                    density: 1.0,
                    dynamic_pressure: 10.0,
                    lift: 10.0,
                    ..Default::default()
                }
            ) > 0.5
        );
    }
}

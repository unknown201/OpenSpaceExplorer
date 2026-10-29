//! Browser Gamepad/joystick input with explicit axis calibration, never shared with physics.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub dead_zone: f64,
    pub exponent: f64,
    pub axes: [usize; 3],
    pub invert: [bool; 3],
    pub throttle_axis: Option<usize>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            dead_zone: 0.08,
            exponent: 1.0,
            axes: [1, 0, 2],
            invert: [true, false, false],
            throttle_axis: None,
        }
    }
}
impl Config {
    pub fn axis(&self, value: f64, invert: bool) -> f64 {
        if !value.is_finite() {
            return 0.0;
        }
        let dead = if self.dead_zone.is_finite() {
            self.dead_zone.clamp(0.0, 0.5)
        } else {
            0.08
        };
        let magnitude = ((value.abs() - dead) / (1.0 - dead)).clamp(0.0, 1.0);
        magnitude.powf(if self.exponent.is_finite() {
            self.exponent.clamp(0.25, 4.0)
        } else {
            1.0
        }) * value.signum()
            * if invert { -1.0 } else { 1.0 }
    }
    pub fn rotation(&self, axes: &[f64]) -> [f64; 3] {
        if !self.enabled {
            return [0.0; 3];
        }
        std::array::from_fn(|i| {
            self.axis(
                axes.get(self.axes[i]).copied().unwrap_or(0.0),
                self.invert[i],
            )
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub fn poll(config: &Config) -> ([f64; 3], Option<f64>) {
    use wasm_bindgen::JsCast;
    let Some(pads) = web_sys::window().and_then(|w| w.navigator().get_gamepads().ok()) else {
        return ([0.; 3], None);
    };
    for pad in pads
        .iter()
        .filter_map(|p| p.dyn_into::<web_sys::Gamepad>().ok())
        .filter(|p| p.connected())
    {
        let axes: Vec<_> = pad
            .axes()
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0))
            .collect();
        return (
            config.rotation(&axes),
            config
                .throttle_axis
                .filter(|_| config.enabled)
                .and_then(|i| axes.get(i))
                .filter(|v| v.is_finite())
                .map(|v| ((1.0 - *v) * 0.5).clamp(0.0, 1.0)),
        );
    }
    ([0.; 3], None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibration_dead_zone_endpoints_and_disconnect() {
        let c = Config::default();
        assert_eq!(c.axis(0.07, false), 0.0);
        assert_eq!(c.axis(1.0, false), 1.0);
        assert_eq!(c.axis(-1.0, true), 1.0);
        assert_eq!(c.rotation(&[]), [0.0; 3]);
        assert_eq!(c.axis(f64::NAN, false), 0.0);
        let c = Config {
            dead_zone: 0.0,
            exponent: 2.0,
            ..c
        };
        assert_eq!(c.axis(0.5, false), 0.25);
    }
}

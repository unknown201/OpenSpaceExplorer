//! Authored KSP engine-effect data and the reference's curve/morph evaluation.
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Deserialize)]
pub struct Library {
    pub engines: HashMap<String, String>,
    pub meshes: HashMap<String, Mesh>,
    pub materials: HashMap<String, Material>,
    pub shaders: HashMap<String, Shader>,
    pub textures: HashMap<String, Texture>,
    #[serde(default)]
    pub particle_materials: HashMap<String, Material>,
    #[serde(default)]
    pub context_settings: HashMap<String, f64>,
    #[serde(default)]
    pub reentry_settings: Value,
    #[serde(default)]
    pub vapor_settings: Value,
}
#[derive(Clone, Deserialize)]
pub struct Mesh {
    pub uri: String,
    pub vertices: u32,
    pub indices: u32,
    pub stride: u32,
    #[serde(default)]
    pub bounds: [[f64; 3]; 2],
}
#[derive(Clone, Deserialize)]
pub struct Texture {
    pub uri: String,
    pub raw_uri: String,
    pub mips: Vec<[u32; 2]>,
    pub srgb: bool,
    pub compression: String,
}
#[derive(Clone, Deserialize)]
pub struct Material {
    pub shader: String,
    pub floats: HashMap<String, f64>,
    pub colors: HashMap<String, [f64; 4]>,
    pub textures: HashMap<String, String>,
}
#[derive(Clone, Deserialize)]
pub struct Shader {
    pub uri: String,
    pub bindings: HashMap<String, [usize; 2]>,
    pub src_blend: u32,
    pub dst_blend: u32,
    pub refraction: bool,
    pub gamma_colors: Vec<String>,
    #[serde(default)]
    pub reentry: bool,
}
#[derive(Clone, Deserialize)]
pub struct Layer {
    /// Engine gimbal ancestor's pivot, in the exported (X-reflected) part frame.
    #[serde(default)]
    pub gimbal_pivot: Option<[f64; 3]>,
    pub name: String,
    pub kind: String,
    pub matrix: [[f64; 4]; 4],
    #[serde(default = "unit_scale")]
    pub local_scale: [f64; 3],
    pub events: Vec<[u32; 2]>,
    #[serde(default)]
    pub mesh: String,
    #[serde(default)]
    pub material: String,
    pub control: Value,
    #[serde(default)]
    pub particle: Value,
    #[serde(default)]
    pub renderer: Value,
    #[serde(default)]
    pub parameter_mappings: Vec<Value>,
}
fn unit_scale() -> [f64; 3] {
    [1.0; 3]
}

pub fn number(v: &Value, key: &str, default: f64) -> f64 {
    v[key].as_f64().unwrap_or(default)
}

/// Unity AnimationCurve: Hermite for ordinary keys; weighted Bezier with an inverted time axis for
/// weighted tangents. Step tangents and fixed gradients retain their original discontinuities.
pub fn curve(v: &Value, time: f64) -> f64 {
    let Some(keys) = v["m_Curve"].as_array() else {
        return time;
    };
    let Some(first) = keys.first() else {
        return time;
    };
    if time <= number(first, "time", 0.0) {
        return number(first, "value", 0.0);
    }
    for pair in keys.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (x0, x1) = (number(a, "time", 0.0), number(b, "time", 1.0));
        if time > x1 {
            continue;
        }
        let span = x1 - x0;
        if span <= 0.0 {
            return number(b, "value", 0.0);
        }
        let (y0, y1) = (number(a, "value", 0.0), number(b, "value", 0.0));
        let (m0, m1) = (number(a, "outSlope", 0.0), number(b, "inSlope", 0.0));
        if !m0.is_finite() || !m1.is_finite() {
            return y0;
        }
        let t = (time - x0) / span;
        let weighted_out = number(a, "weightedMode", 0.0) as u32 & 2 != 0;
        let weighted_in = number(b, "weightedMode", 0.0) as u32 & 1 != 0;
        if weighted_out || weighted_in {
            let w0 = if weighted_out {
                number(a, "outWeight", 1.0 / 3.0)
            } else {
                1.0 / 3.0
            };
            let w1 = if weighted_in {
                number(b, "inWeight", 1.0 / 3.0)
            } else {
                1.0 / 3.0
            };
            let bezier = |s: f64, p0: f64, p1: f64, p2: f64, p3: f64| {
                let r = 1.0 - s;
                r * r * r * p0 + 3.0 * r * r * s * p1 + 3.0 * r * s * s * p2 + s * s * s * p3
            };
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..32 {
                let s = (lo + hi) * 0.5;
                if bezier(s, 0.0, w0, 1.0 - w1, 1.0) < t {
                    lo = s
                } else {
                    hi = s
                }
            }
            return bezier(
                (lo + hi) * 0.5,
                y0,
                y0 + m0 * span * w0,
                y1 - m1 * span * w1,
                y1,
            );
        }
        return (2.0 * t * t * t - 3.0 * t * t + 1.0) * y0
            + (t * t * t - 2.0 * t * t + t) * span * m0
            + (-2.0 * t * t * t + 3.0 * t * t) * y1
            + (t * t * t - t * t) * span * m1;
    }
    number(keys.last().unwrap(), "value", 0.0)
}

pub fn gradient(v: &Value, time: f64) -> [f64; 4] {
    let color = |key: &Value| {
        [
            number(key, "r", 1.0),
            number(key, "g", 1.0),
            number(key, "b", 1.0),
            number(key, "a", 1.0),
        ]
    };
    let sample = |alpha: bool| {
        let count = number(
            v,
            if alpha {
                "m_NumAlphaKeys"
            } else {
                "m_NumColorKeys"
            },
            0.0,
        ) as usize;
        let prefix = if alpha { "atime" } else { "ctime" };
        let mut result = color(&v["key0"]);
        for i in 1..count {
            let a = number(v, &format!("{prefix}{}", i - 1), 0.0) / 65535.0;
            let b = number(v, &format!("{prefix}{i}"), 65535.0) / 65535.0;
            let c = color(&v[format!("key{i}")]);
            if time <= b {
                let t = if number(v, "m_Mode", 0.0) == 1.0 {
                    if time >= b {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    ((time - a) / (b - a).max(1e-12)).clamp(0.0, 1.0)
                };
                return std::array::from_fn(|j| result[j] + (c[j] - result[j]) * t);
            }
            result = c;
        }
        result
    };
    let mut c = sample(false);
    c[3] = sample(true)[3];
    c
}

pub fn weights(control: &Value, throttle: f64, pressure: f64) -> [f32; 4] {
    let mut result = [0.0; 4];
    let mut set = |key: &str, fallback: usize, value: f64| {
        let index = number(control, key, fallback as f64) as usize;
        if index < 3 {
            result[index] = value as f32
        }
    };
    set("ZeroThrottleBlendshapeIndex", 0, 1.0 - throttle);
    let scale = curve(&control["ThrottleScaleForAtmoBlendCurve"], throttle);
    let threshold =
        number(control, "UpperAtmoNormalizedPressureThreshold", 0.0092).clamp(1e-9, 0.999999);
    if pressure > threshold {
        let t = 1.0 - (pressure - threshold) / (1.0 - threshold);
        set(
            "UpperAtmosphereBlendshapeIndex",
            1,
            curve(&control["AtmoToUpperBlendCurve"], t) * scale,
        );
        set("VacuumBlendshapeIndex", 2, 0.0);
    } else {
        let t = 1.0 - pressure / threshold;
        set(
            "UpperAtmosphereBlendshapeIndex",
            1,
            curve(&control["UpperAtmoToVacuumBlendCurve"], t) * scale,
        );
        set(
            "VacuumBlendshapeIndex",
            2,
            curve(&control["VacuumFromUpperAtmoBlendCurve"], t) * scale,
        );
    }
    result
}

pub fn material_values(
    material: &Material,
    shader: &Shader,
    control: &Value,
    throttle: f64,
    pressure: f64,
) -> [[f32; 4]; 64] {
    let mut floats = material.floats.clone();
    let mut colors = material.colors.clone();
    for (array, throttle_array, is_color) in [
        (
            "AtmoVac_Float_ShaderParameters",
            "Throttle_Float_ShaderMultipliers",
            false,
        ),
        (
            "AtmoVac_Color_ShaderParameters",
            "Throttle_Color_ShaderMultipliers",
            true,
        ),
    ] {
        let Some(parameters) = control[array].as_array() else {
            continue;
        };
        for parameter in parameters {
            let Some(name) = parameter["ShaderParamName"].as_str() else {
                continue;
            };
            let multiplier = control[throttle_array]
                .as_array()
                .and_then(|a| a.iter().find(|p| p["ShaderParamName"] == name));
            if is_color {
                let c = gradient(&parameter["MinMaxGradient"], pressure);
                let k = multiplier
                    .map(|p| gradient(&p["MinMaxGradient"], throttle))
                    .unwrap_or([1.0; 4]);
                colors.insert(name.into(), std::array::from_fn(|i| c[i] * k[i]));
            } else {
                let value = curve(&parameter["MinMaxCurve"], pressure)
                    * multiplier
                        .map(|p| curve(&p["MinMaxCurve"], throttle))
                        .unwrap_or(1.0);
                floats.insert(name.into(), value);
            }
        }
    }
    colors.insert("_texcoord_ST".into(), [1.0, 1.0, 0.0, 0.0]);
    // KSP's PlayerSettings selects linear rendering. ShaderLab Color properties arrive in sRGB.
    for name in &shader.gamma_colors {
        if let Some(c) = colors.get_mut(name) {
            for value in &mut c[..3] {
                *value = srgb_to_linear(*value);
            }
        }
    }
    let mut result = [[0.0; 4]; 64];
    for (name, [offset, dim]) in &shader.bindings {
        let value = colors
            .get(name)
            .copied()
            .unwrap_or([*floats.get(name).unwrap_or(&0.0); 4]);
        for (i, v) in value.iter().enumerate().take(*dim) {
            if offset + i < 256 {
                result[(offset + i) / 4][(offset + i) % 4] = *v as f32;
            }
        }
    }
    result
}

pub fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn authored_tangents_and_weighted_time_are_preserved() {
        let c =
            json!({"m_Curve":[{"time":0,"value":0,"outSlope":3},{"time":1,"value":1,"inSlope":0}]});
        assert!((curve(&c, 0.5) - 0.875).abs() < 1e-9);
        let c = json!({"m_Curve":[{"time":0,"value":0,"outSlope":0,"weightedMode":2,"outWeight":0.1},
            {"time":1,"value":1,"inSlope":0,"weightedMode":1,"inWeight":0.7}]});
        assert!(curve(&c, 0.5) > 0.7);
    }
    #[test]
    fn pressure_morph_handoff_is_continuous() {
        let linear =
            json!({"m_Curve":[{"time":0,"value":0,"outSlope":1},{"time":1,"value":1,"inSlope":1}]});
        let inverse = json!({"m_Curve":[{"time":0,"value":1,"outSlope":-1},{"time":1,"value":0,"inSlope":-1}]});
        let c = json!({"ThrottleScaleForAtmoBlendCurve":linear,"AtmoToUpperBlendCurve":linear,
            "UpperAtmoToVacuumBlendCurve":inverse,"VacuumFromUpperAtmoBlendCurve":linear});
        assert_eq!(weights(&c, 1.0, 1.0), [0.0; 4]);
        assert_eq!(weights(&c, 1.0, 0.0), [0.0, 0.0, 1.0, 0.0]);
        let a = weights(&c, 1.0, 0.0092 - 1e-8);
        let b = weights(&c, 1.0, 0.0092 + 1e-8);
        assert!((a[1] - b[1]).abs() < 1e-5);
        assert_eq!(weights(&c, 0.0, 0.0), [1.0, 0.0, 0.0, 0.0]);
    }
}

//! Unity particle recipes used by the shipped engine ignition, shutdown and smoke effects.
use crate::engine_fx::{self as fx, Layer};
use glam::{DMat4, DQuat, DVec3};
use serde_json::Value;
use std::{collections::HashMap, rc::Rc};

pub type Key = (u64, usize);
#[derive(Clone, Copy, Default)]
pub struct Context {
    pub normalized_thrust: f64,
    pub normalized_thrust_distance: f64,
    pub vertical_deviation: f64,
}
pub struct Input {
    pub key: Key,
    pub layers: Rc<Vec<Layer>>,
    pub layer: usize,
    pub matrix: DMat4,
    pub throttle: f64,
    pub commanded: f64,
    pub pressure: f64,
    pub mode: u32,
    pub context: Context,
}
struct Source {
    layers: Rc<Vec<Layer>>,
    layer: usize,
    matrix: DMat4,
    previous: DMat4,
    throttle: f64,
    pressure: f64,
    running: bool,
    playing: bool,
    epoch: f64,
    remainder: f64,
    delay: f64,
    context: Context,
}
pub struct Particle {
    pub key: Key,
    pub position: DVec3,
    pub velocity: DVec3,
    pub age: f64,
    pub lifetime: f64,
    pub size: DVec3,
    pub color: [f64; 4],
    pub rotation: DVec3,
    pub random: [f64; 4],
    pub local: bool,
}
pub struct Sprite {
    pub position: DVec3,
    pub axis_x: DVec3,
    pub axis_y: DVec3,
    pub axis_z: DVec3,
    pub color: [f32; 4],
    pub uv: [f32; 4],
    pub next_uv: [f32; 4],
    pub blend: f32,
    pub material: String,
    pub mesh: String,
}
pub struct System {
    sources: HashMap<Key, Source>,
    pub particles: Vec<Particle>,
    random: u32,
}
impl Default for System {
    fn default() -> Self {
        Self {
            sources: HashMap::new(),
            particles: Vec::new(),
            random: 0x5a17e38b,
        }
    }
}
fn enabled(v: &Value) -> bool {
    v["enabled"].as_bool().unwrap_or(false)
}
fn flag(v: &Value, k: &str) -> bool {
    v[k].as_bool()
        .unwrap_or_else(|| v[k].as_u64().unwrap_or(0) != 0)
}
fn vector(v: &Value) -> DVec3 {
    DVec3::new(
        fx::number(v, "x", 0.0),
        fx::number(v, "y", 0.0),
        fx::number(v, "z", 0.0),
    )
}

/// VFXParameterMapping scales a captured prefab value, never the preceding frame's value.
fn context_multiplier(layer: &Layer, context: Context, throttle: f64, parameter: &str) -> f64 {
    layer
        .parameter_mappings
        .iter()
        .rev()
        .find_map(|mapping| {
            if mapping["VFXParameter"]["Id"].as_str()? != parameter {
                return None;
            }
            let time = match mapping["ContextVariable"]["Id"].as_str()? {
                "vessel_throttle" => throttle,
                "derived_normalized_thrust" => context.normalized_thrust,
                "derived_normalized_thrustdistance" => context.normalized_thrust_distance,
                "derived_vertical_deviation" => context.vertical_deviation,
                _ => 0.0,
            };
            Some(fx::curve(&mapping["Curve"], time))
        })
        .unwrap_or(1.0)
}

pub fn minmax(v: &Value, time: f64, random: f64) -> f64 {
    let scalar = fx::number(v, "scalar", 1.0);
    match fx::number(v, "minMaxState", 0.0) as u32 {
        1 => scalar * fx::curve(&v["maxCurve"], time),
        2 => {
            let a = fx::curve(&v["minCurve"], time);
            scalar * (a + (fx::curve(&v["maxCurve"], time) - a) * random)
        }
        3 => {
            let a = fx::number(v, "minScalar", scalar);
            a + (scalar - a) * random
        }
        _ => scalar,
    }
}
fn color(v: &Value, time: f64, random: f64) -> [f64; 4] {
    let c = |v: &Value| {
        [
            fx::number(v, "r", 1.0),
            fx::number(v, "g", 1.0),
            fx::number(v, "b", 1.0),
            fx::number(v, "a", 1.0),
        ]
    };
    match fx::number(v, "minMaxState", 0.0) as u32 {
        1 => fx::gradient(&v["maxGradient"], time),
        2 => {
            let a = c(&v["minColor"]);
            let b = c(&v["maxColor"]);
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * random)
        }
        3 => {
            let a = fx::gradient(&v["minGradient"], time);
            let b = fx::gradient(&v["maxGradient"], time);
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * random)
        }
        4 => fx::gradient(&v["maxGradient"], random),
        _ => c(&v["maxColor"]),
    }
}
fn controlled(
    control: &Value,
    prefix: &str,
    throttle: f64,
    original: f64,
    time: f64,
    random: f64,
) -> f64 {
    let gate = format!("control{}{}", prefix[..1].to_uppercase(), &prefix[1..]);
    if !flag(control, &gate) {
        return original;
    }
    let low = fx::number(control, &format!("{prefix}Min"), original);
    let high = fx::number(control, &format!("{prefix}Max"), original);
    match fx::number(control, &format!("{prefix}Type"), 0.0) as u32 {
        1 => low + high * throttle * random,
        2 => (low + (high - low) * throttle) * fx::curve(&control[format!("{prefix}Curve")], time),
        _ => low + (high - low) * throttle,
    }
}
impl System {
    pub fn clear(&mut self) {
        self.sources.clear();
        self.particles.clear();
    }
    fn random(&mut self) -> f64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random as f64 / u32::MAX as f64
    }
    pub fn step(&mut self, dt: f64, inputs: &[Input]) {
        if !dt.is_finite() || !(0.0..=0.25).contains(&dt) {
            self.clear();
            return;
        }
        // Update current emitter transforms before integrating local-space particles.
        for input in inputs {
            let layer = &input.layers[input.layer];
            let active = input.throttle > 0.001;
            let source = self.sources.entry(input.key).or_insert_with(|| Source {
                layers: input.layers.clone(),
                layer: input.layer,
                matrix: input.matrix,
                previous: input.matrix,
                throttle: 0.0,
                pressure: input.pressure,
                running: false,
                playing: false,
                epoch: 0.0,
                remainder: 0.0,
                delay: 0.0,
                context: input.context,
            });
            source.previous = source.matrix;
            source.matrix = input.matrix;
            source.pressure = input.pressure;
            source.context = input.context;
            let event = if active && !source.running {
                6
            } else if !active && source.running {
                if input.commanded > 0.001 {
                    4
                } else {
                    7
                }
            } else {
                10
            };
            let matches = |event| {
                layer
                    .events
                    .iter()
                    .any(|e| e[0] == input.mode && e[1] == event)
            };
            if event != 10 && (matches(event) || (active && matches(10))) {
                source.playing = true;
                source.epoch = 0.0;
                source.remainder = 0.0;
                source.delay = minmax(&layer.particle["startDelay"], 0.0, 0.5).max(0.0);
            }
            if matches(10) {
                source.playing = active;
            }
            source.running = active;
            source.throttle = if active {
                input.throttle
            } else {
                source.throttle
            };
        }
        if dt == 0.0 {
            return;
        }
        for p in &mut self.particles {
            p.age += dt;
            let mut additional = DVec3::ZERO;
            if let Some(source) = self.sources.get(&p.key) {
                let layer = &source.layers[source.layer];
                let convert = |value: DVec3, module: &Value| {
                    let world = flag(module, "inWorldSpace");
                    if world && p.local {
                        source.matrix.inverse().transform_vector3(value)
                    } else if !world && !p.local {
                        source.matrix.transform_vector3(value)
                    } else {
                        value
                    }
                };
                let sample = |module: &Value| {
                    DVec3::new(
                        minmax(&module["x"], p.age / p.lifetime, p.random[0]),
                        minmax(&module["y"], p.age / p.lifetime, p.random[1]),
                        minmax(&module["z"], p.age / p.lifetime, p.random[2]),
                    )
                };
                let force = &layer.particle["ForceModule"];
                if enabled(force) {
                    p.velocity += convert(sample(force), force) * dt;
                }
                let velocity = &layer.particle["VelocityModule"];
                if enabled(velocity) {
                    additional = convert(sample(velocity), velocity);
                }
                let module = &layer.particle["ClampVelocityModule"];
                if enabled(module) {
                    let limit = minmax(&module["magnitude"], p.age / p.lifetime, p.random[0]);
                    let speed = p.velocity.length();
                    let damp = fx::number(module, "dampen", 0.0).clamp(0.0, 1.0);
                    if speed > limit && limit >= 0.0 {
                        p.velocity *= 1.0 - (1.0 - limit / speed) * damp;
                    }
                    let mut drag = minmax(&module["drag"], p.age / p.lifetime, p.random[1]);
                    if flag(module, "multiplyDragByParticleVelocity") {
                        drag *= p.velocity.length();
                    }
                    if flag(module, "multiplyDragByParticleSize") {
                        drag *= p.size.max_element();
                    }
                    p.velocity *= (-drag.max(0.0) * dt).exp();
                }
            }
            p.position += (p.velocity + additional) * dt;
        }
        self.particles.retain(|p| p.age < p.lifetime);
        let keys: Vec<_> = self.sources.keys().copied().collect();
        for key in keys {
            // Pull state out while spawning to keep the random generator independent of map borrows.
            let mut source = self.sources.remove(&key).unwrap();
            let layer = &source.layers[source.layer];
            let ps = &layer.particle;
            let alive = self.particles.iter().filter(|p| p.key == key).count();
            let max = fx::number(&ps["InitialModule"], "maxNumParticles", 1000.0) as usize;
            if !source.playing || !enabled(&ps["EmissionModule"]) {
                self.sources.insert(key, source);
                continue;
            }
            let speed = fx::number(ps, "simulationSpeed", 1.0);
            let duration = if flag(&layer.control, "controlDuration") {
                (1.0 - source.throttle)
                    * (fx::number(&layer.control, "durationMax", 1.0)
                        - fx::number(&layer.control, "durationMin", 1.0))
                    + fx::number(&layer.control, "durationMin", 1.0)
            } else {
                fx::number(ps, "lengthInSec", 1.0)
            }
            .max(1e-6);
            let raw_start = source.epoch - source.delay;
            let start = raw_start.max(0.0);
            let end = (raw_start + dt * speed).max(0.0);
            let looping = flag(ps, "looping");
            let normalized = (start / duration).fract();
            let emission = &ps["EmissionModule"];
            let mut births = Vec::new();
            if start < duration || looping {
                let rate = controlled(
                    &layer.control,
                    "emissionOverTime",
                    source.throttle,
                    minmax(&emission["rateOverTime"], normalized, 0.5),
                    normalized,
                    0.5,
                )
                .max(0.0)
                    * context_multiplier(
                        layer,
                        source.context,
                        source.throttle,
                        "emission_rate_over_time",
                    );
                let travel = source
                    .matrix
                    .w_axis
                    .truncate()
                    .distance(source.previous.w_axis.truncate());
                let by_distance =
                    minmax(&emission["rateOverDistance"], normalized, 0.5).max(0.0) * travel;
                let active_time = if looping {
                    end - start
                } else {
                    end.min(duration) - start
                };
                let total = source.remainder + rate * active_time + by_distance;
                let count = total.floor() as usize;
                for i in 0..count.min(max - alive) {
                    births.push(
                        (start - raw_start) / speed
                            + (i as f64 + 1.0 - source.remainder)
                                / (rate * speed + by_distance / dt).max(1e-9),
                    );
                }
                source.remainder = total.fract();
                if let Some(bursts) = emission["m_Bursts"].as_array() {
                    let first_loop = (start / duration).floor() as u32;
                    let last_loop = if looping {
                        (end / duration).floor() as u32
                    } else {
                        0
                    };
                    for loop_index in first_loop..=last_loop {
                        for burst in bursts {
                            let repeat = fx::number(burst, "repeatInterval", 0.0).max(1e-6);
                            let cycles = fx::number(burst, "cycleCount", 1.0) as u32;
                            let base =
                                loop_index as f64 * duration + fx::number(burst, "time", 0.0);
                            let first = ((start - base) / repeat).ceil().max(0.0) as u32;
                            let until = end.min((loop_index + 1) as f64 * duration);
                            let mut last = ((until - base) / repeat).ceil().max(0.0) as u32;
                            if cycles != 0 {
                                last = last.min(cycles);
                            }
                            // Unity cycleCount=0 repeats indefinitely, bounded here by this tick.
                            for cycle in first..last {
                                let time = base + cycle as f64 * repeat;
                                if time >= start
                                    && time < end
                                    && self.random() <= fx::number(burst, "probability", 1.0)
                                {
                                    let count =
                                        minmax(&burst["countCurve"], normalized, self.random())
                                            .round()
                                            .max(0.0)
                                            as usize;
                                    for _ in
                                        0..count.min(max - alive - births.len().min(max - alive))
                                    {
                                        births.push((time - raw_start) / speed);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for birth in births {
                let r = std::array::from_fn(|_| self.random());
                let initial = &ps["InitialModule"];
                let control = &layer.control;
                let life = controlled(
                    control,
                    "lifetime",
                    source.throttle,
                    minmax(&initial["startLifetime"], normalized, r[0]),
                    normalized,
                    r[0],
                ) * context_multiplier(
                    layer,
                    source.context,
                    source.throttle,
                    "start_lifetime",
                );
                if life <= 0.0 {
                    continue;
                }
                let initial_speed =
                    controlled(
                        control,
                        "speed",
                        source.throttle,
                        minmax(&initial["startSpeed"], normalized, r[1]),
                        normalized,
                        r[1],
                    ) * context_multiplier(layer, source.context, source.throttle, "start_speed");
                let shape = &ps["ShapeModule"];
                let az = r[2] * std::f64::consts::TAU;
                let (mut position, mut direction) = (DVec3::ZERO, DVec3::Z);
                if enabled(shape) {
                    let radius = fx::number(&shape["radius"], "value", 0.0);
                    let thickness = fx::number(shape, "radiusThickness", 1.0);
                    let radial = ((1.0 - thickness).powi(2)
                        + (1.0 - (1.0 - thickness).powi(2)) * r[3])
                        .sqrt();
                    position = DVec3::new(az.cos(), az.sin(), 0.0) * radius * radial;
                    let angle = if flag(control, "controlShapeAngle") {
                        let low = fx::number(control, "shapeAngleMin", 0.0);
                        low + (fx::number(control, "shapeAngleMax", 0.0) - low) * source.throttle
                    } else {
                        fx::number(shape, "angle", 0.0)
                    }
                    .to_radians()
                        * radial;
                    direction =
                        DVec3::new(az.cos() * angle.sin(), az.sin() * angle.sin(), angle.cos());
                    match fx::number(shape, "type", 4.0) as u32 {
                        5 => {
                            position = DVec3::new(r[1] - 0.5, r[2] - 0.5, r[3] - 0.5);
                            direction = DVec3::Z;
                        }
                        17 => {
                            let phi = r[1] * std::f64::consts::TAU;
                            let minor = fx::number(shape, "donutRadius", 0.2) * r[3].sqrt();
                            position = DVec3::new(
                                az.cos() * (radius + minor * phi.cos()),
                                az.sin() * (radius + minor * phi.cos()),
                                minor * phi.sin(),
                            );
                            direction =
                                DVec3::new(az.cos() * phi.cos(), az.sin() * phi.cos(), phi.sin());
                        }
                        _ => {}
                    }
                    let q = DQuat::from_euler(
                        glam::EulerRot::ZXY,
                        fx::number(&shape["m_Rotation"], "z", 0.0).to_radians(),
                        fx::number(&shape["m_Rotation"], "x", 0.0).to_radians(),
                        fx::number(&shape["m_Rotation"], "y", 0.0).to_radians(),
                    );
                    position =
                        q * (position * vector(&shape["m_Scale"])) + vector(&shape["m_Position"]);
                    direction = q * direction;
                }
                let local = fx::number(ps, "moveWithTransform", 0.0) == 1.0;
                let mut velocity = direction * initial_speed;
                if !local {
                    let f = (birth / dt).clamp(0.0, 1.0);
                    let emitter = source
                        .previous
                        .w_axis
                        .truncate()
                        .lerp(source.matrix.w_axis.truncate(), f);
                    position = source.matrix.transform_vector3(position) + emitter;
                    velocity = source.matrix.transform_vector3(velocity);
                    let inherit = &ps["InheritVelocityModule"];
                    if enabled(inherit) {
                        velocity += (source.matrix.w_axis.truncate()
                            - source.previous.w_axis.truncate())
                            / dt
                            * minmax(&inherit["m_Curve"], 0.0, r[0]);
                    }
                }
                let mut size =
                    controlled(
                        control,
                        "size",
                        source.throttle,
                        minmax(&initial["startSize"], normalized, r[0]),
                        normalized,
                        r[0],
                    ) * context_multiplier(layer, source.context, source.throttle, "start_size");
                if flag(control, "sizePressureMode") {
                    size *= fx::curve(&control["sizePressureCurve"], 1.0 - source.pressure);
                }
                let mut size = if flag(initial, "size3D") {
                    DVec3::new(
                        size,
                        minmax(&initial["startSizeY"], normalized, r[1]),
                        minmax(&initial["startSizeZ"], normalized, r[2]),
                    )
                } else {
                    DVec3::splat(size)
                };
                if flag(control, "controlSize") && flag(control, "XYZSizeMode") {
                    let low = vector(&control["sizeMinXYZ"]);
                    let high = vector(&control["sizeMaxXYZ"]);
                    let linear = low + (high - low) * source.throttle;
                    size = match fx::number(control, "sizexyzType", 0.0) as u32 {
                        1 => low + high * source.throttle * DVec3::new(r[0], r[1], r[2]),
                        2 => {
                            linear
                                * DVec3::new(
                                    fx::curve(&control["sizeCurveX"], normalized),
                                    fx::curve(&control["sizeCurveY"], normalized),
                                    fx::curve(&control["sizeCurveZ"], normalized),
                                )
                        }
                        _ => linear,
                    };
                }
                let rotation = DVec3::new(
                    minmax(&initial["startRotationX"], normalized, r[1]),
                    minmax(&initial["startRotationY"], normalized, r[2]),
                    minmax(&initial["startRotation"], normalized, r[3]),
                );
                let age = dt - birth;
                if age >= life {
                    continue;
                }
                self.particles.push(Particle {
                    key,
                    position: position + velocity * age,
                    velocity,
                    age,
                    lifetime: life,
                    size,
                    color: color(&initial["startColor"], normalized, r[2]),
                    rotation,
                    random: r,
                    local,
                });
            }
            source.epoch += dt * speed;
            if !looping && end >= duration {
                source.playing = false
            }
            self.sources.insert(key, source);
        }
        let input_keys: std::collections::HashSet<_> = inputs.iter().map(|i| i.key).collect();
        self.sources.retain(|key, source| {
            if !input_keys.contains(key) {
                source.playing = false;
            }
            input_keys.contains(key) || self.particles.iter().any(|p| p.key == *key)
        });
    }

    pub fn sprites(&self, camera: DVec3, forward: DVec3, up: DVec3) -> Vec<Sprite> {
        let right = forward.cross(up).normalize();
        let up = right.cross(forward).normalize();
        let mut sprites = Vec::new();
        for p in &self.particles {
            let Some(source) = self.sources.get(&p.key) else {
                continue;
            };
            let layer = &source.layers[source.layer];
            let ps = &layer.particle;
            let t = (p.age / p.lifetime).clamp(0.0, 1.0);
            let position = if p.local {
                source.matrix.transform_point3(p.position)
            } else {
                p.position
            };
            if (position - camera).dot(forward) <= 0.0 {
                continue;
            }
            let module = &ps["SizeModule"];
            let mut size = p.size;
            if enabled(module) {
                size.x *= minmax(&module["curve"], t, p.random[0]);
                size.y *= minmax(
                    if flag(module, "separateAxes") {
                        &module["y"]
                    } else {
                        &module["curve"]
                    },
                    t,
                    p.random[1],
                );
                size.z *= minmax(
                    if flag(module, "separateAxes") {
                        &module["z"]
                    } else {
                        &module["curve"]
                    },
                    t,
                    p.random[2],
                );
            }
            let scale = DVec3::new(
                source.matrix.x_axis.truncate().length(),
                source.matrix.y_axis.truncate().length(),
                source.matrix.z_axis.truncate().length(),
            );
            size *= scale;
            let mut c = p.color;
            if enabled(&ps["ColorModule"]) {
                let mut life_color = color(&ps["ColorModule"]["gradient"], t, p.random[1]);
                if flag(&layer.control, "controlAlpha") {
                    let low = fx::gradient(&layer.control["upperAtmoGradient"], t);
                    let high = fx::gradient(&layer.control["lowerAtmoGradient"], t);
                    let blend = ((source.pressure - 0.2) / 0.1).clamp(0.0, 1.0);
                    life_color = std::array::from_fn(|i| low[i] + (high[i] - low[i]) * blend);
                }
                for i in 0..4 {
                    c[i] *= life_color[i];
                }
            }
            let uv = &ps["UVModule"];
            let mut rect = [0.0, 0.0, 1.0, 1.0];
            let mut next = rect;
            let mut blend = 0.0;
            if enabled(uv) {
                let cols = fx::number(uv, "tilesX", 1.0).max(1.0) as u32;
                let rows = fx::number(uv, "tilesY", 1.0).max(1.0) as u32;
                let frame = minmax(&uv["startFrame"], t, p.random[3])
                    + minmax(&uv["frameOverTime"], t, p.random[2]) * fx::number(uv, "cycles", 1.0);
                let frame = frame.fract() * (cols * rows) as f64;
                let make = |index: u32| {
                    [
                        (index % cols) as f32 / cols as f32,
                        (index / cols) as f32 / rows as f32,
                        1.0 / cols as f32,
                        1.0 / rows as f32,
                    ]
                };
                rect = make(frame.floor() as u32);
                next = make((frame.floor() as u32 + 1) % (cols * rows));
                blend = frame.fract() as f32;
            }
            let rotation = &ps["RotationModule"];
            let mut angle = p.rotation.z;
            if enabled(rotation) {
                angle += minmax(&rotation["curve"], t, p.random[3]) * p.age;
            }
            let render_mode = fx::number(&layer.renderer, "m_RenderMode", 0.0) as u32;
            let (mut x, mut y, mut z) = (right, up, -forward);
            if render_mode == 1 {
                let velocity = if p.local {
                    source.matrix.transform_vector3(p.velocity)
                } else {
                    p.velocity
                };
                let projected = velocity - forward * velocity.dot(forward);
                if projected.length() > 1e-6 {
                    y = projected.normalize();
                    x = y.cross(-forward).normalize();
                }
                size.y = size.y * fx::number(&layer.renderer, "m_LengthScale", 1.0)
                    + velocity.length() * fx::number(&layer.renderer, "m_VelocityScale", 0.0);
            } else if render_mode == 4 {
                let q = DQuat::from_euler(glam::EulerRot::ZXY, angle, p.rotation.x, p.rotation.y);
                x = source.matrix.transform_vector3(q * DVec3::X).normalize();
                y = source.matrix.transform_vector3(q * DVec3::Y).normalize();
                z = source.matrix.transform_vector3(q * DVec3::Z).normalize();
            } else {
                let (s, c) = angle.sin_cos();
                let previous = x;
                x = previous * c + y * s;
                y = y * c - previous * s;
            }
            sprites.push(Sprite {
                position,
                axis_x: x * size.x,
                axis_y: y * size.y,
                axis_z: z * size.z,
                color: c.map(|v| v as f32),
                uv: rect,
                next_uv: next,
                blend,
                material: layer.material.clone(),
                mesh: layer.mesh.clone(),
            });
        }
        sprites
            .sort_unstable_by(|a, b| b.position.dot(forward).total_cmp(&a.position.dot(forward)));
        sprites
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn minmax_constants_and_curves_preserve_authored_ranges() {
        assert_eq!(minmax(&json!({"scalar":8}), 0.5, 0.0), 8.0);
        assert_eq!(
            minmax(&json!({"minMaxState":3,"minScalar":2,"scalar":8}), 0.0, 0.5),
            5.0
        );
        assert_eq!(
            minmax(
                &json!({"minMaxState":1,"scalar":4,"maxCurve":{"m_Curve":[{"time":0,"value":0,"outSlope":1},{"time":1,"value":1,"inSlope":1}]}}),
                0.25,
                0.0
            ),
            1.0
        );
    }
    fn recipe(events: Value, cap: usize) -> Rc<Vec<Layer>> {
        Rc::new(vec![serde_json::from_value(json!({"name":"source burst","kind":"particle","matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]],
            "events":events,"control":{},"particle":{"lengthInSec":0.2,"looping":false,"startDelay":{"scalar":0},"simulationSpeed":1,
            "InitialModule":{"maxNumParticles":cap,"startLifetime":{"scalar":0.08},"startSpeed":{"scalar":0},"startSize":{"scalar":1}},
            "EmissionModule":{"enabled":true,"rateOverTime":{"scalar":0},"rateOverDistance":{"scalar":0},"m_Bursts":[{"time":0,"countCurve":{"scalar":10},"cycleCount":1,"probability":1}]}}})).unwrap()])
    }
    fn input(layers: Rc<Vec<Layer>>, throttle: f64) -> Input {
        Input {
            key: (0, 0),
            layers,
            layer: 0,
            matrix: DMat4::IDENTITY,
            throttle,
            commanded: throttle,
            pressure: 1.0,
            mode: 0,
            context: Context::default(),
        }
    }
    #[test]
    fn source_particle_limit_does_not_pause_the_emitter_clock() {
        let mut s = System::default();
        let layers = recipe(json!([[0, 6]]), 3);
        s.step(0.02, &[input(layers.clone(), 1.0)]);
        assert_eq!(s.particles.len(), 3);
        for _ in 0..12 {
            s.step(0.02, &[input(layers.clone(), 1.0)]);
        }
        assert!(
            s.particles.is_empty(),
            "a full source must not replay delayed bursts"
        );
    }
    #[test]
    fn shutdown_burst_fires_once_and_preserves_existing_particles() {
        let mut s = System::default();
        let layers = recipe(json!([[0, 7]]), 100);
        s.step(0.02, &[input(layers.clone(), 1.0)]);
        assert!(s.particles.is_empty());
        s.step(0.02, &[input(layers.clone(), 0.0)]);
        assert_eq!(s.particles.len(), 10);
        s.step(0.02, &[input(layers.clone(), 0.0)]);
        assert_eq!(s.particles.len(), 10);
        for _ in 0..10 {
            s.step(0.02, &[input(layers.clone(), 0.0)]);
        }
        assert!(s.particles.is_empty());
    }
    #[test]
    fn running_sources_honor_the_authored_start_delay() {
        let mut layers = recipe(json!([[0, 10]]), 100);
        Rc::get_mut(&mut layers).unwrap()[0].particle["startDelay"] = json!({"scalar":0.1});
        let mut s = System::default();
        for _ in 0..4 {
            s.step(0.02, &[input(layers.clone(), 1.0)]);
        }
        assert!(s.particles.is_empty());
        for _ in 0..2 {
            s.step(0.02, &[input(layers.clone(), 1.0)]);
        }
        assert_eq!(s.particles.len(), 10);
    }
    #[test]
    fn throttle_curves_use_emitter_time_instead_of_particle_randomness() {
        let control = json!({"controlSpeed":true,"speedMin":0,"speedMax":8,"speedType":2,
            "speedCurve":{"m_Curve":[{"time":0,"value":0,"outSlope":1},{"time":1,"value":1,"inSlope":1}]}});
        assert_eq!(controlled(&control, "speed", 0.5, 99.0, 0.25, 0.8), 1.0);
    }
    #[test]
    fn contextual_lifetime_mapping_scales_the_prefab_value_once() {
        let mut layers = recipe(json!([[0, 6]]), 100);
        Rc::get_mut(&mut layers).unwrap()[0].parameter_mappings = vec![json!({
            "VFXParameter":{"Id":"start_lifetime"},
            "ContextVariable":{"Id":"derived_normalized_thrust"},
            "Curve":{"m_Curve":[{"time":0,"value":0,"outSlope":2},{"time":1,"value":2,"inSlope":2}]}
        })];
        let mut system = System::default();
        for _ in 0..2 {
            let mut source = input(layers.clone(), 1.0);
            source.context.normalized_thrust = 0.75;
            system.step(0.02, &[source]);
        }
        assert_eq!(system.particles.len(), 10);
        assert!(system
            .particles
            .iter()
            .all(|particle| (particle.lifetime - 0.12).abs() < 1e-9));
    }
    #[test]
    fn zero_cycle_count_repeats_bursts_through_the_source_duration() {
        let mut layers = recipe(json!([[0, 10]]), 100);
        let particle = &mut Rc::get_mut(&mut layers).unwrap()[0].particle;
        particle["InitialModule"]["startLifetime"] = json!({"scalar":1});
        particle["EmissionModule"]["m_Bursts"][0] = json!({"time":0,"countCurve":{"scalar":1},"cycleCount":0,"repeatInterval":0.05,"probability":1});
        let mut system = System::default();
        system.step(0.2, &[input(layers.clone(), 1.0)]);
        assert_eq!(system.particles.len(), 4);
        system.step(0.2, &[input(layers, 1.0)]);
        assert_eq!(system.particles.len(), 4);
    }
}

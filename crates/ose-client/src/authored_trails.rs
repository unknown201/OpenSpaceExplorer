//! TrailRenderer ribbons with original lifetime, minimum vertex spacing, width and color curves.
use crate::{
    authored_particles::{Key, Sprite},
    engine_fx::{self, Layer},
};
use glam::{DMat4, DVec3};
use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
};
pub struct Input {
    pub key: Key,
    pub layers: Rc<Vec<Layer>>,
    pub layer: usize,
    pub matrix: DMat4,
    pub intensity: f64,
}
struct Trail {
    layers: Rc<Vec<Layer>>,
    layer: usize,
    points: VecDeque<(DVec3, f64)>,
    intensity: f64,
}
#[derive(Default)]
pub struct System {
    trails: HashMap<Key, Trail>,
}
impl System {
    pub fn clear(&mut self) {
        self.trails.clear();
    }
    pub fn count(&self) -> usize {
        self.trails
            .values()
            .map(|t| t.points.len().saturating_sub(1))
            .sum()
    }
    pub fn step(&mut self, dt: f64, inputs: &[Input]) {
        if !dt.is_finite() || !(0.0..=0.25).contains(&dt) {
            self.clear();
            return;
        }
        for trail in self.trails.values_mut() {
            let lifetime = engine_fx::number(&trail.layers[trail.layer].renderer, "m_Time", 0.5);
            for point in &mut trail.points {
                point.1 += dt;
            }
            while trail.points.front().is_some_and(|p| p.1 > lifetime) {
                trail.points.pop_front();
            }
        }
        for input in inputs {
            let trail = self.trails.entry(input.key).or_insert_with(|| Trail {
                layers: input.layers.clone(),
                layer: input.layer,
                points: VecDeque::new(),
                intensity: input.intensity,
            });
            if input.intensity > 0.001 {
                trail.intensity = input.intensity;
                let position = input.matrix.w_axis.truncate();
                let spacing = engine_fx::number(
                    &input.layers[input.layer].renderer,
                    "m_MinVertexDistance",
                    0.2,
                );
                if dt > 0.0
                    && trail
                        .points
                        .back()
                        .is_none_or(|p| p.0.distance(position) >= spacing)
                {
                    trail.points.push_back((position, 0.0));
                }
            }
        }
        self.trails
            .retain(|key, trail| !trail.points.is_empty() || inputs.iter().any(|i| i.key == *key));
    }
    pub fn sprites(&self, camera: DVec3) -> Vec<Sprite> {
        let mut sprites = Vec::new();
        for trail in self.trails.values() {
            let layer = &trail.layers[trail.layer];
            let params = &layer.renderer["m_Parameters"];
            let lifetime = engine_fx::number(&layer.renderer, "m_Time", 0.5).max(1e-6);
            for (a, b) in trail.points.iter().zip(trail.points.iter().skip(1)) {
                let position = (a.0 + b.0) * 0.5;
                let y = b.0 - a.0;
                if y.length_squared() < 1e-10 {
                    continue;
                }
                let normal = (camera - position).normalize_or_zero();
                let x = y.cross(normal).normalize_or_zero();
                let age = (a.1 + b.1) * 0.5 / lifetime;
                let width = engine_fx::number(params, "widthMultiplier", 1.0)
                    * engine_fx::curve(&params["widthCurve"], age);
                let mut color = engine_fx::gradient(&params["colorGradient"], age);
                color[3] *= trail.intensity;
                sprites.push(Sprite {
                    position,
                    axis_x: x * width,
                    axis_y: y,
                    axis_z: normal,
                    color: color.map(|v| v as f32),
                    uv: [0.0, 0.0, 1.0, 1.0],
                    next_uv: [0.0, 0.0, 1.0, 1.0],
                    blend: 0.0,
                    material: layer.material.clone(),
                    mesh: String::new(),
                });
            }
        }
        sprites
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trail_spacing_and_lifetime_are_simulation_time_not_render_frames() {
        let layer:Layer=serde_json::from_value(serde_json::json!({"name":"trail","kind":"trail","matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]],"events":[[0,10]],"control":{},"renderer":{"m_Time":0.5,"m_MinVertexDistance":0.2}})).unwrap();
        let layers = Rc::new(vec![layer]);
        let mut system = System::default();
        for x in [0.0, 0.1, 0.3] {
            system.step(
                0.02,
                &[Input {
                    key: (1, 0),
                    layers: layers.clone(),
                    layer: 0,
                    matrix: DMat4::from_translation(DVec3::X * x),
                    intensity: 1.0,
                }],
            );
        }
        assert_eq!(system.count(), 1);
        for _ in 0..30 {
            system.step(0.02, &[]);
        }
        assert_eq!(system.count(), 0);
    }
}

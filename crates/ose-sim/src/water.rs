//! Water coefficients derived from the reference PartBehavior/PhysicsSettings paths.
//! Runtime comparison lives in tools/water-parity; these formula tests alone do not establish parity.
use ose_data::{DragCube, PartBounds, PartDef, WaterProperties};
use ose_math::DVec3;

#[derive(Clone, Debug)]
pub struct WaterPart {
    pub properties: WaterProperties,
    pub bounds: PartBounds,
    pub volume_m3: f64,
    pub wet: bool,
    pub elapsed_s: f64,
    pub drag_scalar: f64,
}

impl WaterPart {
    pub fn from_def(d: Option<&PartDef>) -> Self {
        let properties = d.map(|d| d.water.clone()).unwrap_or_default();
        let cube = d.and_then(|d| {
            d.drag_cubes
                .iter()
                .find(|c| c.name == properties.cube_name)
                .or_else(|| d.drag_cubes.first())
        });
        let bounds = d
            .and_then(|d| d.drag_cubes.first())
            .map(|c| PartBounds {
                size_m: c.size,
                center_m: c.center,
            })
            .or_else(|| d.and_then(|d| d.bounds))
            .unwrap_or(PartBounds {
                size_m: DVec3::new(1., 1., 1.),
                center_m: DVec3::ZERO,
            });
        let volume_m3 = cube.map(displacement).unwrap_or_else(|| {
            d.and_then(|d| d.bounds)
                .map(|b| b.size_m.x * b.size_m.y * b.size_m.z * 0.75)
                .unwrap_or(1.)
        });
        Self {
            properties,
            bounds,
            volume_m3,
            wet: false,
            elapsed_s: 0.,
            drag_scalar: 4.5,
        }
    }

    /// Begin a new part-entry event after the bounds leave the water.
    pub fn reset_contact(&mut self) {
        self.wet = false;
        self.elapsed_s = 0.;
    }

    pub fn drag(&mut self, speed: f64, down_dot: f64, dt: f64) -> f64 {
        let early = self.elapsed_s < 3.;
        let limit = 4.5 * if early { 0.5 + down_dot } else { 1. };
        if speed >= 1.6 {
            if !early && self.drag_scalar > 0.15 {
                self.drag_scalar += (0.15 - self.drag_scalar) * 0.25 * (2. - 1.25 * down_dot) * dt;
            }
        } else if self.drag_scalar < limit {
            self.drag_scalar += (limit - self.drag_scalar) * 0.25 * dt;
        }
        if early && !self.wet {
            self.drag_scalar = self
                .drag_scalar
                .min((0.05 + 0.95 * down_dot) * (limit - 0.15) + 0.15);
        }
        self.elapsed_s += dt;
        self.wet = true;
        self.drag_scalar
    }
}

pub fn displacement(c: &DragCube) -> f64 {
    let s = c.size;
    let volume = s.x * s.y * s.z;
    let x = c.area[0] / (s.y * s.z);
    let y = c.area[2] / (s.x * s.z);
    let z = c.area[4] / (s.x * s.y);
    let sum = x + y + z;
    if sum.is_finite() && sum > 0. {
        volume * (x.min(z) + 2. * x * z) / 3. * y
    } else {
        volume
    }
}

/// Linear fraction between the shallowest and deepest bounds corners, including entry ramp.
/// The force point is the depth-weighted wet corners, blended towards the authored CoB.
pub fn immersion(points: [DVec3; 8], up: DVec3, origin_depth: f64, cob: DVec3) -> (f64, DVec3) {
    let depths = points.map(|p| origin_depth - p.dot(up));
    let lo = depths.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = depths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if hi <= 0. || lo >= hi {
        return (0., cob);
    }
    let fraction = if lo >= 0. { 1. } else { hi / (hi - lo) };
    let mut at = cob;
    if lo < 0. {
        let mut sum = DVec3::ZERO;
        let mut weight = 0.;
        for (p, d) in points.into_iter().zip(depths) {
            if d > 0. {
                sum = sum + p * d;
                weight += d;
            }
        }
        at = at + sum * ((1. - fraction) / weight);
    }
    (fraction * (hi / 0.2).min(1.), at)
}

pub fn destroys_on_entry(velocity: DVec3, up: DVec3, tolerance: f64) -> bool {
    (0.1 * velocity.length()).max(-velocity.dot(up)) > 1.2 * tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_areas_reduce_displacement_and_degenerate_cubes_stay_finite() {
        let mut c = DragCube {
            name: String::new(),
            area: [4.; 6],
            drag: [1.; 6],
            depth: [1.; 6],
            center: DVec3::ZERO,
            size: DVec3::new(2., 2., 2.),
        };
        assert!((displacement(&c) - 8.).abs() < 1e-12);
        c.area[0] = 2.;
        c.area[4] = 2.;
        assert!((displacement(&c) - 8. / 3.).abs() < 1e-12);
        c.size = DVec3::ZERO;
        assert_eq!(displacement(&c), 0.);
    }
    #[test]
    fn entry_threshold_includes_horizontal_speed_and_is_strict() {
        let up = DVec3::new(0., 1., 0.);
        assert!(!destroys_on_entry(DVec3::new(0., -18., 0.), up, 15.));
        assert!(destroys_on_entry(DVec3::new(0., -18.001, 0.), up, 15.));
        assert!(!destroys_on_entry(DVec3::new(180., 0., 0.), up, 15.));
        assert!(destroys_on_entry(DVec3::new(180.01, 0., 0.), up, 15.));
        assert!(!destroys_on_entry(DVec3::new(0., 19., 0.), up, 15.));
    }
    #[test]
    fn surface_entry_ramps_and_fully_submerged_force_returns_to_authored_centre() {
        let points = std::array::from_fn(|i| {
            DVec3::new(
                if i & 1 == 0 { -1. } else { 1. },
                if i & 2 == 0 { -1. } else { 1. },
                if i & 4 == 0 { -1. } else { 1. },
            )
        });
        let up = DVec3::new(0., 1., 0.);
        let (f, p) = immersion(points, up, -0.9, DVec3::ZERO);
        assert!((f - 0.025).abs() < 1e-12);
        assert!((p.y + 0.95).abs() < 1e-12);
        let cob = DVec3::new(0.1, -0.1, 0.);
        assert_eq!(immersion(points, up, 2., cob), (1., cob));
    }
    #[test]
    fn drag_retains_entry_history_and_decays_after_three_seconds() {
        let mut p = WaterPart::from_def(None);
        assert!((p.drag(5., 0., 0.02) - 0.255).abs() < 1e-12);
        for _ in 0..200 {
            p.drag(5., 0., 0.02);
        }
        assert!(p.drag_scalar < 0.255 && p.drag_scalar > 0.15);
        let previous = p.drag_scalar;
        p.drag(0.1, 0., 0.02);
        assert!(p.drag_scalar > previous);
        p.reset_contact();
        assert!(!p.wet && p.elapsed_s == 0.);
    }
}

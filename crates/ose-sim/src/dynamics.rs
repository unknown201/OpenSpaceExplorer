//! Craft-frame mass properties for flight. Parts are oriented boxes; fuel is distributed with its tank.
//! Units are tonnes, metres and seconds, so the inertia tensor maps rad/s² to kN·m.

use ose_math::DVec3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inertia {
    pub x: DVec3,
    pub y: DVec3,
    pub z: DVec3,
}

impl Inertia {
    pub fn apply(self, v: DVec3) -> DVec3 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }

    pub fn solve(self, torque: DVec3) -> DVec3 {
        let det = self.x.dot(self.y.cross(self.z));
        if det <= 0.0 || !det.is_finite() {
            return DVec3::ZERO;
        }
        DVec3::new(
            torque.dot(self.y.cross(self.z)),
            torque.dot(self.z.cross(self.x)),
            torque.dot(self.x.cross(self.y)),
        ) * (1.0 / det)
    }

    fn add_outer(&mut self, v: DVec3, scale: f64) {
        self.x = self.x + v * (v.x * scale);
        self.y = self.y + v * (v.y * scale);
        self.z = self.z + v * (v.z * scale);
    }

    /// Add a box about the craft's mass centre. `axes` are its orthonormal craft-frame axes.
    pub fn add_box(&mut self, mass: f64, size: DVec3, axes: [DVec3; 3], offset: DVec3) {
        let own = DVec3::new(
            size.y * size.y + size.z * size.z,
            size.x * size.x + size.z * size.z,
            size.x * size.x + size.y * size.y,
        ) * (mass / 12.0);
        for (axis, moment) in axes.into_iter().zip([own.x, own.y, own.z]) {
            self.add_outer(axis, moment);
        }
        let r2 = mass * offset.length_squared();
        self.x.x += r2;
        self.y.y += r2;
        self.z.z += r2;
        self.add_outer(offset, -mass);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MassProperties {
    pub mass_t: f64,
    pub centre_m: DVec3,
    pub inertia: Inertia,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_box_and_parallel_axis_have_analytical_moments() {
        let h = 0.5_f64.sqrt();
        let mut i = Inertia::default();
        // 12 t, 2x4x6 m: principal moments 52,40,20. Rotate 45° about Z.
        i.add_box(
            12.0,
            DVec3::new(2.0, 4.0, 6.0),
            [
                DVec3::new(h, h, 0.0),
                DVec3::new(-h, h, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ],
            DVec3::new(1.0, 2.0, 0.0),
        );
        assert!((i.x - DVec3::new(94.0, -18.0, 0.0)).length() < 1e-10);
        assert!((i.y - DVec3::new(-18.0, 58.0, 0.0)).length() < 1e-10);
        assert!((i.z - DVec3::new(0.0, 0.0, 80.0)).length() < 1e-10);
        let a = DVec3::new(2.0, -3.0, 4.0);
        assert!((i.solve(i.apply(a)) - a).length() < 1e-10);
    }
}

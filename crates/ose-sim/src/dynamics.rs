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
    /// Rotate a full part-space tensor and translate it with the parallel-axis theorem.
    pub fn add_tensor(&mut self, mass: f64, tensor: [DVec3; 3], axes: [DVec3; 3], offset: DVec3) {
        let rotate = |v: DVec3| axes[0] * v.x + axes[1] * v.y + axes[2] * v.z;
        let column = |e: DVec3| {
            let local = DVec3::new(axes[0].dot(e), axes[1].dot(e), axes[2].dot(e));
            rotate(tensor[0] * local.x + tensor[1] * local.y + tensor[2] * local.z) * mass
        };
        self.x = self.x + column(DVec3::new(1.0, 0.0, 0.0));
        self.y = self.y + column(DVec3::new(0.0, 1.0, 0.0));
        self.z = self.z + column(DVec3::new(0.0, 0.0, 1.0));
        let r2 = mass * offset.length_squared();
        self.x.x += r2;
        self.y.y += r2;
        self.z.z += r2;
        self.add_outer(offset, -mass);
    }
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
    fn authored_tensor_rotation_and_mass_offset_match_analytic_box() {
        let axes = [
            DVec3::new(0., 1., 0.),
            DVec3::new(-1., 0., 0.),
            DVec3::new(0., 0., 1.),
        ];
        let offset = DVec3::new(1., 2., 3.);
        let mut imported = Inertia::default();
        imported.add_tensor(
            12.,
            [
                DVec3::new(52. / 12., 0., 0.),
                DVec3::new(0., 40. / 12., 0.),
                DVec3::new(0., 0., 20. / 12.),
            ],
            axes,
            offset,
        );
        let mut analytic = Inertia::default();
        analytic.add_box(12., DVec3::new(2., 4., 6.), axes, offset);
        assert!((imported.x - analytic.x).length() < 1e-10);
        assert!((imported.y - analytic.y).length() < 1e-10);
        assert!((imported.z - analytic.z).length() < 1e-10);
    }

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

#[derive(serde::Deserialize)]
struct InertiaSettings {
    enabled: bool,
    scalar: f64,
    low_mass_scalar: f64,
    low_mass_limit_t: f64,
}
/// Multiplier on the current-mass unit tensor. RigidbodyBehavior caches its
/// unscaled initial tensor, then rescales it without changing rigid-body mass.
pub(crate) fn reference_inertia_factor(
    mode: u8,
    mass: f64,
    initial_mass: f64,
    part_count: usize,
) -> f64 {
    static SETTINGS: std::sync::LazyLock<InertiaSettings> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../assets/own/inertia-settings.json"))
            .expect("inertia settings")
    });
    if !SETTINGS.enabled || mode != 2 {
        return 1.;
    }
    if part_count == 1 {
        if mass <= SETTINGS.low_mass_limit_t {
            SETTINGS.low_mass_scalar * initial_mass / mass
        } else {
            1.
        }
    } else {
        SETTINGS.scalar * initial_mass / (mass * mass)
    }
}

#[cfg(test)]
mod reference_scaling_tests {
    use super::*;
    #[test]
    fn cached_reference_tensor_scaling_does_not_scale_the_lever_mass() {
        assert_eq!(reference_inertia_factor(2, 2., 2., 4), 2.5);
        assert_eq!(reference_inertia_factor(2, 1., 2., 4), 10.);
        assert_eq!(reference_inertia_factor(2, 1., 2., 1), 1.);
        assert_eq!(reference_inertia_factor(2, 0.01, 0.02, 1), 100.);
        assert_eq!(reference_inertia_factor(0, 1., 2., 4), 1.);
        let mut inertia = Inertia::default();
        inertia.add_tensor(
            2.,
            [
                DVec3::new(1., 0., 0.) * 2.5,
                DVec3::new(0., 1., 0.) * 2.5,
                DVec3::new(0., 0., 1.) * 2.5,
            ],
            [
                DVec3::new(1., 0., 0.),
                DVec3::new(0., 1., 0.),
                DVec3::new(0., 0., 1.),
            ],
            DVec3::new(3., 0., 0.),
        );
        assert_eq!(inertia.x.x, 5.);
        assert_eq!(inertia.y.y, 23.);
    }
}

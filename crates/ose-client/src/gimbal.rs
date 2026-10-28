//! Presentation of the authoritative thrust vector, around the engine's authored pivots.
use glam::{DMat4, DQuat, DVec3};

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Definition {
    pub transform: String,
    #[serde(default)]
    pub visual_transforms: Vec<String>,
    pub response_speed: Option<f64>,
}

pub struct Driver {
    rotation: DQuat,
    ut: f64,
}

impl Definition {
    pub fn controls(&self, name: &str) -> bool {
        if self.visual_transforms.is_empty() {
            name == self.transform
        } else {
            self.visual_transforms.iter().any(|n| n == name)
        }
    }
}

impl Driver {
    pub fn new(ut: f64) -> Self {
        Self {
            rotation: DQuat::IDENTITY,
            ut,
        }
    }

    pub fn update(&mut self, direction: DVec3, ut: f64, speed: Option<f64>) -> DQuat {
        let target = if direction.is_finite() && direction.length_squared() > 0.5 {
            DQuat::from_rotation_arc(DVec3::Y, direction.normalize())
        } else {
            DQuat::IDENTITY
        };
        let dt = (ut - self.ut).max(0.0);
        self.ut = ut;
        // Exponential response is stable at high warp and independent of render frame rate.
        let t = speed.map(|s| 1.0 - (-s.max(0.0) * dt).exp()).unwrap_or(1.0);
        self.rotation = self.rotation.slerp(target, t).normalize();
        self.rotation
    }
}

pub fn about_pivot(rotation: DQuat, pivot: DVec3) -> DMat4 {
    DMat4::from_translation(pivot) * DMat4::from_quat(rotation) * DMat4::from_translation(-pivot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nozzle_swings_around_mount_and_exhaust_follows_it() {
        let pivot = DVec3::new(0.2, 1.0, -0.3);
        let q = DQuat::from_rotation_z(0.05);
        let m = about_pivot(q, pivot);
        assert!((m.transform_point3(pivot) - pivot).length() < 1e-12);
        let lip = pivot - DVec3::Y * 2.0;
        assert!((m.transform_point3(lip) - lip).length() > 0.09);
        assert!((m.transform_vector3(-DVec3::Y) + q * DVec3::Y).length() < 1e-12);
    }
    #[test]
    fn response_freezes_when_paused_returns_to_neutral_and_is_frame_independent() {
        let direction = DQuat::from_rotation_x(0.1) * DVec3::Y;
        let mut a = Driver::new(0.0);
        let mut b = Driver::new(0.0);
        let q = a.update(direction, 0.1, Some(10.0));
        for n in 1..=10 {
            b.update(direction, n as f64 * 0.01, Some(10.0));
        }
        assert!(q.angle_between(b.rotation) < 1e-7);
        assert!(a.update(DVec3::Y, 0.1, Some(10.0)).angle_between(q) < 1e-7);
        assert!(
            a.update(DVec3::Y, 2.0, Some(10.0))
                .angle_between(DQuat::IDENTITY)
                < 1e-6
        );
    }
}

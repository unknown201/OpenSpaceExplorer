//! Presentation-only attitude prediction and reconciliation. Authoritative flight state stays untouched.

use glam::{DMat3, DQuat, DVec3};

const PREDICT_MS: f64 = 100.0;
const CORRECT_MS: f64 = 50.0;
const STALE_MS: f64 = 500.0;
const MAX_PREDICT_ANGLE: f64 = std::f64::consts::PI / 6.0;

/// Exported meshes mirror Unity X, while physics mirrors Unity Z. A half-turn
/// around Y converts the exported craft frame to the physics vessel frame.
pub fn mesh_to_vessel() -> DQuat {
    DQuat::from_xyzw(0.0, 1.0, 0.0, 0.0)
}

/// Body, warp and surface/flight changes invalidate an angular velocity estimate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub body: u16,
    pub warp: u32,
    pub grounded: bool,
}

/// Craft +Y is its nose and +Z its top. Orthonormalize the wire axes before making a quaternion.
pub fn from_axes(facing: DVec3, top: DVec3) -> Option<DQuat> {
    if !facing.is_finite() || !top.is_finite() {
        return None;
    }
    let y = facing.normalize_or_zero();
    let z = (top - y * top.dot(y)).normalize_or_zero();
    if y.length_squared() < 0.5 || z.length_squared() < 0.5 {
        return None;
    }
    Some(DQuat::from_mat3(&DMat3::from_cols(y.cross(z), y, z)).normalize())
}

pub struct Track {
    rotation: DQuat,
    correction: DQuat,
    /// Local rotation vector per wall-clock millisecond, derived from simulation time and warp.
    velocity: DVec3,
    ut: f64,
    received_ms: f64,
    context: Context,
}

impl Track {
    pub fn new(rotation: DQuat, ut: f64, received_ms: f64, context: Context) -> Self {
        Self {
            rotation,
            correction: DQuat::IDENTITY,
            velocity: DVec3::ZERO,
            ut,
            received_ms,
            context,
        }
    }

    pub fn update(&mut self, mut rotation: DQuat, ut: f64, received_ms: f64, context: Context) {
        // q and -q describe the same pose. Always take the short arc, including heading wraparound.
        if self.rotation.dot(rotation) < 0.0 {
            rotation = -rotation;
        }
        let dt_ms = (ut - self.ut) * 1000.0 / context.warp.max(1) as f64;
        let gap = received_ms - self.received_ms;
        if context != self.context
            || dt_ms <= 0.0
            || !dt_ms.is_finite()
            || !(0.0..=STALE_MS).contains(&gap)
            || dt_ms > STALE_MS
            || self.rotation.angle_between(rotation) > std::f64::consts::FRAC_PI_2
        {
            *self = Self::new(rotation, ut, received_ms, context);
            return;
        }
        let displayed = self.sample(received_ms);
        self.velocity = (self.rotation.inverse() * rotation).to_scaled_axis() / dt_ms;
        self.rotation = rotation;
        // Preserve the displayed pose at arrival, then remove prediction error over a short window.
        self.correction = (rotation.inverse() * displayed).normalize();
        self.ut = ut;
        self.received_ms = received_ms;
    }

    pub fn sample(&self, now_ms: f64) -> DQuat {
        let age = (now_ms - self.received_ms).max(0.0);
        let mut advance = self.velocity * age.min(PREDICT_MS);
        if advance.length() > MAX_PREDICT_ANGLE {
            advance = advance.normalize() * MAX_PREDICT_ANGLE;
        }
        (self.rotation
            * DQuat::from_scaled_axis(advance)
            * self
                .correction
                .slerp(DQuat::IDENTITY, (age / CORRECT_MS).min(1.0)))
        .normalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_geometry_matches_physics_axes_for_stock_craft() {
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        // Check mounted geometry, not just the vessel nose (which is unchanged by
        // the missing half-turn). Off-axis points expose the upside-down render.
        let flight_crafts: Vec<_> = crafts
            .iter()
            .filter_map(ose_data::craft::level_layout)
            .collect();
        for craft in crafts.iter().chain(&flight_crafts) {
            for part in &craft.parts {
                let p = part.local_position;
                let q = part.local_rotation;
                let unity_q = DQuat::from_xyzw(q.x, q.y, q.z, q.w).normalize();
                let mesh_q = DQuat::from_xyzw(q.x, -q.y, -q.z, q.w).normalize();
                for v in [DVec3::X, DVec3::Y, DVec3::Z] {
                    let unity = DVec3::new(p.x, p.y, p.z) + unity_q * v;
                    let physics = DVec3::new(unity.x, unity.y, -unity.z);
                    let exported = DVec3::new(-p.x, p.y, p.z) + mesh_q * DVec3::new(-v.x, v.y, v.z);
                    assert!(
                        (mesh_to_vessel() * exported - physics).length() < 1e-8,
                        "{}: {}",
                        craft.name,
                        part.part_id
                    );
                }
            }
        }
    }

    #[test]
    fn stock_planes_share_the_launch_layout_and_keep_gear_below_the_cockpit() {
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        for name in ["Stock04_Jet", "Stock03_Drone"] {
            let craft = crafts.iter().find(|c| c.name == name).unwrap();
            let flight = ose_data::craft::level_layout(craft).unwrap();
            assert!(ose_data::craft::level_layout(&flight).is_none());
            let q = |p: &ose_data::craft::CraftPart| {
                let q = p.local_rotation;
                DQuat::from_xyzw(q.x, q.y, q.z, q.w).normalize()
            };
            let root = craft.parts.iter().position(|p| p.parent.is_none()).unwrap();
            let turn = q(&flight.parts[root]) * q(&craft.parts[root]).inverse();
            assert!((q(&flight.parts[root]) * DVec3::Y - DVec3::Y).length() < 1e-6);
            // Hangar up becomes Unity +Z, hence physics -Z (the level basis's up).
            assert!((turn * DVec3::Y - DVec3::Z).length() < 1e-6);
            for part in &flight.parts {
                if part.part_id.starts_with("landinggear_") {
                    assert!(part.local_position.z < 1e-6, "{name}: gear above cockpit");
                }
            }
        }
    }
    const CONTEXT: Context = Context {
        body: 1,
        warp: 1,
        grounded: false,
    };
    fn yaw(degrees: f64) -> DQuat {
        DQuat::from_rotation_z(degrees.to_radians())
    }
    fn close(a: DQuat, b: DQuat) {
        assert!(a.angle_between(b).abs() < 1e-6, "{a:?} != {b:?}");
    }

    #[test]
    fn steady_turn_is_continuous_at_arrival_and_between_snapshots() {
        let mut track = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
        track.update(yaw(10.0), 0.1, 100.0, CONTEXT);
        close(track.sample(100.0), yaw(0.0));
        close(track.sample(150.0), yaw(15.0));
        let before = track.sample(200.0);
        track.update(yaw(20.0), 0.2, 200.0, CONTEXT);
        close(track.sample(200.0), before);
        close(track.sample(225.0), yaw(22.5));
        close(track.sample(250.0), yaw(25.0));
        // A missing snapshot cannot keep a vessel spinning indefinitely.
        close(track.sample(300.0), track.sample(10_000.0));
    }

    #[test]
    fn wraparound_roll_and_axes_stay_orthonormal() {
        let mut track = Track::new(yaw(359.0), 0.0, 0.0, CONTEXT);
        track.update(-yaw(1.0), 0.1, 100.0, CONTEXT);
        close(track.sample(150.0), yaw(2.0));
        let pose = yaw(34.0) * DQuat::from_rotation_y(1.2);
        close(
            from_axes(pose * DVec3::Y * 2.0, pose * (DVec3::Z + DVec3::Y * 0.1)).unwrap(),
            pose,
        );
        assert!(from_axes(DVec3::ZERO, DVec3::Z).is_none());
        assert!(from_axes(DVec3::Y, DVec3::Y).is_none());
        assert!(from_axes(DVec3::NAN, DVec3::Z).is_none());
    }

    #[test]
    fn jitter_uses_server_time_and_stops_reconcile_without_snapping() {
        let mut track = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
        track.update(yaw(10.0), 0.1, 130.0, CONTEXT);
        close(track.sample(180.0), yaw(15.0));
        let before = track.sample(200.0);
        track.update(yaw(10.0), 0.2, 200.0, CONTEXT);
        close(track.sample(200.0), before);
        close(track.sample(250.0), yaw(10.0));
    }

    #[test]
    fn discontinuities_reset_prediction() {
        for (ut, now, context) in [
            (0.0, 200.0, CONTEXT),
            (0.2, 900.0, CONTEXT),
            (0.2, 200.0, Context { body: 2, ..CONTEXT }),
            (0.2, 200.0, Context { warp: 4, ..CONTEXT }),
            (
                0.2,
                200.0,
                Context {
                    grounded: true,
                    ..CONTEXT
                },
            ),
        ] {
            let mut track = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
            track.update(yaw(10.0), 0.1, 100.0, CONTEXT);
            track.update(yaw(20.0), ut, now, context);
            close(track.sample(now), yaw(20.0));
            close(track.sample(now + 100.0), yaw(20.0));
        }
    }

    #[test]
    fn prediction_angle_is_bounded_even_for_fast_rolls() {
        let mut track = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
        track.update(yaw(80.0), 0.02, 20.0, CONTEXT);
        close(track.sample(120.0), yaw(110.0));
    }
}

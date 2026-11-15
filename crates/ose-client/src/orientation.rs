//! Presentation-only attitude prediction and reconciliation. Authoritative flight state stays untouched.

use glam::{DMat3, DQuat, DVec3};

const PREDICT_MS: f64 = 100.0;
const CORRECT_MS: f64 = 50.0;
const STALE_MS: f64 = 500.0;
const MAX_PREDICT_ANGLE: f64 = std::f64::consts::PI / 6.0;
/// How long a steering change takes to show in the authoritative state at first (the round trip and a
/// snapshot interval), ms; learned from the vessel's response afterwards, within these bounds.
const STEER_LAG_MS: f64 = 150.0;
const STEER_LAG_MIN_MS: f64 = 30.0;
const STEER_LAG_MAX_MS: f64 = 600.0;
/// How fast learned figures follow new samples (share of each sample).
const LEARN_RATE: f64 = 0.3;

/// The pilot's steering as the client predicts it until the server's state catches up: how the vessel turns
/// per unit of input (learned from how it answered earlier input) and the input changes not yet seen in
/// its state. Presentation only: the server still flies the vessel.
#[derive(Clone, Copy)]
struct Steering {
    /// Angular acceleration per unit input about local X (pitch), Y (roll) and Z (yaw), rad/ms²; zero until
    /// learned.
    alpha: DVec3,
    /// How long an input change takes to show in the authoritative state, ms.
    lag_ms: f64,
    /// The input held now, local axes.
    input: DVec3,
    /// Input changes not yet in the authoritative state: (change, when), oldest first (at most a few).
    pending: [(DVec3, f64); 4],
    pending_len: usize,
    /// The previous authoritative local spin and when it arrived, for learning.
    last: Option<(DVec3, f64)>,
}

impl Default for Steering {
    fn default() -> Self {
        Self {
            alpha: DVec3::ZERO,
            lag_ms: STEER_LAG_MS,
            input: DVec3::ZERO,
            pending: [(DVec3::ZERO, 0.0); 4],
            pending_len: 0,
            last: None,
        }
    }
}

impl Steering {
    fn pending(&self) -> &[(DVec3, f64)] {
        &self.pending[..self.pending_len]
    }

    /// The input the authoritative state has been answering: what is held less what it has not seen yet.
    fn seen_input(&self) -> DVec3 {
        self.pending()
            .iter()
            .fold(self.input, |u, (change, _)| u - *change)
    }

    fn set_input(&mut self, input: DVec3, now_ms: f64) {
        let change = input - self.input;
        if change.length_squared() < 1e-12 {
            return;
        }
        self.input = input;
        if self.pending_len == self.pending.len() {
            // fold the oldest into the next: they show together
            self.pending[1].0 += self.pending[0].0;
            self.pending.copy_within(1.., 0);
            self.pending_len -= 1;
        }
        self.pending[self.pending_len] = (change, now_ms);
        self.pending_len += 1;
    }

    /// A new authoritative spin (local, rad/ms) arrived at `received_ms`: learn from it and drop the input
    /// changes it now shows.
    fn observe(&mut self, velocity: DVec3, received_ms: f64) {
        if let Some((before, at)) = self.last {
            let dt = received_ms - at;
            if dt > 1.0 && dt <= STALE_MS {
                let accel = (velocity - before) / dt;
                let seen = self.seen_input();
                let learn = |alpha: f64, a: f64, u: f64| {
                    if u.abs() >= 0.5 && (a / u).is_finite() {
                        // the first sample is taken whole, later ones blended in
                        if alpha == 0.0 {
                            a / u
                        } else {
                            alpha + (a / u - alpha) * LEARN_RATE
                        }
                    } else {
                        alpha
                    }
                };
                // a change showing for the first time: its lag, when the vessel answers it the way it should
                if let Some(&(change, since)) = self.pending().first() {
                    let expect = self.alpha * change;
                    let answered = (0..3).any(|k| {
                        expect[k].abs() > 1e-12
                            && accel[k] * expect[k] > 0.0
                            && (accel[k] - (self.alpha * seen)[k]).abs() > 0.3 * expect[k].abs()
                    });
                    if answered {
                        let lag = (received_ms - since).clamp(STEER_LAG_MIN_MS, STEER_LAG_MAX_MS);
                        self.lag_ms += (lag - self.lag_ms) * LEARN_RATE;
                    }
                }
                // only while the state answers input held all along (no change in between)
                if self
                    .pending()
                    .first()
                    .is_none_or(|(_, since)| *since > received_ms)
                {
                    self.alpha = DVec3::new(
                        learn(self.alpha.x, accel.x, seen.x),
                        learn(self.alpha.y, accel.y, seen.y),
                        learn(self.alpha.z, accel.z, seen.z),
                    );
                }
            }
        }
        self.last = Some((velocity, received_ms));
        let lag = self.lag_ms;
        let keep: Vec<(DVec3, f64)> = self
            .pending()
            .iter()
            .copied()
            .filter(|(_, since)| received_ms < since + lag)
            .collect();
        self.pending_len = keep.len();
        self.pending[..keep.len()].copy_from_slice(&keep);
    }

    /// The turn the pending input adds by `now_ms`, local rotation vector, rad: each change spins the vessel up
    /// (at its learned rate) from when it was made until the server's state shows it.
    fn extra(&self, now_ms: f64) -> DVec3 {
        self.pending()
            .iter()
            .map(|(change, since)| {
                let t = (now_ms - since).clamp(0.0, self.lag_ms);
                self.alpha * *change * (0.5 * t * t)
            })
            .fold(DVec3::ZERO, |a, b| a + b)
    }
}

/// Exported meshes mirror Unity X, while physics mirrors Unity Z. A half-turn
/// around Y converts the exported craft frame to the physics vessel frame.
pub fn mesh_to_vessel() -> DQuat {
    DQuat::from_xyzw(0.0, 1.0, 0.0, 0.0)
}

/// Place exported craft geometry around the simulation's current centre of mass.
pub fn mesh_origin(centre: ose_math::DVec3) -> DVec3 {
    mesh_to_vessel().inverse() * -DVec3::new(centre.x, centre.y, centre.z)
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
    steering: Steering,
}

impl Track {
    /// Use authoritative spin rather than inferring it from two delayed poses. This
    /// avoids one extra snapshot interval of apparent rotation after SAS has stopped.
    pub fn set_world_velocity(&mut self, omega: DVec3) {
        self.velocity = if omega.is_finite() && !self.context.grounded {
            self.rotation.inverse() * omega * (self.context.warp as f64 / 1000.0)
        } else {
            DVec3::ZERO
        };
        if !self.context.grounded {
            // input changes the state shows now stop being predicted: what they had added on screen joins
            // the correction and fades out with it
            let before = self.steering.extra(self.received_ms);
            self.steering.observe(self.velocity, self.received_ms);
            let after = self.steering.extra(self.received_ms);
            self.correction = (DQuat::from_scaled_axis(after).inverse()
                * DQuat::from_scaled_axis(before)
                * self.correction)
                .normalize();
        }
    }

    /// The pilot's steering input held now (pitch, yaw, roll, each -1..1), so the vessel starts turning on
    /// screen before the server's state shows it.
    pub fn steer(&mut self, pitch: f64, yaw: f64, roll: f64, now_ms: f64) {
        self.steering
            .set_input(DVec3::new(pitch, roll, yaw), now_ms);
    }
    pub fn new(rotation: DQuat, ut: f64, received_ms: f64, context: Context) -> Self {
        Self {
            rotation,
            correction: DQuat::IDENTITY,
            velocity: DVec3::ZERO,
            ut,
            received_ms,
            context,
            steering: Steering::default(),
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
            // what was learned of the vessel carries over; what was pending does not
            let steering = Steering {
                pending_len: 0,
                last: None,
                ..self.steering
            };
            *self = Self::new(rotation, ut, received_ms, context);
            self.steering = steering;
            return;
        }
        let displayed = self.sample(received_ms);
        self.velocity = (self.rotation.inverse() * rotation).to_scaled_axis() / dt_ms;
        self.rotation = rotation;
        // Preserve the displayed pose at arrival, then remove prediction error over a short window. The
        // steering still predicted stays on top of the new pose (`sample`), so it is left out here.
        let steering = if self.context.grounded {
            DQuat::IDENTITY
        } else {
            DQuat::from_scaled_axis(self.steering.extra(received_ms))
        };
        self.correction = (steering.inverse() * rotation.inverse() * displayed).normalize();
        self.ut = ut;
        self.received_ms = received_ms;
    }

    pub fn sample(&self, now_ms: f64) -> DQuat {
        let age = (now_ms - self.received_ms).max(0.0);
        let mut advance = self.velocity * age.min(PREDICT_MS);
        if !self.context.grounded {
            advance += self.steering.extra(now_ms);
        }
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

/// Closest and farthest the flight camera zooms (m).
pub const MIN_DISTANCE: f64 = 4.0;
pub const MAX_DISTANCE: f64 = 5.0e6;

/// The original's starting camera distance for a vessel (BasicCameraRigSolution.UpdateDefaultGimbalStateDistance):
/// half the craft's longest box side over tan(half the 60 degree field of view), plus its vessel-switch backoff
/// (2.5) and padding (15 m). A three-part rocket sits about 21 m out, a 40 m launcher about 52 m.
pub fn fit_distance(extent: DVec3) -> f64 {
    let half = if extent.is_finite() {
        extent.max_element().max(0.0) * 0.5
    } else {
        0.0
    };
    (half / (60f64.to_radians() * 0.5).tan() + 2.5 + 15.0).clamp(MIN_DISTANCE, MAX_DISTANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_camera_starts_a_few_craft_lengths_out_like_the_original() {
        // Tin Can + FL-T200 + LV-T45: about 1.25 m wide, 4 m tall.
        let small = fit_distance(DVec3::new(1.25, 4.0, 1.25));
        assert!((small - (2.0 * 3f64.sqrt() + 17.5)).abs() < 1e-9, "{small}");
        // A 40 m launcher stands about 52 m out, still well inside the zoom range.
        let big = fit_distance(DVec3::new(10.0, 40.0, 10.0));
        assert!((51.0..53.0).contains(&big), "{big}");
        // Nothing loaded yet (an empty box) or junk bounds: the padding alone, never NaN.
        let empty = fit_distance(DVec3::splat(f64::NEG_INFINITY));
        assert_eq!(empty, 17.5);
        assert!(fit_distance(DVec3::splat(1.0e9)) <= MAX_DISTANCE);
        assert!(fit_distance(DVec3::ZERO) >= MIN_DISTANCE);
    }

    #[test]
    fn authoritative_spin_stops_prediction_after_release_despite_jitter() {
        let mut track = Track::new(DQuat::IDENTITY, 0., 0., CONTEXT);
        track.set_world_velocity(DVec3::Z);
        close(track.sample(80.), DQuat::from_rotation_z(0.08));
        track.update(DQuat::from_rotation_z(0.1), 0.1, 130., CONTEXT);
        track.set_world_velocity(DVec3::ZERO);
        close(track.sample(200.), DQuat::from_rotation_z(0.1));
        close(track.sample(350.), DQuat::from_rotation_z(0.1));
        track.set_world_velocity(DVec3::splat(f64::NAN));
        close(track.sample(360.), DQuat::from_rotation_z(0.1));
    }

    #[test]
    fn craft_rotates_about_wet_mass_centre_with_no_bottom_anchor() {
        let centre = ose_math::DVec3::new(0.3, 6.0, -0.4);
        let mesh_centre = DVec3::new(-0.3, 6.0, 0.4);
        let origin = mesh_origin(centre);
        let vessel = DVec3::new(100.0, 500.0, 200.0);
        let engine = DVec3::new(0.0, 0.0, 0.0);
        for rotation in [
            DQuat::IDENTITY,
            DQuat::from_rotation_z(0.6),
            DQuat::from_rotation_x(1.0),
        ] {
            assert!((vessel + rotation * (mesh_centre + origin) - vessel).length() < 1e-9);
        }
        assert!(
            (DQuat::from_rotation_z(0.6) * (engine + origin) - (engine + origin)).length() > 3.0
        );
    }

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

    /// A vessel the server turns about yaw at `alpha` rad/s² per unit input, answering input `lag_ms` late,
    /// published every 100 ms; the pilot holds `input(t)`. Returns the track and the ideal yaw (no lag) by
    /// time, rad.
    fn steered(
        input: impl Fn(f64) -> f64,
        alpha: f64,
        lag_ms: f64,
        until_ms: f64,
        mut probe: impl FnMut(&Track, f64, f64),
    ) {
        let (mut theta, mut omega) = (0.0, 0.0);
        let (mut ideal, mut ideal_omega) = (0.0, 0.0);
        let mut track = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
        let mut t = 0.0;
        while t < until_ms {
            // 1 ms steps: the server answers late, the ideal at once
            let u = input(t - lag_ms);
            omega += alpha * u / 1000.0;
            theta += omega / 1000.0;
            ideal_omega += alpha * input(t) / 1000.0;
            ideal += ideal_omega / 1000.0;
            t += 1.0;
            track.steer(0.0, input(t), 0.0, t);
            if (t as u64).is_multiple_of(100) {
                track.update(DQuat::from_rotation_z(theta), t / 1000.0, t, CONTEXT);
                track.set_world_velocity(DVec3::Z * omega);
            }
            probe(&track, t, ideal);
        }
    }

    #[test]
    fn the_pilots_steering_shows_at_once_and_hands_over_to_the_server_smoothly() {
        // yaw left then right to stop (which teaches the client how the craft answers), then yaw again
        let input = |t: f64| {
            if (1000.0..1400.0).contains(&t) || (3000.0..3400.0).contains(&t) {
                1.0
            } else if (1400.0..1800.0).contains(&t) {
                -1.0
            } else {
                0.0
            }
        };
        let angle = |q: DQuat| q.to_euler(glam::EulerRot::ZYX).0;
        let mut previous: Option<f64> = None;
        let mut biggest_step: f64 = 0.0;
        let mut at_press = (0.0, 0.0);
        let mut first_press_shown = f64::NAN;
        steered(input, 0.5, 150.0, 4500.0, |track, t, ideal| {
            let shown = angle(track.sample(t));
            if t > 2000.0 {
                if let Some(p) = previous {
                    biggest_step = biggest_step.max((shown - p).abs());
                }
                previous = Some(shown);
            }
            if t == 1000.0 || t == 3000.0 {
                at_press = (shown, ideal);
            }
            if t == 1100.0 {
                first_press_shown = shown - at_press.0;
            }
            if t == 3100.0 {
                // the server has not answered yet (150 ms), but the craft is already turning on screen,
                // close to where it would be with no delay at all (0.5 rad/s² for 0.1 s: 2.5 mrad)
                let (shown, ideal) = (shown - at_press.0, ideal - at_press.1);
                assert!((ideal - 0.0025).abs() < 1e-4, "{ideal}");
                assert!(
                    (shown - ideal).abs() < 0.25 * ideal,
                    "shown {shown} ideal {ideal}"
                );
            }
        });
        // before anything was learned the first press showed nothing early
        assert!(first_press_shown.abs() < 1e-9, "{first_press_shown}");
        // no jumps as the server's state takes over: never much more than the craft's own fastest turn
        // (0.2 rad/s, 0.2 mrad per ms)
        assert!(biggest_step < 0.0005, "{biggest_step} rad in one ms");
        // without steering nothing is added
        let mut quiet = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
        quiet.update(yaw(0.0), 0.1, 100.0, CONTEXT);
        close(quiet.sample(150.0), yaw(0.0));
    }

    #[test]
    fn prediction_angle_is_bounded_even_for_fast_rolls() {
        let mut track = Track::new(yaw(0.0), 0.0, 0.0, CONTEXT);
        track.update(yaw(80.0), 0.02, 20.0, CONTEXT);
        close(track.sample(120.0), yaw(110.0));
    }
}

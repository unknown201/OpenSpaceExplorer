//! Ascent from the ground to a low circular orbit: vertical climb, a linear pitch-over, throttle held
//! near 1.5 g low in the atmosphere, coast to apoapsis, then a circularization burn.
//!
//! The profile is the one the ISS assembly test pilot (`ose-sim/examples/iss_assembly.rs`) flew ten
//! deliveries with. Airless ascents (from the Mun) are R5.
use crate::{acceleration, body, orbit, stage_if_spent, Status};
use bevy_ecs::entity::Entity;
use ose_sim::{flight::Attitude, Universe};

#[derive(Clone, Debug)]
pub struct AscentParams {
    /// Altitude where the pitch-over starts (m).
    pub turn_start_m: f64,
    /// Altitude where the vessel is horizontal (m).
    pub turn_end_m: f64,
    /// Stop the climb burn once apoapsis reaches this altitude (m).
    pub apoapsis_m: f64,
    /// The ascent is done once periapsis is above this altitude (m).
    pub periapsis_m: f64,
    /// Compass heading (degrees, 90 = east).
    pub heading_deg: f64,
    /// Below this altitude the throttle holds a thrust-to-weight of `max_twr` (m).
    pub throttle_limit_below_m: f64,
    pub max_twr: f64,
    /// Stage once in orbit, for a launcher that has to be dropped (for example from a docking port).
    pub stage_in_orbit: bool,
    /// Give up after this much flight time (s).
    pub timeout_s: f64,
}

impl Default for AscentParams {
    fn default() -> Self {
        Self {
            turn_start_m: 1_000.0,
            turn_end_m: 45_000.0,
            apoapsis_m: 140_000.0,
            periapsis_m: 85_000.0,
            heading_deg: 90.0,
            throttle_limit_below_m: 20_000.0,
            max_twr: 1.5,
            stage_in_orbit: false,
            timeout_s: 1_500.0,
        }
    }
}

/// Pitch above the horizon (degrees) for an altitude.
pub fn pitch_deg(p: &AscentParams, alt_m: f64) -> f64 {
    if alt_m < p.turn_start_m {
        return 90.0;
    }
    let span = (p.turn_end_m - p.turn_start_m).max(1.0);
    (90.0 * (1.0 - (alt_m - p.turn_start_m) / span)).clamp(0.0, 90.0)
}

pub struct Ascent {
    pub params: AscentParams,
    started: Option<f64>,
    coasting: bool,
    /// When the engines were cut in orbit, before dropping the launcher.
    cutoff: Option<f64>,
    tick: u64,
}

impl Ascent {
    pub fn new(params: AscentParams) -> Self {
        Self {
            params,
            started: None,
            coasting: false,
            cutoff: None,
            tick: 0,
        }
    }

    pub fn update(&mut self, u: &mut Universe, e: Entity) -> Status {
        u.wake(e);
        let Some(p) = u.powered(e).cloned() else {
            return Status::Failed("vessel is gone".into());
        };
        if p.crashed {
            return Status::Failed("crashed during ascent".into());
        }
        let start = *self.started.get_or_insert_with(|| {
            u.stage(e);
            u.ut()
        });
        let (Some(o), Some((_, mu, radius))) = (orbit(u, e), body(u, e)) else {
            return Status::Failed("no orbit".into());
        };
        let alt = p.r.length() - radius;
        if self.cutoff.is_some()
            || (o.eccentricity() < 1.0 && o.periapsis() > radius + self.params.periapsis_m)
        {
            // Cut the engines and let the throttle spool down before dropping the launcher, as a
            // player would. Separating at full thrust lets the booster ram the payload: in a test
            // that wrecked the ISS core (9.2 t down to 3 t) and cost it 60 m/s.
            let cut = *self.cutoff.get_or_insert_with(|| {
                u.set_controls(e, 0.0, Attitude::OrbitPrograde);
                u.ut()
            });
            if u.ut() - cut < 2.0 || p.throttle > 0.0 {
                return Status::Running;
            }
            if self.params.stage_in_orbit {
                u.stage(e);
            }
            let fore = u
                .powered(e)
                .map(|p| crate::control_axes(p).1)
                .unwrap_or(p.facing);
            u.set_controls(e, 0.0, Attitude::Fixed(fore));
            return Status::Done;
        }
        if u.ut() - start > self.params.timeout_s {
            return Status::Failed(format!(
                "no orbit after {:.0} s: altitude {alt:.0} m, periapsis {:.0} m, apoapsis {:.0} m",
                self.params.timeout_s,
                o.periapsis() - radius,
                o.apoapsis() - radius
            ));
        }
        if stage_if_spent(u, e) {
            return Status::Running;
        }
        if p.propellant_t < 0.01 {
            return Status::Failed(format!(
                "out of propellant at {alt:.0} m, periapsis {:.0} m, apoapsis {:.0} m",
                o.periapsis() - radius,
                o.apoapsis() - radius
            ));
        }
        if !self.coasting
            && o.eccentricity() < 1.0
            && o.apoapsis() - radius > self.params.apoapsis_m
        {
            self.coasting = true;
        }
        let accel = acceleration(u, e);
        let (throttle, attitude) = if !self.coasting {
            let g = mu / p.r.length_squared();
            let throttle = if alt < self.params.throttle_limit_below_m && accel > 0.0 {
                (g * self.params.max_twr / accel).clamp(0.3, 1.0)
            } else {
                1.0
            };
            (
                throttle,
                Attitude::PitchHeading {
                    pitch_deg: pitch_deg(&self.params, alt),
                    heading_deg: self.params.heading_deg,
                },
            )
        } else {
            let time = o.time_to_true_anomaly(std::f64::consts::PI, u.ut());
            let needed = ((mu / p.r.length()).sqrt() - p.v.length()).max(0.0);
            let firing = time < needed / accel.max(0.01) * 0.65 + 8.0 || time > o.period - 20.0;
            (if firing { 1.0 } else { 0.0 }, Attitude::OrbitPrograde)
        };
        // Commands at 10 Hz, like a player's client sends them.
        if self.tick.is_multiple_of(5) {
            u.set_controls(e, throttle, attitude);
        }
        self.tick += 1;
        Status::Running
    }

    /// Ticks per step a headless harness can run at.
    pub fn suggested_warp(&self) -> u32 {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_goes_from_vertical_to_horizontal() {
        let p = AscentParams::default();
        assert_eq!(pitch_deg(&p, 0.0), 90.0);
        assert_eq!(pitch_deg(&p, 23_000.0), 45.0);
        assert_eq!(pitch_deg(&p, 60_000.0), 0.0);
    }
}

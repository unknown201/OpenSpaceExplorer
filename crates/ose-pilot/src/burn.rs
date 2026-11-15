//! A finite burn: coast until shortly before the burn, turn to the burn direction, then fire so the
//! burn is centred on its planned time, and cut off on measured delta-v (gravity removed).
//!
//! The engines are assumed to push along the craft's fore axis until the first half metre per second
//! shows whether they push forward or backward along it. After a vessel splits, its frame can come out reversed relative to
//! its engines (a module undocked by its forward port), and the pilot then steers by what it measured,
//! as a player would after watching the navball.
use crate::{acceleration, aim, body, stage_if_spent, state, Status};
use bevy_ecs::entity::Entity;
use ose_math::{rendezvous, DVec3};
use ose_sim::{flight::Attitude, Universe};

/// Seconds before ignition to start turning.
const ALIGN_LEAD_S: f64 = 90.0;
/// Remaining delta-v (m/s) at which the burn counts as done.
const DONE_MPS: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Coast,
    Align,
    Fire,
}

pub struct Burn {
    pub delta_v: DVec3,
    /// When the middle of the burn should happen (UT, s).
    pub centre_ut: f64,
    phase: Phase,
    left: DVec3,
    last: Option<(DVec3, DVec3, f64)>,
    fire_started: f64,
    /// Delta-v delivered while firing so far, inertial, and the measured thrust axis in the craft's
    /// own (right, fore, top) axes.
    delivered: DVec3,
    thrust_axis: Option<DVec3>,
}

/// Where the engines push, inertial: the craft's fore axis, or the measured axis.
fn thrust_direction(p: &ose_sim::flight::Powered, measured: Option<DVec3>) -> DVec3 {
    match measured {
        None => p.facing,
        Some(a) => {
            let (x, y, z) = ose_sim::flight::vessel_basis(p);
            (x * a.x + y * a.y + z * a.z).normalized()
        }
    }
}

impl Burn {
    pub fn new(delta_v: DVec3, centre_ut: f64) -> Self {
        Self {
            delta_v,
            centre_ut,
            phase: Phase::Coast,
            left: delta_v,
            last: None,
            fire_started: 0.0,
            delivered: DVec3::ZERO,
            thrust_axis: None,
        }
    }

    pub fn remaining(&self) -> DVec3 {
        self.left
    }

    fn ignition_ut(&self, accel: f64) -> f64 {
        self.centre_ut - self.delta_v.length() / accel.max(1e-3) * 0.5
    }

    pub fn update(&mut self, u: &mut Universe, e: Entity) -> Status {
        let direction = self.delta_v.normalized();
        if self.delta_v.length() < DONE_MPS {
            return Status::Done;
        }
        if self.phase == Phase::Coast {
            // The vessel may stay on rails while it coasts.
            let accel = acceleration(u, e);
            if u.ut() < self.ignition_ut(accel) - ALIGN_LEAD_S {
                return Status::Running;
            }
            self.phase = Phase::Align;
        }
        u.wake(e);
        let Some(p) = u.powered(e).cloned() else {
            return Status::Failed("vessel is gone".into());
        };
        if p.crashed {
            return Status::Failed("crashed during a burn".into());
        }
        let accel = acceleration(u, e);
        // Steer the engines' thrust axis onto the burn.
        let axis = thrust_direction(&p, self.thrust_axis);
        let hold = Attitude::Fixed(aim(&p, axis, direction));
        if self.phase == Phase::Align {
            u.set_controls(e, 0.0, hold);
            if accel <= 0.0 && !stage_if_spent(u, e) {
                return Status::Failed("no engine for the burn".into());
            }
            // Ignite on time even if the turn has not quite settled: the facing check while firing
            // keeps the throttle closed until it points the right way.
            if u.ut() >= self.ignition_ut(accel) {
                self.phase = Phase::Fire;
                self.fire_started = u.ut();
                self.last = Some((p.r, p.v, u.ut()));
            }
            return Status::Running;
        }
        // Fire.
        if let (Some((r0, v0, t0)), Some((r1, v1)), Some((_, mu, _))) =
            (self.last, state(u, e), body(u, e))
        {
            if u.ut() > t0 {
                let got = rendezvous::delivered_delta_v(mu, r0, v0, r1, v1, u.ut() - t0);
                self.left = self.left - got;
                self.delivered = self.delivered + got;
                if self.thrust_axis.is_none() && self.delivered.length() > 0.5 {
                    // Only the sign is taken: RCS holding attitude adds a little sideways push, and
                    // engines that sit off the axis are not something a stock craft does.
                    let fore = if self.delivered.dot(p.facing) < 0.0 {
                        -1.0
                    } else {
                        1.0
                    };
                    self.thrust_axis = Some(DVec3::new(0.0, fore, 0.0));
                }
            }
        }
        self.last = Some((p.r, p.v, u.ut()));
        if self.left.dot(direction) <= DONE_MPS {
            u.set_controls(e, 0.0, hold);
            return Status::Done;
        }
        if u.ut() - self.fire_started > 900.0 {
            u.set_controls(e, 0.0, hold);
            return Status::Failed(format!(
                "burn took too long, {:.1} m/s left",
                self.left.length()
            ));
        }
        if stage_if_spent(u, e) {
            return Status::Running;
        }
        if accel <= 0.0 || p.propellant_t <= 0.0 {
            u.set_controls(e, 0.0, hold);
            return Status::Failed(format!(
                "out of propellant, {:.1} m/s left",
                self.left.length()
            ));
        }
        let throttle = if axis.dot(direction) > 0.999 {
            (self.left.dot(direction) / accel / 0.5).clamp(0.0, 1.0)
        } else {
            0.0
        };
        u.set_controls(e, throttle, hold);
        Status::Running
    }

    /// Ticks per step a headless harness can run at (50 ticks per simulated second).
    pub fn suggested_warp(&self, u: &Universe, e: Entity) -> u32 {
        match self.phase {
            Phase::Fire => 1,
            Phase::Align => 4,
            Phase::Coast => {
                let to_go = self.ignition_ut(acceleration(u, e)) - ALIGN_LEAD_S - u.ut();
                ((to_go * 50.0).round() as u32).clamp(1, 1000)
            }
        }
    }
}

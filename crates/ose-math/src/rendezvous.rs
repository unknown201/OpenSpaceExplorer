//! Advisory two-body rendezvous. Nothing here schedules or executes a maneuver.
//! Universal-variable Lambert boundary-value solution, followed by independent Kepler propagation.

use crate::{DVec3, Orbit};

/// Velocity delivered by thrust between authoritative samples, excluding central gravity.
pub fn delivered_delta_v(mu: f64, r0: DVec3, v0: DVec3, r1: DVec3, v1: DVec3, dt: f64) -> DVec3 {
    let g0 = r0 * (-mu / r0.length().powi(3));
    let g1 = r1 * (-mu / r1.length().powi(3));
    v1 - v0 - (g0 + g1) * (0.5 * dt)
}
use std::f64::consts::{PI, TAU};

fn finite(v: DVec3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

fn stumpff(z: f64) -> (f64, f64) {
    if z > 1e-6 {
        let x = z.sqrt();
        ((1.0 - x.cos()) / z, (x - x.sin()) / x.powi(3))
    } else if z < -1e-6 {
        let x = (-z).sqrt();
        ((1.0 - x.cosh()) / z, (x.sinh() - x) / x.powi(3))
    } else {
        (
            0.5 - z / 24.0 + z * z / 720.0,
            1.0 / 6.0 - z / 120.0 + z * z / 5040.0,
        )
    }
}

/// Single-revolution solution with angular momentum toward `normal`. Degenerate endpoints and
/// invalid inputs return None, rather than fabricating a direction or an encounter.
pub fn lambert(
    mu: f64,
    from: DVec3,
    to: DVec3,
    seconds: f64,
    normal: DVec3,
) -> Option<(DVec3, DVec3)> {
    lambert_revolutions(mu, from, to, seconds, normal, 0)
}

/// Multi-revolution upper-time branch. Arrival is verified independently by `plan`.
pub fn lambert_revolutions(
    mu: f64,
    from: DVec3,
    to: DVec3,
    seconds: f64,
    normal: DVec3,
    revolutions: u32,
) -> Option<(DVec3, DVec3)> {
    if !mu.is_finite()
        || mu <= 0.0
        || !seconds.is_finite()
        || seconds <= 0.0
        || !finite(from)
        || !finite(to)
        || !finite(normal)
        || normal.length() < 0.5
    {
        return None;
    }
    let (r1, r2) = (from.length(), to.length());
    if r1 < 1.0 || r2 < 1.0 {
        return None;
    }
    let cos = (from.dot(to) / (r1 * r2)).clamp(-1.0, 1.0);
    let sin = (1.0 - cos * cos).max(0.0).sqrt()
        * if from.cross(to).dot(normal) >= 0.0 {
            1.0
        } else {
            -1.0
        };
    if sin.abs() < 1e-7 || (1.0 - cos) < 1e-12 {
        return None;
    }
    let a = sin * (r1 * r2 / (1.0 - cos)).sqrt();
    let evaluate = |z: f64| -> Option<(f64, f64)> {
        let (c, s) = stumpff(z);
        if c <= 0.0 {
            return None;
        }
        let y = r1 + r2 + a * (z * s - 1.0) / c.sqrt();
        if y < 0.0 {
            return None;
        }
        let t = ((y / c).powf(1.5) * s + a * y.sqrt()) / mu.sqrt();
        t.is_finite().then_some((t, y))
    };
    // The zero-revolution time is monotone on this interval. Scan also finds the lower feasible y.
    let mut bracket = None;
    let mut previous = None;
    let lower = if revolutions == 0 {
        -4.0 * PI * PI
    } else {
        (TAU * revolutions as f64).powi(2) + 1e-5
    };
    let upper = (TAU * (revolutions + 1) as f64).powi(2) - 1e-5;
    for i in 0..=192 {
        let z = lower + (upper - lower) * i as f64 / 192.0;
        if let Some((t, _)) = evaluate(z) {
            if let Some((old_z, old_t)) = previous {
                if old_t <= seconds && t >= seconds {
                    bracket = Some((old_z, z));
                    break;
                }
            }
            previous = Some((z, t));
        }
    }
    let (mut lo, mut hi) = bracket?;
    for _ in 0..64 {
        let mid = (lo + hi) * 0.5;
        if evaluate(mid)?.0 < seconds {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let y = evaluate((lo + hi) * 0.5)?.1;
    let f = 1.0 - y / r1;
    let g = a * (y / mu).sqrt();
    let gd = 1.0 - y / r2;
    if g.abs() < 1e-8 {
        return None;
    }
    let v1 = (to - from * f) * g.recip();
    let v2 = (to * gd - from) * g.recip();
    (finite(v1) && finite(v2)).then_some((v1, v2))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transfer {
    pub departure_ut: f64,
    pub arrival_ut: f64,
    pub delta_v: DVec3,
    pub arrival_delta_v: DVec3,
    pub orbit: Orbit,
    pub miss_m: f64,
}

impl Transfer {
    pub fn total_delta_v(&self) -> f64 {
        self.delta_v.length() + self.arrival_delta_v.length()
    }
}

/// Search departure times and encounter durations, rejecting atmosphere-crossing transfers and
/// independently verifying their arrival. `budget_mps` includes arrival braking, not just departure.
pub fn plan(
    chaser: Orbit,
    target: Orbit,
    now: f64,
    minimum_radius: f64,
    budget_mps: f64,
) -> Option<Transfer> {
    if !now.is_finite()
        || !minimum_radius.is_finite()
        || minimum_radius <= 0.0
        || !budget_mps.is_finite()
        || budget_mps <= 0.0
        || chaser.mu != target.mu
        || !chaser.period.is_finite()
        || !target.period.is_finite()
        || chaser.periapsis() < minimum_radius
        || target.periapsis() < minimum_radius
    {
        return None;
    }
    let period = chaser.period.min(target.period);
    if !(60.0..1e9).contains(&period) {
        return None;
    }
    let mut best: Option<(f64, Transfer)> = None;
    for departure in 0..=8 {
        let at = now + 45.0 + period * 0.5 * departure as f64 / 8.0;
        let (r, v) = chaser.state_at(at);
        for duration in 0..=48 {
            let dt = period * (0.08 + 1.4 * duration as f64 / 48.0);
            let arrival = at + dt;
            let (tr, tv) = target.state_at(arrival);
            let Some((v1, v2)) = lambert(chaser.mu, r, tr, dt, target.frame.z) else {
                continue;
            };
            let transfer = Orbit::from_state_vectors(chaser.mu, r, v1, at);
            if !transfer.period.is_finite() || transfer.periapsis() < minimum_radius {
                continue;
            }
            let miss = (transfer.state_at(arrival).0 - tr).length();
            if !miss.is_finite() || miss > 5.0 {
                continue;
            }
            let candidate = Transfer {
                departure_ut: at,
                arrival_ut: arrival,
                delta_v: v1 - v,
                arrival_delta_v: tv - v2,
                orbit: transfer,
                miss_m: miss,
            };
            let dv = candidate.total_delta_v();
            if dv > budget_mps || candidate.delta_v.length() < 0.05 {
                continue;
            }
            // Fuel dominates; a small time cost prefers a timely encounter over a marginal saving.
            let score = dv + (arrival - now) / period * 3.0;
            if best.is_none_or(|(old, _)| score < old) {
                best = Some((score, candidate));
            }
        }
    }
    // A later delivery can be almost half an orbit away in the same orbit. A zero-revolution
    // intercept may need more propellant than it carries; phasing over several revolutions is valid
    // and cheaper. An expensive immediate intercept must not hide a fuel-efficient phasing option.
    let economical = (budget_mps * 0.25).min(150.0);
    if best.is_none_or(|(_, p)| p.total_delta_v() > economical) {
        for revolutions in 1..=8 {
            for departure in 0..=2 {
                let at = now + 60.0 + period * departure as f64 * 0.25;
                let (r, v) = chaser.state_at(at);
                for duration in 0..=48 {
                    let dt = period * (revolutions as f64 + 0.1 + 1.3 * duration as f64 / 48.0);
                    let arrival = at + dt;
                    let (tr, tv) = target.state_at(arrival);
                    let Some((v1, v2)) =
                        lambert_revolutions(chaser.mu, r, tr, dt, target.frame.z, revolutions)
                    else {
                        continue;
                    };
                    let orbit = Orbit::from_state_vectors(chaser.mu, r, v1, at);
                    if !orbit.period.is_finite() || orbit.periapsis() < minimum_radius {
                        continue;
                    }
                    let miss = (orbit.state_at(arrival).0 - tr).length();
                    let candidate = Transfer {
                        departure_ut: at,
                        arrival_ut: arrival,
                        delta_v: v1 - v,
                        arrival_delta_v: tv - v2,
                        orbit,
                        miss_m: miss,
                    };
                    let cost = candidate.total_delta_v();
                    if !miss.is_finite()
                        || miss > 5.0
                        || cost > budget_mps
                        || candidate.delta_v.length() < 0.05
                    {
                        continue;
                    }
                    let score = cost + (arrival - now) / period * 3.0;
                    if best.is_none_or(|(old, _)| score < old) {
                        best = Some((score, candidate));
                    }
                }
            }
            if best.is_some_and(|(_, p)| p.total_delta_v() <= economical) {
                break;
            }
        }
    }
    best.map(|(_, transfer)| transfer)
}

/// Correct an imperfect burn while retaining its encounter time. This avoids replacing a small
/// midcourse adjustment with another long phasing orbit. Fall back to `plan` if it is no longer safe.
pub fn correction(
    chaser: Orbit,
    target: Orbit,
    departure: f64,
    arrival: f64,
    minimum_radius: f64,
    budget: f64,
) -> Option<Transfer> {
    if !departure.is_finite()
        || !arrival.is_finite()
        || arrival - departure < 60.0
        || chaser.mu != target.mu
        || !minimum_radius.is_finite()
        || minimum_radius <= 0.0
        || !budget.is_finite()
        || budget <= 0.0
    {
        return None;
    }
    let (r, v) = chaser.state_at(departure);
    let (tr, tv) = target.state_at(arrival);
    let mut best: Option<Transfer> = None;
    for revolutions in 0..=8 {
        let Some((v1, v2)) = lambert_revolutions(
            chaser.mu,
            r,
            tr,
            arrival - departure,
            target.frame.z,
            revolutions,
        ) else {
            continue;
        };
        let orbit = Orbit::from_state_vectors(chaser.mu, r, v1, departure);
        let miss_m = (orbit.state_at(arrival).0 - tr).length();
        let p = Transfer {
            departure_ut: departure,
            arrival_ut: arrival,
            delta_v: v1 - v,
            arrival_delta_v: tv - v2,
            orbit,
            miss_m,
        };
        if orbit.period.is_finite()
            && orbit.periapsis() >= minimum_radius
            && miss_m.is_finite()
            && miss_m < 5.0
            && p.total_delta_v() <= budget
            && best.is_none_or(|b| p.total_delta_v() < b.total_delta_v())
        {
            best = Some(p);
        }
    }
    best
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelativeMotion {
    pub distance_m: f64,
    pub closing_mps: f64,
    pub speed_mps: f64,
    pub lateral_mps: f64,
    pub stopping_distance_m: Option<f64>,
    pub brake_in_s: Option<f64>,
}

pub fn relative(
    offset: DVec3,
    target_minus_own_velocity: DVec3,
    acceleration: f64,
) -> Option<RelativeMotion> {
    if !finite(offset) || !finite(target_minus_own_velocity) {
        return None;
    }
    let distance = offset.length();
    let closing = -target_minus_own_velocity.dot(offset.normalized());
    let speed = target_minus_own_velocity.length();
    let stopping = (acceleration.is_finite() && acceleration > 1e-8)
        .then(|| speed * speed / (2.0 * acceleration));
    Some(RelativeMotion {
        distance_m: distance,
        closing_mps: closing,
        speed_mps: speed,
        lateral_mps: (speed * speed - closing * closing).max(0.0).sqrt(),
        stopping_distance_m: stopping,
        brake_in_s: stopping
            .filter(|_| closing > 0.01)
            .map(|d| (distance - d - 100.0) / closing),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneWindow {
    pub wait_s: f64,
    pub heading_deg: f64,
    pub plane_error_deg: f64,
}

/// Next surface crossing of a target's orbital plane. This is explicitly a plane opportunity,
/// not a promise of rendezvous: insertion duration and orbital phase still need a transfer plan.
pub fn plane_window(surface: DVec3, spin: f64, normal: DVec3) -> Option<PlaneWindow> {
    if !finite(surface)
        || !finite(normal)
        || !spin.is_finite()
        || spin.abs() < 1e-12
        || surface.length() < 1.0
        || normal.length() < 0.5
    {
        return None;
    }
    let n = normal.normalized();
    let r = surface.normalized();
    let amplitude = r.x.hypot(r.y) * n.x.hypot(n.y);
    let c = n.z * r.z;
    let plane_error = r.dot(n).clamp(-1.0, 1.0).asin().abs().to_degrees();
    let wait = if amplitude < 1e-9 {
        if c.abs() > 1e-5 {
            return None;
        }
        0.0
    } else {
        let ratio = -c / amplitude;
        if ratio.abs() > 1.0 {
            return None;
        }
        let phase = r.y.atan2(r.x) - n.y.atan2(n.x);
        let angle = ratio.acos();
        [angle, -angle]
            .into_iter()
            .map(|a| {
                if spin > 0.0 {
                    (a - phase).rem_euclid(TAU) / spin
                } else {
                    (phase - a).rem_euclid(TAU) / -spin
                }
            })
            .fold(f64::INFINITY, f64::min)
    };
    let angle = spin * wait;
    let (s, co) = angle.sin_cos();
    let up = DVec3::new(r.x * co - r.y * s, r.x * s + r.y * co, r.z);
    let tangent = n.cross(up).normalized();
    let north = (DVec3::new(0.0, 0.0, 1.0) - up * up.z).normalized();
    let east = north.cross(up);
    let heading = tangent
        .dot(east)
        .atan2(tangent.dot(north))
        .to_degrees()
        .rem_euclid(360.0);
    Some(PlaneWindow {
        wait_s: wait,
        heading_deg: heading,
        plane_error_deg: plane_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const MU: f64 = 3.5316e12;
    fn circular(radius: f64, phase: f64) -> Orbit {
        let (s, c) = phase.sin_cos();
        Orbit::from_state_vectors(
            MU,
            DVec3::new(c, s, 0.0) * radius,
            DVec3::new(-s, c, 0.0) * (MU / radius).sqrt(),
            0.0,
        )
    }
    #[test]
    fn lambert_arrives_at_an_independently_propagated_target() {
        for phase in [0.2, 1.0, 2.0, 4.0, 5.7] {
            let target = circular(710_000.0, phase);
            let from = DVec3::new(690_000.0, 0.0, 0.0);
            let arrival = 0.4 * target.period;
            let (to, _) = target.state_at(arrival);
            let (v, _) = lambert(MU, from, to, arrival, target.frame.z).unwrap();
            let actual = Orbit::from_state_vectors(MU, from, v, 0.0)
                .state_at(arrival)
                .0;
            assert!(
                (actual - to).length() < 0.1,
                "phase {phase}: {} m",
                (actual - to).length()
            );
        }
    }
    #[test]
    fn plan_reserves_braking_and_stays_out_of_the_atmosphere() {
        let me = circular(690_000.0, 0.0);
        let target = circular(710_000.0, 0.3);
        let p = plan(me, target, 0.0, 671_000.0, 1000.0).unwrap();
        assert!(p.orbit.periapsis() >= 671_000.0);
        assert!(p.total_delta_v() < 1000.0);
        assert!(p.arrival_delta_v.length() > 0.0);
        assert!(p.miss_m < 0.1);
        assert!(plan(me, target, 0.0, 671_000.0, p.total_delta_v() * 0.1).is_none());
        assert!(plan(me, target, 0.0, 800_000.0, 1000.0).is_none());
    }
    #[test]
    fn sideways_motion_is_not_mistaken_for_a_safe_stop() {
        let r = relative(DVec3::new(100.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0), 0.5).unwrap();
        assert_eq!(r.closing_mps, 0.0);
        assert_eq!(r.lateral_mps, 10.0);
        assert_eq!(r.stopping_distance_m, Some(100.0));
        assert!(relative(DVec3::ZERO, DVec3::ZERO, 0.0)
            .unwrap()
            .stopping_distance_m
            .is_none());
    }
    #[test]
    fn plane_opportunity_is_real_and_unreachable_planes_are_refused() {
        let normal = DVec3::new(0.0, -0.5, 0.75_f64.sqrt());
        let p = plane_window(DVec3::new(0.0, 1.0, 0.0), 0.001, normal).unwrap();
        let a = 0.001 * p.wait_s;
        assert!((DVec3::new(-a.sin(), a.cos(), 0.0).dot(normal)).abs() < 1e-9);
        assert!(plane_window(DVec3::new(0.5, 0.0, 0.75_f64.sqrt()), 0.001, normal).is_none());
    }
    #[test]
    fn invalid_and_degenerate_inputs_do_not_create_a_cue() {
        assert!(lambert(
            MU,
            DVec3::ZERO,
            DVec3::ZERO,
            100.0,
            DVec3::new(0.0, 0.0, 1.0)
        )
        .is_none());
        assert!(plan(
            circular(690_000.0, 0.0),
            circular(710_000.0, 1.0),
            f64::NAN,
            670_000.0,
            1000.0
        )
        .is_none());
    }
    #[test]
    fn a_later_delivery_can_phase_instead_of_requiring_an_expensive_direct_intercept() {
        let a = circular(710_000.0, 0.0);
        let b = circular(710_000.0, 2.7);
        let p = plan(a, b, 0.0, 671_000.0, 300.0).expect("multi-orbit phasing opportunity");
        assert!(p.total_delta_v() < 300.0);
        assert!(p.arrival_ut > a.period);
        assert!((p.orbit.state_at(p.arrival_ut).0 - b.state_at(p.arrival_ut).0).length() < 1.0);
        let generous = plan(a, b, 0.0, 671_000.0, 1500.0).unwrap();
        assert!(
            generous.total_delta_v() < 300.0,
            "extra fuel must not hide the economical phasing option"
        );
    }
    #[test]
    fn a_small_burn_error_can_keep_the_existing_encounter() {
        let target = circular(710_000.0, 0.6);
        let original = plan(circular(710_000.0, 0.0), target, 0.0, 671_000.0, 1500.0).unwrap();
        let at = original.departure_ut + 30.0;
        let (r, v) = original.orbit.state_at(at);
        let perturbed = Orbit::from_state_vectors(MU, r, v + DVec3::new(0.6, 0.1, 0.0), at);
        let p = correction(
            perturbed,
            target,
            at + 180.0,
            original.arrival_ut,
            671_000.0,
            1500.0,
        )
        .unwrap();
        assert_eq!(p.arrival_ut, original.arrival_ut);
        assert!(p.delta_v.length() < 10.0);
        assert!(
            (p.orbit.state_at(p.arrival_ut).0 - target.state_at(p.arrival_ut).0).length() < 5.0
        );
    }
}

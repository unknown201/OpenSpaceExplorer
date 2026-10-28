//! EVA resource resolution. The reference uses force/Isp mass flow and a bounded angular-rate controller.
use crate::flight::{self, Attitude, Powered, StepOutcome};
use ose_math::DVec3;

#[derive(Clone, Debug)]
pub struct Step {
    pub force_kn: DVec3,
    target_rate: DVec3,
    fraction: f64,
    rate_cap: f64,
}

/// Resolve one shared pack before applying movement or rotation. Inputs are in the UI's craft axes.
pub fn resolve(
    p: &mut Powered,
    pressure_atm: f64,
    throttle_allowed: bool,
    omega: f64,
    dt: f64,
) -> Step {
    let settings = p.jetpack.as_ref().expect("configured jetpack");
    let (x, y, z) = flight::vessel_basis(p);
    let world = |v: DVec3| x * v.x + y * v.y + z * v.z;
    let current_rate = p.angvel + y * p.roll_rate;
    let enabled = p.controlled && p.groups.rcs && !p.eva_hop && !p.crashed && dt > 0.0;
    let clamp = |v: DVec3| {
        DVec3::new(
            v.x.clamp(-1.0, 1.0),
            v.y.clamp(-1.0, 1.0),
            v.z.clamp(-1.0, 1.0),
        )
    };
    let mut movement = if enabled {
        clamp(p.rcs.translate)
    } else {
        DVec3::ZERO
    };
    if enabled && throttle_allowed && p.ignited {
        movement.y = (movement.y + p.throttle.clamp(0.0, 1.0)).clamp(-1.0, 1.0);
    }
    let vacuum_isp = settings.isp_curve.evaluate(0.0);
    let pressure_scale = if vacuum_isp > 0.0 {
        (settings.isp_curve.evaluate(pressure_atm.max(0.0)) / vacuum_isp).max(0.0)
    } else {
        0.0
    };
    let mut rotation_effort = 0.0;
    let mut target_rate = current_rate;
    let mut steering = DVec3::ZERO;
    if enabled && !p.landed {
        let raw = p.rotation_input + p.trim;
        let input = clamp(DVec3::new(-raw.x, raw.z + p.roll_input, raw.y));
        let base_rate = settings.rotate_torque_knm * pressure_scale / p.mass_t.max(0.001);
        if input.length_squared() > 1e-12 {
            rotation_effort = input.length();
            target_rate = world(input) * base_rate;
            steering = world(input);
        } else {
            let target = flight::attitude_direction(p.attitude, p.r, p.v, omega);
            let axis = y.cross(target);
            let angle = axis.length().atan2(y.dot(target));
            if p.attitude != Attitude::Free && angle > 1e-8 && base_rate > 0.0 {
                let direction = if axis.length_squared() > 1e-12 {
                    axis.normalized()
                } else {
                    x
                };
                let rate = base_rate.min(angle / dt);
                rotation_effort = rate / base_rate;
                target_rate = direction * rate;
                steering = direction * rotation_effort;
            } else if settings.reset_enabled
                && current_rate.length() > settings.reset_threshold_deg_s.to_radians()
            {
                rotation_effort = 1.0;
                let speed = current_rate.length();
                target_rate = current_rate
                    * ((speed - settings.reset_speed_deg_s.to_radians() * dt).max(0.0) / speed);
                steering = -current_rate.normalized();
            }
        }
    }
    let exhaust = vacuum_isp * ose_data::G0;
    let lever = if settings.rotate_offset_m > 0.0 {
        settings.rotate_offset_m
    } else {
        1.0
    };
    let requested = if exhaust > 0.0 {
        (settings.move_force_kn * movement.length()
            + settings.rotate_torque_knm / lever * rotation_effort)
            / exhaust
            * dt
    } else {
        0.0
    };
    let burn = requested.min(p.propellant_t.max(0.0));
    let fraction = if requested > 0.0 {
        burn / requested
    } else {
        0.0
    };
    let force_kn = world(movement) * (settings.move_force_kn * pressure_scale * fraction);
    p.propellant_t = (p.propellant_t - burn).max(0.0);
    p.mass_t = (p.mass_t - burn).max(0.001);
    p.rcs.powered = if requested > 0.0 {
        fraction
    } else if p.propellant_t > 0.0 {
        1.0
    } else {
        0.0
    };
    p.rcs.usage = (movement.length() + rotation_effort) * fraction;
    p.steer_cmd = steering * fraction;
    Step {
        force_kn,
        target_rate,
        fraction,
        rate_cap: settings.rotate_max_speed_deg_s.to_radians().max(0.0),
    }
}

impl Step {
    pub fn turn(&self, p: &mut Powered, dt: f64, out: &mut StepOutcome) {
        let before = p.angvel + p.facing * p.roll_rate;
        // Resource shortfalls also limit rate changes and braking; an empty pack cannot stop a spin.
        let mut rate = before + (self.target_rate - before) * self.fraction;
        if self.fraction > 0.0 && rate.length() > self.rate_cap {
            rate = rate.normalized() * self.rate_cap;
        }
        let top = flight::vessel_top(p);
        p.facing = flight::rotate(p.facing, rate, dt).normalized();
        if (p.rotation_input.x.abs() > 1e-3 || p.rotation_input.y.abs() > 1e-3)
            && matches!(p.attitude, Attitude::Fixed(_))
        {
            p.attitude = Attitude::Fixed(p.facing);
        }
        let top = flight::rotate(top, rate, dt);
        flight::set_vessel_top(p, (top - p.facing * top.dot(p.facing)).normalized());
        p.roll_rate = rate.dot(p.facing);
        p.angvel = rate - p.facing * p.roll_rate;
        out.angular_accel = (rate - before) * (1.0 / dt);
        out.angvel = rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Powered {
        let mut p = Powered::new(
            flight::FlightSpec {
                stages: vec![flight::StageSpec {
                    start_mass_t: 0.095,
                    propellant_t: 0.025,
                    ..Default::default()
                }],
                drag_area_m2: 0.0,
                drag_cd: 0.0,
            },
            DVec3::new(700000.0, 0.0, 0.0),
            DVec3::new(0.0, 100.0, 0.0),
            false,
        );
        p.jetpack = Some(Default::default());
        p.walker = true;
        p.ignited = true;
        p.groups.rcs = true;
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        p.attitude = Attitude::Free;
        p
    }
    fn body() -> crate::bodies::Body {
        crate::bodies::Body {
            id: "Test".into(),
            mu: 0.0,
            mass_kg: 0.0,
            radius_m: 1.0,
            soi_radius_m: None,
            rotation_period_s: 0.0,
            initial_rotation_deg: 0.0,
            is_star: false,
            ocean: None,
            site: None,
        }
    }
    #[test]
    fn full_pressure_curve_changes_move_and_rotation_without_changing_mass_flow() {
        for (pressure, isp) in [(0.0, 240.0), (1.0, 100.0), (2.5, 27.50059375), (4.0, 0.001)] {
            let mut p = fixture();
            p.throttle = 1.0;
            p.rcs.translate = DVec3::new(1.0, 0.0, 0.0);
            p.rotation_input = DVec3::new(0.0, 1.0, 0.0);
            let before = p.mass_t;
            let (x, y, z) = flight::vessel_basis(&p);
            let burn = (0.25 * 2.0_f64.sqrt() + 0.125 / 0.25) / (240.0 * ose_data::G0) * 0.1;
            let step = resolve(&mut p, pressure, true, 0.0, 0.1);
            assert!((before - p.mass_t - burn).abs() < 1e-12);
            assert!((step.force_kn - (x + y) * (0.25 * isp / 240.0)).length() < 1e-12);
            let mut out = StepOutcome::default();
            step.turn(&mut p, 0.1, &mut out);
            let expected_rate = 0.125 * isp / 240.0 / before;
            assert!((out.angvel - z * expected_rate).length() < 1e-10);
        }
    }
    #[test]
    fn final_partial_move_and_rotation_share_the_pack_and_the_empty_tick_is_free_of_actuation() {
        let dt = 0.02;
        let requested = (0.25 + 0.125 / 0.25) / (240.0 * ose_data::G0) * dt;
        for fraction in [0.0, 0.25, 1.0] {
            let mut p = fixture();
            p.throttle = 1.0;
            p.rotation_input = DVec3::new(0.0, 1.0, 0.0);
            p.propellant_t = requested * fraction;
            p.mass_t = 0.07 + p.propellant_t;
            let mass_before = p.mass_t;
            let v = p.v;
            let (x, y, z) = flight::vessel_basis(&p);
            let out = flight::integrate(&mut p, &body(), None, None, 0.0, dt);
            assert!((p.mass_t - 0.07).abs() < 1e-12 && p.propellant_t < 1e-12);
            assert!((out.thrust_kn - 0.25 * fraction).abs() < 1e-12);
            assert!((p.v - v - y * (0.25 * fraction / 0.07 * dt)).length() < 1e-10);
            let rate = (0.125 * fraction / mass_before).min(280.0_f64.to_radians());
            assert!(
                (out.angvel - z * rate).length() < 1e-10,
                "{fraction}: {:?}",
                out.angvel
            );
            let v = p.v;
            let angular = p.angvel + p.facing * p.roll_rate;
            flight::integrate(&mut p, &body(), None, None, dt, dt);
            assert!((p.v - v).length() < 1e-10);
            assert!((p.angvel + p.facing * p.roll_rate - angular).length() < 1e-10);
            assert!(x.dot(y).abs() < 1e-12);
        }
    }
    #[test]
    fn automatic_braking_requires_fuel_and_idle_spin_with_reset_disabled_consumes_none() {
        let dt = 0.02;
        for fraction in [0.0, 0.25, 1.0] {
            let mut p = fixture();
            p.roll_rate = 0.5;
            let request = 0.125 / 0.25 / (240.0 * ose_data::G0) * dt;
            p.propellant_t = request * fraction;
            p.mass_t = 0.07 + p.propellant_t;
            flight::integrate(&mut p, &body(), None, None, 0.0, dt);
            let expected = 0.5 - 200.0_f64.to_radians() * dt * fraction;
            assert!((p.roll_rate - expected).abs() < 1e-10);
            assert!(p.propellant_t < 1e-12);
        }
        let mut p = fixture();
        p.jetpack.as_mut().unwrap().reset_enabled = false;
        p.roll_rate = 2.0;
        let fuel = p.propellant_t;
        flight::integrate(&mut p, &body(), None, None, 0.0, dt);
        assert_eq!(p.propellant_t, fuel);
        assert!((p.roll_rate - 2.0).abs() < 1e-12);
    }
    #[test]
    fn main_throttle_and_translation_are_one_move_request_and_jumping_disables_the_pack() {
        let mut p = fixture();
        p.throttle = 1.0;
        p.rcs.translate = DVec3::new(0.0, 1.0, 0.0);
        let before = p.propellant_t;
        let step = resolve(&mut p, 0.0, true, 0.0, 0.1);
        assert!((step.force_kn.length() - 0.25).abs() < 1e-12);
        assert!((before - p.propellant_t - 0.25 / (240.0 * ose_data::G0) * 0.1).abs() < 1e-12);
        p.eva_hop = true;
        p.rotation_input = DVec3::new(1.0, 1.0, 1.0);
        let fuel = p.propellant_t;
        let step = resolve(&mut p, 0.0, true, 0.0, 0.1);
        assert_eq!(step.force_kn, DVec3::ZERO);
        assert_eq!(p.propellant_t, fuel);
    }
    #[test]
    fn authored_rate_limit_applies_after_delivery_and_does_not_stop_an_empty_pack() {
        let mut p = fixture();
        p.mass_t = 0.01;
        p.propellant_t = 0.005;
        p.rotation_input = DVec3::new(0.0, 1.0, 0.0);
        let step = resolve(&mut p, 0.0, true, 0.0, 0.02);
        let mut out = StepOutcome::default();
        step.turn(&mut p, 0.02, &mut out);
        assert!((out.angvel.length() - 280.0_f64.to_radians()).abs() < 1e-12);
        p.propellant_t = 0.0;
        p.roll_rate = 6.0;
        p.angvel = DVec3::ZERO;
        let previous = p.facing * 6.0;
        let step = resolve(&mut p, 0.0, true, 0.0, 0.02);
        step.turn(&mut p, 0.02, &mut out);
        assert!((out.angvel - previous).length() < 1e-12);
    }
    #[test]
    fn manual_override_keeps_the_new_hold_and_empty_coast_preserves_the_inertial_top() {
        let mut p = fixture();
        p.attitude = Attitude::Fixed(p.facing);
        p.rotation_input = DVec3::new(1.0, 0.0, 0.0);
        flight::integrate(&mut p, &body(), None, None, 0.0, 0.02);
        assert_eq!(p.attitude, Attitude::Fixed(p.facing));
        p.rotation_input = DVec3::ZERO;
        p.attitude = Attitude::Free;
        p.propellant_t = 0.0;
        p.angvel = DVec3::ZERO;
        p.roll_rate = 0.0;
        p.roll_rad = 0.5;
        p.v = DVec3::new(0.0, 100000.0, 100000.0);
        let top = flight::vessel_top(&p);
        flight::integrate(&mut p, &body(), None, None, 0.02, 0.5);
        assert!((flight::vessel_top(&p) - top).length() < 1e-12);
    }
}

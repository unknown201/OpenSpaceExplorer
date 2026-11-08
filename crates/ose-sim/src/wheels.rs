//! Driving on wheels the way the original does it. Each KSP2 wheel is a VehiclePhysics `Wheel` under a
//! `KSPWheelController` (`Module_WheelBase`, `Module_WheelMotor`, `Module_WheelSteering`,
//! `Module_WheelBrakes`): the motor turns the wheel, the wheel's spin against the ground speed is the tyre
//! slip, and the tyre pushes the craft with a friction force that grows with slip (VPP `TireFriction`,
//! the Linear model with the part's adherent/peak/limit points scaled by `GeeBias`). The craft moves as one
//! rigid body on the ground plane: speed along and across its drive direction and a yaw rate.
//!
//! Ported behaviour, per wheel and fixed step (50 Hz, 8 wheel substeps as `ActiveSubsteps`):
//! - drive, brake and steering inputs slew at `driveResponse`, `BrakeResponse` and `SteeringResponse`
//!   (`Mathf.MoveTowards` in `KSPWheelController`); full brakes hold with `MaxBrakeTorqueRest`;
//! - motor torque is `torqueCurve` at the wheel's rim speed, rounded to 0.01 kN·m, times the drive state
//!   and the charge delivered; traction control scales the input down by the last step's slip;
//! - the wheel's spin inertia is `mass * r^2 / 2`, never under 0.01 t·m² (`Wheel.RecalculateConstants`),
//!   which is what makes a light rover wheel slow to spin up;
//! - the tyre force is limited by the friction curve and by the impulse that would stop the contact
//!   (`tireAdherentImpulseRatio` of the supported mass per step); below the adherent slip the tyre holds
//!   like static friction (`IsAdherentSlip`, `ComputeAdherentForce*`);
//! - the force on the craft is smoothed at `TireForceSharpness` (`VehicleBase.ApplyTireForce`);
//! - each steering wheel turns toward one turn centre found from the craft's centre of mass
//!   (`Module_WheelSteering.FindTurnCenter`/`UpdateSteering`), within a range that narrows with speed
//!   (`SteeringMaxAngleCurve`).
//!
//! What it leaves out: suspension (loads are the craft's weight shared evenly), the ground's own grip and
//! drag (stock ground is 1 and 0), and the contact's sideways slope hold.

use crate::flight::Drive;
use ose_data::WheelDynamics;
use serde::{Deserialize, Serialize};

/// The original's physics step, s (wheels run in `FixedUpdate`).
pub const WHEEL_DT: f64 = 0.02;
/// `Data_WheelBase.ActiveSubsteps`.
const SUBSTEPS: usize = 8;
/// `TireFriction.maxAdherentSpeed`, m/s: no static grip above it.
const MAX_ADHERENT_SPEED: f64 = 6.388889;
/// `VehicleBase.tireAdherentImpulseRatio`.
const ADHERENT_IMPULSE_RATIO: f64 = 0.5;
/// `Data_WheelBase.TireForceSharpness` (stock parts keep the default).
const TIRE_FORCE_SHARPNESS: f64 = 10.0;
/// `KSPWheelController.MaxBrakeTorqueRest`, kN·m: held brakes.
const MAX_BRAKE_TORQUE_REST: f64 = 1000.0;
/// `Wheel.RecalculateConstants`: the least spin inertia, t·m².
const MIN_WHEEL_INERTIA: f64 = 0.01;
/// Slower than this with the brakes holding, the craft is at rest (VPP lets such a vehicle sleep).
const REST_SPEED_MPS: f64 = 0.05;

/// What one wheel is doing: its spin and the slewed inputs, and the tyre force and contact velocity
/// of the last step (sideways, forward).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WheelState {
    pub spin_rad_s: f64,
    pub drive: f64,
    /// Share of the current steering range (`SteerState`).
    pub steer: f64,
    pub brake: f64,
    pub tire_force_kn: [f64; 2],
    pub velocity_mps: [f64; 2],
}

/// VPP `TireFriction` with the Linear curve as `Module_WheelBase.ApplyGeeBias`/`UpdateFriction` set it up.
struct Friction {
    adherent_slip: f64,
    peak: (f64, f64),
    limit: (f64, f64),
}

impl Friction {
    fn new(d: &WheelDynamics, gee: f64) -> Self {
        let multiplier = (gee * d.gee_bias + 0.4).clamp(0.4, 10.0);
        // TireFriction.Settings.ApplyAPLConstraints
        let peak_x = d.peak_slip_mps.max(0.01);
        let peak = (peak_x, (d.friction_peak * multiplier).max(0.0));
        let limit = (
            d.limit_slip_mps.max(peak_x),
            (d.friction_limit * multiplier).max(0.0),
        );
        let adherent = (d.friction_adherent * multiplier).max(0.0);
        let adherent_slip = if peak.1 > 0.01 {
            (adherent * peak.0 / peak.1).min(peak.0)
        } else {
            peak.0
        };
        Friction {
            adherent_slip,
            peak,
            limit,
        }
    }

    /// `LinealFriction.EvaluateForce`: the force at slip speed `s`, kN.
    fn force(&self, s: f64, load: f64) -> f64 {
        let (px, py) = self.peak;
        let (lx, ly) = self.limit;
        load * if s < px {
            s * py / px
        } else if s < lx {
            py + (ly - py) * (s - px) / (lx - px)
        } else {
            ly
        }
    }

    /// `TireFriction.GetAdherentSlip`: static grip fades out with the contact's speed.
    fn adherent(&self, speed: f64) -> f64 {
        self.adherent_slip * (1.0 - speed / MAX_ADHERENT_SPEED).clamp(0.0, 1.0)
    }
}

fn move_towards(from: f64, to: f64, step: f64) -> f64 {
    if (to - from).abs() <= step {
        to
    } else {
        from + (to - from).signum() * step
    }
}

/// `Module_WheelSteering.UpdateSteering`: the angle, degrees (positive turns the wheel right), that points
/// a wheel at `at` (forward, right of the centre of mass, m) at the turn centre for steering input `steer`
/// (positive right) over a steering range of `range` degrees.
pub fn steer_target_deg(at: [f64; 2], steer: f64, range: f64) -> f64 {
    let angle = range * steer.abs().min(1.0);
    if angle <= 0.0 || steer == 0.0 {
        return 0.0;
    }
    let side = steer.signum();
    // FindTurnCenter: from the point the wheel's distance ahead of (or behind) the centre of mass, along
    // the lateral axis turned by the steering angle, `len / tan(angle)` out
    let (s, c) = angle.to_radians().sin_cos();
    let len = at[0].abs();
    let centre = [len * (1.0 - c), side * len * c * c / s];
    let d = [centre[0] - at[0], centre[1] - at[1]];
    let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if n < 1e-9 {
        return 0.0;
    }
    let off = (d[1].abs() / n).clamp(0.0, 1.0).acos().to_degrees();
    // wheels ahead of the centre of mass turn into the turn, those behind it away
    if at[0] > 0.0 {
        off * side
    } else {
        -off * side
    }
}

/// `Math.Round(x * 100) / 100` (banker's rounding, as .NET's).
fn round_cents(x: f64) -> f64 {
    (x * 100.0).round_ties_even() / 100.0
}

/// One wheel against the ground for a step: VPP `Wheel.ComputeStateUpstream`/`EvaluateTorqueDownstream`
/// under its Euler solver, the contact velocity held for the step.
struct Tyre<'a> {
    friction: &'a Friction,
    load: f64,
    radius: f64,
    inertia: f64,
    /// Contact velocity (sideways, forward), m/s, and the force that would stop it, kN.
    velocity: [f64; 2],
    external: [f64; 2],
}

/// Solve the 3x3 system `m x = b` (Cramer's rule); `None` when it is singular.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut x = [0.0; 3];
    for (k, xk) in x.iter_mut().enumerate() {
        let mut a = m;
        for (row, bi) in a.iter_mut().zip(b) {
            row[k] = bi;
        }
        *xk = det(a) / d;
    }
    Some(x)
}

/// Rounded to single precision, as the original's `float` wheel state.
fn f32r(x: f64) -> f64 {
    x as f32 as f64
}

/// `Mathf.InverseLerp`.
fn inverse_lerp(a: f64, b: f64, v: f64) -> f64 {
    if a != b {
        ((v - a) / (b - a)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// `Mathf.Clamp`: never panics, the lower bound wins when the bounds cross.
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `Mathf.Sign`: zero counts as positive.
fn sign(x: f64) -> f64 {
    if x >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// `Wheel.SolveSymmetricConstraint`.
fn solve_symmetric(sum: f64, t: f64, max: f64) -> f64 {
    if max == 0.0 || ((sum >= 0.0) == (max >= 0.0) && sum.abs() > max.abs()) {
        let k = inverse_lerp(0.0, sum * 0.5, t);
        sum + (max - sum) * k
    } else {
        sum
    }
}

impl Tyre<'_> {
    /// Run the wheel's substeps from spin `spin` (rad/s) with drive and brake torques (kN·m) over `dt`:
    /// the mean force on the craft (sideways, forward), kN, and the spin at the end.
    fn frame(&self, spin: f64, drive: f64, brake: f64, dt: f64) -> ([f64; 2], f64) {
        // VPP keeps the wheel in single precision (`float`), doing only the slips in double: rounding at
        // the same places matters, because a wheel rolling with the ground sits at zero slip, where the
        // adherent tyre's force flips sign
        let h = f32r(dt / SUBSTEPS as f64);
        let (r, inertia) = (f32r(self.radius), f32r(self.inertia));
        let inv_inertia = f32r(1.0 / inertia);
        let [vx, vy] = self.velocity.map(f32r);
        let speed = vx.hypot(vy);
        let adherent_slip = self.friction.adherent(speed);
        let per = if adherent_slip > 0.0 {
            self.friction.force(adherent_slip, self.load) / adherent_slip
        } else {
            0.0
        };
        let mut l = f32r(spin * inertia);
        let mut impulse = [0.0, 0.0];
        for _ in 0..SUBSTEPS {
            let w = f32r(l * inv_inertia);
            // ComputeStateUpstream
            let forward_slip = f32r(w * r - vy);
            let slip = vx.hypot(forward_slip);
            let adherent = slip < adherent_slip;
            let (mut tyre_x, tyre_y, adherent_y) = if adherent {
                // ComputeAdherentForceForward
                let y = if vx.abs() < adherent_slip {
                    (adherent_slip * adherent_slip - vx * vx).sqrt()
                } else {
                    0.0
                } * sign(forward_slip);
                (0.0, -per * y, y)
            } else if slip < 1e-6 {
                (0.0, 0.0, 0.0)
            } else {
                let f = self.friction.force(slip, self.load) / slip;
                (-f * vx, -f * forward_slip, 0.0)
            };
            // EvaluateTorqueDownstream (DirectDrive passes the motor torque straight through)
            let tyre_torque = tyre_y * r;
            let brake_torque = brake * sign(-l);
            let stop = f32r(-l / h);
            let sync = f32r(f32r(f32r(vy / r - w) * inertia) / h);
            let mut sum = drive + tyre_torque + brake_torque;
            if stop.abs() < sync.abs() || (sync.abs() - stop.abs()).abs() < 1e-6 {
                sum = solve_symmetric(sum, brake_torque, stop);
                sum = solve_symmetric(sum, tyre_torque, sync);
            } else {
                sum = solve_symmetric(sum, tyre_torque, sync);
                sum = solve_symmetric(sum, brake_torque, stop);
            }
            let mut pushed = clamp(self.external[1] * r - drive, -brake, brake) + drive;
            let along = if sync <= 0.0 {
                let bound = tyre_torque.max(sync);
                if pushed < 0.0 {
                    let least = tyre_torque * inverse_lerp(adherent_y, 0.0, forward_slip);
                    if pushed < least {
                        pushed = least;
                    }
                    pushed - bound
                } else {
                    clamp(pushed, -bound, -tyre_torque)
                }
            } else {
                let bound = tyre_torque.min(sync);
                if pushed > 0.0 {
                    let most = tyre_torque * inverse_lerp(adherent_y, 0.0, forward_slip);
                    if pushed > most {
                        pushed = most;
                    }
                    pushed - bound
                } else {
                    clamp(pushed, -tyre_torque, -bound)
                }
            };
            let fy = along / r;
            if adherent {
                // ComputeAdherentForceSideways, with the forward slip scaled to the force taken
                let y = if tyre_y != 0.0 {
                    adherent_y * fy / tyre_y
                } else {
                    0.0
                };
                let x = if y.abs() < adherent_slip {
                    (adherent_slip * adherent_slip - y * y).sqrt()
                } else {
                    0.0
                };
                tyre_x = -per * x;
            }
            let fx = clamp(self.external[0], -tyre_x.abs(), tyre_x.abs());
            l = f32r(l + f32r(sum * h));
            impulse[0] += fx * h;
            impulse[1] += fy * h;
        }
        ([impulse[0] / dt, impulse[1] / dt], l / inertia)
    }
}

impl Drive {
    /// One step on the ground for a vessel of `mass_t` under gravity `g` on a body whose surface gravity
    /// is `gee` standard gravities (`gravityASL`, which sets the tyre grip and the brakes): gravity along
    /// the ground `slope_acc` and other pushes `acc` (forward, right), m/s². Updates speed, sideways speed,
    /// yaw rate and heading; returns how far it went (forward, right of its heading at the start of the
    /// step), m.
    pub fn step(
        &mut self,
        mass_t: f64,
        g: f64,
        gee: f64,
        slope_acc: [f64; 2],
        acc: [f64; 2],
        dt: f64,
    ) -> [f64; 2] {
        self.step_turning(mass_t, g, gee, slope_acc, acc, 0.0, dt)
    }

    /// `step` with a torque about the vertical from outside the wheels (the airflow on fins and wings),
    /// kN·m, positive turning right: a rolling plane's tail straightens it into the wind as KSP2's
    /// rigidbody does.
    #[allow(clippy::too_many_arguments)]
    pub fn step_turning(
        &mut self,
        mass_t: f64,
        g: f64,
        gee: f64,
        slope_acc: [f64; 2],
        acc: [f64; 2],
        yaw_torque_knm: f64,
        dt: f64,
    ) -> [f64; 2] {
        if self.wheels.is_empty() || mass_t <= 0.0 || dt <= 0.0 {
            self.speed_mps = 0.0;
            self.lateral_mps = 0.0;
            self.yaw_rate_rad_s = 0.0;
            return [0.0, 0.0];
        }
        let frames = ((dt / WHEEL_DT) - 1e-9).ceil().max(1.0) as usize;
        let h = dt / frames as f64;
        let mut moved = [0.0, 0.0];
        let mut turned = 0.0_f64;
        for _ in 0..frames {
            let (fwd, right) = (self.speed_mps, self.lateral_mps);
            let (s, c) = turned.sin_cos();
            moved[0] += (fwd * c - right * s) * h;
            moved[1] += (fwd * s + right * c) * h;
            self.frame(mass_t, g, gee, slope_acc, acc, yaw_torque_knm, h);
            turned += self.yaw_rate_rad_s * h;
        }
        self.heading_rad = (self.heading_rad + turned).rem_euclid(std::f64::consts::TAU);
        moved
    }

    /// The mass each wheel carries, t: the craft's weight over its wheels with no net moment about the
    /// centre of mass, the least-squares split for three or more wheels, shifted toward the outside of a
    /// turn and the back under acceleration by `cg_height_m` (Σ N·x = -m·a_x·h, Σ N·y = -m·a_y·h). A wheel
    /// that would lift carries nothing.
    fn load_shares(&self, mass_t: f64, g: f64) -> Vec<f64> {
        let n = self.wheels.len();
        let even = vec![mass_t / n.max(1) as f64; n];
        if n < 2 || g <= 0.0 {
            return even;
        }
        let tilt = self.cg_height_m / g;
        let b = [
            mass_t,
            -mass_t * self.acceleration_mps2[0] * tilt,
            -mass_t * self.acceleration_mps2[1] * tilt,
        ];
        // rows: 1, x, y; minimum-norm solution m = Aᵀ (A Aᵀ)⁻¹ b, with a little regularisation for wheels
        // in a line (no lateral or longitudinal moment arm)
        let rows: Vec<[f64; 3]> = self
            .wheels
            .iter()
            .map(|w| [1.0, w.at_m[0], w.at_m[1]])
            .collect();
        let mut m = [[0.0; 3]; 3];
        for row in &rows {
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += row[i] * row[j];
                }
            }
        }
        for (i, row) in m.iter_mut().enumerate() {
            row[i] += 1e-6;
        }
        let Some(lambda) = solve3(m, b) else {
            return even;
        };
        let mut shares: Vec<f64> = rows
            .iter()
            .map(|row| (row[0] * lambda[0] + row[1] * lambda[1] + row[2] * lambda[2]).max(0.0))
            .collect();
        let total: f64 = shares.iter().sum();
        if total <= 0.0 {
            return even;
        }
        for s in &mut shares {
            *s *= mass_t / total;
        }
        shares
    }

    /// The yaw moment of inertia used on the ground, t·m²: the craft's own, or its mass spread over its
    /// wheels when it has none.
    pub fn yaw_inertia(&self, mass_t: f64) -> f64 {
        if self.yaw_inertia_tm2 > 0.0 {
            return self.yaw_inertia_tm2;
        }
        let n = self.wheels.len().max(1) as f64;
        let spread = self
            .wheels
            .iter()
            .map(|w| w.at_m[0] * w.at_m[0] + w.at_m[1] * w.at_m[1])
            .sum::<f64>()
            / n;
        mass_t * spread.max(0.25)
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        &mut self,
        mass_t: f64,
        g: f64,
        gee: f64,
        slope_acc: [f64; 2],
        acc: [f64; 2],
        yaw_torque_knm: f64,
        h: f64,
    ) {
        let shares = self.load_shares(mass_t, g);
        let throttle = self.throttle.clamp(-1.0, 1.0) * self.powered.clamp(0.0, 1.0);
        let (u, w, r) = (self.speed_mps, self.lateral_mps, self.yaw_rate_rad_s);
        let mut force = [0.0, 0.0];
        let mut torque = 0.0;
        let mut held = true;
        for (wheel, &share_t) in self.wheels.iter_mut().zip(&shares) {
            let d = &wheel.dynamics;
            let st = &mut wheel.state;
            let [px, py] = wheel.at_m;
            let radius = wheel.radius_m.max(0.01);
            let inertia = (0.5 * d.mass_t * radius * radius).max(MIN_WHEEL_INERTIA);
            // steering: toward the turn centre, within a range that narrows with the wheel's speed
            let mut delta = 0.0;
            if wheel.steers {
                let range = d
                    .steering_curve
                    .keys
                    .iter()
                    .map(|k| k.v)
                    .fold(f64::MIN, f64::max)
                    .max(0.01);
                let speed = st.velocity_mps[1].abs();
                let (max_angle, response) = if d.steering_curve.keys.is_empty() {
                    (range, d.steering_response * d.steering_response)
                } else {
                    let share = if d.steering_max_angle_curve.keys.is_empty() {
                        1.0
                    } else {
                        d.steering_max_angle_curve.evaluate(speed).clamp(0.0, 1.0)
                    };
                    (
                        round_cents(range * share),
                        d.steering_response * d.steering_curve.evaluate(speed) / range,
                    )
                };
                let input = if max_angle > 0.0 {
                    (steer_target_deg(wheel.at_m, self.steer, range) / max_angle).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                st.steer = move_towards(st.steer, input, response.max(0.0) * h);
                delta = (st.steer * max_angle).to_radians();
            }
            let (sd, cd) = delta.sin_cos();
            let ahead = [cd, sd];
            let side = [-sd, cd];
            // the contact's velocity, craft (forward, right) then wheel (sideways, forward) axes
            let vb = [u - r * py, w + r * px];
            let vx = vb[0] * side[0] + vb[1] * side[1];
            let vy = vb[0] * ahead[0] + vb[1] * ahead[1];
            let speed = (vx * vx + vy * vy).sqrt();
            // motor: the curve at the rim speed, scaled by the slewed input and the charge delivered
            let mut drive_torque = 0.0;
            if let Some(curve) = &wheel.torque_knm {
                let slip = (st.spin_rad_s * radius - vy) * h;
                let input = (throttle * (1.0 - (slip * d.traction_control).abs())).clamp(-1.0, 1.0);
                st.drive = move_towards(st.drive, input, d.drive_response.max(0.0) * h);
                let max = round_cents(curve.evaluate((st.spin_rad_s * radius).abs()).max(0.0));
                drive_torque = st.drive * max;
            }
            // brakes: slewed in, released at once; fully on they hold the wheel
            let mut brake_torque = 0.0;
            if wheel.brake_knm > 0.0 {
                let input = if self.brakes {
                    (1.0 + (gee - 1.0) * st.spin_rad_s.abs().clamp(0.0, 1.0)).min(1.0)
                } else {
                    0.0
                };
                let response = if input != 0.0 {
                    d.brake_response
                } else {
                    100.0
                };
                st.brake = move_towards(st.brake, input, response.max(0.0) * h);
                brake_torque = if st.brake >= 1.0 {
                    st.brake * MAX_BRAKE_TORQUE_REST
                } else {
                    st.brake * wheel.brake_knm
                };
            } else {
                st.brake = 0.0;
            }
            let load = (share_t * g).max(d.min_downforce_kn);
            let friction = Friction::new(d, gee);
            // VehicleBase.CalculateLocalFrame: the impulse that would stop the contact (half of it per step
            // up to the adherent speed), plus the hold against the slope at a crawl
            let ratio = if speed <= MAX_ADHERENT_SPEED {
                ADHERENT_IMPULSE_RATIO
            } else if speed >= MAX_ADHERENT_SPEED * 1.1 {
                1.0
            } else {
                ADHERENT_IMPULSE_RATIO
                    + (1.0 - ADHERENT_IMPULSE_RATIO) * (speed - MAX_ADHERENT_SPEED)
                        / (0.1 * MAX_ADHERENT_SPEED)
            };
            let crawl = ((1.0 - speed * speed) / 0.75).clamp(0.0, 1.0);
            let external = [
                -ratio * share_t * vx / h
                    - (slope_acc[0] * side[0] + slope_acc[1] * side[1]) * share_t * crawl,
                -ratio * share_t * vy / h
                    - (slope_acc[0] * ahead[0] + slope_acc[1] * ahead[1]) * share_t * crawl,
            ];
            let tyre = Tyre {
                friction: &friction,
                load,
                radius,
                inertia,
                velocity: [vx, vy],
                external,
            };
            let (sum, spin) = tyre.frame(st.spin_rad_s, drive_torque, brake_torque, h);
            st.spin_rad_s = spin;
            let locked = brake_torque > 0.0 && spin.abs() < REST_SPEED_MPS / radius;
            held &= locked;
            // VehicleBase.ApplyTireForce smooths what reaches the craft
            let k = (TIRE_FORCE_SHARPNESS * h).clamp(0.0, 1.0);
            for (j, f) in sum.iter().enumerate() {
                st.tire_force_kn[j] += (f - st.tire_force_kn[j]) * k;
            }
            st.velocity_mps = [vx, vy];
            let [tx, ty] = st.tire_force_kn;
            let f = [ahead[0] * ty + side[0] * tx, ahead[1] * ty + side[1] * tx];
            force[0] += f[0];
            force[1] += f[1];
            torque += px * f[1] - py * f[0];
        }
        // the craft: a rigid body on the ground plane, in axes that turn with it
        let ax = force[0] / mass_t + slope_acc[0] + acc[0];
        let ay = force[1] / mass_t + slope_acc[1] + acc[1];
        // what the wheels feel next step: the change of velocity in the turning frame plus the turn's
        // centripetal pull (to the right while turning right)
        self.acceleration_mps2 = [ax - w * r, ay + u * r];
        let nr = r + (torque + yaw_torque_knm) / self.yaw_inertia(mass_t) * h;
        let (s, c) = (r * h).sin_cos();
        let (nu, nw) = (u + ax * h, w + ay * h);
        self.speed_mps = nu * c + nw * s;
        self.lateral_mps = -nu * s + nw * c;
        self.yaw_rate_rad_s = nr;
        if self.brakes
            && held
            && self.speed_mps.hypot(self.lateral_mps) < REST_SPEED_MPS
            && self.yaw_rate_rad_s.abs() < REST_SPEED_MPS
        {
            // held by its brakes: at rest
            self.speed_mps = 0.0;
            self.lateral_mps = 0.0;
            self.yaw_rate_rad_s = 0.0;
            for wheel in &mut self.wheels {
                wheel.state.tire_force_kn = [0.0, 0.0];
                wheel.state.spin_rad_s = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_points_front_and_rear_wheels_at_one_centre() {
        // KSP2's construction: the front wheel at the range itself, the rear one turned the other way
        let front = steer_target_deg([1.0, 0.0], 1.0, 40.0);
        assert!((front - 40.0).abs() < 1e-9, "{front}");
        let rear = steer_target_deg([-1.0, 0.0], 1.0, 40.0);
        assert!(rear < -40.0, "{rear}");
        assert_eq!(steer_target_deg([0.0, 0.7], 1.0, 40.0), 0.0);
        assert_eq!(steer_target_deg([1.0, 0.5], 0.0, 40.0), 0.0);
        let left = steer_target_deg([1.0, 0.5], -1.0, 40.0);
        let right = steer_target_deg([1.0, -0.5], 1.0, 40.0);
        assert!((left + right).abs() < 1e-9, "mirrored: {left} {right}");
    }

    #[test]
    fn rover_tyre_friction_follows_the_linear_curve_scaled_for_kerbin() {
        let d = WheelDynamics {
            friction_adherent: 0.3,
            peak_slip_mps: 3.0,
            friction_peak: 1.5,
            limit_slip_mps: 6.0,
            friction_limit: 1.3,
            ..Default::default()
        };
        let f = Friction::new(&d, 1.0);
        // GeeBias 1.6 at 1 g doubles the coefficients
        assert!((f.force(3.0, 1.0) - 3.0).abs() < 1e-12);
        assert!((f.force(1.5, 1.0) - 1.5).abs() < 1e-12);
        assert!((f.force(10.0, 1.0) - 2.6).abs() < 1e-12);
        assert!((f.adherent_slip - 0.6).abs() < 1e-12);
        assert_eq!(f.adherent(MAX_ADHERENT_SPEED), 0.0);
    }
}

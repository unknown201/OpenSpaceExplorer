//! Port of the installed KSP2 VesselSAS / PIDclamp control update.
//! State and controller units are reference pitch, roll, yaw; physics mirrors Unity Z.
use glam::{DMat3, DQuat, DVec3, EulerRot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Pid {
    integral: f64,
    previous: f64,
}
impl Pid {
    // Keep the reference gains and its two distinct tuning/clamp scalars explicit.
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        error: f64,
        dt: f64,
        kp: f64,
        kd: f64,
        cap: f64,
        tuning: f64,
        clamp: f64,
    ) -> f64 {
        let old = self.integral;
        self.integral += error * dt;
        let derivative = (error - self.previous) / dt;
        self.previous = error;
        let output = (kp * error + kd * derivative) * tuning;
        if output.abs() > cap * clamp {
            self.integral = old;
        }
        output.clamp(-cap * clamp, cap * clamp) / cap
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sas {
    pid: [Pid; 3],
    lock: [f64; 4],
    last: [f64; 4],
    pub initialized: bool,
    #[serde(default)]
    pub acquiring_fixed_target: bool,
    response: [f64; 3],
    decay: [f64; 3],
    decay_locked: bool,
    damping: bool,
    cooldown: i32,
    /// The reference's fully decayed stability lock zeros residual spin.
    pub kill_rotation: bool,
}
impl Default for Sas {
    fn default() -> Self {
        Self {
            pid: [Pid::default(); 3],
            lock: DQuat::IDENTITY.to_array(),
            last: DQuat::IDENTITY.to_array(),
            initialized: false,
            acquiring_fixed_target: false,
            response: [0.; 3],
            decay: [1.; 3],
            decay_locked: false,
            damping: false,
            cooldown: 20,
            kill_rotation: false,
        }
    }
}
pub fn scalar(ratio: f64) -> f64 {
    if ratio <= 0. {
        10.
    } else if ratio <= 0.1 {
        -90. * ratio + 10.
    } else if ratio <= 10. {
        1.
    } else if ratio <= 50. {
        -0.0167 * ratio + 1.1667
    } else {
        0.5
    }
}
pub fn orientation(facing: DVec3, top: DVec3) -> DQuat {
    DQuat::from_mat3(&DMat3::from_cols(facing.cross(top), facing, top)).normalize()
}
impl Sas {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn set_lock(&mut self, q: DQuat) {
        self.lock = q.to_array();
        self.last = self.lock;
        self.pid = [Pid::default(); 3];
        self.initialized = true;
    }
    /// `target=None` selects stability assist; pointing modes supply an inertial direction.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        current: DQuat,
        target: Option<DVec3>,
        omega: DVec3,
        input: [f64; 3],
        acceleration: [f64; 3],
        dt: f64,
        warp: f64,
    ) -> [f64; 3] {
        if !self.initialized {
            self.set_lock(current);
        }
        self.kill_rotation = false;
        let q = (DQuat::from_array(self.lock).inverse() * current).normalize();
        // Unity Z-X-Y Euler decomposition, reflected to the physics rotation convention.
        let (z, x, y) = q.to_euler(EulerRot::ZXY);
        // Unity exposes Euler angles in [0, 360). CheckCoasting runs BEFORE AngularTrim.
        let mut error = [-x, -y, z].map(|v| v.rem_euclid(std::f64::consts::TAU));
        if let Some(direction) = target {
            let v = current.inverse() * direction;
            error[0] = -(std::f64::consts::FRAC_PI_2 - v.y.atan2(-v.z));
            if error[0] < -std::f64::consts::PI {
                error[0] += std::f64::consts::TAU;
            }
            error[2] = v.x.atan2(v.y);
        }
        if target.is_some() || !self.damping {
            for i in [0, 2] {
                let w = [omega.x, omega.y, omega.z][i];
                let stop = dt + (w / acceleration[i].max(0.0001)).abs();
                let arrival = (error[i] / w).abs();
                if error[i].signum() != w.signum() && stop > arrival {
                    self.pid[i] = Pid::default();
                    error[i] = -error[i];
                } else if error[i].signum() != w.signum() && 2. * stop > arrival {
                    self.pid[i] = Pid::default();
                    error[i] = 0.;
                }
            }
        }
        for e in &mut error {
            if *e > std::f64::consts::PI {
                *e -= std::f64::consts::TAU;
            }
        }
        for (i, e) in error.iter().enumerate() {
            if self.response[i].abs() < 0.02 && *e < 0.1 {
                self.decay[i] = (self.decay[i] - 0.005).max(0.1);
            } else {
                self.decay[i] = (self.decay[i] + 0.1).min(1.);
            }
        }
        if self.decay.iter().all(|d| *d <= 0.1) {
            if !self.decay_locked {
                self.decay_locked = true;
                self.lock = current.to_array();
                self.pid = [Pid::default(); 3];
                self.kill_rotation = true;
            }
        } else {
            self.decay_locked = false;
        }
        for i in 0..3 {
            let (kp, kd, cap) = if i == 1 {
                (9000., 600., 2000.)
            } else {
                (15000., 2000., 1500.)
            };
            self.response[i] = self.pid[i].update(
                error[i],
                dt,
                kp,
                kd,
                cap,
                scalar(acceleration[i]).clamp(0.5, 10.) / warp,
                warp / self.decay[i],
            );
        }
        self.damping = false;
        let mut manual = [false; 3];
        for i in 0..3 {
            let u = input[i];
            let r = self.response[i];
            if u != 0. && (u.signum() != r.signum() || u.abs() > r.abs()) {
                self.response[i] = u;
                manual[i] = true;
                self.lock = current.to_array();
                self.pid[i] = Pid::default();
                self.damping = true;
            }
        }
        if manual.iter().any(|v| *v) {
            self.cooldown = 20;
        } else {
            self.cooldown = (self.cooldown - 1).max(0);
        }
        for (i, w) in [omega.x, omega.y, omega.z].iter().enumerate() {
            if w.abs() > 0.1 {
                self.damping = true;
                self.lock = self.last;
                if (target.is_none() || i == 1) && self.cooldown > 0 {
                    self.pid[i] = Pid::default();
                } else if target.is_none() || i == 1 {
                    self.pid = [Pid::default(); 3];
                }
            }
        }
        self.last = current.to_array();
        self.response.map(|v| v.clamp(-1., 1.))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiled_reference_source_traces_match_every_pid_step() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/control-reference.json")).unwrap();
        for trace in data["traces"].as_array().unwrap() {
            let roll = trace["axis"].as_u64().unwrap() == 1;
            let dt = trace["dt"].as_f64().unwrap();
            let mut pid = Pid::default();
            for (i, row) in trace["rows"].as_array().unwrap().iter().enumerate() {
                if i == 72 {
                    pid = Pid::default();
                }
                let v = |n: usize| row[n].as_f64().unwrap();
                let actual = pid.update(
                    v(0),
                    dt,
                    if roll { 9000. } else { 15000. },
                    if roll { 600. } else { 2000. },
                    if roll { 2000. } else { 1500. },
                    v(1),
                    v(2),
                );
                assert!(
                    (actual - v(3)).abs() < 1e-12,
                    "axis={} dt={dt} step={i}: {actual} vs {}",
                    trace["axis"],
                    v(3)
                );
            }
        }
        for row in data["scalars"].as_array().unwrap() {
            assert!((scalar(row[0].as_f64().unwrap()) - row[1].as_f64().unwrap()).abs() < 1e-12);
        }
    }
    #[test]
    fn reference_autotune_breakpoints() {
        for (x, y) in [
            (0., 10.),
            (0.05, 5.5),
            (0.1, 1.),
            (10., 1.),
            (50., 0.3317),
            (51., 0.5),
        ] {
            assert!((scalar(x) - y).abs() < 1e-10);
        }
    }
    #[test]
    fn reference_pid_trace_and_clamp() {
        let mut p = Pid::default();
        for (e, want) in [
            (0.01, 0.7666666666666667),
            (0.01, 0.1),
            (0., -0.6666666666666666),
            (1., 1.),
        ] {
            assert!((p.update(e, 0.02, 15000., 2000., 1500., 1., 1.) - want).abs() < 1e-12);
        }
    }
    #[test]
    fn full_manual_command_and_release() {
        let mut s = Sas::default();
        assert_eq!(
            s.update(
                DQuat::IDENTITY,
                None,
                DVec3::ZERO,
                [1., 0., -1.],
                [1.; 3],
                0.02,
                1.
            ),
            [1., 0., -1.]
        );
        let r = s.update(
            DQuat::IDENTITY,
            None,
            DVec3::ZERO,
            [0.; 3],
            [1.; 3],
            0.02,
            1.,
        );
        assert_eq!(r, [0.; 3]);
    }
}

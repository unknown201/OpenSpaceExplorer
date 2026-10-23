//! Keplerian orbits: elements <-> state vectors, propagation in time.
//!
//! Follows `docs/reference-notes/orbits.md`. Everything is in one right-handed Z-up frame centred on the
//! reference body; angles in the public element struct are degrees (as authored in the data), internal work is
//! radians. Semi-major axis is negative for hyperbolic orbits.

use crate::DVec3;
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

/// Classical elements plus the epoch they are valid at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Elements {
    pub semi_major_axis: f64,
    pub eccentricity: f64,
    pub inclination_deg: f64,
    pub longitude_of_ascending_node_deg: f64,
    pub argument_of_periapsis_deg: f64,
    pub mean_anomaly_at_epoch: f64,
    pub epoch: f64,
}

/// Orthonormal orbital frame: `x` toward periapsis, `y` 90 degrees ahead in the orbit, `z` the orbit normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: DVec3,
    pub y: DVec3,
    pub z: DVec3,
}

impl Frame {
    /// Same construction as the original `CelestialFrame.SetFrame(LAN, inc, argPe)` (radians).
    pub fn from_angles(lan: f64, inc: f64, argpe: f64) -> Frame {
        let (sl, cl) = lan.sin_cos();
        let (si, ci) = inc.sin_cos();
        let (sw, cw) = argpe.sin_cos();
        Frame {
            x: DVec3::new(cl * cw - sl * ci * sw, sl * cw + cl * ci * sw, si * sw),
            y: DVec3::new(-cl * sw - sl * ci * cw, -sl * sw + cl * ci * cw, si * cw),
            z: DVec3::new(sl * si, -cl * si, ci),
        }
    }
}

/// An orbit around a body with gravitational parameter `mu`, with cached derived quantities.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub mu: f64,
    pub elements: Elements,
    pub frame: Frame,
    pub mean_motion: f64,
    /// `+inf` for hyperbolic orbits.
    pub period: f64,
    /// Seconds since periapsis at `elements.epoch`.
    pub obt_at_epoch: f64,
}

const FORWARD: DVec3 = DVec3::new(0.0, 0.0, 1.0);
const RIGHT: DVec3 = DVec3::new(1.0, 0.0, 0.0);

impl Orbit {
    pub fn from_elements(mu: f64, elements: Elements) -> Orbit {
        let frame = Frame::from_angles(
            elements.longitude_of_ascending_node_deg.to_radians(),
            elements.inclination_deg.to_radians(),
            elements.argument_of_periapsis_deg.to_radians(),
        );
        let a = elements.semi_major_axis.abs();
        let mean_motion = (mu / (a * a * a)).sqrt();
        let period = if elements.eccentricity < 1.0 {
            TAU / mean_motion
        } else {
            f64::INFINITY
        };
        Orbit {
            mu,
            elements,
            frame,
            mean_motion,
            period,
            obt_at_epoch: elements.mean_anomaly_at_epoch / mean_motion,
        }
    }

    /// Elements from a position and velocity relative to the body, valid at `ut`.
    pub fn from_state_vectors(mu: f64, r: DVec3, v: DVec3, ut: f64) -> Orbit {
        let h = r.cross(v);
        let rmag = r.length();
        let degenerate = h.length_squared().abs() < f64::EPSILON;
        let mut an = if degenerate {
            r.cross(FORWARD)
        } else {
            FORWARD.cross(h)
        };
        if an.length_squared().abs() < f64::EPSILON {
            an = RIGHT;
        }
        let lan = an.y.atan2(an.x).to_degrees().rem_euclid(360.0);
        let ev = v.cross(h) * (1.0 / mu) - r * (1.0 / rmag);
        let e = ev.length();
        let energy = v.length_squared() / 2.0 - mu / rmag;
        let p = h.length_squared() / mu;
        let a = if e < 1.0 {
            -mu / (2.0 * energy)
        } else {
            -p / (ev.length_squared() - 1.0)
        };

        let (x, mut argpe) = if e.abs() < f64::EPSILON {
            (an.normalized(), 0.0)
        } else {
            let x = ev.normalized();
            let mut w = safe_acos(an.dot(x) / an.length());
            if x.z < 0.0 {
                w = TAU - w;
            }
            (x, w)
        };
        let (y, z) = if degenerate {
            let y = an.normalized();
            (y, x.cross(y))
        } else {
            let z = h.normalized();
            (z.cross(x), z)
        };
        let inc = safe_acos(z.z);
        if argpe.is_nan() {
            argpe = 0.0;
        }
        let frame = Frame { x, y, z };
        let nu = r.dot(y).atan2(r.dot(x));
        let ea = eccentric_from_true(nu, e);
        let m = mean_from_eccentric(ea, e);
        let elements = Elements {
            semi_major_axis: a,
            eccentricity: e,
            inclination_deg: inc.to_degrees(),
            longitude_of_ascending_node_deg: lan,
            argument_of_periapsis_deg: argpe.to_degrees(),
            mean_anomaly_at_epoch: m,
            epoch: ut,
        };
        let mean_motion = (mu / (a.abs().powi(3))).sqrt();
        let period = if e < 1.0 {
            TAU / mean_motion
        } else {
            f64::INFINITY
        };
        Orbit {
            mu,
            elements,
            frame,
            mean_motion,
            period,
            obt_at_epoch: m / mean_motion,
        }
    }

    pub fn eccentricity(&self) -> f64 {
        self.elements.eccentricity
    }
    pub fn semi_major_axis(&self) -> f64 {
        self.elements.semi_major_axis
    }
    pub fn periapsis(&self) -> f64 {
        (1.0 - self.elements.eccentricity) * self.elements.semi_major_axis
    }
    pub fn apoapsis(&self) -> f64 {
        (1.0 + self.elements.eccentricity) * self.elements.semi_major_axis
    }
    pub fn semi_latus_rectum(&self) -> f64 {
        self.elements.semi_major_axis
            * (1.0 - self.elements.eccentricity * self.elements.eccentricity)
    }

    /// Seconds since periapsis at `ut`, in `(-T/2, T/2]` for elliptic orbits.
    pub fn obt_at(&self, ut: f64) -> f64 {
        let el = &self.elements;
        if el.eccentricity < 1.0 {
            if ut.is_infinite() {
                return f64::NAN;
            }
            let mut obt = (ut - el.epoch + self.obt_at_epoch) % self.period;
            if obt > self.period / 2.0 {
                obt -= self.period;
            }
            obt
        } else if ut.is_infinite() {
            ut
        } else {
            self.obt_at_epoch + (ut - el.epoch)
        }
    }

    pub fn mean_anomaly_at(&self, ut: f64) -> f64 {
        self.obt_at(ut) * self.mean_motion
    }

    pub fn eccentric_anomaly_at(&self, ut: f64) -> f64 {
        solve_eccentric_anomaly(self.mean_anomaly_at(ut), self.elements.eccentricity)
    }

    pub fn true_anomaly_at(&self, ut: f64) -> f64 {
        true_from_eccentric(self.eccentric_anomaly_at(ut), self.elements.eccentricity)
    }

    /// Position and velocity relative to the body at true anomaly `nu`.
    pub fn state_at_true_anomaly(&self, nu: f64) -> (DVec3, DVec3) {
        let e = self.elements.eccentricity;
        let (s, c) = nu.sin_cos();
        let p = self.semi_latus_rectum();
        let k = (self.mu / p).sqrt();
        let r = p / (1.0 + e * c);
        let pos = self.frame.x * (c * r) + self.frame.y * (s * r);
        let vel = self.frame.x * (-s * k) + self.frame.y * ((c + e) * k);
        (pos, vel)
    }

    /// Position and velocity relative to the body at universal time `ut`.
    pub fn state_at(&self, ut: f64) -> (DVec3, DVec3) {
        self.state_at_true_anomaly(self.true_anomaly_at(ut))
    }

    pub fn position_at(&self, ut: f64) -> DVec3 {
        self.state_at(ut).0
    }

    pub fn radius_at_true_anomaly(&self, nu: f64) -> f64 {
        self.semi_latus_rectum() / (1.0 + self.elements.eccentricity * nu.cos())
    }

    /// True anomaly (positive branch) at which the radius equals `r`.
    pub fn true_anomaly_at_radius(&self, r: f64) -> f64 {
        let e = self.elements.eccentricity;
        safe_acos((self.semi_latus_rectum() / r - 1.0) / e)
    }

    /// Seconds from `ut` until the orbit next reaches true anomaly `nu` (elliptic: wrapped into `[0, T)`).
    pub fn time_to_true_anomaly(&self, nu: f64, ut: f64) -> f64 {
        let e = self.elements.eccentricity;
        let ea = eccentric_from_true(nu, e);
        let m = mean_from_eccentric(ea, e);
        let obt_target = m / self.mean_motion;
        let dt = obt_target - self.obt_at(ut);
        if e < 1.0 {
            dt.rem_euclid(self.period)
        } else {
            dt
        }
    }

    pub fn time_to_periapsis(&self, ut: f64) -> f64 {
        let obt = self.obt_at(ut);
        if self.elements.eccentricity < 1.0 {
            (self.period - obt) % self.period
        } else {
            -obt
        }
    }
}

fn safe_acos(c: f64) -> f64 {
    c.clamp(-1.0, 1.0).acos()
}

/// Eccentric anomaly from true anomaly (hyperbolic: `H`, infinite past the asymptote).
pub fn eccentric_from_true(nu: f64, e: f64) -> f64 {
    let (s, c) = (nu / 2.0).sin_cos();
    if e < 1.0 {
        2.0 * ((1.0 - e).sqrt() * s).atan2((1.0 + e).sqrt() * c)
    } else {
        let x = ((e - 1.0) / (e + 1.0)).sqrt() * s / c;
        if x >= 1.0 {
            f64::INFINITY
        } else if x <= -1.0 {
            f64::NEG_INFINITY
        } else {
            ((1.0 + x) / (1.0 - x)).ln()
        }
    }
}

/// True anomaly from eccentric anomaly.
pub fn true_from_eccentric(ea: f64, e: f64) -> f64 {
    if e < 1.0 {
        let (s, c) = (ea / 2.0).sin_cos();
        2.0 * ((1.0 + e).sqrt() * s).atan2((1.0 - e).sqrt() * c)
    } else if ea == f64::INFINITY {
        (-1.0 / e).acos()
    } else if ea == f64::NEG_INFINITY {
        -(-1.0 / e).acos()
    } else {
        let s = (ea / 2.0).sinh();
        let c = (ea / 2.0).cosh();
        2.0 * ((e + 1.0).sqrt() * s).atan2((e - 1.0).sqrt() * c)
    }
}

/// Mean anomaly from eccentric anomaly.
pub fn mean_from_eccentric(ea: f64, e: f64) -> f64 {
    if e < 1.0 {
        ea - e * ea.sin()
    } else if ea.is_infinite() {
        ea
    } else {
        e * ea.sinh() - ea
    }
}

/// Eccentric anomaly from mean anomaly, with the original's solver selection and tolerances.
pub fn solve_eccentric_anomaly(m: f64, e: f64) -> f64 {
    if e < 1.0 {
        if e < 0.8 {
            solve_elliptic_newton(m, e, 1e-7)
        } else {
            solve_elliptic_extreme(m, e, 8)
        }
    } else {
        solve_hyperbolic(m, e, 1e-7)
    }
}

fn solve_elliptic_newton(m: f64, e: f64, max_error: f64) -> f64 {
    let mut ea = m + e * m.sin() + 0.5 * e * e * (2.0 * m).sin();
    let mut d: f64 = 1.0;
    let mut guard = 0;
    while d.abs() > max_error && guard < 200 {
        let f = ea - e * ea.sin();
        d = (m - f) / (1.0 - e * ea.cos());
        ea += d;
        guard += 1;
    }
    ea
}

fn solve_elliptic_extreme(m: f64, e: f64, iterations: usize) -> f64 {
    let mut ea = m + 0.85 * e * m.sin().signum();
    for _ in 0..iterations {
        let es = e * ea.sin();
        let ec = e * ea.cos();
        let f = ea - es - m;
        let fp = 1.0 - ec;
        let fpp = es;
        ea += -5.0 * f / (fp + fp.signum() * (16.0 * fp * fp - 20.0 * f * fpp).abs().sqrt());
    }
    ea
}

fn solve_hyperbolic(m: f64, e: f64, max_error: f64) -> f64 {
    if m.is_infinite() {
        return m;
    }
    let x = 2.0 * m / e;
    let mut h = ((x * x + 1.0).sqrt() + x).ln();
    let mut d: f64 = 1.0;
    let mut guard = 0;
    while d.abs() > max_error && guard < 200 {
        d = (e * h.sinh() - h - m) / (e * h.cosh() - 1.0);
        h -= d;
        guard += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const KERBIN_MU: f64 = 3.5316e12;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
    }

    #[test]
    fn circular_equatorial_orbit_speed_and_period() {
        let r = 700_000.0;
        let o = Orbit::from_elements(
            KERBIN_MU,
            Elements {
                semi_major_axis: r,
                eccentricity: 0.0,
                inclination_deg: 0.0,
                longitude_of_ascending_node_deg: 0.0,
                argument_of_periapsis_deg: 0.0,
                mean_anomaly_at_epoch: 0.0,
                epoch: 0.0,
            },
        );
        let (p, v) = o.state_at(0.0);
        assert!(close(p.length(), r, 1e-12));
        assert!(close(v.length(), (KERBIN_MU / r).sqrt(), 1e-12));
        assert!(close(o.period, TAU * (r.powi(3) / KERBIN_MU).sqrt(), 1e-12));
        // a quarter period later we are 90 degrees along, still at radius r
        let (p2, _) = o.state_at(o.period / 4.0);
        assert!(close(p2.length(), r, 1e-9));
        assert!(p.dot(p2).abs() / (r * r) < 1e-9);
    }

    #[test]
    fn elements_round_trip_through_state_vectors() {
        let cases = [
            (0.3, 30.0, 40.0, 50.0, 1.0),
            (0.0, 0.0, 0.0, 0.0, 2.0),
            (0.9, 80.0, 200.0, 300.0, 0.5),
            (0.1, 179.0, 10.0, 350.0, 3.0),
        ];
        for (e, inc, lan, argpe, m0) in cases {
            let el = Elements {
                semi_major_axis: 1.2e6,
                eccentricity: e,
                inclination_deg: inc,
                longitude_of_ascending_node_deg: lan,
                argument_of_periapsis_deg: argpe,
                mean_anomaly_at_epoch: m0,
                epoch: 123.0,
            };
            let o = Orbit::from_elements(KERBIN_MU, el);
            let ut = 5000.0;
            let (p, v) = o.state_at(ut);
            let back = Orbit::from_state_vectors(KERBIN_MU, p, v, ut);
            assert!(
                close(back.elements.semi_major_axis, el.semi_major_axis, 1e-9),
                "a {e} {inc}"
            );
            assert!((back.elements.eccentricity - e).abs() < 1e-9, "e");
            assert!(
                (back.elements.inclination_deg - inc).abs() < 1e-7,
                "inc {}",
                back.elements.inclination_deg
            );
            // frames must agree even when angles are ambiguous (circular / equatorial)
            let (p2, v2) = back.state_at(ut + 777.0);
            let (p1, v1) = o.state_at(ut + 777.0);
            assert!(
                (p2 - p1).length() < 1e-3,
                "pos {e} {inc}: {:?} vs {:?}",
                p1,
                p2
            );
            assert!((v2 - v1).length() < 1e-6, "vel {e} {inc}");
        }
    }

    #[test]
    fn hyperbolic_orbit_round_trips_and_escapes() {
        let r = DVec3::new(700_000.0, 0.0, 0.0);
        let v_esc = (2.0 * KERBIN_MU / r.length()).sqrt();
        let v = DVec3::new(0.0, v_esc * 1.2, 0.0);
        let o = Orbit::from_state_vectors(KERBIN_MU, r, v, 0.0);
        assert!(o.eccentricity() > 1.0);
        assert!(o.semi_major_axis() < 0.0);
        assert!(o.period.is_infinite());
        let (p0, v0) = o.state_at(0.0);
        assert!((p0 - r).length() < 1e-3);
        assert!((v0 - v).length() < 1e-6);
        let (p1, _) = o.state_at(10_000.0);
        assert!(p1.length() > r.length());
        let nu_soi = o.true_anomaly_at_radius(84_159_286.0);
        let t = o.time_to_true_anomaly(nu_soi, 0.0);
        assert!(t > 0.0 && t.is_finite());
        assert!(close(o.position_at(t).length(), 84_159_286.0, 1e-6));
    }

    #[test]
    fn anomaly_solvers_match_keplers_equation() {
        for e in [0.0, 0.3, 0.79, 0.8, 0.95, 0.999] {
            for m in [-3.0, -1.0, 0.0, 0.5, 2.0, 3.1] {
                let ea = solve_eccentric_anomaly(m, e);
                let back = mean_from_eccentric(ea, e);
                assert!((back - m).abs() < 1e-6, "e={e} m={m} got {back}");
            }
        }
        for e in [1.05, 1.5, 3.0] {
            for m in [-5.0, -0.5, 0.0, 0.5, 5.0] {
                let h = solve_eccentric_anomaly(m, e);
                assert!((mean_from_eccentric(h, e) - m).abs() < 1e-6, "e={e} m={m}");
            }
        }
    }

    #[test]
    fn time_to_periapsis_and_true_anomaly_are_consistent() {
        let el = Elements {
            semi_major_axis: 1.0e6,
            eccentricity: 0.4,
            inclination_deg: 10.0,
            longitude_of_ascending_node_deg: 20.0,
            argument_of_periapsis_deg: 30.0,
            mean_anomaly_at_epoch: 1.0,
            epoch: 0.0,
        };
        let o = Orbit::from_elements(KERBIN_MU, el);
        let tpe = o.time_to_periapsis(0.0);
        assert!(close(o.position_at(tpe).length(), o.periapsis(), 1e-9));
        let tap = o.time_to_true_anomaly(PI, 0.0);
        assert!(close(o.position_at(tap).length(), o.apoapsis(), 1e-9));
        assert!(tap < o.period && tpe < o.period);
    }
}

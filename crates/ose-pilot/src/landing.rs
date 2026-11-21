//! A powered landing at a site on an airless body, from orbit: line the orbit's plane up with the
//! site, lower periapsis over it, then brake under closed-loop guidance onto a point just above the
//! site and settle straight down. For delivery routes to colonies (`docs/colonies-design.md`).
//!
//! The braking guidance is the classic fixed-time polynomial (Apollo-style) law: with `t` to go, the
//! acceleration `6 (r_t - r) / t² - 2 (v_t + 2 v) / t` brings the vessel to position `r_t` with velocity
//! `v_t` in time `t` under constant acceleration jerk; gravity is added back for the thrust. It is
//! recomputed every tick, so it corrects for everything the model leaves out (curvature, rotation,
//! staging, the attitude lag). Ignition comes when the cheapest time-to-go needs a set share of the
//! engines' acceleration, so the burn is efficient and leaves margin.
use crate::{acceleration, aim, body, burn::Burn, orbit, stage_if_spent, state, Status};
use bevy_ecs::entity::Entity;
use ose_math::{DVec3, Orbit};
use ose_sim::{
    bodies::Body,
    flight::{self, Attitude},
    Universe,
};

#[derive(Clone, Debug)]
pub struct LandingParams {
    pub site_lat_deg: f64,
    pub site_lon_deg: f64,
    /// Periapsis over the site after the deorbit burn, above the site's ground (m).
    pub periapsis_above_site_m: f64,
    /// Braking ends this high over the site (m), then the vessel settles straight down.
    pub hover_m: f64,
    /// Speed of the final settle (m/s).
    pub settle_mps: f64,
    /// Ignite when the best braking needs this share of the engines' acceleration.
    pub ignite_share: f64,
    /// Give up this long after starting (s).
    pub timeout_s: f64,
}

impl LandingParams {
    pub fn at(lat_deg: f64, lon_deg: f64) -> Self {
        Self {
            site_lat_deg: lat_deg,
            site_lon_deg: lon_deg,
            periapsis_above_site_m: 2_500.0,
            hover_m: 60.0,
            settle_mps: 2.0,
            ignite_share: 0.6,
            timeout_s: 6.0 * 3600.0,
        }
    }
}

enum Phase {
    /// Round the orbit out first at apoapsis, when it is eccentric (a low or uneven capture).
    Circularize(Burn),
    PlanPlane,
    Plane(Burn),
    PlanDeorbit,
    Deorbit(Burn),
    Coast,
    Brake { end_ut: f64 },
    Settle { still_s: f64 },
    Down,
}

pub struct Landing {
    pub params: LandingParams,
    phase: Phase,
    started: Option<f64>,
    rounded: bool,
    /// Where the vessel came to rest (lat, lon) and how far from the site (m).
    pub landed_at: Option<(f64, f64)>,
    pub miss_m: Option<f64>,
    pub log: Vec<String>,
}

/// The site's ground altitude (m above the radius).
fn site_alt(u: &Universe, b: Entity, lat: f64, lon: f64) -> f64 {
    let body = u.world.get::<Body>(b).expect("body");
    let terrain = u.world.get::<flight::Terrain>(b).map(|t| t.0.clone());
    flight::ground_height_at(body, terrain.as_deref(), lat, lon)
}

/// The surface's velocity at inertial position `r` (the body spins about +Z).
fn surface_velocity(body: &Body, r: DVec3) -> DVec3 {
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    DVec3::new(0.0, 0.0, omega).cross(r)
}

/// The fixed-time polynomial guidance: the acceleration (gravity included) that takes (r, v) to
/// (rt, vt) in `t` seconds.
pub fn guidance(r: DVec3, v: DVec3, rt: DVec3, vt: DVec3, t: f64) -> DVec3 {
    (rt - r) * (6.0 / (t * t)) - (vt + v * 2.0) * (2.0 / t)
}

fn gravity(mu: f64, r: DVec3) -> DVec3 {
    let d = r.length().max(1.0);
    r * (-mu / (d * d * d))
}

/// The orbit after an impulse `dv` at `ut`.
fn after(o: &Orbit, ut: f64, dv: DVec3) -> Orbit {
    let (r, v) = o.state_at(ut);
    Orbit::from_state_vectors(o.mu, r, v + dv, ut)
}

/// The retrograde impulse at `ut` that puts periapsis at radius `rp`, by bisection.
fn lower_periapsis(o: &Orbit, ut: f64, rp: f64) -> DVec3 {
    let (_, v) = o.state_at(ut);
    // usually lower it; an orbit already dipping below `rp` (a low capture) is raised instead
    let raise = after(o, ut, v * 0.0).periapsis() < rp;
    let (dir, mut hi) = if raise {
        (v.normalized(), 0.2 * v.length())
    } else {
        (-v.normalized(), v.length())
    };
    let mut lo = 0.0;
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        let p = after(o, ut, dir * mid).periapsis();
        if (p > rp) != raise {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    dir * hi
}

impl Landing {
    pub fn new(params: LandingParams) -> Self {
        Self {
            params,
            phase: Phase::PlanPlane,
            started: None,
            rounded: false,
            landed_at: None,
            miss_m: None,
            log: Vec::new(),
        }
    }

    /// Where the aim point is at `ut`: inertial position and velocity, `up_m` over the site's ground.
    fn site(&self, u: &Universe, b: Entity, up_m: f64, ut: f64) -> (DVec3, DVec3) {
        let (lat, lon) = (self.params.site_lat_deg, self.params.site_lon_deg);
        let alt = site_alt(u, b, lat, lon) + up_m;
        let body = u.world.get::<Body>(b).expect("body");
        flight::surface_state_at(body, lat, lon, alt, ut)
    }

    pub fn update(&mut self, u: &mut Universe, e: Entity) -> Status {
        let Some((b, mu, _)) = body(u, e) else {
            return Status::Failed("no body".into());
        };
        let now = u.ut();
        let started = *self.started.get_or_insert(now);
        if now - started > self.params.timeout_s {
            return Status::Failed("the landing took too long".into());
        }
        if u.powered(e).is_some_and(|p| p.crashed) {
            return Status::Failed("crashed while landing".into());
        }
        let Some(o) = orbit(u, e) else {
            return Status::Failed("no orbit".into());
        };
        match &mut self.phase {
            Phase::Down => Status::Done,
            Phase::Circularize(burn) => match burn.update(u, e) {
                Status::Done => {
                    self.phase = Phase::PlanPlane;
                    Status::Running
                }
                Status::Failed(why) => Status::Failed(format!("circularizing: {why}")),
                Status::Running => Status::Running,
            },
            Phase::PlanPlane => {
                if let Some((_, _, radius)) = body(u, e) {
                    self.log.push(format!(
                        "from a {:.1} x {:.1} km orbit",
                        (o.periapsis() - radius) / 1000.0,
                        (o.apoapsis() - radius) / 1000.0
                    ));
                }
                if !self.rounded && o.eccentricity() > 0.03 && o.period.is_finite() {
                    self.rounded = true;
                    let ta = now + o.time_to_true_anomaly(std::f64::consts::PI, now);
                    let (ra, va) = o.state_at(ta);
                    let v_circ = (mu / ra.length()).sqrt();
                    let dv = va.normalized() * (v_circ - va.length());
                    self.log
                        .push(format!("circularize {:.1} m/s at apoapsis", dv.length()));
                    self.phase = Phase::Circularize(Burn::new(dv, ta));
                    return Status::Running;
                }
                // Turn the orbit's plane so it holds the site as the vessel will pass over it, about a
                // half orbit after the burn.
                let t_b = now + 300.0;
                let (rb, vb) = o.state_at(t_b);
                let half = if o.period.is_finite() {
                    o.period * 0.5
                } else {
                    1800.0
                };
                let (s, _) = self.site(u, b, 0.0, t_b + half);
                let n_now = rb.cross(vb).normalized();
                let mut n = rb.cross(s).normalized();
                if n.dot(n_now) < 0.0 {
                    n = -n;
                }
                let speed_h = (vb - rb.normalized() * vb.dot(rb.normalized())).length();
                let v_new =
                    rb.normalized() * vb.dot(rb.normalized()) + n.cross(rb.normalized()) * speed_h;
                let dv = v_new - vb;
                self.log
                    .push(format!("plane change {:.1} m/s", dv.length()));
                self.phase = if dv.length() < 1.0 {
                    Phase::PlanDeorbit
                } else {
                    Phase::Plane(Burn::new(dv, t_b))
                };
                Status::Running
            }
            Phase::Plane(burn) => match burn.update(u, e) {
                Status::Done => {
                    self.phase = Phase::PlanDeorbit;
                    Status::Running
                }
                Status::Failed(why) => Status::Failed(format!("plane change: {why}")),
                Status::Running => Status::Running,
            },
            Phase::PlanDeorbit => {
                // The burn time whose new periapsis falls closest to the site as it will be then.
                let rp = {
                    let (s, _) = self.site(u, b, 0.0, now);
                    s.length() + self.params.periapsis_above_site_m
                };
                let period = if o.period.is_finite() {
                    o.period
                } else {
                    3600.0
                };
                let mut best: Option<(f64, f64, DVec3)> = None;
                for i in 0..360 {
                    let t = now + 240.0 + period * i as f64 / 360.0;
                    let dv = lower_periapsis(&o, t, rp);
                    let low = after(&o, t, dv);
                    // a burn point below `rp` cannot set it there
                    if (low.periapsis() - rp).abs() > 500.0 {
                        continue;
                    }
                    let tp = t + low.time_to_true_anomaly(0.0, t);
                    let (rp_at, _) = low.state_at(tp);
                    let (s, _) = self.site(u, b, 0.0, tp);
                    let angle = rp_at
                        .normalized()
                        .dot(s.normalized())
                        .clamp(-1.0, 1.0)
                        .acos();
                    if best.as_ref().is_none_or(|x| angle < x.0) {
                        best = Some((angle, t, dv));
                    }
                }
                let Some((angle, t, dv)) = best else {
                    return Status::Failed("no deorbit found".into());
                };
                let (s_now, _) = self.site(u, b, 0.0, now);
                self.log.push(format!(
                    "site {:.0} m up, {:.1} deg off the plane",
                    s_now.length() - body(u, e).map_or(0.0, |x| x.2),
                    (90.0 - s_now.normalized().dot(o.frame.z).clamp(-1.0, 1.0).acos().to_degrees()).abs()
                ));
                self.log.push(format!(
                    "deorbit {:.1} m/s at {:.0} s, periapsis {:.2} deg from the site",
                    dv.length(),
                    t - now,
                    angle.to_degrees()
                ));
                self.phase = Phase::Deorbit(Burn::new(dv, t));
                Status::Running
            }
            Phase::Deorbit(burn) => match burn.update(u, e) {
                Status::Done => {
                    self.phase = Phase::Coast;
                    Status::Running
                }
                Status::Failed(why) => Status::Failed(format!("deorbit: {why}")),
                Status::Running => Status::Running,
            },
            Phase::Coast => {
                let Some((r, v)) = state(u, e) else {
                    return Status::Failed("no state".into());
                };
                let a_max = acceleration(u, e);
                if a_max <= 0.0 && !stage_if_spent(u, e) {
                    return Status::Failed("no engine to land with".into());
                }
                // Ignite when the site is as far ahead as braking at a share of full thrust takes.
                let (site, site_v) = self.site(u, b, self.params.hover_m, now);
                let rel = v - site_v;
                let up = site.normalized();
                let to_site = site - r;
                let horizontal = to_site - up * to_site.dot(up);
                let speed_h = (rel - up * rel.dot(up)).length();
                let closing = rel.dot(horizontal.normalized());
                let decel = self.params.ignite_share * a_max;
                let distance = horizontal.length();
                if closing > 0.0 && distance <= speed_h * speed_h / (2.0 * decel) {
                    let t = (2.0 * distance / speed_h.max(1.0)).max(10.0);
                    self.log.push(format!(
                        "braking {distance:.0} m short of the site at {speed_h:.0} m/s, {t:.0} s to go"
                    ));
                    u.wake(e);
                    self.phase = Phase::Brake { end_ut: now + t };
                }
                Status::Running
            }
            Phase::Brake { end_ut } => {
                let end_ut = *end_ut;
                u.wake(e);
                stage_if_spent(u, e);
                let Some(p) = u.powered(e).cloned() else {
                    return Status::Failed("vessel is gone".into());
                };
                let t = end_ut - now;
                if t < 1.0 || p.landed {
                    self.log.push("settling".into());
                    self.phase = Phase::Settle { still_s: 0.0 };
                    return Status::Running;
                }
                let (rt, vt) = self.site(u, b, self.params.hover_m, end_ut);
                let up = rt.normalized();
                let a =
                    guidance(p.r, p.v, rt, vt - up * self.params.settle_mps, t) - gravity(mu, p.r);
                self.thrust(u, e, &p, a);
                Status::Running
            }
            Phase::Settle { still_s } => {
                u.wake(e);
                stage_if_spent(u, e);
                let Some(p) = u.powered(e).cloned() else {
                    return Status::Failed("vessel is gone".into());
                };
                let body = u.world.get::<Body>(b).expect("body").clone();
                let v_rel = p.v - surface_velocity(&body, p.r);
                let up = p.r.normalized();
                if p.landed {
                    u.set_controls(e, 0.0, Attitude::Up);
                    *still_s += 1.0 / ose_sim::TICKS_PER_SECOND as f64;
                    if *still_s > 3.0 && v_rel.length() < 0.5 {
                        let here = flight::lat_lon(&body, p.r, now);
                        let site = (self.params.site_lat_deg, self.params.site_lon_deg);
                        let miss = crate::descent::arc_deg(here, site).to_radians() * body.radius_m;
                        self.log.push(format!("landed {miss:.0} m from the site"));
                        self.landed_at = Some(here);
                        self.miss_m = Some(miss);
                        self.phase = Phase::Down;
                        return Status::Done;
                    }
                    return Status::Running;
                }
                *still_s = 0.0;
                // hold the descent at the settle speed and kill the drift
                let v_up = v_rel.dot(up);
                let drift = v_rel - up * v_up;
                let g = gravity(mu, p.r).length();
                let a = up * (g + 1.5 * (-self.params.settle_mps - v_up)) - drift * 0.8;
                self.thrust(u, e, &p, a);
                Status::Running
            }
        }
    }

    /// Point the engines along `a` and throttle for its size; hold fire while the craft points
    /// well away from it.
    fn thrust(&self, u: &mut Universe, e: Entity, p: &flight::Powered, a: DVec3) {
        let a_max = acceleration(u, e);
        if a.length() < 1e-6 || a_max <= 0.0 {
            u.set_controls(e, 0.0, Attitude::Up);
            return;
        }
        let dir = a.normalized();
        let hold = Attitude::Fixed(aim(p, p.facing, dir));
        let along = p.facing.dot(dir);
        let throttle = if along > 0.9 {
            (a.length() * along / a_max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        u.set_controls(e, throttle, hold);
    }

    /// Ticks per step a headless harness can run at.
    pub fn suggested_warp(&self, u: &Universe, e: Entity) -> u32 {
        match &self.phase {
            Phase::Plane(b) | Phase::Deorbit(b) | Phase::Circularize(b) => b.suggested_warp(u, e),
            Phase::Coast => {
                let alt = match (state(u, e), body(u, e)) {
                    (Some((r, _)), Some((_, _, radius))) => r.length() - radius,
                    _ => 0.0,
                };
                if alt > 30_000.0 {
                    10
                } else {
                    1
                }
            }
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guidance_does_nothing_on_a_path_that_already_arrives() {
        let (r, v) = (DVec3::new(1000.0, 0.0, 0.0), DVec3::new(-10.0, 5.0, 0.0));
        let t = 30.0;
        let a = guidance(r, v, r + v * t, v, t);
        assert!(a.length() < 1e-9, "{a:?}");
        // and stops a vessel moving at the target: it must turn around
        let a = guidance(r, v, r, DVec3::ZERO, t);
        assert!(a.dot(v) < 0.0);
    }
}

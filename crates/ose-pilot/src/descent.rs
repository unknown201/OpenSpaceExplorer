//! Come home: a deorbit burn timed so the vessel comes down near a site (KSC), then reentry with the
//! controls free, parachutes armed once it has slowed, and rest on the ground or in the water.
//! Rendezvous v2, step R6.
//!
//! The burn is planned on the vacuum orbit: the atmosphere brings the vessel down at roughly a fixed
//! angle along the track from its vacuum periapsis, for a given craft and entry (`past_periapsis_deg`,
//! negative when it lands short). Like a launch profile, it is measured from a real flight.
use crate::{body, burn::Burn, orbit, Status};
use bevy_ecs::entity::Entity;
use ose_math::{DVec3, Orbit};
use ose_sim::{
    bodies::Body,
    flight::{self, Attitude, Situation},
    Universe,
};

#[derive(Clone, Debug)]
pub struct DescentParams {
    pub site_lat_deg: f64,
    pub site_lon_deg: f64,
    /// Periapsis altitude after the deorbit burn (m).
    pub periapsis_alt_m: f64,
    /// Where the vessel comes down relative to its vacuum periapsis, along the track (degrees).
    pub past_periapsis_deg: f64,
    /// Arm the parachutes below this speed relative to the air (m/s).
    pub chute_speed_mps: f64,
    /// Give up this long after the burn (s).
    pub timeout_s: f64,
}

impl DescentParams {
    /// Come down at the KSC launch pad.
    pub fn ksc() -> Self {
        Self {
            site_lat_deg: -0.6836,
            site_lon_deg: -74.8198,
            periapsis_alt_m: 45_000.0,
            past_periapsis_deg: 0.0,
            chute_speed_mps: 250.0,
            timeout_s: 3600.0,
        }
    }
}

enum Phase {
    Plan,
    Burn(Burn),
    Entry { armed: bool, since: f64 },
    Down,
}

pub struct Descent {
    pub params: DescentParams,
    phase: Phase,
    /// Where the plan expected to come down, and where the vessel did (lat, lon).
    pub planned: Option<(f64, f64)>,
    pub landed_at: Option<(f64, f64)>,
    /// The vacuum periapsis after the burn (ut, position, orbit normal), and the measured angle from
    /// it to where the vessel came down.
    reference: Option<(f64, DVec3, DVec3)>,
    flown: Option<f64>,
    still_s: f64,
    last_ut: Option<f64>,
    pub log: Vec<String>,
}

/// Great-circle angle between two (lat, lon) points (degrees).
pub fn arc_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (
        a.0.to_radians(),
        a.1.to_radians(),
        b.0.to_radians(),
        b.1.to_radians(),
    );
    let c = la1.sin() * la2.sin() + la1.cos() * la2.cos() * (lo1 - lo2).cos();
    c.clamp(-1.0, 1.0).acos().to_degrees()
}

/// The retrograde impulse at `ut` that puts periapsis at radius `rp`, by bisection.
fn deorbit_dv(o: &Orbit, ut: f64, rp: f64) -> DVec3 {
    let (r, v) = o.state_at(ut);
    let back = -v.normalized();
    let (mut lo, mut hi) = (0.0, v.length());
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        let after = Orbit::from_state_vectors(o.mu, r, v + back * mid, ut);
        if after.periapsis() > rp {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    back * hi
}

/// The next periapsis: (ut, position). The reference point a descent is aimed by.
fn periapsis(o: &Orbit, now: f64) -> (f64, DVec3) {
    let t = now + o.time_to_true_anomaly(0.0, now);
    (t, o.state_at(t).0)
}

impl Descent {
    pub fn new(params: DescentParams) -> Self {
        Self {
            params,
            phase: Phase::Plan,
            planned: None,
            landed_at: None,
            reference: None,
            flown: None,
            still_s: 0.0,
            last_ut: None,
            log: Vec::new(),
        }
    }

    pub fn update(&mut self, u: &mut Universe, e: Entity) -> Status {
        let Some((b, _, radius)) = body(u, e) else {
            return Status::Failed("no body".into());
        };
        let Some(body) = u.world.get::<Body>(b).cloned() else {
            return Status::Failed("no body".into());
        };
        let site = (self.params.site_lat_deg, self.params.site_lon_deg);
        match &mut self.phase {
            Phase::Down => Status::Done,
            Phase::Plan => {
                let Some(o) = orbit(u, e) else {
                    return Status::Failed("no orbit".into());
                };
                let now = u.ut();
                let period = if o.period.is_finite() {
                    o.period
                } else {
                    return Status::Failed("not in orbit".into());
                };
                let rp = radius + self.params.periapsis_alt_m;
                let mut best: Option<(f64, f64, DVec3, (f64, f64))> = None;
                // Two orbits of burn times, so the site has turned under the track at least once.
                for i in 0..720 {
                    let t = now + 120.0 + 2.0 * period * i as f64 / 720.0;
                    let dv = deorbit_dv(&o, t, rp);
                    let (r, v) = o.state_at(t);
                    let after = Orbit::from_state_vectors(o.mu, r, v + dv, t);
                    let (ti, ri) = periapsis(&after, t);
                    let h = after.frame.z.normalized();
                    let (s, c) = self.params.past_periapsis_deg.to_radians().sin_cos();
                    let rs = ri * c + h.cross(ri) * s + h * (h.dot(ri) * (1.0 - c));
                    let down = flight::lat_lon(&body, rs, ti);
                    let miss = arc_deg(down, site);
                    if best.is_none_or(|b| miss < b.0) {
                        best = Some((miss, t, dv, down));
                    }
                }
                let Some((miss, t, dv, down)) = best else {
                    return Status::Failed("no deorbit reaches the ground".into());
                };
                self.planned = Some(down);
                self.log.push(format!(
                    "DEORBIT {:.1} m/s in {:.0} s, aiming {:.2} deg from the site",
                    dv.length(),
                    t - now,
                    miss
                ));
                self.phase = Phase::Burn(Burn::new(dv, t));
                Status::Running
            }
            Phase::Burn(burn) => match burn.update(u, e) {
                Status::Done => {
                    if let Some(o) = orbit(u, e) {
                        let (t, r) = periapsis(&o, u.ut());
                        self.reference = Some((t, r, o.frame.z.normalized()));
                    }
                    self.phase = Phase::Entry {
                        armed: false,
                        since: u.ut(),
                    };
                    Status::Running
                }
                other => other,
            },
            Phase::Entry { armed, since } => {
                u.wake(e);
                let Some(p) = u.powered(e).cloned() else {
                    return Status::Failed("vessel is gone".into());
                };
                if p.crashed {
                    let alt = p.r.length() - radius;
                    return Status::Failed(format!(
                        "destroyed on the way down at {alt:.0} m, {:.0} m/s",
                        p.v.length()
                    ));
                }
                let situation = u.situation(e);
                let surface_v = p.v
                    - DVec3::new(0.0, 0.0, std::f64::consts::TAU / body.rotation_period_s)
                        .cross(p.r);
                if u.ut() - *since > self.params.timeout_s {
                    return Status::Failed(format!(
                        "still not down: {situation:?} at {:.0} m, {:.1} m/s over the ground",
                        p.r.length() - radius,
                        surface_v.length()
                    ));
                }
                // At rest: on the ground or in the water, or simply not moving over the ground
                // for ten seconds (afloat on a calm sea reads as flying at a couple of metres).
                if surface_v.length() < 0.5 {
                    self.still_s += u.ut() - self.last_ut.unwrap_or(u.ut());
                } else {
                    self.still_s = 0.0;
                }
                self.last_ut = Some(u.ut());
                let resting = matches!(situation, Some(Situation::Landed | Situation::Splashed))
                    && surface_v.length() < 1.0;
                if resting || self.still_s > 10.0 {
                    let at = flight::lat_lon(&body, p.r, u.ut());
                    self.landed_at = Some(at);
                    if let Some((tp, rp, h)) = self.reference {
                        // the landing spot where it stood at periapsis time, measured along the track
                        let turn = -std::f64::consts::TAU / body.rotation_period_s * (u.ut() - tp);
                        let (s, c) = turn.sin_cos();
                        let r = DVec3::new(c * p.r.x - s * p.r.y, s * p.r.x + c * p.r.y, p.r.z);
                        self.flown = Some(rp.cross(r).dot(h).atan2(rp.dot(r)).to_degrees());
                    }
                    self.log.push(format!(
                        "DOWN {:?} at ({:.3}, {:.3}), {:.1} km from the site",
                        situation.unwrap(),
                        at.0,
                        at.1,
                        arc_deg(at, site).to_radians() * radius / 1000.0
                    ));
                    self.phase = Phase::Down;
                    return Status::Done;
                }
                if !*armed
                    && surface_v.length() < self.params.chute_speed_mps
                    && p.r.length() - radius < 40_000.0
                {
                    *armed = true;
                    let n = u.arm_parachutes(e).unwrap_or(0);
                    self.log.push(format!(
                        "CHUTES {n} armed at {:.0} m, {:.0} m/s",
                        p.r.length() - radius,
                        surface_v.length()
                    ));
                }
                // No attitude held: a capsule's mass and heat shield sit at one end, and the air turns
                // it shield first on its own, which an autopilot fighting it would only spoil.
                u.set_controls(e, 0.0, Attitude::Free);
                Status::Running
            }
        }
    }

    /// Where the vessel came down relative to its vacuum periapsis (degrees along the track), to use
    /// as `past_periapsis_deg` for the next descent of the same craft.
    pub fn flown_past_periapsis_deg(&self) -> Option<f64> {
        self.flown
    }

    pub fn suggested_warp(&self, u: &Universe, e: Entity) -> u32 {
        match &self.phase {
            Phase::Burn(b) => b.suggested_warp(u, e),
            Phase::Entry { .. } => {
                let alt = match (crate::state(u, e), body(u, e)) {
                    (Some((r, _)), Some((_, _, radius))) => r.length() - radius,
                    _ => 0.0,
                };
                if alt > 75_000.0 {
                    50
                } else {
                    4
                }
            }
            _ => 1,
        }
    }
}

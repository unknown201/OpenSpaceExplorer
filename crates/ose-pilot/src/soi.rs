//! Moving between spheres of influence: from a planet's orbit to one of its moons, or from a moon's
//! orbit back out to its planet. The transfer burn is found by searching the server's own patched-conic
//! prediction (`ose_sim::trajectory::predict`) for the departure time and delta-v whose arrival
//! periapsis is closest to the goal, so the plan agrees with what the simulation will fly. After the
//! burn the pilot re-predicts from the actual orbit and corrects if needed, and on arrival it brakes
//! into a circular orbit at periapsis. Rendezvous v2, step R4.
use crate::{body, burn::Burn, orbit, Status};
use bevy_ecs::entity::Entity;
use ose_math::{DVec3, Orbit};
use ose_sim::{
    bodies::{Body, BodyIndex, ReferenceBody},
    trajectory::{self, Patch, SolverParams, Transition},
    Universe,
};

/// Where the arrival periapsis may land, either side of the goal, before a correction (m).
const TOLERANCE_M: f64 = 15_000.0;

/// The parent body of `b`, if it has one.
pub fn parent(u: &Universe, b: Entity) -> Option<Entity> {
    u.world.get::<ReferenceBody>(b).map(|r| r.0)
}

/// The arrival found in a prediction: the first patch around `to`, its periapsis altitude, and whether
/// it orbits the same way round as `sense` (the goal's angular momentum direction).
fn arrival(u: &Universe, patches: &[Patch], to: Entity, sense: DVec3) -> Option<(f64, f64, bool)> {
    let radius = u.world.get::<Body>(to)?.radius_m;
    let i = patches.iter().position(|p| p.body == to)?;
    if patches[..i].iter().any(|p| p.end == Transition::Collision) {
        return None;
    }
    let p = &patches[i];
    let alt = p.orbit.periapsis() - radius;
    Some((alt, p.start_ut, p.orbit.frame.z.dot(sense) > 0.0))
}

fn predict(u: &Universe, around: Entity, o: Orbit, ut: f64) -> Vec<Patch> {
    let index = u.world.resource::<BodyIndex>();
    trajectory::predict(&u.world, index, around, o, ut, &SolverParams::default())
}

/// The orbit after an impulse `dv` at `ut`.
fn after(o: Orbit, ut: f64, dv: DVec3) -> Orbit {
    let (r, v) = o.state_at(ut);
    Orbit::from_state_vectors(o.mu, r, v + dv, ut)
}

/// The best impulse within the samples: (score, ut, dv, arrival periapsis altitude).
fn search(
    u: &Universe,
    around: Entity,
    o: Orbit,
    to: Entity,
    goal_alt: f64,
    sense: DVec3,
    candidates: impl Iterator<Item = (f64, DVec3)>,
) -> Option<(f64, f64, DVec3, f64)> {
    let mut best: Option<(f64, f64, DVec3, f64)> = None;
    for (ut, dv) in candidates {
        let patches = predict(u, around, after(o, ut, dv), ut);
        let Some((alt, _, same_way)) = arrival(u, &patches, to, sense) else {
            continue;
        };
        if alt < 5_000.0 {
            continue;
        }
        // Fuel first, then periapsis accuracy; arriving the wrong way round costs a lot to fix.
        let score =
            dv.length() + (alt - goal_alt).abs() / 1_000.0 + if same_way { 0.0 } else { 400.0 };
        if best.is_none_or(|b| score < b.0) {
            best = Some((score, ut, dv, alt));
        }
    }
    best
}

enum Phase {
    Plan,
    Transfer(Burn),
    Coast { checked: bool },
    Capture(Burn),
    Done,
}

pub struct SoiTransfer {
    /// The body to end up orbiting.
    pub to: Entity,
    /// Circular orbit altitude to end in (m), e.g. the target station's.
    pub goal_alt_m: f64,
    /// Angular momentum direction of the goal orbit (the target's), to arrive orbiting the same way.
    pub sense: DVec3,
    phase: Phase,
    pub log: Vec<String>,
}

impl SoiTransfer {
    pub fn new(to: Entity, goal_alt_m: f64, sense: DVec3) -> Self {
        Self {
            to,
            goal_alt_m,
            sense,
            phase: Phase::Plan,
            log: Vec::new(),
        }
    }

    pub fn update(&mut self, u: &mut Universe, e: Entity) -> Status {
        let Some((here, mu, _)) = body(u, e) else {
            return Status::Failed("no body".into());
        };
        let Some(o) = orbit(u, e) else {
            return Status::Failed("no orbit".into());
        };
        match &mut self.phase {
            Phase::Done => Status::Done,
            Phase::Plan => {
                let now = u.ut();
                let period = if o.period.is_finite() {
                    o.period
                } else {
                    3_600.0
                };
                let samples = (0..72).map(|i| now + 300.0 + period * i as f64 / 72.0);
                let to_moon = parent(u, self.to) == Some(here);
                // Prograde burns from a rough Hohmann (out to a moon) or escape (back to the parent)
                // estimate, then a fine pass around the best.
                let (r1, v1) = (o.semi_major_axis(), (mu / o.semi_major_axis()).sqrt());
                let estimate = if to_moon {
                    let r2 = u
                        .world
                        .get::<ose_sim::bodies::OnRails>(self.to)
                        .map(|o| o.0.semi_major_axis())
                        .unwrap_or(r1 * 10.0);
                    v1 * ((2.0 * r2 / (r1 + r2)).sqrt() - 1.0)
                } else {
                    v1 * (2f64.sqrt() - 1.0) * 1.05
                };
                let mut candidates = Vec::new();
                for t in samples {
                    let pro = o.state_at(t).1.normalized();
                    for k in -10..=20 {
                        candidates.push((t, pro * (estimate + k as f64 * 4.0)));
                    }
                }
                let Some((_, t, dv, _)) = search(
                    u,
                    here,
                    o,
                    self.to,
                    self.goal_alt_m,
                    self.sense,
                    candidates.into_iter(),
                ) else {
                    return Status::Failed("no transfer reaches the destination".into());
                };
                let speed = dv.length();
                let fine = (-40..=40).flat_map(|i| {
                    (-8..=8).map(move |k| (t + i as f64 * 5.0, speed + k as f64 * 0.5))
                });
                let fine = fine.map(|(t, s)| (t, o.state_at(t).1.normalized() * s));
                let (_, t, dv, alt) =
                    search(u, here, o, self.to, self.goal_alt_m, self.sense, fine).unwrap_or((
                        0.0,
                        t,
                        dv,
                        f64::NAN,
                    ));
                self.log.push(format!(
                    "SOI PLAN burn {:.1} m/s in {:.0} s, arrival periapsis {:.0} km",
                    dv.length(),
                    t - now,
                    alt / 1000.0
                ));
                self.phase = Phase::Transfer(Burn::new(dv, t));
                Status::Running
            }
            Phase::Transfer(b) => match b.update(u, e) {
                Status::Done => {
                    self.phase = Phase::Coast { checked: false };
                    Status::Running
                }
                other => other,
            },
            Phase::Coast { checked } => {
                if here == self.to {
                    // Arrived: brake into a circular orbit at periapsis.
                    let t = if o.eccentricity() < 1.0 {
                        u.ut() + o.time_to_true_anomaly(0.0, u.ut())
                    } else {
                        // a hyperbola reaches periapsis once: before it if the vessel is inbound
                        let (r, v) = o.state_at(u.ut());
                        if r.dot(v) < 0.0 {
                            u.ut() + o.time_to_periapsis(u.ut())
                        } else {
                            return Status::Failed("passed periapsis before braking".into());
                        }
                    };
                    let (r, v) = o.state_at(t);
                    let circular = r.cross(v).cross(r).normalized() * (mu / r.length()).sqrt();
                    self.log.push(format!(
                        "SOI ARRIVED periapsis {:.0} km, brake {:.1} m/s",
                        (r.length()
                            - u.world
                                .get::<Body>(self.to)
                                .map(|b| b.radius_m)
                                .unwrap_or(0.0))
                            / 1000.0,
                        (circular - v).length()
                    ));
                    self.phase = Phase::Capture(Burn::new(circular - v, t));
                    return Status::Running;
                }
                if !*checked {
                    // One mid-course check from the orbit actually flown.
                    *checked = true;
                    let patches = predict(u, here, o, u.ut());
                    let off = arrival(u, &patches, self.to, self.sense)
                        .map(|(alt, _, _)| (alt - self.goal_alt_m).abs())
                        .unwrap_or(f64::INFINITY);
                    if off > TOLERANCE_M {
                        let t = u.ut() + 600.0;
                        let (r, v) = o.state_at(t);
                        let (pro, rad) = (v.normalized(), r.normalized());
                        let normal = r.cross(v).normalized();
                        let mut candidates = Vec::new();
                        for a in -15..=15 {
                            for b in -15..=15 {
                                for c in [-4, 0, 4] {
                                    candidates.push((
                                        t,
                                        pro * a as f64 + rad * b as f64 + normal * c as f64,
                                    ));
                                }
                            }
                        }
                        if let Some((_, t, dv, alt)) = search(
                            u,
                            here,
                            o,
                            self.to,
                            self.goal_alt_m,
                            self.sense,
                            candidates.into_iter(),
                        ) {
                            self.log.push(format!(
                                "SOI CORRECTION {:.1} m/s, arrival periapsis {:.0} km",
                                dv.length(),
                                alt / 1000.0
                            ));
                            self.phase = Phase::Transfer(Burn::new(dv, t));
                            return Status::Running;
                        }
                        return Status::Failed("lost the encounter and found no correction".into());
                    }
                }
                Status::Running
            }
            Phase::Capture(b) => match b.update(u, e) {
                Status::Done => {
                    self.phase = Phase::Done;
                    Status::Done
                }
                other => other,
            },
        }
    }

    pub fn suggested_warp(&self, u: &Universe, e: Entity) -> u32 {
        match &self.phase {
            Phase::Transfer(b) | Phase::Capture(b) => b.suggested_warp(u, e),
            Phase::Coast { .. } => 1000,
            _ => 1,
        }
    }
}

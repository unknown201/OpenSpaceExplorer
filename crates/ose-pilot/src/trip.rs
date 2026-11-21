//! A flight to a site on another body and a landing there: for delivery routes to colonies
//! (`docs/colonies-design.md`). From the pad it climbs to orbit, moves sphere of influence by sphere of
//! influence to the site's body ([`SoiTransfer`], ending in a low circular orbit there), then lands at the
//! site ([`Landing`]). Started from orbit it skips the ascent, and started around the site's body it goes
//! straight to the landing, so a run interrupted by a restart carries on from where it is.
use crate::{
    ascent::{Ascent, AscentParams},
    body,
    landing::{Landing, LandingParams},
    mission::safe_altitude,
    soi::{parent, SoiTransfer},
    Status,
};
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{
    bodies::{Body, OnRails},
    Universe,
};

/// Height of the parking orbit over the site's body before the landing, above its safe altitude (m).
const PARKING_ABOVE_SAFE_M: f64 = 15_000.0;

#[derive(Clone, Debug)]
pub struct TripParams {
    /// The site's body (`Body::id`).
    pub body: String,
    pub landing: LandingParams,
    /// Climb from the ground first.
    pub ascent: Option<AscentParams>,
}

/// What the trip is doing, for status lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripLeg {
    Ascent,
    SoiTransfer,
    Landing,
    Landed,
    Failed,
}

enum Phase {
    Ascent(Ascent),
    Plan,
    Soi(SoiTransfer),
    Land(Landing),
    Landed,
    Failed(String),
}

pub struct Trip {
    vessel: Entity,
    params: TripParams,
    phase: Phase,
    /// Where it came to rest (lat, lon) and how far from the aim point (m).
    rest: Option<((f64, f64), f64)>,
    pub log: Vec<String>,
}

impl Trip {
    pub fn new(vessel: Entity, params: TripParams) -> Self {
        let phase = match &params.ascent {
            Some(a) => Phase::Ascent(Ascent::new(a.clone())),
            None => Phase::Plan,
        };
        Self {
            vessel,
            params,
            phase,
            rest: None,
            log: Vec::new(),
        }
    }

    pub fn vessel(&self) -> Entity {
        self.vessel
    }

    pub fn leg(&self) -> TripLeg {
        match &self.phase {
            Phase::Ascent(_) => TripLeg::Ascent,
            Phase::Plan | Phase::Soi(_) => TripLeg::SoiTransfer,
            Phase::Land(_) => TripLeg::Landing,
            Phase::Landed => TripLeg::Landed,
            Phase::Failed(_) => TripLeg::Failed,
        }
    }

    /// Where the vessel came to rest and how far from the aim point (m), once landed.
    pub fn landed(&self) -> Option<((f64, f64), f64)> {
        self.rest
    }

    fn fail(&mut self, why: String) -> Status {
        self.log.push(format!("FAILED: {why}"));
        self.phase = Phase::Failed(why.clone());
        Status::Failed(why)
    }

    pub fn update(&mut self, u: &mut Universe) -> Status {
        let e = self.vessel;
        let t = u.ut();
        match &mut self.phase {
            Phase::Landed => Status::Done,
            Phase::Failed(why) => Status::Failed(why.clone()),
            Phase::Ascent(a) => match a.update(u, e) {
                Status::Running => Status::Running,
                Status::Failed(why) => self.fail(why),
                Status::Done => {
                    self.log.push(format!("ORBIT at UT {t:.0}"));
                    self.phase = Phase::Plan;
                    Status::Running
                }
            },
            Phase::Plan => match hop(u, e, &self.params.body) {
                Err(why) => self.fail(why),
                Ok(Some((to, alt, sense))) => {
                    self.log.push(format!("SOI hop at {:.0} km", alt / 1000.0));
                    self.phase = Phase::Soi(SoiTransfer::new(to, alt, sense));
                    Status::Running
                }
                Ok(None) => {
                    self.log.push(format!("LANDING from UT {t:.0}"));
                    self.phase = Phase::Land(Landing::new(self.params.landing.clone()));
                    Status::Running
                }
            },
            Phase::Soi(s) => {
                let status = s.update(u, e);
                self.log.append(&mut s.log);
                match status {
                    Status::Running => Status::Running,
                    Status::Failed(why) => self.fail(why),
                    Status::Done => {
                        self.phase = Phase::Plan;
                        Status::Running
                    }
                }
            }
            Phase::Land(l) => {
                let status = l.update(u, e);
                self.log.append(&mut l.log);
                match status {
                    Status::Running => Status::Running,
                    Status::Failed(why) => self.fail(why),
                    Status::Done => {
                        self.rest = l.landed_at.zip(l.miss_m);
                        self.log.push(format!("LANDED at UT {t:.0}"));
                        self.phase = Phase::Landed;
                        Status::Done
                    }
                }
            }
        }
    }

    /// Ticks per step a headless harness can run at.
    pub fn suggested_warp(&self, u: &Universe) -> u32 {
        let e = self.vessel;
        match &self.phase {
            Phase::Ascent(a) => a.suggested_warp(),
            Phase::Soi(s) => s.suggested_warp(u, e),
            Phase::Land(l) => l.suggested_warp(u, e),
            _ => 1,
        }
    }
}

/// The next sphere-of-influence hop toward the body called `to`: down to a moon of this body, up to its
/// parent, or up first on the way to a sibling. Returns the body, the circular altitude to end at, and
/// the orbit sense (the way that body's moons go round). `None` once around `to`.
fn hop(u: &Universe, e: Entity, to: &str) -> Result<Option<(Entity, f64, DVec3)>, String> {
    let (here, _, _) = body(u, e).ok_or("the vessel is gone")?;
    let there = u
        .body_entity(to)
        .ok_or_else(|| format!("no body called {to}"))?;
    if here == there {
        return Ok(None);
    }
    let sense_of = |b: Entity| {
        u.world
            .get::<OnRails>(b)
            .map(|r| r.0.frame.z)
            .unwrap_or(DVec3::new(0.0, 0.0, 1.0))
    };
    // down: the moon itself, or the moon of this body on the way to it
    let mut down = Some(there);
    while let Some(b) = down {
        if parent(u, b) == Some(here) {
            let alt = safe_altitude(u, b) + PARKING_ABOVE_SAFE_M;
            return Ok(Some((b, alt, sense_of(b))));
        }
        down = parent(u, b);
    }
    // otherwise up to this body's parent
    let up = parent(u, here).ok_or("no route between these bodies")?;
    if u.world.get::<Body>(up).is_none() {
        return Err("no route between these bodies".into());
    }
    Ok(Some((
        up,
        safe_altitude(u, up) + 30_000.0,
        sense_of(here),
    )))
}

//! A whole delivery flight: optional ascent, a fuel-budgeted transfer to the target with course
//! corrections, braking at the encounter, then the RCS approach and docking.
//!
//! Same orbit-to-orbit steps the ISS assembly test pilot used, made tick-driven. When the target
//! orbits another body (a Mun station from LKO, or back), the pilot first moves into the target's
//! sphere of influence with [`SoiTransfer`], one hop at a time, then rendezvouses there.
//! Launch windows are R5 of `docs/rendezvous-v2-plan.md`.
use crate::depart::Depart;
use crate::soi::{parent, SoiTransfer};
use crate::window::LaunchProfile;
use crate::{
    approach::Approach, ascent::Ascent, ascent::AscentParams, body, burn::Burn, orbit, state,
    Status,
};
use bevy_ecs::entity::Entity;
use ose_data::{craft::Craft, PartDef, ResourceDef};
use ose_math::rendezvous;
use ose_sim::{flight::FlightSpec, Universe};

/// The parts and resources the vessels are built from, and how the server builds a flight spec for
/// a craft (needed when undocking splits a vessel).
pub struct Catalog<'a> {
    pub defs: &'a [PartDef],
    pub resources: &'a [ResourceDef],
    pub spec: &'a dyn Fn(&Craft) -> FlightSpec,
}

#[derive(Clone, Debug)]
pub struct MissionParams {
    pub ascent: Option<AscentParams>,
    /// Wait on the ground for a launch window, from this measured ascent profile, before the
    /// ascent. The ascent then flies into the target's plane. Only for a target around the same body.
    pub launch_profile: Option<LaunchProfile>,
    /// Where the target should be at orbit insertion, ahead of the vessel (degrees).
    pub launch_lead_deg: f64,
    /// How far ahead to look for a launch window (s).
    pub launch_horizon_s: f64,
    /// Start docked: undock at this port of ours first, then back off.
    pub depart_from_port: Option<String>,
    pub target: Entity,
    pub target_port: String,
    pub own_port: String,
    /// Transfers must keep periapsis above this altitude (m). `None`: just above the atmosphere, or
    /// 10 km over an airless body.
    pub safe_altitude_m: Option<f64>,
    /// Fraction of the vessel's vacuum delta-v a plan may spend.
    pub budget_fraction: f64,
    /// Lead time between planning and the transfer burn (s).
    pub plan_lead_s: f64,
    /// Accept an encounter that passes within this distance (m); otherwise correct.
    pub accept_miss_m: f64,
    /// Hand over to the approach once braking leaves the target this close (m).
    pub approach_range_m: f64,
    pub max_corrections: u32,
}

impl MissionParams {
    pub fn new(target: Entity, target_port: &str, own_port: &str) -> Self {
        Self {
            ascent: None,
            launch_profile: None,
            launch_lead_deg: 2.0,
            launch_horizon_s: 2.0 * 86_400.0,
            depart_from_port: None,
            target,
            target_port: target_port.into(),
            own_port: own_port.into(),
            safe_altitude_m: None,
            budget_fraction: 0.9,
            plan_lead_s: 360.0,
            accept_miss_m: 1_500.0,
            approach_range_m: 8_000.0,
            max_corrections: 12,
        }
    }
}

/// Where the mission is, for logs and the routes panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    Depart,
    Wait,
    Ascent,
    Plan,
    SoiTransfer,
    Transfer,
    Brake,
    Approach,
    Docked,
    Failed,
}

enum Phase {
    Depart(Depart),
    /// On the ground until the launch window (computed on the first tick).
    Wait(Option<f64>),
    Ascent(Ascent),
    Plan,
    Soi(SoiTransfer),
    Transfer(Burn, f64),
    Brake(Burn),
    Approach(Approach),
    Docked,
    Failed(String),
}

pub struct Mission {
    pub params: MissionParams,
    /// The vessel being flown. Undocking can make it a new entity.
    vessel: Entity,
    phase: Phase,
    encounter: Option<f64>,
    corrections: u32,
    flown_profile: Option<LaunchProfile>,
    /// One line per notable event, for logs and tests.
    pub log: Vec<String>,
}

impl Mission {
    pub fn new(vessel: Entity, params: MissionParams) -> Self {
        let phase = match (&params.depart_from_port, &params.ascent) {
            (Some(port), _) => Phase::Depart(Depart::new(port)),
            (None, Some(_)) if params.launch_profile.is_some() => Phase::Wait(None),
            (None, Some(a)) => Phase::Ascent(Ascent::new(a.clone())),
            (None, None) => Phase::Plan,
        };
        Self {
            params,
            vessel,
            phase,
            encounter: None,
            corrections: 0,
            flown_profile: None,
            log: Vec::new(),
        }
    }

    pub fn vessel(&self) -> Entity {
        self.vessel
    }

    /// How this mission's ascent flew, once it reached orbit.
    pub fn flown_profile(&self) -> Option<LaunchProfile> {
        self.flown_profile
    }

    pub fn leg(&self) -> Leg {
        match self.phase {
            Phase::Depart(_) => Leg::Depart,
            Phase::Wait(_) => Leg::Wait,
            Phase::Ascent(_) => Leg::Ascent,
            Phase::Plan => Leg::Plan,
            Phase::Soi(_) => Leg::SoiTransfer,
            Phase::Transfer(..) => Leg::Transfer,
            Phase::Brake(_) => Leg::Brake,
            Phase::Approach(_) => Leg::Approach,
            Phase::Docked => Leg::Docked,
            Phase::Failed(_) => Leg::Failed,
        }
    }

    fn fail(&mut self, why: String) -> Status {
        self.log.push(format!("FAILED {why}"));
        self.phase = Phase::Failed(why.clone());
        Status::Failed(why)
    }

    pub fn update(&mut self, u: &mut Universe, catalog: &Catalog) -> Status {
        let t = u.ut();
        let e = self.vessel;
        match &mut self.phase {
            Phase::Docked => Status::Done,
            Phase::Failed(why) => Status::Failed(why.clone()),
            Phase::Depart(d) => match d.update(u, e, catalog) {
                Status::Running => Status::Running,
                Status::Failed(why) => self.fail(why),
                Status::Done => {
                    if let Some(v) = d.vessel {
                        self.vessel = v;
                    }
                    self.log.push(format!("DEPARTED at UT {t:.0}"));
                    self.phase = match &self.params.ascent {
                        Some(a) => Phase::Ascent(Ascent::new(a.clone())),
                        None => Phase::Plan,
                    };
                    Status::Running
                }
            },
            Phase::Wait(until) => {
                let until = match until {
                    Some(t) => *t,
                    None => {
                        let profile = self.params.launch_profile.expect("set for a wait");
                        let Some(w) = crate::window::for_vessel(
                            u,
                            e,
                            self.params.target,
                            profile,
                            self.params.launch_lead_deg,
                            self.params.launch_horizon_s,
                        ) else {
                            return self.fail("no launch window".into());
                        };
                        self.log.push(format!(
                            "WINDOW lift off in {:.0} s, {:.2} deg out of plane, phase off by {:.1} deg",
                            w.ut - t,
                            w.plane_deg,
                            w.phase_error_deg
                        ));
                        *until = Some(w.ut);
                        w.ut
                    }
                };
                if t + 1e-6 >= until {
                    let mut a = self.params.ascent.clone().expect("set for a wait");
                    if a.plane.is_none() {
                        a.plane = orbit(u, self.params.target).map(|o| o.frame.z);
                    }
                    self.phase = Phase::Ascent(Ascent::new(a));
                }
                Status::Running
            }
            Phase::Ascent(a) => match a.update(u, e) {
                Status::Running => Status::Running,
                Status::Failed(why) => self.fail(why),
                Status::Done => {
                    self.flown_profile = a.profile();
                    if let (Some(o), Some((_, _, radius))) = (orbit(u, e), body(u, e)) {
                        self.log.push(format!(
                            "ORBIT {:.0} x {:.0} km at UT {t:.0}, {:?}",
                            (o.periapsis() - radius) / 1000.0,
                            (o.apoapsis() - radius) / 1000.0,
                            self.flown_profile
                        ));
                    }
                    self.phase = Phase::Plan;
                    Status::Running
                }
            },
            Phase::Plan => self.plan(u, e, catalog),
            Phase::Soi(s) => {
                let status = s.update(u, e);
                self.log.append(&mut s.log);
                match status {
                    Status::Running => Status::Running,
                    Status::Failed(why) => self.fail(why),
                    Status::Done => {
                        if let (Some(o), Some((_, _, radius))) = (orbit(u, e), body(u, e)) {
                            self.log.push(format!(
                                "CAPTURED {:.0} x {:.0} km at UT {t:.0}",
                                (o.periapsis() - radius) / 1000.0,
                                (o.apoapsis() - radius) / 1000.0
                            ));
                        }
                        self.encounter = None;
                        self.phase = Phase::Plan;
                        Status::Running
                    }
                }
            }
            Phase::Transfer(burn, arrival) => {
                let arrival = *arrival;
                match burn.update(u, e) {
                    Status::Running => Status::Running,
                    Status::Failed(why) => self.fail(why),
                    Status::Done => {
                        let (Some(own), Some(target)) = (orbit(u, e), orbit(u, self.params.target))
                        else {
                            return self.fail("lost the target".into());
                        };
                        let miss = (own.state_at(arrival).0 - target.state_at(arrival).0).length();
                        self.log
                            .push(format!("TRANSFER done, predicted pass {miss:.0} m"));
                        if miss > self.params.accept_miss_m
                            && self.corrections + 1 < self.params.max_corrections
                        {
                            self.phase = Phase::Plan;
                            return Status::Running;
                        }
                        let dv = target.state_at(arrival).1 - own.state_at(arrival).1;
                        // Brake a little early so the vessel stops short of the target, not past it.
                        self.phase = Phase::Brake(Burn::new(dv, arrival - 10.0));
                        Status::Running
                    }
                }
            }
            Phase::Brake(burn) => match burn.update(u, e) {
                Status::Running => Status::Running,
                Status::Failed(why) => self.fail(why),
                Status::Done => {
                    let range = match (state(u, e), state(u, self.params.target)) {
                        (Some(a), Some(b)) => (a.0 - b.0).length(),
                        _ => return self.fail("lost the target".into()),
                    };
                    self.log.push(format!("BRAKED at {range:.0} m"));
                    if range < self.params.approach_range_m {
                        self.phase = Phase::Approach(Approach::new(
                            self.params.target,
                            &self.params.target_port,
                            &self.params.own_port,
                        ));
                    } else {
                        if self.corrections + 1 >= self.params.max_corrections {
                            return self.fail(format!("still {range:.0} m away after braking"));
                        }
                        self.encounter = None;
                        self.phase = Phase::Plan;
                    }
                    Status::Running
                }
            },
            Phase::Approach(a) => match a.update(u, e, catalog.defs) {
                Status::Running => Status::Running,
                Status::Failed(why) => self.fail(why),
                Status::Done => {
                    if let Some(kept) = a.docked(u, e) {
                        self.vessel = kept;
                    }
                    self.log.push(format!("DOCKED at UT {t:.0}"));
                    self.phase = Phase::Docked;
                    Status::Done
                }
            },
        }
    }

    /// The next sphere-of-influence hop toward the target's body, if it orbits another one: down to
    /// a moon of this body, up to this body's parent, or up first on the way to a sibling moon.
    /// Returns the body to go to, the circular altitude to end at, and the orbit sense to match.
    fn hop(
        &self,
        u: &Universe,
        e: Entity,
    ) -> Result<Option<(Entity, f64, ose_math::DVec3)>, String> {
        let (Some((here, _, _)), Some((there, _, there_radius))) =
            (body(u, e), body(u, self.params.target))
        else {
            return Err("lost the target".into());
        };
        if here == there {
            return Ok(None);
        }
        let target = orbit(u, self.params.target).ok_or("lost the target")?;
        if parent(u, there) == Some(here) || parent(u, here) == Some(there) {
            return Ok(Some((
                there,
                target.semi_major_axis() - there_radius,
                target.frame.z,
            )));
        }
        let Some(up) = parent(u, here) else {
            return Err("no route between these bodies".into());
        };
        // Park in a low orbit of the parent, the way the planet's other moons orbit.
        let sense = u
            .world
            .get::<ose_sim::bodies::OnRails>(here)
            .map(|r| r.0.frame.z)
            .unwrap_or(target.frame.z);
        Ok(Some((up, safe_altitude(u, up) + 30_000.0, sense)))
    }

    fn plan(&mut self, u: &mut Universe, e: Entity, catalog: &Catalog) -> Status {
        match self.hop(u, e) {
            Err(why) => return self.fail(why),
            Ok(Some((to, alt, sense))) => {
                let name = |b| {
                    u.world
                        .get::<ose_sim::bodies::Body>(b)
                        .map(|b| b.id.clone())
                        .unwrap_or_default()
                };
                self.log
                    .push(format!("SOI to {} at {:.0} km", name(to), alt / 1000.0));
                self.phase = Phase::Soi(SoiTransfer::new(to, alt, sense));
                return Status::Running;
            }
            Ok(None) => {}
        }
        self.corrections += 1;
        let (Some(own), Some(target), Some((_, _, radius))) =
            (orbit(u, e), orbit(u, self.params.target), body(u, e))
        else {
            return self.fail("lost the target".into());
        };
        let Some(craft) = u.current_craft(e) else {
            return self.fail("vessel is gone".into());
        };
        // Already close and nearly matched (say, parked next to the station): go straight in.
        if let (Some(a), Some(b)) = (state(u, e), state(u, self.params.target)) {
            if (a.0 - b.0).length() < self.params.approach_range_m && (a.1 - b.1).length() < 5.0 {
                self.log.push(format!(
                    "CLOSE at {:.0} m, approaching",
                    (a.0 - b.0).length()
                ));
                self.phase = Phase::Approach(Approach::new(
                    self.params.target,
                    &self.params.target_port,
                    &self.params.own_port,
                ));
                return Status::Running;
            }
        }
        let budget = ose_data::staging::delta_v_by_stage(&craft, catalog.defs, catalog.resources)
            .total_vacuum_mps
            * self.params.budget_fraction;
        let depart = u.ut() + self.params.plan_lead_s;
        let floor = radius
            + self
                .params
                .safe_altitude_m
                .unwrap_or_else(|| safe_altitude(u, body(u, e).unwrap().0));
        let plan = self
            .encounter
            .and_then(|arrival| rendezvous::correction(own, target, depart, arrival, floor, budget))
            .or_else(|| rendezvous::plan(own, target, depart, floor, budget));
        let Some(plan) = plan else {
            return self.fail(format!(
                "no transfer within {budget:.0} m/s: own orbit {:.0} x {:.0} km, target {:.0} x {:.0} km, {:.2} deg apart",
                (own.periapsis() - radius) / 1000.0,
                (own.apoapsis() - radius) / 1000.0,
                (target.periapsis() - radius) / 1000.0,
                (target.apoapsis() - radius) / 1000.0,
                own.frame.z.dot(target.frame.z).clamp(-1.0, 1.0).acos().to_degrees()
            ));
        };
        self.encounter = Some(plan.arrival_ut);
        self.log.push(format!(
            "PLAN {}: depart in {:.0} s, {:.1} m/s + braking {:.1} m/s",
            self.corrections,
            plan.departure_ut - u.ut(),
            plan.delta_v.length(),
            plan.arrival_delta_v.length()
        ));
        self.phase = Phase::Transfer(Burn::new(plan.delta_v, plan.departure_ut), plan.arrival_ut);
        Status::Running
    }

    /// Ticks per step a headless harness can run at.
    pub fn suggested_warp(&self, u: &Universe) -> u32 {
        let e = self.vessel;
        match &self.phase {
            Phase::Depart(d) => d.suggested_warp(),
            Phase::Wait(Some(until)) => {
                // ticks are 1/50 s; land on the window, not past it
                (((until - u.ut()) * 50.0 / 2.0) as u32).clamp(1, 1000)
            }
            Phase::Ascent(a) => a.suggested_warp(),
            Phase::Transfer(b, _) | Phase::Brake(b) => b.suggested_warp(u, e),
            Phase::Approach(a) => a.suggested_warp(),
            Phase::Soi(s) => s.suggested_warp(u, e),
            _ => 1,
        }
    }
}

/// The lowest altitude a transfer may pass at: just above the atmosphere, or 10 km over an airless
/// body (clear of the Mun's highest peaks).
pub fn safe_altitude(u: &Universe, b: Entity) -> f64 {
    u.atmosphere(b)
        .map(|a| a.depth_m + 1_000.0)
        .unwrap_or(10_000.0)
}

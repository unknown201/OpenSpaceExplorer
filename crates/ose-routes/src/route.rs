//! Routes and their runs (design 3.3, 3.4 and 3.10). A route is made from a finished proving flight. At each
//! launch window the scheduler asks the server for a fresh copy of the craft on the pad, and the route
//! pilot (`ose_pilot::mission`) flies it to the depot like any vessel: it is simulated, drawn and can hit
//! things. Docked, it pumps what the proving flight delivered.
//!
//! A route may end at a colony instead (`Endpoint::Colony`, `docs/colonies-design.md`): its proving flight
//! ended landed there, and each run flies from KSC to the colony's body and lands within it
//! (`ose_pilot::trip`), where it stays and what it carries becomes the colony's stock.
//!
//! Routes run one way from KSC. After delivering, a run stays docked, rotates out when the next one reaches
//! orbit, flies home or is let go (`WhenDelivered`, design 3.10); the route waits while the recorded port is
//! taken ("no launch when all ports are full"). Depot-to-depot shuttles come next.
//!
//! The route itself is saved with the campaign. Its pilot lives only in memory (`Pilots`): after a restart
//! or a reload the pilots are made again from the saved routes (`Routes::resume`). A run in flight is found
//! by its vessel's name and flies on from where it is: still climbing, it ascends again; in orbit, it
//! plans its rendezvous from there; docked, it pumps what is left of its cargo. A run leaving the depot
//! carries on backing off or flying home. A run whose vessel is gone ends as Missed.
use crate::book::{Depot, Routes};
use crate::recording::{Event, State};
use bevy_ecs::entity::Entity;
use ose_data::craft::Craft;
use ose_math::DVec3;
use ose_pilot::{
    ascent::AscentParams,
    depart::Depart,
    descent::{Descent, DescentParams},
    landing::LandingParams,
    mission::{Catalog, Leg, Mission, MissionParams},
    trip::{Trip, TripLeg, TripParams},
    window::{self, LaunchProfile},
    Status,
};
use ose_sim::{bodies::Body, Universe};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A run that has not docked after this long has missed (s).
pub const RUN_TIME_LIMIT_S: f64 = 3.0 * 86_400.0;
/// The earliest launch after a route is free to launch (s): time for the pad crew to roll it out.
pub const ROLLOUT_S: f64 = 30.0;
/// Pumping one resource stops when nothing has moved for this long (s).
pub const PUMP_STALL_S: f64 = 10.0;
/// How far ahead the scheduler looks for a launch window (s).
pub const WINDOW_HORIZON_S: f64 = 2.0 * 86_400.0;

/// What to do with the run a route left docked at the depot (design 3.10, "cleaning up by hand").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StayedCommand {
    /// Undock it and fly it home.
    SendHome,
    /// Undock it and leave it drifting.
    LetGo,
    /// Keep it as part of the depot: the route no longer counts it.
    Keep,
}

/// A run aims this far from the colony's site, a little further round for each run, so the runs do not
/// land on each other (m). Well inside the colony's radius (`ose_colonies::RADIUS_M`).
pub const COLONY_AIM_M: f64 = 120.0;

/// A colony as a route's endpoint: the site the runs land at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColonySite {
    pub id: u64,
    pub name: String,
    pub body: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
}

/// Where a route's runs go. Routes saved before colonies have no endpoint and load as depot routes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Endpoint {
    /// Dock at a depot: the depot and its port are `Route::depot`, `depot_port` and `own_port`.
    #[default]
    Depot,
    /// Land within a colony and stay as its stock.
    Colony(ColonySite),
}

/// What a run does once it has pumped its cargo (design 3.10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhenDelivered {
    /// Stay docked for good: each run takes a port.
    Stay,
    /// Stay docked until the next run is in orbit, then undock and fly home, freeing the port for it.
    #[default]
    Rotate,
    /// Undock and fly home: deorbit for KSC, and the vessel is recovered where it comes to rest.
    Return,
    /// Undock, back off, and leave it there.
    LetGo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    /// Everything recorded was pumped across.
    Delivered,
    /// It docked but delivered less: it burned more on the way, or the depot was full.
    Short,
    /// It could not dock, and was left where it is.
    Missed(String),
    /// It was destroyed.
    Lost(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunLog {
    pub n: u32,
    pub vessel_name: String,
    pub launched_ut: f64,
    pub ended_ut: f64,
    pub outcome: Outcome,
    pub delivered: BTreeMap<String, f64>,
    /// After delivering: "docked", "let go", "flying home", "recovered" or what went wrong.
    #[serde(default)]
    pub after: Option<String>,
    /// Leaving the depot now (undocking, backing off or flying home), and whether it flies home.
    #[serde(default)]
    pub leaving: Option<bool>,
    /// The players who took control of it on the way (design 3.11).
    #[serde(default)]
    pub flown_by: Vec<u64>,
}

/// The run in flight now, as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveRun {
    pub n: u32,
    pub vessel_name: String,
    pub launched_ut: f64,
    /// Pumped into the depot so far, by resource.
    #[serde(default)]
    pub delivered: BTreeMap<String, f64>,
    /// The player flying it by hand now, if one is (design 3.11): the route pilot stands aside.
    #[serde(default)]
    pub manual: Option<u64>,
    /// Everyone who took control of it on the way.
    #[serde(default)]
    pub flown_by: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub id: u64,
    pub name: String,
    /// Who created it: its runs are theirs.
    pub owner: u64,
    /// The proving flight it was made from.
    pub proving: u64,
    pub craft: Craft,
    /// The launch site, as the proving flight stood on it: body, inertial position and when.
    pub site_body: String,
    pub site_r: DVec3,
    pub site_ut: f64,
    /// `Depot::id`, its port (named as when the depot was marked), and ours (as in `craft`).
    pub depot: u64,
    pub depot_port: String,
    pub own_port: String,
    /// What each run pumps into the depot: what the proving flight delivered.
    pub cargo: BTreeMap<String, f64>,
    /// The proving flight's ascent (lift-off to orbit, s, and downrange angle, degrees).
    pub ascent: Option<(f64, f64)>,
    /// Paused by a player or after a Lost run, with why.
    pub paused: Option<String>,
    /// What it is doing or waiting for, for the routes panel.
    pub status: String,
    pub next_launch: Option<f64>,
    pub active: Option<ActiveRun>,
    pub runs: Vec<RunLog>,
    #[serde(default)]
    pub when_delivered: WhenDelivered,
    /// The run left docked at the depot port (Stay and Rotate).
    #[serde(default)]
    pub stayed: Option<u32>,
    /// Where the runs go: a depot, or a colony.
    #[serde(default)]
    pub endpoint: Endpoint,
}

impl Route {
    /// The colony the runs land at, if the route ends at one.
    pub fn colony(&self) -> Option<&ColonySite> {
        match &self.endpoint {
            Endpoint::Colony(c) => Some(c),
            Endpoint::Depot => None,
        }
    }
}

/// The server should launch this route's craft on the pad now.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchRequest {
    pub route: u64,
    pub owner: u64,
    pub craft: Craft,
    pub vessel_name: String,
}

enum Stage {
    Flying(Box<Mission>),
    /// To a colony, and the landing there.
    Trip(Box<Trip>),
    Pumping {
        /// What is left to pump, and the one going now with its depot total at the start, the target total,
        /// and the last amount seen moving and when.
        queue: Vec<(String, f64)>,
        current: Option<(String, f64, f64, f64, f64)>,
        delivered: BTreeMap<String, f64>,
    },
}

/// A delivered run leaving the depot.
enum Leave {
    Undock(Box<Depart>, Entity),
    Descend(Box<Descent>, Entity),
}

struct Homebound {
    route: u64,
    n: u32,
    /// Fly home after backing off (otherwise let it go).
    home: bool,
    leave: Leave,
}

/// The route pilots, by route. Not saved.
#[derive(Default)]
pub struct Pilots {
    runs: BTreeMap<u64, Stage>,
    homebound: Vec<Homebound>,
    /// The pilots were made again from the saved routes (`Routes::resume`).
    resumed: bool,
    /// The vessels players fly by hand now, and who flies each (`set_manual`).
    manual: BTreeMap<Entity, u64>,
}

impl Pilots {
    /// The leg the route's run is flying, if it is flying.
    pub fn leg(&self, route: u64) -> Option<Leg> {
        match self.runs.get(&route)? {
            Stage::Flying(m) => Some(m.leg()),
            Stage::Trip(t) => Some(trip_leg(t)),
            Stage::Pumping { .. } => None,
        }
    }

    /// The vessel a route's run is, while flying.
    pub fn vessel(&self, route: u64) -> Option<Entity> {
        match self.runs.get(&route)? {
            Stage::Flying(m) => Some(m.vessel()),
            Stage::Trip(t) => Some(t.vessel()),
            Stage::Pumping { .. } => None,
        }
    }

    /// The fastest warp every run in flight allows: 1 while one docks or pumps, none without runs.
    pub fn suggested_warp(&self, u: &Universe) -> Option<u32> {
        self.runs
            .values()
            .filter_map(|s| match s {
                // a run flown by hand is the player's to warp
                Stage::Flying(m) if self.manual.contains_key(&m.vessel()) => None,
                Stage::Flying(m) => Some(m.suggested_warp(u)),
                Stage::Trip(t) if self.manual.contains_key(&t.vessel()) => None,
                Stage::Trip(t) => Some(t.suggested_warp(u)),
                Stage::Pumping { .. } => Some(1),
            })
            .chain(self.homebound.iter().map(|h| match &h.leave {
                Leave::Undock(d, _) => d.suggested_warp(),
                Leave::Descend(d, e) => d.suggested_warp(u, *e),
            }))
            .min()
    }

    /// The vessels players are flying now, each with its player (design 3.11). A route run among them is
    /// theirs: its pilot stands aside until they let go of it, then picks it up from where it is.
    pub fn set_manual(&mut self, flown: BTreeMap<Entity, u64>) {
        self.manual = flown;
    }

    /// Forget every pilot (the universe was replaced).
    pub fn clear(&mut self) {
        self.runs.clear();
        self.homebound.clear();
        self.resumed = false;
    }
}

/// A route's runs changed (a launch, a docking, an outcome), for telling its owner.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteChange {
    pub owner: u64,
    pub route: u64,
}

fn trip_leg(t: &Trip) -> Leg {
    match t.leg() {
        TripLeg::Ascent => Leg::Ascent,
        TripLeg::SoiTransfer => Leg::SoiTransfer,
        TripLeg::Landing | TripLeg::Landed => Leg::Landing,
        TripLeg::Failed => Leg::Failed,
    }
}

fn tanks(craft: &Craft, which: &dyn Fn(&str) -> bool, resource: &str) -> (Vec<usize>, f64) {
    let mut parts = Vec::new();
    let mut total = 0.0;
    for (i, p) in craft.parts.iter().enumerate() {
        if !which(&p.guid) {
            continue;
        }
        for r in p.resources.iter().filter(|r| r.resource == resource) {
            parts.push(i);
            total += r.stored_units;
        }
    }
    parts.dedup();
    (parts, total)
}

/// The ascent of a proving flight: lift-off to orbit, and the angle flown downrange.
fn ascent_profile(r: &crate::recording::Recording) -> Option<(f64, f64)> {
    let at = |want: fn(&Event) -> bool| r.markers.iter().find(|m| want(&m.event)).map(|m| m.ut);
    let up = at(|e| *e == Event::Liftoff)?;
    let orbit = at(|e| matches!(e, Event::Orbit { .. }))?;
    let near = |ut: f64| {
        r.trajectory
            .iter()
            .min_by(|a, b| (a.ut - ut).abs().total_cmp(&(b.ut - ut).abs()))
    };
    let (a, b) = (near(up)?, near(orbit)?);
    let angle =
        a.r.normalized()
            .dot(b.r.normalized())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
    Some((orbit - up, angle))
}

impl Routes {
    /// Make a route from a finished one-way proving flight.
    pub fn create_route(&mut self, player: u64, proving: u64, name: &str) -> Result<u64, String> {
        let p = self
            .provings
            .iter()
            .find(|p| p.id == proving)
            .ok_or("no such recording")?;
        let r = &p.recorder.recording;
        let (depot, depot_port, own_port, endpoint) = match (&r.state, &p.colony) {
            (State::Landed, Some(c)) => {
                (0, String::new(), String::new(), Endpoint::Colony(c.clone()))
            }
            (State::OneWay, _) => {
                let depot = p.depot.ok_or("the recording has no depot")?;
                let (Some(depot_port), Some(own_port)) = (r.depot_port.clone(), r.own_port.clone())
                else {
                    return Err("the recording has no docking ports".into());
                };
                (depot, depot_port, own_port, Endpoint::Depot)
            }
            _ => {
                return Err(
                    "only a one-way recording (ended docked at its depot, or landed at a \
                            colony) makes a route yet"
                        .into(),
                )
            }
        };
        let colony = matches!(endpoint, Endpoint::Colony(_));

        let site = r.trajectory.first().ok_or("the recording has no flight")?;
        let name = name.trim();
        let name = if name.is_empty() {
            format!(
                "{} to {}",
                r.craft.name,
                r.depot.clone().unwrap_or_default()
            )
        } else {
            name.chars().take(64).collect()
        };
        let route = Route {
            id: 0,
            name,
            owner: player,
            proving,
            craft: r.craft.clone(),
            site_body: site.body.clone(),
            site_r: site.r,
            site_ut: site.ut,
            depot,
            depot_port,
            own_port,
            cargo: r.delivered.clone(),
            ascent: ascent_profile(r),
            paused: None,
            status: "Waiting for a launch window".into(),
            next_launch: None,
            active: None,
            runs: Vec::new(),
            // a run landed at a colony stays as its stock
            when_delivered: if colony {
                WhenDelivered::Stay
            } else {
                WhenDelivered::Rotate
            },
            stayed: None,
            endpoint,
        };
        let id = self.fresh_route_id();
        self.routes.push(Route { id, ..route });
        Ok(id)
    }

    fn route_mut(&mut self, id: u64) -> Result<&mut Route, String> {
        self.routes
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| "no such route".to_string())
    }

    /// What the route's runs do once delivered.
    pub fn set_when_delivered(&mut self, id: u64, when: WhenDelivered) -> Result<(), String> {
        let r = self.route_mut(id)?;
        if r.colony().is_some() && when != WhenDelivered::Stay {
            return Err("a run landed at a colony stays there as its stock".into());
        }
        r.when_delivered = when;
        r.next_launch = None;
        Ok(())
    }

    /// Send home, let go or keep the run the route left docked at the depot.
    pub fn stayed_command(
        &mut self,
        u: &Universe,
        pilots: &mut Pilots,
        id: u64,
        cmd: StayedCommand,
    ) -> Result<(), String> {
        let depots = self.depots.clone();
        let r = self.route_mut(id)?;
        let n = r
            .stayed
            .ok_or("no run of this route is docked at the depot")?;
        let (after, leaving) = match cmd {
            StayedCommand::Keep => ("kept at the depot".to_string(), None),
            StayedCommand::SendHome | StayedCommand::LetGo => {
                let home = cmd == StayedCommand::SendHome;
                let d = depots
                    .iter()
                    .find(|d| d.id == r.depot)
                    .ok_or("the depot is gone")?;
                let (depart, dv) = send_off(u, d, &r.depot_port)?;
                pilots.homebound.push(Homebound {
                    route: id,
                    n,
                    home,
                    leave: Leave::Undock(Box::new(depart), dv),
                });
                ("undocking".to_string(), Some(home))
            }
        };
        r.stayed = None;
        r.next_launch = None;
        if let Some(x) = r.runs.iter_mut().find(|x| x.n == n) {
            x.after = Some(after);
            x.leaving = leaving;
        }
        Ok(())
    }

    /// Pause a route (its run in flight carries on) or resume it.
    pub fn pause_route(&mut self, id: u64, paused: bool) -> Result<(), String> {
        let r = self.route_mut(id)?;
        r.paused = paused.then(|| "paused".to_string());
        r.next_launch = None;
        Ok(())
    }

    /// A colony was abandoned: its routes pause (their runs in flight carry on).
    pub fn colony_abandoned(&mut self, colony: u64) {
        for r in self
            .routes
            .iter_mut()
            .filter(|r| r.colony().is_some_and(|c| c.id == colony))
        {
            r.paused = Some("the colony was abandoned".into());
            r.next_launch = None;
        }
    }

    /// Delete a route. Its run in flight is left where it is, flown by nobody.
    pub fn delete_route(&mut self, pilots: &mut Pilots, id: u64) -> Result<(), String> {
        let i = self
            .routes
            .iter()
            .position(|r| r.id == id)
            .ok_or("no such route")?;
        self.routes.remove(i);
        pilots.runs.remove(&id);
        Ok(())
    }

    /// Once a second: which routes should launch now. Also ends a run whose pilot is gone and could not be
    /// made again (`resume`).
    pub fn schedule(&mut self, u: &mut Universe, pilots: &Pilots) -> Vec<LaunchRequest> {
        let ut = u.ut();
        let mut out = Vec::new();
        let depots = self.depots.clone();
        for r in &mut self.routes {
            if let Some(a) = r.active.clone() {
                // (before `fly` has resumed the pilots, the run is still theirs)
                if pilots.resumed && !pilots.runs.contains_key(&r.id) {
                    r.runs.push(RunLog {
                        n: a.n,
                        vessel_name: a.vessel_name,
                        launched_ut: a.launched_ut,
                        ended_ut: ut,
                        outcome: Outcome::Missed("the pilot was interrupted".into()),
                        delivered: BTreeMap::new(),
                        after: None,
                        leaving: None,
                        flown_by: a.flown_by,
                    });
                    r.active = None;
                } else {
                    continue;
                }
            }
            if let Some(why) = &r.paused {
                r.status = format!("Paused: {why}");
                r.next_launch = None;
                continue;
            }
            if r.colony().is_some() {
                // no window to wait for: the trip phases its own transfer
                let when = *r.next_launch.get_or_insert(ut + ROLLOUT_S);
                if ut < when {
                    r.status = format!("Next launch in {:.0} s", when - ut);
                    continue;
                }
                r.status = "Launching".into();
                out.push(LaunchRequest {
                    route: r.id,
                    owner: r.owner,
                    craft: r.craft.clone(),
                    vessel_name: format!("{} {}", r.name, r.runs.len() + 1),
                });
                continue;
            }
            let Some(d) = depots.iter().find(|d| d.id == r.depot) else {
                r.status = "Waiting: the depot is no longer a depot".into();
                continue;
            };
            let Some(dv) = d.vessel() else {
                r.status = format!("Waiting: {} cannot be found", d.name);
                continue;
            };
            // a rotating route frees the port of its own run that stayed
            let freeable = r.when_delivered == WhenDelivered::Rotate && r.stayed.is_some();
            if !freeable && !port_free(u, d, dv, &r.depot_port) {
                r.status = format!("Waiting: no free port at {}", d.name);
                r.next_launch = None;
                continue;
            }
            let when = match r.next_launch {
                Some(t) => t,
                None => {
                    let t = launch_time(u, r, dv).unwrap_or(ut + ROLLOUT_S);
                    r.next_launch = Some(t);
                    t
                }
            };
            if ut < when {
                r.status = format!("Next launch in {:.0} s", when - ut);
                continue;
            }
            r.status = "Launching".into();
            out.push(LaunchRequest {
                route: r.id,
                owner: r.owner,
                craft: r.craft.clone(),
                vessel_name: format!("{} {}", r.name, r.runs.len() + 1),
            });
        }
        out
    }

    /// The server could not launch: the route tries again shortly.
    pub fn launch_failed(&mut self, route: u64, why: &str) {
        if let Ok(r) = self.route_mut(route) {
            r.status = format!("Waiting: {why}");
            r.next_launch = None;
        }
    }

    /// The server launched the route's craft as `vessel`: the pilot takes it.
    pub fn launched(
        &mut self,
        u: &Universe,
        pilots: &mut Pilots,
        route: u64,
        vessel: Entity,
        vessel_name: &str,
    ) -> Result<(), String> {
        let depots = self.depots.clone();
        let r = self.route_mut(route)?;
        let n = r.runs.len() as u32 + 1;
        let stage = match r.colony() {
            Some(c) => Stage::Trip(Box::new(colony_trip(u, c, vessel, true, n)?)),
            None => {
                let d = depots.iter().find(|d| d.id == r.depot).ok_or("no depot")?;
                Stage::Flying(Box::new(run_mission(u, r, d, vessel, true)?))
            }
        };
        pilots.runs.insert(route, stage);
        r.active = Some(ActiveRun {
            n,
            vessel_name: vessel_name.into(),
            launched_ut: u.ut(),
            delivered: BTreeMap::new(),
            manual: None,
            flown_by: Vec::new(),
        });
        r.next_launch = None;
        r.status = "Run in flight".into();
        Ok(())
    }

    /// Every simulation step: fly each run. Returns the routes whose run moved on.
    pub fn fly(
        &mut self,
        u: &mut Universe,
        pilots: &mut Pilots,
        catalog: &Catalog,
    ) -> Vec<RouteChange> {
        let mut out = Vec::new();
        if !pilots.resumed {
            pilots.resumed = true;
            out = self.resume(u, pilots);
        }
        if pilots.runs.is_empty() && pilots.homebound.is_empty() {
            return out;
        }
        let ut = u.ut();
        let depots = self.depots.clone();
        let mut leaving: Vec<Homebound> = Vec::new();
        for r in &mut self.routes {
            let Some(stage) = pilots.runs.get_mut(&r.id) else {
                continue;
            };
            let Some(active) = r.active.clone() else {
                pilots.runs.remove(&r.id);
                continue;
            };
            let depot = depots.iter().find(|d| d.id == r.depot);
            let mut ended: Option<(Outcome, BTreeMap<String, f64>)> = None;
            let by_hand = match &*stage {
                Stage::Flying(m) => Some(m.vessel()),
                Stage::Trip(t) => Some(t.vessel()),
                Stage::Pumping { .. } => None,
            }
            .filter(|e| pilots.manual.contains_key(e) || active.manual.is_some());
            if let Some(e) = by_hand {
                {
                    let gone = u.vessel(e).is_none();
                    match pilots.manual.get(&e).copied().filter(|_| !gone) {
                        Some(player) => {
                            // flown by hand: the pilot stands aside
                            if active.manual != Some(player) {
                                if let Some(a) = r.active.as_mut() {
                                    a.manual = Some(player);
                                    if !a.flown_by.contains(&player) {
                                        a.flown_by.push(player);
                                    }
                                }
                                out.push(RouteChange {
                                    owner: r.owner,
                                    route: r.id,
                                });
                            }
                            r.status = format!("Run {}: flown by hand", active.n);
                        }
                        None => {
                            // handed back (or docked, or lost, by hand): pick it up from where it is
                            let found = (!gone).then_some(e);
                            let mut a = active.clone();
                            a.manual = None;
                            match pick_up(u, r, depot, &mut a, found) {
                                Ok(next) => {
                                    *stage = next;
                                    r.active = Some(a);
                                    r.status = format!("Run {}: handed back", active.n);
                                }
                                Err(outcome) => {
                                    end_run(r, a, ut, outcome);
                                    pilots.runs.remove(&r.id);
                                }
                            }
                            out.push(RouteChange {
                                owner: r.owner,
                                route: r.id,
                            });
                        }
                    }
                    continue;
                }
            }
            match stage {
                Stage::Flying(m) => {
                    let before = m.leg();
                    let status = m.update(u, catalog);
                    if m.leg() != before {
                        out.push(RouteChange {
                            owner: r.owner,
                            route: r.id,
                        });
                    }
                    r.status = format!("Run {}: {:?}", active.n, m.leg());
                    // in orbit: the run that stayed makes way for this one
                    let in_orbit = matches!(
                        m.leg(),
                        Leg::Plan | Leg::Transfer | Leg::SoiTransfer | Leg::Brake | Leg::Approach
                    );
                    if in_orbit && r.when_delivered == WhenDelivered::Rotate {
                        if let (Some(n), Some(d)) = (r.stayed, depot) {
                            r.stayed = None;
                            let after = match send_off(u, d, &r.depot_port) {
                                Ok((depart, dv)) => {
                                    leaving.push(Homebound {
                                        route: r.id,
                                        n,
                                        home: true,
                                        leave: Leave::Undock(Box::new(depart), dv),
                                    });
                                    "undocking to fly home".to_string()
                                }
                                Err(why) => format!("could not undock: {why}"),
                            };
                            let going = after.starts_with("undocking");
                            if let Some(x) = r.runs.iter_mut().find(|x| x.n == n) {
                                x.after = Some(after);
                                x.leaving = going.then_some(true);
                            }
                        }
                    }
                    match status {
                        Status::Running if ut - active.launched_ut > RUN_TIME_LIMIT_S => {
                            ended = Some((Outcome::Missed("out of time".into()), BTreeMap::new()));
                        }
                        Status::Running => {}
                        Status::Failed(why) => {
                            let e = m.vessel();
                            let gone =
                                u.vessel(e).is_none() || u.powered(e).is_some_and(|p| p.crashed);
                            ended = Some((
                                if gone {
                                    Outcome::Lost(why)
                                } else {
                                    Outcome::Missed(why)
                                },
                                BTreeMap::new(),
                            ));
                        }
                        Status::Done => {
                            let queue: Vec<(String, f64)> = r
                                .cargo
                                .iter()
                                .filter(|(_, v)| **v > 1e-6)
                                .map(|(k, v)| (k.clone(), *v))
                                .collect();
                            *stage = Stage::Pumping {
                                queue,
                                current: None,
                                delivered: BTreeMap::new(),
                            };
                            r.status = format!("Run {}: docked, pumping", active.n);
                            out.push(RouteChange {
                                owner: r.owner,
                                route: r.id,
                            });
                        }
                    }
                }
                Stage::Trip(t) => {
                    let before = t.leg();
                    let status = t.update(u);
                    if t.leg() != before {
                        out.push(RouteChange {
                            owner: r.owner,
                            route: r.id,
                        });
                    }
                    r.status = format!("Run {}: {:?}", active.n, trip_leg(t));
                    let e = t.vessel();
                    match status {
                        Status::Running if ut - active.launched_ut > RUN_TIME_LIMIT_S => {
                            ended = Some((Outcome::Missed("out of time".into()), BTreeMap::new()));
                        }
                        Status::Running => {}
                        Status::Failed(why) => {
                            let gone =
                                u.vessel(e).is_none() || u.powered(e).is_some_and(|p| p.crashed);
                            ended = Some((
                                if gone {
                                    Outcome::Lost(why)
                                } else {
                                    Outcome::Missed(why)
                                },
                                BTreeMap::new(),
                            ));
                        }
                        Status::Done => {
                            if let Some(c) = r.colony() {
                                ended = Some(arrived(u, c, e, &r.cargo));
                            }
                        }
                    }
                }
                Stage::Pumping {
                    queue,
                    current,
                    delivered,
                } => match depot.and_then(|d| Some((d, d.vessel()?))) {
                    None => {
                        ended = Some((Outcome::Lost("the depot is gone".into()), delivered.clone()))
                    }
                    Some((d, dv)) => {
                        let done = pump(u, d, dv, queue, current, delivered).is_some();
                        if let Some(a) = r.active.as_mut() {
                            a.delivered = delivered.clone();
                        }
                        if done {
                            let full = r
                                .cargo
                                .iter()
                                .all(|(k, v)| delivered.get(k).copied().unwrap_or(0.0) >= 0.99 * v);
                            ended = Some((
                                if full {
                                    Outcome::Delivered
                                } else {
                                    Outcome::Short
                                },
                                delivered.clone(),
                            ));
                        }
                    }
                },
            }
            if let Some((outcome, delivered)) = ended {
                if let Outcome::Lost(why) = &outcome {
                    // don't keep sending craft into the same wreck (design 3.4)
                    r.paused = Some(format!("run {} was lost: {why}", active.n));
                }
                let docked = matches!(outcome, Outcome::Delivered | Outcome::Short);
                let mut after = None;
                let mut going = None;
                if docked && r.colony().is_some() {
                    after = Some(LANDED_AT_COLONY.to_string());
                } else if docked {
                    after = Some(match r.when_delivered {
                        WhenDelivered::Stay | WhenDelivered::Rotate => {
                            r.stayed = Some(active.n);
                            "docked".to_string()
                        }
                        WhenDelivered::Return | WhenDelivered::LetGo => {
                            let home = r.when_delivered == WhenDelivered::Return;
                            match depot.map(|d| send_off(u, d, &r.depot_port)) {
                                Some(Ok((depart, dv))) => {
                                    leaving.push(Homebound {
                                        route: r.id,
                                        n: active.n,
                                        home,
                                        leave: Leave::Undock(Box::new(depart), dv),
                                    });
                                    going = Some(home);
                                    "undocking".to_string()
                                }
                                Some(Err(why)) => format!("could not undock: {why}"),
                                None => "the depot is gone".to_string(),
                            }
                        }
                    });
                }
                r.runs.push(RunLog {
                    n: active.n,
                    vessel_name: active.vessel_name,
                    launched_ut: active.launched_ut,
                    ended_ut: ut,
                    outcome,
                    delivered,
                    after,
                    leaving: going,
                    flown_by: active.flown_by.clone(),
                });
                r.active = None;
                r.status = "Run ended".into();
                pilots.runs.remove(&r.id);
                out.push(RouteChange {
                    owner: r.owner,
                    route: r.id,
                });
            }
        }
        pilots.homebound.extend(leaving);
        self.fly_home(u, pilots, catalog, &mut out);
        out
    }

    /// The runs leaving the depot: back off, then fly home or drift.
    fn fly_home(
        &mut self,
        u: &mut Universe,
        pilots: &mut Pilots,
        catalog: &Catalog,
        out: &mut Vec<RouteChange>,
    ) {
        let mut keep = Vec::new();
        for mut h in std::mem::take(&mut pilots.homebound) {
            let name = self
                .routes
                .iter()
                .find(|r| r.id == h.route)
                .and_then(|r| r.runs.iter().find(|x| x.n == h.n))
                .map(|x| x.vessel_name.clone());
            let (note, done) = match &mut h.leave {
                Leave::Undock(d, docked) => match d.update(u, *docked, catalog) {
                    // the undocked half keeps the run's name, to be found again after a restart
                    Status::Running if d.vessel.is_some() => {
                        if let (Some(v), Some(name)) = (d.vessel, &name) {
                            if u.vessel(v).is_some_and(|x| x.0.name != *name) {
                                u.rename_vessel(v, name);
                            }
                        }
                        (None, false)
                    }
                    Status::Running => (None, false),
                    Status::Done if h.home => match d.vessel {
                        Some(e) => {
                            if let Some(name) = &name {
                                u.rename_vessel(e, name);
                            }
                            h.leave =
                                Leave::Descend(Box::new(Descent::new(DescentParams::ksc())), e);
                            (Some("flying home".to_string()), false)
                        }
                        None => (Some("lost after undocking".to_string()), true),
                    },
                    Status::Done => (Some("let go".to_string()), true),
                    Status::Failed(why) => (Some(format!("could not undock: {why}")), true),
                },
                Leave::Descend(d, e) => match d.update(u, *e) {
                    Status::Running => (None, false),
                    Status::Done => {
                        // recovered where it came to rest
                        u.despawn_vessel(*e);
                        (Some("recovered".to_string()), true)
                    }
                    Status::Failed(why) => (Some(format!("lost on the way home: {why}")), true),
                },
            };
            if let Some(note) = note {
                if let Some(r) = self.routes.iter_mut().find(|r| r.id == h.route) {
                    if let Some(x) = r.runs.iter_mut().find(|x| x.n == h.n) {
                        x.after = Some(note);
                        if done {
                            x.leaving = None;
                        }
                    }
                    out.push(RouteChange {
                        owner: r.owner,
                        route: r.id,
                    });
                }
            }
            if !done {
                keep.push(h);
            }
        }
        pilots.homebound = keep;
    }

    /// Make the pilots again from the saved routes, after a restart or a reload: each run in flight, and
    /// each run leaving the depot, is found by its vessel's name and carries on from where it is.
    pub fn resume(&mut self, u: &mut Universe, pilots: &mut Pilots) -> Vec<RouteChange> {
        let ut = u.ut();
        let depots = self.depots.clone();
        let depot_vessels: Vec<Entity> = depots.iter().filter_map(|d| d.vessel()).collect();
        let mut out = Vec::new();
        for r in &mut self.routes {
            let depot = depots.iter().find(|d| d.id == r.depot);
            let mut changed = false;
            if let Some(a) = r
                .active
                .clone()
                .filter(|_| !pilots.runs.contains_key(&r.id))
            {
                changed = true;
                let found = find_by_name(u, &a.vessel_name, &depot_vessels);
                let mut a = a;
                let resumed = pick_up(u, r, depot, &mut a, found);
                match resumed {
                    Ok(stage) => {
                        r.status = format!("Run {}: resumed", a.n);
                        pilots.runs.insert(r.id, stage);
                    }
                    Err(outcome) => end_run(r, a, ut, outcome),
                }
            }
            for x in r.runs.iter_mut().filter(|x| x.leaving.is_some()) {
                if pilots
                    .homebound
                    .iter()
                    .any(|h| h.route == r.id && h.n == x.n)
                {
                    continue;
                }
                changed = true;
                let home = x.leaving == Some(true);
                let leave = match find_by_name(u, &x.vessel_name, &depot_vessels) {
                    Some(e) if home => Ok(Leave::Descend(
                        Box::new(Descent::new(DescentParams::ksc())),
                        e,
                    )),
                    Some(_) => Err("let go".to_string()),
                    // still docked: undock again
                    None => match depot.map(|d| send_off(u, d, &r.depot_port)) {
                        Some(Ok((depart, dv))) => Ok(Leave::Undock(Box::new(depart), dv)),
                        Some(Err(why)) => Err(format!("could not undock: {why}")),
                        None => Err("the depot is gone".to_string()),
                    },
                };
                match leave {
                    Ok(leave) => pilots.homebound.push(Homebound {
                        route: r.id,
                        n: x.n,
                        home,
                        leave,
                    }),
                    Err(note) => {
                        x.after = Some(note);
                        x.leaving = None;
                    }
                }
            }
            if changed {
                out.push(RouteChange {
                    owner: r.owner,
                    route: r.id,
                });
            }
        }
        out
    }

    fn fresh_route_id(&mut self) -> u64 {
        self.bump_id()
    }
}

/// Pick up a run whose pilot is gone or was set aside (a restart, or a player handing it back), from where
/// its vessel `found` is now: still climbing (or on the pad), it ascends again; in orbit, it plans from
/// there; with no vessel but the depot port taken, it docked, and pumps what is left of its cargo.
fn pick_up(
    u: &mut Universe,
    r: &Route,
    depot: Option<&Depot>,
    a: &mut ActiveRun,
    found: Option<Entity>,
) -> Result<Stage, Outcome> {
    let docked = r.stayed.is_none()
        && depot.is_some_and(|d| {
            d.vessel()
                .is_some_and(|dv| !port_free(u, d, dv, &r.depot_port))
        });
    match found {
        Some(e) => {
            let p = u.powered(e).or_else(|| u.dormant(e));
            if p.is_some_and(|p| p.crashed) {
                return Err(Outcome::Lost("crashed".into()));
            }
            let landed = p.is_some_and(|p| p.landed);
            if let Some(c) = r.colony() {
                if landed && colony_distance(u, e, c).is_some_and(|d| d <= ose_colonies::RADIUS_M) {
                    // it landed in the colony meanwhile (while the server was down, or by hand)
                    let (outcome, held) = arrived(u, c, e, &r.cargo);
                    a.delivered = held;
                    return Err(outcome);
                }
            }
            let climbing = p.is_some_and(|p| p.landed)
                || match (ose_pilot::orbit(u, e), ose_pilot::body(u, e)) {
                    (Some(o), Some((b, _, radius))) => {
                        o.periapsis() < radius + ose_pilot::mission::safe_altitude(u, b)
                    }
                    _ => true,
                };
            if let Some(c) = r.colony() {
                return colony_trip(u, c, e, climbing, a.n)
                    .map(|t| Stage::Trip(Box::new(t)))
                    .map_err(Outcome::Missed);
            }
            depot
                .ok_or("the depot is gone".to_string())
                .and_then(|d| run_mission(u, r, d, e, climbing))
                .map(|m| Stage::Flying(Box::new(m)))
                .map_err(Outcome::Missed)
        }
        None if docked => Ok(Stage::Pumping {
            queue: r
                .cargo
                .iter()
                .map(|(k, v)| (k.clone(), v - a.delivered.get(k).copied().unwrap_or(0.0)))
                .filter(|(_, v)| *v > 1e-6)
                .collect(),
            current: None,
            delivered: a.delivered.clone(),
        }),
        None => Err(Outcome::Missed("the run's vessel is gone".into())),
    }
}

/// End the route's run `a` with `outcome`, without delivering (a delivery ends in `fly`).
fn end_run(r: &mut Route, a: ActiveRun, ut: f64, outcome: Outcome) {
    if let Outcome::Lost(why) = &outcome {
        // don't keep sending craft into the same wreck (design 3.4)
        r.paused = Some(format!("run {} was lost: {why}", a.n));
    }
    let landed = r.colony().is_some() && matches!(outcome, Outcome::Delivered | Outcome::Short);
    r.runs.push(RunLog {
        n: a.n,
        vessel_name: a.vessel_name,
        launched_ut: a.launched_ut,
        ended_ut: ut,
        outcome,
        delivered: a.delivered,
        after: landed.then(|| LANDED_AT_COLONY.to_string()),
        leaving: None,
        flown_by: a.flown_by,
    });
    r.active = None;
    r.status = "Run ended".into();
}

/// What a run that landed at a colony did after: it stays.
const LANDED_AT_COLONY: &str = "landed at the colony";

/// The route pilot for a run to a colony: climbing first if `ascend`, then across to the colony's body and
/// down at the run's aim point.
fn colony_trip(
    u: &Universe,
    c: &ColonySite,
    vessel: Entity,
    ascend: bool,
    n: u32,
) -> Result<Trip, String> {
    let b = u.body_entity(&c.body).ok_or("the colony's body is gone")?;
    let radius = u
        .world
        .get::<Body>(b)
        .ok_or("the colony's body is gone")?
        .radius_m;
    let (lat, lon) = aim_point(c, radius, n);
    Ok(Trip::new(
        vessel,
        TripParams {
            body: c.body.clone(),
            landing: LandingParams::at(lat, lon),
            ascent: ascend.then(|| AscentParams {
                stage_in_orbit: true,
                ..AscentParams::default()
            }),
        },
    ))
}

/// Where run `n` lands: `COLONY_AIM_M` from the site, each run a golden angle further round.
pub fn aim_point(c: &ColonySite, radius_m: f64, n: u32) -> (f64, f64) {
    let d = COLONY_AIM_M / radius_m;
    let a = n as f64 * 2.399_963;
    let lat = c.lat_deg + (d * a.cos()).to_degrees();
    let lon = c.lon_deg + (d * a.sin() / c.lat_deg.to_radians().cos().max(0.05)).to_degrees();
    (lat, lon)
}

/// How far a vessel stands from the colony's site over the ground (m), if it is on the colony's body.
fn colony_distance(u: &Universe, e: Entity, c: &ColonySite) -> Option<f64> {
    let p = u.powered(e).or_else(|| u.dormant(e))?;
    let (b, _, _) = ose_pilot::body(u, e)?;
    let body = u.world.get::<Body>(b)?;
    if body.id != c.body {
        return None;
    }
    let here = ose_sim::flight::lat_lon(body, p.r, u.ut());
    Some(ose_colonies::surface_distance(
        body.radius_m,
        here,
        (c.lat_deg, c.lon_deg),
    ))
}

/// A run's trip ended landed: within the colony it delivered what its tanks hold (electric charge and
/// intake air are free); outside it missed.
fn arrived(
    u: &Universe,
    c: &ColonySite,
    e: Entity,
    cargo: &BTreeMap<String, f64>,
) -> (Outcome, BTreeMap<String, f64>) {
    match colony_distance(u, e, c) {
        Some(d) if d <= ose_colonies::RADIUS_M => {
            let mut held = BTreeMap::new();
            for p in u.current_craft(e).map(|c| c.parts).unwrap_or_default() {
                for r in p.resources {
                    if !ose_colonies::FREE.contains(&r.resource.as_str()) && r.stored_units > 1e-6 {
                        *held.entry(r.resource).or_insert(0.0) += r.stored_units;
                    }
                }
            }
            let full = cargo
                .iter()
                .all(|(k, v)| held.get(k).copied().unwrap_or(0.0) >= 0.99 * v);
            (
                if full {
                    Outcome::Delivered
                } else {
                    Outcome::Short
                },
                held,
            )
        }
        Some(d) => (
            Outcome::Missed(format!("landed {d:.0} m from the colony")),
            BTreeMap::new(),
        ),
        None => (
            Outcome::Missed("landed away from the colony's body".into()),
            BTreeMap::new(),
        ),
    }
}

/// The route pilot for a run: to the depot's recorded port, climbing first if `ascend`.
fn run_mission(
    u: &Universe,
    r: &Route,
    d: &Depot,
    vessel: Entity,
    ascend: bool,
) -> Result<Mission, String> {
    let dv = d.vessel().ok_or("no depot vessel")?;
    let port = d
        .current_name(&r.depot_port)
        .ok_or("the depot's port is gone")?;
    let mut params = MissionParams::new(dv, port, &r.own_port);
    if ascend {
        let plane = ose_pilot::state(u, dv).map(|(r, v)| r.cross(v).normalized());
        params.ascent = Some(AscentParams {
            stage_in_orbit: true,
            plane,
            ..AscentParams::default()
        });
    }
    Ok(Mission::new(vessel, params))
}

/// The vessel named `name` that carries no depot (the first, if several).
fn find_by_name(u: &mut Universe, name: &str, depots: &[Entity]) -> Option<Entity> {
    u.vessels()
        .into_iter()
        .filter(|e| !depots.contains(e))
        .find(|e| u.vessel(*e).is_some_and(|v| v.0.name == name))
}

/// Undock what is docked at the depot's recorded port: a pilot that backs it off, and the depot's vessel.
fn send_off(u: &Universe, d: &Depot, port: &str) -> Result<(Depart, Entity), String> {
    let dv = d.vessel().ok_or("the depot cannot be found")?;
    let name = d.current_name(port).ok_or("the depot's port is gone")?;
    let craft = u.current_craft(dv).ok_or("the depot has no parts")?;
    let i = craft
        .parts
        .iter()
        .position(|p| p.guid == name)
        .ok_or("the depot's port is gone")?;
    let other = u
        .docked_links(dv)
        .into_iter()
        .find_map(|(a, b)| {
            if a == i {
                Some(b)
            } else if b == i {
                Some(a)
            } else {
                None
            }
        })
        .ok_or("nothing is docked there")?;
    Ok((Depart::new(&craft.parts[other].guid), dv))
}

/// Whether the depot's recorded port has nothing docked to it.
fn port_free(u: &Universe, d: &Depot, dv: Entity, port: &str) -> bool {
    let Some(name) = d.current_name(port) else {
        return false;
    };
    let Some(craft) = u.current_craft(dv) else {
        return false;
    };
    let Some(i) = craft.parts.iter().position(|p| p.guid == name) else {
        return false;
    };
    !u.docked_links(dv).iter().any(|&(a, b)| a == i || b == i)
}

/// When to lift off: the next launch window for the depot's orbit, from the proving flight's ascent.
fn launch_time(u: &Universe, r: &Route, depot: Entity) -> Option<f64> {
    let (duration_s, downrange_deg) = r.ascent?;
    let (body_e, _, _) = ose_pilot::body(u, depot)?;
    let body = u.world.get::<Body>(body_e)?;
    if body.id != r.site_body {
        return None;
    }
    let target = ose_pilot::orbit(u, depot)?;
    let now = u.ut();
    // the site turns with the body from where the proving flight stood
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let a = omega * (now - r.site_ut);
    let (s, c) = a.sin_cos();
    let site = DVec3::new(
        r.site_r.x * c - r.site_r.y * s,
        r.site_r.x * s + r.site_r.y * c,
        r.site_r.z,
    );
    let w = window::search(
        body,
        site,
        now,
        &target,
        LaunchProfile {
            duration_s,
            downrange_deg,
        },
        2.0,
        now + ROLLOUT_S,
        WINDOW_HORIZON_S,
    );
    Some(w.ut.max(now + ROLLOUT_S))
}

/// Pump the route's cargo into the depot, one resource at a time. Returns `Some` when done.
fn pump(
    u: &mut Universe,
    d: &Depot,
    dv: Entity,
    queue: &mut Vec<(String, f64)>,
    current: &mut Option<(String, f64, f64, f64, f64)>,
    delivered: &mut BTreeMap<String, f64>,
) -> Option<()> {
    let ut = u.ut();
    let craft = u.current_craft(dv)?;
    let is_depot = |g: &str| d.parts.contains(g);
    if let Some((res, start, goal, last, last_ut)) = current.clone() {
        let (_, now) = tanks(&craft, &is_depot, &res);
        delivered.insert(res.clone(), (now - start).max(0.0));
        let moving = now > last + 1e-6;
        if now >= goal - 1e-6 || (!moving && ut - last_ut > PUMP_STALL_S) {
            let _ = u.transfer_resource(dv, &res, &[], &[]);
            *current = None;
        } else if moving {
            *current = Some((res, start, goal, now, ut));
        }
        return None;
    }
    let Some((res, want)) = queue.first().cloned() else {
        return Some(());
    };
    queue.remove(0);
    let (from, _) = tanks(&craft, &|g| !is_depot(g), &res);
    let (to, have) = tanks(&craft, &is_depot, &res);
    if from.is_empty() || to.is_empty() || u.transfer_resource(dv, &res, &from, &to).is_err() {
        delivered.insert(res, 0.0);
        return None;
    }
    *current = Some((res, have, have + want, have, ut));
    None
}

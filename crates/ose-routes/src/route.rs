//! Routes and their runs (design 3.3, 3.4 and 3.10). A route is made from a finished proving flight. At each
//! launch window the scheduler asks the server for a fresh copy of the craft on the pad, and the route
//! pilot (`ose_pilot::mission`) flies it to the depot like any vessel: it is simulated, drawn and can hit
//! things. Docked, it pumps what the proving flight delivered.
//!
//! Step 2a covers one-way routes from KSC that stay docked: each run takes a port, and the route waits while
//! the recorded port is taken (design 3.10, "no launch when all ports are full"). Rotate, return home, let
//! go and depot-to-depot shuttles come next.
//!
//! The route itself is saved with the campaign. Its pilot lives only in memory (`Pilots`): a run interrupted
//! by a restart or a reload ends as Missed, and its vessel stays where it is.
use crate::book::{Depot, Routes};
use crate::recording::{Event, State};
use bevy_ecs::entity::Entity;
use ose_data::craft::Craft;
use ose_math::DVec3;
use ose_pilot::{
    ascent::AscentParams,
    mission::{Catalog, Leg, Mission, MissionParams},
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
}

/// The run in flight now, as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveRun {
    pub n: u32,
    pub vessel_name: String,
    pub launched_ut: f64,
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
    Pumping {
        /// What is left to pump, and the one going now with its depot total at the start, the target total,
        /// and the last amount seen moving and when.
        queue: Vec<(String, f64)>,
        current: Option<(String, f64, f64, f64, f64)>,
        delivered: BTreeMap<String, f64>,
    },
}

/// The route pilots, by route. Not saved.
#[derive(Default)]
pub struct Pilots {
    runs: BTreeMap<u64, Stage>,
}

impl Pilots {
    /// The leg the route's run is flying, if it is flying.
    pub fn leg(&self, route: u64) -> Option<Leg> {
        match self.runs.get(&route)? {
            Stage::Flying(m) => Some(m.leg()),
            Stage::Pumping { .. } => None,
        }
    }

    /// The vessel a route's run is, while flying.
    pub fn vessel(&self, route: u64) -> Option<Entity> {
        match self.runs.get(&route)? {
            Stage::Flying(m) => Some(m.vessel()),
            Stage::Pumping { .. } => None,
        }
    }

    /// The fastest warp every run in flight allows: 1 while one docks or pumps, none without runs.
    pub fn suggested_warp(&self, u: &Universe) -> Option<u32> {
        self.runs
            .values()
            .map(|s| match s {
                Stage::Flying(m) => m.suggested_warp(u),
                Stage::Pumping { .. } => 1,
            })
            .min()
    }

    /// Forget every pilot (the universe was replaced).
    pub fn clear(&mut self) {
        self.runs.clear();
    }
}

/// A route's runs changed (a launch, a docking, an outcome), for telling its owner.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteChange {
    pub owner: u64,
    pub route: u64,
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
        if r.state != State::OneWay {
            return Err(
                "only a one-way recording (ended docked at its depot) makes a route yet".into(),
            );
        }
        let depot = p.depot.ok_or("the recording has no depot")?;
        let (Some(depot_port), Some(own_port)) = (r.depot_port.clone(), r.own_port.clone()) else {
            return Err("the recording has no docking ports".into());
        };
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

    /// Pause a route (its run in flight carries on) or resume it.
    pub fn pause_route(&mut self, id: u64, paused: bool) -> Result<(), String> {
        let r = self.route_mut(id)?;
        r.paused = paused.then(|| "paused".to_string());
        r.next_launch = None;
        Ok(())
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

    /// Once a second: which routes should launch now. Also ends runs whose pilot is gone (after a restart
    /// or a reload).
    pub fn schedule(&mut self, u: &mut Universe, pilots: &Pilots) -> Vec<LaunchRequest> {
        let ut = u.ut();
        let mut out = Vec::new();
        let depots = self.depots.clone();
        for r in &mut self.routes {
            if let Some(a) = r.active.clone() {
                if !pilots.runs.contains_key(&r.id) {
                    r.runs.push(RunLog {
                        n: a.n,
                        vessel_name: a.vessel_name,
                        launched_ut: a.launched_ut,
                        ended_ut: ut,
                        outcome: Outcome::Missed("the pilot was interrupted".into()),
                        delivered: BTreeMap::new(),
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
            let Some(d) = depots.iter().find(|d| d.id == r.depot) else {
                r.status = "Waiting: the depot is no longer a depot".into();
                continue;
            };
            let Some(dv) = d.vessel() else {
                r.status = format!("Waiting: {} cannot be found", d.name);
                continue;
            };
            if !port_free(u, d, dv, &r.depot_port) {
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
        let d = depots.iter().find(|d| d.id == r.depot).ok_or("no depot")?;
        let dv = d.vessel().ok_or("no depot vessel")?;
        let port = d
            .current_name(&r.depot_port)
            .ok_or("the depot's port is gone")?;
        let mut params = MissionParams::new(dv, port, &r.own_port);
        let plane = ose_pilot::state(u, dv).map(|(r, v)| r.cross(v).normalized());
        params.ascent = Some(AscentParams {
            stage_in_orbit: true,
            plane,
            ..AscentParams::default()
        });
        pilots
            .runs
            .insert(route, Stage::Flying(Box::new(Mission::new(vessel, params))));
        r.active = Some(ActiveRun {
            n: r.runs.len() as u32 + 1,
            vessel_name: vessel_name.into(),
            launched_ut: u.ut(),
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
        if pilots.runs.is_empty() {
            return Vec::new();
        }
        let ut = u.ut();
        let mut out = Vec::new();
        let depots = self.depots.clone();
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
                Stage::Pumping {
                    queue,
                    current,
                    delivered,
                } => match depot.and_then(|d| Some((d, d.vessel()?))) {
                    None => {
                        ended = Some((Outcome::Lost("the depot is gone".into()), delivered.clone()))
                    }
                    Some((d, dv)) => {
                        if pump(u, d, dv, queue, current, delivered).is_some() {
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
                r.runs.push(RunLog {
                    n: active.n,
                    vessel_name: active.vessel_name,
                    launched_ut: active.launched_ut,
                    ended_ut: ut,
                    outcome,
                    delivered,
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
        out
    }

    fn fresh_route_id(&mut self) -> u64 {
        self.bump_id()
    }
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

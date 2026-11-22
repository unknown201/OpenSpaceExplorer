//! Depot-to-depot shuttles (design 3.12). There is no launch pad at a station, so a shuttle route has one
//! vessel, the one that flew the proving flight, and it goes round and round: it fills up at the origin
//! depot, flies to the destination depot, pumps the cargo across, and flies back to the origin. The origin
//! pays: what the shuttle carries and burns comes out of the origin depot's tanks, so the route waits while
//! the origin is short ("Waiting: Station Alpha is short of MonoPropellant").
//!
//! The proving flight starts in orbit next to the origin depot (`Routes::start`, the recorder's
//! `start_from`) and ends docked at the destination, like a one-way flight from KSC. Each leg is a
//! rendezvous of `ose_pilot::mission`: undock from one depot's port and dock at the other's, in the same SOI
//! or across SOIs (the mission plans the hop).
//!
//! The shuttle's leg is saved; its pilot is not (`Pilots::shuttles`). Whenever the pilot is missing (after a
//! restart or a reload, or once a player hands the shuttle back) it is made again from the leg and from
//! where the shuttle is: free and flying (found by its name), or docked at one depot's port or the other's.
//! If the shuttle is nowhere, it was lost, and the route stops until someone flies a new proving flight.
use crate::book::{Depot, Routes};
use crate::recording::State;
use crate::route::{
    docked_partner, find_by_name, port_free, pump, settle, tanks, Outcome, Pilots, RouteChange,
    RunLog, RUN_TIME_LIMIT_S,
};
use bevy_ecs::entity::Entity;
use ose_pilot::{
    mission::{Catalog, Leg, Mission, MissionParams},
    Status,
};
use ose_sim::Universe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Loading waits this long before trying again when the origin is short (s).
pub const RETRY_LOAD_S: f64 = 60.0;

/// Where a shuttle is in its round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShuttleLeg {
    /// Docked at the origin, filling up.
    Loading,
    /// On the way to the destination.
    Outbound,
    /// Docked at the destination, pumping the cargo across.
    Unloading,
    /// On the way back to the origin.
    Inbound,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shuttle {
    pub id: u64,
    pub name: String,
    pub owner: u64,
    /// The proving flight it was made from.
    pub proving: u64,
    /// `Depot::id` of where it fills up and where it delivers.
    pub origin: u64,
    pub dest: u64,
    /// The destination's port, named as when that depot was marked.
    pub dest_port: String,
    /// The origin's port it docks at, named as when that depot was marked; chosen from the free ports the
    /// first time it comes back.
    pub origin_port: Option<String>,
    /// The shuttle's own port, by its name in the shuttle's vessel now (docking renames repeated names).
    pub own_port: String,
    /// What each trip pumps into the destination: what the proving flight delivered.
    pub cargo: BTreeMap<String, f64>,
    /// What the proving flight burned on the way, which the shuttle must carry besides the cargo.
    pub fuel: BTreeMap<String, f64>,
    /// The shuttle vessel's name (it gets it back after each undocking).
    pub vessel_name: String,
    pub leg: ShuttleLeg,
    /// When the leg, or the trip, started.
    pub leg_ut: f64,
    pub trip_ut: f64,
    /// The trip under way (1 for the first).
    pub n: u32,
    /// Pumped into the destination so far on this trip.
    pub delivered: BTreeMap<String, f64>,
    pub paused: Option<String>,
    pub status: String,
    pub trips: Vec<RunLog>,
    /// The player flying it by hand now, and everyone who did on this trip (design 3.11).
    #[serde(default)]
    pub manual: Option<u64>,
    #[serde(default)]
    pub flown_by: Vec<u64>,
    /// It failed (lost or stuck), and waits for a player to resume the route.
    #[serde(default)]
    pub stuck: bool,
}

/// A shuttle's pilot, in memory only.
pub(crate) enum ShuttleStage {
    Fly(Box<Mission>),
    /// Filling up at the origin, or unloading at the destination.
    Pump {
        dv: Entity,
        queue: Vec<(String, f64)>,
        current: Option<(String, f64, f64, f64, f64)>,
        moved: BTreeMap<String, f64>,
    },
    /// Docked at the origin until then (short of something, or paused).
    Wait(f64),
}

impl ShuttleStage {
    pub(crate) fn vessel(&self) -> Option<Entity> {
        match self {
            ShuttleStage::Fly(m) => Some(m.flying()),
            _ => None,
        }
    }

    pub(crate) fn suggested_warp(
        &self,
        u: &Universe,
        manual: &BTreeMap<Entity, u64>,
    ) -> Option<u32> {
        match self {
            ShuttleStage::Fly(m) if manual.contains_key(&m.flying()) => None,
            ShuttleStage::Fly(m) => Some(m.suggested_warp(u)),
            ShuttleStage::Pump { .. } => Some(1),
            ShuttleStage::Wait(_) => None,
        }
    }

    pub(crate) fn leg(&self) -> Option<Leg> {
        match self {
            ShuttleStage::Fly(m) => Some(m.leg()),
            _ => None,
        }
    }
}

/// Where the shuttle is now.
enum Found {
    Flying(Entity),
    AtOrigin,
    AtDest,
    Nowhere,
}

fn change(s: &Shuttle) -> RouteChange {
    RouteChange {
        owner: s.owner,
        route: s.id,
    }
}

impl Routes {
    /// Make a shuttle route from a proving flight that left a depot and ended docked at another.
    pub(crate) fn create_shuttle(
        &mut self,
        player: u64,
        proving: u64,
        name: &str,
    ) -> Result<u64, String> {
        let p = self
            .provings
            .iter()
            .find(|p| p.id == proving)
            .ok_or("no such recording")?;
        let r = &p.recorder.recording;
        if r.state != State::OneWay {
            return Err("a shuttle's recording must end docked at the other depot".into());
        }
        let origin = p.origin.ok_or("the recording did not leave a depot")?;
        let dest = p.depot.ok_or("the recording has no depot")?;
        if origin == dest {
            return Err("a shuttle goes to another depot than the one it left".into());
        }
        let (Some(dest_port), Some(own_port)) = (r.depot_port.clone(), r.own_port.clone()) else {
            return Err("the recording has no docking ports".into());
        };
        if self.shuttles.iter().any(|s| s.proving == proving) {
            return Err("this recording already has its shuttle".into());
        }
        let name = name.trim();
        let name = if name.is_empty() {
            format!(
                "{} shuttle to {}",
                r.origin.clone().unwrap_or_default(),
                r.depot.clone().unwrap_or_default()
            )
        } else {
            name.chars().take(64).collect()
        };
        let shuttle = Shuttle {
            id: 0,
            vessel_name: name.clone(),
            name,
            owner: player,
            proving,
            origin,
            dest,
            dest_port,
            origin_port: None,
            own_port,
            cargo: r.delivered.clone(),
            fuel: Routes::used(p),
            // the proving flight delivered: the first trip starts with the way back
            leg: ShuttleLeg::Inbound,
            leg_ut: 0.0,
            trip_ut: 0.0,
            n: 1,
            delivered: BTreeMap::new(),
            paused: None,
            status: "Leaving for the origin".into(),
            trips: Vec::new(),
            manual: None,
            flown_by: Vec::new(),
            stuck: false,
        };
        let id = self.bump_id();
        self.shuttles.push(Shuttle { id, ..shuttle });
        Ok(id)
    }

    /// Every simulation step: fly each shuttle, making its pilot again when it is missing.
    pub(crate) fn fly_shuttles(
        &mut self,
        u: &mut Universe,
        pilots: &mut Pilots,
        catalog: &Catalog,
        out: &mut Vec<RouteChange>,
    ) {
        if self.shuttles.is_empty() {
            return;
        }
        let ut = u.ut();
        let depots = self.depots.clone();
        let depot_vessels: Vec<Entity> = depots.iter().filter_map(|d| d.vessel()).collect();
        for s in &mut self.shuttles {
            let (Some(o), Some(d)) = (
                depots.iter().find(|d| d.id == s.origin),
                depots.iter().find(|d| d.id == s.dest),
            ) else {
                s.status = "Waiting: a depot is no longer a depot".into();
                pilots.shuttles.remove(&s.id);
                continue;
            };
            // flown by hand: the pilot stands aside; handed back, it is made again from where it is
            if let Some(e) = pilots
                .shuttles
                .get(&s.id)
                .and_then(|st| st.vessel())
                .filter(|e| !depot_vessels.contains(e))
            {
                if let Some(&player) = pilots.manual.get(&e) {
                    if s.manual != Some(player) {
                        s.manual = Some(player);
                        if !s.flown_by.contains(&player) {
                            s.flown_by.push(player);
                        }
                        out.push(change(s));
                    }
                    s.status = format!("Trip {}: flown by hand", s.n);
                    continue;
                }
            }
            if s.manual.take().is_some() {
                pilots.shuttles.remove(&s.id);
                out.push(change(s));
            }
            let stage = match pilots.shuttles.entry(s.id) {
                std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::btree_map::Entry::Vacant(e) => {
                    // stuck until a player resumes it; paused, it stays at the origin
                    if s.stuck || (s.paused.is_some() && s.leg == ShuttleLeg::Loading) {
                        if !s.stuck {
                            s.status = format!("Paused at {}", o.name);
                        }
                        continue;
                    }
                    match start_leg(u, s, o, d, &depot_vessels, catalog) {
                        Ok(stage) => e.insert(stage),
                        Err(outcome) => {
                            lost(s, ut, outcome);
                            out.push(change(s));
                            continue;
                        }
                    }
                }
            };
            let next = match stage {
                ShuttleStage::Wait(t) => {
                    if ut >= *t {
                        Next::Again
                    } else {
                        Next::Stay
                    }
                }
                ShuttleStage::Fly(m) => {
                    let before = m.leg();
                    let status = m.update(u, catalog);
                    // the undocked shuttle keeps its name, to be found again
                    let e = m.flying();
                    if u.vessel(e).is_some_and(|v| v.0.name != s.vessel_name)
                        && !depot_vessels.contains(&e)
                    {
                        u.rename_vessel(e, &s.vessel_name);
                    }
                    if m.leg() != before {
                        out.push(change(s));
                    }
                    let to = if s.leg == ShuttleLeg::Outbound {
                        &d.name
                    } else {
                        &o.name
                    };
                    s.status = format!("Trip {}: {:?} to {to}", s.n, m.leg());
                    match status {
                        Status::Running if ut - s.leg_ut > RUN_TIME_LIMIT_S => {
                            Next::Fail(Outcome::Missed("out of time".into()))
                        }
                        Status::Running => Next::Stay,
                        Status::Failed(why) => {
                            let gone =
                                u.vessel(e).is_none() || u.powered(e).is_some_and(|p| p.crashed);
                            Next::Fail(if gone {
                                Outcome::Lost(why)
                            } else {
                                Outcome::Missed(why)
                            })
                        }
                        Status::Done => Next::Docked,
                    }
                }
                ShuttleStage::Pump {
                    dv,
                    queue,
                    current,
                    moved,
                } => {
                    let into: Box<dyn Fn(&str) -> bool> = if s.leg == ShuttleLeg::Unloading {
                        let parts = d.parts.clone();
                        Box::new(move |g| parts.contains(g))
                    } else {
                        let parts = o.parts.clone();
                        Box::new(move |g| !parts.contains(g))
                    };
                    let done = pump(u, *dv, &into, queue, current, moved).is_some();
                    if s.leg == ShuttleLeg::Unloading {
                        for (k, v) in moved.iter() {
                            s.delivered.insert(k.clone(), *v);
                        }
                    }
                    if done {
                        Next::Pumped(*dv)
                    } else {
                        Next::Stay
                    }
                }
            };
            match next {
                Next::Stay => {}
                Next::Again => {
                    pilots.shuttles.remove(&s.id);
                }
                Next::Fail(outcome) => {
                    pilots.shuttles.remove(&s.id);
                    lost(s, ut, outcome);
                    out.push(change(s));
                }
                Next::Docked => {
                    pilots.shuttles.remove(&s.id);
                    s.leg = if s.leg == ShuttleLeg::Outbound {
                        ShuttleLeg::Unloading
                    } else {
                        ShuttleLeg::Loading
                    };
                    s.leg_ut = ut;
                    out.push(change(s));
                }
                Next::Pumped(dv) => {
                    pilots.shuttles.remove(&s.id);
                    if s.leg == ShuttleLeg::Unloading {
                        let full = s
                            .cargo
                            .iter()
                            .all(|(k, v)| s.delivered.get(k).copied().unwrap_or(0.0) >= 0.99 * v);
                        s.trips.push(RunLog {
                            n: s.n,
                            vessel_name: s.vessel_name.clone(),
                            launched_ut: s.trip_ut,
                            ended_ut: ut,
                            outcome: if full {
                                Outcome::Delivered
                            } else {
                                Outcome::Short
                            },
                            delivered: std::mem::take(&mut s.delivered),
                            after: Some("back to the origin".into()),
                            leaving: None,
                            flown_by: std::mem::take(&mut s.flown_by),
                        });
                        s.status = format!("Trip {} done, back to {}", s.n, o.name);
                        s.n += 1;
                        s.leg = ShuttleLeg::Inbound;
                        s.leg_ut = ut;
                    } else {
                        // filled up: enough to deliver the cargo and get there?
                        let craft = u.current_craft(dv);
                        let short = s
                            .cargo
                            .keys()
                            .chain(s.fuel.keys())
                            .find(|res| {
                                let need = s.cargo.get(*res).copied().unwrap_or(0.0)
                                    + s.fuel.get(*res).copied().unwrap_or(0.0);
                                let have = craft
                                    .as_ref()
                                    .map(|c| tanks(c, &|g| !o.parts.contains(g), res).1)
                                    .unwrap_or(0.0);
                                need > 1e-6 && have < 0.99 * need
                            })
                            .cloned();
                        if let Some(res) = short {
                            s.status = format!("Waiting: {} is short of {res}", o.name);
                            pilots
                                .shuttles
                                .insert(s.id, ShuttleStage::Wait(ut + RETRY_LOAD_S));
                        } else if s.paused.is_some() {
                            s.status = format!("Paused at {}", o.name);
                        } else {
                            s.leg = ShuttleLeg::Outbound;
                            s.leg_ut = ut;
                            s.trip_ut = ut;
                        }
                    }
                    out.push(change(s));
                }
            }
        }
    }
}

enum Next {
    Stay,
    /// Make the pilot again (after a wait).
    Again,
    Fail(Outcome),
    Docked,
    Pumped(Entity),
}

/// The shuttle failed: log the trip and stop the route until a player sees to it.
fn lost(s: &mut Shuttle, ut: f64, outcome: Outcome) {
    let why = match &outcome {
        Outcome::Lost(why) => format!("the shuttle was lost: {why}"),
        Outcome::Missed(why) => format!("the shuttle is stuck: {why}"),
        _ => "stopped".into(),
    };
    s.trips.push(RunLog {
        n: s.n,
        vessel_name: s.vessel_name.clone(),
        launched_ut: s.trip_ut,
        ended_ut: ut,
        outcome,
        delivered: std::mem::take(&mut s.delivered),
        after: None,
        leaving: None,
        flown_by: std::mem::take(&mut s.flown_by),
    });
    s.status = format!("Paused: {why}");
    s.paused = Some(why);
    s.stuck = true;
}

/// Where the shuttle is: flying free (by its name), or docked at the origin's port or the destination's.
fn find(u: &mut Universe, s: &Shuttle, o: &Depot, d: &Depot, depot_vessels: &[Entity]) -> Found {
    if let Some(e) = find_by_name(u, &s.vessel_name, depot_vessels) {
        return Found::Flying(e);
    }
    let docked = |u: &Universe, dep: &Depot, port: &str| {
        dep.vessel().is_some_and(|v| !port_free(u, dep, v, port))
    };
    // a leg that ends docked is checked first, at its end
    let at_dest = docked(u, d, &s.dest_port);
    let at_origin = s.origin_port.as_deref().is_some_and(|p| docked(u, o, p));
    match (s.leg, at_origin, at_dest) {
        (ShuttleLeg::Loading | ShuttleLeg::Inbound, true, _) => Found::AtOrigin,
        (ShuttleLeg::Unloading | ShuttleLeg::Outbound, _, true) => Found::AtDest,
        (_, true, _) => Found::AtOrigin,
        (_, _, true) => Found::AtDest,
        _ => Found::Nowhere,
    }
}

/// Make the shuttle's pilot for its leg, from where the shuttle is now.
fn start_leg(
    u: &mut Universe,
    s: &mut Shuttle,
    o: &Depot,
    d: &Depot,
    depot_vessels: &[Entity],
    catalog: &Catalog,
) -> Result<ShuttleStage, Outcome> {
    let ut = u.ut();
    let found = find(u, s, o, d, depot_vessels);
    // where it is decides the leg when they disagree (a player flew it, or it was stopped half way)
    s.leg = match (s.leg, &found) {
        (_, Found::Nowhere) => return Err(Outcome::Lost("the shuttle is gone".into())),
        // at the origin: about to leave, or filling up
        (ShuttleLeg::Outbound, Found::AtOrigin) => ShuttleLeg::Outbound,
        (_, Found::AtOrigin) => ShuttleLeg::Loading,
        // at the destination: about to leave, or unloading
        (ShuttleLeg::Inbound | ShuttleLeg::Loading, Found::AtDest) => ShuttleLeg::Inbound,
        (_, Found::AtDest) => ShuttleLeg::Unloading,
        (ShuttleLeg::Loading, Found::Flying(_)) => ShuttleLeg::Inbound,
        (ShuttleLeg::Unloading, Found::Flying(_)) => ShuttleLeg::Outbound,
        (leg, Found::Flying(_)) => leg,
    };
    if s.leg_ut == 0.0 {
        s.leg_ut = ut;
    }
    if s.trip_ut == 0.0 {
        s.trip_ut = ut;
    }
    let missed = |why: &str| Outcome::Missed(why.to_string());
    match s.leg {
        ShuttleLeg::Unloading => {
            let dv = d
                .vessel()
                .ok_or_else(|| missed("the depot cannot be found"))?;
            s.status = format!("Trip {}: unloading at {}", s.n, d.name);
            Ok(ShuttleStage::Pump {
                dv,
                queue: s
                    .cargo
                    .iter()
                    .map(|(k, v)| (k.clone(), v - s.delivered.get(k).copied().unwrap_or(0.0)))
                    .filter(|(_, v)| *v > 1e-6)
                    .collect(),
                current: None,
                moved: s.delivered.clone(),
            })
        }
        ShuttleLeg::Loading => {
            let ov = o
                .vessel()
                .ok_or_else(|| missed("the depot cannot be found"))?;
            let craft = u.current_craft(ov).ok_or_else(|| missed("no origin"))?;
            s.status = format!("Filling up at {}", o.name);
            // top up what it carries and burns, as far as its tanks take
            let mut queue = Vec::new();
            for res in s.cargo.keys().chain(s.fuel.keys()) {
                if queue.iter().any(|(r, _)| r == res) {
                    continue;
                }
                let room: f64 = craft
                    .parts
                    .iter()
                    .filter(|p| !o.parts.contains(&p.guid))
                    .flat_map(|p| p.resources.iter())
                    .filter(|r| &r.resource == res)
                    .map(|r| (r.capacity_units - r.stored_units).max(0.0))
                    .sum();
                if room > 1e-6 {
                    queue.push((res.clone(), room));
                }
            }
            Ok(ShuttleStage::Pump {
                dv: ov,
                queue,
                current: None,
                moved: BTreeMap::new(),
            })
        }
        ShuttleLeg::Outbound | ShuttleLeg::Inbound => {
            let outbound = s.leg == ShuttleLeg::Outbound;
            let (to, from) = if outbound { (d, o) } else { (o, d) };
            let tv = to
                .vessel()
                .ok_or_else(|| missed("the depot cannot be found"))?;
            // where it starts: docked at the other depot (undock first), or flying
            let (vessel, depart) = match found {
                Found::Flying(e) => {
                    settle(u, e);
                    (e, None)
                }
                _ => {
                    let from_port = if outbound {
                        s.origin_port.clone().unwrap_or_default()
                    } else {
                        s.dest_port.clone()
                    };
                    let (partner, fv) =
                        docked_partner(u, from, &from_port).map_err(|why| missed(&why))?;
                    s.own_port = partner.clone();
                    (fv, Some(partner))
                }
            };
            let port = if outbound {
                d.current_name(&s.dest_port)
                    .ok_or_else(|| missed("the depot's port is gone"))?
                    .to_string()
            } else {
                let port = origin_port(u, s, o, vessel, catalog)
                    .ok_or_else(|| missed("no free port at the origin"))?;
                s.origin_port = o.original_name(&port).map(str::to_string);
                port
            };
            let mut params = MissionParams::new(tv, &port, &s.own_port);
            params.depart_from_port = depart;
            Ok(ShuttleStage::Fly(Box::new(Mission::new(vessel, params))))
        }
    }
}

/// The origin's port to come back to: the one it used before while free, or a free one that fits the
/// shuttle's port.
fn origin_port(
    u: &Universe,
    s: &Shuttle,
    o: &Depot,
    shuttle: Entity,
    catalog: &Catalog,
) -> Option<String> {
    let ov = o.vessel()?;
    if let Some(p) = s.origin_port.as_deref() {
        if port_free(u, o, ov, p) {
            return o.current_name(p).map(str::to_string);
        }
    }
    let craft = u.current_craft(ov)?;
    let sizes: Vec<String> = u
        .current_craft(shuttle)
        .and_then(|c| {
            let part = c.parts.iter().find(|p| p.guid == s.own_port)?;
            let def = catalog.defs.iter().find(|d| d.id == part.part_id)?;
            Some(def.docking.as_ref()?.sizes.clone())
        })
        .unwrap_or_default();
    ose_sim::docking::free_ports(&craft, catalog.defs)
        .into_iter()
        .filter(|p| o.parts.contains(&craft.parts[p.part].guid))
        .filter(|p| sizes.is_empty() || p.def.sizes.iter().any(|z| sizes.contains(z)))
        .map(|p| craft.parts[p.part].guid.clone())
        .next()
}

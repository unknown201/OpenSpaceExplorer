//! The proving flight: a player flies a route once, and the server records what happened. Design 3.2.
//!
//! The recorder is driven by observation, not by hooks into every server command: call
//! [`Recorder::observe`] every tick (it samples at 10 Hz in physics and 1 Hz on rails) and it works out
//! liftoff, orbit, arrival at the depot, docking, what was transferred, what was left behind, deorbit and
//! landing from the vessel's state. The vessel is followed by its parts' identities, so docking (when the
//! depot's entity may absorb ours) and undocking (when we may come away as a new entity) keep working.
use bevy_ecs::entity::Entity;
use ose_data::craft::Craft;
use ose_math::DVec3;
use ose_sim::{
    bodies::{Body, ReferenceBody},
    flight::{self, Powered, Situation},
    Universe,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Within this distance of a depot the vessel has arrived, and its trajectory is kept in the depot's
/// frame so an approach can be replayed against wherever the depot is now (m).
pub const DEPOT_BUBBLE_M: f64 = 3_000.0;
/// Trajectory sample spacing in physics and on rails (s).
pub const PHYSICS_SAMPLE_S: f64 = 0.1;
pub const RAILS_SAMPLE_S: f64 = 1.0;
/// Engines quiet this long end a burn (s).
pub const BURN_GAP_S: f64 = 5.0;

/// Stage of the proving flight. Fuel and time are kept per phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Pad,
    Ascent,
    /// In orbit: phasing, transfer burns and corrections, up to arrival at the depot.
    Transfer,
    /// Inside the depot's bubble, before docking.
    Approach,
    Docked,
    /// Undocked, flying home until the deorbit.
    Return,
    Descent,
}

/// Something that happened, in order, with its universe time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Liftoff,
    Orbit {
        periapsis_m: f64,
        apoapsis_m: f64,
    },
    BurnStart,
    BurnEnd,
    Arrived {
        depot: String,
    },
    Docked {
        depot_port: String,
        own_port: String,
    },
    Undocked {
        /// Our parts that stayed on the depot.
        payload: Vec<String>,
    },
    Deorbit,
    Landed {
        lat_deg: f64,
        lon_deg: f64,
        splashed: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub ut: f64,
    pub event: Event,
}

/// Time and resources one phase took. `used` counts what left our own tanks (burned or transferred).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseStats {
    pub phase: Option<Phase>,
    pub start_ut: f64,
    pub end_ut: f64,
    pub used: BTreeMap<String, f64>,
}

/// One trajectory sample. `rel` is the position and velocity relative to the depot, in the depot's own
/// axes (right, fore, top), while inside its bubble.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub ut: f64,
    pub body: String,
    pub r: DVec3,
    pub v: DVec3,
    pub rel: Option<(DVec3, DVec3)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    Recording,
    /// Recovered after flying home: a round trip.
    RoundTrip,
    /// Finished while docked at the depot: the vessel stays (design 3.10).
    OneWay,
    Dropped(String),
}

/// A finished or running proving flight, as kept in the save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// The craft as it stood on the pad.
    pub craft: Craft,
    pub site_body: String,
    pub site_lat_deg: f64,
    pub site_lon_deg: f64,
    /// The depot's name, and its port and ours, once docked. Our port is named as in `craft`; the depot's
    /// as the depot was marked (`book::Depot::original`), once the routes book has seen the docking.
    pub depot: Option<String>,
    pub depot_port: Option<String>,
    pub own_port: Option<String>,
    pub markers: Vec<Marker>,
    pub phases: Vec<PhaseStats>,
    /// Resources the depot's tanks gained while we were docked, by resource.
    pub delivered: BTreeMap<String, f64>,
    /// Our parts left docked at the depot.
    pub payload: Vec<String>,
    pub trajectory: Vec<Sample>,
    pub state: State,
}

impl Recording {
    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.markers.iter().map(|m| &m.event)
    }

    /// Total of a resource used across all phases.
    pub fn used(&self, resource: &str) -> f64 {
        self.phases
            .iter()
            .filter_map(|p| p.used.get(resource))
            .sum()
    }

    pub fn duration_s(&self) -> f64 {
        match (self.phases.first(), self.phases.last()) {
            (Some(a), Some(b)) => b.end_ut - a.start_ut,
            _ => 0.0,
        }
    }
}

/// Which vessels count as depots: the server answers from the players' depot flags.
pub type IsDepot<'a> = &'a dyn Fn(&Universe, Entity) -> bool;

/// Saved with the campaign, so a quicksave, a revert and a restart keep it. The entities are only hints: the
/// vessel is found again by its parts, and the depot by distance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recorder {
    pub recording: Recording,
    /// Our parts (still with us, or left as payload). Parts dropped by staging leave this set.
    ours: BTreeSet<String>,
    #[serde(skip, default = "no_entity")]
    vessel: Entity,
    phase: Phase,
    phase_start: f64,
    phase_tanks: BTreeMap<String, f64>,
    last_sample: f64,
    burning: bool,
    last_lit: f64,
    /// The depot we are near or docked to, and the depot's tanks when we docked.
    #[serde(skip)]
    depot: Option<Entity>,
    depot_tanks_at_dock: BTreeMap<String, f64>,
    depot_guids: BTreeSet<String>,
    /// The nearest depot's parts, as last seen before docking.
    near_guids: BTreeSet<String>,
    /// Our vessel's parts last tick, in order. Crafts launched from the editor share part names (`p1`,
    /// `p2`, ...), and docking renames the repeats, so the docked craft is told apart by position: the
    /// vessel kept comes first, the one joined to it after (`ose_sim::docking::merge`).
    #[serde(default)]
    prev_parts: Vec<String>,
    /// Where we were last seen (body, position, speed, time), to pick our piece among same-named copies.
    #[serde(default)]
    last_seen: Option<(String, DVec3, f64, f64)>,
}

fn no_entity() -> Entity {
    Entity::PLACEHOLDER
}

fn sum_tanks(craft: &Craft, which: &dyn Fn(&str) -> bool) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for p in craft.parts.iter().filter(|p| which(&p.guid)) {
        for r in &p.resources {
            *out.entry(r.resource.clone()).or_insert(0.0) += r.stored_units;
        }
    }
    out
}

fn body_of(u: &Universe, e: Entity) -> Option<(Entity, Body)> {
    let b = u.world.get::<ReferenceBody>(e)?.0;
    Some((b, u.world.get::<Body>(b)?.clone()))
}

fn flight_state(u: &Universe, e: Entity) -> Option<(&Powered, bool)> {
    if let Some(p) = u.powered(e) {
        return Some((p, true));
    }
    u.dormant(e).map(|p| (p, false))
}

/// The lowest altitude that counts as orbit: the top of the atmosphere, or 10 km over an airless body.
fn orbit_floor(u: &Universe, b: Entity) -> f64 {
    u.atmosphere(b).map(|a| a.depth_m).unwrap_or(10_000.0)
}

impl Recorder {
    /// Start recording a vessel standing on the pad or runway.
    pub fn start(u: &Universe, vessel: Entity) -> Result<Self, String> {
        let craft = u.current_craft(vessel).ok_or("no such vessel")?;
        let (p, _) = flight_state(u, vessel).ok_or("the vessel is not flying")?;
        if !p.landed {
            return Err("recording starts on the pad or runway".into());
        }
        let (_, body) = body_of(u, vessel).ok_or("no body")?;
        let (lat, lon) = flight::lat_lon(&body, p.r, u.ut());
        let ours: BTreeSet<String> = craft.parts.iter().map(|p| p.guid.clone()).collect();
        let tanks = sum_tanks(&craft, &|_| true);
        let prev_parts = craft.parts.iter().map(|p| p.guid.clone()).collect();
        Ok(Self {
            recording: Recording {
                craft,
                site_body: body.id.clone(),
                site_lat_deg: lat,
                site_lon_deg: lon,
                depot: None,
                depot_port: None,
                own_port: None,
                markers: Vec::new(),
                phases: Vec::new(),
                delivered: BTreeMap::new(),
                payload: Vec::new(),
                trajectory: Vec::new(),
                state: State::Recording,
            },
            ours,
            vessel,
            phase: Phase::Pad,
            phase_start: u.ut(),
            phase_tanks: tanks,
            // finite, so the save's JSON keeps it
            last_sample: f64::MIN,
            burning: false,
            last_lit: 0.0,
            depot: None,
            depot_tanks_at_dock: BTreeMap::new(),
            depot_guids: BTreeSet::new(),
            near_guids: BTreeSet::new(),
            prev_parts,
            last_seen: Some((body.id.clone(), p.r, p.v.length(), u.ut())),
        })
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The entity carrying our parts now.
    pub fn vessel(&self) -> Entity {
        self.vessel
    }

    /// The universe was replaced (a quickload or a revert): the vessel is found again by its parts.
    pub fn universe_replaced(&mut self) {
        self.vessel = Entity::PLACEHOLDER;
        self.depot = None;
    }

    pub fn is_recording(&self) -> bool {
        self.recording.state == State::Recording
    }

    fn mark(&mut self, ut: f64, event: Event) {
        self.recording.markers.push(Marker { ut, event });
    }

    /// Our parts still with the vessel (not left on the depot).
    fn with_us(&self, guid: &str) -> bool {
        self.ours.contains(guid) && !self.recording.payload.iter().any(|g| g == guid)
    }

    fn close_phase(&mut self, ut: f64, craft: &Craft, next: Phase) {
        let now = sum_tanks(craft, &|g| self.with_us(g));
        let mut used = BTreeMap::new();
        for (k, before) in &self.phase_tanks {
            let d = before - now.get(k).copied().unwrap_or(0.0);
            if d.abs() > 1e-6 {
                used.insert(k.clone(), d);
            }
        }
        self.recording.phases.push(PhaseStats {
            phase: Some(self.phase),
            start_ut: self.phase_start,
            end_ut: ut,
            used,
        });
        self.phase = next;
        self.phase_start = ut;
        self.phase_tanks = now;
    }

    fn drop_it(&mut self, why: &str) {
        self.recording.state = State::Dropped(why.into());
    }

    /// Find the entity that carries our parts, after a merge or split. Docked, the vessel must hold all of
    /// them: after undocking, our piece is the one without the depot's parts. Among copies of the same craft
    /// (same part names) only those near where we were last seen count.
    fn follow(&mut self, u: &mut Universe) -> Option<Craft> {
        let docked = self.phase == Phase::Docked;
        let holds = |me: &Self, c: &Craft| {
            let n = c.parts.iter().filter(|p| me.with_us(&p.guid)).count();
            if docked {
                n == me.ours.len()
            } else {
                n > 0
            }
        };
        if let Some(c) = u.current_craft(self.vessel) {
            if holds(self, &c) {
                return Some(c);
            }
        }
        let ut = u.ut();
        let near = |u: &Universe, e: Entity, me: &Self| -> Option<f64> {
            let Some((body, r, speed, t)) = &me.last_seen else {
                return Some(0.0);
            };
            let (p, _) = flight_state(u, e)?;
            let (_, b) = body_of(u, e)?;
            let d = (p.r - *r).length();
            let gate = 1_000.0 + 2.0 * speed * (ut - t).abs();
            (b.id == *body && d < gate).then_some(d)
        };
        // (no depot parts while docked, our part count, -distance)
        let mut best: Option<((bool, usize, f64), Entity, Craft)> = None;
        let mut hints = vec![self.depot];
        hints.extend(u.vessels().into_iter().map(Some));
        for e in hints.into_iter().flatten() {
            let Some(c) = u.current_craft(e) else {
                continue;
            };
            let n = c.parts.iter().filter(|p| self.with_us(&p.guid)).count();
            if n == 0 {
                continue;
            }
            let Some(d) = near(u, e, self) else {
                continue;
            };
            let free = !c.parts.iter().any(|p| self.depot_guids.contains(&p.guid));
            let key = (!docked || free, n, -d);
            if best.as_ref().is_none_or(|b| key > b.0) {
                best = Some((key, e, c));
            }
        }
        let (_, e, c) = best?;
        self.vessel = e;
        Some(c)
    }

    /// Look at the vessel once; call every tick (it samples at its own rate).
    pub fn observe(&mut self, u: &mut Universe, is_depot: IsDepot) {
        if !self.is_recording() {
            return;
        }
        let ut = u.ut();
        let Some(craft) = self.follow(u) else {
            return self.drop_it("the vessel was destroyed");
        };
        // parts dropped by staging are no longer ours
        let present: BTreeSet<&str> = craft.parts.iter().map(|p| p.guid.as_str()).collect();
        if self.phase < Phase::Docked {
            self.ours.retain(|g| present.contains(g.as_str()));
        }
        let e = self.vessel;
        let Some((p, physics)) = flight_state(u, e).map(|(p, ph)| (p.clone(), ph)) else {
            return self.drop_it("the vessel was destroyed");
        };
        if p.crashed {
            return self.drop_it("the vessel was destroyed");
        }
        let Some((b, body)) = body_of(u, e) else {
            return;
        };
        let alt = p.r.length() - body.radius_m;
        let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
        let floor = orbit_floor(u, b);
        let foreign: BTreeSet<String> = craft
            .parts
            .iter()
            .filter(|q| !self.ours.contains(&q.guid))
            .map(|q| q.guid.clone())
            .collect();

        // Docking: our vessel grew. Our parts are its first or its last ones, the depot's the rest.
        let grew = !self.prev_parts.is_empty() && craft.parts.len() > self.prev_parts.len();
        let guids: Vec<String> = craft.parts.iter().map(|q| q.guid.clone()).collect();
        let (n, m) = (self.prev_parts.len(), guids.len());
        let prev = std::mem::replace(&mut self.prev_parts, guids.clone());
        self.last_seen = Some((body.id.clone(), p.r, p.v.length(), ut));
        if self.phase < Phase::Docked && grew {
            let (mine, theirs) = if guids[..n] == prev[..] {
                (0..n, n..m)
            } else {
                (m - n..m, 0..m - n)
            };
            // the parts we joined must be the depot's we were closing on
            if self.near_guids.is_empty() || theirs.len() != self.near_guids.len() {
                return self.drop_it("docked with something that is not its depot");
            }
            self.ours = guids[mine.clone()].iter().cloned().collect();
            let foreign: BTreeSet<String> = guids[theirs.clone()].iter().cloned().collect();
            // our port by its name in the craft as launched (docking may have renamed it), the
            // depot's by its name in the docked vessel
            let mut depot_port = None;
            let mut own_port = None;
            for (a, c) in u.docked_links(e) {
                for (x, y) in [(a, c), (c, a)] {
                    if mine.contains(&x) && theirs.contains(&y) {
                        own_port = prev.get(x - mine.start).cloned();
                        depot_port = guids.get(y).cloned();
                    }
                }
            }
            self.depot_guids = foreign.clone();
            self.depot_tanks_at_dock = sum_tanks(&craft, &|g| foreign.contains(g));
            self.recording.depot_port = depot_port.clone();
            self.recording.own_port = own_port.clone();
            self.close_phase(ut, &craft, Phase::Docked);
            self.mark(
                ut,
                Event::Docked {
                    depot_port: depot_port.unwrap_or_default(),
                    own_port: own_port.unwrap_or_default(),
                },
            );
            return;
        }
        if self.phase == Phase::Docked {
            // keep the depot's gains up to date, so finishing while docked has them
            let now = sum_tanks(&craft, &|g| self.depot_guids.contains(g));
            self.update_delivered(&now);
            if foreign.is_empty() {
                // undocked: our parts that did not come with us stayed on the depot
                let payload: Vec<String> = self
                    .ours
                    .iter()
                    .filter(|g| !present.contains(g.as_str()))
                    .cloned()
                    .collect();
                self.recording.payload = payload.clone();
                self.close_phase(ut, &craft, Phase::Return);
                self.mark(ut, Event::Undocked { payload });
                self.depot = None;
            }
            return;
        }

        match self.phase {
            Phase::Pad if !p.landed && !p.splashed => {
                self.close_phase(ut, &craft, Phase::Ascent);
                self.mark(ut, Event::Liftoff);
            }
            Phase::Ascent
                if orbit.eccentricity() < 1.0 && orbit.periapsis() - body.radius_m > floor =>
            {
                self.close_phase(ut, &craft, Phase::Transfer);
                self.mark(
                    ut,
                    Event::Orbit {
                        periapsis_m: orbit.periapsis() - body.radius_m,
                        apoapsis_m: orbit.apoapsis() - body.radius_m,
                    },
                );
            }
            Phase::Transfer | Phase::Approach => {
                // the nearest depot, and arrival in its bubble
                let mut near: Option<(f64, Entity)> = None;
                for d in u.vessels() {
                    if d == e || !is_depot(u, d) {
                        continue;
                    }
                    if let Some((q, _)) = flight_state(u, d) {
                        let dist = (q.r - p.r).length();
                        if body_of(u, d).map(|x| x.0) == Some(b) && near.is_none_or(|n| dist < n.0)
                        {
                            near = Some((dist, d));
                        }
                    }
                }
                self.depot = near.map(|n| n.1);
                if let Some(c) = self.depot.and_then(|d| u.current_craft(d)) {
                    self.near_guids = c.parts.iter().map(|q| q.guid.clone()).collect();
                }
                if self.phase == Phase::Transfer {
                    if let Some((_, d)) = near.filter(|n| n.0 < DEPOT_BUBBLE_M) {
                        let name = u
                            .world
                            .get::<ose_sim::vessels::Vessel>(d)
                            .map(|v| v.name.clone())
                            .unwrap_or_default();
                        self.recording.depot = Some(name.clone());
                        self.close_phase(ut, &craft, Phase::Approach);
                        self.mark(ut, Event::Arrived { depot: name });
                    }
                }
            }
            Phase::Return if orbit.periapsis() - body.radius_m < floor => {
                self.close_phase(ut, &craft, Phase::Descent);
                self.mark(ut, Event::Deorbit);
            }
            Phase::Descent => {
                let situation = u.situation(e);
                let omega = std::f64::consts::TAU / body.rotation_period_s;
                let ground = (p.v - DVec3::new(0.0, 0.0, omega).cross(p.r)).length();
                let down = matches!(situation, Some(Situation::Landed | Situation::Splashed))
                    || (alt < 100.0 && ground < 0.5);
                if down && ground < 1.0 {
                    let (lat, lon) = flight::lat_lon(&body, p.r, ut);
                    self.close_phase(ut, &craft, Phase::Descent);
                    self.mark(
                        ut,
                        Event::Landed {
                            lat_deg: lat,
                            lon_deg: lon,
                            splashed: situation == Some(Situation::Splashed) || p.splashed,
                        },
                    );
                    self.recording.state = State::RoundTrip;
                    return;
                }
            }
            _ => {}
        }

        // burns: engines lit in orbit or on the way home
        if matches!(
            self.phase,
            Phase::Transfer | Phase::Approach | Phase::Return
        ) {
            // a burn ends after the engines have been quiet for a few seconds, so the throttle
            // easing in and out at cut-off stays one burn
            let lit = p.throttle > 0.0 && physics;
            if lit {
                self.last_lit = ut;
                if !self.burning {
                    self.burning = true;
                    self.mark(ut, Event::BurnStart);
                }
            } else if self.burning && ut - self.last_lit > BURN_GAP_S {
                self.burning = false;
                let at = self.last_lit;
                self.mark(at, Event::BurnEnd);
            }
        }

        // trajectory
        let spacing = if physics {
            PHYSICS_SAMPLE_S
        } else {
            RAILS_SAMPLE_S
        };
        if ut - self.last_sample >= spacing - 1e-9 {
            self.last_sample = ut;
            let rel = self.depot.and_then(|d| {
                let (q, _) = flight_state(u, d)?;
                let dr = p.r - q.r;
                if dr.length() > DEPOT_BUBBLE_M {
                    return None;
                }
                let dv = p.v - q.v;
                let (x, y, z) = flight::vessel_basis(q);
                Some((
                    DVec3::new(dr.dot(x), dr.dot(y), dr.dot(z)),
                    DVec3::new(dv.dot(x), dv.dot(y), dv.dot(z)),
                ))
            });
            self.recording.trajectory.push(Sample {
                ut,
                body: body.id.clone(),
                r: p.r,
                v: p.v,
                rel,
            });
        }
    }

    fn update_delivered(&mut self, now: &BTreeMap<String, f64>) {
        self.recording.delivered = now
            .iter()
            .filter_map(|(k, v)| {
                let gained = v - self.depot_tanks_at_dock.get(k).copied().unwrap_or(0.0);
                (gained > 1e-6).then(|| (k.clone(), gained))
            })
            .collect();
    }

    /// The player ends the recording while docked: a one-way route, the vessel stays (design 3.10).
    pub fn finish_docked(&mut self, u: &mut Universe) -> Result<(), String> {
        if self.phase != Phase::Docked || !self.is_recording() {
            return Err("only a recording docked at its depot can end there".into());
        }
        let craft = self.follow(u).ok_or("the vessel is gone")?;
        let now = sum_tanks(&craft, &|g| self.depot_guids.contains(g));
        self.update_delivered(&now);
        let ut = u.ut();
        self.close_phase(ut, &craft, Phase::Docked);
        self.recording.state = State::OneWay;
        Ok(())
    }

    /// Revert to the VAB drops the recording (revert to launch restarts it: the server starts anew).
    pub fn reverted(&mut self) {
        self.drop_it("reverted");
    }
}

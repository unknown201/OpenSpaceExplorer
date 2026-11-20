//! A campaign's routes state: which vessels are depots, and the proving flights being or already recorded.
//! Design 3.1 and 3.2. Kept in the campaign save, so a quicksave, a revert and a restart restore it.
//!
//! A depot is followed by its parts, as the recorder follows a flight: a tanker docking to it, or payload
//! left behind, makes a bigger vessel that is still the depot, and a tanker undocking leaves without the
//! depot's parts. Part names repeat between crafts (the editor names them `p0`, `p1`, ...), so they are only
//! compared within the depot's own vessel, and a docking tells the depot's half by position.
use crate::recording::{Phase, Recorder, State};
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{bodies::ReferenceBody, Universe};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Depot {
    pub id: u64,
    /// The player who marked it.
    pub owner: u64,
    /// The vessel's name when marked.
    pub name: String,
    /// Its parts in the vessel carrying it (with what was left docked to it).
    pub parts: BTreeSet<String>,
    /// Each part's name when marked, and its name now: docking renames repeated names.
    #[serde(default)]
    pub names: Vec<(String, String)>,
    /// That vessel's parts last tick, in order, to tell its half of a docking.
    #[serde(default)]
    prev_parts: Vec<String>,
    /// Where it was last seen (body, position, speed, time).
    #[serde(default)]
    last_seen: Option<(String, DVec3, f64, f64)>,
    /// The vessel carrying it; found again by its parts after a load.
    #[serde(skip, default = "no_entity")]
    vessel: Entity,
}

fn no_entity() -> Entity {
    Entity::PLACEHOLDER
}

fn place(u: &Universe, e: Entity) -> Option<(String, DVec3, f64, f64)> {
    let p = u.powered(e).or_else(|| u.dormant(e))?;
    let b = u.world.get::<ReferenceBody>(e)?.0;
    let id = u.body_id(b)?.to_string();
    Some((id, p.r, p.v.length(), u.ut()))
}

impl Depot {
    /// A part's name now, from its name when the depot was marked.
    pub fn current_name(&self, original: &str) -> Option<&str> {
        self.names
            .iter()
            .find(|(o, _)| o == original)
            .map(|(_, n)| n.as_str())
    }

    /// A part's name when the depot was marked, from its name now.
    pub fn original_name(&self, now: &str) -> Option<&str> {
        self.names
            .iter()
            .find(|(_, n)| n == now)
            .map(|(o, _)| o.as_str())
    }

    /// The vessel carrying it now, if it is still there.
    pub fn vessel(&self) -> Option<Entity> {
        (self.vessel != Entity::PLACEHOLDER).then_some(self.vessel)
    }

    /// Follow the depot to the vessel carrying it, through dockings and undockings.
    fn follow(&mut self, u: &mut Universe) {
        let count = |c: &ose_data::craft::Craft| {
            c.parts
                .iter()
                .filter(|p| self.parts.contains(&p.guid))
                .count()
        };
        let mut found = u.current_craft(self.vessel).filter(|c| count(c) > 0);
        if found.is_none() {
            // docked into another vessel, or split off: the nearest vessel holding most of its parts
            let ut = u.ut();
            let mut best: Option<((usize, f64), Entity, ose_data::craft::Craft)> = None;
            for e in u.vessels() {
                let Some(c) = u.current_craft(e) else {
                    continue;
                };
                let n = count(&c);
                let Some(here) = place(u, e) else {
                    continue;
                };
                let d = match &self.last_seen {
                    Some((body, r, speed, t)) => {
                        let d = (here.1 - *r).length();
                        if here.0 != *body || d > 1_000.0 + 2.0 * speed * (ut - t).abs() {
                            continue;
                        }
                        d
                    }
                    None => 0.0,
                };
                if n > 0 && best.as_ref().is_none_or(|b| (n, -d) > b.0) {
                    best = Some(((n, -d), e, c));
                }
            }
            if let Some((_, e, c)) = best {
                self.vessel = e;
                found = Some(c);
            }
        }
        let Some(craft) = found else {
            self.vessel = Entity::PLACEHOLDER;
            return;
        };
        let guids: Vec<String> = craft.parts.iter().map(|p| p.guid.clone()).collect();
        if self.names.is_empty() {
            // marked before names were kept
            self.names = self
                .prev_parts
                .iter()
                .map(|g| (g.clone(), g.clone()))
                .collect();
        }
        let (n, m) = (self.prev_parts.len(), guids.len());
        if n > 0 && m > n {
            // docked: the vessel kept comes first, the one joined to it after
            let block = if guids[..n] == self.prev_parts[..] {
                0..n
            } else {
                m - n..m
            };
            for (_, now) in &mut self.names {
                if let Some(k) = self.prev_parts.iter().position(|g| g == now) {
                    *now = guids[block.start + k].clone();
                }
            }
            self.parts = guids[block].iter().cloned().collect();
        } else {
            let here: BTreeSet<&String> = guids.iter().collect();
            self.parts.retain(|g| here.contains(g));
            self.names.retain(|(_, now)| here.contains(now));
        }
        self.prev_parts = guids;
        self.last_seen = place(u, self.vessel);
    }
}

/// One proving flight, by the player who flies it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proving {
    pub id: u64,
    pub player: u64,
    pub recorder: Recorder,
    /// The depot it docked at (`Depot::id`).
    #[serde(default)]
    pub depot: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Routes {
    pub depots: Vec<Depot>,
    pub provings: Vec<Proving>,
    /// Routes made from finished proving flights (`route.rs`).
    #[serde(default)]
    pub routes: Vec<crate::route::Route>,
    #[serde(default)]
    next_id: u64,
}

/// What changed in a tick, for telling the players.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub player: u64,
    pub proving: u64,
}

impl Routes {
    pub(crate) fn bump_id(&mut self) -> u64 {
        self.fresh_id()
    }

    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The depot `vessel` carries, if any (as last followed).
    pub fn depot_of(&self, vessel: Entity) -> Option<&Depot> {
        self.depots.iter().find(|d| d.vessel == vessel)
    }

    /// Mark a vessel as a depot (or unmark the depot it carries). Returns whether anything changed.
    pub fn set_depot(
        &mut self,
        u: &Universe,
        vessel: Entity,
        owner: u64,
        on: bool,
    ) -> Result<bool, String> {
        let craft = u.current_craft(vessel).ok_or("no such vessel")?;
        let carried = self.depots.iter().position(|d| d.vessel == vessel);
        match (on, carried) {
            (true, Some(_)) | (false, None) => Ok(false),
            (false, Some(i)) => {
                self.depots.remove(i);
                Ok(true)
            }
            (true, None) => {
                let name = u
                    .world
                    .get::<ose_sim::vessels::Vessel>(vessel)
                    .map(|v| v.name.clone())
                    .unwrap_or_default();
                let id = self.fresh_id();
                let guids: Vec<String> = craft.parts.iter().map(|p| p.guid.clone()).collect();
                self.depots.push(Depot {
                    id,
                    owner,
                    name,
                    parts: guids.iter().cloned().collect(),
                    names: guids.iter().map(|g| (g.clone(), g.clone())).collect(),
                    prev_parts: guids,
                    last_seen: place(u, vessel),
                    vessel,
                });
                Ok(true)
            }
        }
    }

    /// The player's proving flight still being recorded.
    pub fn recording(&self, player: u64) -> Option<&Proving> {
        self.provings
            .iter()
            .find(|p| p.player == player && p.recorder.is_recording())
    }

    /// Start recording a proving flight of a vessel on the pad. A player records one flight at a time.
    pub fn start(&mut self, u: &Universe, vessel: Entity, player: u64) -> Result<u64, String> {
        if self.recording(player).is_some() {
            return Err("you are already recording a route".into());
        }
        if self.depot_of(vessel).is_some() {
            return Err("a depot cannot fly its own route".into());
        }
        let recorder = Recorder::start(u, vessel)?;
        let id = self.fresh_id();
        self.provings.push(Proving {
            id,
            player,
            recorder,
            depot: None,
        });
        Ok(id)
    }

    /// The player ends their recording: docked at the depot it is a one-way route, anywhere else it is
    /// dropped. Returns the proving flight's id.
    pub fn finish(&mut self, u: &mut Universe, player: u64) -> Result<u64, String> {
        let p = self
            .provings
            .iter_mut()
            .find(|p| p.player == player && p.recorder.is_recording())
            .ok_or("you are not recording a route")?;
        if p.recorder.phase() == Phase::Docked {
            p.recorder.finish_docked(u)?;
        } else {
            p.recorder.recording.state = State::Dropped("stopped".into());
        }
        Ok(p.id)
    }

    /// Forget a finished or dropped proving flight.
    pub fn forget(&mut self, player: u64, id: u64) -> Result<(), String> {
        let i = self
            .provings
            .iter()
            .position(|p| p.id == id && p.player == player)
            .ok_or("no such recording")?;
        if self.provings[i].recorder.is_recording() {
            return Err("stop the recording first".into());
        }
        self.provings.remove(i);
        Ok(())
    }

    /// Whether `e` carries a depot.
    pub fn is_depot_vessel(&self, e: Entity) -> bool {
        self.depot_of(e).is_some()
    }

    /// The universe was replaced (a quickload or a revert): every vessel is found again by its parts.
    pub fn universe_replaced(&mut self) {
        for d in &mut self.depots {
            d.vessel = Entity::PLACEHOLDER;
        }
        for p in &mut self.provings {
            p.recorder.universe_replaced();
        }
    }

    /// Follow each depot to the vessel carrying it (done by `tick`).
    pub fn follow_depots(&mut self, u: &mut Universe) {
        for d in &mut self.depots {
            d.follow(u);
        }
    }

    /// Every simulation step: follow each flight being recorded. Returns the flights whose phase or state
    /// changed.
    pub fn tick(&mut self, u: &mut Universe) -> Vec<Change> {
        self.follow_depots(u);
        if !self.provings.iter().any(|p| p.recorder.is_recording()) {
            return Vec::new();
        }
        let depots: BTreeSet<Entity> = self.depots.iter().filter_map(|d| d.vessel()).collect();
        let is_depot = |_: &Universe, e: Entity| depots.contains(&e);
        let mut out = Vec::new();
        for p in self
            .provings
            .iter_mut()
            .filter(|p| p.recorder.is_recording())
        {
            let before = (p.recorder.phase(), p.recorder.recording.markers.len());
            p.recorder.observe(u, &is_depot);
            let after = (p.recorder.phase(), p.recorder.recording.markers.len());
            if before.0 != Phase::Docked && after.0 == Phase::Docked {
                // which depot, and its port by the name it had when marked
                let at = p.recorder.vessel();
                if let Some(d) = self.depots.iter().find(|d| d.vessel == at) {
                    p.depot = Some(d.id);
                    let r = &mut p.recorder.recording;
                    if let Some(name) = r.depot_port.as_deref().and_then(|n| d.original_name(n)) {
                        r.depot_port = Some(name.to_string());
                    }
                }
            }
            if before != after || !p.recorder.is_recording() {
                out.push(Change {
                    player: p.player,
                    proving: p.id,
                });
            }
        }
        out
    }

    /// Resources a proving flight used, all phases together.
    pub fn used(p: &Proving) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        for ph in &p.recorder.recording.phases {
            for (k, v) in &ph.used {
                *out.entry(k.clone()).or_insert(0.0) += v;
            }
        }
        out
    }
}

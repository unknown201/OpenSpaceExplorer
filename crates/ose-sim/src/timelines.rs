//! Each player's own time: timelines (the "subspaces" of the KSP multiplayer mods).
//!
//! A universe without timelines runs one clock, as it always has. With timelines on (`enable_timelines`) every
//! vessel belongs to one timeline (`Timeline`), and each timeline has its own clock and its own time warp. A
//! step advances every timeline by its own warp, one after the other, and while a timeline steps the universe
//! shows only its vessels: its clock is the universe's time (`UniverseTime`, `WarpFactor`, `WarpFocus` hold
//! the active timeline's values), and the vessel lists and queries of the step leave out everything in other
//! timelines (`Universe::here`). Vessels in different timelines therefore never touch, dock or collide; each
//! sees the other where its orbit puts it at its own time.
//!
//! A timeline can only go forward. One that is behind can catch up with one ahead (`catch_up`): it runs at
//! `CATCH_UP_WARP` (as far as its vessels allow) until its clock reaches the other's, and then joins it, its
//! vessels moving over (`step` reports the join). A new timeline starts as a copy of the clock of the one it
//! splits from (`split`), so moving vessels into it changes nothing about them.
//!
//! Vessels made while a timeline is active (launches, debris, Kerbals going on EVA) join it: they are tagged
//! when the timeline is left or its step ends.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use ose_data::Tick;

use crate::{vessels, Universe, UniverseTime, WarpFactor, WarpFocus};

/// The timeline a vessel is in.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Timeline(pub u32);

/// How fast a timeline catching up with another runs (its vessels' own limits still apply).
pub const CATCH_UP_WARP: u32 = 1_000_000;

/// One timeline's clock.
#[derive(Clone, Debug)]
pub struct Clock {
    pub tick: Tick,
    /// The warp asked for (the vessels' limits apply when it steps).
    pub warp: u32,
    focus: Vec<Entity>,
    /// Running to catch up with this timeline, then joining it.
    pub catching_up: Option<u32>,
}

#[derive(Resource, Clone, Debug)]
pub struct Timelines {
    clocks: BTreeMap<u32, Clock>,
    active: u32,
    next: u32,
    /// Who is in which timeline (players, for the server): they move with its vessels when it joins another.
    members: BTreeMap<u64, u32>,
}

/// Timelines as a save keeps them.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedTimelines {
    /// (id, clock, warp) each.
    pub clocks: Vec<(u32, Tick, u32)>,
    pub members: Vec<(u64, u32)>,
    pub active: u32,
}

impl Universe {
    /// Give every vessel its timeline's own clock: the current clock becomes timeline 0, with every vessel in
    /// it. Does nothing when timelines are on already.
    pub fn enable_timelines(&mut self) {
        if self.timelines_enabled() {
            return;
        }
        let clock = Clock {
            tick: self.tick(),
            warp: self.warp(),
            focus: self
                .world
                .get_resource::<WarpFocus>()
                .map(|f| f.0.clone())
                .unwrap_or_default(),
            catching_up: None,
        };
        self.world.insert_resource(Timelines {
            clocks: BTreeMap::from([(0, clock)]),
            active: 0,
            next: 1,
            members: BTreeMap::new(),
        });
        self.tag_untagged();
    }

    pub fn timelines_enabled(&self) -> bool {
        self.world.get_resource::<Timelines>().is_some()
    }

    /// The timeline the universe shows now (0 without timelines).
    pub fn active_timeline(&self) -> u32 {
        self.world
            .get_resource::<Timelines>()
            .map(|t| t.active)
            .unwrap_or(0)
    }

    /// Every timeline, by id.
    pub fn timeline_ids(&self) -> Vec<u32> {
        match self.world.get_resource::<Timelines>() {
            Some(t) => t.clocks.keys().copied().collect(),
            None => vec![0],
        }
    }

    /// A timeline's clock (the active one's as it stands now).
    pub fn timeline_tick(&self, id: u32) -> Option<Tick> {
        let t = self.world.get_resource::<Timelines>()?;
        if id == t.active {
            return Some(self.tick());
        }
        t.clocks.get(&id).map(|c| c.tick)
    }

    /// A timeline's clock and warp, and the one it is catching up with.
    pub fn timeline_clock(&self, id: u32) -> Option<Clock> {
        let t = self.world.get_resource::<Timelines>()?;
        let mut c = t.clocks.get(&id)?.clone();
        if id == t.active {
            c.tick = self.tick();
            c.warp = self.warp();
        }
        Some(c)
    }

    /// The timeline vessel `e` is in (a vessel made since the active timeline was entered is in that one).
    pub fn timeline_of(&self, e: Entity) -> u32 {
        self.world
            .get::<Timeline>(e)
            .map(|t| t.0)
            .unwrap_or_else(|| self.active_timeline())
    }

    /// Whether vessel `e` is in the timeline the universe shows now (always, without timelines).
    pub(crate) fn here(&self, e: Entity) -> bool {
        match self.world.get_resource::<Timelines>() {
            None => true,
            Some(t) => self
                .world
                .get::<Timeline>(e)
                .is_none_or(|x| x.0 == t.active),
        }
    }

    /// Vessels made while the active timeline was shown join it.
    fn tag_untagged(&mut self) {
        let active = self.active_timeline();
        let untagged: Vec<Entity> = {
            let mut q = self
                .world
                .query_filtered::<Entity, (With<vessels::Vessel>, Without<Timeline>)>();
            q.iter(&self.world).collect()
        };
        for e in untagged {
            self.world.entity_mut(e).insert(Timeline(active));
        }
    }

    /// Show timeline `id`: its clock becomes the universe's time. Returns false for a timeline that does not
    /// exist (nothing changes then).
    pub fn enter(&mut self, id: u32) -> bool {
        let Some(t) = self.world.get_resource::<Timelines>() else {
            return id == 0;
        };
        if !t.clocks.contains_key(&id) {
            return false;
        }
        if t.active == id {
            return true;
        }
        self.tag_untagged();
        let (tick, warp) = (self.tick(), self.warp());
        let focus = self
            .world
            .get_resource::<WarpFocus>()
            .map(|f| f.0.clone())
            .unwrap_or_default();
        let next = {
            let mut t = self.world.resource_mut::<Timelines>();
            let active = t.active;
            if let Some(c) = t.clocks.get_mut(&active) {
                c.tick = tick;
                c.warp = warp;
                c.focus = focus;
            }
            t.active = id;
            t.clocks[&id].clone()
        };
        self.world.resource_mut::<UniverseTime>().0 = next.tick;
        self.world.resource_mut::<WarpFactor>().0 = next.warp.max(1);
        self.world.insert_resource(WarpFocus(next.focus));
        true
    }

    /// A new timeline at the active one's time (warp 1), for vessels to move into (`move_to_timeline`).
    pub fn split(&mut self) -> Option<u32> {
        self.tag_untagged();
        let tick = self.tick();
        let mut t = self.world.get_resource_mut::<Timelines>()?;
        let id = t.next;
        t.next += 1;
        t.clocks.insert(
            id,
            Clock {
                tick,
                warp: 1,
                focus: Vec::new(),
                catching_up: None,
            },
        );
        Some(id)
    }

    /// Move vessels into timeline `id`; both clocks must read the same time (a timeline just split off, or one
    /// that has caught up).
    pub fn move_to_timeline(&mut self, vessels: &[Entity], id: u32) -> bool {
        self.tag_untagged();
        let Some(to) = self.timeline_tick(id) else {
            return false;
        };
        for &e in vessels {
            if self.timeline_tick(self.timeline_of(e)) != Some(to) {
                return false;
            }
        }
        for &e in vessels {
            if let Ok(mut ent) = self.world.get_entity_mut(e) {
                ent.insert(Timeline(id));
            }
        }
        true
    }

    /// The vessels in timeline `id`.
    pub fn timeline_vessels(&mut self, id: u32) -> Vec<Entity> {
        self.tag_untagged();
        let mut q = self
            .world
            .query_filtered::<(Entity, &Timeline), With<vessels::Vessel>>();
        q.iter(&self.world)
            .filter(|(_, t)| t.0 == id)
            .map(|(e, _)| e)
            .collect()
    }

    /// Ask for a timeline's warp (applied, within its vessels' limits, when it steps).
    pub fn set_timeline_warp(&mut self, id: u32, warp: u32) -> bool {
        let back = self.active_timeline();
        if !self.enter(id) {
            return false;
        }
        self.set_warp(warp.max(1));
        self.enter(back);
        true
    }

    /// Run timeline `from` forward to catch up with timeline `to`, which must be ahead; they become one when
    /// the clocks meet (`step` reports it).
    pub fn catch_up(&mut self, from: u32, to: u32) -> Result<(), String> {
        let (Some(a), Some(b)) = (self.timeline_tick(from), self.timeline_tick(to)) else {
            return Err("no such time".into());
        };
        if from == to {
            return Err("already there".into());
        }
        if b < a {
            return Err("that time is behind yours: nothing goes back in time".into());
        }
        let mut t = self.world.resource_mut::<Timelines>();
        if let Some(c) = t.clocks.get_mut(&from) {
            c.catching_up = Some(to);
        }
        Ok(())
    }

    /// Stop catching up (the timeline keeps the time it has reached).
    pub fn stop_catch_up(&mut self, id: u32) {
        if let Some(mut t) = self.world.get_resource_mut::<Timelines>() {
            if let Some(c) = t.clocks.get_mut(&id) {
                c.catching_up = None;
            }
        }
    }

    /// Drop timeline `id` when it holds no vessels (its players having joined another). The active timeline
    /// stays.
    pub fn remove_timeline_if_empty(&mut self, id: u32) -> bool {
        if id == self.active_timeline()
            || !self.timeline_vessels(id).is_empty()
            || self.timeline_members(id).next().is_some()
        {
            return false;
        }
        self.world
            .get_resource_mut::<Timelines>()
            .is_some_and(|mut t| t.clocks.remove(&id).is_some())
    }

    /// The timelines as a save keeps them (None without timelines).
    pub fn saved_timelines(&self) -> Option<SavedTimelines> {
        let t = self.world.get_resource::<Timelines>()?;
        Some(SavedTimelines {
            clocks: t
                .clocks
                .keys()
                .filter_map(|&id| {
                    let c = self.timeline_clock(id)?;
                    Some((id, c.tick, c.warp))
                })
                .collect(),
            members: t.members.iter().map(|(&m, &id)| (m, id)).collect(),
            active: t.active,
        })
    }

    /// Restore timelines from a save, the universe's clock and warp being the active timeline's already
    /// (`set_timeline` puts each vessel back in its own).
    pub fn restore_timelines(&mut self, saved: &SavedTimelines) {
        let mut clocks: BTreeMap<u32, Clock> = saved
            .clocks
            .iter()
            .map(|&(id, tick, warp)| {
                (
                    id,
                    Clock {
                        tick,
                        warp: warp.max(1),
                        focus: Vec::new(),
                        catching_up: None,
                    },
                )
            })
            .collect();
        clocks.entry(saved.active).or_insert(Clock {
            tick: self.tick(),
            warp: self.warp(),
            focus: Vec::new(),
            catching_up: None,
        });
        let next = clocks.keys().max().map_or(0, |m| m + 1);
        self.world.insert_resource(Timelines {
            clocks,
            active: saved.active,
            next,
            members: saved.members.iter().copied().collect(),
        });
    }

    /// The timeline `member` is in (None: none yet, or without timelines).
    pub fn member_timeline(&self, member: u64) -> Option<u32> {
        let t = self.world.get_resource::<Timelines>()?;
        t.members
            .get(&member)
            .copied()
            .filter(|id| t.clocks.contains_key(id))
    }

    /// Put `member` in timeline `id`.
    pub fn set_member_timeline(&mut self, member: u64, id: u32) {
        if let Some(mut t) = self.world.get_resource_mut::<Timelines>() {
            t.members.insert(member, id);
        }
    }

    /// The members in timeline `id`.
    pub fn timeline_members(&self, id: u32) -> impl Iterator<Item = u64> + '_ {
        self.world
            .get_resource::<Timelines>()
            .into_iter()
            .flat_map(move |t| t.members.iter().filter(move |x| *x.1 == id).map(|x| *x.0))
    }

    /// The timeline furthest ahead (where a newcomer starts).
    pub fn newest_timeline(&self) -> u32 {
        self.timeline_ids()
            .into_iter()
            .max_by_key(|&id| (self.timeline_tick(id).map(|t| t.0), std::cmp::Reverse(id)))
            .unwrap_or(0)
    }

    /// Put vessel `e` in timeline `id` (restoring a save).
    pub fn set_timeline(&mut self, e: Entity, id: u32) {
        if let Ok(mut ent) = self.world.get_entity_mut(e) {
            ent.insert(Timeline(id));
        }
    }

    /// The tick a catching-up active timeline must stop at this step (its target's clock).
    pub(crate) fn catch_up_stop(&self) -> Option<Tick> {
        let t = self.world.get_resource::<Timelines>()?;
        let to = t.clocks.get(&t.active)?.catching_up?;
        t.clocks.get(&to).map(|c| c.tick)
    }

    /// Step every timeline by its own warp. Returns the timelines that caught up and joined another this step:
    /// (the one that caught up, the one it joined).
    pub(crate) fn step_timelines(&mut self) -> Vec<(u32, u32)> {
        let back = self.active_timeline();
        let mut joined = Vec::new();
        for id in self.timeline_ids() {
            if let Some(to) = self.step_timeline(id) {
                joined.push((id, to));
            }
        }
        self.finish_step(&joined, back);
        joined
    }

    /// Step timeline `id` by its own warp, leaving it shown. Returns the timeline it joined, when it was
    /// catching up and got there (its vessels are in that one now).
    pub fn step_timeline(&mut self, id: u32) -> Option<u32> {
        if !self.enter(id) {
            return None;
        }
        let catching = self.timeline_clock(id).and_then(|c| c.catching_up);
        if let Some(to) = catching {
            if self.timeline_ids().contains(&to) {
                self.set_warp(CATCH_UP_WARP);
            } else {
                self.stop_catch_up(id);
            }
        }
        self.step_here();
        self.tag_untagged();
        let to = catching?;
        let stop = self.catch_up_stop()?;
        if self.tick() < stop || self.timeline_tick(to) != Some(stop) {
            return None;
        }
        for e in self.timeline_vessels(id) {
            self.world.entity_mut(e).insert(Timeline(to));
        }
        if let Some(mut t) = self.world.get_resource_mut::<Timelines>() {
            for m in t.members.values_mut() {
                if *m == id {
                    *m = to;
                }
            }
        }
        self.stop_catch_up(id);
        self.set_warp(1);
        Some(to)
    }

    /// After stepping every timeline: those emptied by a join (`(from, to)` each) go, and `back` is shown
    /// again (or the one it joined).
    pub fn finish_step(&mut self, joined: &[(u32, u32)], back: u32) {
        let mut back = back;
        for &(from, to) in joined {
            if back == from {
                back = to;
            }
            if self.active_timeline() == from {
                self.enter(to);
            }
            self.remove_timeline_if_empty(from);
        }
        self.enter(back);
    }

    /// Bring vessel `e` from a timeline behind into timeline `id`: only a vessel on rails (its orbit says
    /// where it is at any time), never one flying under physics.
    pub fn bring_forward(&mut self, e: Entity, id: u32) -> Result<(), String> {
        self.tag_untagged();
        let from = self.timeline_of(e);
        if from == id {
            return Ok(());
        }
        let (Some(a), Some(b)) = (self.timeline_tick(from), self.timeline_tick(id)) else {
            return Err("no such time".into());
        };
        if a > b {
            return Err("it is ahead of you in time".into());
        }
        if self.world.get::<crate::flight::Powered>(e).is_some() {
            return Err("it is flying in another time".into());
        }
        self.world.entity_mut(e).insert(Timeline(id));
        Ok(())
    }

    /// Where vessel `e` is seen from the timeline shown now, relative to its body: its own state when it is in
    /// this timeline; from another, where its orbit puts it at this time, or, for a craft at rest on the
    /// ground or the sea, where its body's turning carries it.
    pub fn seen_state(
        &self,
        e: Entity,
        orbit: &ose_math::Orbit,
    ) -> (ose_math::DVec3, ose_math::DVec3) {
        let ut = self.ut();
        if !self.here(e) {
            if let Some(p) = self
                .world
                .get::<crate::flight::Powered>(e)
                .filter(|p| crate::flight::resting(p) || p.crashed)
            {
                let then = self
                    .timeline_tick(self.timeline_of(e))
                    .map(|t| t.seconds())
                    .unwrap_or(ut);
                let period = self
                    .world
                    .get::<crate::bodies::ReferenceBody>(e)
                    .and_then(|b| self.world.get::<crate::bodies::Body>(b.0))
                    .map(|b| b.rotation_period_s)
                    .unwrap_or(0.0);
                let omega = if period > 0.0 {
                    std::f64::consts::TAU / period
                } else {
                    0.0
                };
                let (s, c) = (omega * (ut - then)).sin_cos();
                let turn = |v: ose_math::DVec3| {
                    ose_math::DVec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z)
                };
                return (turn(p.r), turn(p.v));
            }
        }
        orbit.state_at(ut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kerbin() -> Universe {
        Universe::with_bodies(&[crate::tests::kerbin_def()])
    }

    #[test]
    fn each_timeline_keeps_its_own_clock_and_warp() {
        let mut u = kerbin();
        let body = u.body_entity("Kerbin").unwrap();
        let a = u.spawn_vessel_in_circular_orbit("a", body, 100_000.0);
        let b = u.spawn_vessel_in_circular_orbit("b", body, 100_000.0);
        u.enable_timelines();
        let t = u.split().unwrap();
        assert!(u.move_to_timeline(&[b], t));
        assert!(u.set_timeline_warp(t, 1000));
        let start = u.tick().0;
        for _ in 0..10 {
            u.step();
        }
        assert_eq!(u.active_timeline(), 0, "the shown timeline stays");
        assert_eq!(u.tick().0, start + 10, "timeline 0 runs at 1x");
        assert_eq!(u.timeline_tick(t).unwrap().0, start + 10_000);
        assert_eq!(u.vessels_here(), vec![a]);
        assert!(u.enter(t));
        assert_eq!(u.vessels_here(), vec![b]);
        assert_eq!(u.warp(), 1000);
        // a vessel made while a timeline is shown is in it
        let c = u.spawn_vessel_in_circular_orbit("c", body, 200_000.0);
        assert!(u.enter(0));
        assert_eq!(u.timeline_of(c), t);
        assert!(!u.vessels_here().contains(&c));
        // the far timeline's vessels sit where their orbit puts them at the time shown
        let (_, _, orbit) = u.vessel(b).unwrap();
        let (r, _) = orbit.state_at(u.ut());
        assert!((u.position_of(b).unwrap() - r).length() < 1.0);
        // only forward
        assert!(u.catch_up(t, 0).is_err());
    }

    #[test]
    fn a_timeline_behind_catches_up_and_joins() {
        let mut u = kerbin();
        let body = u.body_entity("Kerbin").unwrap();
        let a = u.spawn_vessel_in_circular_orbit("a", body, 100_000.0);
        let b = u.spawn_vessel_in_circular_orbit("b", body, 100_000.0);
        u.enable_timelines();
        let ahead = u.split().unwrap();
        assert!(u.move_to_timeline(&[b], ahead));
        u.set_timeline_warp(ahead, 100);
        for _ in 0..50 {
            u.step();
        }
        u.set_timeline_warp(ahead, 1);
        let target = u.timeline_tick(ahead).unwrap().0;
        assert!(target > u.tick().0 + 4000);
        u.catch_up(0, ahead).unwrap();
        let mut joined = Vec::new();
        for _ in 0..100 {
            joined.extend(u.step_timelines());
            if !joined.is_empty() {
                break;
            }
        }
        assert_eq!(joined, vec![(0, ahead)]);
        assert_eq!(u.timeline_ids(), vec![ahead], "the emptied timeline goes");
        assert_eq!(u.active_timeline(), ahead);
        assert_eq!(u.timeline_of(a), ahead);
        let now = u.tick().0;
        assert!(now >= target && now <= target + 1, "{now} vs {target}");
        assert_eq!(u.warp(), 1);
        let mut here = u.vessels_here();
        here.sort();
        let mut both = vec![a, b];
        both.sort();
        assert_eq!(here, both);
    }
}

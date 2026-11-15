//! Independent player clocks. Bodies are evaluated analytically at each timeline's time.
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use crate::{Tick, Universe};

#[derive(Component, Clone, Copy, Debug)]
pub struct Timeline(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineClock {
    pub tick: Tick,
    pub warp: u32,
}

impl Universe {
    pub fn timeline_of(&self, e: Entity) -> u64 {
        self.world.get::<Timeline>(e).map_or(0, |t| t.0)
    }

    pub fn clock(&self, id: u64) -> TimelineClock {
        self.timelines.get(&id).copied().unwrap_or(TimelineClock { tick: self.tick(), warp: self.warp() })
    }

    pub fn assign_timeline(&mut self, e: Entity, id: u64) {
        let clock = self.clock(id);
        self.timelines.entry(id).or_insert(clock);
        let changed = self.world.get::<Timeline>(e).is_none_or(|t| t.0 != id);
        self.world.entity_mut(e).insert(Timeline(id));
        if changed && self.powered(e).is_none() {
            let index = self.world.resource::<crate::bodies::BodyIndex>().clone();
            let ut = self.vessel_tick(e).seconds();
            crate::vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
        }
    }

    pub fn vessel_tick(&self, e: Entity) -> Tick {
        let id = self.timeline_of(e);
        if self.active_timeline == Some(id) { self.tick() } else { self.clock(id).tick }
    }

    pub fn same_time(&self, a: Entity, b: Entity) -> bool {
        self.vessel_tick(a) == self.vessel_tick(b)
            && self.clock(self.timeline_of(a)).warp == self.clock(self.timeline_of(b)).warp
    }

    pub(crate) fn in_active_timeline(&self, e: Entity) -> bool {
        self.active_timeline.is_none_or(|id| self.timeline_of(e) == id)
    }

    pub fn set_timeline_warp(&mut self, id: u64, factor: u32) {
        let base = (self.tick(), self.warp());
        let clock = self.clock(id);
        self.set_tick(clock.tick);
        self.active_timeline = Some(id);
        self.set_warp(factor);
        self.timelines.insert(id, TimelineClock { tick: clock.tick, warp: self.warp() });
        self.active_timeline = None;
        self.set_tick(base.0);
        self.world.resource_mut::<crate::WarpFactor>().0 = base.1;
    }

    pub(crate) fn step_timelines(&mut self) {
        if self.timelines.is_empty() {
            self.step_active();
            return;
        }
        let base = (self.tick(), self.warp());
        let clocks: Vec<_> = self.timelines.iter().map(|(&id, &c)| (id, c)).collect();
        // Untagged scenery/probes remain in the shared timeline.
        self.active_timeline = Some(0);
        self.step_active();
        let shared = (self.tick(), self.warp());
        for (id, c) in clocks {
            if id == 0 { continue; }
            self.active_timeline = Some(id);
            self.set_tick(c.tick);
            self.world.resource_mut::<crate::WarpFactor>().0 = c.warp;
            self.step_active();
            self.timelines.insert(id, TimelineClock { tick: self.tick(), warp: self.warp() });
        }
        self.active_timeline = None;
        self.set_tick(shared.0);
        self.world.resource_mut::<crate::WarpFactor>().0 = shared.1;
        if self.timelines.contains_key(&0) {
            self.timelines.insert(0, TimelineClock { tick: shared.0, warp: base.1 });
        }
    }
}

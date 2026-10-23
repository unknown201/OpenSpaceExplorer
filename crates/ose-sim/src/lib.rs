//! Deterministic simulation: one crate, run natively by the server and in WASM by the client.
//!
//! State lives in a `bevy_ecs` `World`. Time is an integer tick count (`Tick`), `TICKS_PER_SECOND` ticks per
//! second of universal time. No I/O, no wall clock, no randomness without an explicit seed.

#![forbid(unsafe_code)]

pub mod bodies;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

pub const TICKS_PER_SECOND: u64 = 50;

/// Universal time in ticks since the start of the save.
#[derive(
    Resource, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Tick(pub u64);

impl Tick {
    pub fn seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }
}

/// Time warp factor granted by the server (1 = real time).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarpFactor(pub u32);

impl Default for WarpFactor {
    fn default() -> Self {
        WarpFactor(1)
    }
}

/// The whole universe.
pub struct Universe {
    pub world: World,
    schedule: Schedule,
}

impl Default for Universe {
    fn default() -> Self {
        Self::new()
    }
}

impl Universe {
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(Tick::default());
        world.insert_resource(WarpFactor::default());
        let mut schedule = Schedule::default();
        schedule.add_systems(advance_tick);
        Self { world, schedule }
    }

    /// Advance by one simulation step (which covers `WarpFactor` ticks of universal time).
    pub fn step(&mut self) {
        self.schedule.run(&mut self.world);
    }

    pub fn tick(&self) -> Tick {
        *self.world.resource::<Tick>()
    }
}

fn advance_tick(mut tick: ResMut<Tick>, warp: Res<WarpFactor>) {
    tick.0 += warp.0 as u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_advances_by_warp_factor() {
        let mut u = Universe::new();
        u.step();
        assert_eq!(u.tick(), Tick(1));
        u.world.insert_resource(WarpFactor(100));
        u.step();
        assert_eq!(u.tick(), Tick(101));
        assert!((u.tick().seconds() - 2.02).abs() < 1e-12);
    }
}

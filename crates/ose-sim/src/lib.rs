//! Deterministic simulation: one crate, run natively by the server and in WASM by the client.
//!
//! State lives in a `bevy_ecs` `World`. Time is an integer tick count (`Tick`), `TICKS_PER_SECOND` ticks per
//! second of universal time. No I/O, no wall clock, no randomness without an explicit seed.

#![forbid(unsafe_code)]

pub mod bodies;
pub mod trajectory;
pub mod vessels;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

pub use ose_data::{Tick, TICKS_PER_SECOND};

/// ECS resource wrapper around the shared [`Tick`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UniverseTime(pub Tick);

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
        world.insert_resource(UniverseTime::default());
        world.insert_resource(WarpFactor::default());
        world.insert_resource(bodies::BodyIndex::default());
        let mut schedule = Schedule::default();
        schedule.add_systems(advance_tick);
        Self { world, schedule }
    }

    /// A universe with the given celestial bodies spawned.
    pub fn with_bodies(defs: &[ose_data::CelestialBody]) -> Self {
        let mut u = Self::new();
        bodies::spawn_bodies(&mut u.world, defs);
        u
    }

    pub fn warp(&self) -> u32 {
        self.world.resource::<WarpFactor>().0
    }

    pub fn set_warp(&mut self, factor: u32) {
        self.world.resource_mut::<WarpFactor>().0 = factor.max(1);
    }

    /// Star-space position and velocity of the body with `id` at universal time `ut` (seconds).
    pub fn body_state(&self, id: &str, ut: f64) -> Option<(ose_math::DVec3, ose_math::DVec3)> {
        let e = *self.world.resource::<bodies::BodyIndex>().by_id.get(id)?;
        Some(bodies::star_space_state(&self.world, e, ut))
    }

    /// Advance by one simulation step (which covers `WarpFactor` ticks of universal time).
    pub fn step(&mut self) {
        self.schedule.run(&mut self.world);
    }

    pub fn tick(&self) -> Tick {
        self.world.resource::<UniverseTime>().0
    }
}

fn advance_tick(mut time: ResMut<UniverseTime>, warp: Res<WarpFactor>) {
    time.0 .0 += warp.0 as u64;
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

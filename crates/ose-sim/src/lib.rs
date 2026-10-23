//! Deterministic simulation, run natively by the server (ADR-0002).
//!
//! State lives in a `bevy_ecs` `World`. Time is an integer tick count (`Tick`), `TICKS_PER_SECOND` ticks per
//! second of universal time. No I/O, no wall clock, no randomness without an explicit seed.

#![forbid(unsafe_code)]

pub mod bodies;
pub mod trajectory;
pub mod vessels;

use bevy_ecs::prelude::*;
use ose_math::{DVec3, Orbit};
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
    params: trajectory::SolverParams,
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
        Self {
            world,
            params: trajectory::SolverParams::default(),
        }
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

    pub fn tick(&self) -> Tick {
        self.world.resource::<UniverseTime>().0
    }

    pub fn ut(&self) -> f64 {
        self.tick().seconds()
    }

    /// Set universal time (used when restoring a save).
    pub fn set_tick(&mut self, tick: Tick) {
        self.world.resource_mut::<UniverseTime>().0 = tick;
    }

    /// Spawn a vessel on rails from orbital elements around `body`.
    pub fn spawn_vessel_with_elements(
        &mut self,
        name: &str,
        body: Entity,
        elements: ose_math::Elements,
    ) -> Entity {
        let mu = self.world.get::<bodies::Body>(body).expect("body").mu;
        let orbit = Orbit::from_elements(mu, elements);
        let e = self
            .world
            .spawn((
                vessels::Vessel { name: name.into() },
                bodies::ReferenceBody(body),
                bodies::OnRails(orbit),
            ))
            .id();
        let ut = self.ut();
        let index = self.world.resource::<bodies::BodyIndex>().clone();
        vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
        e
    }

    pub fn body_entity(&self, id: &str) -> Option<Entity> {
        self.world
            .resource::<bodies::BodyIndex>()
            .by_id
            .get(id)
            .copied()
    }

    pub fn body_id(&self, e: Entity) -> Option<&str> {
        self.world.get::<bodies::Body>(e).map(|b| b.id.as_str())
    }

    /// Star-space position and velocity of the body with `id` at universal time `ut` (seconds).
    pub fn body_state(&self, id: &str, ut: f64) -> Option<(DVec3, DVec3)> {
        let e = self.body_entity(id)?;
        Some(bodies::star_space_state(&self.world, e, ut))
    }

    /// Spawn a vessel on rails around `body` from a relative state at the current time.
    pub fn spawn_vessel(&mut self, name: &str, body: Entity, r: DVec3, v: DVec3) -> Entity {
        let ut = self.ut();
        let e = vessels::spawn_vessel_on_rails(&mut self.world, name, body, r, v, ut);
        let index = self.world.resource::<bodies::BodyIndex>().clone();
        vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
        e
    }

    /// Spawn a vessel in a circular equatorial orbit `altitude_m` above `body`.
    pub fn spawn_vessel_in_circular_orbit(
        &mut self,
        name: &str,
        body: Entity,
        altitude_m: f64,
    ) -> Entity {
        let b = self.world.get::<bodies::Body>(body).expect("body");
        let r = b.radius_m + altitude_m;
        let v = (b.mu / r).sqrt();
        self.spawn_vessel(name, body, DVec3::new(r, 0.0, 0.0), DVec3::new(0.0, v, 0.0))
    }

    pub fn vessels(&mut self) -> Vec<Entity> {
        let mut q = self.world.query_filtered::<Entity, With<vessels::Vessel>>();
        q.iter(&self.world).collect()
    }

    pub fn vessel(&self, e: Entity) -> Option<(&vessels::Vessel, Entity, Orbit)> {
        Some((
            self.world.get::<vessels::Vessel>(e)?,
            self.world.get::<bodies::ReferenceBody>(e)?.0,
            self.world.get::<bodies::OnRails>(e)?.0,
        ))
    }

    /// Schedule an impulsive burn for a vessel at universal time `ut` (seconds, must be in the future).
    pub fn schedule_burn(&mut self, e: Entity, burn: vessels::ScheduledBurn) -> bool {
        if burn.ut < self.ut() || self.vessel(e).is_none() {
            return false;
        }
        self.world.entity_mut(e).insert(burn);
        true
    }

    pub fn scheduled_burn(&self, e: Entity) -> Option<vessels::ScheduledBurn> {
        self.world.get::<vessels::ScheduledBurn>(e).copied()
    }

    /// Predicted trajectory of a vessel from the current time. With a scheduled burn, the patches up to the
    /// burn end with `Transition::Maneuver` and the prediction continues on the post-burn orbit.
    pub fn predict(&self, e: Entity) -> Vec<trajectory::Patch> {
        let Some((_, body, orbit)) = self.vessel(e) else {
            return Vec::new();
        };
        let index = self.world.resource::<bodies::BodyIndex>();
        let ut = self.ut();
        let Some(burn) = self.scheduled_burn(e) else {
            return trajectory::predict(&self.world, index, body, orbit, ut, &self.params);
        };
        let mut patches = trajectory::predict(&self.world, index, body, orbit, ut, &self.params);
        // keep the patches before the burn; the one containing it is cut at the burn time
        let mut kept = Vec::new();
        let mut burn_body = body;
        let mut burn_orbit = orbit;
        let mut cut_done = false;
        for p in patches.drain(..) {
            let repeats = p.end == trajectory::Transition::Final && p.orbit.eccentricity() < 1.0;
            if p.start_ut <= burn.ut && (burn.ut < p.end_ut || repeats) {
                let mut cut = p;
                cut.end_ut = burn.ut;
                cut.end = trajectory::Transition::Maneuver;
                burn_body = p.body;
                burn_orbit = p.orbit;
                kept.push(cut);
                cut_done = true;
                break;
            }
            kept.push(p);
        }
        if !cut_done {
            // the burn lies beyond the predicted horizon (e.g. after an impact); nothing to add
            return kept;
        }
        let after = vessels::orbit_after_burn(&burn_orbit, &burn);
        let mut rest =
            trajectory::predict(&self.world, index, burn_body, after, burn.ut, &self.params);
        if let Some(first) = rest.first_mut() {
            first.start = trajectory::Transition::Maneuver;
        }
        kept.extend(rest);
        kept
    }

    /// Advance by one simulation step covering up to `WarpFactor` ticks. The step is shortened so that no
    /// vessel skips over its next SOI transition (the original's timescale override at SOI crossings); the
    /// crossing then lands on the step boundary and the handoff happens exactly there.
    pub fn step(&mut self) {
        let warp = self.warp() as u64;
        let now = self.tick().0;
        let mut ticks = warp;
        for e in self.vessels() {
            let mut stops = Vec::new();
            if let Some(nt) = self.world.get::<vessels::NextTransition>(e) {
                stops.push(nt.ut);
            }
            if let Some(b) = self.world.get::<vessels::ScheduledBurn>(e) {
                stops.push(b.ut);
            }
            for stop in stops {
                let at_tick = (stop * TICKS_PER_SECOND as f64).ceil().max(0.0) as u64;
                if at_tick > now && at_tick - now < ticks {
                    ticks = at_tick - now;
                }
            }
        }
        self.world.resource_mut::<UniverseTime>().0 = Tick(now + ticks.max(1));
        let ut = self.ut();
        let index = self.world.resource::<bodies::BodyIndex>().clone();
        for e in self.vessels() {
            let burned = vessels::apply_due_burn(&mut self.world, e, ut);
            let t = vessels::update_reference_body(&mut self.world, &index, e, ut);
            let due = self
                .world
                .get::<vessels::NextTransition>(e)
                .is_some_and(|n| n.ut <= ut);
            if burned || t != vessels::SoiTransition::None || due {
                vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
            }
        }
    }
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

    #[test]
    fn scheduled_burn_raises_apoapsis_and_lands_on_the_tick() {
        use ose_data::{CelestialBody as BodyDef, BIG_G, G0};
        let mu = 1.00034160493135 * G0 * 600_000.0 * 600_000.0;
        let kerbin = BodyDef {
            id: "Kerbin".into(),
            display_name: "Kerbin".into(),
            parent: None,
            is_star: false,
            is_home_world: true,
            radius_m: 600_000.0,
            gravity_asl: 1.00034160493135,
            mu,
            mass_kg: mu / BIG_G,
            soi_radius_m: None,
            rotation_period_s: 21_549.425,
            initial_rotation_deg: 0.0,
            is_tidally_locked: false,
            has_solid_surface: true,
            ocean: None,
            orbit: None,
            atmosphere: None,
            star_luminosity: None,
        };
        let mut u = Universe::with_bodies(&[kerbin]);
        let body = u.body_entity("Kerbin").unwrap();
        let v = u.spawn_vessel_in_circular_orbit("probe", body, 100_000.0);
        let burn = vessels::ScheduledBurn {
            ut: 1000.0,
            prograde_mps: 841.6,
            normal_mps: 0.0,
            radial_mps: 0.0,
        };
        assert!(u.schedule_burn(v, burn));
        // prediction shows the maneuver and the raised apoapsis
        let p = u.predict(v);
        assert_eq!(p[0].end, trajectory::Transition::Maneuver);
        assert!((p[0].end_ut - 1000.0).abs() < 1e-9);
        let ap = p[1].orbit.apoapsis();
        assert!((ap - 12_000_000.0).abs() < 100_000.0, "apoapsis {ap}");
        // warp 100000 would skip the burn; the step stops exactly on it
        u.set_warp(100_000);
        u.step();
        assert_eq!(u.tick(), Tick(50_000));
        assert!(u.scheduled_burn(v).is_none(), "burn applied");
        let (_, _, orbit) = u.vessel(v).unwrap();
        assert!((orbit.apoapsis() - 12_000_000.0).abs() < 100_000.0);
        u.step();
        assert_eq!(u.tick(), Tick(150_000));
    }
}

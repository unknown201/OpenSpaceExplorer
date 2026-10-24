//! Deterministic simulation, run natively by the server (ADR-0002).
//!
//! State lives in a `bevy_ecs` `World`. Time is an integer tick count (`Tick`), `TICKS_PER_SECOND` ticks per
//! second of universal time. No I/O, no wall clock, no randomness without an explicit seed.

#![forbid(unsafe_code)]

pub mod aero;
pub mod bodies;
pub mod flight;
pub mod missions;
pub mod resources;
pub mod rigid;
pub mod science;
pub mod structure;
pub mod trajectory;
pub mod vessels;

use bevy_ecs::prelude::*;
use ose_math::{DVec3, Orbit};
use serde::{Deserialize, Serialize};

pub use ose_data::{Tick, TICKS_PER_SECOND};

/// ECS resource wrapper around the shared [`Tick`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UniverseTime(pub Tick);

/// Distance of the home world from the star, m: solar panels are rated there.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct HomeDistance(pub f64);

/// Time warp factor granted by the server (1 = real time).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarpFactor(pub u32);

impl Default for WarpFactor {
    fn default() -> Self {
        WarpFactor(1)
    }
}

/// What `Universe::stage` did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageResult {
    pub fired: bool,
    /// The separated stage, now its own vessel.
    pub debris: Option<Entity>,
}

/// The whole universe.
pub struct Universe {
    pub world: World,
    params: trajectory::SolverParams,
    /// Pieces that broke off powered vessels since the last `take_broken_off`: (vessel, new debris).
    broken_off: Vec<(Entity, Entity)>,
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
            broken_off: Vec::new(),
        }
    }

    /// A universe with the given celestial bodies spawned.
    pub fn with_bodies(defs: &[ose_data::CelestialBody]) -> Self {
        let mut u = Self::new();
        bodies::spawn_bodies(&mut u.world, defs);
        if let Some(a) = defs
            .iter()
            .find(|d| d.is_home_world)
            .and_then(|d| d.orbit.as_ref())
            .map(|o| o.semi_major_axis_m)
        {
            u.world.insert_resource(HomeDistance(a));
        }
        u
    }

    /// Starlight at a point `r` from `body` (body-centred inertial) relative to the home world's: inverse
    /// square from the star, 0 when `body` (or the planet it orbits) is in the way.
    pub fn sunlight(&self, body: Entity, r: DVec3, ut: f64) -> f64 {
        let Some(root) = self.world.resource::<bodies::BodyIndex>().root else {
            return 1.0;
        };
        let p = bodies::star_space_state(&self.world, body, ut).0 + r;
        let d = p.length();
        if d <= 0.0 || body == root && r.length() <= 0.0 {
            return 0.0;
        }
        let mut blockers = vec![body];
        if let Some(parent) = self.world.get::<bodies::ReferenceBody>(body).map(|b| b.0) {
            if parent != root {
                blockers.push(parent);
            }
        }
        for b in blockers.into_iter().filter(|&b| b != root) {
            let Some(radius) = self.world.get::<bodies::Body>(b).map(|x| x.radius_m) else {
                continue;
            };
            let c = bodies::star_space_state(&self.world, b, ut).0;
            // closest point of the segment from the vessel to the star (at the origin)
            let s = (c.dot(p) / (d * d)).clamp(0.0, 1.0);
            if s < 1.0 && (c - p * s).length() < radius {
                return 0.0;
            }
        }
        match self.world.get_resource::<HomeDistance>() {
            Some(h) => (h.0 / d).powi(2),
            None => 1.0,
        }
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

    /// Spawn a powered vessel standing on the surface of `body` at the given latitude/longitude.
    pub fn spawn_powered_on_surface(
        &mut self,
        name: &str,
        body: Entity,
        spec: flight::FlightSpec,
        lat_deg: f64,
        lon_deg: f64,
    ) -> Entity {
        let ut = self.ut();
        let b = self.world.get::<bodies::Body>(body).expect("body").clone();
        let ground = self
            .world
            .get::<flight::Terrain>(body)
            .map(|t| t.0.ground_m(lat_deg, lon_deg))
            .unwrap_or(0.0);
        let (r, v) = flight::surface_state_at(&b, lat_deg, lon_deg, ground, ut);
        let orbit = Orbit::from_state_vectors(b.mu, r, v, ut);
        let powered = flight::Powered::new(spec, r, v, true);
        self.world
            .spawn((
                vessels::Vessel { name: name.into() },
                bodies::ReferenceBody(body),
                bodies::OnRails(orbit),
                powered,
            ))
            .id()
    }

    /// Attach a terrain height map to the body with `id`. False when the body is unknown.
    pub fn set_terrain(&mut self, id: &str, map: ose_data::terrain::TerrainMap) -> bool {
        let Some(e) = self.body_entity(id) else {
            return false;
        };
        self.world
            .entity_mut(e)
            .insert(flight::Terrain(std::sync::Arc::new(map)));
        true
    }

    /// Ground height (terrain or sea) above the nominal radius at a body-fixed latitude/longitude.
    pub fn ground_height(&self, body: Entity, lat_deg: f64, lon_deg: f64) -> f64 {
        self.world
            .get::<flight::Terrain>(body)
            .map(|t| t.0.ground_m(lat_deg, lon_deg))
            .unwrap_or(0.0)
    }

    /// Attach a saved powered-flight state to an existing vessel (restore).
    pub fn set_powered(&mut self, e: Entity, p: flight::Powered) {
        self.world.entity_mut(e).insert(p);
    }

    pub fn powered(&self, e: Entity) -> Option<&flight::Powered> {
        self.world.get::<flight::Powered>(e)
    }

    /// Whether any vessel is flying under power (time warp is then capped at physics warp). Vessels standing
    /// on the ground with the throttle at zero, and wrecks, do not count.
    pub fn has_powered(&mut self) -> bool {
        let mut q = self.world.query::<&flight::Powered>();
        q.iter(&self.world)
            .any(|p| !p.crashed && (!p.landed || p.throttle > 0.0))
    }

    /// Give a vessel a part structure so flight loads its joints and pieces can break off.
    pub fn set_structure(&mut self, e: Entity, mut s: structure::Structure) {
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            s.sync(&mut p);
        } else if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
            s.sync(&mut d.0);
        }
        self.world.entity_mut(e).insert(s);
    }

    pub fn structure(&self, e: Entity) -> Option<&structure::Structure> {
        self.world.get::<structure::Structure>(e)
    }

    /// Debris created by structural failure since the last call, as (vessel it came from, debris).
    pub fn take_broken_off(&mut self) -> Vec<(Entity, Entity)> {
        std::mem::take(&mut self.broken_off)
    }

    /// Keep a flight state for a vessel on rails so it can be woken later (crafts launched straight into
    /// orbit, restored saves).
    pub fn set_dormant(&mut self, e: Entity, p: flight::Powered) {
        self.world.entity_mut(e).insert(flight::Dormant(p));
    }

    pub fn dormant(&self, e: Entity) -> Option<&flight::Powered> {
        self.world.get::<flight::Dormant>(e).map(|d| &d.0)
    }

    /// Put a dormant vessel back under powered flight from its current rails state. Returns false when the
    /// vessel has no flight state (a bare probe) or is already powered.
    pub fn wake(&mut self, e: Entity) -> bool {
        if self.world.get::<flight::Powered>(e).is_some() {
            return true;
        }
        let Some(mut p) = self.world.get::<flight::Dormant>(e).map(|d| d.0.clone()) else {
            return false;
        };
        let Some(orbit) = self.world.get::<bodies::OnRails>(e).map(|o| o.0) else {
            return false;
        };
        let (r, v) = orbit.state_at(self.ut());
        p.r = r;
        p.v = v;
        p.landed = false;
        p.coasting_since = None;
        let mut ent = self.world.entity_mut(e);
        ent.remove::<flight::Dormant>();
        ent.remove::<vessels::NextTransition>();
        ent.insert(p);
        true
    }

    /// Throttle (0..1) and attitude for a vessel. A vessel on rails with a dormant flight state is woken
    /// when the throttle is above zero. False for a crashed vessel or one with no flight state.
    pub fn set_controls(&mut self, e: Entity, throttle: f64, attitude: flight::Attitude) -> bool {
        let throttle = throttle.clamp(0.0, 1.0);
        if self.world.get::<flight::Powered>(e).is_none() {
            if self.world.get::<flight::Dormant>(e).is_none() {
                return false;
            }
            if throttle <= 0.0 {
                // nothing to do on rails; remember the attitude for the next wake
                if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
                    d.0.attitude = attitude;
                }
                return true;
            }
            self.wake(e);
        }
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                if p.crashed || !p.controlled {
                    return false;
                }
                p.throttle = throttle;
                p.attitude = attitude;
                true
            }
            None => false,
        }
    }

    /// Fire the next stage of a vessel (powered or dormant on rails). Separated structure becomes a debris
    /// vessel that keeps the vessel's position and velocity, pushed 3 m back along the thrust axis at 1.5 m/s.
    pub fn stage(&mut self, e: Entity) -> StageResult {
        let ut = self.ut();
        let (outcome, r, v, facing, area) =
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                let o = p.stage_next();
                (o, p.r, p.v, p.facing, p.spec.drag_area_m2)
            } else if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
                let o = d.0.stage_next();
                let (facing, area) = (d.0.facing, d.0.spec.drag_area_m2);
                let Some(orbit) = self.world.get::<bodies::OnRails>(e).map(|o| o.0) else {
                    return StageResult {
                        fired: o.fired,
                        debris: None,
                    };
                };
                let (r, v) = orbit.state_at(ut);
                (o, r, v, facing, area)
            } else {
                return StageResult {
                    fired: false,
                    debris: None,
                };
            };
        let mut outcome = outcome;
        if outcome.fired {
            let mut st = self.world.entity_mut(e).take::<structure::Structure>();
            if let Some(st) = &mut st {
                let before = st.net.mass_t();
                if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                    st.sync(&mut p);
                } else if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
                    st.sync(&mut d.0);
                }
                outcome.dropped_t = (before - st.net.mass_t()).max(0.0);
            }
            if let Some(st) = st {
                self.world.entity_mut(e).insert(st);
            }
        }
        let debris = if outcome.fired && outcome.dropped_t > 0.01 {
            self.spawn_debris(
                e,
                outcome.dropped_t,
                r - facing * 3.0,
                v - facing * 1.5,
                facing,
                area,
            )
        } else {
            None
        };
        StageResult {
            fired: outcome.fired,
            debris,
        }
    }

    fn spawn_debris(
        &mut self,
        parent: Entity,
        mass_t: f64,
        r: DVec3,
        v: DVec3,
        facing: DVec3,
        drag_area_m2: f64,
    ) -> Option<Entity> {
        let body = self.world.get::<bodies::ReferenceBody>(parent)?.0;
        let mu = self.world.get::<bodies::Body>(body)?.mu;
        let name = format!(
            "{} debris",
            self.world
                .get::<vessels::Vessel>(parent)
                .map(|v| v.name.as_str())
                .unwrap_or("vessel")
        );
        let ut = self.ut();
        let (r2, v2) = (r, v);
        let spec = flight::FlightSpec {
            stages: vec![flight::StageSpec {
                start_mass_t: mass_t,
                ..Default::default()
            }],
            drag_area_m2,
            drag_cd: flight::DEFAULT_CD,
        };
        let mut p = flight::Powered::new(spec, r2, v2, false);
        p.facing = facing;
        let orbit = Orbit::from_state_vectors(mu, r2, v2, ut);
        Some(
            self.world
                .spawn((
                    vessels::Vessel { name },
                    bodies::ReferenceBody(body),
                    bodies::OnRails(orbit),
                    p,
                    vessels::Debris,
                ))
                .id(),
        )
    }

    /// Atmosphere of a body, if any.
    pub fn atmosphere(&self, body: Entity) -> Option<&flight::Atmo> {
        self.world.get::<flight::Atmosphere>(body).map(|a| &a.0)
    }

    pub fn situation(&self, e: Entity) -> Option<flight::Situation> {
        let p = self.powered(e)?;
        let body_e = self.world.get::<bodies::ReferenceBody>(e)?.0;
        let body = self.world.get::<bodies::Body>(body_e)?;
        Some(flight::situation(p, body, self.atmosphere(body_e)))
    }

    /// Remove a vessel entity (no-op for unknown entities).
    pub fn despawn_vessel(&mut self, e: Entity) {
        if self.world.get::<vessels::Vessel>(e).is_some() {
            self.world.despawn(e);
        }
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
        let mut warp = self.warp() as u64;
        if self.has_powered() {
            warp = warp.min(flight::MAX_PHYSICS_WARP as u64);
        }
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
        // powered flight: integrate every tick of this step
        let dt = 1.0 / TICKS_PER_SECOND as f64;
        let start_ut = now as f64 * dt;
        let mut powered_vessels: Vec<Entity> = {
            let mut q = self.world.query_filtered::<Entity, With<flight::Powered>>();
            q.iter(&self.world).collect()
        };
        let mut to_rails = Vec::new();
        for e in powered_vessels.drain(..) {
            let Some(body_e) = self.world.get::<bodies::ReferenceBody>(e).map(|r| r.0) else {
                continue;
            };
            let body = self
                .world
                .get::<bodies::Body>(body_e)
                .expect("body")
                .clone();
            let atmo = self
                .world
                .get::<flight::Atmosphere>(body_e)
                .map(|a| a.0.clone());
            let terrain = self
                .world
                .get::<flight::Terrain>(body_e)
                .map(|t| t.0.clone());
            let mut p = self
                .world
                .get::<flight::Powered>(e)
                .expect("powered")
                .clone();
            let mut structure = self.world.entity_mut(e).take::<structure::Structure>();
            let sun = if structure.is_some() {
                self.sunlight(body_e, p.r, ut)
            } else {
                0.0
            };
            let mut pieces = Vec::new();
            // a vessel standing on the ground with its engines off just turns with the body: one long step
            let parked = p.landed && !p.crashed && p.throttle <= 0.0;
            let (steps, step_dt) = if parked {
                (1, dt * ticks.max(1) as f64)
            } else {
                (ticks.max(1), dt)
            };
            for i in 0..steps {
                if let Some(st) = &mut structure {
                    st.sync(&mut p);
                    // part drag for the airflow now
                    p.drag_cda_m2 = atmo.as_ref().map(|a| {
                        let omega = if body.rotation_period_s > 0.0 {
                            std::f64::consts::TAU / body.rotation_period_s
                        } else {
                            0.0
                        };
                        let v_air = p.v - DVec3::new(0.0, 0.0, omega).cross(p.r);
                        let alt = p.r.length() - body.radius_m;
                        let sound = a.speed_of_sound(alt);
                        let mach = if sound > 0.0 {
                            v_air.length() / sound
                        } else {
                            0.0
                        };
                        st.aero(p.stage, p.facing, v_air, a.density(alt), mach)
                    });
                }
                let mass_before = p.mass_t;
                let was_landed = p.landed;
                let out = flight::integrate(
                    &mut p,
                    &body,
                    atmo.as_ref(),
                    terrain.as_deref(),
                    start_ut + i as f64 * step_dt,
                    step_dt,
                );
                self.log_flight_step(
                    e,
                    &body,
                    terrain.as_deref(),
                    &p,
                    was_landed,
                    start_ut + (i + 1) as f64 * step_dt,
                );
                if let (Some(st), true) = (&structure, out.thrust_kn > 0.0) {
                    let ids: Vec<String> = st
                        .live_engines(p.stage)
                        .into_iter()
                        .map(|i| st.part_ids[i].clone())
                        .collect();
                    if let Some(mut log) = self.log_mut(e) {
                        for id in ids {
                            log.activated(&id);
                        }
                    }
                }
                if let Some(st) = &mut structure {
                    // the propellant burned comes out of the tanks through the flow graph
                    let burned = (mass_before - p.mass_t).max(0.0);
                    st.burn(&mut p, burned, step_dt);
                    // wheels work in proportion to how fast the vessel is being turned
                    let turning = (p.angvel.length() / 0.2).clamp(0.0, 1.0);
                    st.electric(&mut p, sun, turning, step_dt);
                    for piece in st.apply(&p, &out, step_dt) {
                        pieces.push((piece, p.r, p.v, p.facing));
                    }
                    if !pieces.is_empty() {
                        st.sync(&mut p);
                    }
                }
            }
            if let Some(st) = structure {
                self.world.entity_mut(e).insert(st);
            }
            let area = p.spec.drag_area_m2;
            for (piece, r, v, facing) in pieces {
                if let Some(d) = self.spawn_debris(
                    e,
                    piece.mass_t,
                    r + piece.offset_m,
                    v + piece.dv_mps,
                    facing,
                    area,
                ) {
                    self.broken_off.push((e, d));
                }
            }
            let orbit = Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
            // back on rails only when the coast cannot hit anything: periapsis above the atmosphere and the
            // highest terrain (or an escape trajectory)
            let clear_top = body.radius_m
                + atmo
                    .as_ref()
                    .map(|a| a.depth_m)
                    .unwrap_or(0.0)
                    .max(terrain.as_ref().map(|t| t.max_m.max(0.0)).unwrap_or(0.0));
            let safe_coast = orbit.eccentricity() >= 1.0 || orbit.periapsis() > clear_top;
            let hand_off = p.coasting_since.is_some_and(|t| ut - t >= 10.0)
                && !p.landed
                && !p.crashed
                && safe_coast;
            self.world
                .entity_mut(e)
                .insert(bodies::OnRails(orbit))
                .insert(p);
            // sphere-of-influence change while powered: re-express the state around the new body
            if !self
                .world
                .get::<flight::Powered>(e)
                .is_some_and(|p| p.landed)
            {
                let t = vessels::update_reference_body(&mut self.world, &index, e, ut);
                if t != vessels::SoiTransition::None {
                    let rails = self.world.get::<bodies::OnRails>(e).map(|o| o.0);
                    if let (Some(o), Some(mut p)) =
                        (rails, self.world.get_mut::<flight::Powered>(e))
                    {
                        let (r, v) = o.state_at(ut);
                        p.r = r;
                        p.v = v;
                    }
                }
            }
            if hand_off {
                to_rails.push(e);
            }
        }
        for e in to_rails {
            if let Some(p) = self.world.get::<flight::Powered>(e).cloned() {
                let mut ent = self.world.entity_mut(e);
                ent.remove::<flight::Powered>();
                ent.insert(flight::Dormant(p));
            }
            vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
        }
        self.advance_experiments(ticks.max(1) as f64 * dt);
        // crashed debris is gone
        let wrecks: Vec<Entity> = {
            let mut q = self
                .world
                .query_filtered::<(Entity, &flight::Powered), With<vessels::Debris>>();
            q.iter(&self.world)
                .filter(|(_, p)| p.crashed)
                .map(|(e, _)| e)
                .collect()
        };
        for e in wrecks {
            self.world.despawn(e);
        }
        for e in self.vessels() {
            if self.world.get::<flight::Powered>(e).is_some() {
                continue;
            }
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

    fn kerbin_def() -> ose_data::CelestialBody {
        use ose_data::{CelestialBody as BodyDef, BIG_G, G0};
        let mu = 1.00034160493135 * G0 * 600_000.0 * 600_000.0;
        BodyDef {
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
        }
    }

    fn upper_stage() -> flight::FlightSpec {
        flight::FlightSpec {
            stages: vec![flight::StageSpec {
                stage_id: 0,
                thrust_vac_kn: 60.0,
                thrust_asl_kn: 30.0,
                isp_vac_s: 345.0,
                isp_asl_s: 170.0,
                propellant_t: 2.0,
                start_mass_t: 4.0,
                ..Default::default()
            }],
            drag_area_m2: 1.0,
            drag_cd: flight::DEFAULT_CD,
        }
    }

    #[test]
    fn vessel_in_orbit_wakes_burns_and_returns_to_rails_with_less_propellant() {
        let mut u = Universe::with_bodies(&[kerbin_def()]);
        let body = u.body_entity("Kerbin").unwrap();
        let v = u.spawn_vessel_in_circular_orbit("upper", body, 100_000.0);
        let (_, _, o0) = u.vessel(v).unwrap();
        let (r0, v0) = o0.state_at(u.ut());
        u.set_dormant(v, flight::Powered::new(upper_stage(), r0, v0, false));
        assert!(u.powered(v).is_none());
        // zero throttle keeps it on rails
        assert!(u.set_controls(v, 0.0, flight::Attitude::OrbitPrograde));
        assert!(u.powered(v).is_none());
        // throttle up: woken at the rails position, burns prograde
        assert!(u.set_controls(v, 1.0, flight::Attitude::OrbitPrograde));
        let p = u.powered(v).expect("woken");
        assert!(
            (p.r - r0).length() < 1.0,
            "woken where the rails orbit puts it"
        );
        for _ in 0..(20 * TICKS_PER_SECOND) {
            u.step();
        }
        let (_, _, o1) = u.vessel(v).unwrap();
        assert!(
            o1.apoapsis() > o0.apoapsis() + 50_000.0,
            "apoapsis {} -> {}",
            o0.apoapsis(),
            o1.apoapsis()
        );
        let burned = 2.0 - u.powered(v).unwrap().propellant_t;
        assert!(burned > 0.3, "propellant burned {burned}");
        // cut throttle; after 10 s of coasting it goes dormant again, keeping what is left
        assert!(u.set_controls(v, 0.0, flight::Attitude::OrbitPrograde));
        for _ in 0..(12 * TICKS_PER_SECOND) {
            u.step();
        }
        assert!(u.powered(v).is_none(), "back on rails");
        let d = u.dormant(v).expect("dormant state kept");
        assert!((d.propellant_t - (2.0 - burned)).abs() < 1e-3);
        // staging works while dormant (single stage: nothing further, propellant dropped)
        assert!(!u.stage(v).fired);
        assert_eq!(u.dormant(v).unwrap().propellant_t, 0.0);
    }

    #[test]
    fn bare_probe_without_flight_state_refuses_controls() {
        let mut u = Universe::with_bodies(&[kerbin_def()]);
        let body = u.body_entity("Kerbin").unwrap();
        let v = u.spawn_vessel_in_circular_orbit("probe", body, 100_000.0);
        assert!(!u.set_controls(v, 1.0, flight::Attitude::OrbitPrograde));
        assert!(u.powered(v).is_none());
    }

    #[test]
    fn powered_vessel_crossing_an_soi_is_handed_to_the_parent_continuously() {
        use ose_data::{CelestialBody as BodyDef, Orbit as OrbitDef, BIG_G, G0};
        let sun_mu = 1.7467 * G0 * 261_600_000.0f64.powi(2);
        let sun = BodyDef {
            id: "Kerbol".into(),
            display_name: "Kerbol".into(),
            parent: None,
            is_star: true,
            is_home_world: false,
            radius_m: 261_600_000.0,
            gravity_asl: 1.7467,
            mu: sun_mu,
            mass_kg: sun_mu / BIG_G,
            soi_radius_m: None,
            rotation_period_s: 432_000.0,
            initial_rotation_deg: 0.0,
            is_tidally_locked: false,
            has_solid_surface: false,
            ocean: None,
            orbit: None,
            atmosphere: None,
            star_luminosity: None,
        };
        let mut k = kerbin_def();
        k.parent = Some("Kerbol".into());
        k.soi_radius_m = Some(84_159_286.0);
        k.orbit = Some(OrbitDef {
            semi_major_axis_m: 13_599_840_256.0,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            longitude_of_ascending_node_deg: 0.0,
            argument_of_periapsis_deg: 0.0,
            mean_anomaly_at_epoch_rad: 0.0,
            epoch_s: 0.0,
        });
        let mut u = Universe::with_bodies(&[sun, k]);
        let kerbin = u.body_entity("Kerbin").unwrap();
        let kerbol = u.body_entity("Kerbol").unwrap();
        // 1000 km inside the SOI, moving outward at 3 km/s, engine at a whisper so it stays powered
        let r = DVec3::new(84_159_286.0 - 1_000_000.0, 0.0, 0.0);
        let vv = DVec3::new(3_000.0, 500.0, 0.0);
        let e = u.spawn_vessel("escaper", kerbin, r, vv);
        u.set_dormant(e, flight::Powered::new(upper_stage(), r, vv, false));
        assert!(u.set_controls(e, 0.01, flight::Attitude::OrbitPrograde));
        let mut last = bodies::star_space_state(&u.world, e, u.ut()).0;
        let mut handed = false;
        for _ in 0..(600 * TICKS_PER_SECOND) {
            u.step();
            let now = bodies::star_space_state(&u.world, e, u.ut()).0;
            assert!(
                (now - last).length() < 1_000.0,
                "jump {}",
                (now - last).length()
            );
            last = now;
            if u.world.get::<bodies::ReferenceBody>(e).unwrap().0 == kerbol {
                handed = true;
                break;
            }
        }
        assert!(handed, "never left Kerbin's SOI");
        let p = u.powered(e).expect("still powered after the handoff");
        let (rails_r, _) = u.vessel(e).unwrap().2.state_at(u.ut());
        assert!(
            (p.r - rails_r).length() < 1.0,
            "powered state re-expressed around Kerbol"
        );
        assert!(p.r.length() > 1.0e10, "now relative to the star");
    }

    #[test]
    fn staging_leaves_a_falling_debris_vessel_that_is_removed_on_impact() {
        let mut u = Universe::with_bodies(&[kerbin_def()]);
        let body = u.body_entity("Kerbin").unwrap();
        let spec = flight::FlightSpec {
            stages: vec![
                flight::StageSpec {
                    stage_id: 1,
                    thrust_vac_kn: 400.0,
                    thrust_asl_kn: 400.0,
                    isp_vac_s: 300.0,
                    isp_asl_s: 300.0,
                    propellant_t: 8.0,
                    start_mass_t: 14.0,
                    ..Default::default()
                },
                flight::StageSpec {
                    stage_id: 0,
                    thrust_vac_kn: 60.0,
                    thrust_asl_kn: 60.0,
                    isp_vac_s: 340.0,
                    isp_asl_s: 340.0,
                    propellant_t: 2.0,
                    start_mass_t: 4.0,
                    ..Default::default()
                },
            ],
            drag_area_m2: 1.0,
            drag_cd: flight::DEFAULT_CD,
        };
        let v = u.spawn_powered_on_surface("hopper", body, spec, 0.0, 0.0);
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        let before = u.powered(v).unwrap().mass_t;
        let res = u.stage(v);
        assert!(res.fired);
        let d = res.debris.expect("debris vessel");
        let dp = u.powered(d).expect("debris is a flying point mass");
        assert!(
            (dp.mass_t + u.powered(v).unwrap().mass_t - before).abs() < 1e-6,
            "mass conserved"
        );
        assert!(u.world.get::<vessels::Debris>(d).is_some());
        assert_eq!(u.vessel(d).unwrap().0.name, "hopper debris");
        assert!(u.vessels().len() == 2);
        // it falls back and is removed when it hits the ground
        for _ in 0..(120 * TICKS_PER_SECOND) {
            u.step();
            if u.vessel(d).is_none() {
                break;
            }
        }
        assert!(u.vessel(d).is_none(), "debris still there");
        assert!(u.vessel(v).is_some());
    }

    fn k1_on_real_kerbin() -> (Universe, Entity, flight::FlightSpec, structure::Structure) {
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
        kerbin.parent = None;
        kerbin.orbit = None;
        kerbin.soi_radius_m = None;
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let st = structure::Structure::build(k1, &defs, &res);
        let u = Universe::with_bodies(&[kerbin]);
        let body = u.body_entity("Kerbin").unwrap();
        (u, body, spec, st)
    }

    #[test]
    fn sunlight_falls_off_with_distance_and_planets_cast_shadows() {
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let u = Universe::with_bodies(&bodies);
        let kerbin = u.body_entity("Kerbin").unwrap();
        let ut = 0.0;
        let k = bodies::star_space_state(&u.world, kerbin, ut).0;
        let to_sun = (k * -1.0).normalized();
        let day = u.sunlight(kerbin, to_sun * 700_000.0, ut);
        assert!(
            (day - 1.0).abs() < 0.05,
            "at home the panels give their rating ({day})"
        );
        assert_eq!(
            u.sunlight(kerbin, to_sun * -700_000.0, ut),
            0.0,
            "night side"
        );
        let moho = u.body_entity("Moho").unwrap();
        let m = bodies::star_space_state(&u.world, moho, ut).0;
        let near = u.sunlight(moho, (m * -1.0).normalized() * 300_000.0, ut);
        assert!(near > 3.0, "Moho is bright ({near})");
    }

    #[test]
    fn kerbal_k1_launches_at_full_thrust_in_one_piece() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        let mut peak_stress: f64 = 0.0;
        for _ in 0..(15 * TICKS_PER_SECOND) {
            u.step();
            peak_stress = peak_stress.max(u.structure(v).unwrap().rigid.max_stress());
        }
        assert!(
            peak_stress > 0.0 && peak_stress < 1.0,
            "loaded but within limits ({peak_stress})"
        );
        let p = u.powered(v).unwrap();
        assert!(!p.landed && p.v.length() > 100.0, "climbing");
        assert!(u.take_broken_off().is_empty());
        assert_eq!(u.structure(v).unwrap().rigid.groups().len(), 1);
    }

    #[test]
    fn kerbal_k1_burns_its_tanks_through_the_flow_graph_and_stages_off_the_boosters() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let spec_mass = spec.stages[0].start_mass_t;
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        let m0 = u.powered(v).unwrap().mass_t;
        assert!(
            (m0 - spec_mass).abs() < 1e-6,
            "tank mass {m0} matches the staging report {spec_mass}"
        );
        let sf = |u: &Universe| {
            let s = u.structure(v).unwrap();
            let r = s.net.resource("SolidFuel").unwrap();
            s.net
                .tanks
                .iter()
                .filter(|t| t.resource == r && s.net.present[t.part])
                .map(|t| t.stored)
                .sum::<f64>()
        };
        let lf = |u: &Universe| {
            let s = u.structure(v).unwrap();
            let r = s.net.resource("Oxidizer").unwrap();
            s.net
                .tanks
                .iter()
                .filter(|t| t.resource == r && s.net.present[t.part])
                .map(|t| t.stored)
                .sum::<f64>()
        };
        let (sf0, ox0) = (sf(&u), lf(&u));
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).unwrap().clone();
        let s = u.structure(v).unwrap();
        assert!(sf(&u) < sf0 && lf(&u) < ox0, "boosters and core both burn");
        assert!(
            (p.mass_t - s.net.mass_t()).abs() < 1e-9,
            "vessel mass is the tank mass"
        );
        assert!(m0 - p.mass_t > 1.0, "burned {} t", m0 - p.mass_t);
        // staging drops the boosters with their leftover solid fuel
        let res = u.stage(v);
        assert!(res.fired);
        let d = res.debris.expect("boosters become debris");
        let dm = u.powered(d).unwrap().mass_t;
        let after = u.powered(v).unwrap().mass_t;
        assert!(
            (dm + after - p.mass_t).abs() < 1e-6,
            "{dm} + {after} vs {}",
            p.mass_t
        );
        assert!(u.structure(v).unwrap().net.mass_t() - after < 1e-9);
    }

    #[test]
    fn kerbal_k2_climbs_in_one_piece_and_each_stage_has_its_own_fuel() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let k2 = crafts.iter().find(|c| c.name == "Kerbal K2").unwrap();
        let report = ose_data::staging::delta_v_by_stage(k2, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("K2", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(k2, &defs, &res));
        // parked on the pad it does not hold time warp down for everyone
        assert!(!u.has_powered());
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        assert!(u.has_powered());
        // boosters and core burn, the boosters drop, the core burns out, the upper stage still has its tanks
        let mut upper = 0.0;
        for _ in 0..3 {
            for _ in 0..(300 * TICKS_PER_SECOND) {
                u.step();
                assert!(u.take_broken_off().is_empty(), "the K2 comes apart");
                if u.powered(v).is_some_and(|p| p.propellant_t <= 1e-6) {
                    break;
                }
            }
            assert!(u.stage(v).fired);
            upper = u.powered(v).unwrap().propellant_t;
            if u.powered(v).unwrap().stage == 2 {
                break;
            }
        }
        let p = u.powered(v).unwrap();
        assert_eq!(p.stage, 2);
        assert!(upper > 10.0, "the Poodle stage kept its {upper} t");
        assert!(p.r.length() - 600_000.0 > 40_000.0, "climbing");
    }

    #[test]
    fn flying_broadside_through_thick_air_tears_parts_off_as_debris() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        // 1 km up, 3 km/s across the stack (dynamic pressure near 5 MPa), nose held straight up
        let r = DVec3::new(600_000.0 + 1_000.0, 0.0, 0.0);
        let mut p = flight::Powered::new(spec, r, DVec3::new(0.0, 3_000.0, 0.0), false);
        p.attitude = flight::Attitude::Fixed(DVec3::new(1.0, 0.0, 0.0));
        p.facing = DVec3::new(1.0, 0.0, 0.0);
        u.set_powered(v, p);
        u.set_structure(v, st);
        let before = u.powered(v).unwrap().mass_t;
        let mut broken = Vec::new();
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
            broken.extend(u.take_broken_off());
        }
        assert!(!broken.is_empty(), "the boosters should tear off");
        let shed: f64 = broken
            .iter()
            .filter_map(|&(from, d)| {
                assert_eq!(from, v);
                assert!(u.world.get::<vessels::Debris>(d).is_some());
                u.powered(d).map(|dp| dp.spec.stages[0].start_mass_t)
            })
            .sum();
        let after = u.powered(v).unwrap().mass_t;
        assert!(after < before - 1.0, "mass {before} -> {after}");
        assert!(shed > 1.0, "debris carries the mass ({shed} t)");
    }

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

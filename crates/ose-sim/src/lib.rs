//! Deterministic simulation, run natively by the server (ADR-0002).
//!
//! State lives in a `bevy_ecs` `World`. Time is an integer tick count (`Tick`), `TICKS_PER_SECOND` ticks per
//! second of universal time. No I/O, no wall clock, no randomness without an explicit seed.

#![forbid(unsafe_code)]

pub mod actuators;
pub mod aero;
pub mod bodies;
pub mod campus;
pub mod commnet;
pub mod contact;
pub mod crew;
pub mod docking;
pub mod dynamics;
pub mod flight;
pub mod missions;
pub mod resources;
pub mod rigid;
pub mod science;
pub mod structure;
pub mod surface;
pub mod thermal;
pub mod trajectory;
pub mod vessels;

use bevy_ecs::prelude::*;
use ose_math::{DVec3, Orbit};
use serde::{Deserialize, Serialize};

pub use ose_data::{Tick, TICKS_PER_SECOND};

/// An action group a player can fire (`ActionGroup` keys: U, R, Backspace, 1..0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionGroup {
    Lights,
    Rcs,
    Abort,
    /// 1..10.
    Custom(u8),
}

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
        let terrain = self.world.get::<flight::Terrain>(body).map(|t| t.0.clone());
        let ground = flight::ground_height_at(&b, terrain.as_deref(), lat_deg, lon_deg);
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

    /// Use a terrain map another universe already holds (a new campaign on the same server).
    pub fn set_terrain_shared(
        &mut self,
        id: &str,
        map: std::sync::Arc<ose_data::terrain::TerrainMap>,
    ) -> bool {
        let Some(e) = self.body_entity(id) else {
            return false;
        };
        self.world.entity_mut(e).insert(flight::Terrain(map));
        true
    }

    /// The terrain maps the bodies hold, by body id.
    pub fn terrain_maps(&mut self) -> Vec<(String, std::sync::Arc<ose_data::terrain::TerrainMap>)> {
        let mut q = self.world.query::<(&bodies::Body, &flight::Terrain)>();
        q.iter(&self.world)
            .map(|(b, t)| (b.id.clone(), t.0.clone()))
            .collect()
    }

    /// How high the sun stands over a body-fixed place at `ut`, degrees (negative at night).
    pub fn sun_elevation_deg(&self, body: Entity, lat_deg: f64, lon_deg: f64, ut: f64) -> f64 {
        let Some(b) = self.world.get::<bodies::Body>(body) else {
            return 0.0;
        };
        let (lat, lon) = (
            lat_deg.to_radians(),
            lon_deg.to_radians() + flight::rotation_angle(b, ut),
        );
        let up = DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin());
        let place = bodies::star_space_state(&self.world, body, ut).0 + up * b.radius_m;
        if place.length() <= 0.0 {
            return 90.0;
        }
        let to_sun = place * (-1.0 / place.length());
        up.dot(to_sun).clamp(-1.0, 1.0).asin().to_degrees()
    }

    /// The first time from `after` at which the sun rises through `elevation_deg` over a place (morning),
    /// to the minute; `after` when the body does not turn.
    pub fn next_morning_ut(
        &self,
        body: Entity,
        lat_deg: f64,
        lon_deg: f64,
        elevation_deg: f64,
        after: f64,
    ) -> f64 {
        let period = self
            .world
            .get::<bodies::Body>(body)
            .map(|b| b.rotation_period_s)
            .unwrap_or(0.0);
        if period <= 0.0 {
            return after;
        }
        let mut prev = self.sun_elevation_deg(body, lat_deg, lon_deg, after);
        let mut t = after;
        while t < after + 2.0 * period {
            t += 60.0;
            let e = self.sun_elevation_deg(body, lat_deg, lon_deg, t);
            if prev < elevation_deg && e >= elevation_deg {
                return t;
            }
            prev = e;
        }
        after
    }

    /// Ground height (terrain or sea) above the nominal radius at a body-fixed latitude/longitude.
    pub fn ground_height(&self, body: Entity, lat_deg: f64, lon_deg: f64) -> f64 {
        let Some(b) = self.world.get::<bodies::Body>(body) else {
            return 0.0;
        };
        let terrain = self.world.get::<flight::Terrain>(body).map(|t| t.0.clone());
        flight::ground_height_at(b, terrain.as_deref(), lat_deg, lon_deg)
    }

    /// Put a built site (the KSC campus) on a body: craft over it stand on its surfaces.
    pub fn set_site(&mut self, id: &str, site: campus::Site) -> bool {
        let Some(e) = self.body_entity(id) else {
            return false;
        };
        match self.world.get_mut::<bodies::Body>(e) {
            Some(mut b) => {
                b.site = Some(std::sync::Arc::new(site));
                true
            }
            None => false,
        }
    }

    /// Attach a saved powered-flight state to an existing vessel (restore).
    pub fn set_powered(&mut self, e: Entity, p: flight::Powered) {
        self.world.entity_mut(e).insert(p);
    }

    pub fn powered(&self, e: Entity) -> Option<&flight::Powered> {
        self.world.get::<flight::Powered>(e)
    }

    /// Electric charge for vessels on rails: panels in the sun, generators and probe cores keep running
    /// through time warp (wheels idle).
    fn charge_on_rails(&mut self, dt: f64, ut: f64) {
        let on_rails: Vec<Entity> = {
            let mut q = self
                .world
                .query_filtered::<Entity, (With<structure::Structure>, With<flight::Dormant>)>();
            q.iter(&self.world).collect()
        };
        for e in on_rails {
            let (Some(body), Some(r)) = (self.body_of(e), self.position_of(e)) else {
                continue;
            };
            let sun = self.sunlight(body, r, ut);
            let Some(mut st) = self.world.entity_mut(e).take::<structure::Structure>() else {
                continue;
            };
            if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
                st.electric(&mut d.0, sun, 0.0, dt);
            }
            self.world.entity_mut(e).insert(st);
        }
    }

    /// RCS translation input in craft axes (x right, y fore, z top; -1..1 each, held until changed); false
    /// without a powered vessel.
    pub fn set_translate(&mut self, e: Entity, t: DVec3) -> bool {
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                p.rcs.translate = t;
                true
            }
            None => false,
        }
    }

    /// Position and velocity of a vessel or body in the star's frame at the current time.
    pub fn star_state(&self, e: Entity) -> Option<(DVec3, DVec3)> {
        let ut = self.ut();
        if self.world.get::<bodies::Body>(e).is_some() {
            return Some(bodies::star_space_state(&self.world, e, ut));
        }
        let body = self.world.get::<bodies::ReferenceBody>(e)?.0;
        let (bp, bv) = bodies::star_space_state(&self.world, body, ut);
        let (r, v) = match self.world.get::<flight::Powered>(e) {
            Some(p) => (p.r, p.v),
            None => self.world.get::<bodies::OnRails>(e)?.0.state_at(ut),
        };
        Some((bp + r, bv + v))
    }

    /// Set (or clear) a vessel's target: another vessel or a body.
    pub fn set_target(&mut self, e: Entity, target: Option<Entity>) -> bool {
        if self.world.get_entity(e).is_err() {
            return false;
        }
        match target {
            Some(t) if t != e && self.star_state(t).is_some() => {
                self.world.entity_mut(e).insert(vessels::Target(t));
                true
            }
            Some(_) => false,
            None => {
                self.world.entity_mut(e).remove::<vessels::Target>();
                true
            }
        }
    }

    pub fn target(&self, e: Entity) -> Option<Entity> {
        self.world.get::<vessels::Target>(e).map(|t| t.0)
    }

    /// Where a vessel's target is from it (inertial offset, m), how fast it moves relative to it (target
    /// velocity minus the vessel's, m/s), and its distance to the target's surface (bodies) or centre.
    pub fn target_relative(&self, e: Entity) -> Option<(DVec3, DVec3, f64)> {
        let t = self.target(e)?;
        let (p, v) = self.star_state(e)?;
        let (tp, tv) = self.star_state(t)?;
        let d = tp - p;
        let surface = self
            .world
            .get::<bodies::Body>(t)
            .map(|b| b.radius_m)
            .unwrap_or(0.0);
        Some((d, tv - v, (d.length() - surface).max(0.0)))
    }

    /// Rotation input with SAS off (pitch, yaw, roll each -1..1, held until changed; see
    /// `Powered::rotation_input`); false without a powered vessel.
    pub fn set_rotation(&mut self, e: Entity, pitch: f64, yaw: f64, roll: f64) -> bool {
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                p.rotation_input = DVec3::new(pitch, yaw, roll);
                true
            }
            None => false,
        }
    }

    /// A Kerbal on EVA runs (held) and jumps (once, from the ground); false for anything else.
    pub fn set_eva(&mut self, e: Entity, run: bool, jump: bool) -> bool {
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) if p.walker => {
                p.eva_run = run;
                p.eva_jump |= jump && p.landed;
                true
            }
            _ => false,
        }
    }

    /// Roll input for a vessel (-1..1, held until changed); false without a powered vessel.
    pub fn set_roll(&mut self, e: Entity, input: f64) -> bool {
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                p.roll_input = input.clamp(-1.0, 1.0);
                true
            }
            None => false,
        }
    }

    /// Set (`Some`) or toggle (`None`) an action group; returns its new state, `None` without a powered
    /// vessel. Lights and RCS are switches; abort cuts the throttle and arms the parachutes; custom groups
    /// (1..10) are switches the craft has no actions bound to yet.
    pub fn action_group(
        &mut self,
        e: Entity,
        group: ActionGroup,
        on: Option<bool>,
    ) -> Option<bool> {
        let mut p = self.world.get_mut::<flight::Powered>(e)?;
        let g = &mut p.groups;
        let slot = match group {
            ActionGroup::Lights => &mut g.lights,
            ActionGroup::Rcs => &mut g.rcs,
            ActionGroup::Abort => &mut g.abort,
            ActionGroup::Custom(n) => g.custom.get_mut((n as usize).checked_sub(1)?)?,
        };
        let state = on.unwrap_or(!*slot);
        *slot = state;
        let abort = group == ActionGroup::Abort && state;
        if abort {
            p.throttle = 0.0;
        }
        if abort {
            self.arm_parachutes(e);
        }
        Some(state)
    }

    /// Jettison a vessel's fairings; how many, `None` without a structure.
    pub fn deploy_fairings(&mut self, e: Entity) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.deploy_fairings())
    }

    /// Open or close a vessel's cargo bays; how many changed, `None` without a structure.
    pub fn open_cargo_bays(&mut self, e: Entity, open: bool) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.open_bays(open))
    }

    /// Switch a vessel's resource converters (fuel cells, reactors) on or off; how many changed, `None`
    /// without a structure.
    pub fn set_converters(&mut self, e: Entity, on: bool) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.set_converters(on))
    }

    /// Wheel throttle, steering and brakes for a vessel; false without a powered vessel.
    pub fn set_drive(&mut self, e: Entity, throttle: f64, steer: f64, brakes: bool) -> bool {
        let Some(mut p) = self.world.get_mut::<flight::Powered>(e) else {
            return false;
        };
        p.drive.throttle = throttle.clamp(-1.0, 1.0);
        p.drive.steer = steer.clamp(-1.0, 1.0);
        p.drive.brakes = brakes;
        true
    }

    /// Lay a powered vessel level (a plane, built nose-forward and turned by `structure::level_layout`):
    /// on the ground it lies on its wheels pointing along compass heading `heading_deg`, and it holds that
    /// heading level until told otherwise.
    pub fn set_level(&mut self, e: Entity, heading_deg: f64) -> bool {
        let Some(mut p) = self.world.get_mut::<flight::Powered>(e) else {
            return false;
        };
        p.level = true;
        p.drive.heading_rad = heading_deg.to_radians().rem_euclid(std::f64::consts::TAU);
        p.facing = flight::heading_dir(p.r, p.drive.heading_rad);
        p.attitude = flight::Attitude::PitchHeading {
            pitch_deg: 0.0,
            heading_deg,
        };
        let mut p = p.clone();
        if let Some(mut st) = self.world.entity_mut(e).take::<structure::Structure>() {
            st.level = true;
            st.sync(&mut p);
            self.world.entity_mut(e).insert(st);
        }
        // stand on the wheels at the new clearance
        let body = self.world.get::<bodies::ReferenceBody>(e).map(|b| b.0);
        if let Some(b) = body
            .and_then(|b| self.world.get::<bodies::Body>(b))
            .cloned()
        {
            let terrain = body
                .and_then(|b| self.world.get::<flight::Terrain>(b))
                .map(|t| t.0.clone());
            let ground = flight::ground_radius(&b, terrain.as_deref(), p.r, self.ut());
            p.r = p.r.normalized() * (ground + p.ground_clearance_m);
        }
        self.world.entity_mut(e).insert(p);
        true
    }

    /// Set the heat difficulty options.
    pub fn set_heat_settings(&mut self, settings: thermal::HeatSettings) {
        self.world.insert_resource(settings);
    }

    /// Fold a vessel's landing gear out or in; how many changed, `None` without a structure. The vessel
    /// stands on whatever is lowest afterwards.
    pub fn deploy_gear(&mut self, e: Entity, out: bool) -> Option<usize> {
        let mut st = self.world.entity_mut(e).take::<structure::Structure>()?;
        let n = st.deploy_gear(out);
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            st.sync(&mut p);
        }
        self.world.entity_mut(e).insert(st);
        Some(n)
    }

    /// Arm a vessel's packed parachutes; how many, `None` without a structure.
    pub fn arm_parachutes(&mut self, e: Entity) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.arm_all_chutes())
    }

    /// Whether any deployable panel of the vessel is out.
    pub fn panels_deployed(&self, e: Entity) -> bool {
        self.world.get::<structure::Structure>(e).is_some_and(|st| {
            st.deployed.iter().enumerate().any(|(i, d)| {
                *d && st.rigid.present[i]
                    && st.electric[i].solar_deployable
                    && st.electric[i].solar_output > 0.0
            })
        })
    }

    /// Fold a vessel's deployable solar panels out or in; how many moved, `None` without a structure.
    pub fn deploy_panels(&mut self, e: Entity, out: bool) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.deploy_panels(out))
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
            s.level = p.level;
            s.sync(&mut p);
        } else if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
            s.level = d.0.level;
            s.sync(&mut d.0);
        }
        self.world.entity_mut(e).insert(s);
        self.stand_on_ground(e);
        // crewed parts get their Kerbals (the server's roster replaces these)
        if self.world.get::<crew::Crew>(e).is_none() {
            let roster: Vec<String> = crew::VETERANS.iter().map(|n| n.to_string()).collect();
            self.crew_vessel(e, &roster);
        }
    }

    /// Put a landed vessel on the ground at its clearance right away (before its first step).
    fn stand_on_ground(&mut self, e: Entity) {
        let ut = self.ut();
        let body = self.world.get::<bodies::ReferenceBody>(e).map(|b| b.0);
        let Some(b) = body
            .and_then(|b| self.world.get::<bodies::Body>(b))
            .cloned()
        else {
            return;
        };
        let terrain = body
            .and_then(|b| self.world.get::<flight::Terrain>(b))
            .map(|t| t.0.clone());
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            if p.landed && !p.crashed && !p.tipped {
                let ground = flight::ground_radius(&b, terrain.as_deref(), p.r, ut);
                p.r = p.r.normalized() * (ground + p.ground_clearance_m);
            }
        }
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
        // The first stage press lights the first stage of a craft that has not fired yet: nothing is dropped.
        let ignite = |p: &mut flight::Powered| !std::mem::replace(&mut p.ignited, true);
        let first = match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => ignite(&mut p),
            None => self
                .world
                .get_mut::<flight::Dormant>(e)
                .is_some_and(|mut d| ignite(&mut d.0)),
        };
        if first {
            if let Some(mut st) = self.world.get_mut::<structure::Structure>(e) {
                st.activate_stage(0);
            }
            return StageResult {
                fired: true,
                debris: None,
            };
        }
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
        let mut impulse = 0.0;
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
                // the new stage's parts fire (parachutes arm)
                let stage = self
                    .world
                    .get::<flight::Powered>(e)
                    .map(|p| p.stage)
                    .or_else(|| self.world.get::<flight::Dormant>(e).map(|d| d.0.stage));
                if let Some(stage) = stage {
                    st.activate_stage(stage);
                    impulse = st.ejection_impulse(stage);
                }
            }
            if let Some(st) = st {
                self.world.entity_mut(e).insert(st);
            }
        }
        let debris = if outcome.fired && outcome.dropped_t > 0.01 {
            // the decouplers push the two sides apart (light pieces get a fifth:
            // GLOBAL_EJECTION_FORCE_SCALE below GLOBAL_EJECTION_FORCE_SCALING_LOWMASS_LIMIT)
            let impulse = if outcome.dropped_t < 0.5 {
                impulse * 0.2
            } else {
                impulse
            };
            let away = if impulse > 0.0 {
                impulse / outcome.dropped_t
            } else {
                0.5
            };
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                if !p.landed && p.mass_t > 0.0 {
                    p.v = p.v + facing * (impulse / p.mass_t);
                }
            }
            self.spawn_debris(
                e,
                outcome.dropped_t,
                r - facing * 3.0,
                v - facing * away,
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
        let heat = self
            .world
            .get_resource::<thermal::HeatSettings>()
            .copied()
            .unwrap_or_default();
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
            // a maneuver hold points at the scheduled burn as it stands now
            if matches!(p.attitude, flight::Attitude::Maneuver(_)) {
                let burn = self.world.get::<vessels::ScheduledBurn>(e).copied();
                let orbit = self.world.get::<bodies::OnRails>(e).map(|o| o.0);
                if let (Some(b), Some(o)) = (burn, orbit) {
                    let dv = vessels::burn_delta_v(&o, &b);
                    if dv.length() > 1e-9 {
                        p.attitude = flight::Attitude::Maneuver(dv.normalized());
                    }
                }
            }
            // a target hold points at the target as it stands now
            if matches!(
                p.attitude,
                flight::Attitude::Target(_) | flight::Attitude::AntiTarget(_)
            ) {
                if let Some((d, _, _)) = self.target_relative(e) {
                    if d.length() > 1e-6 {
                        let u = d.normalized();
                        p.attitude = match p.attitude {
                            flight::Attitude::AntiTarget(_) => flight::Attitude::AntiTarget(u),
                            _ => flight::Attitude::Target(u),
                        };
                    }
                }
            }
            let comm_blocked = self.commnet_blocks_control(e);
            let mut structure = self.world.entity_mut(e).take::<structure::Structure>();
            let sun = if structure.is_some() {
                self.sunlight(body_e, p.r, ut)
            } else {
                0.0
            };
            let mut pieces = Vec::new();
            // a vessel standing on the ground with its engines off just turns with the body: one long step
            let parked = p.landed
                && !p.crashed
                && p.throttle <= 0.0
                && p.drive.throttle == 0.0
                && p.drive.speed_mps == 0.0
                && flight::resting_on_ground(&p, &body, terrain.as_deref(), start_ut);
            let (steps, step_dt) = if parked {
                (1, dt * ticks.max(1) as f64)
            } else {
                (ticks.max(1), dt)
            };
            for i in 0..steps {
                if let Some(st) = &mut structure {
                    st.sync(&mut p);
                    let mut burned = Vec::new();
                    let mut lift = DVec3::ZERO;
                    let mut torque = DVec3::ZERO;
                    let mut surfaces = 0.0;
                    // part drag (and heating) for the airflow now
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
                        let ground = flight::ground_radius(&body, terrain.as_deref(), p.r, ut);
                        st.update_chutes(
                            a.pressure_kpa(alt) / 101.325,
                            p.r.length() - ground,
                            mach,
                            p.landed,
                            v_air.length(),
                            step_dt,
                        );
                        let frame = flight::vessel_basis(&p);
                        let forces = st.aerodynamics(p.stage, frame, v_air, a.density(alt), mach);
                        let cda = forces.cda_m2;
                        lift = forces.lift_kn;
                        torque = forces.torque_knm;
                        surfaces = st.control_authority(v_air, a.density(alt), mach);
                        if heat.enabled {
                            burned = st.update_heat(
                                frame,
                                v_air,
                                a.density(alt),
                                mach,
                                a.temperature_k.evaluate(alt),
                                thermal::conductivity(alt, a.depth_m),
                                &heat,
                                step_dt,
                            );
                        }
                        cda
                    });
                    p.lift_kn = lift;
                    p.aero_torque_knm = torque;
                    p.surface_torque_knm = surfaces;
                    if !burned.is_empty() {
                        match st.burn_up(&burned, p.facing) {
                            Some(off) => {
                                for piece in off {
                                    pieces.push((piece, p.r, p.v, p.facing));
                                }
                                st.sync(&mut p);
                            }
                            // the root burned up: the vessel is lost
                            None => {
                                p.crashed = true;
                                p.throttle = 0.0;
                            }
                        }
                    }
                }
                let env = flight::EngineEnvironment::at(&p, &body, atmo.as_ref());
                let step_ut = start_ut + i as f64 * step_dt;
                let propulsion = structure.as_mut().map(|st| {
                    st.electric_services(&mut p, sun, step_dt);
                    st.sync(&mut p);
                    if comm_blocked {
                        p.controlled = false;
                        p.throttle = 0.0;
                    }
                    let mut propulsion = st.propulsion(&p, env, step_dt);
                    // Plan with rated wheel axes; current delivery is resolved at each wheel below.
                    p.wheel_scale = 1.0;
                    st.configure_actuators(&p, env, &mut propulsion);
                    st.configure_surface(
                        &mut p,
                        &body,
                        terrain.as_deref(),
                        step_ut,
                        step_dt,
                        &mut propulsion,
                    );
                    st.resolve_wheel_power(&mut p, &body, &mut propulsion, step_dt);
                    propulsion
                });
                let before = p.clone();
                let was_landed = p.landed;
                let mut out = flight::integrate_with_propulsion(
                    &mut p,
                    &body,
                    atmo.as_ref(),
                    terrain.as_deref(),
                    start_ut + i as f64 * step_dt,
                    step_dt,
                    propulsion,
                );
                if let Some(st) = &mut structure {
                    st.resolve_surface(
                        &before,
                        &mut p,
                        &body,
                        terrain.as_deref(),
                        step_ut,
                        step_dt,
                        &mut out,
                    );
                }
                // a hard landing breaks the parts it lands on; the rest may come through
                if out.crashed {
                    if let Some(st) = &mut structure {
                        let survivors = if out.contact_damage.is_empty() {
                            st.impact(out.impact_mps, p.facing).map(|(_, off)| off)
                        } else {
                            st.contact_damage(&out.contact_damage, &p)
                        };
                        if let Some(off) = survivors {
                            p.crashed = false;
                            out.crashed = false;
                            for piece in off {
                                pieces.push((piece, p.r, p.v, p.facing));
                            }
                            st.sync(&mut p);
                        }
                    }
                }
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
                if structure.is_none() && p.walker && p.rcs.usage > 0.0 {
                    // a Kerbal's jetpack translation burns the pack
                    let flow = p.rcs.thrust_kn * p.rcs.usage.min(1.0)
                        / (p.rcs.isp_vac_s.max(1.0) * ose_data::G0);
                    let burn = (flow * step_dt).min(p.propellant_t);
                    p.propellant_t -= burn;
                    p.mass_t -= burn;
                    p.rcs.powered = if p.propellant_t > 0.0 { 1.0 } else { 0.0 };
                }
                if let Some(st) = &mut structure {
                    // Propulsion already drew exactly the fuel that produced this step's impulse.
                    st.sync(&mut p);
                    let pressure = atmo
                        .as_ref()
                        .map(|a| a.pressure_kpa(p.r.length() - body.radius_m) / 101.325)
                        .unwrap_or(0.0);
                    st.rcs_burn(&mut p, pressure, step_dt);
                    if comm_blocked {
                        // a probe out of contact cannot be flown (CommNetRequired)
                        p.controlled = false;
                        p.throttle = 0.0;
                    }
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
        // the network once a simulated second
        if now / TICKS_PER_SECOND != (now + ticks.max(1)) / TICKS_PER_SECOND {
            self.update_commnet();
        }
        self.charge_on_rails(ticks.max(1) as f64 * dt, ut);
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
        assert!(
            u.stage(v).debris.is_none(),
            "the first press lights the engines"
        );
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
        assert!(
            u.stage(v).debris.is_none(),
            "the first press lights the engines"
        );
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
        assert!(
            u.stage(v).debris.is_none(),
            "the first press lights the engines"
        );
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
    fn the_first_stage_press_on_the_pad_ignites_the_engine() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Start", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        // full throttle and one stage press, as a player launches: the engine lights, nothing drops, even when
        // a few ticks run in between (another player's connection, a busy server)
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        for _ in 0..3 {
            u.step();
        }
        // the throttle alone does nothing on the pad
        assert!(u.powered(v).unwrap().landed);
        let r = u.stage(v);
        assert!(r.fired && r.debris.is_none());
        assert_eq!(u.powered(v).unwrap().stage, 0);
        assert!(u.powered(v).unwrap().propellant_t > 0.5);
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).unwrap();
        assert!(!p.landed && p.v.dot(p.r.normalized()) > 10.0, "lifted off");
        // a second craft that drops its engine stage on the pad just sits there
        let spec2 = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let w = u.spawn_powered_on_surface("Start2", body, spec2, 0.0, 1.0);
        u.set_structure(w, structure::Structure::build(c, &defs, &res));
        u.set_controls(w, 1.0, flight::Attitude::Up);
        u.stage(w);
        u.stage(w);
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
        }
        let q = u.powered(w).unwrap();
        assert!(
            q.landed && !q.crashed,
            "landed {} crashed {}",
            q.landed,
            q.crashed
        );
        // the next press drops the spent stage as before
        for _ in 0..(120 * TICKS_PER_SECOND) {
            u.step();
            if u.powered(v).is_some_and(|p| p.propellant_t <= 1e-6) {
                break;
            }
        }
        let r = u.stage(v);
        assert!(r.fired && r.debris.is_some());
    }

    #[test]
    fn a_craft_stands_on_its_lowest_part_and_tips_over_when_it_lands_leaning() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec.clone(), 0.0, 0.0);
        u.set_structure(v, st);
        u.step();
        let p = u.powered(v).unwrap().clone();
        assert!(p.ground_clearance_m > 2.0, "{}", p.ground_clearance_m);
        let alt = p.r.length() - 600_000.0;
        assert!(
            (alt - p.ground_clearance_m).abs() < 0.5,
            "stands on its engine bell: {alt}"
        );
        let tolerance = p.crash_tolerance_mps;
        assert!(tolerance > 0.0);
        // drop it from 20 m: upright and slow it stands, leaning 45 degrees it falls over, fast it breaks
        let drop = |u: &mut Universe, lean: f64, speed: f64| {
            let mut q = p.clone();
            let up = p.r.normalized();
            let east = DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
            q.landed = false;
            q.crashed = false;
            q.tipped = false;
            q.r = up * (p.r.length() + 0.5);
            q.v = DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425).cross(q.r)
                - up * speed;
            q.facing = (up * lean.cos() + east * lean.sin()).normalized();
            q.attitude = flight::Attitude::Fixed(q.facing);
            q.throttle = 0.0;
            u.set_powered(v, q);
            for _ in 0..TICKS_PER_SECOND {
                u.step();
            }
            u.powered(v).unwrap().clone()
        };
        let upright = drop(&mut u, 0.05, 2.0);
        assert!(
            upright.landed && !upright.tipped && !upright.crashed,
            "{upright:?}"
        );
        let leaning = drop(&mut u, 0.8, 2.0);
        assert!(
            leaning.tipped && !leaning.crashed,
            "falls over: {leaning:?}"
        );
        assert!(
            leaning.angvel.length() > 0.1 || leaning.landed,
            "contact produces a physical tumble before rest"
        );
        assert!(
            leaning.r.length() - 600_000.0 < upright.r.length() - 600_000.0,
            "lying lower"
        );
        // a little faster than its bottom parts take: they break and the rest comes through
        let parts = |u: &Universe| {
            u.structure(v)
                .unwrap()
                .rigid
                .present
                .iter()
                .filter(|x| **x)
                .count()
        };
        let before = parts(&u);
        let hard = drop(&mut u, 0.0, tolerance + 5.0);
        assert!(
            hard.landed && !hard.crashed,
            "faster than its bottom parts' {tolerance} m/s: they break"
        );
        assert!(parts(&u) < before, "the engine broke off");
        // far faster: it breaks all the way up to the pod
        let fast = drop(&mut u, 0.0, 150.0);
        assert!(fast.crashed);
    }

    #[test]
    fn a_steep_re_entry_burns_up_a_bare_pod_but_not_a_shielded_one() {
        let run = |shield: bool| -> (bool, f64, f64) {
            let (mut u, body, _, _) = k1_on_real_kerbin();
            let (c, defs, res) = structure::test_capsule(shield);
            let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
            let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
            let v = u.spawn_powered_on_surface("Capsule", body, spec, 0.0, 0.0);
            u.set_structure(v, structure::Structure::build(&c, &defs, &res));
            {
                let mut p = u.world.get_mut::<flight::Powered>(v).unwrap();
                p.r = DVec3::new(645_000.0, 0.0, 0.0);
                p.v = DVec3::new(-1_800.0, 2_600.0, 0.0);
                p.facing = -p.v.normalized();
                p.landed = false;
                p.throttle = 0.0;
                p.attitude = flight::Attitude::SurfaceRetrograde;
            }
            let mut hottest: f64 = 0.0;
            for _ in 0..(120 * TICKS_PER_SECOND) {
                u.step();
                let p = u.powered(v).unwrap();
                let alt = p.r.length() - 600_000.0;
                if let Some(st) = u.structure(v) {
                    hottest = hottest.max(st.temperature_k[0]);
                }
                if p.crashed || alt < 15_000.0 {
                    return (p.crashed, alt, hottest);
                }
            }
            let p = u.powered(v).unwrap();
            (p.crashed, p.r.length() - 600_000.0, hottest)
        };
        let (lost, alt, hottest) = run(false);
        assert!(
            lost && alt > 15_000.0,
            "the bare pod burns up high: {alt} m, pod at {hottest} K"
        );
        let (lost, alt, hottest) = run(true);
        assert!(
            !lost,
            "the shielded pod comes through: {alt} m, pod at {hottest} K"
        );
        assert!(hottest < 850.0, "{hottest} K");
    }

    #[test]
    fn a_rover_drives_steers_and_brakes_on_its_wheels() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts
            .iter()
            .find(|c| c.name == "Inquisitive Rover")
            .unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Rover", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        u.step();
        let p0 = u.powered(v).unwrap().clone();
        assert!(p0.landed && !p0.tipped);
        assert_eq!(p0.drive.wheels.len(), 6, "six motor wheels");
        assert_eq!(
            p0.drive.forward,
            [0.0, 1.0],
            "the probe core points along +Z"
        );
        let ground = |u: &Universe| {
            // position over the ground, taking out the body's turn
            let p = u.powered(v).unwrap();
            let body_e = u.body_entity("Kerbin").unwrap();
            let b = u.world.get::<bodies::Body>(body_e).unwrap();
            let (lat, lon) = flight::lat_lon(b, p.r, u.ut());
            (lat, lon)
        };
        let (lat0, lon0) = ground(&u);
        // full drive for 10 s
        assert!(u.set_drive(v, 1.0, 0.0, false));
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).unwrap().clone();
        assert!(p.landed && !p.crashed);
        assert!(
            p.drive.speed_mps > 5.0 && p.drive.speed_mps < 12.0,
            "{} m/s",
            p.drive.speed_mps
        );
        let (lat1, lon1) = ground(&u);
        let moved = ((lat1 - lat0).powi(2) + (lon1 - lon0).powi(2))
            .sqrt()
            .to_radians()
            * 600_000.0;
        assert!(moved > 40.0, "drove {moved} m");
        // heading north: latitude grows
        assert!(lat1 > lat0, "{lat0} -> {lat1}");
        assert!(p.drive.powered > 0.9, "the batteries feed the motors");
        // steering turns it
        u.set_drive(v, 1.0, 1.0, false);
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        let turned = u.powered(v).unwrap().drive.heading_rad;
        assert!(turned > 0.3 && turned < 6.0, "turned right to {turned} rad");
        // brakes stop it
        u.set_drive(v, 0.0, 0.0, true);
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
        }
        assert_eq!(
            u.powered(v).unwrap().drive.speed_mps,
            0.0,
            "braked to a stop"
        );
    }

    #[test]
    fn maneuver_hold_points_at_the_burn_roll_turns_and_abort_cuts_and_arms() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        // Control authority comes from the attached part actuators.
        u.set_powered(v, p);
        u.set_structure(v, st);
        // a pure normal burn in a minute
        u.schedule_burn(
            v,
            vessels::ScheduledBurn {
                ut: u.ut() + 60.0,
                prograde_mps: 0.0,
                normal_mps: 100.0,
                radial_mps: 0.0,
            },
        );
        // a whisper of throttle keeps it under power (a coast goes back on rails)
        assert!(u.set_controls(v, 0.01, flight::Attitude::Maneuver(DVec3::ZERO)));
        assert!(u.set_roll(v, 1.0));
        for _ in 0..(20 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).unwrap().clone();
        let flight::Attitude::Maneuver(d) = p.attitude else {
            panic!("{:?}", p.attitude)
        };
        let normal = p.r.cross(p.v).normalized();
        assert!(d.dot(normal) > 0.99, "points along the burn: {d:?}");
        assert!(p.facing.dot(normal) > 0.9, "turned to it: {:?}", p.facing);
        assert!(
            (p.roll_rate - flight::ROLL_RATE_RAD_S).abs() < 0.05,
            "roll input reaches the commanded rate: {}",
            p.roll_rate
        );
        // groups
        assert_eq!(u.action_group(v, ActionGroup::Lights, None), Some(true));
        assert_eq!(u.action_group(v, ActionGroup::Lights, None), Some(false));
        assert_eq!(
            u.action_group(v, ActionGroup::Custom(10), Some(true)),
            Some(true)
        );
        assert_eq!(u.action_group(v, ActionGroup::Custom(11), Some(true)), None);
        u.set_controls(v, 1.0, flight::Attitude::OrbitPrograde);
        assert_eq!(
            u.action_group(v, ActionGroup::Abort, Some(true)),
            Some(true)
        );
        let p = u.powered(v).unwrap();
        assert_eq!(p.throttle, 0.0);
        assert!(p.groups.custom[9] && p.groups.abort);
    }

    #[test]
    fn a_decoupler_pushes_the_stage_away() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_vessel_in_circular_orbit("Start", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        u.set_powered(v, p);
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        let mut debris = None;
        for _ in 0..4 {
            let before = u.powered(v).unwrap().v;
            let r = u.stage(v);
            if let Some(d) = r.debris {
                debris = Some((d, before));
                break;
            }
        }
        let (d, before) = debris.expect("the tank and engine drop");
        let after = u.powered(v).unwrap().v;
        let dv_ship = (after - before).dot(DVec3::new(0.0, 1.0, 0.0));
        let dv_debris = (u.powered(d).unwrap().v - before).dot(DVec3::new(0.0, 1.0, 0.0));
        // a 1.25 m decoupler (ejectionForce 50): 0.5 kN s each way
        assert!(dv_ship > 0.1 && dv_ship < 2.0, "pod pushed {dv_ship} m/s");
        assert!(dv_debris < -0.1, "stage pushed back {dv_debris} m/s");
    }

    #[test]
    fn rcs_translates_the_k2_on_monopropellant_only_with_the_group_on() {
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Kerbal K2").unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let run = |rcs_on: bool| -> (f64, f64) {
            let (mut u, body, _, _) = k1_on_real_kerbin();
            let v = u.spawn_vessel_in_circular_orbit("K2", body, 100_000.0);
            let mut p = flight::Powered::new(
                spec.clone(),
                DVec3::new(700_000.0, 0.0, 0.0),
                DVec3::new(0.0, 2_246.0, 0.0),
                false,
            );
            p.facing = DVec3::new(1.0, 0.0, 0.0);
            p.attitude = flight::Attitude::Fixed(DVec3::new(1.0, 0.0, 0.0));
            u.set_powered(v, p);
            u.set_structure(v, structure::Structure::build(c, &defs, &res));
            u.action_group(v, ActionGroup::Rcs, Some(rcs_on));
            let mono = |u: &Universe| -> f64 {
                let st = u.structure(v).unwrap();
                let m = st.net.resource("MonoPropellant").unwrap();
                st.net
                    .tanks
                    .iter()
                    .filter(|t| t.resource == m)
                    .map(|t| t.stored)
                    .sum()
            };
            let m0 = mono(&u);
            // fore along the nose (radially out here) for 5 s
            assert!(u.set_translate(v, DVec3::new(0.0, 1.0, 0.0)));
            let r0 = u.powered(v).unwrap().r;
            let v0 = u.powered(v).unwrap().v;
            for _ in 0..(5 * TICKS_PER_SECOND) {
                u.step();
            }
            let p = u.powered(v).unwrap();
            // radial speed gained (gravity pulls both runs the same)
            let gained = (p.v - v0).dot(r0.normalized());
            (gained, m0 - mono(&u))
        };
        let (dv_on, used_on) = run(true);
        let (dv_off, used_off) = run(false);
        assert!(dv_on > dv_off + 0.2, "RCS pushed: {dv_on} vs {dv_off} m/s");
        assert!(
            used_on > 0.0 && used_off == 0.0,
            "monoprop {used_on} / {used_off}"
        );
    }

    #[test]
    fn a_jet_lies_level_on_the_runway_rolls_rotates_and_flies() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let c = structure::level_layout(c).expect("built nose-forward");
        assert!(
            structure::level_layout(crafts.iter().find(|c| c.name == "Kerbal K1").unwrap())
                .is_none(),
            "rockets already point up"
        );
        let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Jet", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(&c, &defs, &res));
        assert!(u.set_level(v, 90.0));
        u.step();
        let p = u.powered(v).unwrap().clone();
        let up = p.r.normalized();
        assert!(p.landed && !p.tipped && !p.crashed, "{p:?}");
        assert!(p.facing.dot(up).abs() < 0.05, "lies level: {:?}", p.facing);
        assert!(
            p.ground_clearance_m < 3.0,
            "on its wheels: {} m",
            p.ground_clearance_m
        );
        assert!(!p.drive.wheels.is_empty(), "rolls on its gear");
        assert!(p.jet.is_some() && p.intakes);
        let alt0 = p.r.length() - 600_000.0;
        // full throttle, nose up 10 degrees: it rolls, rotates and lifts off
        u.stage(v);
        u.set_controls(
            v,
            1.0,
            flight::Attitude::PitchHeading {
                pitch_deg: 10.0,
                heading_deg: 90.0,
            },
        );
        let mut lifted_at = None;
        for i in 0..(90 * TICKS_PER_SECOND) {
            u.step();
            let p = u.powered(v).unwrap();
            assert!(!p.crashed, "crashed at step {i}");
            if !p.landed && lifted_at.is_none() {
                lifted_at = Some((i, p.drive.speed_mps));
            }
            if lifted_at.is_some() && p.r.length() - 600_000.0 > alt0 + 100.0 {
                break;
            }
        }
        let (step, speed) = lifted_at.expect("took off");
        let p = u.powered(v).unwrap();
        assert!(
            p.r.length() - 600_000.0 > alt0 + 100.0,
            "climbing: {} m above the runway {p:?}",
            p.r.length() - 600_000.0 - alt0
        );
        assert!(
            speed > 20.0,
            "lifted off at {speed} m/s after {} s",
            step as f64 / 50.0
        );
    }

    #[test]
    fn with_sas_off_the_keys_spin_the_craft_and_it_keeps_spinning() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        // a moment of inertia and reaction wheels to turn with
        for s in &mut p.spec.stages {
            s.moi_tm2 = 20.0;
            s.wheel_torque_knm = 5.0;
        }
        u.set_powered(v, p);
        u.set_structure(v, st);
        // a whisper of throttle keeps it under power
        assert!(u.set_controls(v, 0.01, flight::Attitude::Free));
        let f0 = u.powered(v).unwrap().facing;
        // pitch for two seconds, then let go
        assert!(u.set_rotation(v, 1.0, 0.0, 0.5));
        for _ in 0..(2 * TICKS_PER_SECOND) {
            u.step();
        }
        let p1 = u.powered(v).unwrap().clone();
        assert!(p1.angvel.length() > 0.01, "spinning: {:?}", p1.angvel);
        assert!(p1.roll_rate > 0.01, "rolling: {}", p1.roll_rate);
        let momentum = |p: &flight::Powered, st: &structure::Structure| {
            let (x, y, z) = flight::vessel_basis(p);
            let w = p.angvel + p.facing * p.roll_rate;
            let l = st
                .mass_properties()
                .inertia
                .apply(DVec3::new(w.dot(x), w.dot(y), w.dot(z)));
            x * l.x + y * l.y + z * l.z
        };
        let l1 = momentum(&p1, u.structure(v).unwrap());
        u.world.get_mut::<flight::Powered>(v).unwrap().ignited = false;
        u.set_rotation(v, 0.0, 0.0, 0.0);
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        let p2 = u.powered(v).unwrap().clone();
        assert!(
            (momentum(&p2, u.structure(v).unwrap()) - l1).length() < 1e-6,
            "angular momentum is conserved during a torque-free tumble"
        );
        assert!(
            p2.facing.dot(f0) < p1.facing.dot(f0),
            "and it keeps turning"
        );
        // SAS back on: it stops and holds
        let hold = p2.facing;
        u.world.get_mut::<flight::Powered>(v).unwrap().ignited = true;
        u.set_controls(v, 0.01, flight::Attitude::Fixed(hold));
        for _ in 0..(20 * TICKS_PER_SECOND) {
            u.step();
        }
        let p3 = u.powered(v).unwrap();
        assert!(p3.angvel.length() < 0.01, "{:?}", p3.angvel);
        assert!(p3.facing.dot(hold) > 0.99, "holds where SAS caught it");
    }

    #[test]
    fn a_target_hold_points_at_the_other_vessel_and_reads_its_distance() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        // Orbiting upper stage: ignited solid launch boosters cannot hold 1% throttle.
        p.stage = st
            .stage_engines
            .iter()
            .position(|engines| {
                !engines.is_empty()
                    && engines
                        .iter()
                        .all(|&i| st.part_ids[i].starts_with("engine_"))
            })
            .unwrap();
        u.set_powered(v, p);
        u.set_structure(v, st);
        // a probe 2 km ahead on the same orbit, drifting apart at 1 m/s
        let other = u.spawn_vessel_in_circular_orbit("Probe", body, 100_000.0);
        let mut q = u.powered(v).unwrap().clone();
        q.r = DVec3::new(700_000.0 - 2.86, 2_000.0, 0.0);
        q.v = DVec3::new(0.0, 2_247.0, 0.0);
        u.set_powered(other, q);
        assert!(!u.set_target(v, Some(v)), "not itself");
        assert!(u.set_target(v, Some(other)));
        let (d, rel, dist) = u.target_relative(v).unwrap();
        assert!((dist - 2_000.0).abs() < 5.0, "{dist}");
        assert!((rel.length() - 1.0).abs() < 0.05, "{rel:?}");
        assert!(d.y > 1_990.0);
        assert!(u.set_controls(v, 0.01, flight::Attitude::Target(DVec3::ZERO)));
        for _ in 0..(20 * TICKS_PER_SECOND) {
            u.step();
        }
        let (d, _, _) = u.target_relative(v).unwrap();
        let p = u.powered(v).unwrap();
        assert!(
            p.facing.dot(d.normalized()) > 0.99,
            "points at it: {:?}",
            p.facing
        );
        // a body as target: the distance is to its surface
        assert!(u.set_target(v, Some(body)));
        let (_, _, dist) = u.target_relative(v).unwrap();
        assert!((dist - 100_000.0).abs() < 2_000.0, "{dist}");
        assert!(u.set_target(v, None));
        assert!(u.target_relative(v).is_none());
    }

    #[test]
    fn the_sun_rises_over_the_ksc_once_a_kerbin_day() {
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let u = Universe::with_bodies(&bodies);
        let kerbin = u.body_entity("Kerbin").unwrap();
        let (lat, lon) = (-0.0972, -74.5577);
        let t = u.next_morning_ut(kerbin, lat, lon, 15.0, 0.0);
        assert!(t > 0.0 && t < 21_549.425 * 1.01, "{t}");
        let e = u.sun_elevation_deg(kerbin, lat, lon, t);
        assert!((14.0..17.0).contains(&e), "{e}");
        // rising: higher a little later, and night half a day on
        assert!(u.sun_elevation_deg(kerbin, lat, lon, t + 600.0) > e);
        assert!(u.sun_elevation_deg(kerbin, lat, lon, t + 21_549.425 * 0.5) < 0.0);
    }

    #[test]
    fn the_starter_rocket_flies_up_and_comes_down_on_its_parachute() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Start", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        u.stage(v);
        // burn out, drop the tank and engine, then arm the chute
        for _ in 0..(200 * TICKS_PER_SECOND) {
            u.step();
            if u.powered(v).is_some_and(|p| p.propellant_t <= 1e-6) {
                break;
            }
        }
        assert!(u.stage(v).debris.is_some(), "tank and engine drop");
        u.stage(v);
        let mut top: f64 = 0.0;
        let mut landing_speed = 0.0;
        for _ in 0..(1_200 * TICKS_PER_SECOND) {
            let before = u.powered(v).unwrap().clone();
            u.step();
            let p = u.powered(v).unwrap();
            top = top.max(p.r.length() - 600_000.0);
            if p.landed {
                landing_speed = (before.v
                    - DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425)
                        .cross(before.r))
                .length();
                break;
            }
        }
        u.step();
        let p = u.powered(v).unwrap();
        let st = u.structure(v).unwrap();
        assert!(top > 1_000.0, "flew to {top} m");
        assert!(p.landed, "came down: {p:?}");
        assert!(
            !p.crashed,
            "the parachute saved it ({landing_speed:.1} m/s)"
        );
        assert!(landing_speed < 12.0, "{landing_speed}");
        assert!(
            st.chutes.contains(&structure::Chute::Cut),
            "cut away after landing"
        );
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
        assert!(
            u.stage(v).debris.is_none(),
            "the first press lights the engines"
        );
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
        // (at Mach 9 this low everything would burn up first)
        u.set_heat_settings(thermal::HeatSettings {
            enabled: false,
            ..Default::default()
        });
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

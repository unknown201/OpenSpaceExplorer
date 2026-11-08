//! Deterministic simulation, run natively by the server (ADR-0002).
//!
//! State lives in a `bevy_ecs` `World`. Time is an integer tick count (`Tick`), `TICKS_PER_SECOND` ticks per
//! second of universal time. No I/O, no wall clock, no randomness without an explicit seed.

#![forbid(unsafe_code)]

pub mod actuators;
pub mod aero;
pub mod bodies;
pub mod campus;
pub mod collide;
pub mod commnet;
pub mod contact;
pub mod crew;
pub mod docking;
pub mod dynamics;
pub mod exhaust;
pub mod flight;
pub mod jetpack;
pub mod missions;
pub mod resources;
pub mod rigid;
pub mod sas;
pub mod science;
pub mod structure;
pub mod struts;
pub mod surface;
pub mod thermal;
pub mod trajectory;
pub mod vessels;
pub mod water;
pub mod wheels;

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

/// The star's luminosity, W (`CelestialBodyComponent.StarLuminosity`): solar panels make their rating where
/// it gives `SOLAR_INSOLATION_AT_HOME` W/m^2.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct StarLuminosity(pub f64);

/// `PhysicsSettings.SolarInsolationAtHome`, W/m^2: the starlight a solar panel's rating is for.
pub const SOLAR_INSOLATION_AT_HOME: f64 = 1360.0;

/// Time warp factor granted by the server (1 = real time).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarpFactor(pub u32);

impl Default for WarpFactor {
    fn default() -> Self {
        WarpFactor(1)
    }
}

/// The part catalog, when the server has provided one (`Universe::set_part_catalog`).
#[derive(Resource)]
pub struct PartCatalog {
    pub defs: Vec<ose_data::PartDef>,
    pub resources: Vec<ose_data::ResourceDef>,
}

/// What was burning when a stage came off: parent part indices, the throttle and the tanks.
pub struct DebrisFire {
    burning: Vec<usize>,
    throttle: f64,
    tanks: Vec<resources::Tank>,
}

/// What `Universe::stage` did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageResult {
    pub fired: bool,
    /// The separated stage, now its own vessel.
    pub debris: Option<Entity>,
}

/// The whole universe.
/// How long a crashed craft stays in the world after the crash, s of game time.
pub const WRECK_LINGER_S: f64 = 30.0;

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

/// A fixed pseudo-random unit-ish vector (each component -1..1) for wreck scatter: the same crash throws
/// the same way.
fn scatter(seed: u64, k: u64) -> DVec3 {
    let mut x = seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    DVec3::new(next(), next(), next())
}

/// `parts` (indices into `craft`) split into the pieces they hold together as, over the craft's tree links.
fn connected_groups(craft: &ose_data::craft::Craft, parts: &[usize]) -> Vec<Vec<usize>> {
    let inside = |i: usize| parts.contains(&i);
    let mut group_of: Vec<Option<usize>> = vec![None; craft.parts.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &i in parts {
        // climb to the topmost part of the set above it
        let mut top = i;
        while let Some(par) = craft.parts.get(top).and_then(|p| p.parent) {
            if !inside(par) {
                break;
            }
            top = par;
        }
        let g = match group_of[top] {
            Some(g) => g,
            None => {
                groups.push(Vec::new());
                group_of[top] = Some(groups.len() - 1);
                groups.len() - 1
            }
        };
        groups[g].push(i);
    }
    groups
}

/// Steps since the universe started, for the CommNet rebuild timer.
#[derive(Resource, Clone, Copy, Debug, Default)]
struct CommNetSteps(u64);

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
        if let Some(l) = defs
            .iter()
            .find(|d| d.parent.is_none())
            .and_then(|d| d.star_luminosity)
            .filter(|&l| l > 0.0)
        {
            u.world.insert_resource(StarLuminosity(l));
        }
        u
    }

    /// Starlight on a solar panel at a point `r` from `body` (body-centred inertial), as a share of a panel's
    /// rating (`PartComponentModule_SolarPanel.CalculateVesselSolarLOS`): the star's flux there,
    /// luminosity / (4 pi d^2), over `SolarInsolationAtHome` (about 0.97 at Kerbin), and 0 when `body` itself
    /// is in the way (`CalculateOcclusionPerStar` tests only the body the craft is at: a moon's planet casts no
    /// shadow on it).
    pub fn sunlight(&self, body: Entity, r: DVec3, ut: f64) -> f64 {
        let Some(root) = self.world.resource::<bodies::BodyIndex>().root else {
            return 1.0;
        };
        let p = bodies::star_space_state(&self.world, body, ut).0 + r;
        let d = p.length();
        if d <= 0.0 || body == root && r.length() <= 0.0 {
            return 0.0;
        }
        if body != root {
            if let Some(radius) = self.world.get::<bodies::Body>(body).map(|x| x.radius_m) {
                // v: body to star, w: craft to body
                let v = bodies::star_space_state(&self.world, body, ut).0 * -1.0;
                let w = r * -1.0;
                let near = w.length_squared() - radius * radius;
                let vw = v.dot(w);
                if vw > near && vw * vw / v.length_squared() > near {
                    return 0.0;
                }
            }
        }
        if let Some(l) = self.world.get_resource::<StarLuminosity>() {
            return l.0 / (4.0 * std::f64::consts::PI * d * d) / SOLAR_INSOLATION_AT_HOME;
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
        if factor > flight::MAX_PHYSICS_WARP {
            let ut = self.ut();
            for e in self.vessels() {
                let Some(p) = self.powered(e) else { continue };
                let Some(body) = self.body_of(e) else {
                    continue;
                };
                // a vessel flying a planned burn stays under power
                if p.crashed
                    || self.world.get::<vessels::ExecutingBurn>(e).is_some()
                    || p.landed
                    || p.throttle > 0.0
                    || p.coasting_since.is_none()
                    || p.rotation_input.length_squared() > 0.0
                    || p.roll_input != 0.0
                    || p.rcs.translate.length_squared() > 0.0
                    || p.manual_steering
                    || p.r.length() <= self.rails_clear_radius(body)
                {
                    continue;
                }
                let p = flight::packed(p.clone());
                self.world
                    .entity_mut(e)
                    .remove::<flight::Powered>()
                    .insert(flight::Dormant(p));
                let index = self.world.resource::<bodies::BodyIndex>().clone();
                vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
            }
        }
        let factor = match self.warp_cap() {
            Some(cap) => factor.min(cap),
            None => factor,
        };
        self.world.resource_mut::<WarpFactor>().0 = factor.max(1);
    }

    /// Margin above air and terrain where analytic coasting must return to physics.
    fn rails_clear_radius(&self, body: Entity) -> f64 {
        let b = self.world.get::<bodies::Body>(body).expect("body");
        b.radius_m
            + self.atmosphere(body).map(|a| a.depth_m).unwrap_or(0.0).max(
                self.world
                    .get::<flight::Terrain>(body)
                    .map(|t| t.0.max_m.max(0.0))
                    .unwrap_or(0.0),
            )
            + 1_000.0
    }

    fn rails_reentry(&self, e: Entity) -> Option<f64> {
        self.dormant(e)?;
        let (_, body, orbit) = self.vessel(e)?;
        let radius = self.rails_clear_radius(body);
        let ut = self.ut();
        if orbit.position_at(ut).length() <= radius + 1.0 {
            return Some(ut);
        }
        if orbit.periapsis() >= radius {
            return None;
        }
        let nu = -orbit.true_anomaly_at_radius(radius);
        let dt = orbit.time_to_true_anomaly(nu, ut);
        (dt.is_finite() && dt >= 0.0).then_some(ut + dt)
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

    pub fn set_raw_rotation(&mut self, e: Entity, input: DVec3) -> bool {
        if ![input.x, input.y, input.z]
            .iter()
            .all(|v| v.is_finite() && (-1.0..=1.0).contains(v))
        {
            return false;
        }
        if input.length_squared() > 0. && self.warp() <= flight::MAX_PHYSICS_WARP {
            self.wake(e);
        }
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            p.raw_rotation_input = Some(input);
            true
        } else if let Some(mut p) = self.world.get_mut::<flight::Dormant>(e) {
            p.0.raw_rotation_input = Some(input);
            true
        } else {
            false
        }
    }

    pub fn set_precision(&mut self, e: Entity, enabled: bool) -> bool {
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            p.rcs.precision = enabled;
            true
        } else if let Some(mut p) = self.world.get_mut::<flight::Dormant>(e) {
            p.0.rcs.precision = enabled;
            true
        } else {
            false
        }
    }

    pub fn configure_part(
        &mut self,
        e: Entity,
        part: u16,
        settings: ose_data::flight_geometry::PartControl,
    ) -> bool {
        if !settings.valid()
            || !self
                .structure(e)
                .is_some_and(|s| s.rigid.present.get(part as usize) == Some(&true))
        {
            return false;
        }
        let update = |p: &mut flight::Powered| {
            p.part_controls.retain(|(i, _)| *i != part);
            p.part_controls.push((part, settings));
            p.sas.reset();
        };
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            update(&mut p);
            true
        } else if let Some(mut p) = self.world.get_mut::<flight::Dormant>(e) {
            update(&mut p.0);
            true
        } else {
            false
        }
    }

    pub fn set_control_point(&mut self, e: Entity, part: u16) -> bool {
        let Some(s) = self.structure(e) else {
            return false;
        };
        let i = part as usize;
        if !s.rigid.present.get(i).copied().unwrap_or(false) || !s.can_control_from(i) {
            return false;
        }
        let q = s.control_rotation(i);
        self.wake(e);
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            p.control_point = Some(part);
            p.control_rotation = q;
            p.sas.reset();
            if p.attitude != flight::Attitude::Free {
                p.attitude = flight::Attitude::Fixed(flight::control_basis(&p).1);
            }
            true
        } else {
            false
        }
    }

    /// Rotation input with SAS off (pitch, yaw, roll each -1..1, held until changed; see
    /// `Powered::rotation_input`); false without a powered vessel.
    pub fn set_rotation(&mut self, e: Entity, pitch: f64, yaw: f64, roll: f64) -> bool {
        if (pitch != 0.0 || yaw != 0.0 || roll != 0.0) && self.warp() <= flight::MAX_PHYSICS_WARP {
            self.wake(e);
        }
        let steering = pitch != 0.0 || yaw != 0.0 || roll != 0.0;
        // steering by hand takes over from a planned burn being flown: hold where it points now
        if steering && self.stop_flying_burn(e) {
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                p.attitude = flight::Attitude::Fixed(flight::control_basis(&p).1);
                p.sas.reset();
            }
        }
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                p.rotation_input = DVec3::new(pitch, yaw, roll);

                true
            }
            None => false,
        }
    }

    /// Stop flying the planned burn under way, if any, and close the throttle it was driving. The node stays
    /// planned. True if a burn was being flown.
    fn stop_flying_burn(&mut self, e: Entity) -> bool {
        if self.world.get::<vessels::ExecutingBurn>(e).is_none() {
            return false;
        }
        self.world.entity_mut(e).remove::<vessels::ExecutingBurn>();
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            p.throttle = 0.0;
        }
        true
    }

    /// The direction the vessel's control axis points (its nose), for a hold on the current heading.
    pub fn facing(&self, e: Entity) -> Option<DVec3> {
        self.powered(e)
            .or_else(|| self.dormant(e))
            .map(|p| flight::control_basis(p).1)
    }

    /// A Kerbal on EVA runs (held) and jumps (once, from the ground); false for anything else.
    pub fn set_eva(&mut self, e: Entity, run: bool, jump: bool) -> bool {
        // Space lets go of a ladder
        if jump && self.world.get::<crew::OnLadder>(e).is_some() {
            self.world.entity_mut(e).remove::<crew::OnLadder>();
            return true;
        }
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) if p.walker => {
                p.eva_run = run;
                p.eva_jump |= jump && p.landed;
                true
            }
            _ => false,
        }
    }

    /// Control trim for a vessel (pitch, yaw, roll each -1..1, held); false without a powered vessel.
    pub fn set_trim(&mut self, e: Entity, trim: DVec3) -> bool {
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                p.trim = DVec3::new(
                    trim.x.clamp(-1.0, 1.0),
                    trim.y.clamp(-1.0, 1.0),
                    trim.z.clamp(-1.0, 1.0),
                );
                true
            }
            None => false,
        }
    }

    /// Ask for (or stop) a vessel's per-part lift and drag in its readout; false without a powered vessel.
    pub fn set_aero_overlay(&mut self, e: Entity, on: bool) -> bool {
        match self.world.get_mut::<flight::Powered>(e) {
            Some(mut p) => {
                p.aero_overlay = on;
                true
            }
            None => false,
        }
    }

    /// Roll input for a vessel (-1..1, held until changed); false without a powered vessel.
    pub fn set_roll(&mut self, e: Entity, input: f64) -> bool {
        if input != 0.0 && self.warp() <= flight::MAX_PHYSICS_WARP {
            self.wake(e);
        }
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
        // and the part actions bound to the group
        let mask = match group {
            ActionGroup::Lights => 0x4,
            ActionGroup::Rcs => 0x8,
            ActionGroup::Abort => 0x40,
            ActionGroup::Custom(n) => ose_data::craft::custom_group_bit(n),
        };
        if let Some(mut st) = self.world.get_mut::<structure::Structure>(e) {
            st.run_action_groups(mask, state);
        }
        Some(state)
    }

    /// Bind a part's actions to action groups in flight (replacing its bindings); they go with the craft.
    pub fn set_part_actions(
        &mut self,
        e: Entity,
        part: usize,
        actions: Vec<(String, u32)>,
    ) -> Result<(), &'static str> {
        if actions
            .iter()
            .any(|(a, _)| !ose_data::craft::ACTIONS.contains(&a.as_str()))
        {
            return Err("unknown action");
        }
        let mut st = self
            .world
            .get_mut::<structure::Structure>(e)
            .ok_or("vessel has no parts")?;
        if part >= st.part_actions.len() || !st.rigid.present[part] {
            return Err("no such part");
        }
        st.part_actions[part] = actions.clone();
        if let Some(mut doc) = self.world.get_mut::<docking::CraftDoc>(e) {
            if let Some(p) = doc.0.parts.get_mut(part) {
                p.actions = actions;
            }
        }
        Ok(())
    }

    /// Change a parachute's deploy mode, opening pressure and full-deploy height in flight; the rest of its
    /// symmetry set follows (`KSPState(CopyToSymmetrySet)`), and the settings go with the craft.
    pub fn set_chute_settings(
        &mut self,
        e: Entity,
        part: usize,
        settings: ose_data::craft::ChuteSettings,
    ) -> Result<(), &'static str> {
        let settings = settings.stepped();
        // The part and the rest of its symmetry set, from the craft as flown.
        let parts: Vec<usize> = match self.world.get::<docking::CraftDoc>(e) {
            Some(d) => match d.0.parts.get(part).map(|p| p.symmetry).filter(|s| *s != 0) {
                Some(s) => (0..d.0.parts.len())
                    .filter(|&k| d.0.parts[k].symmetry == s)
                    .collect(),
                None => vec![part],
            },
            None => vec![part],
        };
        let mut st = self
            .world
            .get_mut::<structure::Structure>(e)
            .ok_or("vessel has no parts")?;
        if st.chute_defs.get(part).is_none_or(|c| c.is_none()) || !st.rigid.present[part] {
            return Err("that part is not a parachute");
        }
        for &k in &parts {
            if let Some(Some(c)) = st.chute_defs.get_mut(k) {
                c.min_pressure_atm = settings.min_pressure_atm as f64;
                c.deploy_altitude_m = settings.deploy_altitude_m as f64;
                st.chute_modes[k] = settings.mode;
            }
        }
        if let Some(mut doc) = self.world.get_mut::<docking::CraftDoc>(e) {
            for &k in &parts {
                if let Some(p) = doc.0.parts.get_mut(k) {
                    p.chute = Some(settings);
                }
            }
        }
        Ok(())
    }

    /// Run one part's action now (its right-click menu); false when it does not apply to that part.
    pub fn part_action(&mut self, e: Entity, part: usize, action: &str, on: bool) -> bool {
        self.world
            .get_mut::<structure::Structure>(e)
            .is_some_and(|mut st| st.part_action(part, action, on))
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
    /// Stand a rocket on its pad as KSP2 does: rolled so its top faces south (the navball reads heading 180),
    /// so D yaws it east and W tips it north. Not for level craft (planes keep their own frame).
    pub fn stand_south(&mut self, e: Entity) -> bool {
        let Some(mut p) = self.world.get_mut::<flight::Powered>(e) else {
            return false;
        };
        let (_, _, north) = flight::surface_frame(p.r);
        let south = -north - p.facing * (-north).dot(p.facing);
        if south.length_squared() < 1e-6 {
            return false;
        }
        // As a roll from the reference (north): the landed vessel keeps its roll, not a free top.
        flight::set_vessel_top(&mut p, south.normalized());
        // Standing on the ground the craft shows its drive heading's roll: face that south too, or it would
        // stand rolled the other way until lift-off and turn round as it leaves the pad.
        p.drive.heading_rad = std::f64::consts::PI;
        let mut p = p.clone();
        if let Some(mut st) = self.world.entity_mut(e).take::<structure::Structure>() {
            st.sync(&mut p);
            self.world.entity_mut(e).insert(st);
        }
        if let Some(mut q) = self.world.get_mut::<flight::Powered>(e) {
            *q = p;
        }
        true
    }

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

    /// Extend or retract a vessel's deployable antennas; how many moved, `None` without a structure.
    pub fn deploy_antennas(&mut self, e: Entity, out: bool) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.deploy_antennas(out))
    }

    /// Rename a vessel; false for anything that is not one.
    pub fn rename_vessel(&mut self, e: Entity, name: &str) -> bool {
        match self.world.get_mut::<vessels::Vessel>(e) {
            Some(mut v) => {
                v.name = name.to_string();
                true
            }
            None => false,
        }
    }

    /// Fold a vessel's deployable solar panels out or in; how many moved, `None` without a structure.
    pub fn deploy_panels(&mut self, e: Entity, out: bool) -> Option<usize> {
        self.world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.deploy_panels(out))
    }

    /// Whether any vessel is flying under power (time warp is then capped at physics warp). Vessels standing
    /// on the ground with the throttle at zero, and wrecks, do not count.
    /// The vessels holding time warp to physics speeds now (see `has_powered`).
    pub fn warp_holders(&mut self) -> Vec<Entity> {
        let mut q = self.world.query::<(Entity, &flight::Powered)>();
        q.iter(&self.world)
            .filter(|(_, p)| !p.crashed && !flight::resting(p))
            .map(|(e, _)| e)
            .collect()
    }

    /// Burning its engines in orbit, clear of air and terrain, and flown by nothing else (no rotation, RCS or
    /// planned burn): KSP2 lets such a craft keep burning under time warp, its thrust held in one inertial
    /// direction (`HandleOrbitalPhysicsUnderThrustStart`).
    pub fn burning_in_orbit(&self, e: Entity) -> bool {
        let (Some(p), Some(body)) = (self.world.get::<flight::Powered>(e), self.body_of(e)) else {
            return false;
        };
        !p.crashed
            && !p.landed
            && !p.splashed
            && p.throttle > 0.0
            && p.propellant_t > 1e-6
            && p.current_stage().is_some_and(|s| s.thrust_vac_kn > 0.0)
            && p.rotation_input.length_squared() == 0.0
            && p.roll_input == 0.0
            && p.rcs.translate.length_squared() == 0.0
            && !p.manual_steering
            && self.world.get::<vessels::ExecutingBurn>(e).is_none()
            && p.r.length() > self.rails_clear_radius(body)
    }

    /// The fastest time warp the vessels in physics allow: physics speeds while one is flying, up to
    /// `MAX_THRUST_WARP` while the only ones flying burn in orbit, no limit when none is.
    pub fn warp_cap(&mut self) -> Option<u32> {
        let flying: Vec<Entity> = {
            let mut q = self.world.query::<(Entity, &flight::Powered)>();
            q.iter(&self.world)
                .filter(|(_, p)| !p.crashed && !flight::resting(p))
                .map(|(e, _)| e)
                .collect()
        };
        if flying.is_empty() {
            None
        } else if flying.iter().all(|&e| self.burning_in_orbit(e)) {
            // KSP2 drops a warped burn to 1x when its engines will run dry within the time it takes to slow
            // down from that rate plus 10 s (`ClampRateToOrbitTransitions`: (1 + rate) / 2 + 10 s)
            let dry_in = flying
                .iter()
                .filter_map(|&e| self.world.get::<flight::Powered>(e))
                .map(|p| {
                    let s = p.current_stage();
                    let flow = s
                        .filter(|s| s.isp_vac_s > 0.0)
                        .map(|s| s.thrust_vac_kn * p.throttle / (s.isp_vac_s * ose_data::G0))
                        .unwrap_or(0.0);
                    if flow > 0.0 {
                        p.propellant_t / flow
                    } else {
                        f64::INFINITY
                    }
                })
                .fold(f64::INFINITY, f64::min);
            Some(
                [100, 50, 10, 4, 2]
                    .into_iter()
                    .find(|&r| {
                        r <= flight::MAX_THRUST_WARP && dry_in > (1.0 + r as f64) / 2.0 + 10.0
                    })
                    .unwrap_or(1),
            )
        } else {
            Some(flight::MAX_PHYSICS_WARP)
        }
    }

    /// A vessel in physics that holds time warp to physics speeds: not a wreck, and not at rest on the
    /// ground or the sea (`flight::resting`).
    pub fn has_powered(&mut self) -> bool {
        let mut q = self.world.query::<&flight::Powered>();
        q.iter(&self.world)
            .any(|p| !p.crashed && !flight::resting(p))
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
        } else {
            let members = self.crew(e);
            self.set_crew(e, members);
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
        self.world
            .entity_mut(e)
            .insert(flight::Dormant(flight::packed(p)));
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
        let top = flight::vessel_top(&p);
        p.r = r;
        p.v = v;
        flight::set_vessel_top(&mut p, top);
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
        // any hold but the maneuver one takes over from a planned burn being flown
        if !matches!(attitude, flight::Attitude::Maneuver(_)) {
            self.stop_flying_burn(e);
        }
        if self.world.get::<flight::Powered>(e).is_none() {
            if self.world.get::<flight::Dormant>(e).is_none() {
                return false;
            }
            let turning = self.warp() <= flight::MAX_PHYSICS_WARP
                && self.dormant(e).is_some_and(|p| p.attitude != attitude);
            if throttle <= 0.0 && !turning {
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
                if p.attitude != attitude {
                    p.sas.reset();
                    let cv = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
                    let (_, y, z) = flight::control_basis(&p);
                    let mut q = crate::sas::orientation(cv(y), cv(z));
                    if let flight::Attitude::Fixed(target) = attitude {
                        q = glam::DQuat::from_rotation_arc(cv(y), cv(target).normalize()) * q;
                    }
                    p.sas.set_lock(q);
                    if let flight::Attitude::Fixed(target) = attitude {
                        p.sas.acquiring_fixed_target = y.dot(target) < 0.995;
                    }
                }
                p.manual_steering = false;
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
        // engines burning now, and the propellant they draw on: a piece that comes off keeps both
        let fire = match (
            self.world.get::<flight::Powered>(e),
            self.world.get::<structure::Structure>(e),
        ) {
            (Some(p), Some(st)) if p.ignited && !p.crashed => Some(DebrisFire {
                burning: st.firing_engines(p.stage, p.throttle),
                throttle: p.throttle,
                tanks: st.net.tanks.clone(),
            }),
            _ => None,
        };
        let (outcome, mut r, v, facing, area) =
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
        // what came off, in connected pieces: (parent craft parts, mass t, offset from the vessel m)
        let mut dropped: Vec<(Vec<usize>, f64, DVec3)> = Vec::new();
        if outcome.fired {
            let mut st = self.world.entity_mut(e).take::<structure::Structure>();
            if let Some(st) = &mut st {
                let before = st.net.mass_t();
                let present = st.rigid.present.clone();
                let part_mass: Vec<f64> =
                    (0..present.len()).map(|i| st.net.mass_of(&[i])).collect();
                if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                    // the stage that stays does not move: the vessel's position follows its centre of mass
                    let centre = st.mass_properties().centre_m;
                    st.sync(&mut p);
                    st.keep_in_place(&mut p, centre);
                    r = p.r;
                } else if let Some(mut d) = self.world.get_mut::<flight::Dormant>(e) {
                    st.sync(&mut d.0);
                }
                outcome.dropped_t = (before - st.net.mass_t()).max(0.0);
                let gone: Vec<usize> = (0..present.len())
                    .filter(|&i| present[i] && !st.rigid.present[i])
                    .collect();
                if let Some(doc) = self.world.get::<docking::CraftDoc>(e) {
                    let keep_c = st.rigid.rest_centre();
                    for g in connected_groups(&doc.0, &gone) {
                        let m: f64 = g.iter().map(|&i| part_mass[i]).sum();
                        // where it sat on the craft (the rest pose: the live one turns with the structure)
                        let c = if m > 0.0 {
                            g.iter()
                                .map(|&i| st.rigid.rest_position(i) * part_mass[i])
                                .fold(DVec3::ZERO, |a, b| a + b)
                                * (1.0 / m)
                        } else {
                            st.rigid.rest_position(g[0])
                        };
                        dropped.push((g, m, st.offset_inertial(facing, c - keep_c)));
                    }
                }
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
            if dropped.is_empty() {
                self.spawn_debris(
                    e,
                    outcome.dropped_t,
                    r - facing * 3.0,
                    v - facing * away,
                    facing,
                    area,
                    &[],
                    DVec3::ZERO,
                    None,
                )
            } else {
                let mut first = None;
                for (parts, m, offset) in dropped {
                    // side pieces (radial boosters) are pushed out as well as back, and tip away
                    let side = offset - facing * offset.dot(facing);
                    let out = if side.length() > 0.5 {
                        side.normalized()
                    } else {
                        DVec3::ZERO
                    };
                    let push = if impulse > 0.0 && m > 0.0 {
                        impulse / outcome.dropped_t.max(m)
                    } else {
                        0.5
                    };
                    let tumble = facing.cross(out) * 0.15;
                    let d = self.spawn_debris(
                        e,
                        m,
                        r + offset,
                        v - facing * push + out * (push * 0.5),
                        facing,
                        area,
                        &parts,
                        tumble,
                        fire.as_ref(),
                    );
                    match (first, d) {
                        (None, Some(d)) => first = Some(d),
                        (Some(_), Some(d)) => self.broken_off.push((e, d)),
                        _ => {}
                    }
                }
                first
            }
        } else {
            None
        };
        StageResult {
            fired: outcome.fired,
            debris,
        }
    }

    /// A piece that came off `parent`: its `parts` (indices into the parent's craft) become its own craft,
    /// re-rooted, posed as they were; it keeps the parent's attitude and turns at `angvel`.
    #[allow(clippy::too_many_arguments)]
    fn spawn_debris(
        &mut self,
        parent: Entity,
        mass_t: f64,
        r: DVec3,
        v: DVec3,
        facing: DVec3,
        drag_area_m2: f64,
        parts: &[usize],
        angvel: DVec3,
        fire: Option<&DebrisFire>,
    ) -> Option<Entity> {
        let top = match self.world.get::<flight::Powered>(parent) {
            Some(p) => flight::vessel_top(p),
            None => self
                .world
                .get::<flight::Dormant>(parent)
                .map(|d| flight::vessel_top(&d.0))
                .unwrap_or(DVec3::ZERO),
        };
        let subset = self
            .world
            .get::<docking::CraftDoc>(parent)
            .filter(|_| !parts.is_empty())
            .map(|d| docking::subset(&d.0, parts));
        // A piece is built from its own parts when the catalog is known: it floats, sinks and breaks part
        // by part like any craft, and keeps the propellant it carried. One carrying an engine that was
        // burning keeps burning (KSP2 leaves a separated stage's engines running) at the same throttle.
        let (parent_tanks, parent_wet) = self
            .world
            .get::<structure::Structure>(parent)
            .map(|st| {
                (
                    Some(st.net.tanks.clone()),
                    st.in_water
                        .iter()
                        .copied()
                        .zip(st.water_drag.iter().copied())
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        // Only pieces that need it are built: one still burning, or one in the sea (where it must float);
        // a spent stage or a wreck on land stays a cheap single body.
        let afloat = self
            .world
            .get::<flight::Powered>(parent)
            .is_some_and(|p| p.splashed)
            || parent_wet.iter().any(|w| w.0);
        // a stage dropped from under engines at throttle may be in their exhaust: it is built to take the blast
        let blasted = self
            .world
            .get::<flight::Powered>(parent)
            .is_some_and(|p| p.throttle > 0.0 && !p.crashed)
            && self
                .world
                .get::<structure::Structure>(parent)
                .is_some_and(|st| st.has_exhaust_engines());
        let lit = |map: &[Option<usize>]| {
            fire.is_some_and(|f| f.burning.iter().any(|&i| map[i].is_some()))
        };
        let burning = match (&subset, self.world.get_resource::<PartCatalog>()) {
            (Some((craft, map)), Some(catalog)) if afloat || blasted || lit(map) => {
                let mut st = structure::Structure::build(craft, &catalog.defs, &catalog.resources);
                if let Some(tanks) = fire.map(|f| &f.tanks).or(parent_tanks.as_ref()) {
                    st.carry_tanks_from(tanks, map);
                }
                // parts already in the sea do not strike it again
                for (i, &(wet, drag)) in parent_wet.iter().enumerate() {
                    if let Some(j) = map.get(i).copied().flatten() {
                        st.in_water[j] = wet;
                        st.water_drag[j] = drag;
                    }
                }
                let engines: Vec<usize> = fire
                    .map(|f| f.burning.iter().filter_map(|&i| map[i]).collect())
                    .unwrap_or_default();
                let throttle = if engines.is_empty() {
                    None
                } else {
                    st.run_engines(0, &engines);
                    fire.map(|f| f.throttle)
                };
                let report =
                    ose_data::staging::delta_v_by_stage(craft, &catalog.defs, &catalog.resources);
                let spec = flight::FlightSpec::from_report(&report, &[], &[], drag_area_m2);
                Some((st, spec, throttle))
            }
            _ => None,
        };
        let craft = subset.map(|s| s.0);
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
        let (spec, structure) = match burning {
            Some((st, spec, throttle)) => (spec, Some((st, throttle))),
            None => (spec, None),
        };
        let mut p = flight::Powered::new(spec, r2, v2, false);
        p.facing = facing;
        // it tumbles freely in the frame it left with
        p.attitude = flight::Attitude::Free;
        p.own_top = top;
        p.angvel = angvel;
        if let Some((_, Some(throttle))) = &structure {
            p.ignited = true;
            p.stage = 0;
            p.throttle = *throttle;
        }
        let orbit = Orbit::from_state_vectors(mu, r2, v2, ut);
        let d = self
            .world
            .spawn((
                vessels::Vessel { name },
                bodies::ReferenceBody(body),
                bodies::OnRails(orbit),
                p,
                vessels::Debris,
            ))
            .id();
        // it starts out overlapping the vessel it left: they pass through each other for a moment
        let until_ut = self.ut() + collide::SEPARATION_GRACE_S;
        self.world.entity_mut(d).insert(collide::SeparatedFrom {
            other: parent,
            until_ut,
        });
        if let Some(c) = craft {
            self.set_craft(d, c);
        }
        if let Some((st, _)) = structure {
            // the rebuild seats nobody: crew stays with the vessel they rode in
            self.set_crew(d, Vec::new());
            self.set_structure(d, st);
        }
        Some(d)
    }

    /// The part catalog and resource definitions the server loaded, so pieces that come off a craft can
    /// be built as vessels of their own.
    pub fn set_part_catalog(
        &mut self,
        defs: Vec<ose_data::PartDef>,
        resources: Vec<ose_data::ResourceDef>,
    ) {
        self.world.insert_resource(PartCatalog { defs, resources });
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

    /// Development only: let planned burns change orbits at their time without engines or propellant (the
    /// old impulsive behaviour). Off by default: a node is a plan, and its burn is flown.
    pub fn set_free_burns(&mut self, on: bool) {
        self.world.insert_resource(vessels::FreeBurns(on));
    }

    /// When the vessel's scheduled burn starts: at the node, as KSP2's burn timer counts down to it
    /// (`ManeuverProvider.GetStartBurnTime`: the burn starts at the node's time and runs for its
    /// `BurnDuration`) and as its maneuver prediction burns (`ManeuverPlanSolver`).
    pub fn burn_start(&self, e: Entity) -> Option<f64> {
        self.scheduled_burn(e).map(|b| b.ut)
    }

    /// The planned burn the vessel is flying: what is left of it, m/s, inertial.
    pub fn burn_left(&self, e: Entity) -> Option<DVec3> {
        self.world
            .get::<vessels::ExecutingBurn>(e)
            .map(|x| x.dv - x.delivered)
    }

    /// Why the vessel's last planned burn stopped short.
    pub fn burn_stopped(&self, e: Entity) -> Option<&str> {
        self.world
            .get::<vessels::BurnStopped>(e)
            .map(|b| b.0.as_str())
    }

    /// Start flying the scheduled burn now: the delta-v it asks for at its time, held and burned.
    fn begin_burn(&mut self, e: Entity) {
        let (Some(b), Some(o)) = (
            self.scheduled_burn(e),
            self.world.get::<bodies::OnRails>(e).map(|o| o.0),
        ) else {
            return;
        };
        let dv = vessels::burn_delta_v(&o, &b);
        self.world
            .entity_mut(e)
            .remove::<vessels::BurnStopped>()
            .insert(vessels::ExecutingBurn {
                dv,
                delivered: DVec3::ZERO,
                dry_s: 0.0,
            });
    }

    /// Plan a chain of burns (maneuver nodes) for a vessel: put in time order, each planned on the orbit
    /// the one before leaves; all must be in the future. An empty list clears them.
    pub fn schedule_burns(&mut self, e: Entity, mut burns: Vec<vessels::ScheduledBurn>) -> bool {
        if self.vessel(e).is_none() || burns.iter().any(|b| b.ut < self.ut()) {
            return false;
        }
        burns.sort_by(|a, b| a.ut.total_cmp(&b.ut));
        // clearing or changing the next node ends the burn being flown for it
        if burns.first() != self.scheduled_burn(e).as_ref() {
            self.stop_flying_burn(e);
        }
        let mut ent = self.world.entity_mut(e);
        ent.remove::<vessels::ScheduledBurn>()
            .remove::<vessels::LaterBurns>();
        if !burns.is_empty() {
            let first = burns.remove(0);
            ent.insert(first);
            if !burns.is_empty() {
                ent.insert(vessels::LaterBurns(burns));
            }
        }
        true
    }

    /// The vessel's planned burns in time order (the scheduled one first).
    pub fn scheduled_burns(&self, e: Entity) -> Vec<vessels::ScheduledBurn> {
        let mut out: Vec<vessels::ScheduledBurn> = self.scheduled_burn(e).into_iter().collect();
        if let Some(l) = self.world.get::<vessels::LaterBurns>(e) {
            out.extend(l.0.iter().copied());
        }
        out
    }

    /// Predicted trajectory of a vessel from the current time. With planned burns, the patches up to each
    /// burn end with `Transition::Maneuver` and the prediction continues on the orbit it leaves, burn after
    /// burn.
    pub fn predict(&self, e: Entity) -> Vec<trajectory::Patch> {
        let Some((_, body, orbit)) = self.vessel(e) else {
            return Vec::new();
        };
        // landed, splashed down or wrecked: no trajectory (the original's map draws none)
        if self
            .world
            .get::<flight::Powered>(e)
            .is_some_and(|p| p.landed || p.splashed || p.crashed)
        {
            return Vec::new();
        }
        let index = self.world.resource::<bodies::BodyIndex>();
        let mut burns = self.scheduled_burns(e).into_iter();
        let stages = self
            .world
            .get::<flight::Powered>(e)
            .or_else(|| self.world.get::<flight::Dormant>(e).map(|d| &d.0))
            .map(vessels::burn_stages)
            .unwrap_or_default();
        let mut kept = Vec::new();
        let (mut body, mut orbit, mut from_ut, mut after_burn) = (body, orbit, self.ut(), false);
        loop {
            let mut patches =
                trajectory::predict(&self.world, index, body, orbit, from_ut, &self.params);
            if after_burn {
                if let Some(first) = patches.first_mut() {
                    first.start = trajectory::Transition::Maneuver;
                }
            }
            let Some(burn) = burns.next() else {
                kept.extend(patches);
                return kept;
            };
            // keep the patches before the burn; the one containing it is cut at the burn time
            let mut cut_done = false;
            for p in patches {
                let repeats =
                    p.end == trajectory::Transition::Final && p.orbit.eccentricity() < 1.0;
                if p.start_ut <= burn.ut && (burn.ut < p.end_ut || repeats) {
                    let mut cut = p;
                    cut.end_ut = burn.ut;
                    cut.end = trajectory::Transition::Maneuver;
                    body = p.body;
                    orbit = p.orbit;
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
            // flown as KSP2 predicts it: a finite burn from the node, stage after stage
            match vessels::finite_burn(&orbit, &burn, &stages) {
                Some((r, v, end)) => {
                    orbit = Orbit::from_state_vectors(orbit.mu, r, v, end);
                    from_ut = end;
                }
                None => {
                    orbit = vessels::orbit_after_burn(&orbit, &burn);
                    from_ut = burn.ut;
                }
            }
            after_burn = true;
        }
    }

    /// Advance by one simulation step covering up to `WarpFactor` ticks. The step is shortened so that no
    /// vessel skips over its next SOI transition (the original's timescale override at SOI crossings); the
    /// crossing then lands on the step boundary and the handoff happens exactly there.
    pub fn step(&mut self) {
        for e in self.vessels() {
            if self.rails_reentry(e).is_some_and(|t| t <= self.ut()) {
                self.wake(e);
            }
        }
        let mut warp = self.warp() as u64;
        if let Some(cap) = self.warp_cap() {
            warp = warp.min(cap as u64);
        }
        let now = self.tick().0;
        let mut ticks = warp;
        for e in self.vessels() {
            let mut stops = Vec::new();
            if let Some(reentry) = self.rails_reentry(e) {
                stops.push(reentry);
            }
            if let Some(nt) = self.world.get::<vessels::NextTransition>(e) {
                stops.push(nt.ut);
            }
            if self.world.get::<vessels::ExecutingBurn>(e).is_none() {
                if let Some(t) = self.burn_start(e) {
                    stops.push(t);
                }
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
        // what Kerbals on EVA stand on (a craft's hull) before they move
        self.kerbals_find_decks(ticks.max(1) as f64 * dt);
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
            // Above physics warp a craft at rest on the ground or the sea (or a wreck) turns with its body
            // and nothing else (integrating it tick by tick at x100000 would never keep up, and its contact
            // or buoyancy at one long step would not hold).
            if warp > flight::MAX_PHYSICS_WARP as u64 && (flight::resting(&p) || p.crashed) {
                let omega = if body.rotation_period_s > 0.0 {
                    std::f64::consts::TAU / body.rotation_period_s
                } else {
                    0.0
                };
                let (s, c) = (omega * ticks.max(1) as f64 * dt).sin_cos();
                let turn = |v: DVec3| DVec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
                p.r = turn(p.r);
                p.v = turn(p.v);
                p.facing = turn(p.facing);
                if p.own_top.length_squared() > 0.0 {
                    p.own_top = turn(p.own_top);
                }
                let orbit = Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
                self.world
                    .entity_mut(e)
                    .insert(bodies::OnRails(orbit))
                    .insert(p);
                continue;
            }
            // under power the planned burns are flown by hand: once the next one's time comes, the one
            // before is done
            if self
                .world
                .get::<vessels::LaterBurns>(e)
                .and_then(|l| l.0.first())
                .is_some_and(|b| b.ut <= self.ut())
            {
                vessels::promote_next_burn(&mut self.world, e);
            }
            // a maneuver hold flies the node from its start
            if matches!(p.attitude, flight::Attitude::Maneuver(_))
                && self.world.get::<vessels::ExecutingBurn>(e).is_none()
                && self.burn_start(e).is_some_and(|t| t <= self.ut())
            {
                self.begin_burn(e);
            }
            let mut exec = self.world.get::<vessels::ExecutingBurn>(e).copied();
            let mut exec_done: Option<Option<String>> = None;
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
            if let Some(st) = structure.as_mut() {
                st.frame_top = flight::vessel_top(&p);
            }
            let sun = if structure.is_some() {
                self.sunlight(body_e, p.r, ut)
            } else {
                0.0
            };
            // flown, the panels face the sun as they are turned (the star sits at the origin)
            if let Some(st) = structure.as_mut() {
                let at = bodies::star_space_state(&self.world, body_e, ut).0 + p.r;
                st.sun_local =
                    (at.length() > 0.0).then(|| st.to_craft_axis(p.facing, -at.normalized()));
            }
            let mut pieces = Vec::new();
            // the pieces a crash threw (they come to rest on the ground rather than crash again)
            let mut wrecked: Vec<usize> = Vec::new();
            // a vessel standing on the ground with its engines off just turns with the body: one long step
            let parked = p.landed
                && !p.crashed
                && p.throttle <= 0.0
                && p.drive.throttle == 0.0
                && p.drive.speed_mps == 0.0
                && flight::resting_on_ground(&p, &body, terrain.as_deref(), start_ut);
            // burning under time warp: longer steps, the thrust held in one inertial direction
            let warp_burn = warp > flight::MAX_PHYSICS_WARP as u64 && self.burning_in_orbit(e);
            let hold = if warp_burn {
                Some(
                    self.world
                        .get::<vessels::WarpBurnHold>(e)
                        .map(|h| h.0)
                        .unwrap_or(p.facing),
                )
            } else {
                None
            };
            match hold {
                Some(dir) => {
                    self.world.entity_mut(e).insert(vessels::WarpBurnHold(dir));
                }
                None => {
                    self.world.entity_mut(e).remove::<vessels::WarpBurnHold>();
                }
            }
            let (steps, step_dt) = if parked {
                (1, dt * ticks.max(1) as f64)
            } else if warp_burn {
                let span = dt * ticks.max(1) as f64;
                let n = (span / flight::THRUST_WARP_STEP_S).ceil().max(1.0);
                (n as u64, span / n)
            } else {
                (ticks.max(1), dt)
            };
            for i in 0..steps {
                if let Some(st) = &mut structure {
                    st.sync(&mut p);
                    let mut burned = Vec::new();
                    let mut lift = DVec3::ZERO;
                    p.aero_drag_kn = None;
                    let mut torque = DVec3::ZERO;
                    let mut surfaces = 0.0;
                    // part drag (and heating) for the airflow now
                    p.drag_cda_m2 = atmo.as_ref().map(|a| {
                        let alt = p.r.length() - body.radius_m;
                        let ground = flight::ground_radius(&body, terrain.as_deref(), p.r, ut);
                        let height = p.r.length() - ground;
                        // in the water (`Splashed`): afloat, or resting on the sea rather than on solid ground
                        let splashed = p.splashed
                            || (p.landed
                                && surface::solid_radius(&body, terrain.as_deref(), p.r, ut) + 0.5
                                    < ground);
                        let flow = structure::Airflow::at(&p, &body, Some(a));
                        st.update_chutes(
                            flow,
                            structure::ChuteAir {
                                pressure_atm: a.pressure_kpa(alt) / 101.325,
                                height_m: height,
                                temperature_k: a.temperature_k.evaluate(alt),
                                conductivity: thermal::conductivity(height, a.depth_m),
                                landed: p.landed,
                                splashed,
                            },
                            step_dt,
                        );
                        let forces = st.aerodynamics_local(p.stage, flow);
                        p.aero_drag_kn = Some(forces.drag_kn);
                        let cda = forces.cda_m2;
                        lift = forces.lift_kn;
                        torque = forces.torque_knm;
                        surfaces = st.control_authority_local(flow);
                        if heat.enabled {
                            burned = st.update_heat_in_flow(
                                flow,
                                a.temperature_k.evaluate(alt),
                                thermal::conductivity(alt, a.depth_m),
                                &heat,
                                step_dt,
                            );
                        }
                        cda
                    });
                    if atmo.is_none() {
                        // Clear the overlay and saved part loads on an airless SOI transition.
                        st.aerodynamics_local(p.stage, structure::Airflow::at(&p, &body, None));
                        // out of the air, engine exhaust is all that heats a part
                        if heat.enabled {
                            burned = st.apply_exhaust_heat(&heat, step_dt);
                        }
                    }
                    p.lift_kn = lift;
                    p.aero_torque_knm = torque;
                    p.surface_torque_knm = surfaces;
                    if !burned.is_empty() {
                        let centre = st.mass_properties().centre_m;
                        match st.burn_up(&burned, p.facing) {
                            Some(off) => {
                                st.keep_in_place(&mut p, centre);
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
                // flying a planned burn: hold along what is left, throttle up once pointed, ease off near the end
                if let (Some(x), None) = (&exec, &exec_done) {
                    let left = x.dv - x.delivered;
                    if left.length() > 1e-6 {
                        let dir = left.normalized();
                        p.attitude = flight::Attitude::Maneuver(dir);
                        let thrust = p
                            .current_stage()
                            .map(|s| s.thrust_vac_kn.max(s.thrust_asl_kn))
                            .unwrap_or(0.0);
                        let a_max = if p.mass_t > 0.0 {
                            thrust / p.mass_t
                        } else {
                            0.0
                        };
                        p.throttle = if p.facing.dot(dir) > 0.996 && a_max > 0.0 {
                            (left.length() / a_max).clamp(0.02, 1.0)
                        } else {
                            0.0
                        };
                    }
                }
                // down before this step (the surface setup below already marks a splashdown)
                let was_landed = p.landed || p.splashed;
                let env = flight::EngineEnvironment::at(&p, &body, atmo.as_ref());
                let step_ut = start_ut + i as f64 * step_dt;
                let propulsion = structure.as_mut().map(|st| {
                    if comm_blocked {
                        p.controlled = false;
                        p.throttle = 0.0;
                    }
                    let mut propulsion = st.powered_services(&mut p, sun, env, step_dt);
                    if comm_blocked {
                        p.controlled = false;
                    }
                    // Plan with rated wheel axes; current delivery is resolved at each wheel below.
                    p.wheel_scale = 1.0;
                    p.sas_warp = warp as f64;
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
                if let Some(dir) = hold {
                    p.facing = dir;
                    p.angvel = DVec3::ZERO;
                    p.roll_rate = 0.0;
                }
                let before = p.clone();
                let mut out = flight::integrate_with_propulsion(
                    &mut p,
                    &body,
                    atmo.as_ref(),
                    terrain.as_deref(),
                    start_ut + i as f64 * step_dt,
                    step_dt,
                    propulsion,
                );
                if let Some(dir) = hold {
                    p.facing = dir;
                    p.angvel = DVec3::ZERO;
                    p.roll_rate = 0.0;
                }
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
                if let (Some(x), None) = (&mut exec, &exec_done) {
                    // what the engines (and anything but gravity) changed this step
                    let r = before.r.length();
                    let g = before.r * (-body.mu / (r * r * r));
                    x.delivered = x.delivered + (p.v - before.v) - g * step_dt;
                    let left = x.dv - x.delivered;
                    let wanted = before.throttle > 0.0;
                    if left.dot(x.dv) <= 0.0 || left.length() < 0.05 {
                        p.throttle = 0.0;
                        exec_done = Some(None);
                    } else if wanted && out.thrust_kn <= 0.0 {
                        x.dry_s += step_dt;
                        if x.dry_s > 3.0 {
                            p.throttle = 0.0;
                            exec_done = Some(Some(format!(
                                "out of thrust with {:.1} m/s to go",
                                left.length()
                            )));
                        }
                    } else {
                        x.dry_s = 0.0;
                    }
                    if p.crashed {
                        exec_done = Some(Some("the vessel crashed".into()));
                    }
                }
                // a hard landing breaks the parts it lands on; the rest may come through
                if out.crashed {
                    if let Some(st) = &mut structure {
                        let centre = st.rigid.rest_centre();
                        let mass_centre = st.mass_properties().centre_m;
                        let (off, lost) = if out.contact_damage.is_empty() {
                            let (_, off, lost) =
                                st.crash(out.impact_mps, p.facing, out.impact_against, centre);
                            (off, lost)
                        } else if let Some(off) = st.contact_damage(&out.contact_damage, &p) {
                            (off, false)
                        } else {
                            let (_, off, _) =
                                st.crash(out.impact_mps, p.facing, out.impact_against, centre);
                            (off, true)
                        };
                        if !lost {
                            p.crashed = false;
                            out.crashed = false;
                            st.keep_in_place(&mut p, mass_centre);
                            for piece in off {
                                pieces.push((piece, p.r, p.v, p.facing));
                            }
                        } else {
                            // the root broke: the vessel is lost, whatever struck it
                            p.crashed = true;
                            p.throttle = 0.0;
                            // the wreck flies apart: the blow bounced back weakly, scattered, tumbling
                            let up = p.r.normalized();
                            let v_in = out.impact_velocity;
                            let bounce = (v_in - up * (2.0 * v_in.dot(up))) * 0.12;
                            let speed = v_in.length();
                            for (k, mut piece) in off.into_iter().enumerate() {
                                let a = scatter(e.to_bits(), k as u64);
                                let b = scatter(e.to_bits() ^ 0x9e37, k as u64);
                                let side = a - up * a.dot(up);
                                piece.dv_mps = bounce
                                    + side * (0.06 * speed)
                                    + up * (0.03 * speed * (1.0 + a.dot(up).abs()));
                                piece.spin = b * 2.0;
                                wrecked.push(pieces.len());
                                pieces.push((piece, p.r, p.v, p.facing));
                            }
                        }
                        st.sync(&mut p);
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
                if structure.is_none() && p.walker && p.jetpack.is_none() && p.rcs.usage > 0.0 {
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
                    if comm_blocked {
                        // a probe out of contact cannot be flown (CommNetRequired)
                        p.controlled = false;
                        p.throttle = 0.0;
                    }
                    let centre = st.mass_properties().centre_m;
                    let torn = st.apply(&p, &out, step_dt);
                    if !torn.is_empty() {
                        st.keep_in_place(&mut p, centre);
                    }
                    for piece in torn {
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
            for (k, (piece, r, v, facing)) in pieces.into_iter().enumerate() {
                // a piece off the side tips away from the vessel
                let side = piece.offset_m - facing * piece.offset_m.dot(facing);
                let tumble = if piece.spin.length_squared() > 0.0 {
                    piece.spin
                } else if side.length() > 0.5 {
                    facing.cross(side.normalized()) * 0.15
                } else {
                    DVec3::ZERO
                };
                let wreck = wrecked.contains(&k);
                if let Some(d) = self.spawn_debris(
                    e,
                    piece.mass_t,
                    r + piece.offset_m,
                    v + piece.dv_mps,
                    facing,
                    area,
                    &piece.parts,
                    tumble,
                    None,
                ) {
                    if wreck {
                        if let Some(mut q) = self.world.get_mut::<flight::Powered>(d) {
                            q.crash_tolerance_mps = 1.0e4;
                        }
                        if let Some(mut st) = self.world.get_mut::<structure::Structure>(d) {
                            st.crash_tolerance.fill(1.0e4);
                        }
                    }
                    self.broken_off.push((e, d));
                }
            }
            match exec_done {
                Some(stopped) => {
                    let mut ent = self.world.entity_mut(e);
                    ent.remove::<vessels::ExecutingBurn>();
                    match stopped {
                        // flown: the next node takes its place
                        None => vessels::promote_next_burn(&mut self.world, e),
                        Some(why) => {
                            ent.insert(vessels::BurnStopped(why));
                        }
                    }
                }
                None => {
                    if let Some(x) = exec {
                        self.world.entity_mut(e).insert(x);
                    }
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
            let flying_burn = self.world.get::<vessels::ExecutingBurn>(e).is_some();
            // a Kerbal on a ladder and the craft it holds stay in physics together
            let held = self.world.get::<crew::OnLadder>(e).is_some() || {
                let mut q = self.world.query::<&crew::OnLadder>();
                q.iter(&self.world).any(|l| l.vessel == e)
            };
            let hand_off = p.coasting_since.is_some_and(|t| ut - t >= 10.0)
                && !flying_burn
                && !held
                && !p.landed
                && !p.crashed
                && safe_coast
                && !p.manual_steering
                && p.rotation_input.length_squared() == 0.0
                && p.roll_input == 0.0
                && p.rcs.translate.length_squared() == 0.0
                // a spinning craft stays under power until it is still, SAS holding or not: going on rails
                // would freeze its spin (KSP2 keeps the active craft in physics while SAS damps it)
                && {
                    // a hold that follows the orbit (prograde, radial, up...) turns the craft once an
                    // orbit, and SAS chasing it jitters by a few mrad/s about that turn: still means not
                    // turning against that frame by more than the chase does (0.3 deg/s, nothing to see)
                    let (still, frame) = if p.attitude == flight::Attitude::Free {
                        (1e-5, DVec3::ZERO)
                    } else if p.attitude.follows_the_orbit() {
                        (5e-3, p.r.cross(p.v) * (1.0 / p.r.length_squared().max(1.0)))
                    } else {
                        (1e-4, DVec3::ZERO)
                    };
                    (p.angvel - frame).length() < still && p.roll_rate.abs() < still
                };
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
                ent.insert(flight::Dormant(flight::packed(p)));
            }
            vessels::refresh_next_transition(&mut self.world, &index, e, ut, &self.params);
        }
        self.advance_experiments(ticks.max(1) as f64 * dt);
        // the network every 3 s of real time, as the original rebuilds its graph (one step is a fiftieth of a
        // second of real time at any warp)
        let rebuild_every = (commnet::REBUILD_S * TICKS_PER_SECOND as f64) as u64;
        let calls = {
            let mut c = self
                .world
                .get_resource_or_insert_with(CommNetSteps::default);
            c.0 += 1;
            c.0
        };
        if calls % rebuild_every == 1 || rebuild_every <= 1 {
            self.update_commnet();
        }
        self.charge_on_rails(ticks.max(1) as f64 * dt, ut);
        self.collide_vessels();
        self.exhaust_on_other_vessels(ticks.max(1) as f64 * dt);
        self.kerbals_meet_hulls();
        self.run_transfers(ticks.max(1) as f64 * dt);
        self.run_transmissions(ticks.max(1) as f64 * dt);
        self.run_ladders(ticks.max(1) as f64 * dt);
        // crashed debris is gone at once; a crashed craft stays for `WRECK_LINGER_S` (its pilot sees the crash
        // and its report) and then is gone too, as the original destroys it
        let wrecks: Vec<(Entity, bool, Option<f64>)> = {
            let mut q = self.world.query::<(
                Entity,
                &flight::Powered,
                Has<vessels::Debris>,
                Option<&vessels::CrashedAt>,
            )>();
            q.iter(&self.world)
                .filter(|(_, p, _, _)| p.crashed)
                .map(|(e, _, debris, at)| (e, debris, at.map(|a| a.0)))
                .collect()
        };
        for (e, debris, at) in wrecks {
            match at {
                _ if debris => self.world.despawn(e),
                Some(t) if ut - t >= WRECK_LINGER_S => self.world.despawn(e),
                Some(_) => true,
                None => {
                    self.world.entity_mut(e).insert(vessels::CrashedAt(ut));
                    true
                }
            };
        }
        for e in self.vessels() {
            if self.world.get::<flight::Powered>(e).is_some() {
                continue;
            }
            if self.rails_reentry(e).is_some_and(|t| t <= ut) {
                self.wake(e);
                self.world.resource_mut::<WarpFactor>().0 = 1;
                continue;
            }
            // a planned burn is flown by the vessel's engines: wake it at the burn's start
            let under_way = self.world.get::<vessels::ExecutingBurn>(e).is_some();
            if self.world.get::<flight::Dormant>(e).is_some()
                && (under_way || self.burn_start(e).is_some_and(|t| t <= ut))
                && !self
                    .world
                    .get_resource::<vessels::FreeBurns>()
                    .is_some_and(|f| f.0)
            {
                if !under_way {
                    self.begin_burn(e);
                }
                if self.wake(e) {
                    if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                        p.attitude = flight::Attitude::Maneuver(DVec3::ZERO);
                    }
                    // (time warp drops to physics warp by itself while it flies)
                }
                continue;
            }
            let burned = self
                .world
                .get_resource::<vessels::FreeBurns>()
                .is_some_and(|f| f.0)
                && vessels::apply_due_burn(&mut self.world, e, ut);
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
        // A new SAS direction wakes a coasting craft even at zero throttle.
        assert!(u.set_controls(v, 0.0, flight::Attitude::OrbitPrograde));
        assert!(u.powered(v).is_some());
        // throttle up: burns prograde from the rails position
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

    /// The starter rocket (Mk1 pod, 1x2 tank, Swivel) lit in a circular 700 km orbit round Kerbin, its nose
    /// held prograde.
    fn starter_in_orbit() -> (Universe, Entity) {
        craft_in_orbit("Stock12_StartRocket")
    }

    /// A stock craft lit in a circular 700 km orbit round Kerbin, its nose prograde.
    fn craft_in_orbit(name: &str) -> (Universe, Entity) {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == name).unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Start", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let mut p = u.powered(v).unwrap().clone();
        p.r = DVec3::new(700_000.0, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / 700_000.0_f64).sqrt(), 0.0);
        p.landed = false;
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        u.set_powered(v, p);
        u.stage(v);
        u.step();
        (u, v)
    }

    #[test]
    fn a_craft_burning_in_orbit_keeps_burning_under_time_warp_and_ends_where_a_real_time_burn_does()
    {
        // KSP2 keeps a craft burning under time warp above the air and terrain (orbital physics under thrust),
        // its thrust held in one inertial direction; we allow up to 100x, the burn integrated in 0.1 s steps.
        let prograde = flight::Attitude::Fixed(DVec3::new(0.0, 1.0, 0.0));
        // a long burn: the pod on two 1x8 tanks and a Swivel
        let run = |warp: u32| {
            let (mut u, v) = craft_in_orbit("Lesson Animation rocket");
            assert!(u.set_controls(v, 1.0, prograde));
            u.step();
            u.set_warp(warp);
            assert_eq!(u.warp(), warp, "warp {warp} allowed while burning in orbit");
            let end = u.ut() + 20.0;
            while u.ut() < end - 1e-9 {
                // the last step no longer than what is left
                let left = ((end - u.ut()) * TICKS_PER_SECOND as f64).round() as u32;
                if left < u.warp() {
                    u.set_warp(left);
                }
                u.step();
            }
            assert!((u.ut() - end).abs() < 1e-6, "{} vs {end}", u.ut());
            let p = u.powered(v).unwrap().clone();
            (p.r, p.v, p.mass_t, p.facing)
        };
        let (r1, v1, m1, f1) = run(1);
        let (r100, v100, m100, f100) = run(100);
        assert!(
            (m1 - m100).abs() < 0.002 * m1,
            "propellant burned alike: {m1} vs {m100} t"
        );
        // within 0.15 % of the 1000 m/s it gained, 50 m after 20 s (the longer steps lag the falling mass)
        assert!((v1 - v100).length() < 1.5, "velocity {v1:?} vs {v100:?}");
        assert!(
            (r1 - r100).length() < 50.0,
            "position {:.2} m apart",
            (r1 - r100).length()
        );
        assert!(
            f100.dot(DVec3::new(0.0, 1.0, 0.0)) > 0.9999,
            "thrust held: {f100:?} ({f1:?})"
        );
    }

    #[test]
    fn a_warped_burn_drops_to_1x_when_its_engines_are_about_to_run_dry() {
        // KSP2 run 19 (T40): a burn warped at 100x fell back to 1x at once with tanks about to run dry;
        // `ClampRateToOrbitTransitions` holds 1x within (1 + rate) / 2 + 10 s of running out
        let (mut u, v) = craft_in_orbit("Lesson Animation rocket");
        assert!(u.set_controls(v, 1.0, flight::Attitude::Fixed(DVec3::new(0.0, 1.0, 0.0))));
        u.step();
        u.set_warp(100);
        assert_eq!(u.warp(), 100);
        let p = u.powered(v).unwrap().clone();
        let s = p.current_stage().unwrap();
        let flow = s.thrust_vac_kn / (s.isp_vac_s * ose_data::G0);
        // 40 s of propellant left: (1 + 50) / 2 + 10 = 35.5 s fits 50x, 100x does not
        let mut q = p.clone();
        q.propellant_t = 40.0 * flow;
        u.set_powered(v, q);
        u.set_warp(100);
        assert_eq!(u.warp(), 50);
        let mut q = p.clone();
        q.propellant_t = 5.0 * flow;
        u.set_powered(v, q);
        u.set_warp(100);
        assert_eq!(u.warp(), 1);
    }

    #[test]
    fn time_warp_stays_at_physics_speeds_for_a_burn_low_down_or_with_the_controls_held() {
        let (mut u, v) = starter_in_orbit();
        // down at 50 km, in the air
        let mut p = u.powered(v).unwrap().clone();
        p.r = DVec3::new(650_000.0, 0.0, 0.0);
        u.set_powered(v, p);
        assert!(u.set_controls(v, 1.0, flight::Attitude::Free));
        u.set_warp(100);
        assert_eq!(u.warp(), flight::MAX_PHYSICS_WARP);
        // up high but turning
        let (mut u, v) = starter_in_orbit();
        assert!(u.set_controls(v, 1.0, flight::Attitude::Free));
        u.set_rotation(v, 1.0, 0.0, 0.0);
        u.set_warp(100);
        assert_eq!(u.warp(), flight::MAX_PHYSICS_WARP);
    }

    #[test]
    fn a_planned_burn_is_predicted_as_a_finite_burn_from_the_node() {
        // KSP2's maneuver prediction (`ManeuverPlanSolver.CalculateOdeManeuverNodes`): the burn starts at the
        // node and holds the node's direction at full thrust for the rocket equation's time; flying exactly
        // that ends on the predicted orbit.
        let (mut u, v) = starter_in_orbit();
        let burn = vessels::ScheduledBurn {
            ut: u.ut() + 30.0,
            prograde_mps: 600.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        };
        assert!(u.schedule_burn(v, burn));
        let after = u
            .predict(v)
            .into_iter()
            .find(|p| p.start == trajectory::Transition::Maneuver)
            .expect("a patch after the burn");
        let impulsive = vessels::orbit_after_burn(&u.vessel(v).unwrap().2, &burn);
        // fly it: wait for the node, then full thrust along the node's direction for the burn time
        let o = u.vessel(v).unwrap().2;
        let dv = vessels::burn_delta_v(&o, &burn);
        let stages = vessels::burn_stages(u.powered(v).unwrap());
        let (_, _, end) = vessels::finite_burn(&o, &burn, &stages).unwrap();
        assert!(
            after.start_ut > burn.ut + 1.0,
            "the patch starts where the burn ends"
        );
        assert!((after.start_ut - end).abs() < 1e-9);
        u.schedule_burns(v, vec![]);
        let hold = flight::Attitude::Fixed(dv.normalized());
        assert!(u.set_controls(v, 0.0, hold));
        while u.ut() < burn.ut - 1e-9 {
            u.step();
        }
        assert!(u.set_controls(v, 1.0, hold));
        while u.ut() < end - 1e-9 {
            u.step();
        }
        assert!(u.set_controls(v, 0.0, hold));
        let p = u.powered(v).unwrap();
        let mu = u
            .world
            .get::<bodies::Body>(u.body_of(v).unwrap())
            .unwrap()
            .mu;
        let flown = Orbit::from_state_vectors(mu, p.r, p.v, u.ut());
        let err = |o: &Orbit| (o.apoapsis() - flown.apoapsis()).abs();
        assert!(
            err(&after.orbit) < 0.005 * flown.apoapsis(),
            "predicted apoapsis {:.0} m, flown {:.0} m",
            after.orbit.apoapsis(),
            flown.apoapsis()
        );
        assert!(
            err(&after.orbit) < err(&impulsive),
            "finite {:.0} m off, impulsive {:.0} m off",
            err(&after.orbit),
            err(&impulsive)
        );
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
    fn a_craft_goes_on_rails_without_its_spin() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let mut p = u.powered(v).unwrap().clone();
        p.r = DVec3::new(700_000.0, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / 700_000.0_f64).sqrt(), 0.0);
        p.landed = false;
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        p.attitude = flight::Attitude::Fixed(p.facing);
        p.angvel = DVec3::new(0.0, 0.0, 0.3);
        u.set_powered(v, p);
        for _ in 0..(20 * TICKS_PER_SECOND) {
            u.step();
            if u.powered(v).is_none() {
                break;
            }
        }
        let d = u.dormant(v).expect("on rails after a quiet coast");
        assert_eq!(d.angvel, DVec3::ZERO);
        assert_eq!(d.roll_rate, 0.0);
    }

    #[test]
    fn a_crashed_craft_is_gone_a_little_after_the_crash() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        let mut p = u.powered(v).unwrap().clone();
        p.crashed = true;
        p.landed = false;
        u.set_powered(v, p);
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
        }
        assert!(
            u.vessel(v).is_some(),
            "the wreck is there to see for a while"
        );
        for _ in 0..((WRECK_LINGER_S as u64 - 5) * TICKS_PER_SECOND) {
            u.step();
        }
        assert!(u.vessel(v).is_none(), "and then gone");
    }

    #[test]
    fn craft_resting_on_the_ground_do_not_hold_warp_down() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        // landed with the throttle left up and nothing to burn (no stages: no propellant)
        let dry = flight::FlightSpec {
            stages: Vec::new(),
            drag_area_m2: 1.0,
            drag_cd: flight::DEFAULT_CD,
        };
        let v = u.spawn_powered_on_surface("dry", body, dry, 0.0, 0.0);
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        // and a roll key left held when its pilot moved on
        u.set_rotation(v, 0.0, 0.0, 1.0);
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        let b = u.world.get::<bodies::Body>(body).unwrap().clone();
        let before = {
            let p = u.powered(v).unwrap();
            (flight::lat_lon(&b, p.r, u.ut()), p.r.length())
        };
        u.set_warp(1000);
        assert_eq!(u.warp(), 1000, "a dry landed craft does not cap warp");
        for _ in 0..20 {
            u.step();
        }
        assert!(u.ut() > 100.0, "time ran at warp ({})", u.ut());
        let p = u.powered(v).unwrap();
        let ((lat, lon), r) = (flight::lat_lon(&b, p.r, u.ut()), p.r.length());
        assert!(
            (lat - before.0 .0).abs() < 1e-6 && (lon - before.0 .1).abs() < 1e-6,
            "stayed put"
        );
        assert!(
            (r - before.1).abs() < 0.01,
            "on the ground ({r} vs {})",
            before.1
        );
        assert!(p.landed && !p.crashed);
        // a craft that can thrust still holds warp down
        let (spec, st) = {
            let (_, _, spec, st) = k1_on_real_kerbin();
            (spec, st)
        };
        let k1 = u.spawn_powered_on_surface("K1", body, spec, 0.0, 1.0);
        u.set_structure(k1, st);
        assert!(u.set_controls(k1, 1.0, flight::Attitude::Up));
        u.set_warp(1000);
        assert_eq!(u.warp(), flight::MAX_PHYSICS_WARP);
    }

    #[test]
    fn a_turn_under_the_launch_hold_stays_where_the_keys_let_go() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        u.stand_south(v);
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        u.stage(v);
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        // D (yaw right) for a moment, then let go
        u.set_rotation(v, 0.0, 1.0, 0.0);
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        u.set_rotation(v, 0.0, 0.0, 0.0);
        let tilt = |u: &Universe| {
            let p = u.powered(v).unwrap();
            p.facing
                .dot(p.r.normalized())
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        };
        let leaned = tilt(&u);
        assert!(leaned > 3.0, "the keys turned it ({leaned} degrees)");
        let p = u.powered(v).unwrap();
        let heading = flight::heading_of(p.r, p.facing).to_degrees();
        assert!((heading - 90.0).abs() < 30.0, "D turns it east ({heading})");
        for _ in 0..(4 * TICKS_PER_SECOND) {
            u.step();
        }
        // The hold is where the keys let go (the air pushing on a tall rocket may hold it off a little).
        let p = u.powered(v).unwrap();
        let flight::Attitude::Fixed(hold) = p.attitude else {
            panic!("a fixed hold, not {:?}", p.attitude);
        };
        let held = hold.normalized().dot(p.r.normalized()).acos().to_degrees();
        assert!(
            (held - leaned).abs() < 2.0,
            "held at {held}, let go at {leaned}"
        );
        let after = tilt(&u);
        assert!(
            after > leaned * 0.5,
            "not back upright ({leaned} -> {after} degrees)"
        );
    }

    /// Kerbin over open sea 200 m deep, with the part catalog, and `name` falling into it from `height` m
    /// above the surface at `speed` m/s, leaning `lean` radians from upright.
    fn into_the_sea(name: &str, lean: f64, speed: f64, height: f64) -> (Universe, Entity) {
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
        let craft = crafts.iter().find(|c| c.name == name).unwrap();
        let mut u = Universe::with_bodies(&[kerbin]);
        u.set_part_catalog(defs.clone(), res.clone());
        let body = u.body_entity("Kerbin").unwrap();
        u.world
            .entity_mut(body)
            .insert(flight::Terrain(std::sync::Arc::new(
                ose_data::terrain::TerrainMap {
                    width: 2,
                    height: 2,
                    scale_m: 0.0,
                    offset_m: -200.0,
                    has_ocean: true,
                    body_radius_m: 600_000.0,
                    data: vec![0; 4],
                    max_m: 0.0,
                    detail: None,
                    overlay: None,
                    flats: vec![],
                },
            )));
        let report = ose_data::staging::delta_v_by_stage(craft, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface(name, body, spec, 0.0, 0.0);
        u.set_craft(v, craft.clone());
        u.set_structure(v, structure::Structure::build(craft, &defs, &res));
        {
            let mut p = u.world.get_mut::<flight::Powered>(v).unwrap();
            let up = p.r.normalized();
            let east = DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
            p.facing = (up * lean.cos() + east * lean.sin()).normalized();
            p.r = up * (600_000.0 + height);
            p.v = DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425).cross(p.r)
                - up * speed;
            p.attitude = flight::Attitude::Free;
            p.landed = false;
        }
        (u, v)
    }

    #[test]
    fn a_gentle_splashdown_floats_and_a_fast_one_breaks_up() {
        let (mut u, v) = into_the_sea("Stock12_StartRocket", 0.0, 5.0, 2.0);
        let parts = u
            .structure(v)
            .unwrap()
            .rigid
            .present
            .iter()
            .filter(|p| **p)
            .count();
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(v).unwrap();
        assert!(p.splashed && !p.crashed, "afloat");
        assert_eq!(
            u.structure(v)
                .unwrap()
                .rigid
                .present
                .iter()
                .filter(|p| **p)
                .count(),
            parts,
            "every part came through"
        );
        // striking the water at 50 m/s is a crash like striking the ground
        let (mut u, v) = into_the_sea("Stock12_StartRocket", 0.0, 50.0, 8.0);
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        let lost = u.powered(v).is_none_or(|p| p.crashed);
        let left = u
            .structure(v)
            .map(|s| s.rigid.present.iter().filter(|p| **p).count())
            .unwrap_or(0);
        assert!(
            lost || left < parts,
            "the impact broke it ({left} of {parts} parts)"
        );
        // a vessel whose root broke is lost, not a hull of no parts drifting down
        if left == 0 {
            assert!(u.powered(v).is_none_or(|p| p.crashed));
        }
    }

    #[test]
    fn pieces_broken_off_in_the_sea_float_rather_than_vanish() {
        let (mut u, _) = into_the_sea("Kerbal K1", 1.57, 40.0, 8.0);
        let mut pieces = Vec::new();
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
            pieces.extend(u.take_broken_off().into_iter().map(|(_, d)| d));
        }
        assert!(!pieces.is_empty(), "a flat 40 m/s impact breaks parts off");
        let floating = pieces
            .iter()
            .filter(|&&d| u.powered(d).is_some_and(|p| p.splashed && !p.crashed))
            .count();
        assert!(
            floating * 2 > pieces.len(),
            "most pieces float ({floating} of {})",
            pieces.len()
        );
        for &d in &pieces {
            if let Some(p) = u.powered(d) {
                assert!(p.r.length().is_finite() && p.mass_t > 0.0, "no empty hulls");
            }
        }
    }

    #[test]
    fn boosters_dropped_while_burning_keep_burning_their_own_fuel() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        u.set_part_catalog(defs, res);
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_craft(
            v,
            crafts.into_iter().find(|c| c.name == "Kerbal K1").unwrap(),
        );
        u.set_structure(v, st);
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        u.stage(v);
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        let d = u.stage(v).debris.expect("boosters come off");
        let solid = |u: &Universe, e: Entity| {
            let s = u
                .structure(e)
                .expect("a burning piece is built from its parts");
            let r = s.net.resource("SolidFuel").unwrap();
            s.net
                .tanks
                .iter()
                .filter(|t| t.resource == r)
                .map(|t| t.stored)
                .sum::<f64>()
        };
        let p = u.powered(d).unwrap().clone();
        assert!(p.ignited && p.throttle == 1.0);
        let st = u.structure(d).unwrap();
        assert!(!st.firing_engines(p.stage, p.throttle).is_empty());
        let sf0 = solid(&u, d);
        assert!(sf0 > 0.0, "they carry what was left in them");
        assert!(u.crew(d).is_empty(), "nobody rides on the boosters");
        let (v0, up) = (p.v, p.r.normalized());
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        let q = u.powered(d).unwrap();
        assert!(solid(&u, d) < sf0, "and burn it");
        assert!(
            (q.v - v0).dot(up) > 0.0,
            "still pushing up against gravity ({:?})",
            q.v - v0
        );
    }

    #[test]
    fn kerbal_k1_burns_its_tanks_through_the_flow_graph_and_stages_off_the_boosters() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let spec_mass = spec.stages[0].start_mass_t;
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        let m0 = u.powered(v).unwrap().mass_t;
        // the staging report counts every part; in flight the K1's physicsless parts (8 flat solar panels
        // at 0.8 kg and 4 radial batteries at 5 kg) weigh nothing, as the original's vessel TotalMass
        let physicsless = 8.0 * 0.0008 + 4.0 * 0.005;
        assert!(
            (m0 - spec_mass + physicsless
                - u.crew(v).len() as f64 * (crew::KERBAL_MASS_T + crew::JETPACK_PROPELLANT_T))
                .abs()
                < 1e-6,
            "tank and occupied-seat mass {m0} matches the staging report {spec_mass} plus crew,              less the physicsless parts"
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
            (p.mass_t - s.mass_properties().mass_t).abs() < 1e-9,
            "vessel mass includes tanks and occupied seats"
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
    fn crew_move_between_seats_and_fuel_moves_between_tanks() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        u.set_crew(v, vec![(0, "Jebediah Kerman".into())]);
        u.step();
        assert_eq!(
            u.move_crew(v, "Jebediah Kerman", 7),
            Err("that part carries no crew")
        );
        assert_eq!(
            u.move_crew(v, "Bob Kerman", 0),
            Err("no such Kerbal aboard")
        );
        assert_eq!(u.move_crew(v, "Jebediah Kerman", 0), Ok(()));
        // half empty the upper tank, then fill it from the lower one
        let (upper, lower) = (7, 19);
        let methalox = {
            let mut st = u.world.get_mut::<structure::Structure>(v).unwrap();
            let ri = st
                .net
                .tanks
                .iter()
                .find(|t| t.part == upper)
                .map(|t| t.resource)
                .unwrap();
            for t in st.net.tanks.iter_mut().filter(|t| t.part == upper) {
                t.stored *= 0.5;
            }
            st.net.resources[ri].clone()
        };
        let stored = |u: &Universe, part: usize| -> (f64, f64) {
            let st = u.structure(v).unwrap();
            st.net
                .tanks
                .iter()
                .filter(|t| t.part == part && st.net.resources[t.resource] == methalox)
                .fold((0.0, 0.0), |a, t| (a.0 + t.stored, a.1 + t.capacity))
        };
        let (before_lower, _) = stored(&u, lower);
        let (before_upper, cap_upper) = stored(&u, upper);
        assert!(u
            .transfer_resource(v, &methalox, &[lower], &[upper])
            .is_ok());
        assert!(u.transfer(v).is_some());
        for _ in 0..(12 * TICKS_PER_SECOND) {
            u.step();
        }
        assert!(u.transfer(v).is_none(), "done by itself");
        let (after_upper, _) = stored(&u, upper);
        let (after_lower, _) = stored(&u, lower);
        assert!(
            (after_upper - cap_upper).abs() < 1e-6,
            "full: {after_upper} / {cap_upper}, lower {before_lower} -> {after_lower}, upper was {before_upper}"
        );
        assert!(
            ((before_lower - after_lower) - (after_upper - before_upper)).abs() < 1e-6,
            "nothing lost"
        );
        assert!(
            u.transfer_resource(v, &methalox, &[lower], &[upper])
                .is_err(),
            "nothing to move"
        );
    }

    #[test]
    fn a_crash_leaves_a_wreck_of_the_parts_that_came_through() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_craft(v, k1.clone());
        u.set_structure(v, st);
        u.step();
        let p0 = u.powered(v).unwrap().clone();
        let up = p0.r.normalized();
        let omega = DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425);
        // nose first at 150 m/s
        let mut q = p0.clone();
        q.landed = false;
        q.r = up * (p0.r.length() + 20.0);
        q.v = omega.cross(q.r) - up * 150.0;
        q.facing = -up;
        q.attitude = flight::Attitude::Free;
        u.set_powered(v, q);
        let mut wreck = Vec::new();
        for _ in 0..TICKS_PER_SECOND {
            u.step();
            wreck.extend(u.take_broken_off().into_iter().map(|(_, d)| d));
        }
        assert!(
            u.powered(v).is_some_and(|p| p.crashed),
            "the vessel is lost"
        );
        assert!(!wreck.is_empty(), "but some of it came through");
        let kept: usize = wreck
            .iter()
            .filter_map(|&d| u.current_craft(d))
            .map(|c| c.parts.len())
            .sum();
        assert!(kept > 0 && kept < k1.parts.len(), "{kept} parts left");
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
        }
        let resting = wreck
            .iter()
            .filter(|&&d| u.powered(d).is_some_and(|p| p.landed))
            .count();
        assert!(resting > 0, "the pieces come to rest on the ground");
    }

    #[test]
    fn crashing_at_speed_destroys_the_craft_whichever_way_it_hits() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec.clone(), 0.0, 0.0);
        u.set_structure(v, st);
        u.step();
        let p = u.powered(v).unwrap().clone();
        let omega = DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425);
        // (nose angle from up, sink m/s, sideways m/s)
        for (lean, sink, side) in [
            (0.0, 150.0, 0.0),
            (std::f64::consts::PI, 150.0, 0.0),
            (1.5, 40.0, 200.0),
            (0.3, 60.0, 100.0),
        ] {
            u.world.despawn(v);
            let v = u.spawn_powered_on_surface("K1", body, spec.clone(), 0.0, 0.0);
            u.set_structure(v, k1_on_real_kerbin().3);
            let mut q = p.clone();
            let up = p.r.normalized();
            let east = DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
            q.landed = false;
            q.r = up * (p.r.length() + 30.0);
            q.v = omega.cross(q.r) - up * sink + east * side;
            q.facing = (up * f64::cos(lean) + east * f64::sin(lean)).normalized();
            q.attitude = flight::Attitude::Free;
            q.throttle = 0.0;
            u.set_powered(v, q);
            let mut seen = Vec::new();
            for i in 0..(3 * TICKS_PER_SECOND) {
                u.step();
                match u.powered(v) {
                    Some(q) => {
                        if i % 10 == 0 {
                            seen.push((
                                i,
                                (q.r.length() - 600_000.0) as i64,
                                q.landed,
                                q.crashed,
                                (q.v - omega.cross(q.r)).length() as i64,
                            ));
                        }
                    }
                    None => {
                        seen.push((i, 0, false, true, 0));
                        break;
                    }
                }
            }
            let q = u.powered(v);
            eprintln!("lean {lean} sink {sink} side {side}: {seen:?}");
            assert!(
                q.is_none_or(|q| q.crashed),
                "lean {lean} sink {sink} side {side}: survived {seen:?}"
            );
        }
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
    fn ksp2_rover_turn_at_half_throttle_and_full_steer_matches_run_8() {
        // KSP2 trace run 8 (reference/ksp2-run8-rover): the stock Inquisitive Rover from rest at half wheel
        // throttle and full steer reaches 0.95 / 1.38 / 1.87 / 2.25 m/s after 2 / 4 / 8 / 10 s, turning at
        // 0.51 / 0.86 / 1.20 / 1.41 rad/s. OSE (single-precision wheels, weight shifted onto the outer
        // wheels): 0.93 / 1.44 / 2.07 / 2.35 m/s at 0.43 / 0.84 / 1.30 / 1.52 rad/s
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
        assert!(u.set_drive(v, 0.5, 1.0, false));
        let ksp2 = [
            (2, 0.95, 0.51),
            (4, 1.38, 0.86),
            (8, 1.87, 1.20),
            (10, 2.25, 1.41),
        ];
        let mut next = 0;
        for t in 1..=(10 * TICKS_PER_SECOND) {
            u.step();
            if next < ksp2.len() && t == ksp2[next].0 * TICKS_PER_SECOND {
                let (s, speed, yaw) = ksp2[next];
                let d = &u.powered(v).unwrap().drive;
                assert!(
                    (d.speed_mps - speed).abs() < 0.25 * speed,
                    "{s} s: {:.2} m/s, KSP2 {speed}",
                    d.speed_mps
                );
                assert!(
                    (d.yaw_rate_rad_s.abs() - yaw).abs() < 0.25 * yaw,
                    "{s} s: yaw {:.2} rad/s, KSP2 {yaw}",
                    d.yaw_rate_rad_s
                );
                next += 1;
            }
        }
        assert_eq!(next, ksp2.len());
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

    /// Stock Inquisitive Rover on level Kerbin ground driven with the KSP2 parity schedule (T16): full wheel
    /// throttle from 1 s for 15 s, coast 3 s, brakes 8 s, half throttle and full right steering 10 s, then
    /// nothing. Samples every tick: (t, ground speed, yaw rate).
    fn rover_parity_run() -> Vec<(f64, f64, f64)> {
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
        let schedule = [
            (1.0, 1.0, 0.0, false),
            (16.0, 0.0, 0.0, false),
            (19.0, 0.0, 0.0, true),
            (27.0, 0.5, 1.0, false),
            (37.0, 0.0, 0.0, false),
        ];
        let mut out = Vec::new();
        let mut next = 0;
        let mut heading = u.powered(v).unwrap().drive.heading_rad;
        for tick in 0..(41 * TICKS_PER_SECOND) {
            let t = tick as f64 / TICKS_PER_SECOND as f64;
            while next < schedule.len() && t >= schedule[next].0 {
                let (_, throttle, steer, brakes) = schedule[next];
                u.set_drive(v, throttle, steer, brakes);
                next += 1;
            }
            u.step();
            let p = u.powered(v).unwrap();
            assert!(
                p.landed && !p.tipped && !p.crashed,
                "on its wheels at {t} s"
            );
            let turn = (p.drive.heading_rad - heading + std::f64::consts::PI)
                .rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            heading = p.drive.heading_rad;
            out.push((
                t + 1.0 / TICKS_PER_SECOND as f64,
                p.drive.speed_mps.hypot(p.drive.lateral_mps),
                turn * TICKS_PER_SECOND as f64,
            ));
        }
        if std::env::var_os("OSE_ROVER_TRACE").is_some() {
            for (t, speed, yaw) in &out {
                println!("rover,{t:.2},{speed:.4},{yaw:.4}");
            }
        }
        out
    }

    #[test]
    fn a_rover_accelerates_brakes_and_turns_like_ksp2s_inquisitive_rover() {
        let run = rover_parity_run();
        let at = |t: f64| {
            *run.iter()
                .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
                .unwrap()
        };
        // KSP2 v0.2.2, same craft and schedule (reference/ksp2-run8-rover): its light wheels spin up slowly
        // (the 0.01 t·m² least wheel inertia) and the motor torque curve, rounded to 0.01 kN·m, runs out
        // near 10 m/s
        for (t, ksp2) in [
            (2.0, 1.78),
            (3.0, 3.32),
            (5.0, 5.89),
            (8.0, 7.79),
            (16.0, 9.83),
        ] {
            let speed = at(t).1;
            assert!(
                (speed - ksp2).abs() < 0.1 * ksp2,
                "{speed:.2} m/s at {t} s, KSP2 {ksp2}"
            );
        }
        // the brakes come on over a second, then hold the wheels and stop it
        assert!(at(20.0).1 > 7.0, "{:?}", at(20.0));
        assert!(at(21.5).1 < 0.3, "{:?}", at(21.5));
        // half throttle with full steering: a tight, slow turn (KSP2 1.20/1.62/2.24 m/s turning at
        // 0.70/1.05/1.41 rad/s). The turn builds up against the craft's yaw inertia, which here still
        // includes the physicsless parts' own inertia (OSE 1.24/1.79/2.40 m/s at 0.66/1.09/1.52 rad/s,
        // with the weight shifted onto the outer wheels as KSP2's suspension does).
        for (t, ksp2, yaw) in [(30.0, 1.20, 0.70), (33.0, 1.62, 1.05), (37.0, 2.24, 1.41)] {
            let (_, speed, rate) = at(t);
            assert!(
                speed > 0.6 * ksp2 && speed < 1.15 * ksp2,
                "{speed:.2} m/s at {t} s, KSP2 {ksp2}"
            );
            assert!(
                rate > 0.55 * yaw && rate < 1.2 * yaw,
                "{rate:.2} rad/s at {t} s, KSP2 {yaw}"
            );
        }
        // let go: the wheels straighten and the turn dies away
        assert!(at(38.0).2 < at(37.0).2, "{:?} {:?}", at(37.0), at(38.0));
        assert!(at(41.0).2 < 0.6, "{:?}", at(41.0));
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
            p.roll_rate > flight::ROLL_RATE_RAD_S,
            "reference manual roll applies torque rather than a rate cap: {}",
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
    fn a_landed_jets_control_surfaces_follow_the_keys() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let c = structure::level_layout(c).unwrap();
        let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Craft", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(&c, &defs, &res));
        assert!(u.set_level(v, 90.0));
        u.step();
        for (input, what) in [
            (DVec3::new(1.0, 0.0, 0.0), "pitch"),
            (DVec3::new(0.0, 1.0, 0.0), "yaw"),
            (DVec3::new(0.0, 0.0, 1.0), "roll"),
        ] {
            assert!(u.set_rotation(v, input.x, input.y, input.z));
            for _ in 0..150 {
                u.step();
            }
            let p = u.powered(v).unwrap().clone();
            assert!(p.landed, "{what}");
            let d = u
                .structure(v)
                .unwrap()
                .surface_deflections(flight::vessel_basis(&p), p.steer_cmd);
            let most = d.iter().map(|x| x.1.abs()).fold(0.0, f64::max);
            assert!(
                most > 1.0_f64.to_radians(),
                "{what}: surfaces visibly move on the ground: {d:?} cmd {:?}",
                p.steer_cmd
            );
            assert!(u.set_rotation(v, -input.x, -input.y, -input.z));
            // Authored hinges take time to travel through neutral after a reversal.
            for _ in 0..150 {
                u.step();
            }
            let reverse = u.powered(v).unwrap();
            assert!(reverse.landed);
            let opposite = u
                .structure(v)
                .unwrap()
                .surface_deflections(flight::vessel_basis(reverse), reverse.steer_cmd);
            assert_eq!(d.len(), opposite.len());
            for ((part, angle), (other, reversed)) in d.iter().zip(&opposite) {
                assert_eq!(part, other);
                assert!(
                    angle.abs() < 1e-5 || angle * reversed < 0.0,
                    "{what}: reversing the key reverses deflection"
                );
            }
        }
    }

    #[test]
    fn a_plane_flying_into_the_ground_nose_first_crashes() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let c = structure::level_layout(c).unwrap();
        let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Craft", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(&c, &defs, &res));
        assert!(u.set_level(v, 90.0));
        u.step();
        let p0 = u.powered(v).unwrap().clone();
        let up = p0.r.normalized();
        let omega = DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425);
        // (nose below the horizon, rad; sink m/s; crashes)
        for (dip, sink, crashes) in [(0.0, 2.0, false), (0.6, 100.0, true)] {
            let mut q = p0.clone();
            q.landed = false;
            // A shallow touchdown starts just above support. Dropping from 10 m
            // would add about 14 m/s of gravity-driven sink before contact.
            q.r = up * (p0.r.length() + if crashes { 10.0 } else { 0.01 });
            q.facing = (p0.facing * f64::cos(dip) - up * f64::sin(dip)).normalized();
            q.v = omega.cross(q.r) + p0.facing * 120.0 - up * sink;
            q.throttle = 0.0;
            q.attitude = flight::Attitude::Free;
            u.set_powered(v, q);
            for _ in 0..(3 * TICKS_PER_SECOND) {
                u.step();
            }
            let q = u.powered(v);
            assert_eq!(
                q.is_none_or(|q| q.crashed),
                crashes,
                "dip {dip} sink {sink}"
            );
        }
    }

    /// KSP2 run 20 (T41): a plane on free-rolling gear, pushed from rest, accelerates at its push less the
    /// wheels' own spin-up; it must not stick at a crawl.
    #[test]
    fn a_plane_on_free_wheels_rolls_away_from_rest() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Stock04_Jet").unwrap();
        let c = structure::level_layout(c).unwrap();
        let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Craft", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(&c, &defs, &res));
        assert!(u.set_level(v, 90.0));
        u.step();
        let mut p = u.powered(v).unwrap().clone();
        assert_eq!(p.drive.wheels.len(), 3);
        // held on its brakes against 15 kN first, as on the runway before the roll
        p.drive.brakes = true;
        for _ in 0..100 {
            p.drive.step(
                p.mass_t,
                9.81,
                1.0,
                [0.0, 0.0],
                [15.0 / p.mass_t, 0.0],
                0.02,
            );
        }
        assert!(p.drive.speed_mps.abs() < 0.2, "{}", p.drive.speed_mps);
        p.drive.brakes = false;
        // 5 kN of thrust, as the Wheesley gives 2.5 s into its spool-up; the tyres' smoothed brake load
        // lets go over the first quarter second, as KSP2's `tireForceSharpness` does
        let push = 5.0 / p.mass_t;
        let mut at_1s = 0.0;
        for k in 1..=200 {
            p.drive
                .step(p.mass_t, 9.81, 1.0, [0.0, 0.0], [push, 0.0], 0.02);
            if k == 50 {
                at_1s = p.drive.speed_mps;
            }
        }
        let slowest = (p.drive.speed_mps - at_1s) / (push * 3.0);
        // KSP2 loses about 0.2 kN to the wheels here; the spin-up of three gear wheels is about 0.5 kN
        assert!(slowest > 0.85, "rolls at {slowest:.2} of its push");
    }

    #[test]
    fn a_truck_on_the_runway_stands_on_its_wheels() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Stock09_Truck").unwrap();
        let c = structure::level_layout(c).unwrap();
        let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let v = u.spawn_powered_on_surface("Craft", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(&c, &defs, &res));
        assert!(u.set_level(v, 90.0));
        u.step();
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        let p = u.powered(v).unwrap().clone();
        let up = p.r.normalized();
        assert!(p.landed && !p.tipped && p.level, "{p:?}");
        assert!(p.facing.dot(up).abs() < 0.05, "lies along the ground");
        assert_eq!(p.drive.wheels.len(), 8);
        // The laid-out wheel centres must lie below the cab in the actual flight frame.
        let wheels: Vec<f64> = c
            .parts
            .iter()
            .filter(|x| x.part_id == "wheel_2v_truck")
            .map(|x| {
                let (bx, by, bz) = flight::vessel_basis(&p);
                let v = rigid::to_vessel(x.local_position);
                (bx * v.x + by * v.y + bz * v.z).dot(up)
            })
            .collect();
        let cockpit = c.parts.iter().find(|x| x.parent.is_none()).unwrap();
        assert!(
            wheels.iter().all(|z| {
                let (bx, by, bz) = flight::vessel_basis(&p);
                let v = rigid::to_vessel(cockpit.local_position);
                *z < (bx * v.x + by * v.y + bz * v.z).dot(up) - 0.5
            }),
            "wheels below the cab: {wheels:?}"
        );
        assert!(
            p.ground_clearance_m < 3.0,
            "on its wheels: {}",
            p.ground_clearance_m
        );
    }

    #[test]
    fn dropped_boosters_fall_away_as_their_own_crafts() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        p.throttle = 0.001;
        u.set_powered(v, p);
        u.set_craft(v, k1.clone());
        u.set_structure(v, st);
        u.step();
        // press through the stages, keeping what falls off
        let mut debris = Vec::new();
        for _ in 0..4 {
            let r = u.stage(v);
            debris.extend(r.debris);
            debris.extend(u.take_broken_off().into_iter().map(|(_, d)| d));
        }
        let boosters: Vec<Entity> = debris
            .iter()
            .copied()
            .filter(|&d| {
                u.current_craft(d).is_some_and(|c| {
                    c.parts
                        .iter()
                        .any(|p| p.part_id == "booster_1v_solid_hammer")
                })
            })
            .collect();
        assert!(
            debris.iter().all(|&d| u.current_craft(d).is_some()),
            "every piece has a craft"
        );
        assert_eq!(boosters.len(), 4, "one piece per booster");
        let debris = boosters;
        let parent = u.powered(v).unwrap().clone();
        for d in debris {
            let c = u.current_craft(d).expect("debris has a craft");
            assert!(
                c.is_tree() && c.parts.len() == 3,
                "{:?}",
                c.parts.iter().map(|p| &p.part_id).collect::<Vec<_>>()
            );
            assert!(c
                .parts
                .iter()
                .any(|p| p.part_id == "booster_1v_solid_hammer"));
            let q = u.powered(d).unwrap();
            assert!(
                q.facing.dot(parent.facing) > 0.99,
                "leaves with the rocket's attitude"
            );
            assert!(
                q.angvel.length() > 0.01,
                "and tips away: {:?} off {:?}",
                q.angvel,
                q.r - parent.r
            );
            // beside the rocket, not inside it
            let off = q.r - parent.r;
            assert!(
                (off - parent.facing * off.dot(parent.facing)).length() > 0.5,
                "to the side: {off:?}"
            );
        }
    }

    #[test]
    fn control_from_a_part_turns_the_frame_but_not_the_vessel() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        // the truck's cab is turned a quarter about X in its craft
        let c = crafts.iter().find(|c| c.name == "Stock09_Truck").unwrap();
        let spec_for = |c: &ose_data::craft::Craft| {
            let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
            flight::FlightSpec::from_report(&report, &[], &[], 1.5)
        };
        let v = u.spawn_vessel_in_circular_orbit("Truck", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec_for(c),
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.2, 0.6, 0.77).normalized();
        p.throttle = 0.001;
        p.attitude = flight::Attitude::Free;
        u.set_powered(v, p);
        u.set_craft(v, c.clone());
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        u.step();
        let axes = |u: &Universe| -> Vec<(DVec3, DVec3)> {
            let st = u.structure(v).unwrap();
            let f = u.powered(v).unwrap().facing;
            (0..st.rigid.len())
                .map(|i| {
                    let q = st.part_rot[i];
                    (
                        st.axis_inertial(f, aero::rotate(q, DVec3::new(0.0, 1.0, 0.0))),
                        st.axis_inertial(f, aero::rotate(q, DVec3::new(1.0, 0.0, 0.0))),
                    )
                })
                .collect()
        };
        let before = axes(&u);
        let rev = u.craft_rev(v);
        let cab = 0;
        u.control_from(v, cab, &defs, &res, &spec_for).unwrap();
        assert_eq!(u.control_part(v), Some(cab));
        assert_ne!(u.craft_rev(v), rev);
        let after = axes(&u);
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            assert!(
                a.0.dot(b.0) > 0.999 && a.1.dot(b.1) > 0.999,
                "part {i} kept its place: {a:?} -> {b:?}"
            );
        }
        // the vessel now points along the cab's axis
        let f = u.powered(v).unwrap().facing;
        assert!(f.dot(after[cab].0) > 0.999, "{f:?}");
        u.step();
        let again = axes(&u);
        assert!(again[3].0.dot(after[3].0) > 0.999, "and stays put");
    }

    #[test]
    fn a_part_moves_to_a_stage_that_has_not_fired() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let spec_for = |c: &ose_data::craft::Craft| {
            let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
            flight::FlightSpec::from_report(&report, &[], &[], 1.5)
        };
        let v = u.spawn_powered_on_surface("K1", body, spec_for(c), 0.0, 0.0);
        u.set_craft(v, c.clone());
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        u.step();
        let craft = u.current_craft(v).unwrap();
        let (from, part) = craft
            .stages
            .iter()
            .enumerate()
            .find_map(|(s, l)| l.first().map(|&p| (s, p)))
            .expect("a staged part");
        let to = craft.stages.len();
        u.move_to_stage(v, part, to, &defs, &res, &spec_for)
            .unwrap();
        let moved = u.current_craft(v).unwrap();
        // the new stage fires first (stages left empty drop out of the list)
        assert!(
            moved.stages.last().unwrap().contains(&part),
            "{:?}",
            moved.stages
        );
        assert_eq!(
            moved.stages.iter().filter(|l| l.contains(&part)).count(),
            1,
            "moved from {from}"
        );
        // once lit, the stage fired is out of reach
        u.stage(v);
        let lit = u.powered(v).unwrap().spec.stages[0].stage_id as usize;
        assert!(u.move_to_stage(v, 0, lit, &defs, &res, &spec_for).is_err());
        assert!(u.powered(v).unwrap().ignited, "still lit after the refit");
    }

    #[test]
    fn the_stage_stack_is_rearranged_in_flight_below_the_stage_that_burns() {
        let (mut u, body, _, _) = k1_on_real_kerbin();
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let c = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let spec_for = |c: &ose_data::craft::Craft| {
            let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
            flight::FlightSpec::from_report(&report, &[], &[], 1.5)
        };
        let v = u.spawn_powered_on_surface("K1", body, spec_for(c), 0.0, 0.0);
        u.set_craft(v, c.clone());
        u.set_structure(v, structure::Structure::build(c, &defs, &res));
        u.step();
        let craft = u.current_craft(v).unwrap();
        let n = craft.stages.len();
        assert!(n >= 3, "{:?}", craft.stages);
        // a part alone in a new stage between the bottom two cards
        let part = craft.stages[n - 2][0];
        let mut stages = craft.stages.clone();
        stages[n - 2].retain(|&x| x != part);
        stages.insert(n - 1, vec![part]);
        u.set_stages(v, stages.clone(), &defs, &res, &spec_for)
            .unwrap();
        let expect: Vec<Vec<usize>> = stages.iter().filter(|s| !s.is_empty()).cloned().collect();
        assert_eq!(u.current_craft(v).unwrap().stages, expect);
        // parts cannot be dropped or doubled
        let mut lost = expect.clone();
        lost[0].pop();
        assert!(u.set_stages(v, lost, &defs, &res, &spec_for).is_err());
        // once the first stage is lit it stays as it is; the rest can still be rearranged, even into a new
        // stage, and the burning stage keeps burning
        u.set_controls(v, 1.0, flight::Attitude::Up);
        u.stage(v);
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        let now = u.current_craft(v).unwrap().stages;
        let k = now.len();
        let mut fired = now.clone();
        let top = fired[0].pop().unwrap();
        fired[k - 1].push(top);
        assert!(
            u.set_stages(v, fired, &defs, &res, &spec_for).is_err(),
            "the lit stage is spent"
        );
        let mut split = now.clone();
        if split[0].len() > 1 {
            let x = split[0].pop().unwrap();
            split.insert(1, vec![x]);
        } else {
            split.swap(0, 1.min(k - 2));
        }
        u.set_stages(v, split.clone(), &defs, &res, &spec_for)
            .unwrap();
        assert_eq!(
            u.current_craft(v).unwrap().stages.last(),
            now.last(),
            "the lit stage is still the last"
        );
        let p = u.powered(v).unwrap().clone();
        assert!(p.ignited);
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        let q = u.powered(v).unwrap();
        let up = q.r.normalized();
        assert!(q.v.dot(up) > p.v.dot(up), "still climbing on the lit stage");
    }

    #[test]
    fn a_spinning_craft_stays_in_physics_while_sas_damps_it() {
        // KSP2 trace T5: SAS takes about 2 s to stop a 1 rad/s spin and wobbles on; going on rails mid-spin
        // froze it at once instead
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        // long coasting already: the rails hand-off is due as soon as the craft allows it
        p.coasting_since = Some(-60.0);
        u.set_powered(v, p);
        u.set_structure(v, st);
        u.step();
        let mut q = u.powered(v).unwrap().clone();
        q.angvel = DVec3::new(0.3, 0.0, 0.0);
        q.attitude = flight::Attitude::Fixed(q.facing);
        q.coasting_since = Some(-60.0);
        u.set_powered(v, q);
        let spin = |u: &Universe| {
            u.powered(v)
                .map(|p| (p.angvel + p.facing * p.roll_rate).length())
        };
        u.step();
        let first = spin(&u).expect("still under power while it spins");
        assert!(first > 0.25, "the spin is damped, not dropped: {first}");
        let mut last = first;
        for i in 0..(10 * TICKS_PER_SECOND) {
            u.step();
            match spin(&u) {
                Some(w) => {
                    // SAS settles it (it may wobble a little, as KSP2's does) and never adds spin
                    assert!(w <= first, "{w} above the spin it started with");
                    if i == 3 * TICKS_PER_SECOND {
                        assert!(w < 0.5 * first, "SAS is taking the spin out: {w}");
                    }
                    last = w;
                }
                // on rails only once it is still
                None => {
                    assert!(last < 1e-3, "went on rails spinning at {last} rad/s");
                    break;
                }
            }
        }
    }

    #[test]
    fn a_craft_holding_prograde_in_orbit_goes_back_on_rails() {
        // holding prograde turns the craft once an orbit (3e-3 rad/s at 100 km); that is not a spin
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let (r, vel) = u.vessel(v).unwrap().2.state_at(u.ut());
        let mut p = flight::Powered::new(spec, r, vel, false);
        p.facing = vel.normalized();
        p.attitude = flight::Attitude::OrbitPrograde;
        p.angvel = r.cross(vel) * (1.0 / r.length_squared());
        u.set_powered(v, p);
        u.set_structure(v, st);
        for _ in 0..(60 * TICKS_PER_SECOND) {
            u.step();
            if u.powered(v).is_none() {
                return;
            }
        }
        let p = u.powered(v).unwrap();
        panic!(
            "still under power after 60 s: |w| {:.2e}, coasting since {:?}",
            p.angvel.length(),
            p.coasting_since
        );
    }

    #[test]
    fn a_held_yaw_with_sas_off_keeps_turning_about_the_same_axis() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let mut p = flight::Powered::new(
            spec,
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::new(0.0, 2_246.0, 0.0),
            false,
        );
        p.facing = DVec3::new(0.3, 0.5, 0.81).normalized();
        for s in &mut p.spec.stages {
            s.moi_tm2 = 5.0;
            s.wheel_torque_knm = 5.0;
        }
        u.set_powered(v, p);
        u.set_structure(v, st);
        // a whisper of throttle keeps it off rails
        assert!(u.set_controls(v, 0.001, flight::Attitude::Free));
        u.step();
        let top0 = flight::vessel_top(u.powered(v).unwrap());
        assert!(u.set_rotation(v, 0.0, 1.0, 0.0));
        let mut turned = 0.0;
        let mut last = u.powered(v).unwrap().facing;
        for _ in 0..(20 * TICKS_PER_SECOND) {
            u.step();
            let p = u.powered(v).unwrap();
            assert!(
                p.facing.dot(top0).abs() < 0.02,
                "the nose stays in the yaw plane: {:?}",
                p.facing
            );
            let top = flight::vessel_top(p);
            assert!(top.dot(top0) > 0.999, "no roll creeps in: {top:?}");
            turned += last.dot(p.facing).clamp(-1.0, 1.0).acos();
            last = p.facing;
        }
        assert!(turned > 1.0, "it turned {turned} rad");
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
            assert!(!p.crashed, "crashed at step {i}: {p:?}");
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
        u.world.get_mut::<flight::Powered>(v).unwrap().ignited = false;
        u.set_rotation(v, 0.0, 0.0, 0.0);
        // Reference wheel torque ramps down after key release; measure the torque-free
        // tumble after that response has decayed, rather than treating release as instantaneous.
        for _ in 0..TICKS_PER_SECOND {
            u.step();
        }
        for _ in 0..(3 * TICKS_PER_SECOND) {
            u.step();
        }
        let p2 = u.powered(v).unwrap().clone();
        // Imported KSP2 rigid bodies follow PhysX 4.1's per-body angular-velocity
        // integration; strict free-momentum conservation is tested on the physical path.
        assert!(
            p2.angvel.length() + p2.roll_rate.abs() > 0.01,
            "still spinning with SAS off"
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
        // Reference damping continually relocks during braking, then holds the caught pose.
        let settled = p3.facing;
        for _ in 0..(5 * TICKS_PER_SECOND) {
            u.step();
        }
        assert!(
            u.powered(v).unwrap().facing.dot(settled) > 0.99,
            "holds the caught pose"
        );
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
    fn ksp2s_clock_starts_near_noon_at_the_ksc() {
        // KSP2's game starts at Y1 D1 00:00 with the sun high over the pad (and setting)
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let u = Universe::with_bodies(&bodies);
        let kerbin = u.body_entity("Kerbin").unwrap();
        let (lat, lon) = (-0.6836, -74.8198);
        let e0 = u.sun_elevation_deg(kerbin, lat, lon, 0.0);
        assert!(e0 > 60.0, "{e0}");
        assert!(
            u.sun_elevation_deg(kerbin, lat, lon, 600.0) < e0,
            "afternoon"
        );
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
        // The chute is staged once the capsule has fallen back into the low, thick air: armed while the
        // capsule is still supersonic it would open as soon as its safety check passed, and at Mach 1.8 a
        // Mk16 canopy overheats (`Module_Parachute.UpdateCut`; see the structure tests).
        let mut top: f64 = 0.0;
        loop {
            u.step();
            let p = u.powered(v).unwrap();
            top = top.max(p.r.length() - 600_000.0);
            if p.v.dot(p.r.normalized()) < 0.0 && p.r.length() - 600_000.0 < 5_000.0 {
                break;
            }
        }
        u.stage(v);
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

    /// The lowest point of any present part below the ground under it, m (positive: buried).
    fn deepest_burial(u: &Universe, v: Entity, body: Entity) -> f64 {
        let p = u.powered(v).unwrap();
        let st = u.structure(v).unwrap();
        let b = u.world.get::<bodies::Body>(body).unwrap();
        let (x, y, z) = flight::vessel_basis(p);
        let world = |q: DVec3| x * q.x + y * q.y + z * q.z;
        let centre = st.mass_properties().centre_m;
        let up = p.r.normalized();
        let ground = flight::ground_radius(b, None, p.r, u.ut());
        (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i])
            .map(|i| {
                let (c, h) = st.part_box(i);
                let bottom = (p.r + world(c - centre)).dot(up)
                    - h.x * x.dot(up).abs()
                    - h.y * y.dot(up).abs()
                    - h.z * z.dot(up).abs();
                ground - bottom
            })
            .fold(f64::NEG_INFINITY, f64::max)
    }

    #[test]
    fn a_parachute_landing_with_the_engine_still_on_leaves_nothing_under_the_ground() {
        // A reported flight: the starter rocket flies up, its pilot never drops the tank and engine, and it
        // comes down on its one parachute with the engine underneath.
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
        for _ in 0..(8 * TICKS_PER_SECOND) {
            u.step();
        }
        assert!(u.set_controls(v, 0.0, flight::Attitude::Up));
        assert!(u.arm_parachutes(v).unwrap() > 0);
        let parts = |u: &Universe| {
            let st = u.structure(v).unwrap();
            st.rigid.present.iter().filter(|x| **x).count()
        };
        let all = parts(&u);
        let mut landing_speed = 0.0;
        let spin = DVec3::new(0.0, 0.0, 2.0 * std::f64::consts::PI / 21_549.425);
        for _ in 0..(1_200 * TICKS_PER_SECOND) {
            let before = u.powered(v).unwrap().clone();
            u.step();
            let p = u.powered(v).unwrap();
            if p.landed || parts(&u) < all {
                landing_speed = (before.v - spin.cross(before.r)).length();
                break;
            }
        }
        // whatever broke on touchdown, the rest stays where it was and settles on top of the ground
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
            assert!(
                deepest_burial(&u, v, body) < 0.05,
                "a part sank {:.2} m into the ground ({landing_speed:.1} m/s, {} of {all} parts left)",
                deepest_burial(&u, v, body),
                parts(&u)
            );
        }
        let p = u.powered(v).unwrap();
        assert!(p.landed && !p.crashed, "{p:?}");
        // the open Mk16 (DragMultiplier 2) brings even the engine down below its 10 m/s tolerance
        assert!(landing_speed < 10.0, "{landing_speed}");
        assert_eq!(parts(&u), all, "the engine survives the landing");
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
        assert!(p.r.length() - 600_000.0 > 40_000.0, "climbing: {p:?}");
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
    fn action_groups_run_the_part_actions_bound_to_them() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, st);
        u.step();
        let engines = u.structure(v).unwrap().live_engines(0);
        assert!(!engines.is_empty());
        let e0 = engines[0];
        // custom group 1 runs the first engine; nothing else is bound
        assert!(u
            .set_part_actions(
                v,
                e0,
                vec![("engine".into(), ose_data::craft::custom_group_bit(1))]
            )
            .is_ok());
        assert!(u.set_part_actions(v, e0, vec![("warp".into(), 1)]).is_err());
        assert_eq!(
            u.action_group(v, ActionGroup::Custom(1), Some(false)),
            Some(false)
        );
        let live = u.structure(v).unwrap().live_engines(0);
        assert!(!live.contains(&e0), "shut down");
        assert_eq!(live.len(), engines.len() - 1, "the others run on");
        u.action_group(v, ActionGroup::Custom(1), Some(true));
        assert!(u.structure(v).unwrap().live_engines(0).contains(&e0));
        // a group nothing is bound to changes no part
        u.action_group(v, ActionGroup::Custom(2), Some(false));
        assert_eq!(u.structure(v).unwrap().live_engines(0).len(), engines.len());
        // and a part's own menu
        assert!(u.part_action(v, e0, "engine", false));
        assert!(
            !u.part_action(v, 0, "engine", false),
            "the pod has no engine"
        );
    }

    #[test]
    fn a_planned_burn_is_flown_by_the_engines_on_fuel() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let (r, vel) = u.vessel(v).unwrap().2.state_at(u.ut());
        let mut p = flight::Powered::new(spec, r, vel, false);
        p.facing = vel.normalized();
        // the upper stage (the last with an engine), lit
        p.ignited = true;
        p.stage = p
            .spec
            .stages
            .iter()
            .rposition(|s| s.thrust_vac_kn > 0.0)
            .unwrap();
        u.set_dormant(v, p);
        u.set_structure(v, st);
        let fuel0 = u
            .dormant(v)
            .or(u.powered(v))
            .map(|p| p.propellant_t)
            .unwrap();
        let ap0 = u.vessel(v).unwrap().2.apoapsis();
        let burn = vessels::ScheduledBurn {
            ut: u.ut() + 120.0,
            prograde_mps: 100.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        };
        assert!(u.schedule_burn(v, burn));
        let start = u.burn_start(v).unwrap();
        assert_eq!(
            start, burn.ut,
            "it starts at the node, as KSP2's burn timer says"
        );
        // KSP2's prediction of the orbit after it (a finite burn from the node)
        let predicted = u
            .predict(v)
            .into_iter()
            .find(|p| p.start == trajectory::Transition::Maneuver)
            .expect("a patch after the burn")
            .orbit;
        // passing the node time changes nothing by itself: the engines fly it
        u.set_warp(4);
        for _ in 0..(400 * TICKS_PER_SECOND / 4) {
            u.step();
            if u.scheduled_burn(v).is_none() || u.burn_stopped(v).is_some() {
                break;
            }
        }
        assert!(u.burn_stopped(v).is_none(), "{:?}", u.burn_stopped(v));
        assert!(u.scheduled_burn(v).is_none(), "flown");
        let p = u.powered(v).or(u.dormant(v)).unwrap().clone();
        assert!(
            p.propellant_t < fuel0,
            "it burned fuel: {fuel0} -> {}",
            p.propellant_t
        );
        let orbit = Orbit::from_state_vectors(
            u.world.get::<bodies::Body>(body).unwrap().mu,
            p.r,
            p.v,
            u.ut(),
        );
        let ap_impulse = vessels::orbit_after_burn(
            &u.vessel(v).unwrap().2,
            &vessels::ScheduledBurn { ut: u.ut(), ..burn },
        );
        let _ = ap_impulse;
        assert!(
            orbit.apoapsis() > ap0 + 100_000.0,
            "raised: {ap0} -> {}",
            orbit.apoapsis()
        );
        // T9: flown from the node as KSP2's timer says, it ends on the orbit predicted for it within 1%
        assert!(
            (orbit.apoapsis() - predicted.apoapsis()).abs() < 0.01 * predicted.apoapsis(),
            "apoapsis {:.0} m, predicted {:.0} m",
            orbit.apoapsis(),
            predicted.apoapsis()
        );
        assert!(
            (orbit.periapsis() - predicted.periapsis()).abs() < 0.01 * predicted.periapsis(),
            "periapsis {:.0} m, predicted {:.0} m",
            orbit.periapsis(),
            predicted.periapsis()
        );
        // a vessel with no engines does not move for its node
        let probe = u.spawn_vessel_in_circular_orbit("probe", body, 120_000.0);
        let before = u.vessel(probe).unwrap().2.apoapsis();
        assert!(u.schedule_burn(
            probe,
            vessels::ScheduledBurn {
                ut: u.ut() + 10.0,
                ..burn
            }
        ));
        u.set_warp(100);
        for _ in 0..10 {
            u.step();
        }
        assert!((u.vessel(probe).unwrap().2.apoapsis() - before).abs() < 1.0);
    }

    #[test]
    fn clearing_the_node_or_taking_the_controls_stops_the_burn_being_flown() {
        let (mut u, body, spec, st) = k1_on_real_kerbin();
        let v = u.spawn_vessel_in_circular_orbit("K1", body, 100_000.0);
        let (r, vel) = u.vessel(v).unwrap().2.state_at(u.ut());
        let mut p = flight::Powered::new(spec, r, vel, false);
        // pointed well away from the burn, so the engines wait for the turn
        p.facing = r.normalized();
        p.ignited = true;
        p.stage = p
            .spec
            .stages
            .iter()
            .rposition(|s| s.thrust_vac_kn > 0.0)
            .unwrap();
        u.set_dormant(v, p);
        u.set_structure(v, st);
        let burn = vessels::ScheduledBurn {
            ut: u.ut() + 5.0,
            prograde_mps: 100.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        };
        let fly_it = |u: &mut Universe| {
            let ut = u.ut() + 5.0;
            assert!(u.schedule_burn(v, vessels::ScheduledBurn { ut, ..burn }));
            // the burn starts at the node, 5 s on
            for _ in 0..(6 * TICKS_PER_SECOND) {
                u.step();
            }
            assert!(u.burn_left(v).is_some(), "the burn is under way");
        };
        let throttle_holds = |u: &mut Universe, how: &str| {
            for _ in 0..TICKS_PER_SECOND {
                u.step();
            }
            let p = u.powered(v).unwrap();
            assert!(u.burn_left(v).is_none(), "{how}: no burn flown");
            assert!(
                (p.throttle - 0.7).abs() < 1e-9,
                "{how}: throttle {}",
                p.throttle
            );
            assert!(
                !matches!(p.attitude, flight::Attitude::Maneuver(_)),
                "{how}"
            );
        };
        // clearing the node gives the throttle back
        fly_it(&mut u);
        assert!(u.schedule_burns(v, Vec::new()));
        let facing = u.facing(v).unwrap();
        assert!(u.set_controls(v, 0.7, flight::Attitude::Fixed(facing)));
        throttle_holds(&mut u, "cleared");
        // so does another hold, with the node still planned
        assert!(u.set_controls(v, 0.0, flight::Attitude::Maneuver(DVec3::ZERO)));
        fly_it(&mut u);
        assert!(u.set_controls(v, 0.7, flight::Attitude::OrbitPrograde));
        throttle_holds(&mut u, "prograde hold");
        // and steering by hand
        assert!(u.schedule_burns(v, Vec::new()));
        assert!(u.set_controls(v, 0.0, flight::Attitude::Maneuver(DVec3::ZERO)));
        fly_it(&mut u);
        assert!(u.set_rotation(v, 0.0, 1.0, 0.0));
        assert!(u.burn_left(v).is_none(), "steering takes over");
        assert!(u.scheduled_burn(v).is_some(), "the node stays planned");
        assert!(u.set_rotation(v, 0.0, 0.0, 0.0));
        let att = u.powered(v).unwrap().attitude;
        assert!(u.set_controls(v, 0.7, att));
        throttle_holds(&mut u, "steered");
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
        // the old impulsive burns, kept for development
        u.set_free_burns(true);
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
        // two chained nodes: a small raise, then a circularising burn half an orbit on
        let t0 = u.ut();
        let (_, _, orbit) = u.vessel(v).unwrap();
        let ap0 = orbit.apoapsis();
        let raise = vessels::ScheduledBurn {
            ut: t0 + 500.0,
            prograde_mps: 50.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        };
        let more = vessels::ScheduledBurn {
            ut: t0 + 900.0,
            prograde_mps: 30.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        };
        assert!(
            u.schedule_burns(v, vec![more, raise]),
            "taken in time order"
        );
        assert_eq!(u.scheduled_burns(v), vec![raise, more]);
        let p = u.predict(v);
        let nodes: Vec<f64> = p
            .iter()
            .filter(|x| x.end == trajectory::Transition::Maneuver)
            .map(|x| x.end_ut)
            .collect();
        assert_eq!(nodes, vec![t0 + 500.0, t0 + 900.0], "{p:?}");
        let last = p.iter().find(|x| x.start_ut >= t0 + 900.0).unwrap();
        assert!(
            last.orbit.apoapsis() > ap0,
            "the patches go on through both nodes"
        );
        u.set_warp(100_000);
        u.step();
        assert_eq!(u.scheduled_burns(v).len(), 1, "the first is done");
        u.step();
        assert!(u.scheduled_burns(v).is_empty(), "then the second");
        let (_, _, orbit) = u.vessel(v).unwrap();
        assert!((orbit.apoapsis() - last.orbit.apoapsis()).abs() < 1_000.0);
        assert!(u.schedule_burns(v, vec![]));
    }
}

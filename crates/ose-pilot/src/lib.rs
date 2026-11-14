//! An unattended pilot that flies a vessel through the same controls a player uses: attitude,
//! throttle, staging and RCS translation. Nothing is teleported, and no burn is applied as an
//! impulse. Docking capture stays with the simulation (`Universe::dock_latched`), exactly as for a
//! player.
//!
//! Every controller is tick-driven: call `update` once before each `Universe::step`, and it returns
//! whether it is still running, done, or failed. The server runs route vessels this way, and tests
//! run the same code headless. Rendezvous v2, step R2 (`docs/rendezvous-v2-plan.md`).
use bevy_ecs::entity::Entity;
use ose_math::{DVec3, Orbit};
use ose_sim::{bodies, Universe};

pub mod approach;
pub mod ascent;
pub mod burn;
pub mod mission;

/// What a controller reports after a tick.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Running,
    Done,
    Failed(String),
}

/// The vessel's position and velocity relative to its reference body, powered or on rails.
pub fn state(u: &Universe, e: Entity) -> Option<(DVec3, DVec3)> {
    if let Some(p) = u.powered(e) {
        return Some((p.r, p.v));
    }
    Some(u.world.get::<bodies::OnRails>(e)?.0.state_at(u.ut()))
}

/// The body the vessel orbits, with its gravitational parameter and radius.
pub fn body(u: &Universe, e: Entity) -> Option<(Entity, f64, f64)> {
    let b = u.world.get::<bodies::ReferenceBody>(e)?.0;
    let body = u.world.get::<bodies::Body>(b)?;
    Some((b, body.mu, body.radius_m))
}

/// The vessel's current osculating orbit.
pub fn orbit(u: &Universe, e: Entity) -> Option<Orbit> {
    let (r, v) = state(u, e)?;
    let (_, mu, _) = body(u, e)?;
    Some(Orbit::from_state_vectors(mu, r, v, u.ut()))
}

/// Vacuum acceleration of the active stage at full throttle (m/s²), or 0 with no engine. Works on
/// rails too, from the vessel's dormant flight state.
pub fn acceleration(u: &Universe, e: Entity) -> f64 {
    u.powered(e)
        .or_else(|| u.dormant(e))
        .map(|p| {
            p.spec
                .stages
                .get(p.stage)
                .map(|s| s.thrust_vac_kn)
                .unwrap_or(0.0)
                / p.mass_t.max(1e-6)
        })
        .unwrap_or(0.0)
}

/// Stage when the active stage has run dry and another stage is left. Returns whether it staged.
pub fn stage_if_spent(u: &mut Universe, e: Entity) -> bool {
    let Some(p) = u.powered(e) else {
        return false;
    };
    if p.propellant_t < 0.01 && p.stage + 1 < p.spec.stages.len() {
        u.stage(e);
        return true;
    }
    false
}

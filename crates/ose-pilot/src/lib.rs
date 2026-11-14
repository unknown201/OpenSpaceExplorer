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
pub mod depart;
pub mod mission;

/// Attitude commands and RCS translation are in the control point's axes, which differ from the
/// craft's own axes after "control from here" or undocking (a shuttle can come away controlled
/// back to front). Returns (right, fore, top) of the control point, inertial.
pub fn control_axes(p: &ose_sim::flight::Powered) -> (DVec3, DVec3, DVec3) {
    ose_sim::flight::control_basis(p)
}

/// The direction to hold (`Attitude::Fixed`, which steers the control point's fore axis) so that
/// the craft axis `axis` (inertial, now) turns to point along `want`.
pub fn aim(p: &ose_sim::flight::Powered, axis: DVec3, want: DVec3) -> DVec3 {
    let g = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
    let turn = glam::DQuat::from_rotation_arc(g(axis.normalized()), g(want.normalized()));
    let fore = turn * g(control_axes(p).1);
    DVec3::new(fore.x, fore.y, fore.z)
}

/// Translation input (control axes) for an inertial direction and magnitude.
pub fn translation_input(p: &ose_sim::flight::Powered, command: DVec3) -> DVec3 {
    let (right, fore, top) = control_axes(p);
    DVec3::new(command.dot(right), command.dot(fore), command.dot(top))
}

/// The RCS mixer ignores inputs under 0.05. Small inputs are accumulated and sent as short 0.1
/// pulses, like a player tapping translation keys.
pub fn pulsed(pulses: &mut [f64; 3], input: DVec3) -> DVec3 {
    let axes = [input.x, input.y, input.z];
    let out: [f64; 3] = std::array::from_fn(|i| {
        if axes[i].abs() >= 0.05 {
            return axes[i];
        }
        pulses[i] += axes[i];
        if pulses[i].abs() >= 0.1 {
            let pulse = pulses[i].signum() * 0.1;
            pulses[i] -= pulse;
            pulse
        } else {
            0.0
        }
    });
    DVec3::new(out[0], out[1], out[2])
}

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

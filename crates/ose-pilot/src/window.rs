//! Launch windows from any landed site: when to lift off so the ascent ends in the target's orbital
//! plane and a little behind the target. Works on any body, from its rotation, the site's position
//! and the target's orbit. Rendezvous v2, step R5.
//!
//! The ascent is described by a [`LaunchProfile`]: how long it takes and how far around the orbit
//! it carries the vessel. Measure it from a real flight with [`crate::ascent::Ascent::profile`]
//! (a route's proving flight, or an earlier launch of the same rocket).
use bevy_ecs::entity::Entity;
use ose_math::{DVec3, Orbit};
use ose_sim::{bodies::Body, Universe};

/// How an ascent flies, measured from a real flight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchProfile {
    /// Lift-off to orbit (s).
    pub duration_s: f64,
    /// Angle from the launch site (inertial, at lift-off) to the insertion point, around the orbit
    /// normal (degrees).
    pub downrange_deg: f64,
}

/// A chosen lift-off time and how good it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchWindow {
    pub ut: f64,
    /// Angle of the site out of the target's plane at lift-off (degrees).
    pub plane_deg: f64,
    /// Target's angle ahead of the insertion point at insertion, minus the wanted lead (degrees).
    pub phase_error_deg: f64,
}

/// Score per degree out of plane. A degree of plane change in low orbit costs about 40 m/s.
const PLANE_WEIGHT: f64 = 40.0;
/// Score per degree of phase. Phase only costs time: a slightly lower orbit makes up a few degrees
/// per revolution.
const PHASE_WEIGHT: f64 = 0.5;

fn rotate_z(v: DVec3, angle: f64) -> DVec3 {
    let (s, c) = angle.sin_cos();
    DVec3::new(c * v.x - s * v.y, s * v.x + c * v.y, v.z)
}

/// Rotate `v` about the unit axis `k` by `angle` (Rodrigues).
fn rotate(v: DVec3, k: DVec3, angle: f64) -> DVec3 {
    let (s, c) = angle.sin_cos();
    v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c))
}

fn wrap_deg(a: f64) -> f64 {
    (a + 540.0).rem_euclid(360.0) - 180.0
}

/// Search lift-off times from `earliest` for the best window. `site` is the landed vessel's
/// body-centred position now. Plane error is weighted far above phase error, because a plane change
/// in orbit costs fuel while phasing only costs time; an hour of waiting counts like a degree of
/// phase, so an earlier window wins a near tie.
#[allow(clippy::too_many_arguments)]
pub fn search(
    body: &Body,
    site: DVec3,
    now: f64,
    target: &Orbit,
    profile: LaunchProfile,
    lead_deg: f64,
    earliest: f64,
    horizon_s: f64,
) -> LaunchWindow {
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let n = target.frame.z.normalized();
    // The best plane the site ever reaches: a site off the equator never lies in an equatorial
    // plane, so plane error is counted from there.
    let lat = site.normalized().z.abs().asin().to_degrees();
    let inclination = n.z.clamp(-1.0, 1.0).acos().to_degrees();
    let floor = (lat - inclination.min(180.0 - inclination)).max(0.0);
    let step = (horizon_s / 20_000.0).max(2.0);
    let mut best: Option<(f64, LaunchWindow)> = None;
    let mut t = earliest;
    while t <= earliest + horizon_s {
        let r = rotate_z(site, omega * (t - now)).normalized();
        let plane_deg = n.dot(r).clamp(-1.0, 1.0).asin().to_degrees().abs();
        let in_plane = (r - n * n.dot(r)).normalized();
        let insertion = rotate(in_plane, n, profile.downrange_deg.to_radians());
        let (rt, _) = target.state_at(t + profile.duration_s);
        let rt = (rt - n * n.dot(rt)).normalized();
        let ahead = insertion
            .cross(rt)
            .dot(n)
            .atan2(insertion.dot(rt))
            .to_degrees();
        let phase_error_deg = wrap_deg(ahead - lead_deg);
        let score = (plane_deg - floor).max(0.0) * PLANE_WEIGHT
            + phase_error_deg.abs() * PHASE_WEIGHT
            + (t - earliest) / 3_600.0;
        if best.is_none_or(|b| score < b.0) {
            best = Some((
                score,
                LaunchWindow {
                    ut: t,
                    plane_deg,
                    phase_error_deg,
                },
            ));
        }
        t += step;
    }
    best.expect("at least one sample").1
}

/// The best window for the landed vessel `e` to reach `target` (same body), searched from now over
/// `horizon_s`.
pub fn for_vessel(
    u: &Universe,
    e: Entity,
    target: Entity,
    profile: LaunchProfile,
    lead_deg: f64,
    horizon_s: f64,
) -> Option<LaunchWindow> {
    let (body_e, _, _) = crate::body(u, e)?;
    let body = u.world.get::<Body>(body_e)?;
    let (site, _) = crate::state(u, e)?;
    let target = crate::orbit(u, target)?;
    let now = u.ut();
    Some(search(
        body,
        site,
        now,
        &target,
        profile,
        lead_deg,
        now + 30.0,
        horizon_s,
    ))
}

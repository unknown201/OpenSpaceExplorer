//! Patched-conic trajectory prediction.
//!
//! Follows the transition rules in `docs/reference-notes/orbits.md`: from the current orbit, each patch ends in
//! a collision with its reference body, an encounter with one of that body's children, an escape to the parent,
//! or runs to a full period (`Final`). The original finds candidate encounters with a geometric conic-conic
//! intercept search; here the candidate closest approaches come from sampling the relative distance over the
//! patch window, then the same time-domain refinement (`SolveClosestApproach`, `SolveSOI_BSP`: halving steps,
//! tolerance 0.01 s, 50 iterations) and the same inbound test (`dot(dr, dv) < 0`).

use crate::bodies::{star_space_state, Body, BodyIndex, OnRails, ReferenceBody};
use bevy_ecs::prelude::*;
use ose_math::{DVec3, Orbit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Initial,
    Encounter,
    Escape,
    Collision,
    /// A scheduled burn cuts the patch here; the next patch starts on the post-burn orbit.
    Maneuver,
    Final,
}

/// One conic segment of a predicted trajectory.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub body: Entity,
    pub orbit: Orbit,
    pub start_ut: f64,
    /// `+inf` for a final hyperbolic leg.
    pub end_ut: f64,
    pub start: Transition,
    pub end: Transition,
}

#[derive(Clone, Copy, Debug)]
pub struct SolverParams {
    pub max_patches: usize,
    pub samples: usize,
    pub time_epsilon_s: f64,
    pub max_time_iterations: usize,
}

impl Default for SolverParams {
    fn default() -> Self {
        SolverParams {
            max_patches: 5,
            samples: 256,
            time_epsilon_s: 0.01,
            max_time_iterations: 50,
        }
    }
}

/// Star-space state of the body `body` at `ut` (its own orbit around its parent, plus ancestors).
fn body_rel(world: &World, body: Entity, ut: f64) -> (DVec3, DVec3) {
    // relative to the parent: just the body's own orbit
    match world.get::<OnRails>(body) {
        Some(r) => r.0.state_at(ut),
        None => (DVec3::ZERO, DVec3::ZERO),
    }
}

fn dist2(orbit: &Orbit, world: &World, child: Entity, ut: f64) -> f64 {
    (orbit.position_at(ut) - body_rel(world, child, ut).0).length_squared()
}

/// Halving-step refinement of a closest approach, as the original `SolveClosestApproach`.
#[allow(clippy::too_many_arguments)]
fn refine_closest(
    orbit: &Orbit,
    world: &World,
    child: Entity,
    mut ut: f64,
    mut dt: f64,
    min_ut: f64,
    max_ut: f64,
    p: &SolverParams,
) -> (f64, f64) {
    let mut best = dist2(orbit, world, child, ut);
    let mut it = 0;
    while dt > p.time_epsilon_s && it < p.max_time_iterations {
        let fwd = if ut + dt > max_ut {
            f64::MAX
        } else {
            dist2(orbit, world, child, ut + dt)
        };
        let back = if ut - dt < min_ut {
            f64::MAX
        } else {
            dist2(orbit, world, child, ut - dt)
        };
        best = best.min(fwd.min(back));
        if best == back {
            ut -= dt;
        } else if best == fwd {
            ut += dt;
        }
        dt /= 2.0;
        it += 1;
    }
    (ut, best.sqrt())
}

/// Bisection for the SOI crossing before the closest approach, as the original `SolveSOI_BSP`.
#[allow(clippy::too_many_arguments)]
fn refine_soi(
    orbit: &Orbit,
    world: &World,
    child: Entity,
    soi: f64,
    mut ut: f64,
    mut dt: f64,
    min_ut: f64,
    max_ut: f64,
    p: &SolverParams,
) -> f64 {
    let soi2 = soi * soi;
    let f = |t: f64| dist2(orbit, world, child, t) - soi2;
    let mut cur = f(ut).abs();
    let mut it = 0;
    while dt > p.time_epsilon_s && it < p.max_time_iterations {
        let fwd = if ut + dt > max_ut {
            f64::MAX
        } else {
            f(ut + dt).abs()
        };
        let back = if ut - dt < min_ut {
            f64::MAX
        } else {
            f(ut - dt).abs()
        };
        cur = cur.min(fwd.min(back));
        if cur == back {
            ut -= dt;
        } else if cur == fwd {
            ut += dt;
        }
        dt /= 2.0;
        it += 1;
    }
    ut
}

/// Predict the trajectory of an object on `orbit` around `body` starting at `ut`.
pub fn predict(
    world: &World,
    index: &BodyIndex,
    body: Entity,
    orbit: Orbit,
    ut: f64,
    p: &SolverParams,
) -> Vec<Patch> {
    let mut patches = Vec::new();
    let mut cur_body = body;
    let mut cur_orbit = orbit;
    let mut start_ut = ut;
    let mut start = Transition::Initial;
    for _ in 0..p.max_patches {
        let (patch, next) = next_patch(world, index, cur_body, cur_orbit, start_ut, start, p);
        patches.push(patch);
        match next {
            Some((b, o)) => {
                cur_body = b;
                cur_orbit = o;
                start_ut = patch.end_ut;
                start = patch.end;
            }
            None => break,
        }
    }
    patches
}

type Next = Option<(Entity, Orbit)>;

fn handoff(world: &World, from: Entity, orbit: &Orbit, to: Entity, ut: f64) -> Orbit {
    // star-space state of the object = from-body star state + relative orbit state
    let (fp, fv) = star_space_state(world, from, ut);
    let (op, ov) = orbit.state_at(ut);
    let (tp, tv) = star_space_state(world, to, ut);
    let mu = world.get::<Body>(to).expect("body").mu;
    Orbit::from_state_vectors(mu, fp + op - tp, fv + ov - tv, ut)
}

fn next_patch(
    world: &World,
    index: &BodyIndex,
    body: Entity,
    orbit: Orbit,
    start_ut: f64,
    start: Transition,
    p: &SolverParams,
) -> (Patch, Next) {
    let b = world.get::<Body>(body).expect("body");
    let period_end = if orbit.eccentricity() < 1.0 {
        start_ut + orbit.period
    } else {
        f64::INFINITY
    };
    let mut patch = Patch {
        body,
        orbit,
        start_ut,
        end_ut: period_end,
        start,
        end: Transition::Final,
    };

    // 1. Collision with the reference body: like the original's `WillCollideWithParent` the highest terrain
    // decides whether to look at all, then the path ends where it first meets the ground under it (terrain,
    // the sea, the campus), as the original's map draws an impact rather than an orbit through the planet.
    let terrain = world
        .get::<crate::flight::Terrain>(body)
        .map(|t| t.0.clone());
    let surface = b.radius_m + terrain.as_ref().map(|t| t.max_m.max(0.0)).unwrap_or(0.0);
    if orbit.periapsis() < surface {
        let height = |t: f64| {
            let r = orbit.position_at(t);
            r.length() - crate::flight::ground_radius(b, terrain.as_deref(), r, t)
        };
        // up to the next periapsis, or a whole revolution when that lies behind
        let tpe = orbit.time_to_periapsis(start_ut);
        let span = if tpe > 0.0 && tpe.is_finite() {
            tpe
        } else if orbit.period.is_finite() {
            orbit.period
        } else {
            0.0
        };
        let steps = 512;
        let mut prev = (start_ut, height(start_ut));
        for k in 1..=steps {
            let t = start_ut + span * k as f64 / steps as f64;
            let h = height(t);
            if prev.1 >= 0.0 && h < 0.0 {
                let (mut lo, mut hi) = (prev.0, t);
                for _ in 0..50 {
                    let mid = 0.5 * (lo + hi);
                    if height(mid) >= 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                patch.end_ut = hi;
                patch.end = Transition::Collision;
                return (patch, None);
            }
            prev = (t, h);
        }
    }

    // 2. Encounters with children of this body, earliest first.
    let window_end = match b.soi_radius_m {
        _ if orbit.eccentricity() >= 1.0
            || orbit.apoapsis() > b.soi_radius_m.unwrap_or(f64::INFINITY) =>
        {
            // leaves the SOI: search until the exit
            match b.soi_radius_m {
                Some(soi) if orbit.apoapsis() > soi || orbit.eccentricity() >= 1.0 => {
                    let nu = orbit.true_anomaly_at_radius(soi);
                    start_ut + orbit.time_to_true_anomaly(nu, start_ut)
                }
                _ => start_ut + 10.0 * orbit.period.min(1.0e9),
            }
        }
        _ => period_end,
    };
    let mut best: Option<(f64, Entity, f64)> = None; // (soi entry ut, child, closest distance)
    for &child in index.by_id.values() {
        let Some(parent) = world.get::<ReferenceBody>(child) else {
            continue;
        };
        if parent.0 != body {
            continue;
        }
        let cb = world.get::<Body>(child).expect("body");
        let Some(soi) = cb.soi_radius_m else { continue };
        let Some(child_orbit) = world.get::<OnRails>(child).map(|r| r.0) else {
            continue;
        };
        // quick reject on radius ranges (original: PeApIntersects with SOI * 1.1)
        let thresh = soi * 1.1;
        let intersects = if orbit.eccentricity() >= 1.0 {
            orbit.periapsis() < child_orbit.apoapsis() + thresh
        } else {
            orbit.periapsis().max(child_orbit.periapsis())
                - orbit.apoapsis().min(child_orbit.apoapsis())
                <= thresh
        };
        if !intersects {
            continue;
        }
        // sample the window for local minima of the distance
        let span = (window_end - start_ut).min(1.0e9);
        if span.is_nan() || span <= 0.0 {
            continue;
        }
        let n = p.samples;
        let step = span / n as f64;
        let mut prev2 = f64::MAX;
        let mut prev = dist2(&orbit, world, child, start_ut);
        for i in 1..=n {
            let t = start_ut + step * i as f64;
            let d = dist2(&orbit, world, child, t);
            if prev < prev2 && prev <= d {
                // local minimum around t - step
                let (ca_ut, ca_dist) = refine_closest(
                    &orbit,
                    world,
                    child,
                    t - step,
                    step,
                    start_ut,
                    window_end,
                    p,
                );
                if ca_dist < soi {
                    let entry = refine_soi(
                        &orbit,
                        world,
                        child,
                        soi,
                        ca_ut,
                        (ca_ut - start_ut) * 0.5,
                        start_ut,
                        ca_ut,
                        p,
                    );
                    if entry > start_ut + p.time_epsilon_s && best.is_none_or(|(u, _, _)| entry < u)
                    {
                        best = Some((entry, child, ca_dist));
                    }
                    break;
                }
            }
            prev2 = prev;
            prev = d;
        }
    }
    if let Some((entry_ut, child, _)) = best {
        // inbound check: relative radial velocity must be negative
        let (vp, vv) = orbit.state_at(entry_ut);
        let (cp, cv) = body_rel(world, child, entry_ut);
        if (vp - cp).dot(vv - cv) < 0.0 {
            patch.end_ut = entry_ut;
            patch.end = Transition::Encounter;
            let next = handoff(world, body, &orbit, child, entry_ut);
            return (patch, Some((child, next)));
        }
    }

    // 3. Escape to the parent.
    match (b.soi_radius_m, world.get::<ReferenceBody>(body)) {
        (Some(soi), Some(parent)) if orbit.apoapsis() > soi || orbit.eccentricity() >= 1.0 => {
            let nu = orbit.true_anomaly_at_radius(soi);
            let exit_ut = start_ut + orbit.time_to_true_anomaly(nu, start_ut);
            patch.end_ut = exit_ut;
            patch.end = Transition::Escape;
            let next = handoff(world, body, &orbit, parent.0, exit_ut);
            return (patch, Some((parent.0, next)));
        }
        _ => {}
    }

    // 4. Final: one period (elliptic) or forever (hyperbolic around the root).
    (patch, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies::spawn_bodies;
    use ose_data::{CelestialBody as BodyDef, Orbit as OrbitDef, BIG_G, G0};
    use std::f64::consts::PI;

    fn body(
        id: &str,
        parent: Option<&str>,
        radius: f64,
        g: f64,
        a: f64,
        soi: Option<f64>,
        m0: f64,
    ) -> BodyDef {
        let mu = g * G0 * radius * radius;
        BodyDef {
            id: id.into(),
            display_name: id.into(),
            parent: parent.map(|s| s.into()),
            is_star: parent.is_none(),
            is_home_world: id == "Kerbin",
            radius_m: radius,
            gravity_asl: g,
            mu,
            mass_kg: mu / BIG_G,
            soi_radius_m: soi,
            rotation_period_s: 21_549.425,
            initial_rotation_deg: 0.0,
            is_tidally_locked: false,
            has_solid_surface: true,
            ocean: None,
            orbit: parent.map(|_| OrbitDef {
                semi_major_axis_m: a,
                eccentricity: 0.0,
                inclination_deg: 0.0,
                longitude_of_ascending_node_deg: 0.0,
                argument_of_periapsis_deg: 0.0,
                mean_anomaly_at_epoch_rad: m0,
                epoch_s: 0.0,
            }),
            atmosphere: None,
            star_luminosity: None,
        }
    }

    const KERBIN_MU: f64 = 1.00034160493135 * G0 * 600_000.0 * 600_000.0;
    const MUN_A: f64 = 12_000_000.0;

    fn system(mun_m0: f64) -> (World, BodyIndex) {
        let defs = vec![
            body("Kerbol", None, 261_600_000.0, 1.7467, 0.0, None, 0.0),
            body(
                "Kerbin",
                Some("Kerbol"),
                600_000.0,
                1.00034160493135,
                13_599_840_256.0,
                Some(84_159_286.0),
                0.0,
            ),
            body(
                "Mun",
                Some("Kerbin"),
                200_000.0,
                0.166,
                MUN_A,
                Some(2_429_559.0),
                mun_m0,
            ),
        ];
        let mut world = World::new();
        let index = spawn_bodies(&mut world, &defs);
        (world, index)
    }

    #[test]
    fn hohmann_transfer_from_100km_encounters_the_mun() {
        // Hohmann geometry: burn at angle 0, Mun must lead by 180 - n_mun * T_transfer.
        let r0 = 700_000.0;
        let a_t = 0.5 * (r0 + MUN_A);
        let t_transfer = PI * (a_t.powi(3) / KERBIN_MU).sqrt();
        let n_mun = (KERBIN_MU / MUN_A.powi(3)).sqrt();
        let lead = PI - n_mun * t_transfer;
        let (world, index) = system(lead);
        let kerbin = index.by_id["Kerbin"];
        let mun = index.by_id["Mun"];

        let v_circ = (KERBIN_MU / r0).sqrt();
        let dv = (KERBIN_MU / r0).sqrt() * ((2.0 * MUN_A / (r0 + MUN_A)).sqrt() - 1.0);
        assert!((dv - 841.6).abs() < 5.0, "hohmann dv {dv}");
        let orbit = Orbit::from_state_vectors(
            KERBIN_MU,
            DVec3::new(r0, 0.0, 0.0),
            DVec3::new(0.0, v_circ + dv, 0.0),
            0.0,
        );

        let patches = predict(&world, &index, kerbin, orbit, 0.0, &SolverParams::default());
        assert!(patches.len() >= 2, "patches: {patches:?}");
        let first = patches[0];
        assert_eq!(first.body, kerbin);
        assert_eq!(first.end, Transition::Encounter, "{patches:?}");
        // SOI entry comes before the transfer apoapsis, within about an hour of it (SOI is 2.4 Mm wide).
        assert!(
            first.end_ut < t_transfer && first.end_ut > t_transfer - 7200.0,
            "entry {} vs transfer {}",
            first.end_ut,
            t_transfer
        );
        let second = patches[1];
        assert_eq!(second.body, mun);
        assert_eq!(second.start, Transition::Encounter);
        // the handoff lands exactly on the SOI sphere
        let r_entry = second.orbit.position_at(first.end_ut).length();
        assert!(
            (r_entry - 2_429_559.0).abs() < 2_000.0,
            "entry radius {r_entry}"
        );
        // A dead-on Hohmann transfer hits the Mun (the bot found periapsis -199 km), so the Mun patch
        // ends in a collision below the SOI entry.
        assert_eq!(second.end, Transition::Collision, "{patches:?}");
        let r_impact = second.orbit.position_at(second.end_ut).length();
        assert!(
            (r_impact - 200_000.0).abs() < 1.0,
            "impact radius {r_impact}"
        );
        // Parity with the bot: SOI entry 5.54 h after the burn (19950 s), within a minute.
        assert!(
            (first.end_ut - 19_950.0).abs() < 60.0,
            "entry {}",
            first.end_ut
        );
    }

    #[test]
    fn a_hop_below_the_highest_mountain_ends_where_it_meets_the_ground() {
        // Kerbin with mountains up to 6 km somewhere but flat sea-level ground under this hop: the path must
        // end at the ground, not run on through the planet because it started below the highest peak.
        let (mut world, index) = system(0.0);
        let kerbin = index.by_id["Kerbin"];
        let radius = world.get::<Body>(kerbin).unwrap().radius_m;
        world
            .entity_mut(kerbin)
            .insert(crate::flight::Terrain(std::sync::Arc::new(
                ose_data::terrain::TerrainMap {
                    width: 2,
                    height: 2,
                    scale_m: 0.0,
                    offset_m: 0.0,
                    has_ocean: false,
                    body_radius_m: radius,
                    data: vec![0; 4],
                    max_m: 6_000.0,
                    detail: None,
                    overlay: None,
                    flats: vec![],
                },
            )));
        let r0 = radius + 1_000.0;
        let up = DVec3::new(r0, 0.0, 0.0);
        let hop = Orbit::from_state_vectors(KERBIN_MU, up, DVec3::new(250.0, 100.0, 0.0), 0.0);
        let p = predict(&world, &index, kerbin, hop, 0.0, &SolverParams::default());
        assert_eq!(p[0].end, Transition::Collision, "{p:?}");
        let impact = p[0].orbit.position_at(p[0].end_ut).length();
        assert!(
            (impact - radius).abs() < 1.0,
            "impact {:.1} m above the ground",
            impact - radius
        );
        assert!(p[0].end_ut > 30.0 && p[0].end_ut < 200.0, "{}", p[0].end_ut);
    }

    #[test]
    fn low_orbit_is_final_and_suborbital_collides() {
        let (world, index) = system(0.0);
        let kerbin = index.by_id["Kerbin"];
        let r0 = 700_000.0;
        let v_circ = (KERBIN_MU / r0).sqrt();
        let circ = Orbit::from_state_vectors(
            KERBIN_MU,
            DVec3::new(r0, 0.0, 0.0),
            DVec3::new(0.0, v_circ, 0.0),
            0.0,
        );
        let p = predict(&world, &index, kerbin, circ, 0.0, &SolverParams::default());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].end, Transition::Final);
        assert!((p[0].end_ut - circ.period).abs() < 1e-6);

        let sub = Orbit::from_state_vectors(
            KERBIN_MU,
            DVec3::new(r0, 0.0, 0.0),
            DVec3::new(0.0, 0.6 * v_circ, 0.0),
            0.0,
        );
        let p = predict(&world, &index, kerbin, sub, 0.0, &SolverParams::default());
        assert_eq!(p[0].end, Transition::Collision);
        let r = sub.position_at(p[0].end_ut).length();
        assert!((r - 600_000.0).abs() < 1.0, "impact radius {r}");
    }

    #[test]
    fn hyperbolic_escape_hands_off_to_kerbol() {
        let (world, index) = system(0.0);
        let kerbin = index.by_id["Kerbin"];
        let kerbol = index.by_id["Kerbol"];
        let r0 = 700_000.0;
        let v_esc = (2.0 * KERBIN_MU / r0).sqrt();
        let hyp = Orbit::from_state_vectors(
            KERBIN_MU,
            DVec3::new(r0, 0.0, 0.0),
            DVec3::new(0.0, 1.3 * v_esc, 0.2 * v_esc),
            0.0,
        );
        let p = predict(&world, &index, kerbin, hyp, 0.0, &SolverParams::default());
        assert_eq!(p[0].end, Transition::Escape, "{p:?}");
        assert!((hyp.position_at(p[0].end_ut).length() - 84_159_286.0).abs() < 1.0);
        assert_eq!(p[1].body, kerbol);
        assert_eq!(p[1].end, Transition::Final);
    }
}

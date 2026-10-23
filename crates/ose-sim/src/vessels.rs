//! Vessels on rails and sphere-of-influence handoff.
//!
//! A vessel on rails is an entity with [`ReferenceBody`] + [`OnRails`] like a moon. Each tick the sim checks
//! which body's SOI contains the vessel (`main_body_at`); when it differs from the reference body the orbit is
//! re-expressed around the new body from the current star-space state vectors (the original's
//! `UpdateFromOrbitAtUT` / `UpdateFromStateVectors` on reference-body change).

use crate::bodies::{main_body_at, star_space_state, Body, BodyIndex, OnRails, ReferenceBody};
use bevy_ecs::prelude::*;
use ose_math::{DVec3, Orbit};

#[derive(Component, Clone, Debug)]
pub struct Vessel {
    pub name: String,
}

/// What happened to a vessel's reference body during an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoiTransition {
    None,
    Escape { from: Entity, to: Entity },
    Encounter { from: Entity, to: Entity },
}

/// Spawn a vessel on rails around `body` with the given relative position and velocity at `ut`.
pub fn spawn_vessel_on_rails(
    world: &mut World,
    name: &str,
    body: Entity,
    r: DVec3,
    v: DVec3,
    ut: f64,
) -> Entity {
    let mu = world.get::<Body>(body).expect("body").mu;
    world
        .spawn((
            Vessel { name: name.into() },
            ReferenceBody(body),
            OnRails(Orbit::from_state_vectors(mu, r, v, ut)),
        ))
        .id()
}

/// Re-express `vessel`'s orbit around whichever body's SOI it is in at `ut`. Returns the transition, if any.
pub fn update_reference_body(
    world: &mut World,
    index: &BodyIndex,
    vessel: Entity,
    ut: f64,
) -> SoiTransition {
    let Some(current) = world.get::<ReferenceBody>(vessel).map(|r| r.0) else {
        return SoiTransition::None;
    };
    let (pos, vel) = star_space_state(world, vessel, ut);
    let Some(target) = main_body_at(world, index, pos, ut) else {
        return SoiTransition::None;
    };
    if target == current {
        return SoiTransition::None;
    }
    let (bp, bv) = star_space_state(world, target, ut);
    let mu = world.get::<Body>(target).expect("body").mu;
    let orbit = Orbit::from_state_vectors(mu, pos - bp, vel - bv, ut);
    world
        .entity_mut(vessel)
        .insert((ReferenceBody(target), OnRails(orbit)));
    // Escape when the new body is an ancestor of the old one, encounter otherwise.
    let mut e = Some(current);
    while let Some(b) = e {
        let parent = world.get::<ReferenceBody>(b).map(|r| r.0);
        if parent == Some(target) {
            return SoiTransition::Escape {
                from: current,
                to: target,
            };
        }
        e = parent;
    }
    SoiTransition::Encounter {
        from: current,
        to: target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies::spawn_bodies;
    use ose_data::{CelestialBody as BodyDef, Orbit as OrbitDef, BIG_G, G0};

    fn body(
        id: &str,
        parent: Option<&str>,
        radius: f64,
        g: f64,
        a: f64,
        soi: Option<f64>,
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
                mean_anomaly_at_epoch_rad: 0.0,
                epoch_s: 0.0,
            }),
            atmosphere: None,
            star_luminosity: None,
        }
    }

    fn kerbol_system(world: &mut World) -> BodyIndex {
        let defs = vec![
            body("Kerbol", None, 261_600_000.0, 1.7467, 0.0, None),
            body(
                "Kerbin",
                Some("Kerbol"),
                600_000.0,
                1.0,
                13_599_840_256.0,
                Some(84_159_286.0),
            ),
            body(
                "Mun",
                Some("Kerbin"),
                200_000.0,
                0.166,
                12_000_000.0,
                Some(2_429_559.0),
            ),
        ];
        spawn_bodies(world, &defs)
    }

    #[test]
    fn escaping_kerbin_hands_the_vessel_to_kerbol_continuously() {
        let mut world = World::new();
        let index = kerbol_system(&mut world);
        let kerbin = index.by_id["Kerbin"];
        let kerbol = index.by_id["Kerbol"];
        let mu = world.get::<Body>(kerbin).unwrap().mu;
        // Hyperbolic escape well above escape speed, mostly outward with a tangential component
        // (a purely radial trajectory has no angular momentum and is not a conic).
        let r = DVec3::new(0.0, 0.0, 700_000.0);
        let v_esc = (2.0 * mu / r.length()).sqrt();
        let v = DVec3::new(0.3 * v_esc, 0.0, 1.5 * v_esc);
        let vessel = spawn_vessel_on_rails(&mut world, "probe", kerbin, r, v, 0.0);
        let mut transition = SoiTransition::None;
        let mut ut = 0.0;
        let mut last_pos = star_space_state(&world, vessel, ut).0;
        while ut < 200_000.0 {
            ut += 10.0;
            let before = star_space_state(&world, vessel, ut).0;
            let t = update_reference_body(&mut world, &index, vessel, ut);
            let after = star_space_state(&world, vessel, ut).0;
            // the handoff must not move the vessel
            assert!(
                (after - before).length() < 1.0,
                "jump at handoff: {}",
                (after - before).length()
            );
            assert!(
                (after - last_pos).length() < 1.0e6,
                "unreasonable step at ut {ut}"
            );
            last_pos = after;
            if t != SoiTransition::None {
                transition = t;
                break;
            }
        }
        assert_eq!(
            transition,
            SoiTransition::Escape {
                from: kerbin,
                to: kerbol
            }
        );
        let rb = world.get::<ReferenceBody>(vessel).unwrap().0;
        assert_eq!(rb, kerbol);
        let dist_from_kerbin = (star_space_state(&world, vessel, ut).0
            - star_space_state(&world, kerbin, ut).0)
            .length();
        assert!(
            (dist_from_kerbin - 84_159_286.0).abs() < 1.0e6,
            "{dist_from_kerbin}"
        );
    }

    #[test]
    fn entering_the_muns_soi_is_an_encounter() {
        let mut world = World::new();
        let index = kerbol_system(&mut world);
        let kerbin = index.by_id["Kerbin"];
        let mun = index.by_id["Mun"];
        let ut = 1000.0;
        // Put the vessel 1000 km from the Mun, inside its SOI, but parented to Kerbin.
        let mun_rel = star_space_state(&world, mun, ut).0 - star_space_state(&world, kerbin, ut).0;
        let mun_vel = star_space_state(&world, mun, ut).1 - star_space_state(&world, kerbin, ut).1;
        let r = mun_rel + DVec3::new(0.0, 0.0, 1_000_000.0);
        let v = mun_vel + DVec3::new(300.0, 0.0, 0.0);
        let vessel = spawn_vessel_on_rails(&mut world, "lander", kerbin, r, v, ut);
        let t = update_reference_body(&mut world, &index, vessel, ut);
        assert_eq!(
            t,
            SoiTransition::Encounter {
                from: kerbin,
                to: mun
            }
        );
        let o = world.get::<OnRails>(vessel).unwrap().0;
        let (p, _) = o.state_at(ut);
        assert!((p.length() - 1_000_000.0).abs() < 1.0, "{}", p.length());
        let mu_mun = world.get::<Body>(mun).unwrap().mu;
        assert_eq!(o.mu, mu_mun);
    }
}

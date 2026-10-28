//! Celestial bodies as ECS entities and their positions in star space.
//!
//! Bodies form a tree rooted at the star. Each non-root body has an [`Orbit`] around its parent; a position in
//! "star space" (the inertial Z-up frame centred on the root) is the sum of relative positions up the chain,
//! exactly like the original's `GetPositionAtUTInStarSpace` for untilted bodies.

use bevy_ecs::prelude::*;
use ose_data::CelestialBody as BodyDef;
use ose_math::{DVec3, Elements, Orbit};
use std::collections::HashMap;

/// Marker plus static properties of a celestial body.
#[derive(Component, Clone, Debug)]
pub struct Body {
    pub id: String,
    pub mu: f64,
    pub mass_kg: f64,
    pub radius_m: f64,
    /// `None` means infinite (the root).
    pub soi_radius_m: Option<f64>,
    pub rotation_period_s: f64,
    pub initial_rotation_deg: f64,
    pub is_star: bool,
    /// Ocean density is authored in tonnes per cubic metre.
    pub ocean: Option<ose_data::Ocean>,
    /// A built site on the surface (the KSC campus) that craft stand on instead of the terrain.
    pub site: Option<std::sync::Arc<crate::campus::Site>>,
}

/// Which body an orbit is around.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceBody(pub Entity);

/// An on-rails Keplerian orbit around [`ReferenceBody`].
#[derive(Component, Clone, Copy, Debug)]
pub struct OnRails(pub Orbit);

/// Index of bodies by id, kept as a resource for lookups.
#[derive(Resource, Default, Debug, Clone)]
pub struct BodyIndex {
    pub by_id: HashMap<String, Entity>,
    pub root: Option<Entity>,
}

/// Spawns every body in `defs` into `world`. Parents must exist in `defs`; order does not matter.
pub fn spawn_bodies(world: &mut World, defs: &[BodyDef]) -> BodyIndex {
    let mut index = BodyIndex::default();
    for d in defs {
        let e = world
            .spawn(Body {
                id: d.id.clone(),
                mu: d.mu,
                mass_kg: d.mass_kg,
                radius_m: d.radius_m,
                soi_radius_m: d.soi_radius_m,
                rotation_period_s: d.rotation_period_s,
                initial_rotation_deg: d.initial_rotation_deg,
                is_star: d.is_star,
                ocean: d.ocean.clone(),
                site: None,
            })
            .id();
        if let Some(a) = &d.atmosphere {
            world
                .entity_mut(e)
                .insert(crate::flight::Atmosphere(crate::flight::Atmo {
                    depth_m: a.depth_m,
                    oxygen: a.contains_oxygen,
                    pressure_kpa: a.pressure_curve.clone(),
                    temperature_k: a.temperature_curve.clone(),
                    molar_mass: a.molar_mass_kg_per_mol,
                }));
        }
        index.by_id.insert(d.id.clone(), e);
    }
    for d in defs {
        let e = index.by_id[&d.id];
        match (&d.parent, &d.orbit) {
            (Some(p), Some(o)) => {
                let parent = index.by_id[p];
                let parent_mu = defs
                    .iter()
                    .find(|b| &b.id == p)
                    .map(|b| b.mu)
                    .expect("parent def");
                let orbit = Orbit::from_elements(
                    parent_mu,
                    Elements {
                        semi_major_axis: o.semi_major_axis_m,
                        eccentricity: o.eccentricity,
                        inclination_deg: o.inclination_deg,
                        longitude_of_ascending_node_deg: o.longitude_of_ascending_node_deg,
                        argument_of_periapsis_deg: o.argument_of_periapsis_deg,
                        mean_anomaly_at_epoch: o.mean_anomaly_at_epoch_rad,
                        epoch: o.epoch_s,
                    },
                );
                world
                    .entity_mut(e)
                    .insert((ReferenceBody(parent), OnRails(orbit)));
            }
            _ => {
                if d.parent.is_none() {
                    index.root = Some(e);
                }
            }
        }
    }
    world.insert_resource(index.clone_shallow());
    index
}

impl BodyIndex {
    fn clone_shallow(&self) -> BodyIndex {
        BodyIndex {
            by_id: self.by_id.clone(),
            root: self.root,
        }
    }
}

/// Position and velocity of `entity` in star space at `ut`, walking the parent chain.
pub fn star_space_state(world: &World, entity: Entity, ut: f64) -> (DVec3, DVec3) {
    let mut pos = DVec3::ZERO;
    let mut vel = DVec3::ZERO;
    let mut cur = Some(entity);
    while let Some(e) = cur {
        match (world.get::<OnRails>(e), world.get::<ReferenceBody>(e)) {
            (Some(rails), Some(parent)) => {
                let (p, v) = rails.0.state_at(ut);
                pos = pos + p;
                vel = vel + v;
                cur = Some(parent.0);
            }
            _ => cur = None,
        }
    }
    (pos, vel)
}

/// Position of `entity` relative to `relative_to` at `ut`.
pub fn relative_position(world: &World, entity: Entity, relative_to: Entity, ut: f64) -> DVec3 {
    star_space_state(world, entity, ut).0 - star_space_state(world, relative_to, ut).0
}

/// The body whose sphere of influence contains the star-space point `p` at `ut`: the deepest body in the tree
/// whose SOI contains the point (the root always does).
pub fn main_body_at(world: &World, index: &BodyIndex, p: DVec3, ut: f64) -> Option<Entity> {
    let mut best = index.root?;
    loop {
        let mut descended = false;
        for (&_, &child) in &index.by_id {
            let Some(parent) = world.get::<ReferenceBody>(child) else {
                continue;
            };
            if parent.0 != best {
                continue;
            }
            let body = world.get::<Body>(child)?;
            let Some(soi) = body.soi_radius_m else {
                continue;
            };
            let cp = star_space_state(world, child, ut).0;
            if (p - cp).length() < soi {
                best = child;
                descended = true;
                break;
            }
        }
        if !descended {
            return Some(best);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_data::{Orbit as OrbitDef, BIG_G, G0};

    fn body(id: &str, parent: Option<&str>, radius: f64, g: f64, a: f64) -> BodyDef {
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
            soi_radius_m: None,
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

    #[test]
    fn mun_position_is_sum_of_kerbin_and_mun_orbits() {
        let mut defs = vec![
            body("Kerbol", None, 261_600_000.0, 1.7467, 0.0),
            body("Kerbin", Some("Kerbol"), 600_000.0, 1.0, 13_599_840_256.0),
            body("Mun", Some("Kerbin"), 200_000.0, 0.166, 12_000_000.0),
        ];
        // give Kerbin an SOI so main_body_at can descend
        defs[1].soi_radius_m = Some(84_159_286.0);
        defs[2].soi_radius_m = Some(2_429_559.0);
        let mut world = World::new();
        let index = spawn_bodies(&mut world, &defs);
        let kerbin = index.by_id["Kerbin"];
        let mun = index.by_id["Mun"];
        let ut = 12_345.0;
        let d = relative_position(&world, mun, kerbin, ut).length();
        assert!((d - 12_000_000.0).abs() < 1e-3, "{d}");
        let k = star_space_state(&world, kerbin, ut).0.length();
        assert!((k - 13_599_840_256.0).abs() < 1.0, "{k}");

        let near_mun = star_space_state(&world, mun, ut).0 + DVec3::new(300_000.0, 0.0, 0.0);
        assert_eq!(main_body_at(&world, &index, near_mun, ut), Some(mun));
        let near_kerbin = star_space_state(&world, kerbin, ut).0 + DVec3::new(0.0, 700_000.0, 0.0);
        assert_eq!(main_body_at(&world, &index, near_kerbin, ut), Some(kerbin));
        assert_eq!(
            main_body_at(&world, &index, DVec3::ZERO, ut),
            Some(index.by_id["Kerbol"])
        );
    }
}

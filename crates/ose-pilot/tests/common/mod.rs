//! Shared setup for the pilot scenarios.
#![allow(dead_code)]
use bevy_ecs::entity::Entity;
use ose_data::{craft::Craft, CelestialBody, PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use ose_pilot::{
    mission::{Catalog, Mission},
    Status,
};
use ose_sim::{
    flight::{FlightSpec, StageInertia},
    Universe,
};
use serde::Deserialize;

pub struct World {
    pub u: Universe,
    pub defs: Vec<PartDef>,
    pub resources: Vec<ResourceDef>,
    pub kerbin: Entity,
}

pub fn world() -> World {
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../../assets/own/parts-catalog.json")).unwrap();
    let resources: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../../assets/own/resources.json")).unwrap();
    let bodies: Vec<CelestialBody> =
        serde_json::from_str(include_str!("../../../../assets/own/kerbol-system.json")).unwrap();
    let mut u = Universe::with_bodies(&bodies);
    u.set_part_catalog(defs.clone(), resources.clone());
    let kerbin = u.body_entity("Kerbin").unwrap();
    World {
        u,
        defs,
        resources,
        kerbin,
    }
}

/// The flight spec the ISS assembly test pilot builds for a craft (staging, inertia, sea-level Isp).
pub fn spec(c: &Craft, defs: &[PartDef], resources: &[ResourceDef]) -> FlightSpec {
    let report = ose_data::staging::delta_v_by_stage(c, defs, resources);
    let def = |id: &str| defs.iter().find(|d| d.id == id).unwrap();
    let mass = |p: &ose_data::craft::CraftPart| {
        def(&p.part_id).mass_t
            + p.resources
                .iter()
                .map(|r| {
                    resources
                        .iter()
                        .find(|d| d.id.eq_ignore_ascii_case(&r.resource))
                        .map(|d| match d.kind {
                            ResourceKind::Basic {
                                mass_per_unit_t, ..
                            } => mass_per_unit_t * r.stored_units,
                            _ => 0.0,
                        })
                        .unwrap_or(0.0)
                })
                .sum::<f64>()
    };
    let inertia = report
        .stages
        .iter()
        .map(|st| {
            let total = st
                .alive
                .iter()
                .map(|i| mass(&c.parts[*i]))
                .sum::<f64>()
                .max(0.001);
            let centre = st.alive.iter().fold(DVec3::ZERO, |v, i| {
                v + c.parts[*i].local_position * mass(&c.parts[*i])
            }) * total.recip();
            let (mut ix, mut iz, mut wheel, mut gimbal) = (0.0_f64, 0.0_f64, 0.0, 0.0);
            for i in &st.alive {
                let p = &c.parts[*i];
                let d = def(&p.part_id);
                let m = mass(p);
                let r = p.local_position - centre;
                let size = d
                    .bounds
                    .map(|b| b.size_m)
                    .unwrap_or(DVec3::new(1.0, 1.0, 1.0));
                ix += m * (r.y * r.y + r.z * r.z + (size.y * size.y + size.z * size.z) / 12.0);
                iz += m * (r.x * r.x + r.y * r.y + (size.x * size.x + size.y * size.y) / 12.0);
                wheel += d.reaction_wheel_torque_knm;
            }
            for i in &st.engines {
                let p = &c.parts[*i];
                if let Some(e) = def(&p.part_id).engines.first() {
                    gimbal += e.max_thrust_kn
                        * e.gimbal_range_deg.unwrap_or(0.0).to_radians().sin()
                        * (p.local_position.y - centre.y).abs();
                }
            }
            StageInertia {
                moi_tm2: ix.max(iz),
                wheel_torque_knm: wheel,
                gimbal_torque_knm: gimbal,
            }
        })
        .collect::<Vec<_>>();
    let asl = report
        .stages
        .iter()
        .map(|st| {
            let (mut n, mut d) = (0.0, 0.0);
            for i in &st.engines {
                for e in &def(&c.parts[*i].part_id).engines {
                    n += e.max_thrust_kn * e.isp_curve.evaluate(1.0);
                    d += e.max_thrust_kn;
                }
            }
            if d > 0.0 {
                n / d
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    let area = c
        .parts
        .iter()
        .filter_map(|p| {
            def(&p.part_id)
                .bounds
                .map(|b| b.size_m.x * b.size_m.z * std::f64::consts::FRAC_PI_4)
        })
        .fold(0.1, f64::max);
    FlightSpec::from_report(&report, &asl, &inertia, area)
}

/// Test setup only: place a craft in a circular equatorial orbit at `altitude_m`, `phase_deg` around
/// from +x, facing prograde. The pilot never places vessels; it flies them.
pub fn in_orbit(w: &mut World, craft: &Craft, altitude_m: f64, phase_deg: f64) -> Entity {
    let kerbin = w.kerbin;
    in_orbit_around(w, kerbin, craft, altitude_m, phase_deg)
}

/// [`in_orbit`] around any body, e.g. the Mun.
pub fn in_orbit_around(
    w: &mut World,
    around: Entity,
    craft: &Craft,
    altitude_m: f64,
    phase_deg: f64,
) -> Entity {
    in_orbit_inclined(w, around, craft, altitude_m, phase_deg, 0.0)
}

/// [`in_orbit_around`], tilted `inclination_deg` about +x (the ascending node is at +x).
pub fn in_orbit_inclined(
    w: &mut World,
    around: Entity,
    craft: &Craft,
    altitude_m: f64,
    phase_deg: f64,
    inclination_deg: f64,
) -> Entity {
    let (si, ci) = inclination_deg.to_radians().sin_cos();
    let tilt = |v: DVec3| DVec3::new(v.x, ci * v.y - si * v.z, si * v.y + ci * v.z);
    let body =
        w.u.world
            .get::<ose_sim::bodies::Body>(around)
            .unwrap()
            .clone();
    let radius = body.radius_m + altitude_m;
    let (s, c) = phase_deg.to_radians().sin_cos();
    let r = tilt(DVec3::new(c, s, 0.0) * radius);
    let speed = (body.mu / radius).sqrt();
    let v = tilt(DVec3::new(-s, c, 0.0) * speed);
    let e = w.u.spawn_vessel(&craft.name, around, r, v);
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    w.u.rebuild_vessel(e, craft, r, v, v.normalized(), vec![], &defs, &res, &|c| {
        spec(c, &defs, &res)
    });
    e
}

#[derive(Deserialize)]
pub struct Modules {
    pub zarya: Craft,
    pub unity: Craft,
    pub zvezda: Craft,
}

pub fn modules() -> Modules {
    serde_json::from_str(include_str!("../data/orbital-modules.json")).unwrap()
}

/// A copy of a craft whose part identities don't collide with the original's.
pub fn renamed(c: &Craft, suffix: &str) -> Craft {
    let mut c = c.clone();
    c.name = format!("{}{suffix}", c.name);
    for p in &mut c.parts {
        p.guid = format!("{}{suffix}", p.guid);
    }
    c
}

/// Fly the missions together until each one ends, or the time runs out.
pub fn fly(w: &mut World, missions: &mut [Mission], limit_s: f64) -> Vec<Status> {
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    let spec_for = |c: &Craft| spec(c, &defs, &res);
    let catalog = Catalog {
        defs: &defs,
        resources: &res,
        spec: &spec_for,
    };
    let start = w.u.ut();
    let mut results = vec![Status::Running; missions.len()];
    while results.contains(&Status::Running) {
        for (m, result) in missions.iter_mut().zip(results.iter_mut()) {
            if *result == Status::Running {
                *result = m.update(&mut w.u, &catalog);
            }
        }
        assert!(
            w.u.ut() - start < limit_s,
            "missions still running after {limit_s} s: {:?}",
            missions
                .iter()
                .map(|m| (m.leg(), m.log.clone()))
                .collect::<Vec<_>>()
        );
        let warp = missions
            .iter()
            .zip(&results)
            .filter(|(_, r)| **r == Status::Running)
            .map(|(m, _)| m.suggested_warp(&w.u))
            .min()
            .unwrap_or(1);
        w.u.set_warp(warp);
        w.u.step();
        w.u.dock_latched(&defs, &res, &spec_for);
    }
    results
}

pub fn links(w: &World, e: Entity) -> usize {
    w.u.docked_links(e).len()
}

/// Test setup only: stand a craft on the KSC pad.
pub fn on_pad(w: &mut World, craft: &Craft) -> Entity {
    let kerbin = w.kerbin;
    on_surface(w, kerbin, craft, -0.6836, -74.8198)
}

/// Test setup only: stand a craft on the ground of any body.
pub fn on_surface(
    w: &mut World,
    body: Entity,
    craft: &Craft,
    lat_deg: f64,
    lon_deg: f64,
) -> Entity {
    let s = spec(craft, &w.defs, &w.resources);
    let e =
        w.u.spawn_powered_on_surface(&craft.name, body, s, lat_deg, lon_deg);
    w.u.set_craft(e, craft.clone());
    w.u.set_structure(
        e,
        ose_sim::structure::Structure::build(craft, &w.defs, &w.resources),
    );
    e
}

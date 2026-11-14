//! Shared setup for the pilot scenarios.
#![allow(dead_code)]
use bevy_ecs::entity::Entity;
use ose_data::{craft::Craft, CelestialBody, PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use ose_sim::{
    flight::{FlightSpec, StageInertia},
    Universe,
};

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
    let body =
        w.u.world
            .get::<ose_sim::bodies::Body>(w.kerbin)
            .unwrap()
            .clone();
    let radius = body.radius_m + altitude_m;
    let (s, c) = phase_deg.to_radians().sin_cos();
    let r = DVec3::new(c, s, 0.0) * radius;
    let speed = (body.mu / radius).sqrt();
    let v = DVec3::new(-s, c, 0.0) * speed;
    let e = w.u.spawn_vessel(&craft.name, w.kerbin, r, v);
    let (defs, res) = (w.defs.clone(), w.resources.clone());
    w.u.rebuild_vessel(e, craft, r, v, v.normalized(), vec![], &defs, &res, &|c| {
        spec(c, &defs, &res)
    });
    e
}

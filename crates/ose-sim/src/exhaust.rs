//! Engine exhaust blasting parts in its path, as the original does it (`Module_Engine.EngineExhaustDamage`):
//! for each nozzle a sphere of the engine's size diameter x `exhaustDamageRadiusMultiplier` / 2 is swept
//! along the exhaust from `exhaustDamageDistanceOffset` (plus its radius) past the nozzle; every other part
//! it meets within `exhaustDamageMaxRange` (less the sphere's diameter) takes `ExhaustDamageValue` x the
//! throttle x min(1 / distance^`exhaustDamageFalloffPower`, `exhaustDamageMaxMutliplier`) of heat
//! (`ThermalData.ExhaustFlux`, kW), and the engine's thrust x the same factor (shared among the parts hit) as
//! a push along the exhaust. No stock engine heats itself (`GenerateHeat` is off and every
//! `HeatAtmosphereCurve` is zero), so the exhaust is what makes engines dangerous to their own craft.
//!
//! The original adds a part's flux once for every hit the sweep reports after it, in PhysX's order; here each
//! part in the path takes it once per nozzle.
use bevy_ecs::prelude::*;
use ose_math::DVec3;

use crate::{bodies, flight, structure::Structure, Universe};

/// One nozzle's exhaust sweep as of the last step (vessel frame): the sphere's start and direction, its
/// radius and reach, and the blast at full strength (heat already at this step's throttle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jet {
    pub part: usize,
    pub exit: DVec3,
    pub origin: DVec3,
    pub dir: DVec3,
    pub radius: f64,
    pub range: f64,
    pub thrust_kn: f64,
    pub damage_kw: f64,
    pub falloff_power: f64,
    pub max_multiplier: f64,
}

/// The parts of a craft a jet meets (`Some` distances within its reach), with the jet given in that craft's
/// vessel frame.
fn hits(
    st: &Structure,
    origin: DVec3,
    dir: DVec3,
    jet: &Jet,
    skip: Option<usize>,
) -> Vec<(usize, f64)> {
    (0..st.rigid.len())
        .filter(|&j| st.rigid.present[j] && Some(j) != skip)
        .filter_map(|j| {
            let (c, h) = st.part_box(j);
            sweep_box(origin, dir, jet.radius, c, h)
                .filter(|&d| d <= jet.range)
                .map(|d| (j, d))
        })
        .collect()
}

impl Universe {
    /// Engine exhaust reaching other craft (`Module_Engine.EngineExhaustDamage` meets every part in the
    /// sweep, whatever craft it is on): the parts it meets heat up (`Structure::outside_exhaust_flux_kw`, taken
    /// up by their next heat step) and their craft is pushed along the exhaust for the `dt` seconds this
    /// covers. Landed craft hold their ground.
    pub(crate) fn exhaust_on_other_vessels(&mut self, dt: f64) {
        let mut q = self.world.query::<&mut Structure>();
        for mut st in q.iter_mut(&mut self.world) {
            if st.outside_exhaust_flux_kw.iter().any(|&f| f != 0.0) {
                st.outside_exhaust_flux_kw.fill(0.0);
            }
        }
        type Craft = (Entity, Entity, DVec3, DVec3, DVec3, f64, bool);
        let crafts: Vec<(Craft, Vec<Jet>)> = {
            let mut q = self
                .world
                .query::<(Entity, &flight::Powered, &bodies::ReferenceBody, &Structure)>();
            q.iter(&self.world)
                .filter(|(_, p, _, _)| !p.crashed)
                .map(|(e, p, b, st)| {
                    let c = st.rigid.rest_centre();
                    let reach = (0..st.rigid.len())
                        .filter(|&i| st.rigid.present[i])
                        .map(|i| {
                            let (centre, half) = st.part_box(i);
                            (centre - c).length() + half.length()
                        })
                        .fold(0.0, f64::max);
                    // jets in inertial terms, from the vessel's position
                    let jets = st
                        .jets
                        .iter()
                        .map(|j| Jet {
                            origin: st.offset_inertial(p.facing, j.origin - c),
                            exit: st.offset_inertial(p.facing, j.exit - c),
                            dir: st.offset_inertial(p.facing, j.dir),
                            ..*j
                        })
                        .collect();
                    ((e, b.0, p.r, p.v, p.facing, reach, p.landed), jets)
                })
                .collect()
        };
        if crafts.len() < 2 || crafts.iter().all(|(_, jets)| jets.is_empty()) {
            return;
        }
        // (craft, part, heat kW, push kN inertial, where relative to the craft's position)
        let mut blasts: Vec<(Entity, usize, f64, DVec3, DVec3)> = Vec::new();
        for ((a, body_a, ra, _, _, _, _), jets) in &crafts {
            for jet in jets {
                for ((b, body_b, rb, _, facing_b, reach_b, _), _) in &crafts {
                    if a == b || body_a != body_b {
                        continue;
                    }
                    let origin = *ra + jet.origin - *rb;
                    if origin.length() > reach_b + jet.range + jet.radius {
                        continue;
                    }
                    let Some(st) = self.world.get::<Structure>(*b) else {
                        continue;
                    };
                    let c = st.rigid.rest_centre();
                    let o = st.inertial_to_vessel(*facing_b, origin) + c;
                    let d = st.inertial_to_vessel(*facing_b, jet.dir);
                    let found = hits(st, o, d, jet, None);
                    let shared = 1.0 / found.len().max(1) as f64;
                    for (j, dist) in found {
                        let scale = damage_scale(dist, jet.falloff_power, jet.max_multiplier);
                        let at = if dist <= 0.0 {
                            *ra + jet.exit - *rb
                        } else {
                            origin + jet.dir * dist
                        };
                        blasts.push((
                            *b,
                            j,
                            jet.damage_kw * scale,
                            jet.dir * (jet.thrust_kn * scale * shared),
                            at,
                        ));
                    }
                }
            }
        }
        for (e, part, heat, push, at) in blasts {
            if let Some(mut st) = self.world.get_mut::<Structure>(e) {
                st.outside_exhaust_flux_kw[part] += heat;
            }
            let inertia = self.blow_inertia(e);
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                if p.landed || p.mass_t <= 0.0 {
                    continue;
                }
                p.v = p.v + push * (dt / p.mass_t);
                if let Some(i) = inertia.filter(|&i| i > 0.0) {
                    let dw = at.cross(push) * (dt / i);
                    let axial = dw.dot(p.facing);
                    p.roll_rate += axial;
                    p.angvel = p.angvel + (dw - p.facing * axial);
                }
            }
        }
    }
}

/// `PartComponent.AssemblySizeTypeDiameters` by `sizeCategory` (`MetaAssemblySizeFilterType`: Auto, XS, S, ...).
pub fn size_diameter_m(size_category: &str) -> f64 {
    const DIAMETERS: [f64; 11] = [
        0.3125, 0.625, 1.25, 2.5, 3.75, 5.0, 10.0, 20.0, 40.0, 60.0, 80.0,
    ];
    let index = match size_category.to_ascii_uppercase().as_str() {
        "XS" => 1,
        "S" => 2,
        "M" => 3,
        "L" => 4,
        "XL" => 5,
        "XXL" => 6,
        "XXXL" => 7,
        "XXXXL" => 8,
        "XXXXXL" => 9,
        "XXXXXXL" => 10,
        _ => 0,
    };
    DIAMETERS[index]
}

/// How far along `dir` (unit) from `origin` a sphere of `radius` first touches a box (`centre`, `half`
/// extents, axis-aligned in the same frame): `Some(0)` when it starts touching, `None` when it passes clear
/// or the box is behind. The box is grown by the radius, which is exact on the faces and slightly generous at
/// the edges and corners.
pub fn sweep_box(
    origin: DVec3,
    dir: DVec3,
    radius: f64,
    centre: DVec3,
    half: DVec3,
) -> Option<f64> {
    let lo = centre - half - DVec3::new(radius, radius, radius);
    let hi = centre + half + DVec3::new(radius, radius, radius);
    let (mut t0, mut t1) = (0.0_f64, f64::INFINITY);
    for (o, d, l, h) in [
        (origin.x, dir.x, lo.x, hi.x),
        (origin.y, dir.y, lo.y, hi.y),
        (origin.z, dir.z, lo.z, hi.z),
    ] {
        if d.abs() < 1e-12 {
            if o < l || o > h {
                return None;
            }
        } else {
            let (a, b) = ((l - o) / d, (h - o) / d);
            let (a, b) = if a < b { (a, b) } else { (b, a) };
            t0 = t0.max(a);
            t1 = t1.min(b);
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// The blast's strength at a hit `distance` m from the sweep's start: a hit at the start counts as 1 m.
pub fn damage_scale(distance: f64, falloff_power: f64, max_multiplier: f64) -> f64 {
    let x = if distance <= 0.0 { 1.0 } else { distance };
    (1.0 / x.powf(falloff_power)).min(max_multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_categories_give_the_originals_diameters() {
        assert_eq!(size_diameter_m("S"), 1.25);
        assert_eq!(size_diameter_m("XS"), 0.625);
        assert_eq!(size_diameter_m("M"), 2.5);
        assert_eq!(size_diameter_m(""), 0.3125);
    }

    #[test]
    fn a_sphere_swept_down_meets_the_box_below_at_its_top_less_the_radius() {
        let down = DVec3::new(0.0, -1.0, 0.0);
        let hit = sweep_box(
            DVec3::ZERO,
            down,
            0.25,
            DVec3::new(0.0, -3.0, 0.0),
            DVec3::new(0.6, 0.5, 0.6),
        );
        assert!((hit.unwrap() - 2.25).abs() < 1e-12);
        // off to the side it passes clear; above it is behind
        assert!(sweep_box(
            DVec3::new(2.0, 0.0, 0.0),
            down,
            0.25,
            DVec3::new(0.0, -3.0, 0.0),
            DVec3::new(0.5, 0.5, 0.5)
        )
        .is_none());
        assert!(sweep_box(
            DVec3::ZERO,
            down,
            0.25,
            DVec3::new(0.0, 3.0, 0.0),
            DVec3::new(0.5, 0.5, 0.5)
        )
        .is_none());
        // starting inside
        assert_eq!(
            sweep_box(
                DVec3::ZERO,
                down,
                0.25,
                DVec3::ZERO,
                DVec3::new(0.5, 0.5, 0.5)
            ),
            Some(0.0)
        );
    }

    #[test]
    fn the_blast_falls_off_with_distance_squared_and_is_capped_close_in() {
        assert_eq!(damage_scale(0.0, 2.0, 1.0), 1.0);
        assert_eq!(damage_scale(0.5, 2.0, 1.0), 1.0);
        assert!((damage_scale(5.0, 2.0, 1.0) - 0.04).abs() < 1e-12);
    }
}

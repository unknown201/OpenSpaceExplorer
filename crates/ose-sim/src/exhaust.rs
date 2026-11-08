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
use ose_math::DVec3;

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

//! Surface contact primitives for the flight proxy. These do not replace per-part collision geometry.
use ose_math::DVec3;

/// Provisional dry-ground friction; material-specific KSP2 coefficients are not imported yet.
pub const STATIC_FRICTION: f64 = 0.8;
pub const SLIDING_FRICTION: f64 = 0.6;

/// Inelastic normal response with Coulomb friction, in the surface's moving frame.
/// Returns velocity and incoming normal speed; the caller applies its collision-damage rule separately.
pub fn resolve_velocity(velocity: DVec3, normal: DVec3) -> (DVec3, f64) {
    let normal_speed = velocity.dot(normal);
    let impact = (-normal_speed).max(0.0);
    let tangent = velocity - normal * normal_speed;
    let speed = tangent.length();
    let remaining = if speed <= STATIC_FRICTION * impact {
        DVec3::ZERO
    } else if speed > 0.0 {
        tangent * ((speed - SLIDING_FRICTION * impact).max(0.0) / speed)
    } else {
        DVec3::ZERO
    };
    (remaining + normal * normal_speed.max(0.0), impact)
}

pub fn can_rest(acceleration: DVec3, normal: DVec3) -> bool {
    let into = -acceleration.dot(normal);
    let tangent = acceleration + normal * into;
    into >= 0.0 && tangent.length() <= STATIC_FRICTION * into + 1e-10
}

/// First sampled crossing of a signed clearance along a segment, refined by bisection.
/// Two-metre samples (at most 256) also catch terrain between clear endpoints. This is a
/// bounded height-field sweep, not exact continuous collision detection for arbitrary meshes.
pub fn sweep(distance_m: f64, clearance: impl Fn(f64) -> f64) -> Option<f64> {
    let samples = (distance_m / 2.0).ceil().clamp(1.0, 256.0) as usize;
    let mut previous = clearance(0.0);
    if previous < -1e-5 {
        return Some(0.0);
    }
    for i in 1..=samples {
        let end = i as f64 / samples as f64;
        let current = clearance(end);
        if current <= 0.0 {
            let mut lo = (i - 1) as f64 / samples as f64;
            if previous <= 0.0 {
                return Some(lo);
            }
            let mut hi = end;
            for _ in 0..24 {
                let mid = (lo + hi) * 0.5;
                if clearance(mid) > 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(hi);
        }
        previous = current;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grazing_impact_keeps_slip_and_friction_cannot_reverse_or_add_energy() {
        let n = DVec3::new(0.0, 1.0, 0.0);
        let v = DVec3::new(100.0, -2.0, 0.0);
        let (after, impact) = resolve_velocity(v, n);
        assert_eq!(impact, 2.0);
        assert!((after - DVec3::new(98.8, 0.0, 0.0)).length() < 1e-10);
        assert!(after.length_squared() < v.length_squared());
        assert_eq!(
            resolve_velocity(DVec3::new(0.1, -2.0, 0.0), n).0,
            DVec3::ZERO
        );
        let leaving = DVec3::new(10.0, 2.0, 0.0);
        assert_eq!(resolve_velocity(leaving, n), (leaving, 0.0));
    }

    #[test]
    fn static_friction_holds_shallow_slopes_and_steep_slopes_slide_downhill() {
        let gravity = DVec3::new(0.0, -10.0, 0.0);
        for (degrees, holds) in [(20.0_f64, true), (50.0_f64, false)] {
            let angle = degrees.to_radians();
            let n = DVec3::new(-angle.sin(), angle.cos(), 0.0);
            assert_eq!(can_rest(gravity, n), holds);
            let (v, _) = resolve_velocity(gravity * 0.02, n);
            if holds {
                assert_eq!(v, DVec3::ZERO);
            } else {
                assert!(v.x < 0.0 && v.y < 0.0);
                assert!(v.dot(n).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn sweep_finds_intermediate_ridges_and_allows_departing_the_surface() {
        // Both endpoints clear; a four-metre-wide ridge crosses the path halfway through.
        let hit = sweep(20.0, |t| ((t - 0.5).abs() * 20.0) - 2.0).unwrap();
        assert!((hit - 0.4).abs() < 1e-7);
        assert_eq!(sweep(20.0, |t| t * 20.0), None);
        assert_eq!(sweep(20.0, |t| -1.0 - t), Some(0.0));
    }
}

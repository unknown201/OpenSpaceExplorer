//! Part temperatures as the original runs them (`ThermalComponentJob`, `ThermalBehavior`): above Mach 1 the
//! parts the airflow reaches heat up with speed and air density, every part cools into the air around it,
//! heat shields facing the flow ablate to turn the heat away, and a part past its maximum temperature burns
//! up. The constants are the `PhysicsSettings` defaults.

use ose_math::DVec3;

/// Temperature a part starts at, K (`AVERAGE_INITIAL_TEMPERATURE`).
pub const INITIAL_TEMPERATURE_K: f64 = 280.0;
/// Thermal mass per tonne (`StandardSpecificHeatCapacity`) and the smallest thermal mass.
pub const SPECIFIC_HEAT: f64 = 500.0;
pub const MIN_THERMAL_MASS: f64 = 1e-4;
pub const ATMOSPHERE_FLUX_SCALE: f64 = 50.0;
const POSITIVE_RATE_SCALE: f64 = 1.0;
const NEGATIVE_RATE_SCALE: f64 = 5.0;
const BACKGROUND_RATE_SCALE: f64 = 1.0;
const BACKGROUND_EPSILON_K: f64 = 1.0;
pub const REENTRY_FLUX_SCALE: f64 = 500.0;
pub const REENTRY_SKIN_FRICTION_SCALE: f64 = 1.0;
pub const REENTRY_MACH_EXPONENT: f64 = 3.0;
pub const REENTRY_DENSITY_EXPONENT: f64 = 0.53;
pub const THERMAL_SKIN_FRICTION: f64 = 0.3;
const ATMOSPHERIC_CONDUCTIVITY: f64 = 0.026;
const THERMAL_WINDOW_SMOOTHING_M: f64 = 500.0;

/// The heat difficulty options (`HeatEnabled`, `HeatScaling`, `HeatPartDestruction`).
#[derive(bevy_ecs::prelude::Resource, Clone, Copy, Debug, PartialEq)]
pub struct HeatSettings {
    pub enabled: bool,
    pub scale: f64,
    pub part_destruction: bool,
}

impl Default for HeatSettings {
    fn default() -> Self {
        HeatSettings {
            enabled: true,
            scale: 1.0,
            part_destruction: true,
        }
    }
}

/// How well the air carries heat away at `alt_m` (`GetThermalConductivity`): full inside the atmosphere,
/// fading out over the last kilometre or so.
pub fn conductivity(alt_m: f64, atmosphere_depth_m: f64) -> f64 {
    let t = ((alt_m - atmosphere_depth_m + 2.0 * THERMAL_WINDOW_SMOOTHING_M)
        / THERMAL_WINDOW_SMOOTHING_M)
        .tanh();
    -0.5 * (ATMOSPHERIC_CONDUCTIVITY * t - ATMOSPHERIC_CONDUCTIVITY)
}

/// Re-entry heat into a part, kW (`UpdateReentryFlux`).
pub fn reentry_flux(mach: f64, exposed: f64, airstream_area_m2: f64, density: f64, cd: f64) -> f64 {
    if exposed <= 0.0 || cd <= 0.0 || density <= 0.0 {
        return 0.0;
    }
    REENTRY_FLUX_SCALE
        * mach.powf(REENTRY_MACH_EXPONENT)
        * exposed
        * airstream_area_m2
        * density.powf(REENTRY_DENSITY_EXPONENT)
        * REENTRY_SKIN_FRICTION_SCALE
        * cd.powf(THERMAL_SKIN_FRICTION)
}

/// One temperature step for a part (`IterationJob.Execute`): the air takes heat from a part hotter than it,
/// re-entry adds heat, and with no flux at all the part drifts to the ambient temperature. Cooling stops at
/// the ambient temperature so long steps stay stable.
#[allow(clippy::too_many_arguments)]
pub fn step_temperature(
    temp_k: f64,
    thermal_mass: f64,
    wetted_area_m2: f64,
    ambient_k: f64,
    conductivity: f64,
    heat_in_kw: f64,
    heat_scale: f64,
    dt: f64,
) -> f64 {
    let air = (ambient_k - temp_k).min(0.0) * wetted_area_m2 * ATMOSPHERE_FLUX_SCALE * conductivity;
    let flux = (air + heat_in_kw) * heat_scale;
    let next = if flux > 0.0 {
        temp_k + flux * POSITIVE_RATE_SCALE / thermal_mass * dt
    } else if flux < 0.0 {
        (temp_k + flux * NEGATIVE_RATE_SCALE / thermal_mass * dt).max(ambient_k.min(temp_k))
    } else if (ambient_k - temp_k).abs() < BACKGROUND_EPSILON_K {
        ambient_k
    } else {
        let step = (ambient_k - temp_k) / (thermal_mass * BACKGROUND_RATE_SCALE) * dt;
        if (ambient_k - temp_k).abs() <= step.abs() {
            ambient_k
        } else {
            temp_k + step
        }
    };
    next.max(0.0)
}

/// How much of each part the airflow reaches (`OcclusionAlgorithmJob`), for parts as boxes (centre and half
/// size, vessel frame) and the direction of travel through the air: parts are taken front to back, and each
/// is shadowed by the outline (across the flow) of every part ahead of it that is itself reached. Below
/// Mach 1 nothing is reached (`UpdateOcclusionConvection`): there is no re-entry heating.
pub fn exposed_proportions(boxes: &[(DVec3, DVec3)], travel: DVec3, mach: f64) -> Vec<f64> {
    let n = boxes.len();
    if mach < 1.0 || !mach.is_finite() || n == 0 {
        return vec![0.0; n];
    }
    let d = travel.normalized();
    let reference = if d.x.abs() < 0.9 {
        DVec3::new(1.0, 0.0, 0.0)
    } else {
        DVec3::new(0.0, 1.0, 0.0)
    };
    let e1 = d.cross(reference).normalized();
    let e2 = d.cross(e1);
    // projected outline (min, max across the flow) and how far forward each part reaches
    let shapes: Vec<([f64; 2], [f64; 2], f64)> = boxes
        .iter()
        .map(|(c, h)| {
            let (mut lo, mut hi, mut front) = ([f64::MAX; 2], [f64::MIN; 2], f64::MIN);
            for k in 0..8 {
                let corner = *c
                    + DVec3::new(
                        if k & 1 == 0 { h.x } else { -h.x },
                        if k & 2 == 0 { h.y } else { -h.y },
                        if k & 4 == 0 { h.z } else { -h.z },
                    );
                let uv = [corner.dot(e1), corner.dot(e2)];
                for a in 0..2 {
                    lo[a] = lo[a].min(uv[a]);
                    hi[a] = hi[a].max(uv[a]);
                }
                front = front.max(corner.dot(d));
            }
            (lo, hi, front)
        })
        .collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| shapes[b].2.total_cmp(&shapes[a].2));
    let mut exposed = vec![0.0; n];
    let mut cones: Vec<usize> = Vec::new();
    for &i in &order {
        let (lo, hi, _) = shapes[i];
        let mut e: f64 = 1.0;
        for &j in &cones {
            let (clo, chi, _) = shapes[j];
            let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
            let overlap = if area > 0.0 {
                let w = (hi[0].min(chi[0]) - lo[0].max(clo[0])).max(0.0);
                let h = (hi[1].min(chi[1]) - lo[1].max(clo[1])).max(0.0);
                w * h / area
            } else {
                0.0
            };
            e = e.min(1.0 - overlap.min(1.0));
            if e >= 0.999 {
                e = 1.0;
            }
        }
        exposed[i] = e;
        if e > 0.0 {
            cones.push(i);
        }
    }
    exposed
}

/// Conservative box-outline occlusion for one point's local flow, O(parts).
/// Unlike the uniform-flow sweep, each rotating part may see a different upstream direction.
pub fn exposed_proportion_at(
    boxes: &[(DVec3, DVec3)],
    target: usize,
    travel: DVec3,
    mach: f64,
) -> f64 {
    if mach < 1.0 || !mach.is_finite() || travel.length_squared() == 0.0 {
        return 0.0;
    }
    let d = travel.normalized();
    let reference = if d.x.abs() < 0.9 {
        DVec3::new(1.0, 0.0, 0.0)
    } else {
        DVec3::new(0.0, 1.0, 0.0)
    };
    let e1 = d.cross(reference).normalized();
    let e2 = d.cross(e1);
    let extent = |axis: DVec3, half: DVec3| {
        axis.x.abs() * half.x + axis.y.abs() * half.y + axis.z.abs() * half.z
    };
    let shape = |(c, h): (DVec3, DVec3)| {
        let u = c.dot(e1);
        let v = c.dot(e2);
        let a = extent(e1, h);
        let b = extent(e2, h);
        ([u - a, v - b], [u + a, v + b], c.dot(d) + extent(d, h))
    };
    let (lo, hi, front) = shape(boxes[target]);
    let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
    if area <= 0.0 {
        return 1.0;
    }
    boxes
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != target)
        .fold(1.0_f64, |exposed, (i, &b)| {
            let (blo, bhi, bfront) = shape(b);
            if bfront < front || (bfront == front && i > target) {
                return exposed;
            }
            let width = (hi[0].min(bhi[0]) - lo[0].max(blo[0])).max(0.0);
            let height = (hi[1].min(bhi[1]) - lo[1].max(blo[1])).max(0.0);
            exposed.min(1.0 - (width * height / area).min(1.0))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_behind_another_of_the_same_size_is_shadowed() {
        let up = DVec3::new(0.0, 1.0, 0.0);
        let unit = DVec3::new(0.5, 0.5, 0.5);
        let stack = [(DVec3::ZERO, unit), (DVec3::new(0.0, 1.0, 0.0), unit)];
        // falling bottom first: the lower box takes the flow, the upper one is in its shadow
        let e = exposed_proportions(&stack, -up, 5.0);
        assert_eq!(e, vec![1.0, 0.0]);
        // half to the side: half shadowed
        let side = [(DVec3::ZERO, unit), (DVec3::new(0.5, 1.0, 0.0), unit)];
        let e = exposed_proportions(&side, -up, 5.0);
        assert!((e[1] - 0.5).abs() < 1e-9, "{e:?}");
        // subsonic: nothing heats
        assert_eq!(exposed_proportions(&stack, -up, 0.8), vec![0.0, 0.0]);
    }

    #[test]
    fn re_entry_heat_grows_with_speed_and_air_and_the_air_cools() {
        let slow = reentry_flux(3.0, 1.0, 1.0, 0.01, 1.0);
        let fast = reentry_flux(6.0, 1.0, 1.0, 0.01, 1.0);
        assert!(
            (fast / slow - 8.0).abs() < 1e-9,
            "flux goes with Mach cubed"
        );
        assert!(reentry_flux(6.0, 1.0, 1.0, 0.1, 1.0) > fast);
        // a hot part in the lower atmosphere cools toward the air, never below it
        let k = conductivity(1_000.0, 70_000.0);
        assert!((k - ATMOSPHERIC_CONDUCTIVITY).abs() < 1e-6);
        assert!(
            conductivity(80_000.0, 70_000.0) < 1e-9,
            "no air above the atmosphere"
        );
        let mut t = 900.0;
        for _ in 0..2_000 {
            t = step_temperature(t, 100.0, 4.0, 288.0, k, 0.0, 1.0, 0.02);
        }
        assert!((288.0..400.0).contains(&t), "{t}");
        // in vacuum with nothing heating it a part drifts slowly to the ambient temperature
        let t2 = step_temperature(300.0, 100.0, 4.0, 200.0, 0.0, 0.0, 1.0, 1.0);
        assert!((t2 - 299.0).abs() < 1e-9, "{t2}");
    }
}

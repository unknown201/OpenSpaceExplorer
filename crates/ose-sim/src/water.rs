//! Water as the original has it (`PartBehavior.BuoyancyUpdate`, `CalculateDisplacement`, `CalculateLiquidDrag`):
//! each part displaces a volume taken from its drag cube, scaled by its own `buoyancy` and the
//! `BuoyancyScalar`, in proportion to how far its bounds are under the surface; a part breaks on entering the
//! water when the vessel's downward speed (or a tenth of its speed) is over 1.2 times its crash tolerance; and
//! the water slows each part as a PhysX linear damping whose strength eases from 4.5 down to 0.15 times the
//! part's drag-cube drag over the first seconds afloat. The original has no flooding and no waves.
use ose_data::PartDef;

/// Water settings exported from the installed reference's loaded PhysicsSettings JSON.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct WaterSettings {
    pub version: String,
    pub source_sha256: String,
    pub buoyancy_scalar: f64,
    pub scale_above_depth_m: f64,
    pub default_volume_m3: f64,
    pub min_crash_mult: f64,
    pub crash_tolerance_mult: f64,
    pub drag_scalar: f64,
    pub drag_scalar_end: f64,
    pub drag_scalar_lerp: f64,
    pub drag_scalar_lerp_dot_mult_base: f64,
    pub drag_scalar_lerp_dot_mult: f64,
    pub drag_min_vel: f64,
    pub drag_min_vel_mult: f64,
    pub drag_part_vel_greater_vessel_mult: f64,
    pub drag_timer_s: f64,
    pub drag_mult_min_for_min_dot: f64,
    pub angular_drag_min_control_sqr_mag: f64,
    pub angular_drag_slow: f64,
    pub drag_slow: f64,
}

pub fn settings() -> &'static WaterSettings {
    static SETTINGS: std::sync::OnceLock<WaterSettings> = std::sync::OnceLock::new();
    SETTINGS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/own/water-settings.json"))
            .expect("validated reference water settings")
    })
}

/// Water a part displaces when fully under, m^3 (`CalculateDisplacement`): its drag cube's box, shrunk by how
/// much of each face the cube fills.
pub fn displacement_m3(def: &PartDef) -> f64 {
    if let Some(c) = def.drag_cubes.first() {
        let (x, y, z) = (c.size.x, c.size.y, c.size.z);
        let mut volume = x * y * z;
        let fill = |area: f64, a: f64, b: f64| {
            if a * b > 0.0 {
                area / (a * b)
            } else {
                f64::NAN
            }
        };
        let (fx, fy, fz) = (
            fill(c.area[0], y, z),
            fill(c.area[2], x, z),
            fill(c.area[4], x, y),
        );
        let sum = fx + fy + fz;
        if sum.is_finite() && sum > 0.0 {
            volume *= (fx.min(fz) + 2.0 * (fx * fz)) / 3.0 * fy;
        }
        volume
    } else if let Some(b) = def.bounds {
        b.size_m.x * b.size_m.y * b.size_m.z * 0.75
    } else {
        settings().default_volume_m3
    }
}

/// Share of a part's buoyancy at these corner depths (positive under water), m: the wet share of its
/// height, and less again while its deepest corner is within `BuoyancyScaleAboveDepth` of the surface.
pub fn submerged_share(min_depth: f64, max_depth: f64) -> f64 {
    let share = if max_depth < 0.0 || min_depth >= max_depth {
        0.0
    } else if min_depth >= 0.0 {
        1.0
    } else {
        max_depth / (max_depth - min_depth)
    };
    let shallow = settings().scale_above_depth_m;
    if max_depth < shallow {
        share * (max_depth / shallow).max(0.0)
    } else {
        share
    }
}

/// Whether a part entering the water at the vessel's surface speed `speed`, of which `down` is downward,
/// breaks.
pub fn breaks_on_entry(speed: f64, down: f64, crash_tolerance: f64) -> bool {
    let s = settings();
    (s.min_crash_mult * speed).max(down) > crash_tolerance * s.crash_tolerance_mult
}

/// One part's water drag state: the drag scalar easing from its splash value to its floating value, and how
/// long the part has been in the water (counted up to `BuoyancyWaterDragTimer`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterDrag {
    pub scalar: f64,
    pub splash_s: f64,
}

impl Default for WaterDrag {
    fn default() -> Self {
        Self {
            scalar: settings().drag_scalar,
            splash_s: 0.0,
        }
    }
}

/// How the water slows a part this step: a linear damping rate, 1/s, on its velocity, and an angular one
/// on its spin when the original overrides it (`None`: the part's usual angular drag).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Damping {
    pub linear: f64,
    pub angular: Option<f64>,
}

impl WaterDrag {
    /// Advance one step in the water (`CalculateLiquidDrag`). `speed` is the vessel's surface speed,
    /// `horizontal` its horizontal part, `dot_abs` how vertical its motion is (1 straight up or down),
    /// `entering` true on the step the part met the water. `area_drag` gives the part's drag-cube `AreaDrag`
    /// for the motion, `submerged` its wet share, `mass_t` its mass, `density` the water's, t/m^3.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        speed: f64,
        horizontal: f64,
        dot_abs: f64,
        entering: bool,
        steering_sqr: f64,
        area_drag: impl FnOnce() -> f64,
        submerged: f64,
        mass_t: f64,
        density: f64,
        dt: f64,
    ) -> Damping {
        let s = settings();
        let early = self.splash_s < s.drag_timer_s;
        if early {
            self.splash_s += dt;
        }
        let slow_drop = speed < s.drag_min_vel * s.drag_min_vel_mult;
        let lerp_mult = s.drag_scalar_lerp_dot_mult_base - dot_abs * s.drag_scalar_lerp_dot_mult;
        let top = if early {
            s.drag_scalar * (0.5 + dot_abs)
        } else {
            s.drag_scalar
        };
        if !slow_drop {
            if !early && self.scalar > s.drag_scalar_end {
                self.scalar +=
                    (s.drag_scalar_end - self.scalar) * s.drag_scalar_lerp * lerp_mult * dt;
            }
        } else if self.scalar < top {
            self.scalar += (top - self.scalar) * s.drag_scalar_lerp * dt;
        }
        if early && entering {
            self.scalar = self.scalar.min(
                (s.drag_mult_min_for_min_dot + dot_abs * (1.0 - s.drag_mult_min_for_min_dot))
                    * (top - s.drag_scalar_end)
                    + s.drag_scalar_end,
            );
        }
        if speed < s.drag_min_vel || speed > horizontal * s.drag_part_vel_greater_vessel_mult {
            // drifting, or bobbing up and down: a flat damping, and the spin damped too unless steered
            Damping {
                linear: s.drag_slow,
                angular: (steering_sqr < s.angular_drag_min_control_sqr_mag)
                    .then_some(s.angular_drag_slow),
            }
        } else {
            let aero = crate::aero::settings();
            let force_per_mps = 0.5
                * density
                * speed
                * area_drag()
                * aero.drag_cube_multiplier
                * self.scalar
                * submerged
                * aero.drag_multiplier;
            Damping {
                linear: force_per_mps / if mass_t != 0.0 { mass_t } else { 0.1 },
                angular: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<PartDef> {
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap()
    }

    #[test]
    fn the_mk1_pod_displaces_two_thirds_of_a_cubic_metre_and_floats_three_times_over() {
        let defs = catalog();
        let pod = defs.iter().find(|d| d.id == "pod_1v_conical_crew").unwrap();
        let v = displacement_m3(pod);
        assert!((v - 0.6705).abs() < 0.002, "{v}");
        assert_eq!(pod.buoyancy, 3.0);
        let adapter = defs.iter().find(|d| d.id == "adapter_1v_cir-sqr").unwrap();
        assert_eq!(adapter.buoyancy, 0.0, "adapters do not float");
    }

    #[test]
    fn buoyancy_grows_with_the_wet_height_and_eases_in_near_the_surface() {
        assert_eq!(submerged_share(0.1, 2.0), 1.0);
        assert_eq!(submerged_share(-1.0, -0.1), 0.0);
        assert!((submerged_share(-1.0, 1.0) - 0.5).abs() < 1e-12);
        // 0.1 m of a 1 m part: a tenth wet, halved again for being within 0.2 m of the surface
        assert!((submerged_share(-0.9, 0.1) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn water_entry_breaks_parts_on_vertical_speed_over_one_point_two_tolerances() {
        // the Mk1 pod (15 m/s): 17 m/s straight down survives, 19 breaks; 100 m/s skimming breaks (10% rule)
        assert!(!breaks_on_entry(17.0, 17.0, 15.0));
        assert!(breaks_on_entry(19.0, 19.0, 15.0));
        assert!(!breaks_on_entry(60.0, 5.0, 15.0));
        assert!(breaks_on_entry(190.0, 5.0, 15.0));
    }

    #[test]
    fn splash_drag_eases_to_the_floating_value() {
        let mut d = WaterDrag::default();
        // a vertical splash at 10 m/s with no horizontal motion damps flat at 2/s and leaves the scalar be
        let first = d.step(10.0, 0.0, 1.0, true, 0.0, || 1.0, 1.0, 1.0, 1.0, 0.02);
        assert_eq!(first.linear, settings().drag_slow);
        assert_eq!(first.angular, Some(settings().angular_drag_slow));
        // skimming in at 5 m/s: the scalar starts low (0.255: a flat entry), then fades to 0.15 after the
        // first three seconds
        let mut boat = WaterDrag::default();
        let start = boat.step(5.0, 5.0, 0.0, true, 0.0, || 1.0, 1.0, 1.0, 1.0, 0.02);
        assert!((boat.scalar - 0.255).abs() < 1e-9, "{}", boat.scalar);
        // 0.5 * 1 t/m^3 * 5 m/s * AreaDrag 1 * 0.1 * 0.255 * 8 / 1 t
        assert!((start.linear - 0.51).abs() < 1e-9, "{start:?}");
        for _ in 0..1000 {
            boat.step(5.0, 5.0, 0.0, false, 0.0, || 1.0, 1.0, 1.0, 1.0, 0.02);
        }
        let later = boat.step(5.0, 5.0, 0.0, false, 0.0, || 1.0, 1.0, 1.0, 1.0, 0.02);
        assert!(later.linear < start.linear, "{start:?} {later:?}");
        assert!((boat.scalar - settings().drag_scalar_end).abs() < 0.02);
    }
}

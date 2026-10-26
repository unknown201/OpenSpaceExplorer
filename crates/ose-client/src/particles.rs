//! Reference pressure mapping and a jitter-tolerant visual clock.
/// Visual thrust must follow the fuel-fed readout, not the throttle lever left at full after burnout.
pub fn exhaust_throttle(
    firing: bool,
    thrust_kn: f64,
    throttle: f64,
    solid: bool,
    crashed: bool,
) -> f64 {
    if crashed || !firing || !thrust_kn.is_finite() || thrust_kn <= 0.001 {
        return 0.0;
    }
    if solid {
        1.0
    } else {
        throttle.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Impact {
    pub position: [f64; 3],
    pub started: f64,
    pub fueled: bool,
    pub fuel: &'static str,
}

/// One burst for an observed live-to-crashed transition. Loading an old wreck must not replay it.
#[derive(Default)]
pub struct CrashFx {
    previous: Option<bool>,
    last_fueled: bool,
    last_fuel: &'static str,
    impact: Option<Impact>,
}
impl CrashFx {
    pub fn update(
        &mut self,
        crashed: bool,
        fueled: bool,
        ut: f64,
        position: [f64; 3],
    ) -> Option<Impact> {
        self.update_fuel(
            crashed,
            if fueled { "methalox" } else { "noFuel" },
            ut,
            position,
        )
    }
    pub fn update_fuel(
        &mut self,
        crashed: bool,
        fuel: &'static str,
        ut: f64,
        position: [f64; 3],
    ) -> Option<Impact> {
        if self.previous == Some(false) && crashed {
            self.impact = Some(Impact {
                position,
                started: ut,
                fueled: self.last_fueled,
                fuel: self.last_fuel,
            });
        }
        if !crashed {
            self.last_fueled = fuel != "noFuel";
            self.last_fuel = fuel;
        }
        self.previous = Some(crashed);
        // Keep the source alive through its delayed emissions and smoke lifetime.
        if self
            .impact
            .is_some_and(|hit| ut < hit.started || ut - hit.started > 20.0)
        {
            self.impact = None;
        }
        self.impact
    }
}

/// The resource definitions assign vfxFuelType; Methalox requires both recipe ingredients.
pub fn fuel_type(resources: &[(String, f64, f64)], legacy_propellant: f64) -> &'static str {
    let has = |name: &str| {
        resources
            .iter()
            .any(|(id, n, _)| id.eq_ignore_ascii_case(name) && *n > 1e-6)
    };
    if has("Methane") && has("Oxidizer") {
        return "methalox";
    }
    let largest = resources
        .iter()
        .filter(|(_, n, _)| *n > 1e-6)
        .filter_map(|(id, n, _)| {
            let fuel = match id.to_ascii_lowercase().as_str() {
                "solidfuel" => "solidFuel",
                "hydrogen" | "methane" | "oxidizer" | "monopropellant" | "evapropellant"
                | "xenon" => "pressurized",
                _ => return None,
            };
            Some((fuel, *n))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    largest
        .map(|r| r.0)
        .unwrap_or(if resources.is_empty() && legacy_propellant > 1e-6 {
            "methalox"
        } else {
            "noFuel"
        })
}

pub fn explosion_asset(fueled: bool, atmosphere: bool) -> &'static str {
    match (fueled, atmosphere) {
        (true, true) => "fx_explosion_methalox_atmosphere",
        (true, false) => "fx_explosion_methalox_vacuum",
        (false, true) => "fx_explosion_noFuel_atmosphere",
        (false, false) => "fx_explosion_noFuel_vacuum",
    }
}

/// A thruster's exhaust points opposite its force. Rotation selects only nozzles whose moment
/// helps the requested turn, rather than lighting every nozzle on a multi-directional RCS block.
pub fn rcs_nozzle_throttle(
    position: glam::DVec3,
    exhaust: glam::DVec3,
    translation: glam::DVec3,
    rotation: glam::DVec3,
    rotate_only: bool,
    jetpack: bool,
) -> f64 {
    let direction = exhaust.normalize_or_zero();
    if rotate_only {
        let alignment = direction.dot(rotation.normalize_or_zero());
        return if alignment >= 0.5 {
            (alignment * rotation.length()).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    let alignment = (-direction).dot(translation.normalize_or_zero());
    let linear = if !jetpack || alignment >= 0.5 {
        (-direction).dot(translation).max(0.0)
    } else {
        0.0
    };
    let angular = if jetpack {
        0.0
    } else {
        position
            .cross(-direction)
            .normalize_or_zero()
            .dot(rotation)
            .max(0.0)
    };
    (linear + angular).clamp(0.0, 1.0)
}

/// Tracks contextual action edges without replaying effects when joining an existing flight.
#[derive(Default)]
pub struct EffectEdges(std::collections::HashMap<(u8, usize), bool>);
impl EffectEdges {
    pub fn rising(&mut self, kind: u8, part: usize, active: bool) -> bool {
        self.0.insert((kind, part), active) == Some(false) && active
    }
}

pub struct Burst {
    pub instance: u64,
    pub asset: String,
    pub model: glam::DMat4,
    pub started: f64,
    pub relative_velocity: f64,
}
#[derive(Default)]
pub struct Bursts {
    next: u64,
    pub active: Vec<Burst>,
}
impl Bursts {
    pub fn queue(
        &mut self,
        asset: impl Into<String>,
        model: glam::DMat4,
        ut: f64,
        relative_velocity: f64,
    ) {
        self.next += 1;
        self.active.push(Burst {
            instance: 0x1000000000000000 | self.next,
            asset: asset.into(),
            model,
            started: ut,
            relative_velocity,
        });
    }
    pub fn expire(&mut self, ut: f64) {
        self.active
            .retain(|b| ut >= b.started && ut - b.started < 60.0);
    }
}
/// The estimated network clock can move slightly backwards on each snapshot. Hold its high-water
/// mark instead of clearing a trail on those normal corrections. Real jumps/long pauses still reset.
#[derive(Default)]
pub struct ParticleClock {
    previous: Option<(f64, f64)>,
}

impl ParticleClock {
    pub fn reset(&mut self) {
        self.previous = None;
    }
    pub fn sample(&mut self, ut: f64, wall_ms: f64) -> f64 {
        let Some((last_ut, last_wall)) = self.previous else {
            self.previous = Some((ut, wall_ms));
            return 0.0;
        };
        let dt = ut - last_ut;
        if !dt.is_finite() || dt.abs() > 0.25 || wall_ms - last_wall > 500.0 {
            self.previous = Some((ut, wall_ms));
            return f64::INFINITY;
        }
        self.previous = Some((ut.max(last_ut), wall_ms));
        dt.max(0.0)
    }
}

/// KSP atmosphereVFXCurve: cubic Hermite keys (0,0,3), (.4,.8,.8), (1,1,0).
pub fn pressure_multiplier(kpa: f64) -> f64 {
    if !kpa.is_finite() {
        return 0.0;
    }
    let p = (kpa / 101.325).clamp(0.0, 1.0);
    let (x0, x1, y0, y1, m0, m1) = if p < 0.4 {
        (0.0, 0.4, 0.0, 0.8, 3.0, 0.8)
    } else {
        (0.4, 1.0, 0.8, 1.0, 0.8, 0.0)
    };
    let t: f64 = (p - x0) / (x1 - x0);
    (2.0 * t.powi(3) - 3.0 * t * t + 1.0) * y0
        + (t.powi(3) - 2.0 * t * t + t) * (x1 - x0) * m0
        + (-2.0 * t.powi(3) + 3.0 * t * t) * y1
        + (t.powi(3) - t * t) * (x1 - x0) * m1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_edges_do_not_replay_loaded_or_unchanged_state() {
        let mut edges = EffectEdges::default();
        assert!(!edges.rising(1, 2, true));
        assert!(!edges.rising(1, 2, true));
        assert!(!edges.rising(1, 2, false));
        assert!(edges.rising(1, 2, true));
        assert!(!edges.rising(1, 2, true));
        assert!(!edges.rising(1, 3, true));
    }
    #[test]
    fn explosion_families_follow_remaining_resources_including_reserve_tanks() {
        let r = |id: &str, n| (id.into(), n, 10.0);
        assert_eq!(
            fuel_type(&[r("Methane", 1.0), r("Oxidizer", 2.0)], 0.0),
            "methalox"
        );
        assert_eq!(
            fuel_type(&[r("Methane", 0.0), r("Oxidizer", 2.0)], 0.0),
            "pressurized"
        );
        assert_eq!(fuel_type(&[r("SolidFuel", 2.0)], 0.0), "solidFuel");
        assert_eq!(fuel_type(&[r("Hydrogen", 2.0)], 0.0), "pressurized");
        assert_eq!(fuel_type(&[r("ElectricCharge", 10.0)], 0.0), "noFuel");
    }
    #[test]
    fn translation_and_torque_fire_only_helpful_nozzles() {
        use glam::DVec3 as V;
        assert_eq!(
            rcs_nozzle_throttle(V::ZERO, -V::X, V::X, V::ZERO, false, false),
            1.0
        );
        assert_eq!(
            rcs_nozzle_throttle(V::ZERO, V::X, V::X, V::ZERO, false, false),
            0.0
        );
        assert_eq!(
            rcs_nozzle_throttle(V::X, -V::Y, V::ZERO, V::Z, false, false),
            1.0
        );
        assert_eq!(
            rcs_nozzle_throttle(V::X, V::Y, V::ZERO, V::Z, false, false),
            0.0
        );
        assert_eq!(
            rcs_nozzle_throttle(V::ZERO, V::X, V::ZERO, V::ZERO, false, false),
            0.0
        );
        assert_eq!(
            rcs_nozzle_throttle(V::ZERO, V::Y, V::ZERO, V::Y, true, true),
            1.0
        );
    }
    #[test]
    fn fuel_starvation_and_crashes_stop_mesh_exhaust_even_with_full_throttle() {
        assert_eq!(exhaust_throttle(false, 0.0, 1.0, false, false), 0.0);
        assert_eq!(exhaust_throttle(true, 0.0, 1.0, false, false), 0.0);
        assert_eq!(exhaust_throttle(true, 100.0, 1.0, false, true), 0.0);
        assert_eq!(exhaust_throttle(true, 100.0, 0.0, true, false), 1.0);
    }
    #[test]
    fn crash_bursts_are_once_only_and_stay_at_the_impact_point() {
        let mut fx = CrashFx::default();
        assert!(fx.update(false, true, 1.0, [0.0; 3]).is_none());
        let hit = fx.update(true, false, 2.0, [1.0, 2.0, 3.0]).unwrap();
        assert!(hit.fueled, "use the pre-impact resource state");
        assert_eq!(
            fx.update(true, false, 3.0, [9.0; 3]).unwrap().position,
            hit.position
        );
        assert_eq!(fx.update(true, false, 4.0, [9.0; 3]).unwrap().started, 2.0);
        assert!(fx.update(true, false, 23.0, [9.0; 3]).is_none());
        assert!(fx.update(true, false, 24.0, [9.0; 3]).is_none());
        assert!(CrashFx::default()
            .update(true, false, 2.0, [0.0; 3])
            .is_none());
    }
    #[test]
    fn empty_rockets_use_the_dry_impact_recipe() {
        let mut fx = CrashFx::default();
        fx.update(false, false, 1.0, [0.0; 3]);
        let hit = fx.update(true, false, 2.0, [0.0; 3]).unwrap();
        assert_eq!(
            explosion_asset(hit.fueled, true),
            "fx_explosion_noFuel_atmosphere"
        );
        assert_eq!(explosion_asset(true, false), "fx_explosion_methalox_vacuum");
    }
    #[test]
    fn snapshots_preserve_the_clock_high_water_mark() {
        let mut clock = ParticleClock::default();
        assert_eq!(clock.sample(10.0, 0.0), 0.0);
        assert!((clock.sample(10.1, 100.0) - 0.1).abs() < 1e-9);
        assert_eq!(clock.sample(10.08, 116.0), 0.0);
        assert!((clock.sample(10.12, 133.0) - 0.02).abs() < 1e-9);
        assert!(!clock.sample(20.0, 150.0).is_finite());
    }
    #[test]
    fn reference_pressure_curve_hits_its_authored_keys() {
        assert_eq!(pressure_multiplier(0.0), 0.0);
        assert!((pressure_multiplier(40.53) - 0.8).abs() < 1e-9);
        assert_eq!(pressure_multiplier(101.325), 1.0);
    }
}

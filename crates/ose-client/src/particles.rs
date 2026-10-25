//! Reference pressure mapping and a jitter-tolerant visual clock.
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

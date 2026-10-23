//! Kepler equation solvers.

/// Solve `M = E - e sin E` for eccentric anomaly `E` (elliptic case, `0 <= e < 1`).
///
/// Newton iteration; converges to 1e-14 rad within a handful of steps.
pub fn eccentric_anomaly(mean_anomaly: f64, e: f64) -> f64 {
    debug_assert!((0.0..1.0).contains(&e), "elliptic solver needs 0 <= e < 1");
    let m = mean_anomaly.rem_euclid(std::f64::consts::TAU);
    let mut ea = if e < 0.8 { m } else { std::f64::consts::PI };
    for _ in 0..50 {
        let f = ea - e * ea.sin() - m;
        let fp = 1.0 - e * ea.cos();
        let d = f / fp;
        ea -= d;
        if d.abs() < 1e-14 {
            break;
        }
    }
    ea
}

/// True anomaly from eccentric anomaly (elliptic case).
pub fn true_anomaly_from_eccentric(ea: f64, e: f64) -> f64 {
    let (s, c) = (ea / 2.0).sin_cos();
    2.0 * ((1.0 + e).sqrt() * s).atan2((1.0 - e).sqrt() * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_orbit_is_identity() {
        for m in [0.0, 1.0, 3.0, 6.0] {
            assert!((eccentric_anomaly(m, 0.0) - m).abs() < 1e-12);
        }
    }

    #[test]
    fn solves_keplers_equation() {
        for e in [0.1, 0.5, 0.9, 0.99] {
            for m in [0.1, 1.0, 2.5, 4.0, 6.0] {
                let ea = eccentric_anomaly(m, e);
                assert!((ea - e * ea.sin() - m).abs() < 1e-12, "e={e} m={m}");
            }
        }
    }
}

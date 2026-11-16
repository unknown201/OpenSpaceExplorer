//! Continuous orbit picking for maneuver placement and timing drags (CSS pixels, seconds).

/// Arrow-key increments in seconds or m/s: Shift is coarse, Ctrl is fine.
pub fn slider_increment(shift: bool, ctrl: bool) -> f64 {
    (if shift { 10.0 } else { 1.0 }) * (if ctrl { 0.1 } else { 1.0 })
}

/// Return squared pointer distance and interpolated UT on the projected orbit. During a drag,
/// lift a closed orbit's time to the revolution nearest the previous node, avoiding a jump at
/// the screen-space seam. A partial trajectory never wraps across an encounter or impact.
pub fn pick_time(
    points: &[(f64, f64, f64)],
    x: f64,
    y: f64,
    previous: Option<f64>,
    period: Option<f64>,
) -> Option<(f64, f64)> {
    points
        .split(|p| !p.0.is_finite() || !p.1.is_finite() || !p.2.is_finite())
        .filter_map(|segment| pick_segment_time(segment, x, y, previous, period))
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

fn pick_segment_time(
    points: &[(f64, f64, f64)],
    x: f64,
    y: f64,
    previous: Option<f64>,
    period: Option<f64>,
) -> Option<(f64, f64)> {
    let start = points.first()?.2;
    let end = points.last()?.2;
    let period = period.filter(|p| p.is_finite() && *p > 0.0 && (end - start - p).abs() < 1e-3);
    points
        .windows(2)
        .filter_map(|pair| {
            let [(ax, ay, at), (bx, by, bt)] = [pair[0], pair[1]];
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            if !len2.is_finite() || len2 < 1e-12 || bt <= at {
                return None;
            }
            let f = (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0);
            let d2 = (ax + f * dx - x).powi(2) + (ay + f * dy - y).powi(2);
            let mut t = at + f * (bt - at);
            if let (Some(previous), Some(period)) = (previous, period) {
                t += ((previous - t) / period).round() * period;
                t = t.max(start);
            }
            (d2.is_finite() && t.is_finite()).then_some((d2, t))
        })
        .min_by(|a, b| {
            a.0.total_cmp(&b.0).then_with(|| match previous {
                Some(t) => (a.1 - t).abs().total_cmp(&(b.1 - t).abs()),
                None => a.1.total_cmp(&b.1),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_drag_moves_continuously_between_samples() {
        let orbit = [(0.0, 0.0, 100.0), (100.0, 0.0, 200.0)];
        for k in 0..=1000 {
            let x = k as f64 / 10.0;
            let (distance, ut) = pick_time(&orbit, x, 3.0, None, None).unwrap();
            assert!((ut - (100.0 + x)).abs() < 1e-9);
            assert!((distance - 9.0).abs() < 1e-9);
        }
    }

    #[test]
    fn closed_orbit_drag_crosses_seam_without_losing_a_revolution() {
        let orbit = [
            (0.0, 0.0, 100.0),
            (10.0, 0.0, 110.0),
            (10.0, 10.0, 120.0),
            (0.0, 10.0, 130.0),
            (0.0, 0.0, 140.0),
        ];
        let before = pick_time(&orbit, 0.0, 0.1, Some(139.0), Some(40.0))
            .unwrap()
            .1;
        let after = pick_time(&orbit, 0.1, 0.0, Some(before), Some(40.0))
            .unwrap()
            .1;
        assert!((before - 139.9).abs() < 1e-9);
        assert!((after - 140.1).abs() < 1e-9);
        // A fresh click selects the first upcoming pass, independent of an old node.
        assert!((pick_time(&orbit, 0.1, 0.0, None, Some(40.0)).unwrap().1 - 100.1).abs() < 1e-9);
    }

    #[test]
    fn truncated_patch_does_not_wrap_and_degenerate_samples_are_ignored() {
        let orbit = [(0.0, 0.0, 100.0), (0.0, 0.0, 100.0), (10.0, 0.0, 110.0)];
        assert_eq!(
            pick_time(&orbit, 5.0, 0.0, Some(145.0), Some(40.0)),
            Some((0.0, 105.0))
        );
        assert_eq!(pick_time(&[], 0.0, 0.0, None, None), None);
    }

    #[test]
    fn picking_does_not_join_separate_prediction_patches() {
        let points = [
            (0.0, 0.0, 0.0),
            (10.0, 0.0, 10.0),
            (f64::NAN, f64::NAN, f64::NAN),
            (100.0, 0.0, 100.0),
            (110.0, 0.0, 110.0),
        ];
        let (distance, _) = pick_time(&points, 55.0, 0.0, None, None).unwrap();
        assert_eq!(
            distance,
            45.0 * 45.0,
            "the gap between patches is not an orbit segment"
        );
    }
}

//! Smoothness counters for the stream and the frame loop, published once a second as JSON on the page
//! (`data-ose-net` on `<body>`) for the netcode benchmark (`e2e/tests/netcode_smoothness.spec.ts`) and
//! for a look in the browser's inspector. Pure data; no DOM, so it is unit-tested natively.
//!
//! The figure that matters is the motion residual: each frame, how far the flown vessel is from where
//! the previous frame's position and velocity put it. Smooth motion has a residual near zero whatever the
//! latency; a clock jump, a snapshot that disagrees with the prediction, or a stalled frame shows up as
//! metres.

/// One second of samples, summarised on `report`.
#[derive(Clone, Debug, Default)]
pub struct NetStats {
    /// `performance.now()` of the last stream message, for arrival spacing.
    last_arrival_ms: Option<f64>,
    arrival_gaps_ms: Vec<f64>,
    /// How far the displayed clock moved at each `TimeState`, ms (0 while it slews instead of jumping).
    clock_steps_ms: Vec<f64>,
    /// How far the clock still has to slew after each `TimeState`, ms.
    clock_pending_ms: Vec<f64>,
    /// First and last (arrival ms, universal time / warp) of the window's `TimeState`s: the server's rate.
    time_first: Option<(f64, f64)>,
    time_last: Option<(f64, f64)>,
    /// Per frame, how far the flown vessel is from where the previous frame predicted it, m.
    residuals_m: Vec<f64>,
    /// The vessel, display time, position and velocity of the previous frame.
    last_motion: Option<(u64, f64, [f64; 3], [f64; 3])>,
    frame_gaps_ms: Vec<f64>,
    last_frame_ms: Option<f64>,
    /// Time spent decoding and applying each stream message, ms.
    handler_ms: Vec<f64>,
    stream_bytes: u64,
    stream_messages: u32,
    /// `performance.now()` the window started at.
    since_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quantiles {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

pub fn quantiles(samples: &[f64]) -> Quantiles {
    if samples.is_empty() {
        return Quantiles::default();
    }
    let mut sorted: Vec<f64> = samples.iter().copied().filter(|x| x.is_finite()).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    // nearest rank: the smallest sample at least q of the way through
    let at =
        |q: f64| sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1];
    Quantiles {
        n: sorted.len(),
        p50: at(0.5),
        p95: at(0.95),
        max: sorted[sorted.len() - 1],
    }
}

impl NetStats {
    /// A stream message (`TimeState`, `Snapshot`, `Vessels`, `Trajectory`) of `bytes` arrived at `now_ms`.
    pub fn arrival(&mut self, now_ms: f64, bytes: usize) {
        self.stream_bytes += bytes as u64;
        self.stream_messages += 1;
        if let Some(last) = self.last_arrival_ms {
            self.arrival_gaps_ms.push(now_ms - last);
        }
        self.last_arrival_ms = Some(now_ms);
    }

    pub fn handled(&mut self, elapsed_ms: f64) {
        self.handler_ms.push(elapsed_ms);
    }

    /// A `TimeState` saying `ut` at warp `warp` arrived at `now_ms`; the displayed clock moved by
    /// `step_ms` on it and has `pending_ms` of slew left.
    pub fn clock(&mut self, now_ms: f64, ut: f64, warp: u32, step_ms: f64, pending_ms: f64) {
        self.clock_steps_ms.push(step_ms);
        self.clock_pending_ms.push(pending_ms);
        let sample = (now_ms, ut / warp.max(1) as f64);
        self.time_first.get_or_insert(sample);
        self.time_last = Some(sample);
    }

    /// A frame at `now_ms` drew the flown vessel `motion` = (id, position m, velocity m/s) at the
    /// display's universal time `ut`. A different vessel, or none, starts the motion history afresh.
    pub fn frame(&mut self, now_ms: f64, ut: f64, motion: Option<(u64, [f64; 3], [f64; 3])>) {
        if let Some(last) = self.last_frame_ms {
            self.frame_gaps_ms.push(now_ms - last);
        }
        self.last_frame_ms = Some(now_ms);
        let Some((id, p, v)) = motion else {
            self.last_motion = None;
            return;
        };
        if let Some((id0, ut0, p0, v0)) = self.last_motion.filter(|m| m.0 == id) {
            let dt = ut - ut0;
            if (0.0..=1.0).contains(&dt) {
                let mut err = 0.0;
                for i in 0..3 {
                    let predicted = p0[i] + 0.5 * (v0[i] + v[i]) * dt;
                    err += (p[i] - predicted) * (p[i] - predicted);
                }
                self.residuals_m.push(err.sqrt());
            }
            let _ = id0;
        }
        self.last_motion = Some((id, ut, p, v));
    }

    /// The window's summary as JSON once a second has passed, clearing the window; `None` sooner.
    pub fn report(&mut self, now_ms: f64) -> Option<String> {
        let since = *self.since_ms.get_or_insert(now_ms);
        if now_ms - since < 1000.0 {
            return None;
        }
        let seconds = (now_ms - since) / 1000.0;
        let q = |v: &[f64]| quantiles(v);
        let arrivals = q(&self.arrival_gaps_ms);
        let clock = q(&self
            .clock_steps_ms
            .iter()
            .map(|x| x.abs())
            .collect::<Vec<_>>());
        let pending = q(&self
            .clock_pending_ms
            .iter()
            .map(|x| x.abs())
            .collect::<Vec<_>>());
        let residual = q(&self.residuals_m);
        let frames = q(&self.frame_gaps_ms);
        let handler = q(&self.handler_ms);
        let jumps = self.residuals_m.iter().filter(|r| **r > 0.5).count();
        // Universal time (at warp 1) the server advanced per wall second over the window: 1 when it keeps up.
        let ut_rate = match (self.time_first, self.time_last) {
            (Some(a), Some(b)) if b.0 - a.0 > 100.0 => (b.1 - a.1) * 1000.0 / (b.0 - a.0),
            _ => 0.0,
        };
        let stalls = self.frame_gaps_ms.iter().filter(|g| **g > 50.0).count();
        let json = format!(
            concat!(
                "{{\"seconds\":{:.2},\"streamKbps\":{:.1},\"streamMessages\":{},",
                "\"arrivalGapMs\":{},\"clockStepMs\":{},\"clockPendingMs\":{},",
                "\"utRate\":{:.3},\"residualM\":{},\"jumps\":{},\"frameGapMs\":{},\"stalls\":{},",
                "\"handlerMs\":{}}}"
            ),
            seconds,
            self.stream_bytes as f64 * 8.0 / 1000.0 / seconds,
            self.stream_messages,
            json_q(arrivals),
            json_q(clock),
            json_q(pending),
            ut_rate,
            json_q(residual),
            jumps,
            json_q(frames),
            stalls,
            json_q(handler),
        );
        self.arrival_gaps_ms.clear();
        self.clock_steps_ms.clear();
        self.clock_pending_ms.clear();
        self.time_first = None;
        self.time_last = None;
        self.residuals_m.clear();
        self.frame_gaps_ms.clear();
        self.handler_ms.clear();
        self.stream_bytes = 0;
        self.stream_messages = 0;
        self.since_ms = Some(now_ms);
        Some(json)
    }
}

fn json_q(q: Quantiles) -> String {
    format!(
        "{{\"n\":{},\"p50\":{:.3},\"p95\":{:.3},\"max\":{:.3}}}",
        q.n, q.p50, q.p95, q.max
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residual_is_zero_for_uniformly_accelerated_motion_and_catches_a_jump() {
        let mut s = NetStats::default();
        // x = 0.5 * 10 * t^2, v = 10 t: the trapezoid rule is exact for constant acceleration.
        for i in 0..10 {
            let t = i as f64 * 0.016;
            s.frame(
                t * 1000.0,
                100.0 + t,
                Some((7, [5.0 * t * t, 0.0, 0.0], [10.0 * t, 0.0, 0.0])),
            );
        }
        assert_eq!(s.residuals_m.len(), 9);
        assert!(
            s.residuals_m.iter().all(|r| *r < 1e-9),
            "{:?}",
            s.residuals_m
        );
        // the clock jumps 40 ms backward while the vessel does 100 m/s: a 4 m jump
        let t = 10.0 * 0.016 - 0.04;
        s.frame(
            160.0,
            100.0 + t,
            Some((7, [5.0 * t * t, 0.0, 0.0], [10.0 * t, 0.0, 0.0])),
        );
        assert_eq!(
            s.residuals_m.len(),
            9,
            "a frame that goes back in time is not a residual"
        );
        s.frame(
            176.0,
            100.0 + t + 0.016,
            Some((7, [5.0, 0.0, 0.0], [1.6, 0.0, 0.0])),
        );
        assert!(s.residuals_m.last().unwrap() > &4.0);
        // another vessel: no residual against the old one's track
        s.frame(
            192.0,
            100.0 + t + 0.032,
            Some((8, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0])),
        );
        assert_eq!(s.residuals_m.len(), 10);
    }

    #[test]
    fn report_summarises_a_second_and_clears() {
        let mut s = NetStats::default();
        assert!(s.report(0.0).is_none());
        s.arrival(0.0, 100);
        s.arrival(100.0, 300);
        s.arrival(230.0, 300);
        s.clock(100.0, 50.0, 1, 12.0, -3.0);
        s.clock(600.0, 50.4, 1, 0.0, 0.0);
        s.handled(0.5);
        s.frame(0.0, 1.0, None);
        s.frame(16.0, 1.016, None);
        s.frame(90.0, 1.09, None);
        assert!(s.report(500.0).is_none());
        let json = s.report(1000.0).unwrap();
        assert!(json.contains("\"streamMessages\":3"), "{json}");
        assert!(json.contains("\"streamKbps\":5.6"), "{json}");
        assert!(
            json.contains(
                "\"arrivalGapMs\":{\"n\":2,\"p50\":100.000,\"p95\":130.000,\"max\":130.000}"
            ),
            "{json}"
        );
        assert!(json.contains("\"stalls\":1"), "{json}");
        assert!(json.contains("\"utRate\":0.800"), "{json}");
        assert!(
            json.contains("\"clockPendingMs\":{\"n\":2,\"p50\":0.000"),
            "{json}"
        );
        assert!(s.report(1500.0).is_none());
        let json = s.report(2100.0).unwrap();
        assert!(json.contains("\"streamMessages\":0"), "{json}");
    }

    #[test]
    fn quantiles_pick_nearest_rank() {
        let q = quantiles(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!((q.n, q.p50, q.p95, q.max), (5, 3.0, 5.0, 5.0));
        assert_eq!(quantiles(&[]), Quantiles::default());
    }
}

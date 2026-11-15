//! Render cadence independent of browser refresh rate and flight input polling.

/// Keep a stall visible for ten seconds after cadence recovers, instead of hiding it in an FPS average.
#[derive(Default)]
pub struct FrameTimings {
    previous_ms: Option<f64>,
    window_ms: f64,
    pub worst_ms: f64,
}

impl FrameTimings {
    pub fn record(&mut self, now_ms: f64) {
        if now_ms - self.window_ms >= 10_000.0 {
            self.window_ms = now_ms;
            self.worst_ms = 0.0;
        }
        if let Some(previous) = self.previous_ms {
            self.worst_ms = self.worst_ms.max(now_ms - previous);
        }
        self.previous_ms = Some(now_ms);
    }

    pub fn pause(&mut self) {
        self.previous_ms = None;
    }

    pub fn worst_at(&self, now_ms: f64) -> f64 {
        self.worst_ms
            .max(self.previous_ms.map_or(0.0, |previous| now_ms - previous))
    }
}

pub struct FramePacer {
    limit: u32,
    next_ms: Option<f64>,
}

impl FramePacer {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            next_ms: None,
        }
    }

    pub fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
        self.next_ms = None;
    }

    pub fn should_render(&mut self, now_ms: f64) -> bool {
        if self.limit == 0 {
            return true;
        }
        let interval = 1000.0 / self.limit as f64;
        let Some(next) = self.next_ms else {
            self.next_ms = Some(now_ms + interval);
            return true;
        };
        // Browser timestamps have small rounding errors. Preserve the phase instead of rounding
        // the cap down to a divisor of the display rate (60 FPS must not become 50 on a 100 Hz screen).
        if now_ms + 0.1 < next {
            return false;
        }
        let elapsed = (now_ms - next).max(0.0);
        self.next_ms = Some(next + ((elapsed / interval).floor() + 1.0) * interval);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_stall_after_cadence_recovers_and_ignores_hidden_time() {
        let mut timing = FrameTimings::default();
        timing.record(0.0);
        timing.record(10.0);
        timing.record(2010.0);
        timing.record(2020.0);
        assert_eq!(timing.worst_ms, 2000.0);
        assert_eq!(timing.worst_at(5020.0), 3000.0);
        timing.pause();
        timing.record(50_000.0);
        timing.record(50_010.0);
        assert_eq!(timing.worst_ms, 10.0);
    }

    #[test]
    fn caps_without_rounding_down_to_a_display_divisor() {
        for refresh in [60, 100, 120, 144, 165, 240] {
            for cap in [30, 60, 120, 0] {
                let mut pacer = FramePacer::new(cap);
                let rendered = (0..refresh * 10)
                    .filter(|i| pacer.should_render(*i as f64 * 1000.0 / refresh as f64))
                    .count();
                let expected = if cap == 0 { refresh } else { cap.min(refresh) } * 10;
                assert!(
                    (rendered as i32 - expected as i32).abs() <= 1,
                    "refresh {refresh}, cap {cap}: {rendered} frames, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn resumes_without_catch_up_frames_and_changes_limits_immediately() {
        let mut pacer = FramePacer::new(60);
        assert!(pacer.should_render(0.0));
        assert!(!pacer.should_render(5.0));
        assert!(pacer.should_render(10_000.0));
        assert!(!pacer.should_render(10_001.0));
        pacer.set_limit(30);
        assert!(pacer.should_render(10_002.0));
        assert!(!pacer.should_render(10_022.0));
        pacer.set_limit(0);
        assert!(pacer.should_render(10_023.0));
        assert!(pacer.should_render(10_024.0));
    }
}

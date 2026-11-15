//! Bounded performance reports over HTTP; independent of the gameplay wire protocol.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub session: String,
    pub sequence: u32,
    pub build: String,
    pub protocol: u16,
    pub reason: Reason,
    pub scene: Scene,
    pub unix_ms: f64,
    pub vessel: Option<u64>,
    pub vessels: u32,
    pub warp: u32,
    pub webgpu: bool,
    pub width: u32,
    pub height: u32,
    pub heap_bytes: Option<f64>,
    pub websocket_buffered_bytes: u32,
    pub metrics: Metrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Periodic,
    Stall,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scene {
    Menu,
    Ksc,
    Editor,
    Flight,
    Map,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub window_ms: f64,
    pub frames: u32,
    pub frame_gap_max_ms: f64,
    pub callback_max_ms: f64,
    pub state_gap_max_ms: f64,
    pub state_age_ms: f64,
    pub message_max_ms: f64,
    pub message_bytes_max: u32,
    pub input_delay_max_ms: f64,
    pub frames_over_100_ms: u32,
    pub motion_residual_max_m: f64,
    pub clock_correction_max_ms: f64,
    pub server_rate_min: Option<f64>,
}

impl Report {
    pub fn valid(&self) -> bool {
        let hex = |s: &str, lo, hi| {
            (lo..=hi).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
        };
        let duration = |x: f64| x.is_finite() && (0.0..=86_400_000.0).contains(&x);
        hex(&self.session, 16, 32)
            && (self.build == "unknown" || hex(&self.build, 7, 40))
            && self.unix_ms.is_finite()
            && self.unix_ms >= 0.0
            && self.width <= 32768
            && self.height <= 32768
            && self
                .heap_bytes
                .is_none_or(|n| n.is_finite() && (0.0..=1e13).contains(&n))
            && [
                self.metrics.window_ms,
                self.metrics.frame_gap_max_ms,
                self.metrics.callback_max_ms,
                self.metrics.state_gap_max_ms,
                self.metrics.state_age_ms,
                self.metrics.message_max_ms,
                self.metrics.input_delay_max_ms,
            ]
            .into_iter()
            .all(duration)
            && self.metrics.motion_residual_max_m.is_finite()
            && self.metrics.motion_residual_max_m >= 0.0
            && duration(self.metrics.clock_correction_max_ms)
            && self
                .metrics
                .server_rate_min
                .is_none_or(|r| r.is_finite() && (0.0..=1e9).contains(&r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unbounded_names_and_nonfinite_timings() {
        let mut report = Report {
            session: "0123456789abcdef".into(),
            sequence: 1,
            build: "abcdef1234567".into(),
            protocol: crate::PROTOCOL_VERSION,
            reason: Reason::Stall,
            scene: Scene::Map,
            unix_ms: 1.0,
            vessel: Some(1),
            vessels: 2,
            warp: 1,
            webgpu: true,
            width: 1920,
            height: 1080,
            heap_bytes: None,
            websocket_buffered_bytes: 0,
            metrics: Metrics::default(),
        };
        assert!(report.valid());
        report.metrics.frame_gap_max_ms = f64::INFINITY;
        assert!(!report.valid());
        report.metrics.frame_gap_max_ms = 2000.0;
        report.session = "an email or log injection\n".into();
        assert!(!report.valid());
    }
}

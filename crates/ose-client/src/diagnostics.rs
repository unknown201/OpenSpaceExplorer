//! Constant-space collection. Reports at 30 s, on a stall (10 s cooldown), or on explicit request.
use ose_net::diagnostics::{Metrics, Reason};

pub fn send(
    report: ose_net::diagnostics::Report,
    token: String,
    doc: web_sys::Document,
    pending: std::rc::Rc<std::cell::Cell<bool>>,
) {
    use wasm_bindgen::JsCast;
    let Ok(body) = serde_json::to_string(&report) else {
        return;
    };
    if let Some(el) = doc.body() {
        el.set_attribute("data-ose-diagnostics", &body).ok();
    }
    pending.set(true);
    crate::set_text(&doc, "performance-report-status", "Sending report…");
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            let options = web_sys::RequestInit::new();
            options.set_method("POST");
            options.set_body(&wasm_bindgen::JsValue::from_str(&body));
            let request = web_sys::Request::new_with_str_and_init("/api/diagnostics", &options)?;
            request.headers().set("Content-Type", "application/json")?;
            request
                .headers()
                .set("Authorization", &format!("Bearer {token}"))?;
            let window =
                web_sys::window().ok_or_else(|| wasm_bindgen::JsValue::from_str("no window"))?;
            let response =
                wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
                    .await?
                    .dyn_into::<web_sys::Response>()?;
            if !response.ok() {
                return Err(wasm_bindgen::JsValue::from_str("report refused"));
            }
            let ack = wasm_bindgen_futures::JsFuture::from(response.json()?).await?;
            let id = js_sys::Reflect::get(&ack, &wasm_bindgen::JsValue::from_str("report_id"))?
                .as_string();
            if id.as_deref() != Some(format!("{}-{}", report.session, report.sequence).as_str()) {
                return Err(wasm_bindgen::JsValue::from_str(
                    "invalid report acknowledgment",
                ));
            }
            Ok::<(), wasm_bindgen::JsValue>(())
        }
        .await;
        pending.set(false);
        if result.is_ok() {
            let id = format!("{}-{}", report.session, report.sequence);
            crate::set_text(
                &doc,
                "performance-report-status",
                &format!("Report sent: {id}"),
            );
        } else {
            crate::set_text(
                &doc,
                "performance-report-status",
                "Could not send report. Please try again.",
            );
        }
    });
}

#[derive(Default)]
pub struct Sampler {
    metrics: Metrics,
    since_ms: Option<f64>,
    last_frame: Option<f64>,
    last_state: Option<f64>,
    last_sent: Option<f64>,
    pub sequence: u32,
}

impl Sampler {
    pub fn frame(&mut self, now: f64) {
        self.since_ms.get_or_insert(now);
        self.metrics.frames = self.metrics.frames.saturating_add(1);
        if let Some(last) = self.last_frame.replace(now) {
            let gap = (now - last).max(0.0);
            self.metrics.frame_gap_max_ms = self.metrics.frame_gap_max_ms.max(gap);
            self.metrics.frames_over_100_ms += u32::from(gap > 100.0);
        }
    }
    pub fn state(&mut self, now: f64, visible: bool) {
        if !visible {
            self.last_state = None;
            return;
        }
        if let Some(last) = self.last_state.replace(now) {
            self.metrics.state_gap_max_ms = self.metrics.state_gap_max_ms.max(now - last);
        }
    }
    pub fn callback(&mut self, duration: f64, visible: bool) {
        if !visible {
            self.pause();
            return;
        }
        self.metrics.callback_max_ms = self.metrics.callback_max_ms.max(duration);
    }
    pub fn message(&mut self, duration: f64, bytes: usize) {
        self.metrics.message_max_ms = self.metrics.message_max_ms.max(duration);
        self.metrics.message_bytes_max = self
            .metrics
            .message_bytes_max
            .max(bytes.min(u32::MAX as usize) as u32);
    }
    pub fn input(&mut self, delay: f64) {
        if delay.is_finite() && (0.0..86_400_000.0).contains(&delay) {
            self.metrics.input_delay_max_ms = self.metrics.input_delay_max_ms.max(delay);
        }
    }
    pub fn network_summary(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        self.metrics.motion_residual_max_m = self
            .metrics
            .motion_residual_max_m
            .max(v["residualM"]["max"].as_f64().unwrap_or(0.0));
        self.metrics.clock_correction_max_ms = self
            .metrics
            .clock_correction_max_ms
            .max(v["clockStepMs"]["max"].as_f64().unwrap_or(0.0));
        if let Some(rate) = v["utRate"].as_f64().filter(|r| r.is_finite() && *r >= 0.0) {
            self.metrics.server_rate_min =
                Some(self.metrics.server_rate_min.map_or(rate, |r| r.min(rate)));
        }
    }
    pub fn pause(&mut self) {
        self.last_frame = None;
        self.last_state = None;
        self.metrics = Metrics::default();
        self.since_ms = None;
    }
    pub fn due(&self, now: f64) -> Option<Reason> {
        let age = self.last_state.map(|last| now - last)?;
        let since = now - self.since_ms?;
        let stalled = self.metrics.frame_gap_max_ms > 250.0
            || self.metrics.callback_max_ms > 100.0
            || self.metrics.message_max_ms > 100.0
            || self.metrics.input_delay_max_ms > 100.0
            || age > 1000.0
            || self.metrics.state_gap_max_ms > 1000.0;
        if stalled && self.last_sent.is_none_or(|last| now - last >= 10_000.0) {
            Some(Reason::Stall)
        } else if since >= 30_000.0 {
            Some(Reason::Periodic)
        } else {
            None
        }
    }
    pub fn take(&mut self, now: f64) -> Metrics {
        let mut metrics = std::mem::take(&mut self.metrics);
        metrics.window_ms = now - self.since_ms.replace(now).unwrap_or(now);
        metrics.state_age_ms = self.last_state.map_or(0.0, |last| now - last);
        self.last_sent = Some(now);
        self.sequence = self.sequence.wrapping_add(1);
        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_browser_and_state_stalls_with_cooldown_and_resets_the_window() {
        let mut s = Sampler::default();
        s.frame(0.0);
        s.state(0.0, true);
        s.frame(2000.0);
        assert_eq!(s.due(2000.0), Some(Reason::Stall));
        let report = s.take(2000.0);
        assert_eq!(report.frames_over_100_ms, 1);
        assert_eq!(report.state_age_ms, 2000.0);
        assert_eq!(s.due(3000.0), None);
        assert_eq!(s.due(12000.0), Some(Reason::Stall));
        s.pause();
        assert_eq!(s.due(50_000.0), None);
        s.frame(50_000.0);
        s.state(50_000.0, true);
        assert_eq!(s.take(50_000.0).frame_gap_max_ms, 0.0);
    }
    #[test]
    fn healthy_windows_report_periodically_and_keep_message_and_input_evidence() {
        let mut s = Sampler::default();
        for t in 0..=300 {
            let now = t as f64 * 100.0;
            s.frame(now);
            s.state(now, true);
        }
        s.message(12.0, 8192);
        s.input(30.0);
        assert_eq!(s.due(30_000.0), Some(Reason::Periodic));
        let report = s.take(30_000.0);
        assert_eq!(report.message_bytes_max, 8192);
        assert_eq!(report.input_delay_max_ms, 30.0);
    }
}

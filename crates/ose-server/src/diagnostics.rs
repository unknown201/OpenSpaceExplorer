//! Rate-limited performance evidence, without simulation locks or account credentials.
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use ose_net::diagnostics::Report;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Default, serde::Serialize)]
struct Phase {
    count: u64,
    total_ms: f64,
    max_ms: f64,
    slow: u64,
    #[serde(skip)]
    warned: Option<Instant>,
}
#[derive(Default)]
struct Window {
    since: Option<Instant>,
    phases: HashMap<&'static str, Phase>,
}
#[derive(Default)]
pub struct Timings(Mutex<Window>);
impl Timings {
    pub fn record(&self, room: &crate::Shared, phase: &'static str, ms: f64, threshold: f64) {
        let now = Instant::now();
        let mut window = self.0.lock().expect("timings lock");
        let since = *window.since.get_or_insert(now);
        let p = window.phases.entry(phase).or_default();
        p.count += 1;
        p.total_ms += ms;
        p.max_ms = p.max_ms.max(ms);
        if ms >= threshold {
            p.slow += 1;
            if p.warned
                .is_none_or(|last| now.duration_since(last).as_secs() >= 5)
            {
                p.warned = Some(now);
                let campaign = room.campaign.try_lock().ok().map(|c| c.id.clone());
                tracing::warn!(phase, elapsed_ms = ms, room = ?campaign,
                    build = ose_net::BUILD_REVISION, "server performance stall");
            }
        }
        if now.duration_since(since).as_secs() >= 30 {
            let campaign = room.campaign.try_lock().ok().map(|c| c.id.clone());
            tracing::info!(room = ?campaign, window_ms = now.duration_since(since).as_millis() as u64,
                phases = %serde_json::to_string(&window.phases).unwrap_or_default(), "server performance summary");
            window.since = Some(now);
            for p in window.phases.values_mut() {
                p.count = 0;
                p.total_ms = 0.0;
                p.max_ms = 0.0;
                p.slow = 0;
            }
        }
    }
}
pub struct Scope<'a> {
    room: &'a crate::Shared,
    phase: &'static str,
    threshold: f64,
    started: Instant,
}
pub fn measure<'a>(room: &'a crate::Shared, phase: &'static str, threshold: f64) -> Scope<'a> {
    Scope {
        room,
        phase,
        threshold,
        started: Instant::now(),
    }
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        self.room.diagnostics.record(
            self.room,
            self.phase,
            self.started.elapsed().as_secs_f64() * 1000.0,
            self.threshold,
        );
    }
}

#[derive(Default)]
struct Limits(HashMap<u64, (Instant, u32)>);
impl Limits {
    fn admit(&mut self, player: u64, now: Instant) -> bool {
        self.0
            .retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        if !self.0.contains_key(&player) && self.0.len() >= 4096 {
            return false;
        }
        let entry = self.0.entry(player).or_insert((now, 0));
        if entry.1 >= 12 {
            return false;
        }
        entry.1 += 1;
        true
    }
}
pub fn websocket_write(player: u64, elapsed_ms: f64, ok: bool) {
    if ok && elapsed_ms < 100.0 {
        return;
    }
    static LIMITS: OnceLock<Mutex<Limits>> = OnceLock::new();
    if LIMITS
        .get_or_init(Default::default)
        .lock()
        .expect("write warning limits")
        .admit(player, Instant::now())
    {
        tracing::warn!(
            player,
            elapsed_ms,
            ok,
            "websocket publication slow or failed"
        );
    }
}
pub async fn report(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
    Json(report): Json<Report>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|s| s.len() <= 256)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let player = s
        .accounts
        .lock()
        .expect("accounts lock")
        .identify(token)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !report.valid() {
        return Err(StatusCode::BAD_REQUEST);
    }
    static LIMITS: OnceLock<Mutex<Limits>> = OnceLock::new();
    if !LIMITS
        .get_or_init(Default::default)
        .lock()
        .expect("report limits lock")
        .admit(player, Instant::now())
    {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let room = crate::rooms::room_of(&s, player);
    let campaign = room.campaign.try_lock().ok().map(|c| c.id.clone());
    let id = format!("{}-{}", report.session, report.sequence);
    tracing::info!(report_id = %id, player, room = ?campaign, server_build = ose_net::BUILD_REVISION,
        server_protocol = ose_net::PROTOCOL_VERSION, report = %serde_json::to_string(&report).unwrap_or_default(),
        "client performance report");
    Ok(Json(
        serde_json::json!({"report_id": id, "server_build": ose_net::BUILD_REVISION}),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_are_per_player_expire_and_bound_cardinality() {
        let mut limits = Limits::default();
        let now = Instant::now();
        for _ in 0..12 {
            assert!(limits.admit(1, now));
        }
        assert!(!limits.admit(1, now));
        assert!(limits.admit(2, now));
        assert!(limits.admit(1, now + Duration::from_secs(60)));
        for i in 2..=4096 {
            assert!(limits.admit(i, now + Duration::from_secs(60)));
        }
        assert!(!limits.admit(4097, now + Duration::from_secs(60)));
    }
}

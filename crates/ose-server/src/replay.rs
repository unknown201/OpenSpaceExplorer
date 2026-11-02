//! Replays: a recording of the world and of what the players sent, to file with a bug report and to run again
//! headlessly (`--replay <file>`), as a regression fixture under `tests/replays/`.
//!
//! The server keeps rolling windows of `WINDOW_STEPS` simulation steps. Each opens with a baseline (the save
//! file of the world then, the craft library and the revert point) and logs every client message with the
//! step count it arrived after, plus a checkpoint of every vessel once a second. Filing (POST /api/replays)
//! writes the last two windows (at least one window of history) to `replays/<id>.json` beside the save.
//!
//! Messages are applied between steps under the world's locks, so a replay that loads the baseline, applies
//! each message after the same number of steps (`crate::handle_decoded`, the code a live session runs) and
//! steps the world (`crate::step_once`) reproduces the flight; the checkpoints show where it diverges if the
//! code has changed since. A fixture's `expect` list states what must hold at the end (the fixed behaviour).

use std::sync::atomic::Ordering;

use ose_net::{ClientMessage, ServerMessage};
use serde::{Deserialize, Serialize};

use crate::{save, Shared};

pub const REPLAY_VERSION: u32 = 1;
/// Five minutes of steps per window; a filed replay holds between one and two windows.
pub const WINDOW_STEPS: u64 = 5 * 60 * ose_sim::TICKS_PER_SECOND;

/// What a player did: sent a message, or connected (Hello) or disconnected.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Event {
    Msg(ClientMessage),
    Joined { name: String },
    Left,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    /// Steps taken before it was handled (the server's step counter).
    pub step: u64,
    pub tick: u64,
    pub player: u64,
    pub event: Event,
}

/// Who was connected and playing when a window opened (not in the save file).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sessions {
    /// (player, display name, open connections)
    pub connected: Vec<(u64, String, usize)>,
    pub playing: Vec<u64>,
    pub warp: Vec<(u64, u32)>,
    pub flying: Vec<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VesselState {
    pub id: u64,
    pub name: String,
    pub owner: Option<u64>,
    /// Body-centred position and velocity, m and m/s.
    pub r: [f64; 3],
    pub v: [f64; 3],
    pub stage: Option<usize>,
    pub propellant_t: Option<f64>,
    pub landed: Option<bool>,
    pub crashed: Option<bool>,
    pub parts: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub step: u64,
    pub tick: u64,
    pub vessels: Vec<VesselState>,
}

/// The revert point a window opened with (`campaigns::RevertPoint`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revert {
    pub player: u64,
    pub editor: save::SaveFile,
    pub launch: save::SaveFile,
    pub craft: ose_data::craft::Craft,
}

/// One thing that must hold when a replay ends: `field` of the vessel named `vessel` compared with `value`
/// (`op`: eq, ne, lt, le, gt, ge). Fields: stage, propellant_t, parts, landed, crashed, altitude_m, speed_mps,
/// count (vessels with that name, 0 when none).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Expect {
    pub vessel: String,
    pub field: String,
    pub op: String,
    pub value: f64,
    #[serde(default)]
    pub why: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayFile {
    pub version: u32,
    pub protocol: u32,
    pub note: String,
    #[serde(default)]
    pub filed_by: String,
    pub filed_unix: u64,
    pub start_step: u64,
    pub baseline: save::SaveFile,
    #[serde(default)]
    pub revert: Option<Revert>,
    #[serde(default)]
    pub sessions: Sessions,
    pub inputs: Vec<Input>,
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub expect: Vec<Expect>,
}

struct Window {
    start_step: u64,
    baseline: save::SaveFile,
    revert: Option<Revert>,
    sessions: Sessions,
    inputs: Vec<Input>,
    checkpoints: Vec<Checkpoint>,
}

#[derive(Default)]
pub struct Recorder {
    current: Option<Window>,
    previous: Option<Window>,
    /// Off while a replay runs (it must not record itself).
    pub disabled: bool,
}

/// Messages that change nothing in the world are not kept.
fn recordable(event: &Event) -> bool {
    let Event::Msg(msg) = event else {
        return true;
    };
    !matches!(
        msg,
        ClientMessage::Ping { .. }
            | ClientMessage::Hello { .. }
            | ClientMessage::CraftList
            | ClientMessage::GetCraft { .. }
            | ClientMessage::KerbalRoster
            | ClientMessage::ScienceInfo
    )
}

fn sessions(s: &Shared) -> Sessions {
    let names = s.names.lock().expect("names lock").clone();
    let mut connected: Vec<(u64, String, usize)> = s
        .connections
        .lock()
        .expect("connections lock")
        .iter()
        .map(|(&id, &n)| (id, names.get(&id).cloned().unwrap_or_default(), n))
        .collect();
    connected.sort();
    let mut playing: Vec<u64> = s
        .playing
        .lock()
        .expect("playing lock")
        .iter()
        .copied()
        .collect();
    playing.sort();
    let mut warp: Vec<(u64, u32)> = s
        .warp_requests
        .lock()
        .expect("warp lock")
        .iter()
        .map(|(&a, &b)| (a, b))
        .collect();
    warp.sort();
    let mut flying: Vec<(u64, u64)> = s
        .flying
        .lock()
        .expect("flying lock")
        .iter()
        .map(|(&a, &b)| (a, b))
        .collect();
    flying.sort();
    Sessions {
        connected,
        playing,
        warp,
        flying,
    }
}

fn baseline(s: &Shared) -> (save::SaveFile, Option<Revert>) {
    let crafts = s.crafts.lock().expect("crafts lock").clone();
    let file = {
        let u = s.universe.lock().expect("sim lock");
        let v = s.vessels.lock().expect("vessels lock");
        let c = s.campaign.lock().expect("campaign lock");
        save::capture(&u, &v, &c, &crafts)
    };
    let revert = s
        .revert
        .lock()
        .expect("revert lock")
        .as_ref()
        .map(|r| Revert {
            player: r.player,
            editor: r.editor.clone(),
            launch: r.launch.clone(),
            craft: r.craft.clone(),
        });
    (file, revert)
}

/// Open a new window (its baseline is the world now), keeping the last one.
pub fn begin(s: &Shared) {
    if s.recorder.lock().expect("recorder lock").disabled {
        return;
    }
    let (file, revert) = baseline(s);
    let sessions = sessions(s);
    let step = s.steps.load(Ordering::SeqCst);
    let mut r = s.recorder.lock().expect("recorder lock");
    r.previous = r.current.take();
    r.current = Some(Window {
        start_step: step,
        baseline: file,
        revert,
        sessions,
        inputs: Vec::new(),
        checkpoints: Vec::new(),
    });
}

/// Something a player did, about to take effect.
pub fn record_event(s: &Shared, player: u64, event: Event) {
    if !recordable(&event) {
        return;
    }
    let tick = s.universe.lock().expect("sim lock").tick().0;
    let step = s.steps.load(Ordering::SeqCst);
    if let Some(w) = s.recorder.lock().expect("recorder lock").current.as_mut() {
        w.inputs.push(Input {
            step,
            tick,
            player,
            event,
        });
    }
}

pub fn vessel_states(s: &Shared) -> Vec<VesselState> {
    let u = s.universe.lock().expect("sim lock");
    let list = s.vessels.lock().expect("vessels lock").clone();
    list.iter()
        .filter_map(|&(e, id, owner)| {
            let r = u.position_of(e)?;
            let (name, v) = match u.vessel(e) {
                Some((vessel, _, orbit)) => (vessel.name.clone(), orbit.state_at(u.ut()).1),
                None => (String::new(), Default::default()),
            };
            let p = u.powered(e);
            let v = p.map(|p| p.v).unwrap_or(v);
            Some(VesselState {
                id,
                name,
                owner,
                r: [r.x, r.y, r.z],
                v: [v.x, v.y, v.z],
                stage: p.map(|p| p.stage),
                // what the attached tanks hold, t
                propellant_t: u.structure(e).map(|st| {
                    st.net
                        .tanks
                        .iter()
                        .filter(|t| st.net.present.get(t.part).copied().unwrap_or(false))
                        .map(|t| t.stored * st.net.unit_mass_t[t.resource])
                        .sum()
                }),
                landed: p.map(|p| p.landed),
                crashed: p.map(|p| p.crashed),
                parts: u
                    .structure(e)
                    .map(|st| st.rigid.present.iter().filter(|&&x| x).count()),
            })
        })
        .collect()
}

/// After each live step: a checkpoint once a second, a new window every `WINDOW_STEPS`.
pub fn after_step(s: &Shared, step: u64) {
    if s.recorder.lock().expect("recorder lock").disabled {
        return;
    }
    if step.is_multiple_of(ose_sim::TICKS_PER_SECOND) {
        let vessels = vessel_states(s);
        let tick = s.universe.lock().expect("sim lock").tick().0;
        if let Some(w) = s.recorder.lock().expect("recorder lock").current.as_mut() {
            w.checkpoints.push(Checkpoint {
                step,
                tick,
                vessels,
            });
        }
    }
    let start = s
        .recorder
        .lock()
        .expect("recorder lock")
        .current
        .as_ref()
        .map(|w| w.start_step);
    if start.is_none_or(|st| step >= st + WINDOW_STEPS) {
        begin(s);
    }
}

/// The last windows as a replay to file.
pub fn file(s: &Shared, note: &str, filed_by: &str) -> Option<ReplayFile> {
    let r = s.recorder.lock().expect("recorder lock");
    let cur = r.current.as_ref()?;
    let first = r.previous.as_ref().unwrap_or(cur);
    let mut inputs = Vec::new();
    let mut checkpoints = Vec::new();
    for w in r.previous.iter().chain(std::iter::once(cur)) {
        inputs.extend(w.inputs.iter().cloned());
        checkpoints.extend(w.checkpoints.iter().cloned());
    }
    Some(ReplayFile {
        version: REPLAY_VERSION,
        protocol: u32::from(ose_net::PROTOCOL_VERSION),
        note: note.to_string(),
        filed_by: filed_by.to_string(),
        filed_unix: crate::now_unix(),
        start_step: first.start_step,
        baseline: first.baseline.clone(),
        revert: first.revert.clone(),
        sessions: first.sessions.clone(),
        inputs,
        checkpoints,
        expect: Vec::new(),
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Divergence {
    pub step: u64,
    pub vessel: u64,
    pub name: String,
    pub what: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub note: String,
    pub inputs: usize,
    pub steps: u64,
    /// Largest distance between a vessel's replayed and recorded position at a checkpoint, m.
    pub max_position_error_m: f64,
    /// The first checkpoint mismatches (stage, parts, a vessel missing or extra, position off by > 1 m).
    pub divergences: Vec<Divergence>,
    pub expectations_failed: Vec<String>,
    pub final_vessels: Vec<VesselState>,
    /// Replies the server sent that refused something (a reason), for reading the run.
    pub refusals: Vec<String>,
    pub ok: bool,
}

fn reason_of(m: &ServerMessage) -> Option<String> {
    let v = serde_json::to_value(m).ok()?;
    let (kind, body) = v.as_object()?.iter().next()?;
    let reason = body.get("reason")?.as_str()?;
    Some(format!("{kind}: {reason}"))
}

fn check(e: &Expect, vessels: &[VesselState], bodies_radius: f64) -> Result<(), String> {
    let named: Vec<&VesselState> = vessels.iter().filter(|v| v.name == e.vessel).collect();
    let got = if e.field == "count" {
        Some(named.len() as f64)
    } else {
        named.first().and_then(|v| {
            let len = |a: [f64; 3]| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            match e.field.as_str() {
                "stage" => v.stage.map(|x| x as f64),
                "propellant_t" => v.propellant_t,
                "parts" => v.parts.map(|x| x as f64),
                "landed" => v.landed.map(|b| f64::from(u8::from(b))),
                "crashed" => v.crashed.map(|b| f64::from(u8::from(b))),
                "altitude_m" => Some(len(v.r) - bodies_radius),
                "speed_mps" => Some(len(v.v)),
                _ => None,
            }
        })
    };
    let Some(got) = got else {
        return Err(format!("{} {}: no such vessel or field", e.vessel, e.field));
    };
    let ok = match e.op.as_str() {
        "eq" => (got - e.value).abs() < 1e-6,
        "ne" => (got - e.value).abs() >= 1e-6,
        "lt" => got < e.value,
        "le" => got <= e.value,
        "gt" => got > e.value,
        "ge" => got >= e.value,
        op => return Err(format!("unknown op {op}")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{} {} {} {}: got {got}{}",
            e.vessel,
            e.field,
            e.op,
            e.value,
            if e.why.is_empty() {
                String::new()
            } else {
                format!(" ({})", e.why)
            }
        ))
    }
}

/// Run `rep` on a world built from its baseline (the server's start-up, with the baseline as the save).
pub fn run(s: &Shared, rep: &ReplayFile, home_radius_m: f64) -> Report {
    s.recorder.lock().expect("recorder lock").disabled = true;
    if let Some(r) = &rep.revert {
        *s.revert.lock().expect("revert lock") = Some(crate::campaigns::RevertPoint {
            player: r.player,
            editor: r.editor.clone(),
            launch: r.launch.clone(),
            craft: r.craft.clone(),
        });
    }
    s.steps.store(rep.start_step, Ordering::SeqCst);
    for (id, name, n) in &rep.sessions.connected {
        s.connections
            .lock()
            .expect("connections lock")
            .insert(*id, *n);
        s.names
            .lock()
            .expect("names lock")
            .insert(*id, name.clone());
    }
    s.playing
        .lock()
        .expect("playing lock")
        .extend(rep.sessions.playing.iter().copied());
    s.warp_requests
        .lock()
        .expect("warp lock")
        .extend(rep.sessions.warp.iter().copied());
    s.flying
        .lock()
        .expect("flying lock")
        .extend(rep.sessions.flying.iter().copied());
    crate::apply_warp(s);
    let end = rep
        .inputs
        .iter()
        .map(|i| i.step)
        .chain(rep.checkpoints.iter().map(|c| c.step))
        .max()
        .unwrap_or(rep.start_step);
    let mut report = Report {
        note: rep.note.clone(),
        inputs: rep.inputs.len(),
        ..Default::default()
    };
    let mut inputs = rep.inputs.iter().peekable();
    let mut checkpoints = rep.checkpoints.iter().peekable();
    loop {
        let step = s.steps.load(Ordering::SeqCst);
        // a live checkpoint is taken at the end of its step, before the messages that came after it
        while let Some(c) = checkpoints.next_if(|c| c.step <= step) {
            if c.step < step {
                continue;
            }
            let now = vessel_states(s);
            for want in &c.vessels {
                let Some(got) = now.iter().find(|v| v.id == want.id) else {
                    report.divergences.push(Divergence {
                        step,
                        vessel: want.id,
                        name: want.name.clone(),
                        what: "missing".into(),
                    });
                    continue;
                };
                let d = ((got.r[0] - want.r[0]).powi(2)
                    + (got.r[1] - want.r[1]).powi(2)
                    + (got.r[2] - want.r[2]).powi(2))
                .sqrt();
                report.max_position_error_m = report.max_position_error_m.max(d);
                let mut what = Vec::new();
                if d > 1.0 {
                    what.push(format!("{d:.1} m off"));
                }
                if got.stage != want.stage {
                    what.push(format!("stage {:?} vs {:?}", got.stage, want.stage));
                }
                if got.parts != want.parts {
                    what.push(format!("parts {:?} vs {:?}", got.parts, want.parts));
                }
                if !what.is_empty() {
                    report.divergences.push(Divergence {
                        step,
                        vessel: want.id,
                        name: want.name.clone(),
                        what: what.join(", "),
                    });
                }
            }
            for extra in now
                .iter()
                .filter(|v| !c.vessels.iter().any(|w| w.id == v.id))
            {
                report.divergences.push(Divergence {
                    step,
                    vessel: extra.id,
                    name: extra.name.clone(),
                    what: "extra".into(),
                });
            }
        }
        while let Some(i) = inputs.next_if(|i| i.step <= step) {
            match &i.event {
                Event::Msg(msg) => {
                    let mut out = Vec::new();
                    crate::handle_decoded(s, i.player, Ok(msg.clone()), &mut out);
                    report.refusals.extend(out.iter().filter_map(reason_of));
                }
                Event::Joined { name } => crate::player_joined(s, i.player, name),
                Event::Left => crate::player_left(s, i.player),
            }
        }
        if step >= end {
            break;
        }
        crate::step_once(s);
    }
    report.divergences.truncate(50);
    report.steps = s.steps.load(Ordering::SeqCst) - rep.start_step;
    report.final_vessels = vessel_states(s);
    report.expectations_failed = rep
        .expect
        .iter()
        .filter_map(|e| check(e, &report.final_vessels, home_radius_m).err())
        .collect();
    report.ok = report.expectations_failed.is_empty();
    report
}

pub fn read(path: &std::path::Path) -> anyhow::Result<ReplayFile> {
    let text = std::fs::read_to_string(path)?;
    let rep: ReplayFile = serde_json::from_str(&text)?;
    anyhow::ensure!(
        rep.version == REPLAY_VERSION,
        "replay version {} not supported (server reads {REPLAY_VERSION})",
        rep.version
    );
    Ok(rep)
}

/// Where filed replays are kept: `replays/` beside the save (the server's working directory).
fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from("replays")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Deserialize)]
pub struct FileRequest {
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub by: String,
}

#[derive(Serialize)]
struct Listed {
    id: String,
    note: String,
    filed_by: String,
    filed_unix: u64,
    inputs: usize,
    seconds: u64,
}

/// POST /api/replays {note, by}: write the last windows to `replays/<id>.json`; answers {id}.
pub async fn file_route(
    axum::extract::State(s): axum::extract::State<crate::AppState>,
    axum::Json(req): axum::Json<FileRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let note: String = req.note.chars().take(4000).collect();
    let by: String = req.by.chars().take(64).collect();
    let Some(rep) = file(&s, &note, &by) else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "nothing recorded yet",
        )
            .into_response();
    };
    let id = format!("{}-{}", rep.filed_unix, rep.inputs.len());
    let write = || -> anyhow::Result<()> {
        std::fs::create_dir_all(dir())?;
        std::fs::write(dir().join(format!("{id}.json")), serde_json::to_vec(&rep)?)?;
        Ok(())
    };
    match write() {
        Ok(()) => {
            tracing::info!(id = %id, by = %by, inputs = rep.inputs.len(), "replay filed");
            axum::Json(serde_json::json!({ "id": id })).into_response()
        }
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/replays: what has been filed, newest first.
pub async fn list_route() -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir()) {
        for e in entries.flatten() {
            let path = e.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Ok(rep) = read(&path) else { continue };
            let seconds = rep
                .checkpoints
                .last()
                .map(|c| (c.step.saturating_sub(rep.start_step)) / ose_sim::TICKS_PER_SECOND)
                .unwrap_or(0);
            out.push(Listed {
                id,
                note: rep.note,
                filed_by: rep.filed_by,
                filed_unix: rep.filed_unix,
                inputs: rep.inputs.len(),
                seconds,
            });
        }
    }
    out.sort_by_key(|l| std::cmp::Reverse(l.filed_unix));
    axum::Json(out).into_response()
}

/// GET /api/replays/{id}: the replay file.
pub async fn get_route(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !valid_id(&id) {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }
    match std::fs::read(dir().join(format!("{id}.json"))) {
        Ok(bytes) => (
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (axum::http::header::CONTENT_DISPOSITION, "attachment"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

//! Delivery routes on the server (`docs/delivery-routes-design.md`, step 1): depot flags and proving flights.
//! The state is `ose_routes::Routes`, kept in the campaign (`save::Campaign::routes`), so the save, the
//! quicksave and revert carry it. The recorder follows each flight every simulation step.

use bevy_ecs::entity::Entity;
use ose_net::{ClientMessage, DepotInfo, RecordingInfo, ServerMessage};
use ose_routes::recording::State;

use crate::{owned_vessel, Shared};

/// The campaign's depots and recordings, with `reason` for a refusal.
pub fn info(s: &Shared, reason: Option<String>) -> ServerMessage {
    // locks in the order the snapshot takes them
    let _u = s.universe.lock().expect("sim lock");
    let list = s.vessels.lock().expect("vessels lock");
    let c = s.campaign.lock().expect("campaign lock");
    let id_of = |e: Entity| list.iter().find(|v| v.0 == e).map(|v| v.1);
    let depots = c
        .routes
        .depots
        .iter()
        .map(|d| DepotInfo {
            id: d.id,
            name: d.name.clone(),
            owner: d.owner,
            vessel: d.vessel().and_then(id_of),
        })
        .collect();
    let recordings = c
        .routes
        .provings
        .iter()
        .map(|p| {
            let r = &p.recorder.recording;
            let (state, dropped) = match &r.state {
                State::Recording => ("Recording", None),
                State::RoundTrip => ("RoundTrip", None),
                State::OneWay => ("OneWay", None),
                State::Dropped(why) => ("Dropped", Some(why.clone())),
            };
            RecordingInfo {
                id: p.id,
                player: p.player,
                craft: r.craft.name.clone(),
                vessel: p
                    .recorder
                    .is_recording()
                    .then(|| id_of(p.recorder.vessel()))
                    .flatten(),
                phase: format!("{:?}", p.recorder.phase()),
                state: state.into(),
                dropped,
                depot: r.depot.clone(),
                duration_s: r.duration_s(),
                used: ose_routes::Routes::used(p).into_iter().collect(),
                delivered: r.delivered.clone().into_iter().collect(),
                payload_parts: r.payload.len() as u32,
                burns: r
                    .events()
                    .filter(|e| matches!(e, ose_routes::recording::Event::BurnStart))
                    .count() as u32,
            }
        })
        .collect();
    ServerMessage::Routes {
        depots,
        recordings,
        reason,
    }
}

/// Answer one of the routes messages.
pub fn handle(s: &Shared, player: u64, msg: &ClientMessage) -> ServerMessage {
    let result: Result<(), String> = match *msg {
        ClientMessage::SetDepot { vessel, depot } => match owned_vessel(s, vessel, player) {
            None => Err("not your vessel".into()),
            Some(e) => {
                let u = s.universe.lock().expect("sim lock");
                let mut c = s.campaign.lock().expect("campaign lock");
                c.routes.set_depot(&u, e, player, depot).map(|changed| {
                    if changed {
                        tracing::info!(player, vessel, depot, "depot flag");
                    }
                })
            }
        },
        ClientMessage::RecordRoute { vessel } => match owned_vessel(s, vessel, player) {
            None => Err("not your vessel".into()),
            Some(e) => {
                let u = s.universe.lock().expect("sim lock");
                let mut c = s.campaign.lock().expect("campaign lock");
                let started = c.routes.start(&u, e, player);
                if let Ok(id) = started {
                    tracing::info!(player, vessel, recording = id, "recording a route");
                    // revert to launch comes back to the pad with the recording just started
                    if let Some(r) = s
                        .revert
                        .lock()
                        .expect("revert lock")
                        .as_mut()
                        .filter(|r| r.player == player)
                    {
                        r.launch.campaign.routes = c.routes.clone();
                    }
                }
                started.map(|_| ())
            }
        },
        ClientMessage::StopRecording => {
            let mut u = s.universe.lock().expect("sim lock");
            let mut c = s.campaign.lock().expect("campaign lock");
            c.routes.finish(&mut u, player).map(|id| {
                tracing::info!(player, recording = id, "recording stopped");
            })
        }
        ClientMessage::ForgetRecording { id } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            c.routes.forget(player, id)
        }
        _ => Ok(()),
    };
    info(s, result.err())
}

/// After each simulation step: follow the flights being recorded, and tell their players what moved on.
pub fn after_step(s: &Shared) {
    let changes = {
        let mut u = s.universe.lock().expect("sim lock");
        let mut c = s.campaign.lock().expect("campaign lock");
        c.routes.tick(&mut u)
    };
    if changes.is_empty() {
        return;
    }
    let msg = info(s, None);
    let mut players: Vec<u64> = changes.iter().map(|c| c.player).collect();
    players.dedup();
    let mut pending = s.pending.lock().expect("pending lock");
    for p in players {
        pending.push((p, msg.clone()));
    }
}

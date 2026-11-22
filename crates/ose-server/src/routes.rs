//! Delivery routes on the server (`docs/delivery-routes-design.md`, steps 1 and 2): depot flags, proving
//! flights and routes. The state is `ose_routes::Routes`, kept in the campaign (`save::Campaign::routes`),
//! so the save, the quicksave and revert carry it. The recorder follows each flight every simulation step,
//! the route pilots (`Shared::route_pilots`, not saved, made again from the routes after a restart or a
//! reload) fly the runs, and once a second the scheduler may ask for a launch, which goes through the same
//! pad launch as a player's.

use bevy_ecs::entity::Entity;
use ose_net::{ClientMessage, DepotInfo, RecordingInfo, RouteInfo, RouteRunInfo, ServerMessage};
use ose_net::{RouteEnd, StayedRun};
use ose_routes::recording::State;
use ose_routes::route::{ColonySite, Outcome, StayedCommand, WhenDelivered};

use crate::{flight_spec_for, owned_vessel, Shared};

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
                State::Landed => ("Landed", None),
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
    let pilots = s.route_pilots.lock().expect("route pilots lock");
    let routes = c
        .routes
        .routes
        .iter()
        .map(|r| RouteInfo {
            id: r.id,
            name: r.name.clone(),
            owner: r.owner,
            recording: r.proving,
            depot: r.depot,
            colony: r.colony().map(|c| c.id),
            status: r.status.clone(),
            paused: r.paused.clone(),
            next_launch_ut: r.next_launch,
            run_vessel: pilots.vessel(r.id).and_then(id_of),
            cargo: r.cargo.clone().into_iter().collect(),
            when_delivered: match r.when_delivered {
                WhenDelivered::Stay => RouteEnd::Stay,
                WhenDelivered::Rotate => RouteEnd::Rotate,
                WhenDelivered::Return => RouteEnd::Return,
                WhenDelivered::LetGo => RouteEnd::LetGo,
            },
            stayed: r.stayed,
            manual: r.active.as_ref().and_then(|a| a.manual),
            runs: r
                .runs
                .iter()
                .map(|x| {
                    let (outcome, reason) = match &x.outcome {
                        Outcome::Delivered => ("Delivered", None),
                        Outcome::Short => ("Short", None),
                        Outcome::Missed(why) => ("Missed", Some(why.clone())),
                        Outcome::Lost(why) => ("Lost", Some(why.clone())),
                    };
                    RouteRunInfo {
                        n: x.n,
                        vessel_name: x.vessel_name.clone(),
                        launched_ut: x.launched_ut,
                        ended_ut: x.ended_ut,
                        outcome: outcome.into(),
                        reason,
                        delivered: x.delivered.clone().into_iter().collect(),
                        after: x.after.clone(),
                        flown_by: x.flown_by.clone(),
                    }
                })
                .collect(),
        })
        .collect();
    ServerMessage::Routes {
        depots,
        recordings,
        routes,
        reason,
    }
}

/// Answer one of the routes messages.
pub fn handle(s: &Shared, player: u64, msg: &ClientMessage) -> ServerMessage {
    let result: Result<(), String> = match *msg {
        ClientMessage::SetDepot { vessel, depot } => match owned_vessel(s, vessel, player) {
            None => Err("not your vessel".into()),
            Some(e) => {
                let mut u = s.universe.lock().expect("sim lock");
                let mut c = s.campaign.lock().expect("campaign lock");
                // A reload clears entity handles; resolve them before changing a depot flag.
                c.routes.follow_depots(&mut u);
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
            // landed within a colony: a route to the colony
            let at = c
                .routes
                .recording(player)
                .map(|p| p.recorder.vessel())
                .and_then(|e| {
                    let landed = u
                        .powered(e)
                        .or_else(|| u.dormant(e))
                        .is_some_and(|p| p.landed);
                    if !landed {
                        return None;
                    }
                    c.colonies
                        .colonies
                        .clone()
                        .into_iter()
                        .find(|col| ose_colonies::members(&mut u, col).contains(&e))
                });
            match at {
                Some(col) => {
                    let site = ColonySite {
                        id: col.id,
                        name: col.name.clone(),
                        body: col.body.clone(),
                        lat_deg: col.lat_deg,
                        lon_deg: col.lon_deg,
                    };
                    c.routes.finish_at_colony(&mut u, player, site).map(|id| {
                        tracing::info!(player, recording = id, colony = col.id, "recording landed");
                    })
                }
                None => c.routes.finish(&mut u, player).map(|id| {
                    tracing::info!(player, recording = id, "recording stopped");
                }),
            }
        }
        ClientMessage::ForgetRecording { id } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            if c.routes.routes.iter().any(|r| r.proving == id) {
                Err("a route is made from this recording".into())
            } else {
                c.routes.forget(player, id)
            }
        }
        ClientMessage::CreateRoute {
            recording,
            ref name,
        } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            c.routes.create_route(player, recording, name).map(|id| {
                tracing::info!(player, recording, route = id, "route created");
            })
        }
        ClientMessage::SetRouteEnd { id, when } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            c.routes.set_when_delivered(
                id,
                match when {
                    RouteEnd::Stay => WhenDelivered::Stay,
                    RouteEnd::Rotate => WhenDelivered::Rotate,
                    RouteEnd::Return => WhenDelivered::Return,
                    RouteEnd::LetGo => WhenDelivered::LetGo,
                },
            )
        }
        ClientMessage::RouteStayed { id, what } => {
            let u = s.universe.lock().expect("sim lock");
            let mut c = s.campaign.lock().expect("campaign lock");
            let mut pilots = s.route_pilots.lock().expect("route pilots lock");
            let cmd = match what {
                StayedRun::SendHome => StayedCommand::SendHome,
                StayedRun::LetGo => StayedCommand::LetGo,
                StayedRun::Keep => StayedCommand::Keep,
            };
            c.routes
                .stayed_command(&u, &mut pilots, id, cmd)
                .map(|_| tracing::info!(player, route = id, ?what, "stayed run"))
        }
        ClientMessage::PauseRoute { id, paused } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            c.routes.pause_route(id, paused)
        }
        ClientMessage::DeleteRoute { id } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            let mut pilots = s.route_pilots.lock().expect("route pilots lock");
            c.routes.delete_route(&mut pilots, id).map(|_| {
                tracing::info!(player, route = id, "route deleted");
            })
        }
        _ => Ok(()),
    };
    info(s, result.err())
}

/// After each simulation step: follow the flights being recorded, fly the route runs, launch what is due
/// (once a second), and tell the players concerned what moved on.
pub fn after_step(s: &Shared, step: u64) {
    // the vessels players fly by hand: a route run among them is theirs until they let go (design 3.11)
    let flown: std::collections::BTreeMap<Entity, u64> = {
        let playing = s.playing.lock().expect("playing lock").clone();
        let flying: Vec<(u64, u64)> = s
            .flying
            .lock()
            .expect("flying lock")
            .iter()
            .filter(|(p, _)| playing.contains(*p))
            .map(|(&p, &v)| (p, v))
            .collect();
        let list = s.vessels.lock().expect("vessels lock");
        flying
            .into_iter()
            .filter_map(|(p, v)| list.iter().find(|x| x.1 == v).map(|x| (x.0, p)))
            .collect()
    };
    let (mut players, requests) = {
        let mut u = s.universe.lock().expect("sim lock");
        let mut c = s.campaign.lock().expect("campaign lock");
        let mut pilots = s.route_pilots.lock().expect("route pilots lock");
        pilots.set_manual(flown);
        let mut players: Vec<u64> = c.routes.tick(&mut u).iter().map(|c| c.player).collect();
        let spec = |craft: &ose_data::craft::Craft| flight_spec_for(s, craft);
        let catalog = ose_pilot::mission::Catalog {
            defs: &s.parts,
            resources: &s.resources,
            spec: &spec,
        };
        players.extend(
            c.routes
                .fly(&mut u, &mut pilots, &catalog)
                .iter()
                .map(|c| c.owner),
        );
        let requests =
            if step.is_multiple_of(ose_sim::TICKS_PER_SECOND) && !c.routes.routes.is_empty() {
                c.routes.schedule(&mut u, &pilots)
            } else {
                Vec::new()
            };
        (players, requests)
    };
    for req in requests {
        launch_run(s, &req);
        players.push(req.owner);
    }
    if players.is_empty() {
        return;
    }
    let msg = info(s, None);
    players.sort_unstable();
    players.dedup();
    let mut pending = s.pending.lock().expect("pending lock");
    for p in players {
        pending.push((p, msg.clone()));
    }
}

/// Launch a route's run on the pad, as a player's launch, and hand it to the route pilot.
fn launch_run(s: &Shared, req: &ose_routes::route::LaunchRequest) {
    let reply = crate::launch_on_pad_now(s, req.owner, &req.craft, None, None);
    let launched = match reply {
        ServerMessage::CraftLaunched {
            vessel: Some(id), ..
        } => s
            .vessels
            .lock()
            .expect("vessels lock")
            .iter()
            .find(|v| v.1 == id)
            .map(|v| (v.0, id)),
        ServerMessage::CraftLaunched {
            reason: Some(why), ..
        } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            c.routes.launch_failed(req.route, &why);
            return;
        }
        _ => None,
    };
    let Some((e, id)) = launched else {
        let mut c = s.campaign.lock().expect("campaign lock");
        c.routes.launch_failed(req.route, "the launch failed");
        return;
    };
    let mut u = s.universe.lock().expect("sim lock");
    u.rename_vessel(e, &req.vessel_name);
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut pilots = s.route_pilots.lock().expect("route pilots lock");
    match c
        .routes
        .launched(&u, &mut pilots, req.route, e, &req.vessel_name)
    {
        Ok(()) => tracing::info!(route = req.route, vessel = id, "route run launched"),
        Err(why) => c.routes.launch_failed(req.route, &why),
    }
}

//! Encode shared state once per publication. Each connection keeps only the newest pending update;
//! reliable command replies continue to be sent directly by the session.

use axum::{
    body::Bytes,
    extract::ws::{Message, WebSocket},
};
use ose_net::{ServerMessage, SNAPSHOT_HZ, TRAJECTORY_HZ};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::watch,
    time::{interval, timeout, MissedTickBehavior},
};

/// A point in the order of stream builds and joins. A counter rather than a clock: on a coarse clock a
/// build begun before a join can read the same `Instant` as the join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mark(u64);

static MARKS: AtomicU64 = AtomicU64::new(0);

/// The current point in that order (taken when a connection subscribes).
pub fn mark() -> Mark {
    Mark(MARKS.fetch_add(1, Ordering::SeqCst))
}

pub struct Batch {
    /// Taken before reading the world, so a joining player can skip work begun before they joined.
    started: Mark,
    messages: Box<[Bytes]>,
}

type Frames = Arc<Batch>;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Streams {
    pub snapshots: watch::Sender<Frames>,
    pub trajectories: watch::Sender<Frames>,
}

impl Default for Streams {
    fn default() -> Self {
        let empty = Arc::new(Batch {
            started: mark(),
            messages: Box::new([]),
        });
        Self {
            snapshots: watch::channel(empty.clone()).0,
            trajectories: watch::channel(empty).0,
        }
    }
}

fn encode(messages: impl IntoIterator<Item = ServerMessage>) -> Option<Box<[Bytes]>> {
    messages
        .into_iter()
        .map(|message| {
            ose_net::encode(&message)
                .map(Bytes::from)
                .map_err(|error| {
                    tracing::error!(%error, "encode shared stream");
                })
                .ok()
        })
        .collect::<Option<Vec<_>>>()
        .map(Vec::into_boxed_slice)
}

pub fn start(s: super::AppState) {
    tokio::spawn(async move {
        let mut snapshots = interval(Duration::from_secs_f64(1.0 / SNAPSHOT_HZ as f64));
        let mut predictions = interval(Duration::from_secs_f64(1.0 / TRAJECTORY_HZ as f64));
        // Old world states have no value after a stall. Never burst to catch up.
        snapshots.set_missed_tick_behavior(MissedTickBehavior::Skip);
        predictions.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = snapshots.tick() => {
                    // Registry maintenance belongs to the world, not to any particular connection.
                    {
                        let mut u = s.universe.lock().expect("sim lock");
                        let mut list = s.vessels.lock().expect("vessels lock");
                        for (from, debris) in u.take_broken_off() {
                            let owner = list.iter().find(|v| v.0 == from).and_then(|v| v.2);
                            let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
                            list.push((debris, id, owner));
                        }
                        list.retain(|&(e, _, _)| u.vessel(e).is_some());
                    }
                    if s.streams.snapshots.receiver_count() > 0 {
                        let started = std::time::Instant::now();
                        publish(&s.streams.snapshots, || {
                            let (time, bodies, vessels) = super::snapshot(&s);
                            [time, bodies, vessels]
                        });
                        tracing::debug!(elapsed_us = started.elapsed().as_micros() as u64,
                            clients = s.streams.snapshots.receiver_count(), "snapshot published");
                    }
                }
                _ = predictions.tick() => {
                    if s.streams.trajectories.receiver_count() > 0 {
                        let started = std::time::Instant::now();
                        publish(&s.streams.trajectories, || super::trajectories(&s));
                        tracing::debug!(elapsed_us = started.elapsed().as_micros() as u64,
                            clients = s.streams.trajectories.receiver_count(), "trajectories published");
                    }
                }
            }
        }
    });
}

fn publish<M: IntoIterator<Item = ServerMessage>>(
    sender: &watch::Sender<Frames>,
    build: impl FnOnce() -> M,
) {
    let started = mark();
    if let Some(messages) = encode(build()) {
        // send() drops the batch if the last client left while it was being built.
        let _ = sender.send(Arc::new(Batch { started, messages }));
    }
}

/// Ignore an in-flight build begun before this connection subscribed (for example before its probe
/// was spawned), even if that build finishes publishing after the subscription was created.
pub async fn next_batch(receiver: &mut watch::Receiver<Frames>, since: Mark) -> Option<Frames> {
    loop {
        receiver.changed().await.ok()?;
        let frames = receiver.borrow_and_update().clone();
        if frames.started >= since {
            return Some(frames);
        }
    }
}

pub async fn send_bytes(socket: &mut WebSocket, bytes: Bytes) -> bool {
    matches!(
        timeout(WRITE_TIMEOUT, socket.send(Message::Binary(bytes))).await,
        Ok(Ok(()))
    )
}

pub async fn send_batch(socket: &mut WebSocket, frames: &Frames) -> bool {
    // Bound the whole update, including trajectories for a large fleet. On timeout the caller exits
    // and drops the socket: a partially written frame must never be retried on this connection.
    timeout(WRITE_TIMEOUT, async {
        for bytes in frames.messages.iter() {
            if socket.send(Message::Binary(bytes.clone())).await.is_err() {
                return false;
            }
        }
        true
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_data::Tick;

    #[tokio::test]
    async fn joining_during_a_build_waits_for_fresh_world_state() {
        let streams = Streams::default();
        let mut subscription = None;
        let mut joined = None;
        publish(&streams.snapshots, || {
            // The world was captured before this player arrived, but publication is still in flight.
            subscription = Some(streams.snapshots.subscribe());
            joined = Some(mark());
            [ServerMessage::Snapshot {
                tick: Tick(1),
                bodies: vec![],
            }]
        });
        let mut subscription = subscription.unwrap();
        let mut next = Box::pin(next_batch(&mut subscription, joined.unwrap()));
        assert!(futures_util::poll!(next.as_mut()).is_pending());
        publish(&streams.snapshots, || {
            [ServerMessage::Snapshot {
                tick: Tick(2),
                bodies: vec![],
            }]
        });
        let frames = next.await.unwrap();
        assert!(matches!(
            ose_net::decode::<ServerMessage>(&frames.messages[0]).unwrap(),
            ServerMessage::Snapshot { tick: Tick(2), .. }
        ));
    }

    #[tokio::test]
    async fn slow_readers_skip_old_batches_and_share_encoded_storage() {
        let streams = Streams::default();
        let mut fast = streams.snapshots.subscribe();
        let mut slow = streams.snapshots.subscribe();
        for tick in 1..=100 {
            publish(&streams.snapshots, || {
                [
                    ServerMessage::TimeState {
                        tick: Tick(tick),
                        warp: 1,
                        warp_limited_by: None,
                    },
                    ServerMessage::Snapshot {
                        tick: Tick(tick),
                        bodies: vec![],
                    },
                ]
            });
            fast.changed().await.unwrap();
            fast.borrow_and_update();
        }
        slow.changed().await.unwrap();
        let newest = slow.borrow_and_update().clone();
        assert!(Arc::ptr_eq(&newest, &fast.borrow()));
        assert!(matches!(
            ose_net::decode::<ServerMessage>(&newest.messages[0]).unwrap(),
            ServerMessage::TimeState {
                tick: Tick(100),
                ..
            }
        ));
        assert!(matches!(
            ose_net::decode::<ServerMessage>(&newest.messages[1]).unwrap(),
            ServerMessage::Snapshot {
                tick: Tick(100),
                ..
            }
        ));
        assert!(!slow.has_changed().unwrap());
        // A joining connection waits for the next publication instead of replaying stale state.
        assert!(!streams.snapshots.subscribe().has_changed().unwrap());
    }
}

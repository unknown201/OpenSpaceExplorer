//! What each player is sent of the world: interest management for the shared streams.
//!
//! The world stream used to send every vessel in full to every player at `SNAPSHOT_HZ`, so the bytes grew with
//! players times vessels times the size of a full flight readout (crew, parts, stages, heat, gimbals and the
//! rest, some kilobytes for a large craft). Now a player gets in full only the vessels they can see up close:
//!
//! - the vessels in their **focus**: the one they pilot and the one their view follows (`ClientMessage::Viewing`,
//!   a rider's vessel or one picked on the map), and
//! - the vessels **near** a focus vessel (within `NEAR_M`, beyond the client's 5 km full-detail drawing range),
//!   or, for a player with no focus (at the KSC, in the VAB), the vessels standing on the home planet.
//!
//! Every other vessel is sent **slim**: name, owner, orbit, position, velocity, target and, instead of its flight
//! readout, a brief of it (situation, EVA, height). The message still lists every vessel at every snapshot, as
//! the client expects; only the detail of far vessels is missing. Trajectories of far
//! vessels are sent every `FAR_TRAJECTORY_EVERY`th publication instead of every one.
//!
//! The work stays shared: each vessel is encoded once per publication in both forms, and a player's message is
//! assembled from those bytes (postcard writes a `Vec` as its length and then its elements back to back).

use std::collections::HashSet;

use axum::body::Bytes;
use ose_data::Tick;
use ose_math::DVec3;
use ose_net::{FlightBrief, FlightSituation, ServerMessage, VesselState};

use crate::Shared;

/// How near a focus vessel another must be to be sent in full, m (the client draws vessels within 5 km).
pub const NEAR_M: f64 = 10_000.0;
/// Far vessels' trajectories go out once in this many trajectory publications.
pub const FAR_TRAJECTORY_EVERY: u64 = 5;

/// One vessel of a publication, encoded in full and slim.
pub struct Entry {
    pub id: u64,
    body: u16,
    position_m: DVec3,
    /// Standing on the home planet (shown at the KSC).
    grounded_home: bool,
    full: Bytes,
    slim: Bytes,
}

/// The `Vessels` message of one publication, ready to assemble per player.
pub struct VesselFrames {
    /// The encoded `ServerMessage::Vessels` up to its vessel count.
    header: Vec<u8>,
    pub entries: Vec<Entry>,
}

/// A vessel without the flight readout only a player close by needs: its situation, whether it is a Kerbal
/// on EVA and its height stay (`VesselState::brief`), for the map and the tracking station.
pub fn slim(v: &VesselState) -> VesselState {
    let mut v = v.clone();
    if let Some(f) = v.flight.take() {
        v.brief = Some(FlightBrief {
            situation: f.situation,
            eva: f.eva,
            altitude_m: f.altitude_m,
        });
    }
    v
}

fn varint(mut n: usize, out: &mut Vec<u8>) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

impl VesselFrames {
    /// Encode a publication's vessels (`home`: the home planet's catalog index).
    pub fn build(tick: Tick, vessels: &[VesselState], home: u16) -> Option<VesselFrames> {
        let mut header = ose_net::encode(&ServerMessage::Vessels {
            tick,
            vessels: Vec::new(),
        })
        .ok()?;
        // the empty list's length, a single zero byte
        header.pop();
        let entries = vessels
            .iter()
            .map(|v| {
                let grounded_home = v.reference_body == home
                    && v.flight.as_ref().is_some_and(|f| {
                        matches!(
                            f.situation,
                            FlightSituation::Landed | FlightSituation::Splashed
                        )
                    });
                Some(Entry {
                    id: v.id,
                    body: v.reference_body,
                    position_m: v.position_m,
                    grounded_home,
                    full: ose_net::encode(v).ok()?.into(),
                    slim: ose_net::encode(&slim(v)).ok()?.into(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(VesselFrames { header, entries })
    }

    /// The vessels `view` sees up close: its focus and those near a focus vessel (or standing on the home
    /// planet, for a player without a focus).
    pub fn near(&self, focus: &[u64]) -> HashSet<u64> {
        let anchors: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| focus.contains(&e.id))
            .collect();
        self.entries
            .iter()
            .filter(|e| {
                if anchors.is_empty() {
                    e.grounded_home
                } else {
                    anchors.iter().any(|a| {
                        a.id == e.id
                            || (a.body == e.body && (a.position_m - e.position_m).length() < NEAR_M)
                    })
                }
            })
            .map(|e| e.id)
            .collect()
    }

    /// The `Vessels` message for a player who sees `near` up close.
    pub fn message(&self, near: &HashSet<u64>) -> Bytes {
        let mut out = self.header.clone();
        varint(self.entries.len(), &mut out);
        for e in &self.entries {
            let bytes = if near.contains(&e.id) {
                &e.full
            } else {
                &e.slim
            };
            out.extend_from_slice(bytes);
        }
        out.into()
    }
}

/// The vessels in `player`'s focus: the one they pilot and the one their view follows.
pub fn focus(s: &Shared, player: u64) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    out.extend(s.flying.lock().expect("flying lock").get(&player));
    out.extend(s.viewing.lock().expect("viewing lock").get(&player));
    out.dedup();
    out
}

/// What one connection was last sent up close, for the trajectory stream.
#[derive(Default)]
pub struct Interest {
    pub near: HashSet<u64>,
    /// Trajectory publications seen.
    pub trajectories: u64,
}

impl Interest {
    /// Whether this publication of `vessel`'s trajectory goes out.
    pub fn wants_trajectory(&self, vessel: u64) -> bool {
        self.near.contains(&vessel) || self.trajectories.is_multiple_of(FAR_TRAJECTORY_EVERY)
    }
}

/// `player` follows `vessel` with their view now (`None`: nothing).
pub fn set_viewing(s: &Shared, player: u64, vessel: Option<u64>) {
    let mut v = s.viewing.lock().expect("viewing lock");
    match vessel {
        Some(id) => v.insert(player, id),
        None => v.remove(&player),
    };
}

/// For tests: decode an assembled message.
#[cfg(test)]
fn decode(bytes: &[u8]) -> (Tick, Vec<VesselState>) {
    match ose_net::decode::<ServerMessage>(bytes).unwrap() {
        ServerMessage::Vessels { tick, vessels } => (tick, vessels),
        m => panic!("{m:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vessel(id: u64, x: f64) -> VesselState {
        ose_net::decode::<VesselState>(
            &ose_net::encode(&VesselState {
                id,
                name: format!("v{id}"),
                owner: Some(1),
                reference_body: 4,
                position_m: DVec3::new(x, 0.0, 0.0),
                velocity_mps: DVec3::ZERO,
                accel_mps2: DVec3::ZERO,
                orbit: ose_net::OrbitElements {
                    semi_major_axis_m: 700_000.0,
                    eccentricity: 0.0,
                    inclination_deg: 0.0,
                    longitude_of_ascending_node_deg: 0.0,
                    argument_of_periapsis_deg: 0.0,
                    mean_anomaly_at_epoch_rad: 0.0,
                    epoch_s: 0.0,
                },
                on_rails: true,
                flight: None,
                target: None,
                craft_rev: 1,
                brief: None,
            })
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_players_message_decodes_as_the_whole_list_with_far_vessels_slim() {
        let list = vec![vessel(1, 0.0), vessel(2, 500.0), vessel(3, 50_000.0)];
        let frames = VesselFrames::build(Tick(7), &list, 4).unwrap();
        let near = frames.near(&[1]);
        assert_eq!(near, HashSet::from([1, 2]));
        let (tick, got) = decode(&frames.message(&near));
        assert_eq!(tick, Tick(7));
        assert_eq!(
            got, list,
            "vessels without a flight readout are the same slim or full"
        );
        // nobody in focus: only vessels standing on the home planet are near
        assert!(frames.near(&[]).is_empty());
        // many vessels: the count's varint takes two bytes
        let many: Vec<VesselState> = (0..300).map(|i| vessel(i, i as f64)).collect();
        let frames = VesselFrames::build(Tick(8), &many, 4).unwrap();
        assert_eq!(decode(&frames.message(&HashSet::new())).1, many);
    }
}

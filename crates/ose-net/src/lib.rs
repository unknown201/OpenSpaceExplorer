//! Wire protocol between client and server. Encoded with `postcard`, one message per WebSocket binary frame.
//!
//! Session: client sends `Hello`; server answers `Welcome` then `BodyCatalog`; after that the server streams
//! `TimeState` + `Snapshot` at a fixed rate (`SNAPSHOT_HZ`) and the client sends intents (`RequestWarp`).
//! Positions are metres in star space (inertial, Z-up, centred on the root star), see
//! `docs/reference-notes/orbits.md`.

#![forbid(unsafe_code)]

use ose_data::Tick;
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 3;
/// Trajectory messages per second the server streams for each vessel.
pub const TRAJECTORY_HZ: u32 = 1;
/// Snapshots per second the server streams to each client.
pub const SNAPSHOT_HZ: u32 = 10;
/// WebSocket path on the server.
pub const WS_PATH: &str = "/ws";

/// Client to server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello {
        protocol: u16,
        token: Option<String>,
        display_name: String,
    },
    RequestWarp {
        factor: u32,
    },
    /// Schedule an impulsive burn on a vessel the player owns, in its orbital frame at `ut`.
    ScheduleBurn {
        vessel: u64,
        ut: f64,
        prograde_mps: f64,
        normal_mps: f64,
        radial_mps: f64,
    },
    /// Remove the scheduled burn from a vessel the player owns.
    ClearBurn {
        vessel: u64,
    },
    /// Upload a craft and put it in a circular orbit (Phase 6 replaces this with a launch pad).
    LaunchCraft {
        craft: ose_data::craft::Craft,
        altitude_m: f64,
    },
    Ping {
        sent_tick: Tick,
    },
}

/// Static description of one celestial body, sent once after `Welcome`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyInfo {
    pub id: String,
    /// Index into `BodyCatalog::bodies` of the parent, `None` for the root star.
    pub parent: Option<u16>,
    pub radius_m: f64,
    pub mu: f64,
    pub soi_radius_m: Option<f64>,
    pub is_star: bool,
    pub has_atmosphere: bool,
    pub atmosphere_depth_m: f64,
    /// Orbit around the parent, `None` for the root. Degrees for angles, radians for mean anomaly.
    pub orbit: Option<OrbitElements>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrbitElements {
    pub semi_major_axis_m: f64,
    pub eccentricity: f64,
    pub inclination_deg: f64,
    pub longitude_of_ascending_node_deg: f64,
    pub argument_of_periapsis_deg: f64,
    pub mean_anomaly_at_epoch_rad: f64,
    pub epoch_s: f64,
}

/// Star-space position and velocity of one body, same order as the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyState {
    pub position_m: DVec3,
    pub velocity_mps: DVec3,
}

/// A vessel as streamed in `Vessels`. `position_m` / `velocity_mps` are star space like `BodyState`;
/// `orbit` is around the body at catalog index `reference_body`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VesselState {
    pub id: u64,
    pub name: String,
    /// Player who owns the vessel, if any.
    pub owner: Option<u64>,
    pub reference_body: u16,
    pub position_m: DVec3,
    pub velocity_mps: DVec3,
    pub orbit: OrbitElements,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchEnd {
    Encounter,
    Escape,
    Collision,
    Maneuver,
    Final,
}

/// One conic of a predicted trajectory, around the body at catalog index `body`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryPatch {
    pub body: u16,
    pub orbit: OrbitElements,
    pub start_ut: f64,
    /// Seconds; infinity is sent as `f64::INFINITY` (postcard encodes it fine).
    pub end_ut: f64,
    pub end: PatchEnd,
}

/// Server to client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome {
        player_id: u64,
        token: String,
        tick: Tick,
    },
    Rejected {
        reason: String,
    },
    BodyCatalog {
        bodies: Vec<BodyInfo>,
    },
    TimeState {
        tick: Tick,
        warp: u32,
        warp_limited_by: Option<u64>,
    },
    /// Positions of every catalog body at `tick`.
    Snapshot {
        tick: Tick,
        bodies: Vec<BodyState>,
    },
    /// Every vessel the client can see, sent with each `Snapshot`.
    Vessels {
        tick: Tick,
        vessels: Vec<VesselState>,
    },
    /// Predicted patched-conic trajectory of one vessel from `tick`, sent at `TRAJECTORY_HZ`.
    Trajectory {
        tick: Tick,
        vessel: u64,
        patches: Vec<TrajectoryPatch>,
    },
    /// Answer to `LaunchCraft`.
    CraftLaunched {
        vessel: Option<u64>,
        name: String,
        parts: u32,
        wet_mass_t: f64,
        dry_mass_t: f64,
        delta_v_vacuum_mps: f64,
        reason: Option<String>,
    },
    /// Answer to `ScheduleBurn` / `ClearBurn`.
    BurnState {
        vessel: u64,
        scheduled: bool,
        reason: Option<String>,
    },
    Pong {
        sent_tick: Tick,
    },
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let m = ServerMessage::TimeState {
            tick: Tick(42),
            warp: 1000,
            warp_limited_by: Some(7),
        };
        let bytes = encode(&m).unwrap();
        let back: ServerMessage = decode(&bytes).unwrap();
        assert_eq!(m, back);

        let s = ServerMessage::Snapshot {
            tick: Tick(1),
            bodies: vec![BodyState {
                position_m: DVec3::new(1.0, 2.0, 3.0),
                velocity_mps: DVec3::new(4.0, 5.0, 6.0),
            }],
        };
        let back: ServerMessage = decode(&encode(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }
}

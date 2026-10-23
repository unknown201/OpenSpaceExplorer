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

pub const PROTOCOL_VERSION: u16 = 7;
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
    /// Join the shared campaign (ADR-0001: one co-op save). Idempotent; answers `CampaignState`.
    JoinCampaign,
    /// Ask for the campaign state without joining.
    CampaignInfo,
    /// Session browser: answers `SessionList` (one entry per server, ADR-0001).
    ListSessions,
    /// Join the session with this id (same as `JoinCampaign` for the shared one); answers `CampaignState`.
    SelectSession {
        id: String,
    },
    /// Configure the shared campaign when nobody has joined it yet (name, mode, difficulty); otherwise the
    /// existing campaign is returned unchanged with `created: false`. Answers `CampaignState`.
    CreateSession {
        name: String,
        mode: SessionMode,
        difficulty: Vec<DifficultyOption>,
    },
    /// Set the player agency (flag and colours), echoed in `CampaignMember::agency`. Answers `CampaignState`.
    SetAgency {
        agency: Agency,
    },
    /// Upload a craft and put it in a circular orbit (Phase 6 replaces this with a launch pad).
    LaunchCraft {
        craft: ose_data::craft::Craft,
        altitude_m: f64,
    },
    /// Store a craft in the player library; answers `CraftSaved`.
    SaveCraft {
        craft: ose_data::craft::Craft,
    },
    /// Remove one of the player's saved crafts; answers `CraftList`.
    DeleteCraft {
        id: u64,
    },
    /// List stock crafts and the player's saved crafts; answers `CraftList`.
    CraftList,
    /// Fetch a listed craft document (stock or own); answers `CraftDocument`.
    GetCraft {
        id: u64,
    },
    /// Launch a listed craft like `LaunchCraft`; answers `CraftLaunched`.
    LaunchSavedCraft {
        id: u64,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    Sandbox,
    Exploration,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DifficultyValue {
    Bool(bool),
    Number(f64),
    Choice(String),
}

/// One difficulty setting as chosen for a campaign.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DifficultyOption {
    pub id: String,
    pub value: DifficultyValue,
}

/// How a difficulty setting is presented: from the imported `DifficultyOptionDefinitions`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DifficultyDefinition {
    pub id: String,
    pub section: String,
    /// "base" (always shown) or "exploration" (exploration mode only).
    pub group: String,
    /// "Bool" or "Percentage".
    pub kind: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

/// A difficulty preset (Easy, Normal, Hard, RocketScientist) as a full option list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DifficultyPreset {
    pub name: String,
    pub options: Vec<DifficultyOption>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Agency {
    pub flag_id: String,
    pub primary_rgb: [u8; 3],
    pub secondary_rgb: [u8; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub mode: SessionMode,
    pub ut_s: f64,
    pub last_played_unix: u64,
    pub players: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampaignMember {
    pub player_id: u64,
    pub display_name: String,
    pub joined_tick: Tick,
    pub agency: Option<Agency>,
}

/// A craft in the library listing. Stock crafts have ids below 1000 and no owner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CraftSummary {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub parts: u32,
    pub stages: u32,
    pub dry_mass_t: f64,
    pub wet_mass_t: f64,
    pub delta_v_vacuum_mps: f64,
    pub owner: Option<u64>,
    pub stock: bool,
}

/// One stage of a craft's delta-v readout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageReadout {
    pub stage_id: u32,
    pub start_mass_t: f64,
    pub end_mass_t: f64,
    pub delta_v_vacuum_mps: f64,
    pub burn_time_s: f64,
    pub thrust_kn: f64,
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
    /// The shared campaign: its name, when it started, who is in it, and whether you are a member.
    CampaignState {
        id: String,
        name: String,
        mode: SessionMode,
        difficulty: Vec<DifficultyOption>,
        started_tick: Tick,
        members: Vec<CampaignMember>,
        you_are_member: bool,
        vessels: u32,
        /// True when this answer is to a `CreateSession` that configured the campaign.
        created: bool,
    },
    /// Answer to `ListSessions`.
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    /// Difficulty option definitions and presets, sent once after `BodyCatalog`.
    DifficultyOptions {
        definitions: Vec<DifficultyDefinition>,
        presets: Vec<DifficultyPreset>,
    },
    /// Answer to `LaunchCraft`. `stages` is the per-stage vacuum delta-v readout, first-firing stage first.
    CraftLaunched {
        vessel: Option<u64>,
        name: String,
        parts: u32,
        wet_mass_t: f64,
        dry_mass_t: f64,
        delta_v_vacuum_mps: f64,
        stages: Vec<StageReadout>,
        reason: Option<String>,
    },
    /// Answer to `CraftList` / `DeleteCraft`.
    CraftList {
        crafts: Vec<CraftSummary>,
    },
    /// Answer to `SaveCraft`.
    CraftSaved {
        id: Option<u64>,
        summary: Option<CraftSummary>,
        reason: Option<String>,
    },
    /// Answer to `GetCraft`.
    CraftDocument {
        id: u64,
        craft: Option<ose_data::craft::Craft>,
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

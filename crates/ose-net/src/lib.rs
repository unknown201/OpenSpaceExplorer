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

pub const PROTOCOL_VERSION: u16 = 34;
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
    /// Session browser: answers `SessionList`, every campaign on the server, the one being played first.
    ListSessions,
    /// Play the campaign with this id (resume); answers `CampaignState`, or `Rejected` while another player
    /// is playing a different campaign.
    SelectSession {
        id: String,
    },
    /// Start a new campaign (name, mode, difficulty), its clock on the first morning at the KSC, and join
    /// it; the one being played goes back on the list. Answers `CampaignState` with `created: true`, or
    /// `Rejected` while another player is playing.
    CreateSession {
        name: String,
        mode: SessionMode,
        difficulty: Vec<DifficultyOption>,
    },
    /// Leave the campaign for the main menu (it stays on the list to resume); answers `SessionList`.
    LeaveSession,
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
    /// Put an uploaded craft on the launch pad (KSC), landed and under powered-flight control; answers
    /// `CraftLaunched`.
    LaunchOnPad {
        craft: ose_data::craft::Craft,
    },
    /// Put a listed craft on the launch pad; answers `CraftLaunched`.
    LaunchSavedOnPad {
        id: u64,
    },
    /// Like `LaunchOnPad` at a launch site from `LaunchSites` (`None`: launch pad 1). `crew` names the
    /// Kerbal for each seat in order (an empty name leaves the seat empty; fewer names leave the rest
    /// empty); `None` fills the seats from the roster.
    LaunchAt {
        craft: ose_data::craft::Craft,
        site: Option<String>,
        crew: Option<Vec<String>>,
    },
    /// Like `LaunchSavedOnPad` at a launch site from `LaunchSites`, crewed like `LaunchAt`.
    LaunchSavedAt {
        id: u64,
        site: Option<String>,
        crew: Option<Vec<String>>,
    },
    /// The campaign's astronauts; answers `KerbalRoster` (which also comes whenever it changes).
    KerbalRoster,
    /// Throttle (0..1) and attitude for one of the player's powered vessels; answers `FlightAck`.
    SetControls {
        vessel: u64,
        throttle: f64,
        attitude: AttitudeMode,
    },
    /// Fire the next stage of a powered vessel; answers `FlightAck`.
    Stage {
        vessel: u64,
    },
    Ping {
        sent_tick: Tick,
    },
    /// Run every experiment on the vessel's parts where it is; answers `ScienceReports`.
    RunExperiments {
        vessel: u64,
    },
    /// Send the vessel's reports home for science points; answers `ScienceSubmitted`.
    TransmitScience {
        vessel: u64,
    },
    /// Recover a vessel standing on the home world: its reports are submitted and it is removed. Answers
    /// `ScienceSubmitted`.
    RecoverVessel {
        vessel: u64,
    },
    /// Ask for the campaign's science, tech tree and missions; answers `ScienceState`.
    ScienceInfo,
    /// Research a tech node with the campaign's free science points; answers `TechUnlock` and
    /// `ScienceState`.
    UnlockTechNode {
        id: String,
    },
    /// A Kerbal (the first aboard when `kerbal` is `None`) steps out on EVA as a vessel of their own;
    /// answers `CrewAck` with it in `new_vessel`.
    GoEva {
        vessel: u64,
        kerbal: Option<String>,
    },
    /// A Kerbal on EVA (`vessel`) climbs into `target` (within 6 m, with a free seat); answers `CrewAck`.
    Board {
        vessel: u64,
        target: u64,
    },
    /// A Kerbal on EVA standing on the ground plants the agency flag; answers `CrewAck` with the flag.
    PlantFlag {
        vessel: u64,
    },
    /// Undock at a docked joint (the docking port part on either side; the last joint when `None`); answers
    /// `Undocked`.
    Undock {
        vessel: u64,
        port: Option<u16>,
    },
    /// Fold the vessel's deployable solar panels out (`deploy`) or in; answers `FlightAck`.
    DeployPanels {
        vessel: u64,
        deploy: bool,
    },
    /// RCS translation in craft axes, each -1..1 and held until changed (x right J/L, y fore H/N, z top
    /// I/K); it fires only with the RCS action group on. Answers `FlightAck`.
    Translate {
        vessel: u64,
        x: f32,
        y: f32,
        z: f32,
    },
    /// Rotation input with SAS off (`AttitudeMode::Free`), each -1..1 and held until changed: pitch turns
    /// the nose toward craft +Z (the navball's top), yaw toward craft +X, roll clockwise about the nose. The
    /// reaction wheels (and RCS with its group on) set how fast the spin builds; nothing damps it. Answers
    /// `FlightAck`.
    SetRotation {
        vessel: u64,
        pitch: f32,
        yaw: f32,
        roll: f32,
    },
    /// A Kerbal on EVA: `run` (Shift) held until changed, `jump` (Space) once from the ground. Its jetpack
    /// translates with `Translate` (always on for a Kerbal: y up along the Kerbal, Ctrl down). Answers
    /// `FlightAck`.
    EvaControls {
        vessel: u64,
        run: bool,
        jump: bool,
    },
    /// Roll input (-1..1, Q/E), held until changed; answers `FlightAck`.
    SetRoll {
        vessel: u64,
        input: f32,
    },
    /// Set (`Some`) or toggle (`None`) an action group; answers `FlightAck`.
    ActionGroup {
        vessel: u64,
        group: ActionGroup,
        on: Option<bool>,
    },
    /// The staging readout of a craft in the editor, without saving or launching it; answers `StageReport`.
    StageReport {
        craft: ose_data::craft::Craft,
    },
    /// Quicksave the campaign (F5); answers `SaveAck`.
    QuickSave,
    /// Load the campaign's quicksave (F9, needs `AllowQuickLoad`); answers `SaveAck`.
    QuickLoad,
    /// Back to the moment of the player's last launch, on the pad (needs `AllowRevert`); answers `SaveAck`.
    RevertToLaunch,
    /// Back to just before the player's last launch, with its craft for the editor (needs `AllowRevert`);
    /// answers `SaveAck` with `craft`.
    RevertToEditor,
    /// Switch the vessel's resource converters (fuel cells, reactor) on or off; answers `FlightAck`.
    SetConverters {
        vessel: u64,
        on: bool,
    },
    /// Drive a vessel on its wheels: wheel throttle (-1 back .. 1 forward), steering (-1 left .. 1 right)
    /// and brakes, held until changed; answers `FlightAck`.
    Drive {
        vessel: u64,
        throttle: f32,
        steer: f32,
        brakes: bool,
    },
    /// Fold the vessel's landing legs and retractable wheels out (`deploy`) or in; answers `FlightAck`.
    DeployGear {
        vessel: u64,
        deploy: bool,
    },
    /// Arm every packed parachute (they open when the air is thick enough); answers `FlightAck`.
    DeployParachutes {
        vessel: u64,
    },
    /// Take on an offered mission at Mission Control; answers `MissionAck` and `ScienceState`.
    AcceptMission {
        id: String,
    },
    /// Hand in a completed mission for its science reward; answers `MissionAck` and `ScienceState`.
    TurnInMission {
        id: String,
    },
}

/// A science report (the original's research report).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportInfo {
    /// `{experiment}_{DataType|SampleType}_{location}`: one submission each.
    pub key: String,
    pub experiment: String,
    pub sample: bool,
    /// `{Body}_{Situation}` or `{Body}_{Situation}_{Region}`.
    pub location: String,
    /// Science points it is worth (before the campaign's science reward multiplier).
    pub value: f64,
}

/// An experiment taking its time (`TimeToComplete`, drawing charge while it runs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunningExperiment {
    pub part: u16,
    pub experiment: String,
    pub remaining_s: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkippedExperiment {
    pub part: u16,
    pub experiment: String,
    pub reason: String,
}

/// One mission's state (texts are the original's localisation keys).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionInfo {
    pub id: String,
    pub name_key: String,
    pub description_key: String,
    pub primary: bool,
    pub granter: String,
    /// Current stage index and the number of stages.
    pub stage: u32,
    pub stages: u32,
    /// The current stage's objective key (empty once complete).
    pub objective_key: String,
    pub complete: bool,
    pub science_reward: f64,
    /// Taken on (missions only progress once accepted; the first is accepted from the start).
    pub accepted: bool,
    /// Handed in: its reward is paid.
    pub turned_in: bool,
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
    /// Present while the vessel is under powered flight (landed, ascending, descending).
    pub flight: Option<FlightReadout>,
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

/// Attitude hold modes for powered flight (mirrors `ose_sim::flight::Attitude`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AttitudeMode {
    Up,
    SurfacePrograde,
    SurfaceRetrograde,
    OrbitPrograde,
    OrbitRetrograde,
    Normal,
    AntiNormal,
    RadialOut,
    RadialIn,
    /// Pitch above the horizon and compass heading (90 = east), degrees.
    PitchHeading {
        pitch_deg: f64,
        heading_deg: f64,
    },
    /// Fixed direction in the reference body's inertial frame.
    Fixed(DVec3),
    /// Toward the vessel's scheduled burn (refused when none is planned).
    Maneuver,
    /// SAS off: nothing holds the attitude; `SetRotation` turns the vessel and its spin carries on.
    Free,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlightSituation {
    Landed,
    Flying,
    SubOrbital,
    Orbit,
    Escaping,
    Crashed,
}

/// Powered-flight readout, present on a `VesselState` while the vessel is under the flight model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlightReadout {
    pub situation: FlightSituation,
    pub altitude_m: f64,
    pub surface_speed_mps: f64,
    pub vertical_speed_mps: f64,
    pub throttle: f64,
    pub attitude: AttitudeMode,
    /// Current stage index (0 = first to fire) and total stages.
    pub stage: u32,
    pub stages: u32,
    pub propellant_t: f64,
    pub propellant_fraction: f64,
    pub mass_t: f64,
    pub thrust_kn: f64,
    pub twr: f64,
    pub dynamic_pressure_kpa: f64,
    /// Ambient static pressure, for atmosphere/vacuum visual effects.
    pub static_pressure_kpa: f64,
    /// Attached, fuel-fed engine part indices currently producing thrust.
    pub firing_engines: Vec<u16>,
    /// Where the vessel actually points (the craft's +Y, its thrust axis): unit vector in the reference
    /// body's inertial frame (Z-up, the same frame as `VesselState`). It turns toward `attitude` at a
    /// limited rate.
    pub facing: DVec3,
    /// Roll reference: unit vector across `facing` along which the craft's forward side (craft +Z, where a
    /// pod's windows face) points. Roll is not simulated, so this is the local north projected across the
    /// vessel axis (east when the axis is north-south).
    pub roll_ref: DVec3,
    /// Electric charge the vessel's batteries hold and can hold, units (0 / 0 without a part structure).
    pub electric_charge: f64,
    pub electric_capacity: f64,
    /// False for an uncrewed vessel whose probe cores have run out of charge: it cannot steer or throttle.
    pub has_control: bool,
    /// Worst joint load as a fraction of its break limit (1 breaks), 0 without a part structure.
    pub joint_stress: f64,
    /// Parts bent away from their rest pose by joint flex (wobble); parts within 0.1 mrad and 2 mm are left
    /// out. Craft part indices, in the craft's own frame (Unity Y-up, as `local_position`).
    pub flex: Vec<PartFlex>,
    /// Kerbals aboard: (craft part index, name).
    pub crew: Vec<(u16, String)>,
    /// This vessel is a Kerbal on EVA.
    pub eva: bool,
    /// Free docking ports (craft part indices) and docked joints (part docked to, part that docked).
    pub docking_ports: Vec<u16>,
    pub docked: Vec<(u16, u16)>,
    /// CommNet: linked to the KSC (directly or through relays) and over how many links; the vessel's best
    /// antenna range, m (0 without one). Transmitting needs the link.
    pub comm_connected: bool,
    pub comm_hops: u32,
    pub comm_range_m: f64,
    /// Deployable solar panels folded out.
    pub panels_deployed: bool,
    /// Each attached deployable panel's command state, by craft part index (animation instances).
    pub panels: Vec<(u16, bool)>,
    /// Fell over on landing and lies on its side.
    pub tipped: bool,
    /// Parachutes: (craft part index, state, how far open 0..1).
    pub parachutes: Vec<(u16, ChuteState, f32)>,
    /// Parts that are heating up or warm: (craft part index, temperature K, maximum K, re-entry heat kW).
    /// Listed when re-entry heat reaches them or they are above 40% of their maximum.
    pub heat: Vec<(u16, f32, f32, f32)>,
    /// Landing legs and wheels: (craft part index, folded out).
    pub gear: Vec<(u16, bool)>,
    /// Driving on wheels: speed along the drive direction (m/s, negative backing up) and brakes on.
    pub drive_speed_mps: f32,
    pub brakes: bool,
    /// Resource converters: (craft part index, switched on, share of its rate it is running at 0..1).
    pub converters: Vec<(u16, bool, f32)>,
    /// Action groups: lights, RCS, abort fired, custom 1..10.
    pub lights: bool,
    pub rcs: bool,
    pub abort: bool,
    pub custom_groups: [bool; 10],
    /// RCS: thrust available along one direction (kN, 0 without thrusters) and how hard it is firing
    /// (share of full thrust, translation plus turning).
    pub rcs_thrust_kn: f32,
    pub rcs_firing: f32,
    /// Force of the wings and other lifting surfaces, kN (lift and their drag).
    pub lift_kn: f32,
    /// Angular velocity, rad/s, inertial (with SAS off it is what the vessel keeps spinning at).
    pub angvel: DVec3,
    /// Resource manager: (resource id, amount, capacity) in units for the whole vessel, and for the tanks
    /// the burning stage's engines draw from.
    pub resources: Vec<(String, f64, f64)>,
    pub stage_resources: Vec<(String, f64, f64)>,
    /// The burning stage is jets (their thrust follows speed and air; `thrust_kn` shows it).
    pub jet: bool,
}

/// Where a Kerbal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KerbalStatus {
    Available,
    /// Aboard a vessel (its id).
    Assigned {
        vessel: u64,
    },
    /// Out on EVA as vessel `vessel`.
    Eva {
        vessel: u64,
    },
    /// Aboard a vessel that crashed.
    Missing,
}

/// One of the campaign's astronauts. KSP2 has no traits or experience: a Kerbal is a name, a looks
/// and a personality (0..1 each), as the original's `KerbalAttributes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KerbalInfo {
    pub name: String,
    pub status: KerbalStatus,
    pub veteran: bool,
    /// When they joined, universal time, s.
    pub enrolled_ut: f64,
    pub bravery: f32,
    pub stupidity: f32,
    pub optimism: f32,
    pub constitution: f32,
    /// The original's variety keys: TYPE, HAIRSTYLE, HAIRCOLOR, EYES, SKINCOLOR, HELMET, TEAMCOLOR1 ...
    pub looks: Vec<(String, String)>,
}

/// A place to launch from (KSC launch pads 1..4, the runway).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaunchSite {
    pub id: String,
    pub name: String,
    pub runway: bool,
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Which way a plane faces there, compass degrees (90 = east).
    pub heading_deg: f64,
}

/// Action groups (keys U, R, Backspace, 1..0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionGroup {
    Lights,
    Rcs,
    Abort,
    /// 1..10 (the 0 key is 10).
    Custom(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveAction {
    QuickSave,
    QuickLoad,
    RevertToLaunch,
    RevertToEditor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChuteState {
    Stowed,
    Armed,
    SemiDeployed,
    Deployed,
    Cut,
}

/// One part's pose away from rest: rotate the part by `rotation` (rotation vector, rad, craft frame) about its
/// origin, then move it by `offset_m`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartFlex {
    pub part: u16,
    pub rotation: [f32; 3],
    pub offset_m: [f32; 3],
}

/// How a body spins, index-aligned with `BodyCatalog::bodies`. Body-fixed longitude 0 is at inertial angle
/// `initial_rotation_deg + 360 * ut / rotation_period_s` about +Z.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodySpin {
    /// Sidereal rotation period, s; 0 for a body that does not spin.
    pub rotation_period_s: f64,
    pub initial_rotation_deg: f64,
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
    /// Thrust at sea level (1 atm), kN, and the thrust-to-weight ratio at the stage's start on Kerbin's
    /// surface (9.81 m/s^2).
    pub thrust_asl_kn: f64,
    pub twr_asl: f64,
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
    /// Answer to `SetControls` / `Stage`.
    FlightAck {
        vessel: u64,
        accepted: bool,
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
    /// Answer to `RunExperiments`: new reports now carried, experiments that did not run, and everything the
    /// vessel carries.
    ScienceReports {
        vessel: u64,
        new: Vec<ReportInfo>,
        skipped: Vec<SkippedExperiment>,
        carried: Vec<ReportInfo>,
        reason: Option<String>,
        /// Experiments that take time and are running now (including ones this request started).
        running: Vec<RunningExperiment>,
    },
    /// A running experiment finished (its new reports are now carried) or was interrupted by the vessel
    /// leaving the situation or region it started in. Sent to the vessel's owner.
    ExperimentFinished {
        vessel: u64,
        part: u16,
        experiment: String,
        reports: Vec<ReportInfo>,
        interrupted: bool,
    },
    /// Answer to `TransmitScience` / `RecoverVessel`: what was submitted and the points it earned (0 for a
    /// report the campaign already had).
    ScienceSubmitted {
        vessel: u64,
        reports: Vec<ReportInfo>,
        earned: f64,
        recovered: bool,
        reason: Option<String>,
    },
    /// The campaign's science: free points are `capacity - allocated`. Sent on `ScienceInfo`, after an
    /// unlock or submission, and to everyone whenever it changes (a mission advanced, a teammate scored).
    ScienceState {
        /// False in sandbox: everything is unlocked and points do not matter.
        tech_tree: bool,
        capacity: u32,
        allocated: u32,
        unlocked: Vec<String>,
        submitted: Vec<ReportInfo>,
        missions: Vec<MissionInfo>,
    },
    /// Answer to `GoEva`, `Board` and `PlantFlag`.
    CrewAck {
        vessel: u64,
        accepted: bool,
        reason: Option<String>,
        /// The Kerbal's vessel after `GoEva`, the flag after `PlantFlag`.
        new_vessel: Option<u64>,
    },
    /// Two vessels latched at their docking ports and are one vessel now: `absorbed` joined `vessel`.
    /// Sent to both owners.
    Docked {
        vessel: u64,
        absorbed: u64,
    },
    /// Answer to `Undock`: the part beyond the port flies off as `new_vessel`.
    Undocked {
        vessel: u64,
        new_vessel: Option<u64>,
        reason: Option<String>,
    },
    /// Answer to `AcceptMission` / `TurnInMission`; `science` is the reward a turn-in paid.
    MissionAck {
        id: String,
        accepted: bool,
        reason: Option<String>,
        science: f64,
    },
    /// The campaign's astronauts (answer to `KerbalRoster`, and whenever it changes).
    KerbalRoster {
        kerbals: Vec<KerbalInfo>,
    },
    /// The launch sites, sent after `DifficultyOptions`.
    LaunchSites {
        sites: Vec<LaunchSite>,
    },
    /// Answer to `ClientMessage::StageReport`: per stage, first-firing first, and the whole craft's vacuum
    /// delta-v.
    StageReport {
        stages: Vec<StageReadout>,
        delta_v_vacuum_mps: f64,
        /// Wet centre of mass, and the centre and direction of thrust of the first stage to fire's engines
        /// (zero without engines); craft frame, like `CraftPart::local_position`.
        com_m: DVec3,
        cot_m: DVec3,
        thrust_dir: DVec3,
        /// Centre of lift: the lifting surfaces weighted by their lift coefficient (zero without any).
        col_m: DVec3,
    },
    /// Answer to `QuickSave`, `QuickLoad`, `RevertToLaunch` and `RevertToEditor`; `craft` is the reverted
    /// craft for the editor.
    SaveAck {
        action: SaveAction,
        accepted: bool,
        reason: Option<String>,
        craft: Option<ose_data::craft::Craft>,
    },
    /// Answer to `UnlockTechNode`.
    TechUnlock {
        id: String,
        accepted: bool,
        reason: Option<String>,
    },
    /// Sent right after `BodyCatalog`: each body's rotation.
    BodyRotation {
        bodies: Vec<BodySpin>,
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

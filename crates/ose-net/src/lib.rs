//! Wire protocol between client and server. Encoded with `postcard`, one message per WebSocket binary frame.
//!
//! Session: client sends `Hello`; server answers `Welcome` then `BodyCatalog`; after that the server streams
//! `TimeState` + `Snapshot` at a fixed rate (`SNAPSHOT_HZ`) and the client sends intents (`RequestWarp`).
//! Positions are metres in star space (inertial, Z-up, centred on the root star), see
//! `docs/reference-notes/orbits.md`.

#![forbid(unsafe_code)]

pub mod diagnostics;
pub const BUILD_REVISION: &str = env!("OSE_BUILD_REVISION");

use ose_data::Tick;
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

/// A camera's forward and up, unit vectors (x, y, z).
pub type CameraAxes = ((f32, f32, f32), (f32, f32, f32));
pub const PROTOCOL_VERSION: u16 = 85;
/// Trajectory messages per second the server streams for each vessel.
pub const TRAJECTORY_HZ: u32 = 1;
/// Snapshots per second the server streams to each client.
pub const SNAPSHOT_HZ: u32 = 10;
/// WebSocket path on the server.
pub const WS_PATH: &str = "/ws";

/// Client to server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    RequestEmailOtp {
        email: String,
    },
    EmailSignIn {
        protocol: u16,
        email: String,
        challenge: String,
        code: String,
        display_name: String,
    },
    ShareSession {
        id: String,
    },
    JoinSessionCode {
        code: String,
    },
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
    /// Remove the scheduled burns from a vessel the player owns.
    ClearBurn {
        vessel: u64,
    },
    /// Plan a chain of burns (maneuver nodes) on a vessel the player owns, replacing any: taken in time
    /// order, each in the orbital frame of the trajectory the one before leaves. The trajectory goes on
    /// through every node (`PatchEnd::Maneuver` at each); on rails each is applied at its time, under
    /// power the next one's time ends the one before. An empty list clears them. Answers `BurnState`.
    ScheduleBurns {
        vessel: u64,
        burns: Vec<Burn>,
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
    /// Claim an unowned session. Ownership persists with the player's account.
    ClaimSession {
        id: String,
    },
    /// Delete an empty session owned by this player.
    DeleteSession {
        id: String,
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
    /// Send the vessel's data reports home over its best working antenna (samples have to be recovered):
    /// answers `ScienceSubmitted` with no reports at once (or the reason it cannot), then each report goes
    /// packet by packet at the antenna's rate, drawing its power, while the link holds, and arrives as its
    /// own `ScienceSubmitted` with the points it earned. `FlightReadout::transmitting` shows it going.
    TransmitScience {
        vessel: u64,
    },
    /// Recover a vessel standing on the home world: its reports are submitted and it is removed. Answers
    /// `ScienceSubmitted`.
    RecoverVessel {
        vessel: u64,
    },
    /// Clear a launch site of the player's own vessels before rolling out another (the original's VAB
    /// "Selected launchpad is occupied. Do you want to clear the launchpad?", `ClearVesselsAtSelectedLaunchSite`).
    /// Refused while another player's vessel stands there. Answers `LaunchSiteCleared`.
    ClearLaunchSite {
        site: Option<String>,
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
    /// A Kerbal on EVA standing on the ground plants the agency flag (one per Kerbal, as KSP2's); answers
    /// `CrewAck` with the flag.
    PlantFlag {
        vessel: u64,
    },
    /// A Kerbal on EVA (`vessel`) takes down a planted flag within reach and carries a flag again; answers
    /// `CrewAck`.
    TakeFlag {
        vessel: u64,
        flag: u64,
    },
    /// Undock at a docked joint (the docking port part on either side; the last joint when `None`); answers
    /// `Undocked`.
    Undock {
        vessel: u64,
        port: Option<u16>,
    },
    /// Extend (`deploy`) or retract the vessel's deployable antennas and dishes; answers `FlightAck`. Their
    /// state shows in `FlightReadout::antennas`.
    DeployAntennas {
        vessel: u64,
        deploy: bool,
    },
    /// Move a part (index into the vessel's craft as `GetVesselCraft` gives it) to another stage (index
    /// into its `stages`, as `StageReadout::stage_id` counts them; one past the end adds a stage that fires
    /// first). Stages that have fired neither give nor take parts. Answers `FlightAck`; `craft_rev` changes.
    MoveToStage {
        vessel: u64,
        part: u16,
        stage: u32,
    },
    /// Control the vessel from one of its parts (a command part or docking port; the root to go back):
    /// SAS, the navball and `facing` then follow that part's axis. The craft is turned to match (ask
    /// `GetVesselCraft` again: `craft_rev` changes) but the vessel does not move. Refused on the ground and
    /// for planes. Answers `FlightAck`.
    ControlFrom {
        vessel: u64,
        part: u16,
    },
    /// Move a Kerbal aboard to another crewed part (craft part index, as `FlightReadout::crew` counts);
    /// refused when that part is full or carries no crew. Answers `FlightAck`.
    MoveCrew {
        vessel: u64,
        kerbal: String,
        to_part: u16,
    },
    /// Start moving `resource` from the `from` tanks into the `to` tanks (craft part indices, as
    /// `FlightReadout::part_resources` counts), split as the original's Resource Manager splits it; a part in
    /// both lists is balanced with the others in both. Each tank's share moves in about a second, then it
    /// stops by itself. Empty lists stop it. Answers `FlightAck`; `FlightReadout::transfer` shows it.
    TransferResource {
        vessel: u64,
        resource: String,
        from: Vec<u16>,
        to: Vec<u16>,
    },
    /// Bind a part's actions (`ose_data::craft::ACTIONS`) to action groups (`KSPActionGroup` bits) in
    /// flight, replacing its bindings; an `ActionGroup` then runs them. Answers `FlightAck`.
    SetPartActions {
        vessel: u64,
        part: u16,
        actions: Vec<(String, u32)>,
    },
    /// Change a parachute's deploy mode, opening pressure and full-deploy height in flight (its part window);
    /// the rest of its symmetry set follows. Answers `FlightAck`.
    SetChuteSettings {
        vessel: u64,
        part: u16,
        settings: ose_data::craft::ChuteSettings,
    },
    /// Run one part's action now (its part menu: shut an engine down, extend one panel, ...). Answers
    /// `FlightAck`.
    PartAction {
        vessel: u64,
        part: u16,
        action: String,
        on: bool,
    },
    /// Rename one of the player's vessels (1..=64 characters, trimmed); answers `FlightAck`.
    RenameVessel {
        vessel: u64,
        name: String,
    },
    /// Fold the vessel's deployable solar panels out (`deploy`) or in; answers `FlightAck`.
    SetPartDeployment {
        vessel: u64,
        part: u16,
        deploy: bool,
    },
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
    /// Rotation input, each -1..1 and held until changed: pitch turns
    /// the nose toward craft +Z (the navball's top), yaw toward craft +X, roll clockwise about the nose. The
    /// reaction wheels (and RCS with its group on) set how fast the spin builds. SAS controls turn rate
    /// and brakes on release; with `AttitudeMode::Free` nothing damps it. Answers
    /// `FlightAck`.
    SetRotation {
        vessel: u64,
        pitch: f32,
        yaw: f32,
        roll: f32,
    },
    /// A Kerbal on EVA: `run` (Shift) held until changed, `jump` (Space) once from the ground (on a ladder
    /// it lets go). Its jetpack translates with `Translate` when RCS is enabled (R; y up along the Kerbal,
    /// Ctrl down). `grab` (F, once) takes hold of the nearest ladder within reach (ladder parts and crewed
    /// parts' built-in ladders) or lets go of the one held; `climb` (W/S, -1..1, held) climbs it at the
    /// original's 0.8 m/s. Answers `FlightAck` (refused for a grab with no ladder near).
    EvaControls {
        vessel: u64,
        run: bool,
        jump: bool,
        grab: bool,
        climb: f32,
    },
    /// Control trim (Alt+W/S, A/D, Q/E): pitch, yaw, roll, each -1..1 and held; it adds to the rotation
    /// input and to the control surfaces' deflection. Answers `FlightAck`.
    SetTrim {
        vessel: u64,
        pitch: f32,
        yaw: f32,
        roll: f32,
    },
    /// Turn the aero overlay (F12) on or off for a vessel: its readout then carries `aero_forces`.
    /// Answers `FlightAck`.
    AeroOverlay {
        vessel: u64,
        on: bool,
    },
    /// The craft of any vessel as it stands now (after staging, decoupling, breaking); answers
    /// `VesselCraft`. `VesselState::craft_rev` changes whenever it does.
    GetVesselCraft {
        vessel: u64,
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
    /// Target another vessel or a body for one of the player's vessels (`None` clears it); answers
    /// `FlightAck`. The target shows in `VesselState::target`.
    SetTarget {
        vessel: u64,
        target: Option<TargetId>,
    },
    /// Jettison the vessel's fairings (also done by staging them); answers `FlightAck`.
    DeployFairings {
        vessel: u64,
    },
    /// Open (`open`) or close the vessel's cargo bays; answers `FlightAck`.
    SetCargoBays {
        vessel: u64,
        open: bool,
    },
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
    ConfigurePart {
        vessel: u64,
        part: u16,
        settings: ose_data::flight_geometry::PartControl,
    },
    SetPrecision {
        vessel: u64,
        enabled: bool,
    },
    SetRawRotation {
        vessel: u64,
        pitch: f32,
        yaw: f32,
        roll: f32,
    },
    /// Replace the vessel's whole stage stack (protocol 62): `stages` lists parts of the craft as
    /// `GetVesselCraft` gives it, in `Craft::stages` order (the last fires first), each stageable part once.
    /// Parts move between unfired stages and new stages can go anywhere below the one lit; the stages that
    /// have fired must stay as they are. Answers `FlightAck`; `craft_rev` changes.
    SetStages {
        vessel: u64,
        stages: Vec<Vec<u16>>,
    },
    /// Pilot this vessel: only one player pilots a vessel, the one whose last command went to it, while they
    /// are in the campaign; any other command to it is refused (`FlightAck`/`CrewAck` naming the pilot).
    /// Other players can still ride along and watch. Answers `FlightAck`; `CampaignMember::flying` shows who
    /// pilots what and is pushed to everyone when it changes.
    TakeControl {
        vessel: u64,
    },
    /// Stop piloting (leaving the flight view), so another player can take the vessel.
    ReleaseControl,
    /// The vessel the player's view follows now (a vessel they ride along in, or picked on the map), or none.
    /// The server sends the vessels near it, and near the one the player pilots, in full detail; others go
    /// without their per-part lists.
    Viewing {
        vessel: Option<u64>,
    },
    /// Catch up with `player`'s time (protocol 70): time warp is each player's own, so players drift apart in
    /// time; this runs the sender's time (and the vessel they pilot) forward at the fastest warp their vessels
    /// allow until it reaches `player`'s, and from then on they share one clock. Nothing goes back in time, so
    /// a player behind the sender is refused (they sync to the sender instead). Answered by `Rejected` when
    /// refused; `PlayerTimes` shows the progress.
    SyncTime {
        player: u64,
    },
    /// Delivery routes (protocol 71, `docs/delivery-routes-design.md`): mark one of the player's vessels as a
    /// depot, or unmark the depot it carries. Answers `Routes`.
    SetDepot {
        vessel: u64,
        depot: bool,
    },
    /// Start recording the proving flight of a route: `vessel` is the player's, standing on the pad or runway.
    /// One recording per player at a time. Answers `Routes`.
    RecordRoute {
        vessel: u64,
    },
    /// End the player's recording: docked at the depot it becomes a one-way route (the vessel stays), anywhere
    /// else it is dropped. Answers `Routes`.
    StopRecording,
    /// Remove one of the player's finished or dropped recordings. Answers `Routes`.
    ForgetRecording {
        id: u64,
    },
    /// The campaign's depots and recordings. Answers `Routes`.
    RoutesInfo,
    /// Make a route from a finished one-way recording (protocol 73): at each launch window the server
    /// launches a fresh copy of the craft and flies it to the depot. `name` empty: craft and depot names.
    /// Answers `Routes`.
    CreateRoute {
        recording: u64,
        name: String,
    },
    /// Pause a route (its run in flight carries on) or resume it. Answers `Routes`.
    PauseRoute {
        id: u64,
        paused: bool,
    },
    /// Delete a route; its run in flight is left where it is. Answers `Routes`.
    DeleteRoute {
        id: u64,
    },
    /// What a route's runs do once they have delivered (protocol 74). Answers `Routes`.
    SetRouteEnd {
        id: u64,
        when: RouteEnd,
    },
    /// Found a colony where the player's `vessel` stands (protocol 75, `docs/colonies-design.md`): landed,
    /// crewed and carrying a colony trunk. Answers `Colonies`; the launch sites gain the colony's pad.
    FoundColony {
        vessel: u64,
        name: String,
    },
    /// Give up one of the player's colonies; its vessels stay. Answers `Colonies`.
    AbandonColony {
        id: u64,
    },
    /// The campaign's colonies. Answers `Colonies`.
    ColoniesInfo,
    /// Send home, let go or keep the run a route left docked at its depot (protocol 77). Answers `Routes`.
    RouteStayed {
        id: u64,
        what: StayedRun,
    },
    /// Kept last so the other variants keep their postcard discriminants (tests match on them).
    /// The player's camera while flying a Kerbal on EVA (protocol 82): its forward and up, unit vectors in
    /// the reference body's inertial frame (the same frame as `FlightReadout::facing`), `None` when it is
    /// not to be used. The jetpack pushes along the camera's axes and the Kerbal turns to face along it, as
    /// KSP2's `JetpackKerbalCharacterStateHandler.UpdateInputCameraFeatures`. No reply.
    EvaCamera {
        vessel: u64,
        camera: Option<CameraAxes>,
    },
    /// Destroy one of the player's own vessels from the Tracking Station (the original's Destroy Object,
    /// `MapUISelectableEntry`), wherever it is, with no recovery. Answers `FlightAck` (protocol 84).
    /// Last, so the variants before it keep their postcard discriminants.
    DestroyVessel {
        vessel: u64,
    },
}

/// What to do with the run a route left docked (`ClientMessage::RouteStayed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StayedRun {
    /// Undock it and fly it home.
    SendHome,
    /// Undock it and leave it drifting.
    LetGo,
    /// Keep it as part of the depot; the route no longer counts it.
    Keep,
}

/// What a route's run does once it has delivered (`ClientMessage::SetRouteEnd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteEnd {
    /// Stay docked for good: each run takes a port.
    Stay,
    /// Stay docked until the next run is in orbit, then fly home, freeing the port for it.
    Rotate,
    /// Undock and fly home to KSC, where it is recovered.
    Return,
    /// Undock, back off, and leave it there.
    LetGo,
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
    /// Why it is waiting (away from where it started, too few crew in its part, no charge); it goes on
    /// by itself when that clears.
    pub paused: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkippedExperiment {
    pub part: u16,
    pub experiment: String,
    pub reason: String,
}

/// Something a campaign member did for the shared agency, told to the rest of the campaign (`AgencyNews`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AgencyEvent {
    /// A tech node was researched with the agency's science.
    TechResearched { node: String },
    /// A mission was taken on at Mission Control.
    MissionAccepted { mission: String },
    /// A mission met its last objective (the agency's vessels, nobody in particular).
    MissionCompleted { mission: String },
    /// A completed mission was handed in for its reward.
    MissionTurnedIn { mission: String, science: f64 },
    /// Reports transmitted or recovered earned the agency this much science.
    ScienceEarned { science: f64, reports: u32 },
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
    pub science_available: bool,
    pub reference_body: u16,
    pub position_m: DVec3,
    pub velocity_mps: DVec3,
    /// Acceleration other than gravity (thrust, drag, lift) the vessel had at `tick`, m/s² in the same
    /// frame; zero on rails, on the ground and for wrecks. With `orbit`'s gravity it lets the client
    /// extrapolate a powered vessel to the frame's time (protocol 58).
    pub accel_mps2: DVec3,
    pub orbit: OrbitElements,
    /// Whether motion follows an analytic orbit rather than active physics.
    pub on_rails: bool,
    /// Instruments are retained for crafts in active physics and on rails.
    pub flight: Option<FlightReadout>,
    /// The vessel's target, if it has one.
    pub target: Option<TargetReadout>,
    /// Changes whenever the vessel's craft does (parts lost or gained); ask `GetVesselCraft` again then.
    pub craft_rev: u32,
    /// What is left of the flight readout of a vessel sent far from anything the player views (`flight` is
    /// then `None`): enough for the map and the tracking station (protocol 69).
    pub brief: Option<FlightBrief>,
}

/// A far vessel's flight readout in brief (see `VesselState::brief`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlightBrief {
    pub situation: FlightSituation,
    pub eva: bool,
    pub altitude_m: f64,
}

impl VesselState {
    /// Its flight situation, from the readout or the brief.
    pub fn situation(&self) -> Option<FlightSituation> {
        self.flight
            .as_ref()
            .map(|f| f.situation)
            .or(self.brief.map(|b| b.situation))
    }

    /// Whether it is a Kerbal on EVA, from the readout or the brief.
    pub fn is_eva(&self) -> bool {
        self.flight.as_ref().map(|f| f.eva).unwrap_or(false) || self.brief.is_some_and(|b| b.eva)
    }

    /// Height above the sea of its body, m, when the server gave it.
    pub fn altitude_m(&self) -> Option<f64> {
        self.flight
            .as_ref()
            .map(|f| f.altitude_m)
            .or(self.brief.map(|b| b.altitude_m))
    }
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
    pub owner: Option<u64>,
    pub owner_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampaignMember {
    pub player_id: u64,
    pub display_name: String,
    pub joined_tick: Tick,
    pub agency: Option<Agency>,
    /// In the campaign right now (connected and joined). `CampaignState` is pushed to everyone in it
    /// whenever someone joins or leaves.
    pub online: bool,
    /// The vessel the member last commanded, while it exists.
    pub flying: Option<u64>,
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
    /// The SAS buttons with the navball on surface speed, as KSP2: horizon north / south, straight up / down.
    SurfaceNormal,
    SurfaceAntiNormal,
    SurfaceRadialOut,
    SurfaceRadialIn,
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
    /// Toward / away from the vessel's target (`SetTarget`; refused without one).
    Target,
    AntiTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlightSituation {
    Landed,
    Flying,
    SubOrbital,
    Orbit,
    Escaping,
    Crashed,
    /// In the water: afloat or sitting on the sea floor in the shallows.
    Splashed,
}

/// One solar module in its original craft part index and authored pivot frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolarPanelState {
    pub part: u16,
    pub pivot_node: Option<String>,
    pub angle_deg: f32,
    pub incidence: f32,
    pub flow_ec_per_s: f32,
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
    /// Current wet centre of mass in the physics vessel frame (Unity Z reflected).
    pub centre_of_mass_m: DVec3,
    /// Delivered thrust directions by engine part, in that part's exported model frame.
    pub engine_gimbals: Vec<(u16, DVec3)>,
    /// Per-pivot exported model-space position and rotation delta, after physical response.
    pub gimbal_poses: Vec<(u16, DVec3, [f64; 4])>,
    /// Delivered nozzle power at exported model-space anchors.
    pub rcs_nozzles: Vec<(u16, DVec3, f32)>,
    pub part_controls: Vec<(u16, ose_data::flight_geometry::PartControl)>,
    pub part_control_capabilities: Vec<(u16, u8)>,
    pub control_point: Option<u16>,
    pub control_rotation: [f64; 4],
    pub thrust_kn: f64,
    pub twr: f64,
    pub dynamic_pressure_kpa: f64,
    /// Ambient static pressure, for atmosphere/vacuum visual effects.
    pub static_pressure_kpa: f64,
    /// Physical ambient density (kg/m³), separate from the artist's pressure curve.
    pub atmosphere_density: f64,
    pub mach_number: f64,
    pub gravity_mps2: f64,
    pub atmosphere_temperature_k: f64,
    /// Attached, exposed lifting surfaces: (part index, deflection coefficient, lift kN).
    pub lifting_surfaces: Vec<(u16, f32, f32)>,
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
    /// Crewed parts whose hatch something blocks (KSP2 `IVAPortraitEVAObstacleDetector`): their Kerbals'
    /// EVA buttons are disabled with "EVA is disabled due to an obstacle.".
    #[serde(default)]
    pub eva_blocked: Vec<u16>,
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
    /// Authoritative per-instance tracking, incidence and producer output, including fixed panels.
    pub solar_panels: Vec<SolarPanelState>,
    pub non_retractable: Vec<u16>,
    pub part_lights: Vec<(u16, bool)>,
    /// An EVA Kerbal's single portable flag. Pickup restores this; planting consumes it.
    pub flag_carried: bool,
    /// Each attached deployable antenna's state (extended), by craft part index.
    pub antennas: Vec<(u16, bool)>,
    /// The part the vessel is controlled from, when not its root (`ControlFrom`).
    pub control_part: Option<u16>,
    /// The report being sent home: (experiment, location, share sent 0..1, reports waiting behind it, why
    /// it is held up when it is: no link, no power).
    pub transmitting: Option<(String, String, f32, u16, Option<String>)>,
    /// Flying a planned burn: the delta-v still to go, m/s. Nodes are flown by the engines (pointed along
    /// the node from half its burn time before, burning until it is delivered); passing a node changes
    /// nothing by itself.
    pub burn_left_mps: Option<f32>,
    /// Why the last planned burn stopped short (out of thrust, crashed); the node stays.
    pub burn_stopped: Option<String>,
    /// Engines the pilot has shut down (`PartAction` "engine" off).
    pub engines_off: Vec<u16>,
    /// Multi-mode engines (protocol 76): (part, the mode it runs in, the mode it is changing to while it
    /// waits out a change, Auto Switch on). `PartAction` "mode" picks the first (off) or second (on) mode
    /// for the part and its symmetry set; "automode" sets Auto Switch for the part.
    #[serde(default)]
    pub engine_modes: Vec<(u16, u8, Option<u8>, bool)>,
    /// Engines flamed out (`Module_Engine` `Flameout`): starved of propellant or of intake air.
    #[serde(default)]
    pub flameouts: Vec<u16>,
    /// Ignited engines (protocol 80, `EngineIgnited`) and the thrust each delivers, kN (`FinalThrustValue`):
    /// the part window reads "Nominal" with it; an engine not ignited reads "Off", or "Occluded" while
    /// shielded from the airstream.
    #[serde(default)]
    pub engine_thrust: Vec<(u16, f32)>,
    /// A Kerbal holding a ladder: (vessel, part).
    pub on_ladder: Option<(u64, u16)>,
    /// A Kerbal climbing in through a hatch (`Board`): the vessel is gone a moment later.
    pub boarding: bool,
    /// Each gimballing engine of the stage running and how far it is turned now: (part, about the part's
    /// own X, about its own Z), rad, up to its gimbal range.
    pub gimbals: Vec<(u16, f32, f32)>,
    /// Each attached tank: (part, resource, stored, capacity), units.
    pub part_resources: Vec<(u16, String, f64, f64)>,
    /// The resource transfer running: (resource, from parts, to parts).
    pub transfer: Option<(String, Vec<u16>, Vec<u16>)>,
    /// Fell over on landing and lies on its side.
    pub tipped: bool,
    /// Parachutes: (craft part index, state, how far open 0..1).
    pub parachutes: Vec<(u16, ChuteState, f32)>,
    /// Each attached parachute's deploy safety (`deploymentSafetyState`, shown while packed or armed), its
    /// canopy temperature (K) and whether the canopy burned away (`ParachuteDestroyed`).
    pub chute_safety: Vec<(u16, ChuteSafety, f32, bool)>,
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
    /// Fuel-fed RCS translation and torque requests in the reference body's inertial frame.
    /// Magnitudes are fractions of the available thrust/torque, zero when inactive or dry.
    pub rcs_translate: DVec3,
    pub rcs_rotate: DVec3,
    /// Force of the wings and other lifting surfaces, kN (lift and their drag).
    pub lift_kn: f32,
    /// Angular velocity, rad/s, inertial (with SAS off it is what the vessel keeps spinning at).
    pub angvel: DVec3,
    /// Resource manager: (resource id, amount, capacity) in units for the whole vessel, and for the tanks
    /// the burning stage's engines draw from.
    pub resources: Vec<(String, f64, f64)>,
    pub stage_resources: Vec<(String, f64, f64)>,
    /// Fairings (craft part index, jettisoned), cargo bays (part, open), and the parts they shield from the
    /// airflow (no drag, no re-entry heat).
    pub fairings: Vec<(u16, bool)>,
    pub cargo_bays: Vec<(u16, bool)>,
    pub shielded: Vec<u16>,
    /// Control surfaces and how far each is deflected now, rad (about its hinge; sign follows the
    /// surface's lift axis).
    pub control_surfaces: Vec<(u16, f32)>,
    /// The stages still to come, as the editor's staging list but from the vessel as it is now (what
    /// its tanks hold, here), first to fire first; and their total delta-v here.
    pub stage_list: Vec<StageReadout>,
    pub delta_v_now_mps: f64,
    /// With the aero overlay on: (part, lift kN, drag kN) for the parts the air pushes, inertial.
    pub aero_forces: Vec<(u16, DVec3, DVec3)>,
    pub surface_deployments: Vec<(u16, f32)>,
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
    /// Display name of the owner of a vessel occupying the spawn area.
    #[serde(default)]
    pub occupied_by: Option<String>,
    /// The vessel standing there and its name (`LaunchpadDialog`: the vessel at the launch pad, which the
    /// owner can fly again or recover).
    #[serde(default)]
    pub occupant: Option<u64>,
    #[serde(default)]
    pub occupant_name: Option<String>,
    pub runway: bool,
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Which way a plane faces there, compass degrees (90 = east).
    pub heading_deg: f64,
    /// Protocol 75: the body it stands on (`None`: the home body), and the colony whose pad it is. A craft
    /// launched there is paid for out of the colony's stock and crewed by its residents.
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub colony: Option<u64>,
}

/// One planned impulsive burn (a maneuver node), in the orbital frame at `ut`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Burn {
    pub ut: f64,
    pub prograde_mps: f64,
    pub normal_mps: f64,
    pub radial_mps: f64,
}

/// Something a vessel can target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetId {
    Vessel(u64),
    /// A body by its `BodyCatalog` index.
    Body(u16),
}

/// A vessel's target as seen from it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetReadout {
    pub target: TargetId,
    /// Distance, m (to a body's surface), and the target's velocity relative to the vessel, m/s, inertial.
    pub distance_m: f64,
    pub relative_velocity_mps: DVec3,
    /// How fast they close, m/s (negative moving apart).
    pub closing_mps: f64,
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

/// Whether an armed parachute may open now (`Data_Parachute.DeploymentSafeStates`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChuteSafety {
    Safe,
    Risky,
    Unsafe,
    /// Out of the air, or already open.
    None,
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
    /// In flight (`FlightReadout::stage_list`): thrust, TWR and delta-v where the vessel is now (the air
    /// pressure and gravity there); 0 in the editor.
    pub thrust_now_kn: f64,
    pub twr_now: f64,
    pub delta_v_now_mps: f64,
}

/// Server to client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    EmailOtpSent {
        email: String,
        challenge: String,
        expires_in_secs: u64,
        retry_after_secs: u64,
    },
    AuthError {
        reason: String,
    },
    AccountState {
        email: Option<String>,
    },
    SessionShareCode {
        id: String,
        code: String,
    },
    SessionJoined {
        id: String,
        name: String,
    },
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
    /// Live session browser and campaign presence, pushed when membership or ownership changes.
    SessionPresence {
        sessions: Vec<SessionInfo>,
        campaign_id: String,
        members: Vec<CampaignMember>,
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
    /// Answer to `GetVesselCraft` (`None` for a vessel without parts, like a bare probe or a flag).
    VesselCraft {
        vessel: u64,
        craft: Option<ose_data::craft::Craft>,
    },
    /// The launch sites, sent after `DifficultyOptions`.
    LaunchSites {
        sites: Vec<LaunchSite>,
    },
    /// Answer to `ClearLaunchSite`: how many vessels were removed, or why not.
    LaunchSiteCleared {
        site: String,
        cleared: u32,
        reason: Option<String>,
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
    /// The campaign is one shared agency: what a teammate just did for it (protocol 69), sent to the other
    /// players in the campaign (the one who did it has their own answer). `by` is their display name, `None`
    /// for the agency as a whole. The `ScienceState` with the change follows on its own.
    AgencyNews {
        by: Option<String>,
        event: AgencyEvent,
    },
    /// Where the other players are in time (protocol 70), once a second while anyone is in another time
    /// than the receiver: time warp is each player's own. Empty once everyone shares one clock again.
    PlayerTimes {
        players: Vec<PlayerTime>,
        /// Catching up with this player's time now (`ClientMessage::SyncTime`).
        syncing_to: Option<u64>,
    },
    /// The campaign's depots and proving flights (protocol 71): the answer to the routes messages, and sent to
    /// a player when their recording moves on (a phase, a burn, docking, landing, or its end). `reason` says
    /// why a request was refused.
    Routes {
        depots: Vec<DepotInfo>,
        recordings: Vec<RecordingInfo>,
        /// Protocol 73.
        routes: Vec<RouteInfo>,
        reason: Option<String>,
    },
    /// The campaign's colonies (protocol 75): the answer to the colony messages, and sent to everyone when
    /// one is founded or abandoned. `reason` says why a request was refused.
    Colonies {
        colonies: Vec<ColonyInfo>,
        reason: Option<String>,
    },
}

/// A colony (`ServerMessage::Colonies`, protocol 75): a site on a body whose landed vessels belong to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColonyInfo {
    pub id: u64,
    pub name: String,
    pub owner: u64,
    pub owner_name: String,
    pub body: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub founded_ut: f64,
    /// Crafts launched from its pad.
    pub launches: u32,
    /// Its vessels, as streamed (vessels in another player's time are left out).
    pub vessels: Vec<u64>,
    /// The Kerbals aboard its vessels.
    pub residents: Vec<String>,
    /// What its vessels' tanks hold: (resource, stored, capacity).
    pub stock: Vec<(String, f64, f64)>,
    /// Protocol 79, population: the empty seats in its vessels, the Kerbals who moved in on their own, and how long until
    /// the next one does (s; none without room or without anyone living there).
    #[serde(default)]
    pub vacancies: u32,
    #[serde(default)]
    pub arrivals: u32,
    #[serde(default)]
    pub next_arrival_in_s: Option<f64>,
    /// Protocol 83: an orbital colony, wherever its anchor vessel (the first of `vessels`) is; its site
    /// (`lat_deg`, `lon_deg`) means nothing, and its pad is a dock in space behind the anchor.
    #[serde(default)]
    pub orbital: bool,
}

/// A delivery route (`ServerMessage::Routes`, protocol 73).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteInfo {
    pub id: u64,
    pub name: String,
    pub owner: u64,
    /// The recording it was made from.
    pub recording: u64,
    /// The depot its runs dock at (0 for a route to a colony).
    pub depot: u64,
    /// Protocol 78: the colony its runs land at instead (`ColonyInfo::id`); its runs stay there.
    #[serde(default)]
    pub colony: Option<u64>,
    /// What it is doing or waiting for ("Next launch in 1200 s", "Waiting: no free port at Station").
    pub status: String,
    pub paused: Option<String>,
    pub next_launch_ut: Option<f64>,
    /// The run in flight: its vessel, if it is still one of its own.
    pub run_vessel: Option<u64>,
    /// What each run pumps into the depot.
    pub cargo: Vec<(String, f64)>,
    /// Protocol 74: what its runs do once delivered, and the run left docked at the depot.
    pub when_delivered: RouteEnd,
    pub stayed: Option<u32>,
    /// Protocol 77: the player flying the run by hand now (`TakeControl` of its vessel; `ReleaseControl`
    /// hands it back to the route pilot, which carries on from where it is).
    pub manual: Option<u64>,
    /// Protocol 81: a depot-to-depot shuttle's origin depot (`DepotInfo::id`): one vessel fills up there,
    /// flies to `depot` and back, round and round. `shuttle_leg` says where it is: Loading, Outbound,
    /// Unloading or Inbound. Its finished trips are `runs`.
    #[serde(default)]
    pub origin: Option<u64>,
    #[serde(default)]
    pub shuttle_leg: Option<String>,
    /// Finished runs, oldest first.
    pub runs: Vec<RouteRunInfo>,
}

/// A finished run of a route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRunInfo {
    pub n: u32,
    pub vessel_name: String,
    pub launched_ut: f64,
    pub ended_ut: f64,
    /// Delivered, Short, Missed or Lost.
    pub outcome: String,
    /// Why it missed or was lost.
    pub reason: Option<String>,
    pub delivered: Vec<(String, f64)>,
    /// Protocol 74: what it did after: "docked", "let go", "flying home", "recovered", or what went wrong.
    pub after: Option<String>,
    /// Protocol 77: the players who took control of it on the way.
    pub flown_by: Vec<u64>,
}

/// A vessel marked as a depot (`ServerMessage::Routes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepotInfo {
    pub id: u64,
    pub name: String,
    pub owner: u64,
    /// The vessel carrying it now (`None` when it is in another player's time or gone).
    pub vessel: Option<u64>,
}

/// A proving flight, recorded or being recorded (`ServerMessage::Routes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingInfo {
    pub id: u64,
    pub player: u64,
    /// The craft's name.
    pub craft: String,
    /// The vessel carrying it now, while recording.
    pub vessel: Option<u64>,
    /// Pad, Ascent, Transfer, Approach, Docked, Return or Descent.
    pub phase: String,
    /// Recording, RoundTrip, OneWay, Landed (at a colony, protocol 78), or Dropped.
    pub state: String,
    /// Why it was dropped.
    pub dropped: Option<String>,
    /// The depot's name, once arrived.
    pub depot: Option<String>,
    pub duration_s: f64,
    /// Resources used from its own tanks (burned or transferred), and what the depot gained.
    pub used: Vec<(String, f64)>,
    pub delivered: Vec<(String, f64)>,
    /// How many of its parts stayed docked at the depot.
    pub payload_parts: u32,
    pub burns: u32,
    /// Protocol 81: the depot a shuttle's proving flight left from (it was started in orbit next to it);
    /// a route made from it is a shuttle between the two.
    #[serde(default)]
    pub origin: Option<String>,
}

/// Another player's time, against the receiver's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerTime {
    pub player: u64,
    /// Seconds ahead of the receiver (negative: behind). Zero: the same clock.
    pub ahead_s: f64,
    /// Their time warp now.
    pub warp: u32,
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

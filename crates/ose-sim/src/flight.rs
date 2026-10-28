//! Powered flight in the reference body's inertial frame: translation, attitude and ground response.
//!
//! Part-backed craft receive fuel-resolved engine forces, aerodynamic forces and moments, and current
//! inertia from `structure`. Vessels without parts retain the simplified [`FlightSpec`] propulsion and
//! attitude path. Part-backed surface forces and corner sweeps live in `surface`; the clearance proxy
//! remains for parked/rolling craft and vessels without parts.
//!
//! Frame: body-centred, inertial, Z-up, the body spinning about +Z at `2 pi / rotation_period`. Surface
//! velocity is `v - omega x r`. Atmosphere density follows the body's pressure and temperature curves
//! (`docs/reference-notes/physics-joints-drag.md`). Drag uses summed part `Cd A`, or the spec's fallback area.

use crate::bodies::Body;
use bevy_ecs::prelude::*;
use ose_data::{Curve, G0};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

pub const IDEAL_GAS_CONSTANT: f64 = 8.31447;
/// Relative impact speed above this counts as a crash (wheel touchdown uses sink speed).
pub const CRASH_SPEED_MPS: f64 = 12.0;
/// Highest warp allowed while any vessel is under powered flight (the original's physics warp).
pub const MAX_PHYSICS_WARP: u32 = 4;
/// Drag coefficient for the single reference area of a craft (tuned later against drag cubes).
pub const DEFAULT_CD: f64 = 0.35;

/// Fastest turn rate the attitude controller commands, rad/s.
pub const MAX_TURN_RATE: f64 = 1.0;

/// One stage of a craft as the flight model sees it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageSpec {
    pub stage_id: u32,
    /// Vacuum thrust of the engines running in this stage, kN.
    pub thrust_vac_kn: f64,
    /// Thrust at one atmosphere, kN (Isp scales thrust at constant mass flow).
    pub thrust_asl_kn: f64,
    pub isp_vac_s: f64,
    pub isp_asl_s: f64,
    /// Propellant this stage can burn, t.
    pub propellant_t: f64,
    /// Total mass at the start of the stage with full propellant, t.
    pub start_mass_t: f64,
    /// Transverse moment of inertia about the centre of mass at the start of the stage, t*m^2.
    /// 0 means "no rotation model": the vessel snaps to its commanded attitude.
    #[serde(default)]
    pub moi_tm2: f64,
    /// Control torque from reaction wheels, kN*m (available whenever the vessel is controlled).
    #[serde(default)]
    pub wheel_torque_knm: f64,
    /// Control torque from engine gimbals at full thrust, kN*m (scales with throttle).
    #[serde(default)]
    pub gimbal_torque_knm: f64,
}

/// Rotation properties of one stage, computed from the craft geometry (see the server's flight spec builder).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageInertia {
    pub moi_tm2: f64,
    pub wheel_torque_knm: f64,
    pub gimbal_torque_knm: f64,
}

/// The craft reduced to what the point-mass model needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlightSpec {
    /// Stages in firing order (first to fire first).
    pub stages: Vec<StageSpec>,
    /// Reference area for drag, m^2.
    pub drag_area_m2: f64,
    pub drag_cd: f64,
}

impl FlightSpec {
    /// Build from the staging report; `drag_area_m2` is the largest part cross-section (rough).
    pub fn from_report(
        report: &ose_data::staging::DeltaVReport,
        isp_asl_by_stage: &[f64],
        inertia_by_stage: &[StageInertia],
        drag_area_m2: f64,
    ) -> FlightSpec {
        let mut stages = Vec::new();
        for (i, s) in report.stages.iter().enumerate() {
            if s.thrust_kn <= 0.0 && s.start_mass_t <= 0.0 {
                continue;
            }
            let prop = (s.start_mass_t - s.end_mass_t).max(0.0);
            let isp_vac = if s.delta_v_vacuum_mps > 0.0
                && s.end_mass_t > 0.0
                && s.start_mass_t > s.end_mass_t
            {
                s.delta_v_vacuum_mps / (G0 * (s.start_mass_t / s.end_mass_t).ln())
            } else {
                0.0
            };
            let isp_asl = isp_asl_by_stage.get(i).copied().unwrap_or(isp_vac);
            let thrust_asl = if isp_vac > 0.0 {
                s.thrust_kn * isp_asl / isp_vac
            } else {
                0.0
            };
            stages.push(StageSpec {
                stage_id: s.stage_id as u32,
                thrust_vac_kn: s.thrust_kn,
                thrust_asl_kn: thrust_asl,
                isp_vac_s: isp_vac,
                isp_asl_s: isp_asl,
                propellant_t: prop,
                start_mass_t: s.start_mass_t,
                moi_tm2: inertia_by_stage.get(i).map(|x| x.moi_tm2).unwrap_or(0.0),
                wheel_torque_knm: inertia_by_stage
                    .get(i)
                    .map(|x| x.wheel_torque_knm)
                    .unwrap_or(0.0),
                gimbal_torque_knm: inertia_by_stage
                    .get(i)
                    .map(|x| x.gimbal_torque_knm)
                    .unwrap_or(0.0),
            });
        }
        FlightSpec {
            stages,
            drag_area_m2: drag_area_m2.max(0.1),
            drag_cd: DEFAULT_CD,
        }
    }
}

/// How the vessel points. Surface directions use the local up / east / north frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Attitude {
    /// Radially outward.
    Up,
    SurfacePrograde,
    SurfaceRetrograde,
    OrbitPrograde,
    OrbitRetrograde,
    Normal,
    AntiNormal,
    RadialOut,
    RadialIn,
    /// Pitch above the horizon (degrees) and compass heading (degrees, 90 = east), surface frame.
    PitchHeading {
        pitch_deg: f64,
        heading_deg: f64,
    },
    /// A fixed direction in the body's inertial frame (normalised on use).
    Fixed(DVec3),
    /// Toward the scheduled burn (the universe keeps the direction current; normalised on use).
    Maneuver(DVec3),
    /// SAS off: nothing holds the attitude; the rotation input turns the vessel and it keeps spinning.
    Free,
    /// Toward the vessel's target, or away from it (the universe keeps the direction current).
    Target(DVec3),
    AntiTarget(DVec3),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Situation {
    Landed,
    Flying,
    SubOrbital,
    Orbit,
    Escaping,
    Crashed,
}

/// Powered-flight state. While present the vessel is integrated here and its `OnRails` orbit is refreshed
/// from `(r, v)` each step so everything that reads orbits keeps working.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Powered {
    pub spec: FlightSpec,
    /// Index into `spec.stages` of the stage currently able to burn; `>= len` when all are spent.
    pub stage: usize,
    pub propellant_t: f64,
    pub mass_t: f64,
    pub r: DVec3,
    pub v: DVec3,
    pub throttle: f64,
    pub attitude: Attitude,
    pub landed: bool,
    pub crashed: bool,
    /// Universal time since which the vessel has been coasting above the atmosphere (for the rails handoff).
    pub coasting_since: Option<f64>,
    /// Thrust axis, unit vector in the body-centred inertial frame.
    #[serde(default = "default_facing")]
    pub facing: DVec3,
    /// Transverse angular velocity, rad/s, inertial; axial spin is stored in `roll_rate`.
    #[serde(default)]
    pub angvel: DVec3,
    /// Fraction of the current stage's engines still attached (engines can break off).
    #[serde(default = "one")]
    pub thrust_scale: f64,
    /// Share of supplied thrust from engines that cannot be throttled (solid boosters).
    #[serde(default)]
    pub locked_thrust_fraction: f64,
    /// Drag `Cd * A` from the part drag cubes for the current airflow, m^2; `None` uses the spec's
    /// reference area and coefficient.
    #[serde(default)]
    pub drag_cda_m2: Option<f64>,
    /// Fraction of the reaction wheel torque that has power.
    #[serde(default = "one")]
    pub wheel_scale: f64,
    /// False for an uncrewed vessel whose probe cores have no power: no steering, no throttle.
    #[serde(default = "yes")]
    pub controlled: bool,
    /// The first stage has been fired. A craft on the launch pad starts unlit, as in the original: the
    /// throttle does nothing until the first stage press lights it (and that press drops nothing).
    #[serde(default = "yes")]
    pub ignited: bool,
    /// How the craft meets the ground, from its parts (0 without a part structure: a point at the centre of
    /// mass, crashing above `CRASH_SPEED_MPS`): centre of mass height above the lowest point standing upright,
    /// the radius of what it stands on, and the crash tolerance of the parts it lands on.
    #[serde(default)]
    pub ground_clearance_m: f64,
    #[serde(default)]
    pub footprint_m: f64,
    #[serde(default)]
    pub crash_tolerance_mps: f64,
    /// Fell over on landing: lying on its side, it cannot lift off.
    #[serde(default)]
    pub tipped: bool,
    /// A Kerbal on EVA: on the ground, the throttle with a level attitude walks instead of firing the pack.
    #[serde(default)]
    pub walker: bool,
    /// Driving on wheels (rovers).
    #[serde(default)]
    pub drive: Drive,
    /// Roll about the vessel axis from the horizon reference, radians, and the roll input (-1..1, Q/E).
    #[serde(default)]
    pub roll_rad: f64,
    #[serde(default)]
    pub roll_input: f64,
    /// Action groups (lights, RCS, abort, custom 1..10).
    #[serde(default)]
    pub groups: ActionGroups,
    /// Rotation input with SAS off (`Attitude::Free`), each -1..1, held: pitch turns the nose toward
    /// craft +Z, yaw toward craft +X, roll is clockwise about the nose. And the roll rate it builds, rad/s.
    #[serde(default)]
    pub rotation_input: DVec3,
    #[serde(default)]
    pub roll_rate: f64,
    /// RCS thrusters (with the RCS group on they translate the vessel and help turn it).
    #[serde(default)]
    pub rcs: RcsState,
    /// A Kerbal on EVA: running (held) and a jump asked for (taken on the ground).
    #[serde(default)]
    pub eva_run: bool,
    #[serde(default)]
    pub eva_jump: bool,
    /// In the air from a jump (the throttle still walks, it does not fire the pack).
    #[serde(default)]
    pub eva_hop: bool,
    /// Force of the wings and other lifting surfaces this step (lift and their drag), kN, inertial, and the
    /// airflow's torque about the centre of mass, kN·m, inertial.
    #[serde(default)]
    pub lift_kn: DVec3,
    #[serde(default)]
    pub aero_torque_knm: DVec3,
    /// Steering torque the control surfaces can give in the airflow now, kN·m, and the steering asked for
    /// this step as a share of the full turning torque about each inertial axis (-1..1; the control surfaces
    /// deflect to it).
    #[serde(default)]
    pub surface_torque_knm: f64,
    #[serde(default)]
    pub steer_cmd: DVec3,
    /// The burning stage's engines breathe air (jets), and whether the vessel has intakes.
    #[serde(default)]
    pub jet: Option<Jet>,
    #[serde(default)]
    pub intakes: bool,
    /// Lies level on the ground (a plane): its axis along its compass heading (`drive.heading_rad`), its
    /// craft +Z down.
    #[serde(default)]
    pub level: bool,
}

/// How a jet's thrust follows the flight: against Mach and against air density (kg/m^3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Jet {
    pub vel_curve: Option<Curve>,
    pub atm_curve: Option<Curve>,
}

impl Jet {
    /// Share of rated thrust at `mach` in air of `density`; nothing without oxygen or intakes.
    pub fn factor(&self, mach: f64, density: f64, oxygen: bool, intakes: bool) -> f64 {
        if !oxygen || !intakes || density <= 0.0 {
            return 0.0;
        }
        let v = self
            .vel_curve
            .as_ref()
            .map(|c| c.evaluate(mach))
            .unwrap_or(1.0);
        let a = self
            .atm_curve
            .as_ref()
            .map(|c| c.evaluate(density))
            .unwrap_or(1.0);
        (v * a).max(0.0)
    }
}

/// A vessel's RCS: what its thrusters can do (from its parts) and how they are used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RcsState {
    /// Thrust the thrusters give along any one direction, kN, and the torque they give about the centre
    /// of mass, kN·m.
    pub thrust_kn: f64,
    pub torque_knm: f64,
    /// Isp in vacuum and at sea level, s.
    pub isp_vac_s: f64,
    pub isp_asl_s: f64,
    /// Translation input in craft axes (x right, y fore along the nose, z top), each -1..1, held.
    pub translate: DVec3,
    /// Share of full thrust used this step (translation plus turning), and the share of the propellant
    /// that was delivered last step (0..1).
    pub usage: f64,
    pub powered: f64,
}

impl Default for RcsState {
    fn default() -> Self {
        RcsState {
            thrust_kn: 0.0,
            torque_knm: 0.0,
            isp_vac_s: 0.0,
            isp_asl_s: 0.0,
            translate: DVec3::ZERO,
            usage: 0.0,
            powered: 1.0,
        }
    }
}

/// Local north (east when the vessel axis is north-south) made perpendicular to `facing`: the craft's top
/// with no roll.
pub fn roll_reference(facing: DVec3, up: DVec3) -> DVec3 {
    let (_, east, north) = surface_frame(up);
    for d in [north, east] {
        let across = d - facing * d.dot(facing);
        if across.length_squared() > 1e-6 {
            return across.normalized();
        }
    }
    up
}

/// The craft's +Z in flight: the roll reference turned by its roll. For a level craft (a plane, whose +Z
/// is its belly) the reference is down across the axis, so it flies wings level.
pub fn vessel_top(p: &Powered) -> DVec3 {
    let up = p.r.normalized();
    let base = if p.level {
        let down = -up - p.facing * (-up).dot(p.facing);
        if down.length_squared() > 1e-6 {
            down.normalized()
        } else {
            roll_reference(p.facing, up)
        }
    } else {
        roll_reference(p.facing, up)
    };
    let side = p.facing.cross(base);
    let (sr, cr) = p.roll_rad.sin_cos();
    base * cr + side * sr
}

/// Right-handed craft axes in the inertial frame, including roll.
pub fn vessel_basis(p: &Powered) -> (DVec3, DVec3, DVec3) {
    let top = vessel_top(p);
    (p.facing.cross(top), p.facing, top)
}

pub(crate) fn set_vessel_top(p: &mut Powered, top: DVec3) {
    p.roll_rad = 0.0;
    let base = vessel_top(p);
    p.roll_rad = top.dot(p.facing.cross(base)).atan2(top.dot(base));
}

/// How fast a vessel rolls at full input, rad/s (scaled by its reaction wheels' power).
pub const ROLL_RATE_RAD_S: f64 = 0.5;

/// A vessel's action groups and what they are set to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionGroups {
    pub lights: bool,
    pub rcs: bool,
    /// Abort fired: throttle cut and parachutes armed.
    pub abort: bool,
    /// Custom groups 1..10 (the 0 key is the tenth).
    pub custom: [bool; 10],
}

/// A vessel's wheels and how it is being driven. On the ground the wheel motors push it along its drive
/// direction, the steering wheels turn it, and brakes and rolling resistance slow it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drive {
    /// The wheels it stands on (from its parts; empty without any).
    pub wheels: Vec<DriveWheel>,
    /// Which way it drives, craft axes (x, z): the root part's pointing direction laid flat.
    pub forward: [f64; 2],
    /// Spread of the wheels along that direction, m (sets how tightly it turns).
    pub wheelbase_m: f64,
    /// Wheel throttle (-1 back .. 1 forward), steering (-1 left .. 1 right) and brakes.
    pub throttle: f64,
    pub steer: f64,
    pub brakes: bool,
    /// Speed along the drive direction, m/s, and the compass heading of the craft's +Z axis, radians
    /// clockwise from north.
    pub speed_mps: f64,
    pub heading_rad: f64,
    /// Share of the motors' charge that was delivered (0..1).
    pub powered: f64,
}

impl Default for Drive {
    fn default() -> Self {
        Drive {
            wheels: Vec::new(),
            forward: [0.0, 1.0],
            wheelbase_m: 1.0,
            throttle: 0.0,
            steer: 0.0,
            brakes: false,
            speed_mps: 0.0,
            heading_rad: 0.0,
            powered: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriveWheel {
    pub radius_m: f64,
    /// Drive torque against ground speed, kN·m (`None`: a free wheel), and its charge use at full drive.
    pub torque_knm: Option<ose_data::Curve>,
    pub ec_per_s: f64,
    pub brake_knm: f64,
    pub steers: bool,
}

/// How far the steering wheels turn at full input, radians.
pub const MAX_STEER_RAD: f64 = 0.5;
/// Tyre grip (share of the weight a wheel can push with) and rolling resistance.
pub const WHEEL_GRIP: f64 = 1.0;
pub const ROLLING_RESISTANCE: f64 = 0.02;

/// A plane on its wheels rotates its nose up above this ground speed, at most this far, rad.
pub const ROTATE_SPEED_MPS: f64 = 20.0;
pub const MAX_ROTATE_RAD: f64 = 0.21;

/// The horizontal direction of compass heading `heading_rad` (clockwise from north) at `r`.
pub fn heading_dir(r: DVec3, heading_rad: f64) -> DVec3 {
    let (_, east, north) = surface_frame(r);
    north * heading_rad.cos() + east * heading_rad.sin()
}

/// The compass heading (clockwise from north) of a direction at `r`.
pub fn heading_of(r: DVec3, d: DVec3) -> f64 {
    let (_, east, north) = surface_frame(r);
    d.dot(east)
        .atan2(d.dot(north))
        .rem_euclid(std::f64::consts::TAU)
}

impl Drive {
    /// The craft's +Z axis on the ground at `r` (the roll reference of a standing craft).
    pub fn roll_ref(&self, r: DVec3) -> DVec3 {
        let (_, east, north) = surface_frame(r);
        north * self.heading_rad.cos() + east * self.heading_rad.sin()
    }

    /// The direction it drives in on the ground at `r`.
    pub fn direction(&self, r: DVec3) -> DVec3 {
        let (up, _, _) = surface_frame(r);
        let z = self.roll_ref(r);
        let x = up.cross(z);
        (z * self.forward[1] + x * self.forward[0]).normalized()
    }

    /// One step on the ground for a vessel of `mass_t` under gravity `g`: the speed it ends with and how far
    /// it went (m, along `direction`); the heading turns with the steering.
    pub fn step(&mut self, mass_t: f64, g: f64, slope_acc: f64, dt: f64) -> f64 {
        if self.wheels.is_empty() || mass_t <= 0.0 {
            self.speed_mps = 0.0;
            return 0.0;
        }
        let v = self.speed_mps;
        let input = self.throttle.clamp(-1.0, 1.0) * self.powered.clamp(0.0, 1.0);
        let push: f64 = self
            .wheels
            .iter()
            .filter_map(|w| {
                let t = w.torque_knm.as_ref()?;
                let last = t.keys.last().map(|k| k.t).unwrap_or(0.0);
                let torque = if v.abs() >= last {
                    0.0
                } else {
                    t.evaluate(v.abs()).max(0.0)
                };
                (w.radius_m > 0.0).then(|| torque / w.radius_m)
            })
            .sum::<f64>()
            * input;
        let grip = WHEEL_GRIP * mass_t * g;
        // the slope pulls it downhill on top of what the motors do
        let push = push.clamp(-grip, grip) + slope_acc * mass_t;
        let brake: f64 = if self.brakes {
            self.wheels
                .iter()
                .filter(|w| w.radius_m > 0.0)
                .map(|w| w.brake_knm / w.radius_m)
                .sum::<f64>()
                .min(grip)
        } else {
            0.0
        };
        let resist = (brake + ROLLING_RESISTANCE * mass_t * g) / mass_t * dt;
        let mut nv = v + push / mass_t * dt;
        if nv.abs() <= resist && push.abs() <= resist * mass_t / dt {
            nv = 0.0;
        } else {
            nv -= nv.signum() * resist.min(nv.abs());
        }
        self.speed_mps = nv;
        if self.wheels.iter().any(|w| w.steers) {
            let yaw = nv * (self.steer.clamp(-1.0, 1.0) * MAX_STEER_RAD).tan()
                / self.wheelbase_m.max(0.5);
            self.heading_rad = (self.heading_rad + yaw * dt).rem_euclid(std::f64::consts::TAU);
        }
        nv * dt
    }
}

/// A Kerbal's walking speed at full throttle, m/s (the original's run is about this).
pub const WALK_SPEED_MPS: f64 = 0.8;
/// Running (Shift) and the jump: the original's `RunMoveSpeed`; the jump's push up is our own (the original
/// pushes for `JumpInitialForceDuration`), forward it carries the run times `JumpRunMoveForceMagnitudeMultiplier`.
pub const RUN_SPEED_MPS: f64 = 2.2;
pub const JUMP_SPEED_MPS: f64 = 2.5;
pub const JUMP_RUN_MULTIPLIER: f64 = 1.2;

fn yes() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

/// The flight state of a vessel that is on rails: stage, propellant and mass are kept so the vessel can
/// be put back under power (a burn from orbit, a descent) with what it had left. `r`/`v` are stale while
/// dormant and are refreshed from the rails orbit on wake.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dormant(pub Powered);

#[derive(Clone, Debug, PartialEq)]
pub struct Atmo {
    pub depth_m: f64,
    /// Jets breathe it.
    pub oxygen: bool,
    pub pressure_kpa: Curve,
    pub temperature_k: Curve,
    pub molar_mass: f64,
}

impl Atmo {
    pub fn pressure_kpa(&self, alt_m: f64) -> f64 {
        if alt_m >= self.depth_m {
            0.0
        } else {
            self.pressure_kpa.evaluate(alt_m).max(0.0)
        }
    }
    /// Speed of sound, m/s (ideal diatomic gas).
    pub fn speed_of_sound(&self, alt_m: f64) -> f64 {
        let t = self.temperature_k.evaluate(alt_m.min(self.depth_m));
        if t <= 0.0 || self.molar_mass <= 0.0 {
            return 0.0;
        }
        (1.4 * IDEAL_GAS_CONSTANT * t / self.molar_mass).sqrt()
    }

    pub fn density(&self, alt_m: f64) -> f64 {
        let p = self.pressure_kpa(alt_m);
        let t = self.temperature_k.evaluate(alt_m);
        if p <= 0.0 || t <= 0.0 {
            0.0
        } else {
            p * 1000.0 * self.molar_mass / (IDEAL_GAS_CONSTANT * t)
        }
    }
}

/// Terrain height model attached to bodies that have a height map.
#[derive(Component, Clone, Debug)]
pub struct Terrain(pub std::sync::Arc<ose_data::terrain::TerrainMap>);

/// Rotation angle of a body at `ut`, radians (body-fixed longitude 0 is at this inertial angle).
pub fn rotation_angle(body: &Body, ut: f64) -> f64 {
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    body.initial_rotation_deg.to_radians() + omega * ut
}

/// Body-fixed latitude and longitude (degrees) of a body-centred inertial position at `ut`.
pub fn lat_lon(body: &Body, r: DVec3, ut: f64) -> (f64, f64) {
    let m = r.length().max(1e-9);
    let lat = (r.z / m).clamp(-1.0, 1.0).asin().to_degrees();
    let lon = (r.y.atan2(r.x) - rotation_angle(body, ut)).to_degrees();
    let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
    (lat, lon)
}

/// Radius of the ground (terrain or sea) under `r` at `ut`, m from the body centre.
pub fn ground_radius(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    r: DVec3,
    ut: f64,
) -> f64 {
    let (lat, lon) = lat_lon(body, r, ut);
    body.radius_m + ground_height_at(body, terrain, lat, lon)
}

/// Normal of the terrain, ocean or site surface, evaluated in inertial coordinates.
pub fn ground_normal(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    r: DVec3,
    ut: f64,
) -> DVec3 {
    let (up, east, north) = surface_frame(r);
    let slope = |axis: DVec3| {
        (ground_radius(body, terrain, r + axis, ut) - ground_radius(body, terrain, r - axis, ut))
            * 0.5
    };
    (up - east * slope(east) - north * slope(north)).normalized()
}

/// Whether a grounded craft can safely use the long-step co-rotation optimization.
pub fn resting_on_ground(
    p: &Powered,
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    ut: f64,
) -> bool {
    if body.ocean.is_some()
        && crate::surface::solid_radius(body, terrain, p.r, ut) < body.radius_m - 1e-5
    {
        return false;
    }
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let spin = DVec3::new(0.0, 0.0, omega);
    let normal = ground_normal(body, terrain, p.r, ut);
    let gravity = p.r * (-body.mu / p.r.length().powi(3)) - spin.cross(spin.cross(p.r));
    let downhill = gravity - normal * gravity.dot(normal);
    (p.v - spin.cross(p.r)).length() < 1e-6
        && crate::contact::can_rest(gravity, normal)
        && (p.drive.wheels.is_empty() || p.drive.brakes || downhill.length() < 1e-6)
}

fn ground_crossing(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    from: DVec3,
    to: DVec3,
    clearance: f64,
    ut: f64,
    dt: f64,
) -> Option<f64> {
    let path = to - from;
    let distance = path.length();
    let ceiling = (body.radius_m + terrain.map(|t| t.max_m.max(0.0)).unwrap_or(0.0)).max(
        body.site
            .as_ref()
            .map(|s| s.surface_radius_bound(body.radius_m))
            .unwrap_or(body.radius_m),
    );
    if from.length().min(to.length()) > ceiling + clearance + distance {
        return None;
    }
    crate::contact::sweep(distance, |t| {
        let r = from + path * t;
        r.length() - ground_radius(body, terrain, r, ut + dt * t) - clearance
    })
}

/// Height of the ground above the nominal radius at a body-fixed latitude/longitude: a built site's
/// surface where there is one (the KSC campus), else the terrain (or sea).
pub fn ground_height_at(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    lat_deg: f64,
    lon_deg: f64,
) -> f64 {
    if let Some(r) = body
        .site
        .as_ref()
        .and_then(|s| s.ground_radius(body.radius_m, crate::campus::direction(lat_deg, lon_deg)))
    {
        return r - body.radius_m;
    }
    terrain.map(|t| t.ground_m(lat_deg, lon_deg)).unwrap_or(0.0)
}

/// Atmosphere model attached to bodies that have one.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Atmosphere(pub Atmo);

fn default_facing() -> DVec3 {
    DVec3::new(0.0, 0.0, 1.0)
}

/// What firing a stage did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageOutcome {
    pub fired: bool,
    /// Mass that separated from the vessel (structure plus the dropped stage's unburned propellant), t.
    pub dropped_t: f64,
}

impl Powered {
    pub fn new(spec: FlightSpec, r: DVec3, v: DVec3, landed: bool) -> Powered {
        let (mass, prop) = match spec.stages.first() {
            Some(s) => (s.start_mass_t, s.propellant_t),
            None => (1.0, 0.0),
        };
        Powered {
            spec,
            stage: 0,
            propellant_t: prop,
            mass_t: mass,
            r,
            v,
            throttle: 0.0,
            attitude: Attitude::Up,
            landed,
            crashed: false,
            coasting_since: None,
            facing: if r.length_squared() > 0.0 {
                r.normalized()
            } else {
                default_facing()
            },
            angvel: DVec3::ZERO,
            thrust_scale: 1.0,
            locked_thrust_fraction: 0.0,
            drag_cda_m2: None,
            wheel_scale: 1.0,
            controlled: true,
            ignited: !landed,
            ground_clearance_m: 0.0,
            footprint_m: 0.0,
            crash_tolerance_mps: 0.0,
            tipped: false,
            walker: false,
            drive: Drive::default(),
            roll_rad: 0.0,
            roll_input: 0.0,
            groups: ActionGroups::default(),
            rotation_input: DVec3::ZERO,
            roll_rate: 0.0,
            eva_run: false,
            eva_jump: false,
            eva_hop: false,
            rcs: RcsState::default(),
            lift_kn: DVec3::ZERO,
            aero_torque_knm: DVec3::ZERO,
            surface_torque_knm: 0.0,
            steer_cmd: DVec3::ZERO,
            jet: None,
            intakes: false,
            level: false,
        }
    }

    pub fn current_stage(&self) -> Option<&StageSpec> {
        self.spec.stages.get(self.stage)
    }

    /// Fire the next stage and take its engines. When the stage jettisons structure (decouplers), that
    /// structure separates together with the current stage's unburned propellant; when it jettisons nothing,
    /// the unburned propellant stays on board and is added to the next stage's.
    pub fn stage_next(&mut self) -> StageOutcome {
        if self.stage + 1 >= self.spec.stages.len() {
            if self.stage < self.spec.stages.len() {
                // last stage: nothing further to fire; its remaining propellant can no longer be used
                self.stage += 1;
                self.propellant_t = 0.0;
            }
            return StageOutcome {
                fired: false,
                dropped_t: 0.0,
            };
        }
        let cur = &self.spec.stages[self.stage];
        let cur_dry_end = cur.start_mass_t - cur.propellant_t;
        let leftover = self.propellant_t.max(0.0);
        self.stage += 1;
        let next = &self.spec.stages[self.stage];
        let jettison = (cur_dry_end - next.start_mass_t).max(0.0);
        if jettison > 0.001 {
            self.mass_t = next.start_mass_t;
            self.propellant_t = next.propellant_t;
            StageOutcome {
                fired: true,
                dropped_t: jettison + leftover,
            }
        } else {
            self.mass_t = next.start_mass_t + leftover;
            self.propellant_t = next.propellant_t + leftover;
            StageOutcome {
                fired: true,
                dropped_t: 0.0,
            }
        }
    }

    /// Stage whose rotation properties apply now (the last one once all are spent).
    fn rotation_stage(&self) -> Option<&StageSpec> {
        self.spec
            .stages
            .get(self.stage)
            .or_else(|| self.spec.stages.last())
    }

    pub fn thrust_kn(&self, pressure_atm: f64) -> f64 {
        match self.current_stage() {
            Some(s) if self.ignited && !self.crashed && self.propellant_t > 0.0 => {
                let p = pressure_atm.clamp(0.0, 1.0);
                (s.thrust_vac_kn + (s.thrust_asl_kn - s.thrust_vac_kn) * p)
                    * (self.locked_thrust_fraction
                        + (1.0 - self.locked_thrust_fraction) * self.throttle)
                    * self.thrust_scale
            }
            _ => 0.0,
        }
    }
}

/// Local surface frame at `r`: (up, east, north) for a body spinning about +Z.
pub fn surface_frame(r: DVec3) -> (DVec3, DVec3, DVec3) {
    let up = r.normalized();
    let z = DVec3::new(0.0, 0.0, 1.0);
    let mut east = z.cross(up);
    if east.length_squared() < 1e-12 {
        east = DVec3::new(0.0, 1.0, 0.0);
    }
    let east = east.normalized();
    let north = up.cross(east).normalized();
    (up, east, north)
}

/// Thrust direction for `attitude` given the state.
pub fn attitude_direction(att: Attitude, r: DVec3, v: DVec3, omega: f64) -> DVec3 {
    let (up, east, north) = surface_frame(r);
    let v_surf = v - DVec3::new(0.0, 0.0, omega).cross(r);
    let h = r.cross(v);
    let normal = if h.length_squared() > 0.0 {
        h.normalized()
    } else {
        up
    };
    let dir = match att {
        Attitude::Up => up,
        Attitude::SurfacePrograde => {
            if v_surf.length() > 1e-6 {
                v_surf.normalized()
            } else {
                up
            }
        }
        Attitude::SurfaceRetrograde => {
            if v_surf.length() > 1e-6 {
                -v_surf.normalized()
            } else {
                up
            }
        }
        Attitude::OrbitPrograde => {
            if v.length() > 1e-6 {
                v.normalized()
            } else {
                up
            }
        }
        Attitude::OrbitRetrograde => {
            if v.length() > 1e-6 {
                -v.normalized()
            } else {
                up
            }
        }
        Attitude::Normal => normal,
        Attitude::AntiNormal => -normal,
        Attitude::RadialOut => up,
        Attitude::RadialIn => -up,
        Attitude::PitchHeading {
            pitch_deg,
            heading_deg,
        } => {
            let (sp, cp) = pitch_deg.to_radians().sin_cos();
            let (sh, ch) = heading_deg.to_radians().sin_cos();
            up * sp + (east * sh + north * ch) * cp
        }
        // a maneuver or target hold with nothing to point at yet keeps to prograde
        Attitude::Maneuver(d) | Attitude::Target(d) | Attitude::AntiTarget(d)
            if d.length_squared() < 1e-18 =>
        {
            if v.length() > 1e-6 {
                v.normalized()
            } else {
                up
            }
        }
        Attitude::Fixed(d) | Attitude::Maneuver(d) | Attitude::Target(d) => d,
        Attitude::AntiTarget(d) => -d,
        // nothing held: the vessel's own axis is not known here, so the local up stands in
        Attitude::Free => up,
    };
    dir.normalized()
}

/// Rotate `f` by the rotation vector `w * dt` (Rodrigues).
pub(crate) fn rotate(f: DVec3, w: DVec3, dt: f64) -> DVec3 {
    let angle = w.length() * dt;
    if angle < 1e-12 {
        return f;
    }
    let k = w.normalized();
    let (sa, ca) = angle.sin_cos();
    (f * ca + k.cross(f) * sa + k * (k.dot(f) * (1.0 - ca))).normalized()
}

/// Turn the thrust axis toward `target` with the torque available: reaction wheels always, gimbals in
/// proportion to `thrust_fraction`. The controller commands a turn rate that can still be stopped at the
/// available angular acceleration (`w = sqrt(2 alpha theta)`), capped at [`MAX_TURN_RATE`].
pub fn steer(p: &mut Powered, target: DVec3, thrust_fraction: f64, dt: f64) {
    let Some(s) = p.rotation_stage().cloned() else {
        p.facing = target;
        return;
    };
    if s.moi_tm2 <= 0.0 {
        p.facing = target;
        p.angvel = DVec3::ZERO;
        return;
    }
    let mass_ratio = if s.start_mass_t > 0.0 {
        (p.mass_t / s.start_mass_t).clamp(0.05, 1.5)
    } else {
        1.0
    };
    let moi = s.moi_tm2 * mass_ratio;
    let rcs_torque = if p.groups.rcs {
        p.rcs.torque_knm * p.rcs.powered.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let torque = if p.controlled {
        s.wheel_torque_knm * p.wheel_scale.clamp(0.0, 1.0)
            + s.gimbal_torque_knm * thrust_fraction.clamp(0.0, 1.0)
            + rcs_torque
            + p.surface_torque_knm
    } else {
        0.0
    };
    // drop roll: only the component of the angular velocity that moves the thrust axis matters here
    let w = p.angvel - p.facing * p.angvel.dot(p.facing);
    if torque > 0.0 {
        let alpha_max = torque / moi; // kN*m / t*m^2 = rad/s^2
        let axis = p.facing.cross(target);
        let sin = axis.length();
        let cos = p.facing.dot(target);
        let theta = sin.atan2(cos);
        let axis_u = if sin > 1e-9 {
            axis * (1.0 / sin)
        } else if cos < 0.0 {
            // pointing exactly away: any perpendicular axis
            let a = p.facing.cross(DVec3::new(1.0, 0.0, 0.0));
            if a.length() > 1e-6 {
                a.normalized()
            } else {
                p.facing.cross(DVec3::new(0.0, 1.0, 0.0)).normalized()
            }
        } else {
            DVec3::ZERO
        };
        let w_des = axis_u * (2.0 * alpha_max * theta * 0.9).sqrt().min(MAX_TURN_RATE);
        let mut a = (w_des - w) * (1.0 / dt);
        let am = a.length();
        if am > alpha_max {
            a = a * (alpha_max / am);
        }
        p.steer_cmd = a * (1.0 / alpha_max);
        p.angvel = w + a * dt;
    } else {
        p.steer_cmd = DVec3::ZERO;
        p.angvel = w;
    }
    p.facing = rotate(p.facing, p.angvel, dt);
}

/// The torque available to turn the vessel, kN·m, and its moment of inertia now, t·m² (reaction wheels,
/// engine gimbals at `thrust_fraction`, RCS with the group on).
fn turning_authority(p: &Powered, thrust_fraction: f64) -> Option<(f64, f64)> {
    let s = p.rotation_stage()?;
    if s.moi_tm2 <= 0.0 {
        return None;
    }
    let mass_ratio = if s.start_mass_t > 0.0 {
        (p.mass_t / s.start_mass_t).clamp(0.05, 1.5)
    } else {
        1.0
    };
    let rcs = if p.groups.rcs {
        p.rcs.torque_knm * p.rcs.powered.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let torque = if p.controlled {
        s.wheel_torque_knm * p.wheel_scale.clamp(0.0, 1.0)
            + s.gimbal_torque_knm * thrust_fraction.clamp(0.0, 1.0)
            + rcs
            + p.surface_torque_knm
    } else {
        0.0
    };
    Some((torque, s.moi_tm2 * mass_ratio))
}

/// SAS off: the rotation input accelerates the vessel's spin about its own axes at the torque it has, and
/// nothing damps it.
pub fn spin(p: &mut Powered, thrust_fraction: f64, dt: f64) {
    let (alpha, _) = match turning_authority(p, thrust_fraction) {
        Some((t, moi)) => (t / moi, moi),
        None => (0.0, 1.0),
    };
    let i = p.rotation_input;
    let i = DVec3::new(
        i.x.clamp(-1.0, 1.0),
        i.y.clamp(-1.0, 1.0),
        i.z.clamp(-1.0, 1.0),
    );
    let top = vessel_top(p);
    let right = p.facing.cross(top);
    // pitch turns the nose toward the top: about -right; yaw toward the right: about +top
    let w_turn = p.angvel - p.facing * p.angvel.dot(p.facing);
    p.steer_cmd = right * -i.x + top * i.y + p.facing * i.z;
    let w = w_turn + (right * -i.x + top * i.y) * (alpha * dt);
    let w = if w.length() > MAX_TURN_RATE {
        w * (MAX_TURN_RATE / w.length())
    } else {
        w
    };
    p.angvel = w;
    p.facing = rotate(p.facing, p.angvel, dt).normalized();
    // roll about the nose: its own rate, built up by the input and kept
    p.roll_rate = (p.roll_rate + i.z * alpha * dt).clamp(-MAX_TURN_RATE, MAX_TURN_RATE);
    p.roll_rad = (p.roll_rad + p.roll_rate * dt).rem_euclid(std::f64::consts::TAU);
}

/// Plan part-backed controls without advancing motion or consuming resources.
pub fn plan_actuators(
    p: &Powered,
    body: &Body,
    engine: &Propulsion,
    dt: f64,
) -> crate::actuators::Allocation {
    let Some(channels) = engine.actuators.as_ref() else {
        return Default::default();
    };
    if p.landed || p.crashed || !p.controlled {
        return crate::actuators::Allocation {
            inputs: vec![0.0; channels.len()],
            ..Default::default()
        };
    }
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let target = attitude_direction(p.attitude, p.r, p.v, omega);
    let max_thrust = p
        .current_stage()
        .map(|s| s.thrust_vac_kn.max(s.thrust_asl_kn))
        .unwrap_or(0.0);
    let frac = if max_thrust > 0.0 {
        engine.thrust_kn / max_thrust
    } else {
        0.0
    };
    let (request, _) = control_request(p, engine, target, frac, dt);
    crate::actuators::allocate(channels, request)
}

fn control_request(
    p: &Powered,
    engine: &Propulsion,
    target: DVec3,
    frac: f64,
    dt: f64,
) -> (DVec3, f64) {
    let (bx, by, bz) = vessel_basis(p);
    let local = |v: DVec3| DVec3::new(v.dot(bx), v.dot(by), v.dot(bz));
    let inertia = engine.mass.inertia;
    let w = local(p.angvel + by * p.roll_rate);
    let fallback_torque = p
        .rotation_stage()
        .map(|s| {
            if !p.controlled {
                return 0.0;
            }
            s.wheel_torque_knm * p.wheel_scale.clamp(0.0, 1.0)
                + s.gimbal_torque_knm * frac.clamp(0.0, 1.0)
                + if p.groups.rcs {
                    p.rcs.torque_knm * p.rcs.powered.clamp(0.0, 1.0)
                } else {
                    0.0
                }
                + p.surface_torque_knm
        })
        .unwrap_or(0.0)
        .max(0.0);
    let torque = engine
        .actuators
        .as_ref()
        .map(|a| {
            if p.controlled {
                a.iter().map(|a| a.torque_knm.length()).sum()
            } else {
                0.0
            }
        })
        .unwrap_or(fallback_torque);
    let limit = |v: DVec3, cap: f64| {
        let n = v.length();
        if n > cap && n > 0.0 {
            v * (cap / n)
        } else {
            v
        }
    };
    let control = if p.attitude == Attitude::Free {
        let i = p.rotation_input;
        limit(DVec3::new(-i.x, i.z, i.y), 1.0) * torque
    } else {
        let cross = by.cross(target);
        let theta = cross.length().atan2(by.dot(target));
        let axis = if cross.length() > 1e-12 {
            cross.normalized()
        } else if by.dot(target) < 0.0 {
            bx
        } else {
            DVec3::ZERO
        };
        let axis = local(axis);
        let moment = inertia.apply(axis).length();
        let alpha = if moment > 0.0 { torque / moment } else { 0.0 };
        // Close to the target, the stopping-speed law can cross the entire error
        // in one tick and start a limit cycle. Bound the requested rate by that error.
        let desired = axis
            * (2.0 * alpha * theta * 0.9)
                .sqrt()
                .min(MAX_TURN_RATE)
                .min(theta / dt)
            + DVec3::new(0.0, ROLL_RATE_RAD_S * p.roll_input.clamp(-1.0, 1.0), 0.0);
        limit(inertia.apply((desired - w) * (1.0 / dt)), torque)
    };
    (control, torque)
}

/// Integrate the part-backed craft as a rotating body. The controller requests bounded torque;
/// external forces can spin it faster than the controller's commanded rate limit.
fn turn_with_parts(
    p: &mut Powered,
    engine: &Propulsion,
    target: DVec3,
    frac: f64,
    dt: f64,
    out: &mut StepOutcome,
) {
    let (bx, by, bz) = vessel_basis(p);
    let local = |v: DVec3| DVec3::new(v.dot(bx), v.dot(by), v.dot(bz));
    let world = |v: DVec3| bx * v.x + by * v.y + bz * v.z;
    let inertia = engine.mass.inertia;
    let w0 = p.angvel + by * p.roll_rate;
    let w = local(w0);
    let (request, torque) = control_request(p, engine, target, frac, dt);
    let control = if let Some(channels) = &engine.actuators {
        let allocation = engine
            .control_allocation
            .clone()
            .unwrap_or_else(|| crate::actuators::allocate(channels, request));
        out.actuator_inputs = allocation.inputs;
        out.actuator_axial_corrections = allocation.axial_corrections;
        out.control_force_kn = world(allocation.force_kn);
        allocation.torque_knm
    } else {
        request
    };
    p.steer_cmd = if torque > 0.0 {
        world(control) * (1.0 / torque)
    } else {
        DVec3::ZERO
    };
    // Advance inertial angular momentum instead of explicitly stepping the gyroscopic term.
    // This preserves momentum in a torque-free tumble, including about non-principal axes.
    let mut momentum = world(inertia.apply(w));
    let mut facing = by;
    let mut top = bz;
    let mut w1 = w0;
    let torque_body = control + engine.torque_knm;
    let external = p.aero_torque_knm + engine.environment_torque_knm;
    let acceleration = inertia.solve(torque_body + local(external)).length();
    let steps = ((w.length() + acceleration * dt) * dt / 0.05)
        .ceil()
        .clamp(1.0, 256.0) as usize;
    let h = dt / steps as f64;
    for _ in 0..steps {
        let mut midpoint_w = w1;
        let mut next_momentum = momentum;
        for _ in 0..4 {
            let y = rotate(facing, midpoint_w, h * 0.5);
            let z = rotate(top, midpoint_w, h * 0.5);
            let x = y.cross(z);
            let t = x * torque_body.x + y * torque_body.y + z * torque_body.z + external;
            next_momentum = momentum + t * h;
            let mid = (momentum + next_momentum) * 0.5;
            let local_w = inertia.solve(DVec3::new(mid.dot(x), mid.dot(y), mid.dot(z)));
            midpoint_w = x * local_w.x + y * local_w.y + z * local_w.z;
        }
        facing = rotate(facing, midpoint_w, h);
        top = rotate(top, midpoint_w, h);
        momentum = next_momentum;
        let x = facing.cross(top);
        let local_w = inertia.solve(DVec3::new(
            momentum.dot(x),
            momentum.dot(facing),
            momentum.dot(top),
        ));
        w1 = x * local_w.x + facing * local_w.y + top * local_w.z;
    }
    p.facing = facing;
    set_vessel_top(p, top);
    p.roll_rate = w1.dot(p.facing);
    p.angvel = w1 - p.facing * p.roll_rate;
    out.angular_accel = (w1 - w0) * (1.0 / dt);
    out.external_angular_accel = out.angular_accel - world(inertia.solve(control));
    out.control_torque_knm = Some(world(control));
    out.angvel = w1;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepOutcome {
    pub actuator_inputs: Vec<f64>,
    pub actuator_axial_corrections: Vec<crate::rigid::PartForce>,
    /// Bounds contacts exceeding this particular part's crash tolerance.
    pub contact_damage: Vec<usize>,
    /// Resolved gimbal and control-surface side force, inertial kN.
    pub control_force_kn: DVec3,
    pub crashed: bool,
    pub touched_down: bool,
    /// Sum of engine thrust magnitudes delivered this step, kN.
    pub thrust_kn: f64,
    /// Aerodynamic drag this step, kN, inertial.
    pub drag_kn: DVec3,
    /// Total angular acceleration and final angular velocity, inertial (including roll for part-backed flight).
    pub angular_accel: DVec3,
    /// External part-force contribution; do not synthesize this again as wheel/gimbal torque.
    pub external_angular_accel: DVec3,
    /// Resolved actuator torque for part-backed flight, inertial; structural loading uses it once.
    pub control_torque_knm: Option<DVec3>,
    pub angvel: DVec3,
    /// Share of that control torque from reaction wheels (the rest from engine gimbals).
    pub wheel_share: f64,
    /// Damage-check speed at touchdown: relative speed, or normal speed for an intact wheel landing.
    pub impact_mps: f64,
    /// Incoming normal speed, kept separate from the relative-speed damage rule.
    pub normal_impact_mps: f64,
}

/// Engine conditions at the vessel, shared by simulation and flight readouts.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineEnvironment {
    pub pressure_atm: f64,
    pub density: f64,
    pub mach: f64,
    pub oxygen: bool,
}

impl EngineEnvironment {
    pub fn at(p: &Powered, body: &Body, atmo: Option<&Atmo>) -> Self {
        let Some(a) = atmo else {
            return Self::default();
        };
        let alt = p.r.length() - body.radius_m;
        let omega = if body.rotation_period_s > 0.0 {
            std::f64::consts::TAU / body.rotation_period_s
        } else {
            0.0
        };
        let speed = (p.v - DVec3::new(0.0, 0.0, omega).cross(p.r)).length();
        let sound = a.speed_of_sound(alt);
        Self {
            pressure_atm: a.pressure_kpa(alt) / 101.325,
            density: a.density(alt),
            mach: if sound > 0.0 { speed / sound } else { 0.0 },
            oxygen: a.oxygen,
        }
    }
}

/// Average engine thrust over a step and the mass actually delivered by the tanks.
/// The resource graph has already removed this fuel; the integrator must not request it again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Propulsion {
    pub actuators: Option<Vec<crate::actuators::Actuator>>,
    /// Inputs already limited by this tick's delivered actuator resources.
    pub control_allocation: Option<crate::actuators::Allocation>,
    /// Resolved per-part drag, including angular airflow. None selects the bulk-velocity fallback.
    pub aerodynamic_drag_kn: Option<DVec3>,
    /// Buoyancy/suspension forces and moments are inertial, unlike mounted engine forces.
    pub environment_force_kn: DVec3,
    pub environment_torque_knm: DVec3,
    pub per_part_contacts: bool,
    pub thrust_kn: f64,
    pub burned_t: f64,
    /// Resultant force and its moment about the current mass centre, in the craft frame.
    pub force_kn: DVec3,
    pub torque_knm: DVec3,
    /// Pre-burn mass distribution; also present when the engines are off.
    pub mass: crate::dynamics::MassProperties,
}

/// Integrate one `dt` of powered flight around `body`.
pub fn integrate(
    p: &mut Powered,
    body: &Body,
    atmo: Option<&Atmo>,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    ut: f64,
    dt: f64,
) -> StepOutcome {
    integrate_with_propulsion(p, body, atmo, terrain, ut, dt, None)
}

/// Integrate with resolved per-engine propulsion, or the legacy stage estimate for vessels without parts.
pub fn integrate_with_propulsion(
    p: &mut Powered,
    body: &Body,
    atmo: Option<&Atmo>,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    ut: f64,
    dt: f64,
    propulsion: Option<Propulsion>,
) -> StepOutcome {
    let mut out = StepOutcome::default();
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let omega_v = DVec3::new(0.0, 0.0, omega);
    if p.crashed {
        // The wreck stays at its impact site on the rotating surface, alongside its world-space FX.
        let (s, c) = (omega * dt).sin_cos();
        let turn = |v: DVec3| DVec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
        p.r = turn(p.r);
        p.facing = turn(p.facing);
        p.v = omega_v.cross(p.r);
        return out;
    }
    let r_mag = p.r.length();
    let alt = r_mag - body.radius_m;
    let (pressure_kpa, rho) = match atmo {
        Some(a) => (a.pressure_kpa(alt), a.density(alt)),
        None => (0.0, 0.0),
    };
    let pressure_atm = pressure_kpa / 101.325;

    // a Kerbal standing on the ground with a level attitude walks that way rather than firing the jetpack
    let walk_dir = if p.walker && p.landed && !p.crashed && p.throttle > 0.0 {
        let (up, _, _) = surface_frame(p.r);
        let d = attitude_direction(p.attitude, p.r, p.v, omega);
        let level = d - up * d.dot(up);
        (d.dot(up) < 0.7 && level.length() > 1e-6).then(|| level.normalized())
    } else {
        None
    };
    // attitude: landed vessels stand upright; otherwise turn toward the commanded attitude with torque
    let thrust_kn = if walk_dir.is_some() || p.eva_hop {
        0.0
    } else if let Some(engine) = propulsion.as_ref() {
        engine.thrust_kn
    } else {
        // jets: their thrust follows the speed and the air
        let air = match (&p.jet, atmo) {
            (Some(j), Some(a)) => {
                let sound = a.speed_of_sound(alt);
                let v_air = (p.v - DVec3::new(0.0, 0.0, omega).cross(p.r)).length();
                let mach = if sound > 0.0 { v_air / sound } else { 0.0 };
                j.factor(mach, rho, a.oxygen, p.intakes)
            }
            (Some(_), None) => 0.0,
            (None, _) => 1.0,
        };
        p.thrust_kn(pressure_atm) * air
    };
    let (bx, by, bz) = vessel_basis(p);
    let resolved_force = propulsion
        .as_ref()
        .map(|e| bx * e.force_kn.x + by * e.force_kn.y + bz * e.force_kn.z);
    if let Some(allocation) = propulsion
        .as_ref()
        .and_then(|e| e.control_allocation.as_ref())
    {
        out.actuator_inputs = allocation.inputs.clone();
        out.actuator_axial_corrections = allocation.axial_corrections.clone();
        let f = allocation.force_kn;
        out.control_force_kn = bx * f.x + by * f.y + bz * f.z;
    }
    if p.landed {
        if !p.tipped {
            p.facing = if p.level {
                // a plane rolls along its heading and, fast enough for the elevators to bite, rotates its
                // nose up toward the commanded pitch (as far as its tail clears the runway)
                let up = p.r.normalized();
                let flat = heading_dir(p.r, p.drive.heading_rad);
                let want = attitude_direction(p.attitude, p.r, p.v, omega);
                let wanted_pitch = if p.attitude == Attitude::Free {
                    // a plane's +Z is its belly: nose up is pitch toward -Z
                    (-p.rotation_input.x).clamp(0.0, 1.0) * MAX_ROTATE_RAD
                } else {
                    want.dot(up).clamp(-1.0, 1.0).asin()
                };
                let pitch = if p.drive.speed_mps.abs() > ROTATE_SPEED_MPS {
                    wanted_pitch.clamp(0.0, MAX_ROTATE_RAD)
                } else {
                    0.0
                };
                (flat * pitch.cos() + up * pitch.sin()).normalized()
            } else {
                p.r.normalized()
            };
        }
        p.angvel = DVec3::ZERO;
    } else {
        let target = attitude_direction(p.attitude, p.r, p.v, omega);
        let max_thrust = p
            .current_stage()
            .map(|s| s.thrust_vac_kn.max(s.thrust_asl_kn))
            .unwrap_or(0.0);
        let frac = if max_thrust > 0.0 {
            thrust_kn / max_thrust
        } else {
            0.0
        };
        if let Some(engine) = propulsion.as_ref().filter(|e| e.mass.mass_t > 0.0) {
            turn_with_parts(p, engine, target, frac, dt, &mut out);
        } else {
            let w0 = p.angvel;
            // the airflow turns the craft; SAS (or the pilot) has to hold against it
            if let Some((_, moi)) = turning_authority(p, frac) {
                p.angvel = p.angvel + p.aero_torque_knm * (dt / moi);
            }
            if p.attitude == Attitude::Free {
                spin(p, frac, dt);
            } else {
                p.roll_rate = 0.0;
                steer(p, target, frac, dt);
                // roll about the axis at the reaction wheels' pace
                let rate =
                    ROLL_RATE_RAD_S * p.wheel_scale.clamp(0.0, 1.0) * p.roll_input.clamp(-1.0, 1.0);
                p.roll_rad = (p.roll_rad + rate * dt).rem_euclid(std::f64::consts::TAU);
            }
            out.angular_accel = (p.angvel - w0) * (1.0 / dt);
            out.angvel = p.angvel;
        }
        out.wheel_share = p
            .rotation_stage()
            .map(|s| {
                let wheel = s.wheel_torque_knm * p.wheel_scale.clamp(0.0, 1.0);
                let gimbal = s.gimbal_torque_knm * frac.clamp(0.0, 1.0);
                if wheel + gimbal > 0.0 {
                    wheel / (wheel + gimbal)
                } else {
                    1.0
                }
            })
            .unwrap_or(1.0);
    }

    // thrust and propellant
    let mut thrust_acc = DVec3::ZERO;
    let force_mass_t = p.mass_t;
    if let Some(engine) = propulsion.as_ref() {
        if p.mass_t > 0.0 && thrust_kn > 0.0 {
            thrust_acc = resolved_force.unwrap_or(DVec3::ZERO) * (1.0 / p.mass_t);
        }
        out.thrust_kn = thrust_kn;
        p.propellant_t = (p.propellant_t - engine.burned_t).max(0.0);
        p.mass_t = (p.mass_t - engine.burned_t).max(0.001);
    } else if thrust_kn > 0.0 {
        let s = p.current_stage().expect("stage");
        let isp = s.isp_vac_s + (s.isp_asl_s - s.isp_vac_s) * pressure_atm.clamp(0.0, 1.0);
        let flow = if isp > 0.0 {
            thrust_kn / (isp * G0)
        } else {
            0.0
        }; // t/s
        let burn = (flow * dt).min(p.propellant_t);
        let frac = if flow * dt > 0.0 {
            burn / (flow * dt)
        } else {
            0.0
        };
        let dir = if p.landed && !p.level {
            attitude_direction(p.attitude, p.r, p.v, omega)
        } else {
            p.facing
        };
        thrust_acc = dir * (thrust_kn * frac / p.mass_t); // kN / t = m/s^2
        out.thrust_kn = thrust_kn * frac;
        p.propellant_t -= burn;
        p.mass_t -= burn;
    }

    thrust_acc = thrust_acc
        + out.control_force_kn * (1.0 / force_mass_t)
        + propulsion
            .as_ref()
            .map(|e| e.environment_force_kn * (1.0 / p.mass_t))
            .unwrap_or(DVec3::ZERO);

    // RCS translation (the RCS group on), in the craft's axes
    if propulsion.is_none() {
        p.rcs.usage = 0.0;
    }
    if propulsion.is_none()
        && p.groups.rcs
        && p.controlled
        && p.rcs.thrust_kn > 0.0
        && p.mass_t > 0.0
    {
        // Turning spends RCS propellant too, including SAS correction. The same normalized
        // request drives the directional nozzle visuals in the authoritative readout.
        if !p.landed && p.rcs.torque_knm > 0.0 {
            p.rcs.usage += p.steer_cmd.length().min(1.0);
        }
        let t = p.rcs.translate;
        let t = DVec3::new(
            t.x.clamp(-1.0, 1.0),
            t.y.clamp(-1.0, 1.0),
            t.z.clamp(-1.0, 1.0),
        );
        let amount = t.length().min(1.0);
        if amount > 0.0 {
            let top = vessel_top(p);
            let right = p.facing.cross(top);
            let dir = (right * t.x + p.facing * t.y + top * t.z).normalized();
            let isp = p.rcs.isp_vac_s
                + (p.rcs.isp_asl_s - p.rcs.isp_vac_s) * pressure_atm.clamp(0.0, 1.0);
            let scale = if p.rcs.isp_vac_s > 0.0 {
                (isp / p.rcs.isp_vac_s).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let thrust = p.rcs.thrust_kn * amount * scale * p.rcs.powered.clamp(0.0, 1.0);
            thrust_acc = thrust_acc + dir * (thrust / p.mass_t);
            p.rcs.usage += amount;
        }
    }

    if p.landed {
        // sitting on the surface: co-rotate; lift off only when thrust beats gravity
        let g = body.mu / (r_mag * r_mag);
        let (up, _, _) = surface_frame(p.r);
        let normal = ground_normal(body, terrain, p.r, ut);
        let lift_acc = if p.mass_t > 0.0 {
            p.lift_kn * (1.0 / p.mass_t)
        } else {
            DVec3::ZERO
        };
        let support_acc = thrust_acc + lift_acc - up * g - omega_v.cross(omega_v.cross(p.r));
        if support_acc.dot(normal) > 0.0 {
            p.landed = false;
        } else {
            let pace = if p.eva_run {
                RUN_SPEED_MPS
            } else {
                WALK_SPEED_MPS
            };
            if let Some(dir) = walk_dir {
                p.r = p.r + dir * (pace * p.throttle.min(1.0) * dt);
            }
            // a Kerbal jumps off the ground, carrying the walk forward
            if p.walker && p.eva_jump && !p.crashed {
                p.eva_jump = false;
                p.eva_hop = true;
                let ahead =
                    walk_dir.map(|d| d * (pace * p.throttle.min(1.0) * JUMP_RUN_MULTIPLIER));
                p.landed = false;
                p.v = omega_v.cross(p.r) + up * JUMP_SPEED_MPS + ahead.unwrap_or(DVec3::ZERO);
                p.r = p.r + up * 0.01;
                return out;
            }
            // on wheels: drive along the ground (a level craft along its nose, pushed by its engines too)
            let mut rolling = DVec3::ZERO;
            if !p.tipped && !p.drive.wheels.is_empty() {
                let dir = if p.level {
                    heading_dir(p.r, p.drive.heading_rad)
                } else {
                    p.drive.direction(p.r)
                };
                if p.level {
                    p.drive.speed_mps += (thrust_acc + lift_acc).dot(dir) * dt;
                }
                // downhill along the way it drives: gravity times the slope's sine
                let ahead = ground_radius(body, terrain, p.r + dir * 2.0, ut + dt);
                let behind = ground_radius(body, terrain, p.r - dir * 2.0, ut + dt);
                let rise = (ahead - behind) / 4.0;
                let slope_acc = -g * rise / (1.0 + rise * rise).sqrt();
                let moved = p.drive.step(p.mass_t, g, slope_acc, dt);
                p.r = p.r + dir * moved;
                rolling = dir * p.drive.speed_mps;
            } else if !p.walker {
                let relative = p.v - omega_v.cross(p.r);
                // Ground reaction removes penetration; friction opposes sliding and can hold a slope.
                rolling = crate::contact::resolve_velocity(relative + support_acc * dt, normal).0;
                p.r = p.r + rolling * dt;
            }
            let ang = omega * dt;
            let (s, c) = ang.sin_cos();
            p.r = DVec3::new(p.r.x * c - p.r.y * s, p.r.x * s + p.r.y * c, p.r.z);
            rolling = DVec3::new(
                rolling.x * c - rolling.y * s,
                rolling.x * s + rolling.y * c,
                rolling.z,
            );
            // standing on its lowest point (or its side once tipped over)
            let ground = ground_radius(body, terrain, p.r, ut + dt) + stand_height(p);
            p.r = p.r.normalized() * ground;
            p.v = omega_v.cross(p.r) + rolling;
            return out;
        }
    }

    // gravity + drag
    let g_acc = p.r * (-body.mu / (r_mag * r_mag * r_mag));
    let v_surf = p.v - omega_v.cross(p.r);
    let resolved_drag = propulsion.as_ref().and_then(|e| e.aerodynamic_drag_kn);
    let drag_acc = if let Some(force) = resolved_drag {
        force * (1.0 / p.mass_t)
    } else if rho > 0.0 && v_surf.length_squared() > 0.0 {
        let q = 0.5 * rho * v_surf.length_squared();
        let cda = p
            .drag_cda_m2
            .unwrap_or(p.spec.drag_cd * p.spec.drag_area_m2);
        let f_n = q * cda; // newtons
        -v_surf.normalized() * (f_n / (p.mass_t * 1000.0))
    } else {
        DVec3::ZERO
    };
    let lift_acc = if p.mass_t > 0.0 {
        p.lift_kn * (1.0 / p.mass_t)
    } else {
        DVec3::ZERO
    };
    out.drag_kn = drag_acc * p.mass_t + p.lift_kn;
    let acc = g_acc + thrust_acc + drag_acc + lift_acc;
    p.v = p.v + acc * dt;
    let top = vessel_top(p);
    let previous_r = p.r;
    p.r = p.r + p.v * dt;
    if propulsion.is_some() {
        // Position changes the roll reference, not the craft's inertial orientation.
        set_vessel_top(p, top);
    }

    // surface contact against the terrain (or sea) under the vessel
    if let Some(hit) = (!propulsion.as_ref().is_some_and(|e| e.per_part_contacts))
        .then(|| ground_crossing(body, terrain, previous_r, p.r, stand_height(p), ut, dt))
        .flatten()
    {
        p.r = previous_r + (p.r - previous_r) * hit;
        let hit_ut = ut + dt * hit;
        let ground = ground_radius(body, terrain, p.r, hit_ut);
        let v_surf = p.v - omega_v.cross(p.r);
        let (up, _, _) = surface_frame(p.r);
        let normal = ground_normal(body, terrain, p.r, hit_ut);
        // It falls over when its lean relative to the actual support plane exceeds its footprint.
        // a level craft leans by how far its nose is off the horizon
        let lean = if p.level {
            p.facing.dot(normal).clamp(-1.0, 1.0).asin().abs()
        } else {
            p.facing.dot(normal).clamp(-1.0, 1.0).acos()
        };
        if p.ground_clearance_m > 0.0 && lean.tan() > p.footprint_m / p.ground_clearance_m {
            p.tipped = true;
            let side = p.facing - up * p.facing.dot(up);
            p.facing = if side.length() > 1e-6 {
                side.normalized()
            } else {
                surface_frame(p.r).1
            };
        } else if p.level {
            // a plane touches down rolling along its nose
            let flat = p.facing - up * p.facing.dot(up);
            if flat.length() > 1e-6 {
                p.facing = flat.normalized();
                p.drive.heading_rad = heading_of(p.r, p.facing);
            }
        } else {
            p.facing = up;
        }
        let (sliding, normal_impact) = crate::contact::resolve_velocity(v_surf, normal);
        // PartBehavior.HandleCollisionEnter compares relative speed with crashTolerance.
        // Preserve the existing wheel/suspension exception for an intact rolling landing.
        let impact = if p.level && !p.tipped && !p.drive.wheels.is_empty() {
            normal_impact
        } else {
            v_surf.length()
        };
        let rolling = if p.level && !p.tipped && !p.drive.wheels.is_empty() {
            p.drive.speed_mps = v_surf.dot(p.facing);
            p.facing * p.drive.speed_mps
        } else {
            sliding
        };
        p.r = up * (ground + stand_height(p));
        // Carry the contact site through the remainder of this tick on a rotating body.
        let (s, c) = (omega * dt * (1.0 - hit)).sin_cos();
        let turn = |v: DVec3| DVec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
        p.r = turn(p.r);
        let rolling = turn(rolling);
        p.v = omega_v.cross(p.r) + rolling;
        p.landed = true;
        p.eva_hop = false;
        p.angvel = DVec3::ZERO;
        p.roll_rate = 0.0;
        out.touched_down = true;
        let speed = impact;
        out.impact_mps = speed;
        out.normal_impact_mps = normal_impact;
        let tolerance = if p.crash_tolerance_mps > 0.0 {
            p.crash_tolerance_mps
        } else {
            CRASH_SPEED_MPS
        };
        if speed > tolerance {
            p.crashed = true;
            p.throttle = 0.0;
            out.crashed = true;
        }
    }

    // coasting bookkeeping for the rails handoff
    let above_atmo = atmo.map(|a| alt > a.depth_m).unwrap_or(true);
    if out.thrust_kn <= 0.0 && !p.landed && above_atmo {
        if p.coasting_since.is_none() {
            p.coasting_since = Some(ut);
        }
    } else {
        p.coasting_since = None;
    }
    out
}

/// How high a landed craft's centre of mass stands: on its lowest point, or on its side (its footprint)
/// once tipped over.
fn stand_height(p: &Powered) -> f64 {
    if p.tipped {
        p.footprint_m
    } else {
        p.ground_clearance_m
    }
}

/// Steepness of the terrain under `r`, radians, from the height map's gradient over about 100 m.
pub fn slope_rad(body: &Body, t: &ose_data::terrain::TerrainMap, r: DVec3, ut: f64) -> f64 {
    let (lat, lon) = lat_lon(body, r, ut);
    let step_m = 100.0;
    let dlat = (step_m / body.radius_m).to_degrees();
    let dlon = dlat / lat.to_radians().cos().max(0.05);
    let dn = t.height_m(lat + dlat, lon) - t.height_m(lat - dlat, lon);
    let de = t.height_m(lat, lon + dlon) - t.height_m(lat, lon - dlon);
    ((dn * dn + de * de).sqrt() / (2.0 * step_m)).atan()
}

/// Classify the flight state.
pub fn situation(p: &Powered, body: &Body, atmo: Option<&Atmo>) -> Situation {
    if p.crashed {
        return Situation::Crashed;
    }
    if p.landed {
        return Situation::Landed;
    }
    let alt = p.r.length() - body.radius_m;
    if let Some(a) = atmo {
        if alt < a.depth_m {
            return Situation::Flying;
        }
    }
    let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, 0.0);
    if orbit.eccentricity() >= 1.0 {
        return Situation::Escaping;
    }
    let atmo_top = body.radius_m + atmo.map(|a| a.depth_m).unwrap_or(0.0);
    if orbit.periapsis() < atmo_top {
        Situation::SubOrbital
    } else {
        Situation::Orbit
    }
}

/// Position on the surface at geographic latitude/longitude (degrees) at time `ut`, and its co-rotating
/// velocity. Longitude 0 is the body's +X axis at `initial_rotation_deg = 0`, `ut = 0`.
pub fn surface_state(body: &Body, lat_deg: f64, lon_deg: f64, ut: f64) -> (DVec3, DVec3) {
    surface_state_at(body, lat_deg, lon_deg, 0.0, ut)
}

/// Like [`surface_state`] but `altitude_m` above the nominal radius.
pub fn surface_state_at(
    body: &Body,
    lat_deg: f64,
    lon_deg: f64,
    altitude_m: f64,
    ut: f64,
) -> (DVec3, DVec3) {
    let omega = if body.rotation_period_s > 0.0 {
        std::f64::consts::TAU / body.rotation_period_s
    } else {
        0.0
    };
    let theta = rotation_angle(body, ut) + lon_deg.to_radians();
    let (sl, cl) = lat_deg.to_radians().sin_cos();
    let r = DVec3::new(cl * theta.cos(), cl * theta.sin(), sl) * (body.radius_m + altitude_m);
    let v = DVec3::new(0.0, 0.0, omega).cross(r);
    (r, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact_site(body: &mut Body, width: f64, height: f64) {
        body.rotation_period_s = 0.0;
        body.site = Some(std::sync::Arc::new(crate::campus::Site {
            lat_deg: 0.0,
            lon_deg: 0.0,
            alt_m: 0.0,
            yaw_rad: 0.0,
            anchor: DVec3::ZERO,
            areas: vec![crate::campus::Area {
                min_x: -width,
                max_x: width,
                min_z: -width,
                max_z: width,
                y: height,
            }],
        }));
    }

    #[test]
    fn ground_normal_follows_a_flat_site_instead_of_planet_radial_up() {
        let mut body = kerbin();
        contact_site(&mut body, 1000.0, 0.0);
        let r = DVec3::new(body.radius_m, 900.0, 0.0);
        let n = ground_normal(&body, None, r, 0.0);
        assert!((n - DVec3::new(1.0, 0.0, 0.0)).length() < 1e-7, "{n:?}");
        assert!((n - r.normalized()).length() > 0.001);
        assert!(
            body.site
                .as_ref()
                .unwrap()
                .surface_radius_bound(body.radius_m)
                >= r.length()
        );
    }

    #[test]
    fn a_fast_path_hits_a_raised_site_between_clear_endpoints() {
        let mut body = kerbin();
        body.mu = 0.0;
        contact_site(&mut body, 2.0, 10.0);
        let from = DVec3::new(body.radius_m + 5.0, -10.0, 0.0);
        let to = DVec3::new(body.radius_m + 5.0, 10.0, 0.0);
        for r in [from, to] {
            assert!(r.length() > ground_radius(&body, None, r, 0.0));
        }
        let hit = ground_crossing(&body, None, from, to, 0.0, 0.0, 0.02).unwrap();
        assert!((hit - 0.4).abs() < 1e-5, "first contact {hit}");
        let mut p = Powered::new(rocket(), from, (to - from) * 50.0, false);
        let out = integrate(&mut p, &body, None, None, 0.0, 0.02);
        assert!(out.touched_down);
        assert!(
            p.r.y < -1.9,
            "stops at the leading edge, not the endpoint: {:?}",
            p.r
        );
    }

    #[test]
    fn a_grazing_landing_slides_and_loses_speed_over_multiple_ticks() {
        let mut body = kerbin();
        body.rotation_period_s = 0.0;
        body.mu = 10.0 * body.radius_m * body.radius_m;
        let mut p = Powered::new(
            rocket(),
            DVec3::new(body.radius_m + 0.01, 0.0, 0.0),
            DVec3::new(-2.0, 10.0, 0.0),
            false,
        );
        let out = integrate(&mut p, &body, None, None, 0.0, 0.02);
        assert!(out.touched_down && !out.crashed);
        assert!((out.normal_impact_mps - 2.2).abs() < 0.001);
        assert!(out.impact_mps > 10.0 && out.impact_mps < 11.0);
        assert!(
            (p.v.y - 8.68).abs() < 0.001,
            "tangential motion survives touchdown: {:?}",
            p.v
        );
        assert!(!resting_on_ground(&p, &body, None, 0.02));
        let start_y = p.r.y;
        let mut speed = p.v.length();
        for i in 1..=100 {
            integrate(&mut p, &body, None, None, i as f64 * 0.02, 0.02);
            assert!(p.v.length() <= speed + 1e-6, "friction cannot add energy");
            speed = p.v.length();
        }
        assert!(p.r.y - start_y > 5.0 && p.r.y - start_y < 8.0);
        assert!(
            speed < 1e-6,
            "friction eventually stops the slide, speed {speed}"
        );
        assert!((p.r.length() - body.radius_m).abs() < 1e-6);
    }

    #[test]
    fn high_speed_grazing_body_contact_uses_relative_speed_for_damage() {
        let mut body = kerbin();
        body.rotation_period_s = 0.0;
        let mut p = Powered::new(
            rocket(),
            DVec3::new(body.radius_m + 0.01, 0.0, 0.0),
            DVec3::new(-2.0, 100.0, 0.0),
            false,
        );
        let out = integrate(&mut p, &body, None, None, 0.0, 0.02);
        assert!(out.touched_down && out.crashed);
        assert!(out.normal_impact_mps < 3.0 && out.impact_mps > 100.0);
    }

    #[test]
    fn grounded_motion_holds_a_shallow_slope_and_slides_down_a_steep_one() {
        let mut body = kerbin();
        body.radius_m = 1000.0;
        body.mu = 10.0 * body.radius_m * body.radius_m;
        contact_site(&mut body, 2000.0, 0.0);
        // A flat plane far from its planetary tangent point is inclined relative to local gravity.
        for (y, stays) in [(200.0, true), (1200.0, false)] {
            let r = DVec3::new(1000.0, y, 0.0);
            let mut p = Powered::new(rocket(), r, DVec3::ZERO, true);
            assert_eq!(resting_on_ground(&p, &body, None, 0.0), stays);
            for i in 0..100 {
                integrate(&mut p, &body, None, None, i as f64 * 0.02, 0.02);
            }
            if stays {
                assert!((p.r - r).length() < 1e-7);
            } else {
                assert!(
                    p.r.y < y - 1.0 && p.v.y < 0.0,
                    "slides downhill: {:?} {:?}",
                    p.r,
                    p.v
                );
            }
            assert!((p.r.x - 1000.0).abs() < 1e-6, "remains on the plane");
        }
    }

    #[test]
    fn parked_optimization_excludes_sliding_and_unbraked_wheels_on_slopes() {
        let mut body = kerbin();
        body.rotation_period_s = 0.0;
        let mut p = Powered::new(
            rocket(),
            DVec3::new(body.radius_m, 0.0, 0.0),
            DVec3::ZERO,
            true,
        );
        p.drive.wheels.push(DriveWheel {
            radius_m: 0.3,
            torque_knm: None,
            ec_per_s: 0.0,
            brake_knm: 2.0,
            steers: false,
        });
        assert!(
            resting_on_ground(&p, &body, None, 0.0),
            "unbraked wheels can rest on a level surface"
        );
        p.v = DVec3::new(0.0, 1.0, 0.0);
        assert!(!resting_on_ground(&p, &body, None, 0.0));
        body.radius_m = 1000.0;
        body.mu = 10.0 * body.radius_m * body.radius_m;
        contact_site(&mut body, 2000.0, 0.0);
        p.r = DVec3::new(1000.0, 200.0, 0.0);
        p.v = DVec3::ZERO;
        assert!(
            !resting_on_ground(&p, &body, None, 0.0),
            "unbraked wheels roll on an incline"
        );
        p.drive.brakes = true;
        assert!(resting_on_ground(&p, &body, None, 0.0));
    }

    fn body_dynamics_fixture() -> (Powered, Propulsion) {
        let mut p = Powered::new(
            rocket(),
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::ZERO,
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        p.mass_t = 10.0;
        p.attitude = Attitude::Free;
        p.controlled = false;
        let engine = Propulsion {
            mass: crate::dynamics::MassProperties {
                mass_t: 10.0,
                inertia: crate::dynamics::Inertia {
                    x: DVec3::new(10.0, 0.0, 0.0),
                    y: DVec3::new(0.0, 20.0, 0.0),
                    z: DVec3::new(0.0, 0.0, 30.0),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        (p, engine)
    }

    #[test]
    fn off_axis_force_follows_roll_and_torque_has_the_expected_impulse() {
        let (mut p, mut engine) = body_dynamics_fixture();
        let mut body = kerbin();
        body.mu = 0.0;
        engine.thrust_kn = 10.0;
        engine.force_kn = DVec3::new(10.0, 0.0, 0.0);
        engine.torque_knm = DVec3::new(0.0, 0.0, 30.0);
        p.roll_rad = std::f64::consts::FRAC_PI_2;
        let out = integrate_with_propulsion(&mut p, &body, None, None, 0.0, 0.1, Some(engine));
        // 90-degree roll maps craft +X to inertial -Z, and craft +Z torque to inertial +X.
        assert!((p.v - DVec3::new(0.0, 0.0, -0.1)).length() < 1e-10);
        assert!((out.angvel - DVec3::new(0.1, 0.0, 0.0)).length() < 1e-10);
        assert!((p.facing.z - 0.005_f64.sin()).abs() < 1e-10);
        assert_eq!(out.control_torque_knm, Some(DVec3::ZERO));
    }

    #[test]
    fn sas_converges_on_small_errors_with_high_authority() {
        for dt in [0.01, 0.02, 0.08] {
            let (mut p, mut engine) = body_dynamics_fixture();
            p.controlled = true;
            p.spec.stages[0].wheel_torque_knm = 1000.0;
            let target = rotate(p.facing, DVec3::new(0.0, 0.0, 1.0), 1e-4);
            p.attitude = Attitude::Fixed(target);
            engine.actuators = None;
            let initial_error = (p.facing - target).length();
            let mut error = initial_error;
            for _ in 0..100 {
                turn_with_parts(
                    &mut p,
                    &engine,
                    target,
                    0.0,
                    dt,
                    &mut StepOutcome::default(),
                );
                error = (p.facing - target).length();
                assert!(error <= initial_error + 1e-10, "dt={dt}, error={error}");
            }
            assert!(error < 1e-8, "dt={dt}: {error}");
            assert!(p.angvel.length() < 1e-8, "dt={dt}: {:?}", p.angvel);
        }
    }

    #[test]
    fn sas_cannot_cancel_external_roll_without_sufficient_torque() {
        let (mut p, mut engine) = body_dynamics_fixture();
        p.attitude = Attitude::Fixed(p.facing);
        engine.torque_knm = DVec3::new(0.0, 20.0, 0.0);
        let mut out = StepOutcome::default();
        let target = p.facing;
        turn_with_parts(&mut p, &engine, target, 0.0, 0.1, &mut out);
        assert!((p.roll_rate - 0.1).abs() < 1e-10);
        p.controlled = true;
        p.spec.stages[0].wheel_torque_knm = 2.0;
        turn_with_parts(&mut p, &engine, target, 0.0, 0.1, &mut out);
        assert!((p.roll_rate - 0.19).abs() < 1e-10);
        assert!((out.control_torque_knm.unwrap().length() - 2.0).abs() < 1e-10);
    }

    #[test]
    fn torque_free_asymmetric_tumble_preserves_momentum_and_energy() {
        let (mut p, engine) = body_dynamics_fixture();
        p.angvel = DVec3::new(2.0, 0.0, 3.0);
        p.roll_rate = 1.0;
        let properties = |p: &Powered| {
            let (x, y, z) = vessel_basis(p);
            let w = p.angvel + p.facing * p.roll_rate;
            let local_w = DVec3::new(w.dot(x), w.dot(y), w.dot(z));
            let l = engine.mass.inertia.apply(local_w);
            (x * l.x + y * l.y + z * l.z, 0.5 * l.dot(local_w))
        };
        let (l0, e0) = properties(&p);
        for _ in 0..1000 {
            turn_with_parts(
                &mut p,
                &engine,
                DVec3::ZERO,
                0.0,
                0.002,
                &mut StepOutcome::default(),
            );
        }
        let (l1, e1) = properties(&p);
        assert!((l1 - l0).length() < 1e-8);
        assert!((e1 / e0 - 1.0).abs() < 1e-4, "energy {e0} -> {e1}");
        assert!((p.angvel + p.facing * p.roll_rate).length() > MAX_TURN_RATE);
    }

    #[test]
    fn a_rover_rolls_down_a_slope_unless_its_brakes_hold_it() {
        let wheel = DriveWheel {
            radius_m: 0.3,
            torque_knm: None,
            ec_per_s: 0.0,
            brake_knm: 2.0,
            steers: false,
        };
        let mut d = Drive {
            wheels: vec![wheel; 4],
            ..Default::default()
        };
        // a 10 degree hill (downhill is backwards), 1 t, Kerbin gravity
        let slope = -9.81 * 10f64.to_radians().sin();
        for _ in 0..50 {
            d.step(1.0, 9.81, slope, 0.02);
        }
        assert!(d.speed_mps < -1.0, "rolls back: {}", d.speed_mps);
        d.speed_mps = 0.0;
        d.brakes = true;
        for _ in 0..50 {
            d.step(1.0, 9.81, slope, 0.02);
        }
        assert_eq!(d.speed_mps, 0.0, "the brakes hold it");
        // on the flat it stays put
        d.brakes = false;
        d.step(1.0, 9.81, 0.0, 0.02);
        assert_eq!(d.speed_mps, 0.0);
    }

    fn kerbin() -> Body {
        let mu = 1.00034160493135 * G0 * 600_000.0 * 600_000.0;
        Body {
            id: "Kerbin".into(),
            mu,
            mass_kg: mu / ose_data::BIG_G,
            radius_m: 600_000.0,
            soi_radius_m: Some(84_159_286.0),
            rotation_period_s: 21_549.425,
            initial_rotation_deg: 0.0,
            is_star: false,
            ocean: None,
            site: None,
        }
    }

    fn kerbin_atmo() -> Atmo {
        // exponential-ish pressure with 5.6 km scale height, 70 km depth; temperature 290 K to 180 K
        let mut keys = Vec::new();
        let mut tkeys = Vec::new();
        let n = 30;
        for i in 0..=n {
            let h = 70_000.0 * i as f64 / n as f64;
            keys.push(ose_data::CurveKey {
                t: h,
                v: 101.325 * (-h / 5600.0).exp(),
                in_tangent: 0.0,
                out_tangent: 0.0,
            });
            tkeys.push(ose_data::CurveKey {
                t: h,
                v: 290.0 - 110.0 * h / 70_000.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            });
        }
        Atmo {
            depth_m: 70_000.0,
            oxygen: true,
            pressure_kpa: Curve { keys },
            temperature_k: Curve { keys: tkeys },
            molar_mass: 0.0289644,
        }
    }

    fn rocket() -> FlightSpec {
        // two stages: booster 2000 kN, Isp 290/320, 60 t prop, 70 t total at liftoff (dry below ~7 t)
        // upper: 250 kN, Isp 280/345, 12 t prop, 16 t total
        FlightSpec {
            stages: vec![
                StageSpec {
                    stage_id: 1,
                    thrust_vac_kn: 2000.0,
                    thrust_asl_kn: 1812.0,
                    isp_vac_s: 320.0,
                    isp_asl_s: 290.0,
                    propellant_t: 50.0,
                    start_mass_t: 72.0,
                    ..Default::default()
                },
                StageSpec {
                    stage_id: 0,
                    thrust_vac_kn: 250.0,
                    thrust_asl_kn: 203.0,
                    isp_vac_s: 345.0,
                    isp_asl_s: 280.0,
                    propellant_t: 12.0,
                    start_mass_t: 16.0,
                    ..Default::default()
                },
            ],
            drag_area_m2: 5.0,
            drag_cd: DEFAULT_CD,
        }
    }

    #[test]
    fn sits_on_the_pad_until_thrust_beats_gravity_then_climbs() {
        let body = kerbin();
        let atmo = kerbin_atmo();
        let (r, v) = surface_state(&body, -0.1, -74.5, 0.0);
        let mut p = Powered::new(rocket(), r, v, true);
        p.ignited = true;
        for i in 0..50 {
            integrate(&mut p, &body, Some(&atmo), None, i as f64 * 0.02, 0.02);
        }
        assert!(p.landed, "no thrust: stays landed");
        assert!((p.r.length() - body.radius_m).abs() < 1e-6);
        p.throttle = 1.0;
        p.attitude = Attitude::Up;
        let mut ut = 1.0;
        for _ in 0..(10 * 50) {
            integrate(&mut p, &body, Some(&atmo), None, ut, 0.02);
            ut += 0.02;
        }
        let alt = p.r.length() - body.radius_m;
        assert!(
            !p.landed && alt > 200.0 && alt < 2000.0,
            "alt after 10 s {alt}"
        );
        assert!(p.mass_t < 72.0 && p.propellant_t < 50.0);
        assert_eq!(situation(&p, &body, Some(&atmo)), Situation::Flying);
    }

    #[test]
    fn gravity_turn_reaches_a_stable_orbit() {
        let body = kerbin();
        let atmo = kerbin_atmo();
        let (r, v) = surface_state(&body, -0.1, -74.5, 0.0);
        let mut p = Powered::new(rocket(), r, v, true);
        p.ignited = true;
        p.throttle = 1.0;
        let dt = 0.02;
        let mut ut = 0.0;
        let target_ap = 80_000.0;
        // ascent: pitch over from 90 deg at 1 km to 0 deg at 45 km, heading east; coast after Ap reached
        let mut coasting = false;
        for _ in 0..(1500 * 50) {
            let alt = p.r.length() - body.radius_m;
            let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
            let ap_alt = orbit.apoapsis() - body.radius_m;
            if !coasting {
                let pitch = if alt < 1_000.0 {
                    90.0
                } else {
                    (90.0 * (1.0 - (alt - 1_000.0) / 44_000.0)).clamp(0.0, 90.0)
                };
                p.attitude = Attitude::PitchHeading {
                    pitch_deg: pitch,
                    heading_deg: 90.0,
                };
                p.throttle = 1.0;
                if p.propellant_t <= 0.0 && p.stage == 0 {
                    p.stage_next();
                }
                if ap_alt >= target_ap && orbit.eccentricity() < 1.0 {
                    coasting = true;
                    p.throttle = 0.0;
                }
            } else {
                // circularise near apoapsis
                let t_ap = orbit.time_to_true_anomaly(std::f64::consts::PI, ut);
                let pe_alt = orbit.periapsis() - body.radius_m;
                if pe_alt > 72_000.0 {
                    p.throttle = 0.0;
                    break;
                }
                if t_ap < 20.0 || t_ap > orbit.period - 20.0 {
                    if p.propellant_t <= 0.0 && p.stage == 0 {
                        p.stage_next();
                    }
                    p.attitude = Attitude::OrbitPrograde;
                    p.throttle = 1.0;
                } else {
                    p.throttle = 0.0;
                }
            }
            let out = integrate(&mut p, &body, Some(&atmo), None, ut, dt);
            assert!(!out.crashed, "crashed at ut {ut}");
            ut += dt;
        }
        let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
        let pe = orbit.periapsis() - body.radius_m;
        let ap = orbit.apoapsis() - body.radius_m;
        assert!(
            pe > 70_000.0 && ap < 250_000.0,
            "orbit {pe:.0} x {ap:.0} m after {ut:.0} s, prop left {:.2} t, stage {}",
            p.propellant_t,
            p.stage
        );
        assert_eq!(situation(&p, &body, Some(&atmo)), Situation::Orbit);
        assert!(p.propellant_t > 0.0, "some margin left");
    }

    #[test]
    fn touches_down_on_terrain_not_the_nominal_radius() {
        use ose_data::terrain::{BodyTerrain, MapRef, TerrainMap};
        let body = kerbin();
        // flat map 2 km above the nominal radius
        let w = 32;
        let h = 16;
        let mut blob = Vec::new();
        for _ in 0..w * h {
            blob.extend_from_slice(&(65535u16 / 2).to_le_bytes());
        }
        let entry = BodyTerrain {
            id: "Kerbin".into(),
            has_ocean: false,
            sites: vec![],
            map: Some(MapRef {
                offset: 0,
                width: w,
                height: h,
                scale_m: 4000.0,
                offset_m: 0.0,
                source: String::new(),
            }),
        };
        let t = TerrainMap::from_entry(&entry, &blob, body.radius_m).unwrap();
        let ground = t.ground_m(0.0, 0.0);
        assert!((ground - 2000.0).abs() < 1.0, "ground {ground}");
        let (r, v) = surface_state_at(&body, 0.0, 0.0, 2050.0, 0.0);
        let up = r.normalized();
        let mut p = Powered::new(rocket(), r, v - up * 3.0, false);
        let mut landed_at = None;
        for i in 0..2000 {
            let ut = i as f64 * 0.02;
            if integrate(&mut p, &body, None, Some(&t), ut, 0.02).touched_down {
                landed_at = Some(p.r.length() - body.radius_m);
                break;
            }
        }
        let alt = landed_at.expect("touched down");
        assert!(
            (alt - 2000.0).abs() < 1.0,
            "landed at {alt} m, terrain at 2000 m"
        );
        let (lat, lon) = lat_lon(&body, p.r, 0.0);
        assert!(lat.abs() < 0.1 && lon.abs() < 1.0, "lat {lat} lon {lon}");
    }

    #[test]
    fn turning_takes_time_set_by_torque_and_inertia() {
        let body = kerbin();
        let r = DVec3::new(body.radius_m + 200_000.0, 0.0, 0.0);
        let v = DVec3::new(0.0, 2_200.0, 0.0);
        let spec = FlightSpec {
            stages: vec![StageSpec {
                thrust_vac_kn: 10.0,
                thrust_asl_kn: 10.0,
                isp_vac_s: 300.0,
                isp_asl_s: 300.0,
                propellant_t: 1.0,
                start_mass_t: 5.0,
                moi_tm2: 100.0,
                wheel_torque_knm: 10.0,
                ..Default::default()
            }],
            drag_area_m2: 1.0,
            drag_cd: DEFAULT_CD,
        };
        let mut p = Powered::new(spec, r, v, false);
        assert!(
            (p.facing - r.normalized()).length() < 1e-9,
            "starts facing radially out"
        );
        let target = DVec3::new(0.0, 1.0, 0.0);
        p.attitude = Attitude::Fixed(target);
        let angle = |p: &Powered| p.facing.dot(target).clamp(-1.0, 1.0).acos().to_degrees();
        for i in 0..50 {
            integrate(&mut p, &body, None, None, i as f64 * 0.02, 0.02);
        }
        // alpha = 0.1 rad/s^2: after 1 s at most 0.05 rad (~3 deg) of turn
        assert!(angle(&p) > 80.0, "after 1 s still {} deg off", angle(&p));
        for i in 50..1500 {
            integrate(&mut p, &body, None, None, i as f64 * 0.02, 0.02);
        }
        assert!(angle(&p) < 2.0, "after 30 s {} deg off", angle(&p));
        assert!(
            p.angvel.length() < 0.02,
            "settled, w = {}",
            p.angvel.length()
        );
    }

    fn fly_gravity_turn(spec: FlightSpec) -> (f64, f64, f64) {
        let body = kerbin();
        let atmo = kerbin_atmo();
        let (r, v) = surface_state(&body, -0.1, -74.5, 0.0);
        let mut p = Powered::new(spec, r, v, true);
        p.ignited = true;
        p.throttle = 1.0;
        let dt = 0.02;
        let mut ut = 0.0;
        let mut coasting = false;
        for _ in 0..(1500 * 50) {
            let alt = p.r.length() - body.radius_m;
            let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
            let ap_alt = orbit.apoapsis() - body.radius_m;
            if !coasting {
                let pitch = if alt < 1_000.0 {
                    90.0
                } else {
                    (90.0 * (1.0 - (alt - 1_000.0) / 44_000.0)).clamp(0.0, 90.0)
                };
                p.attitude = Attitude::PitchHeading {
                    pitch_deg: pitch,
                    heading_deg: 90.0,
                };
                p.throttle = 1.0;
                if p.propellant_t <= 0.0 && p.stage == 0 {
                    p.stage_next();
                }
                if ap_alt >= 80_000.0 && orbit.eccentricity() < 1.0 {
                    coasting = true;
                    p.throttle = 0.0;
                }
            } else {
                let t_ap = orbit.time_to_true_anomaly(std::f64::consts::PI, ut);
                if orbit.periapsis() - body.radius_m > 72_000.0 {
                    break;
                }
                p.attitude = Attitude::OrbitPrograde;
                if t_ap < 20.0 || t_ap > orbit.period - 20.0 {
                    if p.propellant_t <= 0.0 && p.stage == 0 {
                        p.stage_next();
                    }
                    p.throttle = 1.0;
                } else {
                    p.throttle = 0.0;
                }
            }
            let out = integrate(&mut p, &body, Some(&atmo), None, ut, dt);
            assert!(!out.crashed, "crashed at ut {ut}");
            ut += dt;
        }
        let orbit = ose_math::Orbit::from_state_vectors(body.mu, p.r, p.v, ut);
        (
            orbit.periapsis() - body.radius_m,
            orbit.apoapsis() - body.radius_m,
            p.propellant_t,
        )
    }

    #[test]
    fn gravity_turn_with_rotation_dynamics_still_reaches_orbit() {
        let mut spec = rocket();
        // booster: 72 t over ~15 m, wheels 15 kN*m, gimbal 2000 kN * sin(3 deg) * 7 m
        spec.stages[0].moi_tm2 = 72.0 * 15.0 * 15.0 / 12.0;
        spec.stages[0].wheel_torque_knm = 15.0;
        spec.stages[0].gimbal_torque_knm = 2000.0 * 3f64.to_radians().sin() * 7.0;
        // upper: 16 t over ~5 m
        spec.stages[1].moi_tm2 = 16.0 * 5.0 * 5.0 / 12.0;
        spec.stages[1].wheel_torque_knm = 15.0;
        spec.stages[1].gimbal_torque_knm = 250.0 * 3f64.to_radians().sin() * 2.0;
        let (pe, ap, prop) = fly_gravity_turn(spec);
        assert!(
            pe > 70_000.0 && ap < 250_000.0,
            "orbit {pe:.0} x {ap:.0} m, prop {prop:.2} t"
        );
    }

    #[test]
    fn staging_drops_structure_with_leftover_fuel_or_keeps_the_fuel() {
        let mut p = Powered::new(
            rocket(),
            DVec3::new(700_000.0, 0.0, 0.0),
            DVec3::ZERO,
            false,
        );
        // burn 10 of the booster's 50 t, then stage: 22 t dry end, next stage starts at 16 t
        p.propellant_t -= 10.0;
        p.mass_t -= 10.0;
        let before = p.mass_t;
        let out = p.stage_next();
        assert!(out.fired);
        assert!(
            (out.dropped_t - (6.0 + 40.0)).abs() < 1e-9,
            "dropped {}",
            out.dropped_t
        );
        assert!(
            (p.mass_t + out.dropped_t - before).abs() < 1e-9,
            "mass is conserved"
        );
        // no jettison: an extra stage that only lights another engine keeps the leftover propellant
        let mut spec = rocket();
        spec.stages[1].start_mass_t = 22.0; // = booster dry end: nothing separates
        spec.stages[1].propellant_t = 12.0;
        let mut q = Powered::new(spec, DVec3::new(700_000.0, 0.0, 0.0), DVec3::ZERO, false);
        q.propellant_t -= 30.0;
        q.mass_t -= 30.0;
        let before = q.mass_t;
        let out = q.stage_next();
        assert!(out.fired && out.dropped_t == 0.0);
        assert!((q.mass_t - before).abs() < 1e-9);
        assert!((q.propellant_t - (12.0 + 20.0)).abs() < 1e-9);
        assert!(!q.stage_next().fired, "nothing after the last stage");
    }

    #[test]
    fn hard_landing_crashes_soft_landing_does_not() {
        let body = kerbin();
        let (r0, _) = surface_state(&body, 0.0, 0.0, 0.0);
        let up = r0.normalized();
        // drop from 50 m with no thrust: ~31 m/s at impact -> crash
        let mut p = Powered::new(
            rocket(),
            up * (body.radius_m + 50.0),
            DVec3::new(0.0, 0.0, 1.0).cross(up * (body.radius_m + 50.0))
                * (std::f64::consts::TAU / body.rotation_period_s),
            false,
        );
        let mut crashed = false;
        for i in 0..1000 {
            if integrate(&mut p, &body, None, None, i as f64 * 0.02, 0.02).crashed {
                crashed = true;
                break;
            }
        }
        assert!(crashed);
        let site = lat_lon(&body, p.r, 0.0);
        let distance = p.r.length();
        let before = p.r;
        integrate(&mut p, &body, None, None, 0.0, 2.0);
        let after = lat_lon(&body, p.r, 2.0);
        assert!((after.0 - site.0).abs() < 1e-9 && (after.1 - site.1).abs() < 1e-9);
        assert!((p.r.length() - distance).abs() < 1e-8);
        assert!(
            (p.r - before).length() > 100.0,
            "the wreck co-rotates instead of sliding away from its explosion"
        );
        // descend at 2 m/s from 20 m: lands
        let mut q = Powered::new(
            rocket(),
            up * (body.radius_m + 20.0),
            DVec3::new(0.0, 0.0, 1.0).cross(up * (body.radius_m + 20.0))
                * (std::f64::consts::TAU / body.rotation_period_s)
                - up * 2.0,
            false,
        );
        q.attitude = Attitude::Up;
        // hover-ish throttle: thrust = weight -> throttle = m g / T_vac
        let g = body.mu / (body.radius_m * body.radius_m);
        q.stage_next();
        // a little under hover so it keeps descending as propellant burns off
        q.throttle = (0.9 * q.mass_t * g / q.spec.stages[1].thrust_vac_kn).min(1.0);
        let mut landed = false;
        for i in 0..5000 {
            let out = integrate(&mut q, &body, None, None, i as f64 * 0.02, 0.02);
            assert!(!out.crashed, "soft descent crashed at step {i}");
            if out.touched_down {
                landed = true;
                break;
            }
        }
        assert!(landed);
        assert_eq!(situation(&q, &body, None), Situation::Landed);
    }
}

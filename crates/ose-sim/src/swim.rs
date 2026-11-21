//! A Kerbal in the water, as KSP2 has it (`WaterDetectKerbalCharacterStateHandler`,
//! `SwimKerbalCharacterStateHandler`, the `eva_kerbal` part's `Data_Kerbal`).
//!
//! KSP2 counts a Kerbal as floating when the bottom of its capsule is at least `SwimMinDepth` (0.25 m) under
//! the surface, or when it is falling and touches the water at all. Floating, it swims: the water holds it up
//! by the same displacement buoyancy as any part (the `eva_kerbal` drag cube, `buoyancy` 0.25), W/S/A/D push
//! it level along the camera's heading with `SwimSlowMoveForceCurve` (0.8 kN at every gravity), Shift times
//! `SwimFastMoveForceMagnitudeMultiplier` (1.5) after easing in at `NormalToSprintLerpSpeed`, and the water's
//! drag (`PartBehavior.CalculateLiquidDrag`, [`crate::water::WaterDrag`]) sets how fast that goes. It cannot
//! jump or plant a flag while it swims, and with its jetpack pushing it is flown by the pack instead. Where
//! the water is shallower it stands on the bed and walks (KSP2's ground detect finds the bed first).
use crate::{
    bodies::Body,
    flight::{Powered, StepOutcome},
    water,
};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

/// `SwimSlowMoveForceCurve`: 0.8 kN at gravity multipliers 0 to 2 (flat).
pub const SWIM_FORCE_KN: f64 = 0.8;
/// `SwimFastMoveForceMagnitudeMultiplier`: the push with Shift held.
pub const SWIM_FAST_FORCE_MULT: f64 = 1.5;
/// `SwimMinDepth`: how far under the surface the bottom of the Kerbal's capsule must be to float, m.
pub const SWIM_MIN_DEPTH_M: f64 = 0.25;
/// The `eva_kerbal` part's `buoyancy` and `crashTolerance`.
pub const KERBAL_BUOYANCY: f64 = 0.25;
pub const KERBAL_CRASH_TOLERANCE_MPS: f64 = 50.0;
/// The speed of sound used to read the drag curves in the water (they only matter at low Mach here).
const WATER_MACH_SPEED_MPS: f64 = 340.0;

/// The `eva_kerbal` part's `Default` drag cube: 0.8 x 1.1 x 0.8 m, standing up its Y axis.
pub fn kerbal_cube() -> ose_data::DragCube {
    ose_data::DragCube {
        name: "Default".into(),
        area: [0.75, 0.75, 0.6, 0.6, 0.85, 0.85],
        drag: [0.92, 0.92, 0.7, 0.97, 0.95, 0.95],
        depth: [0.4, 0.4, 0.4, 0.7, 0.4, 0.4],
        drag_multiplier: [1.0; 6],
        center: DVec3::ZERO,
        size: DVec3::new(0.8, 1.1, 0.8),
    }
}

/// Water a Kerbal displaces when fully under, m^3 (`CalculateDisplacement` of its cube).
pub fn kerbal_displacement_m3() -> f64 {
    static V: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *V.get_or_init(|| water::cube_displacement_m3(&kerbal_cube()))
}

/// A Kerbal's water state while it is in the water: its drag easing from the splash to the floating value,
/// and whether it was floating (swimming) rather than standing on the bed this step.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct InWater {
    pub drag_scalar: f64,
    pub splash_s: f64,
    pub swimming: bool,
}

/// The water at a point: the surface and the bed under it (radii from the body's centre, m) and its density,
/// t/m^3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sea {
    pub surface_m: f64,
    pub floor_m: f64,
    pub density: f64,
}

/// The water at `r`, if there is any: a body with an ocean where the land (or a built site) is below sea
/// level.
pub fn sea(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    r: DVec3,
    ut: f64,
) -> Option<Sea> {
    let ocean = body.ocean.as_ref()?;
    let floor = crate::surface::solid_radius(body, terrain, r, ut);
    (floor < body.radius_m - 1e-5).then(|| Sea {
        surface_m: body.radius_m,
        floor_m: floor,
        density: ocean.density.max(0.0),
    })
}

/// How far under the surface the bottom of the Kerbal's capsule is, m (negative above it).
pub fn immersion_m(p: &Powered, sea: &Sea) -> f64 {
    sea.surface_m - (p.r.length() - p.ground_clearance_m)
}

/// Whether the water takes this Kerbal this step (`WaterDetect` sets `IsFloatingInWater`, and the swim
/// handler then runs unless the jetpack is in use): its capsule's bottom `SwimMinDepth` under, or any of it
/// under while it is not standing on something. A Kerbal on a ladder, standing on a craft, or flying its
/// jetpack (RCS on and a push asked for) is not swimming.
pub fn takes(p: &Powered, sea: &Sea) -> bool {
    // standing on a craft's hull (one floating beside it, or a sunk one under it, is not underfoot)
    let on_deck = p.landed && p.deck.is_some_and(|d| d.radius_m >= p.r.length() - 0.01);
    if !p.walker || p.crashed || on_deck {
        return false;
    }
    if p.groups.rcs && p.rcs.translate.length_squared() > 1e-12 {
        return false;
    }
    let depth = immersion_m(p, sea);
    depth >= SWIM_MIN_DEPTH_M || (!p.landed && depth > 0.0)
}

/// One physics step of a Kerbal in the water. `dir` is the level heading it swims toward (W/S/A/D against
/// the camera), `None` when no key is held; `omega` is the body's spin, rad/s.
pub fn step(
    p: &mut Powered,
    body: &Body,
    sea: &Sea,
    dir: Option<DVec3>,
    omega: f64,
    dt: f64,
    out: &mut StepOutcome,
) {
    let omega_v = DVec3::new(0.0, 0.0, omega);
    let r_mag = p.r.length();
    let up = p.r * (1.0 / r_mag);
    let g = body.mu / (r_mag * r_mag);
    let half = p.ground_clearance_m;
    let max_depth = sea.surface_m - (r_mag - half);
    let min_depth = sea.surface_m - (r_mag + half);
    let mut v_s = p.v - omega_v.cross(p.r);
    let speed = v_s.length();
    let entering = p.eva_water.is_none();
    // striking the water as any part does: over 1.2 times its crash tolerance coming down (or a tenth of its
    // whole speed) breaks it
    if entering
        && !p.landed
        && water::breaks_on_entry(speed, -v_s.dot(up), KERBAL_CRASH_TOLERANCE_MPS)
    {
        p.crashed = true;
        p.throttle = 0.0;
        out.crashed = true;
        out.impact_mps = speed;
        out.impact_velocity = v_s;
        out.normal_impact_mps = (-v_s.dot(up)).max(0.0);
        return;
    }
    let mut drag = p
        .eva_water
        .map(|w| water::WaterDrag {
            scalar: w.drag_scalar,
            splash_s: w.splash_s,
        })
        .unwrap_or_default();
    let mass = p.mass_t.max(1e-6);
    let submerged = water::submerged_share(min_depth, max_depth);
    let ws = water::settings();
    let lift_kn = kerbal_displacement_m3()
        * sea.density
        * ws.buoyancy_scalar
        * KERBAL_BUOYANCY
        * submerged
        * g;
    // the pace eases between the slow and the fast stroke, as the walk eases into the run
    let sprint_to = if p.eva_run { 1.0 } else { 0.0 };
    let ease = crate::flight::SPRINT_LERP_PER_S * dt;
    p.eva_sprint = if (sprint_to - p.eva_sprint).abs() <= ease {
        sprint_to
    } else {
        p.eva_sprint + ease.copysign(sprint_to - p.eva_sprint)
    };
    let stroke_kn = SWIM_FORCE_KN * (1.0 + (SWIM_FAST_FORCE_MULT - 1.0) * p.eva_sprint);
    let push_kn = dir
        .map(|d| d * (stroke_kn * p.throttle.clamp(0.0, 1.0)))
        .unwrap_or(DVec3::ZERO);
    // the water's drag on the Kerbal's cube, standing up (its Y axis) and facing the way it moves
    let horizontal = (v_s - up * v_s.dot(up)).length();
    let dot_abs = if speed > 0.0 {
        (v_s.dot(up) / speed).abs()
    } else {
        1.0
    };
    let travel = if speed > 0.0 {
        DVec3::new(0.0, v_s.dot(up) / speed, horizontal / speed)
    } else {
        DVec3::new(0.0, 1.0, 0.0)
    };
    let damping = drag.step(
        speed,
        horizontal,
        dot_abs,
        entering,
        0.0,
        || {
            crate::aero::Faces::from_cube(&kerbal_cube()).area_drag(
                &crate::aero::settings().drag_curves,
                travel,
                speed / WATER_MACH_SPEED_MPS,
            )
        },
        submerged,
        mass,
        sea.density,
        dt,
    );
    // forces, then the water's damping (PhysX: v' = (v + F dt / m)(1 - c dt)), in the turning ground's frame
    let push_acc = (up * lift_kn + push_kn) * (1.0 / mass);
    v_s = (v_s + (push_acc - up * g) * dt) * (1.0 - damping.linear * dt).max(0.0);
    p.accel_ng_mps2 = push_acc;
    let mut r = p.r + v_s * dt;
    // the bed under the water holds it up
    let floor = sea.floor_m + half;
    let on_bed = r.length() <= floor;
    if on_bed {
        let n = r.normalized();
        r = n * floor;
        v_s = v_s - n * v_s.dot(n).min(0.0);
    }
    let (s, c) = (omega * dt).sin_cos();
    let turn = |v: DVec3| DVec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
    p.r = turn(r);
    v_s = turn(v_s);
    p.v = omega_v.cross(p.r) + v_s;
    // it keeps upright in the water (`ShouldApplyUpright`)
    p.facing = p.r.normalized();
    p.own_top = DVec3::ZERO;
    p.angvel = DVec3::ZERO;
    p.roll_rate = 0.0;
    p.eva_jump = false;
    p.eva_hop = false;
    p.eva_push_s = 0.0;
    p.coasting_since = None;
    p.landed = on_bed;
    p.splashed = true;
    p.eva_water = Some(InWater {
        drag_scalar: drag.scalar,
        splash_s: drag.splash_s,
        swimming: !on_bed || immersion_m(p, sea) >= SWIM_MIN_DEPTH_M,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kerbal_cube_and_constants_are_the_installed_eva_kerbal_part() {
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let k = defs.iter().find(|d| d.id == "eva_kerbal").unwrap();
        let cube = &k.drag_cubes[0];
        let ours = kerbal_cube();
        assert_eq!(cube.area, ours.area);
        assert_eq!(cube.drag, ours.drag);
        assert_eq!(cube.size, ours.size);
        assert_eq!(cube.center, ours.center);
        assert_eq!(k.buoyancy, KERBAL_BUOYANCY);
        assert_eq!(k.crash_tolerance, KERBAL_CRASH_TOLERANCE_MPS);
        assert!((water::displacement_m3(k) - kerbal_displacement_m3()).abs() < 1e-12);
    }
}

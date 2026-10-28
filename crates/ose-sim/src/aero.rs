//! Part drag from the original's six-face drag cubes (`Data_Drag.GetAeroDataForDirection`,
//! `Module_Drag.UpdateDrag`; see `docs/reference-notes/physics-joints-drag.md`).
//!
//! A face pointing into the airflow takes the "tip" curve, one pointing away the "tail" curve and one along the
//! flow the "surface" curve, all scaled by the mach multiplier. The drag force is
//! `AreaDrag * DragCubeMultiplier(0.1) * pseudoRe * DragMultiplier(8) * q_Pa * 0.001` kN, so the equivalent
//! `Cd * A` in m^2 is `0.8 * AreaDrag * pseudoRe`.

use ose_data::{Curve, DragCube};
use ose_math::DVec3;
use serde::Deserialize;

/// Lift multipliers (`PhysicsSettings.LiftMultiplier`, `LiftDragMultiplier`).
pub const LIFT_MULTIPLIER: f64 = 0.036;
pub const LIFT_DRAG_MULTIPLIER: f64 = 0.015;

/// A lifting surface's curves: lift and drag against |sin(angle of attack)|, and their Mach factors.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LiftCurves {
    pub lift: Curve,
    pub lift_mach: Curve,
    pub drag: Curve,
    pub drag_mach: Curve,
}

/// `PhysicsSettings.DefaultLiftingSurfaceCurve` (wings, fins, control surfaces).
pub fn default_lift_curves() -> LiftCurves {
    settings().lift_curves["Default"].clone()
}

pub fn body_lift_curves() -> LiftCurves {
    settings().lift_curves["BodyLift"].clone()
}

/// Selected settings from the reference's loaded Global/PhysicsSettings.json.
#[derive(Debug, Deserialize)]
pub struct AeroSettings {
    pub version: String,
    pub source_sha256: String,
    pub lift_multiplier: f64,
    pub lift_drag_multiplier: f64,
    pub body_lift_multiplier: f64,
    pub drag_multiplier: f64,
    pub drag_cube_multiplier: f64,
    pub drag_curves: DragCurves,
    pub lift_curves: std::collections::BTreeMap<String, LiftCurves>,
}

pub fn settings() -> &'static AeroSettings {
    static SETTINGS: std::sync::OnceLock<AeroSettings> = std::sync::OnceLock::new();
    SETTINGS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/own/aero-settings.json"))
            .expect("validated reference aero settings")
    })
}

/// Lift plus drag of one lifting surface, kN, for travel direction `travel` (unit) through air at dynamic
/// pressure `q_pa` (`Module_LiftingSurface.GetLiftVector` / `GetDragVector`). `axis` is the surface's lift
/// axis in the same frame.
pub fn surface_force(
    c: &LiftCurves,
    surface: &ose_data::LiftingSurface,
    axis: DVec3,
    travel: DVec3,
    q_pa: f64,
    mach: f64,
) -> DVec3 {
    let dot = travel.dot(axis);
    let abs = if surface.omnidirectional {
        dot.abs()
    } else {
        dot.clamp(0.0, 1.0)
    };
    let lift = dot.signum()
        * c.lift.evaluate(abs)
        * c.lift_mach.evaluate(mach)
        * surface.deflection_lift_coeff;
    let drag = c.drag.evaluate(abs) * c.drag_mach.evaluate(mach) * surface.deflection_lift_coeff;
    let mut force = axis * (-q_pa * settings().lift_multiplier * lift);
    if surface.perpendicular_only {
        force = force - travel * force.dot(travel);
    }
    if surface.use_internal_drag_model {
        force = force - travel * (q_pa * drag * settings().lift_drag_multiplier);
    }
    force
}

/// The `PhysicsSettings` drag curves.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DragCurves {
    pub tip: Curve,
    pub surface: Curve,
    pub tail: Curve,
    pub multiplier: Curve,
    pub cd: Curve,
    pub cd_power: Curve,
    pub pseudo_reynolds: Curve,
}

impl Default for DragCurves {
    fn default() -> Self {
        settings().drag_curves.clone()
    }
}

/// Face normals in the cube order `+X, -X, +Y, -Y, +Z, -Z` (part space, Unity axes).
const NORMALS: [(f64, f64, f64); 6] = [
    (1.0, 0.0, 0.0),
    (-1.0, 0.0, 0.0),
    (0.0, 1.0, 0.0),
    (0.0, -1.0, 0.0),
    (0.0, 0.0, 1.0),
    (0.0, 0.0, -1.0),
];

/// A part's drag faces after occlusion by attached parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Faces {
    pub area: [f64; 6],
    pub drag: [f64; 6],
}

impl Faces {
    pub fn from_cube(c: &DragCube) -> Faces {
        Faces {
            area: c.area,
            drag: c.drag,
        }
    }

    /// Cover the face an attach node sits on by the node's cross-section (`SetExposedArea`).
    pub fn occlude(&mut self, node_dir: DVec3, node_size: i32) {
        let diameter = match node_size {
            s if s <= 0 => 0.625,
            s => 1.25 * s as f64,
        };
        let covered = std::f64::consts::FRAC_PI_4 * diameter * diameter;
        let (ax, ay, az) = (node_dir.x.abs(), node_dir.y.abs(), node_dir.z.abs());
        let face = if ay >= ax && ay >= az {
            if node_dir.y >= 0.0 {
                2
            } else {
                3
            }
        } else if ax >= az {
            if node_dir.x >= 0.0 {
                0
            } else {
                1
            }
        } else if node_dir.z >= 0.0 {
            4
        } else {
            5
        };
        self.area[face] = (self.area[face] - covered).max(0.0);
    }

    /// `Data_Drag.GetAeroDataForDirection` lift contribution of the exposed faces, part-local.
    pub fn body_lift_vector(&self, lift_curve: &Curve, direction: DVec3) -> DVec3 {
        NORMALS.iter().enumerate().fold(DVec3::ZERO, |sum, (i, n)| {
            let normal = DVec3::new(n.0, n.1, n.2);
            let dot = normal.dot(direction);
            if dot > 0.0 {
                sum - normal * (dot * self.area[i] * self.drag[i] * lift_curve.evaluate(dot))
            } else {
                sum
            }
        })
    }

    /// The original's `AreaDrag` for travel direction `d` (unit, part space) at `mach`.
    pub fn area_drag(&self, curves: &DragCurves, d: DVec3, mach: f64) -> f64 {
        let mult = curves.multiplier.evaluate(mach);
        let (tip, surface, tail) = (
            curves.tip.evaluate(mach),
            curves.surface.evaluate(mach),
            curves.tail.evaluate(mach),
        );
        let power = curves.cd_power.evaluate(mach);
        let mut area_drag = 0.0;
        for (i, n) in NORMALS.iter().enumerate() {
            let dot = n.0 * d.x + n.1 * d.y + n.2 * d.z;
            let dot_n = (dot + 1.0) * 0.5;
            let curve_val = if dot_n <= 0.5 {
                tail + (surface - tail) * (dot_n * 2.0)
            } else {
                surface + (tip - surface) * ((dot_n - 0.5) * 2.0)
            } * mult;
            let cd = if self.drag[i] < 1.0 {
                curves.cd.evaluate(self.drag[i]).max(0.0).powf(power)
            } else {
                self.drag[i]
            };
            area_drag += self.area[i] * curve_val * cd;
        }
        area_drag
    }
}

impl Faces {
    /// The part's area facing travel direction `d` (part space) and its drag coefficient there
    /// (`GetAeroDataForDirection`: `AreaAirstream`, `DragCoeff = AreaDrag / Area`).
    pub fn airstream(&self, curves: &DragCurves, d: DVec3, mach: f64) -> (f64, f64) {
        let mult = curves.multiplier.evaluate(mach);
        let (tip, surface, tail) = (
            curves.tip.evaluate(mach),
            curves.surface.evaluate(mach),
            curves.tail.evaluate(mach),
        );
        let mut airstream = 0.0;
        let mut area = 0.0;
        for (i, n) in NORMALS.iter().enumerate() {
            let dot = n.0 * d.x + n.1 * d.y + n.2 * d.z;
            let dot_n = (dot + 1.0) * 0.5;
            let curve_val = if dot_n <= 0.5 {
                tail + (surface - tail) * (dot_n * 2.0)
            } else {
                surface + (tip - surface) * ((dot_n - 0.5) * 2.0)
            } * mult;
            airstream += self.area[i] * dot.clamp(0.0, 1.0);
            area += self.area[i] * curve_val;
        }
        let cd = if area > 0.0 {
            self.area_drag(curves, d, mach) / area
        } else {
            0.0
        };
        (airstream, cd)
    }

    /// Area open to the air, m^2 (`WettedArea`: the exposed faces summed).
    pub fn wetted(&self) -> f64 {
        self.area.iter().sum()
    }
}

/// Rotate `v` by a unit quaternion (x, y, z, w).
pub fn rotate(q: [f64; 4], v: DVec3) -> DVec3 {
    rotate_inverse([-q[0], -q[1], -q[2], q[3]], v)
}

/// Equivalent `Cd * A`, m^2, for an `AreaDrag` at `density * speed` (kg/m^3 * m/s).
pub fn cda_m2(curves: &DragCurves, area_drag: f64, rho_v: f64) -> f64 {
    settings().drag_multiplier
        * settings().drag_cube_multiplier
        * area_drag
        * curves.pseudo_reynolds.evaluate(rho_v)
}

/// Rotate `v` by the inverse of a unit quaternion (x, y, z, w).
pub fn rotate_inverse(q: [f64; 4], v: DVec3) -> DVec3 {
    let (qx, qy, qz, qw) = (-q[0], -q[1], -q[2], q[3]);
    // v' = v + 2w (u x v) + 2 u x (u x v), u = (qx, qy, qz)
    let u = DVec3::new(qx, qy, qz);
    let t = u.cross(v) * 2.0;
    v + t * qw + u.cross(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)] // Preserve the serialized reference key, not an idealized angle.
    fn exported_reference_settings_preserve_named_families_and_runtime_multipliers() {
        let data = settings();
        assert_eq!(data.version, "0.1.16");
        assert_eq!(data.source_sha256.len(), 64);
        assert_eq!(data.lift_multiplier, 0.036);
        assert_eq!(data.lift_drag_multiplier, 0.015);
        assert_eq!(data.body_lift_multiplier, 18.0);
        assert_eq!(data.drag_multiplier * data.drag_cube_multiplier, 0.8);
        assert_eq!(data.lift_curves.len(), 4);
        assert_eq!(
            data.lift_curves["Default"].lift.evaluate(0.7071068),
            0.5926583
        );
        assert_eq!(data.lift_curves["BodyLift"].lift.evaluate(0.7071068), 1.0);
        assert_eq!(data.lift_curves["SpeedBrake"].lift.evaluate(0.7071068), 0.0);
        assert_eq!(
            data.lift_curves["CapsuleBottom"].lift_mach.evaluate(5.0),
            0.0625
        );
        assert_eq!(data.drag_curves.cd.evaluate(0.7), 0.35);
    }

    fn tank() -> Faces {
        // the 1.25 m methalox tank's cube (fueltank_1v_inline_methalox_1x4)
        Faces {
            area: [
                2.500088, 2.500088, 1.21755731, 1.21755731, 2.379063, 2.379063,
            ],
            drag: [
                0.7823667,
                0.7823462,
                0.8810437,
                0.877301753,
                0.765476644,
                0.7651058,
            ],
        }
    }

    #[test]
    fn nose_on_is_far_less_drag_than_broadside() {
        let c = DragCurves::default();
        let t = tank();
        let nose = t.area_drag(&c, DVec3::new(0.0, 1.0, 0.0), 0.3);
        let side = t.area_drag(&c, DVec3::new(1.0, 0.0, 0.0), 0.3);
        assert!(side > 1.3 * nose, "side {side} nose {nose}");
        // subsonic: tip and tail at 1, the multiplier at 0.5, so nose-on is half of both end faces' Cd * area
        let cd = |x: f64| c.cd.evaluate(x);
        let expect = 0.5 * (1.21755731 * cd(0.8810437) + 1.21755731 * cd(0.877301753))
            + 0.5 * 0.02 * (2.500088 * cd(0.7823667) * 2.0 + 2.379063 * cd(0.765476644) * 2.0);
        assert!((nose - expect).abs() < 0.02 * expect, "{nose} vs {expect}");
    }

    #[test]
    fn stacked_faces_are_covered_and_drag_rises_through_mach_one() {
        let c = DragCurves::default();
        let mut t = tank();
        t.occlude(DVec3::new(0.0, 1.0, 0.0), 1);
        t.occlude(DVec3::new(0.0, -1.0, 0.0), 1);
        assert_eq!(t.area[2], 0.0);
        assert_eq!(t.area[3], 0.0);
        let free = tank();
        let d = DVec3::new(0.0, 1.0, 0.0);
        assert!(t.area_drag(&c, d, 0.3) < 0.1 * free.area_drag(&c, d, 0.3));
        assert!(free.area_drag(&c, d, 1.1) > 2.0 * free.area_drag(&c, d, 0.5));
    }

    #[test]
    fn a_bare_tank_nose_on_at_100_mps_sea_level() {
        // parity target from the notes: drag on a 1.25 m stack at 100 m/s at sea level
        let c = DragCurves::default();
        let area_drag = tank().area_drag(&c, DVec3::new(0.0, 1.0, 0.0), 100.0 / 340.0);
        let cda = cda_m2(&c, area_drag, 1.225 * 100.0);
        let f_n = 0.5 * 1.225 * 100.0f64.powi(2) * cda;
        // about Cd 0.6 on the 1.23 m^2 end: a few kN
        assert!((2_000.0..8_000.0).contains(&f_n), "{f_n} N");
    }

    #[test]
    fn inverse_rotation_undoes_a_quarter_turn() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        // 90 degrees about +Y takes +Z to +X
        let q = [0.0, s, 0.0, s];
        let v = rotate_inverse(q, DVec3::new(1.0, 0.0, 0.0));
        assert!((v.z - 1.0).abs() < 1e-9 && v.x.abs() < 1e-9, "{v:?}");
    }
}

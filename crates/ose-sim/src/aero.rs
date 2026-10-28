//! Part drag from the original's six-face drag cubes (`Data_Drag.GetAeroDataForDirection`,
//! `Module_Drag.UpdateDrag`; see `docs/reference-notes/physics-joints-drag.md`).
//!
//! A face pointing into the airflow takes the "tip" curve, one pointing away the "tail" curve and one along the
//! flow the "surface" curve, all scaled by the mach multiplier. The drag force is
//! `AreaDrag * DragCubeMultiplier(0.1) * pseudoRe * DragMultiplier(8) * q_Pa * 0.001` kN, so the equivalent
//! `Cd * A` in m^2 is `0.8 * AreaDrag * pseudoRe`.

use ose_data::{Curve, CurveKey, DragCube};
use ose_math::DVec3;

fn curve(keys: &[(f64, f64, f64, f64)]) -> Curve {
    Curve {
        keys: keys
            .iter()
            .map(|&(t, v, i, o)| CurveKey {
                t,
                v,
                in_tangent: i,
                out_tangent: o,
            })
            .collect(),
    }
}

/// Lift multipliers (`PhysicsSettings.LiftMultiplier`, `LiftDragMultiplier`).
pub const LIFT_MULTIPLIER: f64 = 0.036;
pub const LIFT_DRAG_MULTIPLIER: f64 = 0.015;

/// A lifting surface's curves: lift and drag against |sin(angle of attack)|, and their Mach factors.
#[derive(Clone, Debug, PartialEq)]
pub struct LiftCurves {
    pub lift: Curve,
    pub lift_mach: Curve,
    pub drag: Curve,
    pub drag_mach: Curve,
}

/// `PhysicsSettings.DefaultLiftingSurfaceCurve` (wings, fins, control surfaces).
pub fn default_lift_curves() -> LiftCurves {
    LiftCurves {
        lift: curve(&[
            (0.0, 0.0, 0.0, 1.965926),
            (0.258819, 0.5114774, 1.990092, 1.905806),
            (0.5, 0.9026583, 133.0 / 188.0, -133.0 / 188.0),
            (
                std::f64::consts::FRAC_1_SQRT_2,
                0.5926583,
                -2.087948,
                -1.990095,
            ),
            (1.0, 0.0, -2.014386, -2.014386),
        ]),
        lift_mach: curve(&[
            (0.0, 1.0, 0.0, 0.0),
            (0.3, 0.5, -1.671345, -0.8273422),
            (1.0, 0.125, -0.0005291355, -0.02625772),
            (5.0, 0.0625, 0.0, 0.0),
            (25.0, 0.05, 0.0, 0.0),
        ]),
        drag: curve(&[
            (0.0, 0.01, 0.0, 0.0),
            (0.3420201, 0.06, 0.1750731, 0.1750731),
            (0.5, 0.24, 2.60928, 2.60928),
            (std::f64::consts::FRAC_1_SQRT_2, 1.7, 3.349777, 3.349777),
            (1.0, 2.4, 1.387938, 0.0),
        ]),
        drag_mach: curve(&[
            (0.0, 0.35, 0.0, -0.8463008),
            (0.15, 0.125, 0.0, 0.0),
            (0.9, 0.275, 0.541598, 0.541598),
            (1.1, 0.75, 0.0, 0.0),
            (1.4, 0.4, -0.3626955, -0.3626955),
            (1.6, 0.35, -0.1545923, -0.1545923),
            (2.0, 0.3, -0.09013031, -0.09013031),
            (5.0, 0.22, 0.0, 0.0),
            (25.0, 0.3, 0.0006807274, 0.0),
        ]),
    }
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
    let drag = if surface.use_internal_drag {
        c.drag.evaluate(abs) * c.drag_mach.evaluate(mach) * surface.deflection_lift_coeff
    } else {
        0.0
    };
    let mut lift_force = axis * (-q_pa * LIFT_MULTIPLIER * lift);
    if surface.perpendicular_only {
        lift_force = lift_force - travel * lift_force.dot(travel);
    }
    lift_force - travel * (q_pa * drag * LIFT_DRAG_MULTIPLIER)
}

/// The `PhysicsSettings` drag curves.
#[derive(Clone, Debug, PartialEq)]
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
        DragCurves {
            tip: curve(&[
                (0.0, 1.0, 0.0, 0.0),
                (0.85, 1.19, 0.696, 0.696),
                (1.1, 2.83, 0.730, 0.730),
                (5.0, 4.0, 0.0, 0.0),
            ]),
            surface: curve(&[
                (0.0, 0.02, 0.0, 0.0),
                (0.85, 0.02, 0.0, 0.0),
                (0.9, 0.01524, -0.0794, -0.0794),
                (1.1, 0.0025, -0.00528, -0.00194),
                (2.0, 0.002083, -2.3e-5, -2.3e-5),
                (5.0, 0.003333, -1.8e-4, -1.8e-4),
                (25.0, 0.001429, -7.1e-5, 0.0),
            ]),
            tail: curve(&[
                (0.0, 1.0, 0.0, 0.0),
                (0.85, 1.0, 0.0, 0.0),
                (1.1, 0.25, -0.0222, -0.0249),
                (1.4, 0.22, -0.0339, -0.0339),
                (5.0, 0.15, -0.0012, -0.0012),
                (25.0, 0.14, 0.0, 0.0),
            ]),
            multiplier: curve(&[
                (0.0, 0.5, 0.0, 0.0),
                (0.85, 0.5, 0.0, 0.0),
                (1.1, 1.3, 0.0, -0.0081),
                (2.0, 0.7, -0.110, -0.110),
                (5.0, 0.6, 0.0, 0.0),
                (10.0, 0.85, 0.022, 0.022),
                (14.0, 0.9, 0.0077, 0.0077),
                (25.0, 0.95, 0.0, 0.0),
            ]),
            cd: curve(&[
                (0.05, 0.0025, 0.15, 0.15),
                (0.4, 0.15, 0.396, 0.396),
                (0.7, 0.35, 0.907, 0.907),
                (0.75, 0.45, 3.21, 3.21),
                (0.8, 0.66, 3.50, 3.50),
                (0.85, 0.8, 2.21, 2.21),
                (0.9, 0.89, 1.1, 1.1),
                (1.0, 1.0, 1.0, 1.0),
            ]),
            cd_power: curve(&[
                (0.0, 1.0, 0.0, 0.00716),
                (0.85, 1.25, 0.778, 0.778),
                (1.1, 2.5, 0.249, 0.249),
                (5.0, 3.0, 0.0, 0.0),
            ]),
            pseudo_reynolds: curve(&[
                (0.0, 4.0, -111111.0, -2975.0),
                (1e-4, 3.0, -251.0, -251.0),
                (0.01, 2.0, -19.6, -19.6),
                (0.1, 1.2, -0.785, -0.391),
                (1.0, 1.0, 0.0, 0.0),
                (100.0, 1.0, 0.0, 0.0),
                (200.0, 0.82, 0.0, 0.0),
                (500.0, 0.86, 1.9e-4, 1.9e-4),
                (1000.0, 0.9, 1.5e-5, 1.5e-5),
                (10000.0, 0.95, 0.0, 0.0),
            ]),
        }
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
    0.8 * area_drag * curves.pseudo_reynolds.evaluate(rho_v)
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

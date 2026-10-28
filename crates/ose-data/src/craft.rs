//! A craft: a tree of part instances with attachment, placement, staging and stored resources.
//!
//! Imported from KSP2 workspace/stock assembly files by `ose-importer`, uploaded by clients, spawned by the
//! server. Geometry is in the craft's own frame (metres, Unity Y-up as authored; converted by the renderer).

use crate::{PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quat {
    /// Unit rotation for a mounted part. Legacy zero/invalid rotations mean identity.
    pub fn normalized(self) -> Self {
        let norm2 = self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w;
        if !norm2.is_finite() || norm2 < 1e-12 {
            return Self {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            };
        }
        let inverse = norm2.sqrt().recip();
        Self {
            x: self.x * inverse,
            y: self.y * inverse,
            z: self.z * inverse,
            w: self.w * inverse,
        }
    }
}

/// Base and accent colours, sRGB bytes (the VAB's Colors tool).
pub type Paint = ([u8; 3], [u8; 3]);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CraftPart {
    /// Stable id within the craft (the original part GUID when imported).
    pub guid: String,
    /// `PartDef::id`.
    pub part_id: String,
    /// Index of the parent part in `Craft::parts`; `None` for the root.
    pub parent: Option<usize>,
    /// Attach node on this part used for the parent link, and the node on the parent.
    pub own_node: Option<String>,
    pub parent_node: Option<String>,
    pub local_position: DVec3,
    pub local_rotation: Quat,
    pub stage_index: i32,
    pub fuel_crossfeed: bool,
    /// Stored resource units by resource id.
    pub resources: Vec<StoredResource>,
    /// A fuel line (compound part) hanging on its parent tank feeds this part (index in `Craft::parts`).
    // (no skip_serializing_if: crafts also travel in postcard, which needs every field)
    #[serde(default)]
    pub fuel_line_target: Option<usize>,
    /// This part's own paint over the craft's (base, accent), when set.
    #[serde(default)]
    pub colors: Option<Paint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredResource {
    pub resource: String,
    pub stored_units: f64,
    pub capacity_units: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Craft {
    pub name: String,
    pub description: String,
    pub parts: Vec<CraftPart>,
    /// Stages by index; each lists part indices activated by that stage. The highest index fires first (as
    /// the original numbers them; `staging::delta_v_by_stage` and the sim follow this).
    pub stages: Vec<Vec<usize>>,
    /// Loose assemblies kept beside the craft in the editor (each its own tree, with its own stages): saved
    /// and loaded with it, left on the ground by a launch.
    #[serde(default)]
    pub loose: Vec<Craft>,
    /// The craft's paint (VAB Colors): base and accent, sRGB; `None` paints it in the agency's colours.
    #[serde(default)]
    pub colors: Option<Paint>,
}

fn layout_rotate(q: [f64; 4], v: DVec3) -> DVec3 {
    let u = DVec3::new(q[0], q[1], q[2]);
    let t = u.cross(v) * 2.0;
    v + t * q[3] + u.cross(t)
}

fn quat_mul(a: Quat, b: Quat) -> Quat {
    Quat {
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
    }
}

/// A craft built nose-forward (the root part pointing along the floor, as planes and rovers are built in
/// the hangar) turned so its pointing direction is craft +Y like a rocket's: the flight model thrusts and
/// steers along +Y. Its old top (+Y) becomes Unity +Z / right-handed craft -Z.
/// `None` for a craft that already points up.
pub fn level_layout(craft: &Craft) -> Option<Craft> {
    let root = craft.parts.iter().position(|p| p.parent.is_none())?;
    let q = craft.parts[root].local_rotation.normalized();
    let c = layout_rotate([q.x, q.y, q.z, q.w], DVec3::new(0.0, 1.0, 0.0));
    if c.y.abs() >= 0.5 {
        return None;
    }
    // the shortest turn from `c` to +Y
    let y = DVec3::new(0.0, 1.0, 0.0);
    let axis = c.cross(y);
    let angle = c.dot(y).clamp(-1.0, 1.0).acos();
    let axis = if axis.length() > 1e-9 {
        axis.normalized()
    } else {
        DVec3::new(1.0, 0.0, 0.0)
    };
    let (sh, ch) = (angle * 0.5).sin_cos();
    let r = Quat {
        x: axis.x * sh,
        y: axis.y * sh,
        z: axis.z * sh,
        w: ch,
    };
    // Unity positions are mirrored in Z into the right-handed craft frame. Keep
    // the hangar belly on craft +Z, which level flight carries toward the ground.
    let r = quat_mul(
        Quat {
            x: 0.0,
            y: 1.0,
            z: 0.0,
            w: 0.0,
        },
        r,
    );
    Some(rotated(craft, r))
}

/// Rotate every part about the craft origin.
pub fn rotated(craft: &Craft, r: Quat) -> Craft {
    let r = r.normalized();
    let mut out = craft.clone();
    for part in &mut out.parts {
        part.local_position = layout_rotate([r.x, r.y, r.z, r.w], part.local_position);
        let pq = part.local_rotation.normalized();
        part.local_rotation = quat_mul(r, pq);
    }
    out
}
/// Mass summary of a craft against a part and resource catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct MassSummary {
    pub dry_t: f64,
    pub resources_t: f64,
    pub unknown_parts: Vec<String>,
}

impl MassSummary {
    pub fn wet_t(&self) -> f64 {
        self.dry_t + self.resources_t
    }
}

/// Vacuum delta-v estimate for a craft: Tsiolkovsky over the whole craft using the mass-weighted best
/// vacuum Isp of its engines, with every stored propellant counted as burnable. A per-stage model comes
/// with the resource flow graph; this is the number for a first readout.
pub fn delta_v_vacuum_mps(craft: &Craft, parts: &[PartDef], resources: &[ResourceDef]) -> f64 {
    let m = craft.mass(parts, resources);
    if m.resources_t <= 0.0 || m.wet_t() <= 0.0 {
        return 0.0;
    }
    let by_id: HashMap<&str, &PartDef> = parts.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut best_isp: f64 = 0.0;
    for p in &craft.parts {
        if let Some(def) = by_id.get(p.part_id.as_str()) {
            for e in &def.engines {
                best_isp = best_isp.max(e.isp_curve.evaluate(0.0));
            }
        }
    }
    if best_isp <= 0.0 {
        return 0.0;
    }
    best_isp * crate::G0 * (m.wet_t() / m.dry_t).ln()
}

impl Craft {
    /// Children of part `i`.
    pub fn children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.parts
            .iter()
            .enumerate()
            .filter(move |(_, p)| p.parent == Some(i))
            .map(|(j, _)| j)
    }

    /// True when every part's parent index is valid and exactly one part is the root.
    pub fn is_tree(&self) -> bool {
        let roots = self.parts.iter().filter(|p| p.parent.is_none()).count();
        roots == 1
            && self
                .parts
                .iter()
                .all(|p| p.parent.is_none_or(|i| i < self.parts.len()))
    }

    pub fn mass(&self, parts: &[PartDef], resources: &[ResourceDef]) -> MassSummary {
        let by_id: HashMap<&str, &PartDef> = parts.iter().map(|p| (p.id.as_str(), p)).collect();
        let res_mass: HashMap<&str, f64> = resources
            .iter()
            .filter_map(|r| match &r.kind {
                ResourceKind::Basic {
                    mass_per_unit_t, ..
                } => Some((r.id.as_str(), *mass_per_unit_t)),
                ResourceKind::Recipe { .. } => None,
            })
            .collect();
        let mut dry = 0.0;
        let mut wet = 0.0;
        let mut unknown = Vec::new();
        for p in &self.parts {
            match by_id.get(p.part_id.as_str()) {
                Some(def) => dry += def.mass_t,
                None => unknown.push(p.part_id.clone()),
            }
            for r in &p.resources {
                wet += r.stored_units * res_mass.get(r.resource.as_str()).copied().unwrap_or(0.0);
            }
        }
        MassSummary {
            dry_t: dry,
            resources_t: wet,
            unknown_parts: unknown,
        }
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::Quat;
    #[test]
    fn mount_rotations_normalize_and_legacy_invalid_values_mean_identity() {
        let q = Quat {
            x: 0.0,
            y: 0.0,
            z: 3.0,
            w: 3.0,
        }
        .normalized();
        assert!((q.z - 0.5_f64.sqrt()).abs() < 1e-15);
        assert!((q.w - 0.5_f64.sqrt()).abs() < 1e-15);
        for q in [
            Quat::default(),
            Quat {
                x: f64::NAN,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            Quat {
                x: 0.0,
                y: f64::INFINITY,
                z: 0.0,
                w: 1.0,
            },
        ] {
            assert_eq!(
                q.normalized(),
                Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0
                }
            );
        }
    }
}

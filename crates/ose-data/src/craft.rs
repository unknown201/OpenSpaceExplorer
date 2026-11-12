//! A craft: a tree of part instances with attachment, placement, staging and stored resources.
//!
//! Imported from KSP2 workspace/stock assembly files by `ose-importer`, uploaded by clients, spawned by the
//! server. Geometry is in the craft's own frame (metres, Unity Y-up as authored; converted by the renderer).

use crate::{PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Shipped EAS-4 extension limits override the C# class-default maximum of 100 m.
pub const STRUT_MIN_LENGTH_M: f64 = 0.01;
pub const STRUT_MAX_LENGTH_M: f64 = 10.0;

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
    /// On the first anchor of a two-anchor (compound) part, a fuel line or a strut: its far anchor, the same
    /// part hung on the part at the other end (index in `Craft::parts`), as the original's
    /// `Data_CompoundPart.OtherEndAnchor` on its `IsFirstAnchor` half. A fuel line lets the far part draw
    /// from the part the first anchor hangs on; a strut braces the two (`Craft::compound_ends`).
    // (no skip_serializing_if: crafts also travel in postcard, which needs every field)
    #[serde(default)]
    pub fuel_line_target: Option<usize>,
    /// This part's own paint over the craft's (base, accent), when set.
    #[serde(default)]
    pub colors: Option<Paint>,
    #[serde(default)]
    pub flight: Option<crate::flight_geometry::PartFlight>,
    /// The part's actions bound to action groups (`Data_ModuleActions.customActionMappings`): action key
    /// (see `ACTIONS`) and the groups as the original's `KSPActionGroup` bits (Gear 2, Lights 4, RCS 8,
    /// SAS 0x10, Brakes 0x20, Abort 0x40, SolarPanels 0x80, Custom01 0x10000 .. Custom10 0x2000000).
    #[serde(default)]
    pub actions: Vec<(String, u32)>,
    /// The symmetry set the part belongs to (the original's `SymmetrySet`): parts placed together with
    /// symmetry share one nonzero id, and the editor keeps them alike; 0 for none.
    #[serde(default)]
    pub symmetry: u32,
    /// A procedural wing's shape (`Data_ProceduralPart`); `None` keeps the authored start shape.
    #[serde(default)]
    pub shape: Option<crate::procedural::WingShape>,
    /// A procedural fairing's wall (`Data_Fairing.CrossSections`); `None` wraps whatever stands on the base.
    #[serde(default)]
    pub fairing: Option<crate::fairing::FairingShape>,
    /// A parachute's deploy settings when changed from the part's own (`Data_Parachute` `DeploymentMode`,
    /// `minAirPressureToOpen`, `deployAltitude`); `None` keeps the part's defaults.
    #[serde(default)]
    pub chute: Option<ChuteSettings>,
}

/// When an armed parachute may open (`Data_Parachute.DeployMode`): only when opening is predicted to keep the
/// canopy below its maximum temperature (`Safe`, every chute's default), also when it may overheat within
/// `SecondsForRisky` (`Risky`), or at once whatever the air and speed (`Immediate`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChuteMode {
    #[default]
    Safe,
    Risky,
    Immediate,
}

/// A parachute's settings in its part window: the deploy mode, the air pressure it opens partly in (atm,
/// 0.01..0.75 in steps of 0.01) and the height above the ground it opens fully at (m, 50..5000 in steps of
/// 50); copied across the part's symmetry set.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChuteSettings {
    pub mode: ChuteMode,
    pub min_pressure_atm: f32,
    pub deploy_altitude_m: f32,
}

impl ChuteSettings {
    /// Clamped and stepped as the original's `SteppedRange`s.
    pub fn stepped(self) -> ChuteSettings {
        ChuteSettings {
            mode: self.mode,
            min_pressure_atm: ((self.min_pressure_atm.clamp(0.01, 0.75) * 100.0).round() / 100.0),
            deploy_altitude_m: (self.deploy_altitude_m.clamp(50.0, 5000.0) / 50.0).round() * 50.0,
        }
    }
}

/// The part actions a group can run: on with the group on, off with it off.
/// `engine`: the engine runs or is shut down; `deploy`: panels, antennas, gear and bays extend or retract;
/// `chute`: parachutes arm (on only); `converter`: converters run or stop; `fairing`: the fairing comes
/// off (on only); `mode`: a multi-mode engine switches to its second mode (on) or back to its first (off);
/// `crossfeed`: a decoupler's, separator's or docking port's crossfeed switches on or off.
pub const ACTIONS: [&str; 7] = [
    "engine",
    "deploy",
    "chute",
    "converter",
    "fairing",
    "mode",
    "crossfeed",
];

/// The `KSPActionGroup` bit of custom group `n` (1..10).
pub fn custom_group_bit(n: u8) -> u32 {
    0x10000u32 << (n.clamp(1, 10) - 1)
}

/// Whether fuel flows out of `part` to the parts attached to it. The original keeps one `FuelCrossfeed` per
/// part, starting from the definition's `fuelCrossFeed`, and its crossfeed toggle
/// (`Module_ToggleCrossfeed`) sets it: on a part with the toggle (decouplers, separators, docking ports) the
/// craft's value decides; elsewhere both the craft and the definition must allow it.
pub fn crossfeed_on(part: &CraftPart, def: Option<&PartDef>) -> bool {
    match def {
        Some(d) if d.crossfeed_toggle() => part.fuel_crossfeed,
        Some(d) => part.fuel_crossfeed && d.fuel_cross_feed,
        None => part.fuel_crossfeed,
    }
}

impl Craft {
    /// Validate linked struts against the shipped part's extent and anchor relationship.
    pub fn strut_error(&self, defs: &[PartDef]) -> Option<String> {
        for (i, p) in self.parts.iter().enumerate() {
            if p.fuel_line_target.is_none()
                || !defs.iter().any(|d| d.id == p.part_id && d.is_strut())
            {
                continue;
            }
            let Some((source, target)) = self.compound_ends(i) else {
                return Some(format!("strut {i} has a missing anchor parent"));
            };
            if source == target || source >= self.parts.len() || target >= self.parts.len() {
                return Some(format!("strut {i} must join different parent parts"));
            }
            let far = &self.parts[p.fuel_line_target.unwrap()];
            let length = (far.local_position - p.local_position).length();
            if !length.is_finite() || !(STRUT_MIN_LENGTH_M..=STRUT_MAX_LENGTH_M).contains(&length) {
                return Some(format!("strut {i} must be between 0.01 and 10 metres long"));
            }
        }
        None
    }

    /// The two parts a fuel line or strut joins, when part `i` is its linked first anchor: the part it hangs
    /// on and the part its far anchor hangs on.
    pub fn compound_ends(&self, i: usize) -> Option<(usize, usize)> {
        let p = self.parts.get(i)?;
        let far = self.parts.get(p.fuel_line_target?)?;
        Some((p.parent?, far.parent?))
    }
}

impl CraftPart {
    /// Derive a procedural wing's mass, lift and control surface from its shape (`ose_data::procedural`),
    /// replacing whatever `flight` held. Run wherever a craft is accepted (the server on launch, the editor on
    /// every change) so a client cannot claim its own numbers. Other parts are left alone.
    pub fn apply_shape(&mut self, def: &PartDef) {
        // A shaped fairing weighs its base and its panels (`Data_Fairing.MassModifierAmount`).
        match (crate::fairing::def(&def.id), self.fairing.take()) {
            (Some(fd), Some(f)) if f.valid_for(&def.id) => {
                let flight =
                    self.flight
                        .get_or_insert_with(|| crate::flight_geometry::PartFlight {
                            part_id: def.id.clone(),
                            ..Default::default()
                        });
                flight.part_id = def.id.clone();
                flight.dry_mass_t = Some(def.mass_t + f.panel_mass_t(fd));
                self.fairing = Some(f);
            }
            _ => {}
        }
        if !crate::procedural::is_procedural_wing(&def.id) {
            self.shape = None;
            return;
        }
        let rig = crate::procedural::rig(&def.id).expect("procedural rig");
        let shape = self
            .shape
            .take()
            .filter(|s| s.valid_for(&def.id))
            .unwrap_or_else(|| crate::procedural::WingShape {
                option: rig.options.len().saturating_sub(1) as u32,
                ..Default::default()
            });
        let mirrored = self.flight.as_ref().is_some_and(|f| f.mirrored);
        if let Some(mut f) = crate::procedural::flight(&def.id, &shape, mirrored) {
            if let (Some(s), Some(authored)) = (&mut f.surface, &def.flight_geometry.surface) {
                s.range_deg = def.ctrl_range_deg;
                s.speed = authored.speed;
            }
            self.flight = Some(f);
        }
        self.shape = Some(shape);
    }

    pub fn dry_mass(&self, def: &PartDef) -> f64 {
        self.flight
            .as_ref()
            .filter(|f| f.part_id == def.id && f.valid())
            .and_then(|f| f.dry_mass_t)
            .unwrap_or(def.mass_t)
    }
    pub fn flight_definition(&self, def: &PartDef) -> PartDef {
        let mut result = def.clone();
        if let Some(f) = self
            .flight
            .as_ref()
            .filter(|f| f.part_id == def.id && f.valid())
        {
            result.mass_t = self.dry_mass(def);
            if let Some(com) = f.mass_offset_m {
                result.mass_offset_m = com;
                result.flight_geometry.mass_offset_m = com;
            }
            if let Some(t) = f.inertia_per_t {
                result.flight_geometry.inertia_per_t = Some(t);
                result.flight_geometry.inertia_source = "runtime_shape".into();
            }
            if let (Some(actual), Some(surface)) = (&f.surface, &mut result.flight_geometry.surface)
            {
                surface.fixed_lift_coeff = Some(actual.fixed_lift_coeff);
                surface.moving_lift_coeff = Some(actual.moving_lift_coeff);
                surface.pivot_m = actual.pivot_m;
                surface.speed = actual.speed;
                result.ctrl_pressure_offset_m = actual.pressure_offset_m;
                result.ctrl_range_deg = actual.range_deg;
            }
        }
        result
    }
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
    /// `CraftPart::apply_shape` on every part whose definition is known.
    pub fn apply_shapes(&mut self, parts: &[PartDef]) {
        for p in &mut self.parts {
            if let Some(def) = parts.iter().find(|d| d.id == p.part_id) {
                p.apply_shape(def);
            }
        }
    }

    /// Give every part its own guid: a repeat keeps the first part's and the later ones get new `p<n>` past the
    /// highest in use. Parts are matched by guid between a craft and its flight (what is still attached, which
    /// stage holds what), and an editor that once numbered new parts by count left repeats after deletions.
    /// Returns whether anything changed.
    pub fn dedupe_guids(&mut self) -> bool {
        let mut next = self
            .parts
            .iter()
            .filter_map(|p| {
                p.guid
                    .strip_prefix('p')
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let mut seen = std::collections::HashSet::new();
        let mut changed = false;
        for p in &mut self.parts {
            if !seen.insert(p.guid.clone()) {
                p.guid = format!("p{next}");
                next += 1;
                seen.insert(p.guid.clone());
                changed = true;
            }
        }
        changed
    }

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
                Some(def) => dry += p.dry_mass(def),
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

#[cfg(test)]
mod guid_tests {
    use super::Craft;

    #[test]
    fn repeated_guids_get_new_ones_past_the_highest() {
        let crafts: Vec<Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let mut c = crafts.into_iter().find(|c| c.parts.len() >= 5).unwrap();
        for (k, g) in ["p1", "p3", "p3", "p2", "p2"].into_iter().enumerate() {
            c.parts[k].guid = g.into();
        }
        for (k, p) in c.parts.iter_mut().enumerate().skip(5) {
            p.guid = format!("x{k}");
        }
        assert!(c.dedupe_guids());
        let g: Vec<&str> = c.parts[..5].iter().map(|p| p.guid.as_str()).collect();
        assert_eq!(
            g,
            ["p1", "p3", "p4", "p2", "p5"],
            "first keeps its guid, repeats go past p3"
        );
        assert!(!c.dedupe_guids(), "already unique");
    }
}

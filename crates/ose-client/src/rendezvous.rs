//! Pure advisory state and docking geometry. Uses the same authored node, GUID and reflected craft
//! frame as the simulation, never a vessel-centre approximation for final alignment.

use crate::vab::Catalog;
use ose_data::craft::Craft;
use ose_math::{rendezvous::Transfer, DVec3};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    Off,
    Minimal,
    Guided,
}
impl Level {
    pub fn parse(s: &str) -> Self {
        match s {
            "minimal" => Self::Minimal,
            "guided" => Self::Guided,
            _ => Self::Off,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Guided => "guided",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Prepare,
    Launch,
    Intercept,
    Coast,
    Approach,
    Dock,
    Unavailable,
    Complete,
}
impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Prepare => "Prepare",
            Self::Launch => "Launch plane",
            Self::Intercept => "Intercept burn",
            Self::Coast => "Coast to encounter",
            Self::Approach => "Match velocity",
            Self::Dock => "Port alignment",
            Self::Unavailable => "Guidance unavailable",
            Self::Complete => "Module attached",
        }
    }
}

pub struct Director {
    pub level: Level,
    pub destination: Option<u64>,
    pub own_port: Option<String>,
    pub target_port: Option<String>,
    pub enabled: bool,
    pub phase: Phase,
    pub transfer: Option<Transfer>,
    pub correction_arrival: Option<f64>,
    pub remaining: DVec3,
    pub hold: bool,
    pub hold_sent_ms: f64,
    pub burn_sample: Option<(f64, DVec3, DVec3, bool)>,
    pub acceleration: f64,
    pub snapshot_ms: f64,
    pub last_render_ms: f64,
    pub cue: Option<DVec3>,
    pub point: Option<DVec3>,
    pub axis_point: Option<DVec3>,
    pub note: String,
    pub world_cues: bool,
    pub audible: bool,
    pub last_prompt: Option<i64>,
    pub signature: Option<(u64, u16, u32, u32)>,
    pub dock_latched: bool,
    pub launch_destination: Option<u64>,
    pub launch_enabled: bool,
}
impl Default for Director {
    fn default() -> Self {
        Self {
            level: Level::Off,
            destination: None,
            own_port: None,
            target_port: None,
            enabled: false,
            phase: Phase::Prepare,
            transfer: None,
            correction_arrival: None,
            remaining: DVec3::ZERO,
            hold: false,
            hold_sent_ms: 0.0,
            burn_sample: None,
            acceleration: 0.0,
            snapshot_ms: 0.0,
            last_render_ms: 0.0,
            cue: None,
            point: None,
            axis_point: None,
            note: String::new(),
            world_cues: true,
            audible: false,
            last_prompt: None,
            signature: None,
            dock_latched: false,
            launch_destination: None,
            launch_enabled: false,
        }
    }
}
impl Director {
    /// The visible aiming cue, shared by the navball and both attitude-hold entry points.
    pub fn active_cue(&self) -> Option<DVec3> {
        if self.enabled
            && self.level != Level::Off
            && matches!(self.phase, Phase::Intercept | Phase::Approach | Phase::Dock)
        {
            self.cue
        } else {
            None
        }
    }

    pub fn invalidate(&mut self) {
        self.transfer = None;
        self.correction_arrival = None;
        self.cue = None;
        self.point = None;
        self.axis_point = None;
        self.remaining = DVec3::ZERO;
        self.burn_sample = None;
        self.signature = None;
        self.dock_latched = false;
    }
}

#[derive(Clone, Debug)]
pub struct Port {
    pub part: usize,
    pub guid: String,
    pub name: String,
    pub position: DVec3,
    pub axis: DVec3,
    pub free: bool,
    pub def: ose_data::DockingPort,
}

pub fn rotate(q: ose_data::craft::Quat, p: DVec3) -> DVec3 {
    let q = q.normalized();
    let u = DVec3::new(q.x, q.y, q.z);
    let t = u.cross(p) * 2.0;
    p + t * q.w + u.cross(t)
}

pub fn ports(craft: &Craft, catalog: &Catalog) -> Vec<Port> {
    craft
        .parts
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let spec = catalog.get(&p.part_id)?;
            let def = spec.docking.as_ref()?;
            let node = spec.attach_nodes.iter().find(|n| n.id == def.node)?;
            let occupied = p.own_node.as_ref() == Some(&def.node)
                || craft
                    .parts
                    .iter()
                    .any(|c| c.parent == Some(i) && c.parent_node.as_ref() == Some(&def.node));
            Some(Port {
                part: i,
                guid: p.guid.clone(),
                name: format!("{} · port {}", spec.display_title(), i + 1),
                position: p.local_position + rotate(p.local_rotation, node.position),
                axis: rotate(p.local_rotation, node.orientation).normalized(),
                free: !occupied,
                def: def.clone(),
            })
        })
        .collect()
}

pub fn compatible(a: &Port, b: &Port) -> bool {
    a.def.sizes.iter().any(|s| b.def.sizes.contains(s))
}

/// The stock HUD's delta-v list excludes stages already fired. Their current engines can still burn.
/// Estimate the active stage from its streamed reachable propellant and the remaining craft's lowest
/// vacuum Isp, keeping a ten-percent reserve instead of advertising fuel that cannot be used to brake.
pub fn active_budget(mass: f64, propellant: f64, craft: &Craft, catalog: &Catalog) -> f64 {
    let isp = craft
        .parts
        .iter()
        .filter_map(|p| catalog.get(&p.part_id))
        .flat_map(|s| s.engines.iter())
        .map(|e| e.isp_vacuum_s)
        .filter(|i| i.is_finite() && *i > 0.0)
        .fold(f64::INFINITY, f64::min);
    if !isp.is_finite()
        || !mass.is_finite()
        || !propellant.is_finite()
        || mass <= 0.0
        || propellant <= 0.0
        || propellant >= mass
    {
        return 0.0;
    }
    0.9 * 9.80665 * isp * (mass / (mass - propellant)).ln()
}

pub fn active_thrust(craft: &Craft, catalog: &Catalog, stage: u32, stages: u32) -> f64 {
    let first_active = stages.saturating_sub(stage + 1) as usize;
    let active: std::collections::HashSet<_> = craft
        .stages
        .iter()
        .skip(first_active)
        .flatten()
        .copied()
        .collect();
    craft
        .parts
        .iter()
        .enumerate()
        .filter(|(i, _)| active.contains(i))
        .map(|(_, p)| p)
        .filter_map(|p| catalog.get(&p.part_id))
        .filter_map(|s| s.engines.first())
        .map(|e| e.max_thrust_kn)
        .sum()
}

/// Physics frame mirrors Unity Z. `centre` is already in that reflected frame.
pub fn world_offset(local: DVec3, centre: DVec3, facing: DVec3, top: DVec3) -> DVec3 {
    let y = facing.normalized();
    let z = (top - y * top.dot(y)).normalized();
    let x = y.cross(z);
    let p = DVec3::new(local.x, local.y, -local.z) - centre;
    x * p.x + y * p.y + z * p.z
}
pub fn world_axis(local: DVec3, facing: DVec3, top: DVec3) -> DVec3 {
    world_offset(local, DVec3::ZERO, facing, top).normalized()
}
pub fn radius(craft: &Craft, catalog: &Catalog, centre: DVec3) -> f64 {
    craft
        .parts
        .iter()
        .map(|p| {
            let spec = catalog.get(&p.part_id);
            let local = p.local_position
                + spec
                    .map(|s| rotate(p.local_rotation, s.centre()))
                    .unwrap_or(DVec3::ZERO);
            let reflected = DVec3::new(local.x, local.y, -local.z);
            (reflected - centre).length() + spec.map(|s| s.size().length() * 0.5).unwrap_or(1.0)
        })
        .fold(1.0, f64::max)
}

/// Staging point is beyond the conservative station/delivery bounds along the selected port axis.
pub fn stand_off(
    target_radius: f64,
    own_radius: f64,
    target_port_offset: DVec3,
    axis: DVec3,
) -> f64 {
    (target_radius - target_port_offset.dot(axis) + own_radius + 20.0).max(own_radius + 15.0)
}

/// Centre-line obstruction check in the authored craft frame. It does not certify the delivery's
/// swept hull; the UI retains an explicit visual-clearance instruction.
pub fn obstructed(craft: &Craft, catalog: &Catalog, port: &Port, length: f64) -> bool {
    let start = port.position + port.axis * 2.0;
    let end = port.position + port.axis * length;
    craft
        .parts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != port.part)
        .any(|(_, p)| {
            let Some(spec) = catalog.get(&p.part_id) else {
                return true;
            };
            let q = p.local_rotation.normalized();
            let inverse = ose_data::craft::Quat {
                x: -q.x,
                y: -q.y,
                z: -q.z,
                w: q.w,
            };
            let a = rotate(inverse, start - p.local_position) - spec.centre();
            let b = rotate(inverse, end - p.local_position) - spec.centre();
            let h = spec.size() * 0.5;
            let d = b - a;
            let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
            for (pos, delta, half) in [(a.x, d.x, h.x), (a.y, d.y, h.y), (a.z, d.z, h.z)] {
                if delta.abs() < 1e-9 {
                    if pos.abs() > half {
                        return false;
                    }
                } else {
                    let x = (-half - pos) / delta;
                    let y = (half - pos) / delta;
                    lo = lo.max(x.min(y));
                    hi = hi.min(x.max(y));
                    if lo > hi {
                        return false;
                    }
                }
            }
            hi >= 0.0 && lo <= 1.0
        })
}

/// Rotate the current control axis by the port-to-port alignment error. Works for a side-mounted
/// own port and preserves the existing control point; does not silently reroot a craft.
pub fn alignment_hold(
    control_forward: DVec3,
    own_port_axis: DVec3,
    target_port_axis: DVec3,
) -> DVec3 {
    let g = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
    let turn = glam::DQuat::from_rotation_arc(
        g(own_port_axis).normalize(),
        -g(target_port_axis).normalize(),
    );
    let v = turn * g(control_forward);
    DVec3::new(v.x, v.y, v.z).normalized()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_active_aiming_phases_expose_a_hold_cue() {
        let cue = DVec3::new(1.0, 0.0, 0.0);
        let mut director = Director {
            enabled: true,
            level: Level::Guided,
            cue: Some(cue),
            ..Default::default()
        };
        for phase in [Phase::Intercept, Phase::Approach, Phase::Dock] {
            director.phase = phase;
            assert_eq!(director.active_cue(), Some(cue));
        }
        for phase in [
            Phase::Prepare,
            Phase::Launch,
            Phase::Coast,
            Phase::Unavailable,
            Phase::Complete,
        ] {
            director.phase = phase;
            assert_eq!(director.active_cue(), None, "stale cue in {phase:?}");
        }
        director.phase = Phase::Intercept;
        director.level = Level::Off;
        assert_eq!(director.active_cue(), None);
        director.level = Level::Minimal;
        assert_eq!(director.active_cue(), Some(cue));
        director.enabled = false;
        assert_eq!(director.active_cue(), None);
    }
    #[test]
    fn reflected_port_pose_and_rotating_port_velocity_follow_physics() {
        let x = DVec3::new(1.0, 0.0, 0.0);
        let y = DVec3::new(0.0, 1.0, 0.0);
        let z = DVec3::new(0.0, 0.0, 1.0);
        assert_eq!(world_axis(z, y, z), -z);
        assert_eq!(
            world_offset(DVec3::new(3.0, 4.0, 5.0), DVec3::new(1.0, 2.0, -1.0), y, z),
            DVec3::new(2.0, 2.0, -4.0)
        );
        let hold = alignment_hold(y, x, z);
        assert!(hold.length() > 0.99);
        let q = glam::DQuat::from_rotation_arc(glam::DVec3::X, glam::DVec3::NEG_Z);
        let expected = q * glam::DVec3::Y;
        assert!((hold - DVec3::new(expected.x, expected.y, expected.z)).length() < 1e-9);
    }
    #[test]
    fn staging_point_is_outside_large_station_bounds() {
        let axis = DVec3::new(1.0, 0.0, 0.0);
        let port = axis * 8.0;
        let offset = stand_off(60.0, 12.0, port, axis);
        assert!(port.x + offset > 60.0 + 12.0);
    }
}

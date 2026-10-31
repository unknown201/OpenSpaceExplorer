//! Docking: finding docking ports that meet, and the craft surgery that joins two vessels into one and takes
//! them apart again (remaining limits: P29 in `docs/PHYSICS-GAPS.md`).
//!
//! A vessel keeps its craft ([`CraftDoc`]) so it can be merged and split. The docking step follows the
//! original's `Module_DockingNode`: ports within their acquire range, facing each other within the acquire
//! angle, latch once they drift slower than the capture speed (0.3 m/s for every stock port); faster than that
//! each pulls toward the other with half its acquire force and turns toward facing it with half its acquire
//! torque. Ports facing each other meet at their faces instead of as hull spheres; misaligned ports strike as
//! hulls do. The original's capture range and capture angle are defined but unused there, so here too. The
//! docked vessel's parts are re-rooted under the port they docked to, turned so the ports face each other, in
//! the first vessel's craft frame.

use bevy_ecs::prelude::*;
use ose_data::craft::{Craft, CraftPart, Quat};
use ose_data::PartDef;
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

use crate::{bodies, flight, structure, Universe};

/// The craft a vessel was built from, kept current (parts dropped, tanks as they are) when it is merged,
/// split or saved.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CraftDoc(pub Craft);

/// Docked joints in a vessel's craft: (the port that was docked to, the port of the vessel that docked).
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Docked {
    pub links: Vec<(usize, usize)>,
}

/// After undocking, two ports do not latch again until they are their `min_distance_to_reengage_m` apart
/// (the original's `Disengaged` state).
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct Undocked {
    pub from: Vec<Disengaged>,
}

/// One port pair kept from latching: the other vessel, this vessel's port part and the other's (indices into
/// each one's `current_craft`), and how far apart the ports must get first, m.
#[derive(Clone, Debug, PartialEq)]
pub struct Disengaged {
    pub other: Entity,
    pub port: usize,
    pub other_port: usize,
    pub min_distance_m: f64,
}

/// Port pairs that face each other closely enough to dock, as (vessel, structure part) pairs: their parts do
/// not strike as hulls (`collide_vessels`); the docking step handles their faces meeting.
#[derive(Resource, Clone, Debug, Default)]
pub struct AlignedPorts(pub Vec<(Entity, usize, Entity, usize)>);

impl AlignedPorts {
    pub fn contains(&self, a: Entity, part_a: usize, b: Entity, part_b: usize) -> bool {
        self.0.iter().any(|&(x, i, y, j)| {
            (x == a && i == part_a && y == b && j == part_b)
                || (x == b && i == part_b && y == a && j == part_a)
        })
    }
}

/// The tick the docking step last ran at, to know how long its acquire forces act.
#[derive(Resource, Clone, Copy, Debug)]
struct LastDockTick(u64);

/// How far apart two vessels' port faces can be for the docking step to look at them, m.
const PORT_LOOK_M: f64 = 10.0;

/// A free docking port of a craft, in the craft frame (Unity Y-up).
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub part: usize,
    pub def: ose_data::DockingPort,
    pub position: DVec3,
    /// Outward axis of the port (unit).
    pub axis: DVec3,
}

fn q_rotate(q: Quat, v: DVec3) -> DVec3 {
    let u = DVec3::new(q.x, q.y, q.z);
    let t = u.cross(v) * 2.0;
    v + t * q.w + u.cross(t)
}

fn q_mul(a: Quat, b: Quat) -> Quat {
    Quat {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}

fn unit(q: Quat) -> Quat {
    let n = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if n < 1e-12 {
        Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    } else {
        Quat {
            x: q.x / n,
            y: q.y / n,
            z: q.z / n,
            w: q.w / n,
        }
    }
}

/// The shortest rotation taking unit `a` onto unit `b`.
fn rotation_between(a: DVec3, b: DVec3) -> Quat {
    let c = a.cross(b);
    let d = a.dot(b);
    if d < -0.999_999 {
        // opposite: half a turn about any perpendicular
        let p = if a.x.abs() < 0.9 {
            a.cross(DVec3::new(1.0, 0.0, 0.0))
        } else {
            a.cross(DVec3::new(0.0, 1.0, 0.0))
        }
        .normalized();
        return Quat {
            x: p.x,
            y: p.y,
            z: p.z,
            w: 0.0,
        };
    }
    unit(Quat {
        x: c.x,
        y: c.y,
        z: c.z,
        w: 1.0 + d,
    })
}

/// Whether an attach node of a part is taken by a connection in the craft.
fn node_used(craft: &Craft, part: usize, node: &str) -> bool {
    craft.parts.iter().enumerate().any(|(i, p)| {
        (i == part && p.own_node.as_deref() == Some(node))
            || (p.parent == Some(part) && p.parent_node.as_deref() == Some(node))
    })
}

/// The craft's docking ports whose docking node is free.
pub fn free_ports(craft: &Craft, defs: &[PartDef]) -> Vec<Port> {
    let mut out = Vec::new();
    for (i, p) in craft.parts.iter().enumerate() {
        let Some(def) = defs.iter().find(|d| d.id == p.part_id) else {
            continue;
        };
        let Some(dock) = &def.docking else { continue };
        let Some(node) = def.attach_nodes.iter().find(|n| n.id == dock.node) else {
            continue;
        };
        if node_used(craft, i, &dock.node) {
            continue;
        }
        let rot = unit(p.local_rotation);
        let axis = if node.orientation.length_squared() > 1e-9 {
            node.orientation.normalized()
        } else {
            node.position.normalized()
        };
        out.push(Port {
            part: i,
            def: dock.clone(),
            position: p.local_position + q_rotate(rot, node.position),
            axis: q_rotate(rot, axis).normalized(),
        });
    }
    out
}

/// Parts re-rooted at `new_root`: the links on its path to the old root are reversed.
fn reroot(parts: &mut [CraftPart], new_root: usize) {
    let mut path = vec![new_root];
    while let Some(p) = parts[*path.last().unwrap()].parent {
        path.push(p);
    }
    // walk from the old root down, turning each link around
    for w in path.windows(2).rev() {
        let (child, parent) = (w[0], w[1]);
        let own = parts[child].own_node.take();
        let theirs = parts[child].parent_node.take();
        parts[parent].parent = Some(child);
        parts[parent].own_node = theirs;
        parts[parent].parent_node = own;
    }
    parts[new_root].parent = None;
}

/// Join craft `b` to craft `a` at ports `pa` (of `a`) and `pb` (of `b`): `b` is turned so its port faces
/// `a`'s, moved so the ports meet, re-rooted at its port and attached under `a`'s. Returns the joined craft;
/// `b`'s part `i` becomes `a.parts.len() + i`.
pub fn merge(a: &Craft, pa: &Port, b: &Craft, pb: &Port) -> Craft {
    let off = a.parts.len();
    let turn = rotation_between(pb.axis, pa.axis * -1.0);
    let mut parts_b: Vec<CraftPart> = b.parts.clone();
    for p in &mut parts_b {
        p.local_position = pa.position + q_rotate(turn, p.local_position - pb.position);
        p.local_rotation = unit(q_mul(turn, p.local_rotation));
    }
    reroot(&mut parts_b, pb.part);
    for p in &mut parts_b {
        p.parent = p.parent.map(|x| x + off);
        p.fuel_line_target = p.fuel_line_target.map(|x| x + off);
        // the docked craft keeps its own paint
        if p.colors.is_none() {
            p.colors = b.colors;
        }
    }
    parts_b[pb.part].parent = Some(pa.part);
    parts_b[pb.part].own_node = Some(pb.def.node.clone());
    parts_b[pb.part].parent_node = Some(pa.def.node.clone());
    let mut parts = a.parts.clone();
    parts.extend(parts_b);
    let mut stages = a.stages.clone();
    stages.extend(
        b.stages
            .iter()
            .map(|s| s.iter().map(|i| i + off).collect::<Vec<usize>>()),
    );
    Craft {
        colors: a.colors,
        loose: Vec::new(),
        name: a.name.clone(),
        description: a.description.clone(),
        parts,
        stages,
    }
}

/// Keep only the parts in `keep` (each keeps its parent if that is kept too), with stages re-indexed and
/// emptied stages dropped. Returns the new craft and the old-to-new index map.
pub fn subset(craft: &Craft, keep: &[usize]) -> (Craft, Vec<Option<usize>>) {
    let mut map = vec![None; craft.parts.len()];
    for (n, &i) in keep.iter().enumerate() {
        map[i] = Some(n);
    }
    let parts = keep
        .iter()
        .map(|&i| {
            let mut p = craft.parts[i].clone();
            p.parent = p.parent.and_then(|x| map[x]);
            p.fuel_line_target = p.fuel_line_target.and_then(|x| map[x]);
            if p.parent.is_none() {
                p.own_node = None;
                p.parent_node = None;
            }
            p
        })
        .collect();
    let stages = craft
        .stages
        .iter()
        .map(|s| s.iter().filter_map(|&i| map[i]).collect::<Vec<usize>>())
        .filter(|s| !s.is_empty())
        .collect();
    (
        Craft {
            colors: craft.colors,
            loose: Vec::new(),
            name: craft.name.clone(),
            description: craft.description.clone(),
            parts,
            stages,
        },
        map,
    )
}

/// Split a craft at the link above `part`: the subtree under (and including) `part` comes off, re-rooted at
/// `part`. Returns (what stays, what comes off) with each one's old-to-new index map.
#[allow(clippy::type_complexity)]
pub fn split(
    craft: &Craft,
    part: usize,
) -> ((Craft, Vec<Option<usize>>), (Craft, Vec<Option<usize>>)) {
    let n = craft.parts.len();
    // a part comes off when `part` is on its way up to the root
    let off: Vec<bool> = (0..n)
        .map(|i| {
            let mut j = Some(i);
            while let Some(k) = j {
                if k == part {
                    return true;
                }
                j = craft.parts[k].parent;
            }
            false
        })
        .collect();
    let stay: Vec<usize> = (0..n).filter(|&i| !off[i]).collect();
    let mut go: Vec<usize> = vec![part];
    go.extend((0..n).filter(|&i| off[i] && i != part));
    (subset(craft, &stay), subset(craft, &go))
}

/// A vessel's frame: the structure's centre of mass at rest and the axes it flies with.
fn vessel_axes(facing: DVec3) -> (DVec3, DVec3, DVec3) {
    let y = facing.normalized();
    let reference = if y.z.abs() < 0.9 {
        DVec3::new(0.0, 0.0, 1.0)
    } else {
        DVec3::new(1.0, 0.0, 0.0)
    };
    let x = y.cross(reference).normalized();
    let z = x.cross(y);
    (x, y, z)
}

/// Where a craft-frame point of vessel `e` is now, body-centred inertial: in the structure's frame, roll
/// included, as its parts strike and are drawn.
fn to_inertial(u: &Universe, e: Entity, craft_point: DVec3, is_dir: bool) -> Option<DVec3> {
    let r = u.position_of(e)?;
    let facing = u
        .world
        .get::<flight::Powered>(e)
        .map(|p| p.facing)
        .or_else(|| u.world.get::<flight::Dormant>(e).map(|d| d.0.facing))
        .unwrap_or(DVec3::new(0.0, 0.0, 1.0));
    let vf = |p: DVec3| DVec3::new(p.x, p.y, -p.z);
    if let Some(st) = u.world.get::<structure::Structure>(e) {
        return Some(if is_dir {
            st.axis_inertial(facing, craft_point)
        } else {
            r + st.offset_inertial(facing, vf(craft_point) - st.rigid.rest_centre())
        });
    }
    let (bx, by, bz) = vessel_axes(facing);
    let local = vf(craft_point);
    let w = bx * local.x + by * local.y + bz * local.z;
    Some(if is_dir { w } else { r + w })
}

/// Two vessels' ports near each other: (vessel a, port a, vessel b, port b), each port's face and outward
/// axis now (inertial), how far apart the faces are and how fast the vessels drift relative to each other.
#[derive(Clone, Debug, PartialEq)]
pub struct Latch {
    pub a: Entity,
    pub port_a: Port,
    pub b: Entity,
    pub port_b: Port,
    pub distance_m: f64,
    pub face_a: DVec3,
    pub face_b: DVec3,
    pub axis_a: DVec3,
    pub axis_b: DVec3,
    pub rel_speed_mps: f64,
}

impl Latch {
    /// How squarely the ports face each other (1 = head on).
    pub fn facing(&self) -> f64 {
        -self.axis_a.dot(self.axis_b)
    }

    /// Within either port's acquire range and facing within its acquire angle: the original pulls these
    /// together (`ValidateDockingConfiguration`, the larger port's figures).
    pub fn acquiring(&self) -> bool {
        let range = self
            .port_a
            .def
            .acquire_range_m
            .max(self.port_b.def.acquire_range_m);
        self.distance_m < range && self.aligned()
    }

    /// Facing within the acquire angle at any distance.
    pub fn aligned(&self) -> bool {
        let min_dot = self
            .port_a
            .def
            .acquire_min_fwd_dot
            .max(self.port_b.def.acquire_min_fwd_dot);
        self.facing() > min_dot
    }

    /// Acquiring and drifting slower than the capture speed: the ports latch (`DockToModule`).
    pub fn captures(&self) -> bool {
        let max_rvel = self
            .port_a
            .def
            .capture_max_rvel_mps
            .min(self.port_b.def.capture_max_rvel_mps);
        self.acquiring() && self.rel_speed_mps < max_rvel
    }
}

/// A vessel's structure frame at one moment (`Structure::offset_inertial`), kept across a rebuild.
struct Frame {
    axes: [DVec3; 3],
    centre: DVec3,
    top: DVec3,
}

impl Frame {
    /// A structure-frame offset, inertial.
    fn offset(&self, v: DVec3) -> DVec3 {
        self.axes[0] * v.x + self.axes[1] * v.y + self.axes[2] * v.z
    }
}

/// A vessel's change of motion from an impulse `j` (kN·s) at `at` (offset from its centre, m) and an angular
/// impulse `turn` (kN·m·s), for its mass and moment of inertia `moi` (t·m²); axial spin goes to `roll_rate`.
fn push(p: &mut flight::Powered, j: DVec3, at: DVec3, turn: DVec3, moi: f64) {
    if p.landed {
        return;
    }
    p.v = p.v + j * (1.0 / p.mass_t.max(0.001));
    let dw = (at.cross(j) + turn) * (1.0 / moi.max(1e-6));
    let axial = dw.dot(p.facing);
    p.roll_rate += axial;
    p.angvel = p.angvel + (dw - p.facing * axial);
}

/// A vessel's parts as a save keeps them: its craft as launched (tanks holding what they hold now), which
/// parts are still on, and what each part is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedParts {
    pub craft: Craft,
    pub present: Vec<bool>,
    pub deployed: Vec<bool>,
    pub chutes: Vec<structure::Chute>,
    pub chute_open: Vec<f64>,
    pub temperature_k: Vec<f64>,
    pub gear_out: Vec<bool>,
    pub converter_on: Vec<bool>,
    pub fairing_deployed: Vec<bool>,
    pub bay_open: Vec<bool>,
    #[serde(default)]
    pub engine_shutdown: Vec<bool>,
    #[serde(default)]
    pub control_part: Option<String>,
    #[serde(default)]
    pub debris: bool,
}

/// Counts changes to a vessel's craft (`VesselState::craft_rev`).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct CraftRev(pub u32);

/// The part a vessel is controlled from (its guid), when not the root.
#[derive(Component, Clone, Debug)]
pub struct ControlPart(pub String);

impl Universe {
    pub fn set_craft(&mut self, e: Entity, mut craft: Craft) {
        // a vessel flies its craft, not the editor's loose assemblies
        craft.loose.clear();
        let rev = self.world.get::<CraftRev>(e).map(|r| r.0).unwrap_or(0);
        self.world
            .entity_mut(e)
            .insert(CraftDoc(craft))
            .insert(CraftRev(rev.wrapping_add(1)));
    }

    /// Changes whenever the vessel's craft does: parts lost or gained, staging edited, control moved.
    pub fn craft_rev(&self, e: Entity) -> u32 {
        let lost = self
            .world
            .get::<structure::Structure>(e)
            .map(|st| st.rigid.present.iter().filter(|x| !**x).count() as u32)
            .unwrap_or(0);
        self.world
            .get::<CraftRev>(e)
            .map(|r| r.0)
            .unwrap_or(0)
            .wrapping_mul(1024)
            .wrapping_add(lost)
    }

    /// What a save keeps of a vessel's parts (`None` without a craft).
    pub fn saved_parts(&self, e: Entity) -> Option<SavedParts> {
        let mut craft = self.world.get::<CraftDoc>(e)?.0.clone();
        let debris = self.world.get::<crate::vessels::Debris>(e).is_some();
        let control_part = self.world.get::<ControlPart>(e).map(|c| c.0.clone());
        let Some(st) = self.world.get::<structure::Structure>(e) else {
            return Some(SavedParts {
                present: vec![true; craft.parts.len()],
                craft,
                control_part,
                debris,
                ..Default::default()
            });
        };
        for t in &st.net.tanks {
            let name = &st.net.resources[t.resource];
            if let Some(r) = craft.parts[t.part]
                .resources
                .iter_mut()
                .find(|r| r.resource.eq_ignore_ascii_case(name))
            {
                r.stored_units = t.stored;
            }
        }
        Some(SavedParts {
            craft,
            present: st.rigid.present.clone(),
            deployed: st.deployed.clone(),
            chutes: st.chutes.clone(),
            chute_open: st.chute_open.clone(),
            temperature_k: st.temperature_k.clone(),
            gear_out: st.gear_out.clone(),
            converter_on: st.converter_on.clone(),
            fairing_deployed: st.fairing_deployed.clone(),
            bay_open: st.bay_open.clone(),
            engine_shutdown: st.engine_shutdown.clone(),
            control_part,
            debris,
        })
    }

    /// Give a restored vessel its parts back as `saved_parts` kept them (its flight state set first: the
    /// structure follows its stage).
    pub fn restore_parts(
        &mut self,
        e: Entity,
        saved: &SavedParts,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
    ) {
        self.set_craft(e, saved.craft.clone());
        let mut st = structure::Structure::build(&saved.craft, defs, resources);
        let n = st.rigid.len();
        let gone: Vec<usize> = (0..n)
            .filter(|&i| !saved.present.get(i).copied().unwrap_or(true))
            .collect();
        if !gone.is_empty() {
            st.rigid.remove_parts(&gone);
            st.net.remove_parts(&gone);
        }
        let copy = |dst: &mut Vec<bool>, src: &[bool]| {
            if src.len() == dst.len() {
                dst.copy_from_slice(src);
            }
        };
        copy(&mut st.deployed, &saved.deployed);
        copy(&mut st.gear_out, &saved.gear_out);
        copy(&mut st.converter_on, &saved.converter_on);
        copy(&mut st.fairing_deployed, &saved.fairing_deployed);
        copy(&mut st.bay_open, &saved.bay_open);
        copy(&mut st.engine_shutdown, &saved.engine_shutdown);
        if saved.chutes.len() == n {
            st.chutes.clone_from(&saved.chutes);
        }
        if saved.chute_open.len() == n {
            st.chute_open.clone_from(&saved.chute_open);
        }
        if saved.temperature_k.len() == n {
            st.temperature_k.clone_from(&saved.temperature_k);
        }
        st.update_shielding();
        self.set_structure(e, st);
        if let Some(g) = &saved.control_part {
            self.world.entity_mut(e).insert(ControlPart(g.clone()));
        }
        if saved.debris {
            self.world.entity_mut(e).insert(crate::vessels::Debris);
        }
    }

    /// The part (index into `current_craft`) the vessel is controlled from, when it is not the root.
    pub fn control_part(&self, e: Entity) -> Option<usize> {
        let g = &self.world.get::<ControlPart>(e)?.0;
        self.current_craft(e)?
            .parts
            .iter()
            .position(|p| &p.guid == g)
    }

    /// The stage lit last, as an index into `current_craft().stages` (it and those above it have fired);
    /// `None` before the first stage press.
    fn lit_stage(&self, e: Entity) -> Option<usize> {
        let p = match self.world.get::<flight::Powered>(e) {
            Some(p) => p,
            None => &self.world.get::<flight::Dormant>(e)?.0,
        };
        if !p.ignited {
            return None;
        }
        let Some(id) = p.spec.stages.get(p.stage).map(|s| s.stage_id as usize) else {
            return Some(0);
        };
        let doc = &self.world.get::<CraftDoc>(e)?.0;
        let present = |i: usize| {
            self.world
                .get::<structure::Structure>(e)
                .is_none_or(|st| st.rigid.present.get(i).copied().unwrap_or(false))
        };
        // `current_craft` leaves out the stages left empty
        Some(
            doc.stages
                .iter()
                .take(id)
                .filter(|l| l.iter().any(|&i| present(i)))
                .count(),
        )
    }

    /// Give a vessel `craft` (its current one, edited) in place of the one it flies, keeping its flight
    /// state, stage, crew and what its parts are doing.
    #[allow(clippy::too_many_arguments)]
    fn refit(
        &mut self,
        e: Entity,
        craft: Craft,
        facing: DVec3,
        own_top: DVec3,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> Result<(), &'static str> {
        let old = match self.world.get::<flight::Powered>(e) {
            Some(p) => p.clone(),
            None => self
                .world
                .get::<flight::Dormant>(e)
                .map(|d| d.0.clone())
                .ok_or("not under power")?,
        };
        let len = self
            .world
            .get::<CraftDoc>(e)
            .map(|c| c.0.parts.len())
            .unwrap_or(0);
        let map = self.present_map(e, len);
        let crew: Vec<(usize, String)> = self
            .crew(e)
            .into_iter()
            .filter_map(|(i, n)| Some((map.get(i).copied().flatten()?, n)))
            .collect();
        let lit = self.lit_stage(e);
        let old_st = self.world.entity_mut(e).take::<structure::Structure>();
        let mut p = old.clone();
        p.spec = spec_for(&craft);
        p.stage = match lit {
            None => 0,
            Some(lit) => {
                let ids: Vec<usize> = p.spec.stages.iter().map(|s| s.stage_id as usize).collect();
                match ids.iter().position(|&i| i == lit) {
                    Some(k) => k,
                    // the lit stage has nothing left in it: the next press fires the one below
                    None => ids
                        .iter()
                        .position(|&i| i < lit)
                        .map(|k| k.saturating_sub(1))
                        .unwrap_or(ids.len()),
                }
            }
        };
        p.facing = facing;
        p.own_top = own_top;
        if self.world.get::<flight::Powered>(e).is_some() {
            self.world.entity_mut(e).insert(p);
        } else {
            self.world.entity_mut(e).insert(flight::Dormant(p));
        }
        self.set_craft(e, craft.clone());
        self.set_crew(e, crew);
        let mut st = structure::Structure::build(&craft, defs, resources);
        if let Some(old_st) = &old_st {
            st.carry_state_from(old_st, &map);
        }
        if own_top.length_squared() > 0.0 {
            st.frame_top = own_top;
        }
        self.set_structure(e, st);
        Ok(())
    }

    /// Move a part (index into `current_craft`) to another stage (index into its `stages`, as
    /// `StageReadout::stage_id` counts them) in flight. Only stages that have not fired yet take part or give
    /// one up.
    pub fn move_to_stage(
        &mut self,
        e: Entity,
        part: usize,
        stage: usize,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> Result<(), &'static str> {
        let mut craft = self.current_craft(e).ok_or("vessel has no parts")?;
        if part >= craft.parts.len() {
            return Err("no such part");
        }
        if stage > craft.stages.len() + 1 {
            return Err("no such stage");
        }
        let p = match self.world.get::<flight::Powered>(e) {
            Some(p) => p.clone(),
            None => self
                .world
                .get::<flight::Dormant>(e)
                .map(|d| d.0.clone())
                .ok_or("not under power")?,
        };
        // stages fire from the highest index down; the one lit and those above it are spent
        let lit = self.lit_stage(e).unwrap_or(usize::MAX);
        let from = craft.stages.iter().position(|s| s.contains(&part));
        if from.is_some_and(|f| f >= lit) || stage >= lit {
            return Err("that stage has already fired");
        }
        if from == Some(stage) {
            return Ok(());
        }
        for s in &mut craft.stages {
            s.retain(|&x| x != part);
        }
        while craft.stages.len() <= stage {
            craft.stages.push(Vec::new());
        }
        craft.stages[stage].push(part);
        self.refit(e, craft, p.facing, p.own_top, defs, resources, spec_for)
    }

    /// Control the vessel from one of its parts (index into `current_craft`; the root to go back): the
    /// craft is turned so that part's axis is the craft's +Y, so SAS, the navball and thrust all read
    /// from it. The vessel does not move. Not on the ground.
    pub fn control_from(
        &mut self,
        e: Entity,
        part: usize,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> Result<(), &'static str> {
        let craft = self.current_craft(e).ok_or("vessel has no parts")?;
        let target = craft.parts.get(part).ok_or("no such part")?;
        let p = self
            .world
            .get::<flight::Powered>(e)
            .ok_or("not under power")?
            .clone();
        if p.landed || p.level {
            return Err("not while on the ground or flying level");
        }
        let st = self
            .world
            .get::<structure::Structure>(e)
            .ok_or("vessel has no parts")?;
        let q = unit(target.local_rotation);
        let qa = [q.x, q.y, q.z, q.w];
        // where the part's axes point now
        let facing = st
            .axis_inertial(p.facing, crate::aero::rotate(qa, DVec3::new(0.0, 1.0, 0.0)))
            .normalized();
        let top = st
            .axis_inertial(
                p.facing,
                crate::aero::rotate(qa, DVec3::new(0.0, 0.0, -1.0)),
            )
            .normalized();
        let guid = target.guid.clone();
        // the part's own frame becomes the craft's
        let turned = structure::rotated(
            &craft,
            Quat {
                x: -q.x,
                y: -q.y,
                z: -q.z,
                w: q.w,
            },
        );
        self.refit(e, turned, facing, top, defs, resources, spec_for)?;
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            // a hold keeps pointing where it did
            if let flight::Attitude::Fixed(_) = p.attitude {
                p.attitude = flight::Attitude::Fixed(facing);
            }
        }
        if craft.parts.iter().position(|x| x.parent.is_none()) == Some(part) && q.w.abs() > 0.9999 {
            self.world.entity_mut(e).remove::<ControlPart>();
        } else {
            self.world.entity_mut(e).insert(ControlPart(guid));
        }
        Ok(())
    }

    /// The vessel's craft as it is now: parts it has lost left out, tanks holding what they hold.
    pub fn current_craft(&self, e: Entity) -> Option<Craft> {
        let doc = &self.world.get::<CraftDoc>(e)?.0;
        let Some(st) = self.world.get::<structure::Structure>(e) else {
            return Some(doc.clone());
        };
        let mut craft = doc.clone();
        for t in &st.net.tanks {
            let name = &st.net.resources[t.resource];
            if let Some(r) = craft.parts[t.part]
                .resources
                .iter_mut()
                .find(|r| r.resource.eq_ignore_ascii_case(name))
            {
                r.stored_units = t.stored;
            }
        }
        let keep: Vec<usize> = (0..craft.parts.len())
            .filter(|&i| st.rigid.present.get(i).copied().unwrap_or(false))
            .collect();
        Some(subset(&craft, &keep).0)
    }

    pub fn docked_links(&self, e: Entity) -> Vec<(usize, usize)> {
        self.world
            .get::<Docked>(e)
            .map(|d| d.links.clone())
            .unwrap_or_default()
    }

    /// Port pairs between nearby vessels close enough to pull together or latch (`Latch::acquiring`).
    pub fn find_latches(&mut self, defs: &[PartDef]) -> Vec<Latch> {
        let mut out: Vec<Latch> = self
            .port_pairs(defs)
            .into_iter()
            .filter(Latch::acquiring)
            .collect();
        out.sort_by(|x, y| x.distance_m.total_cmp(&y.distance_m));
        out
    }

    /// Where vessel `e`'s free port on part `part` (index into its `current_craft`) is now: its face and
    /// outward axis, inertial.
    fn port_now(&self, e: Entity, part: usize, defs: &[PartDef]) -> Option<(DVec3, DVec3)> {
        let craft = self.current_craft(e)?;
        let port = free_ports(&craft, defs)
            .into_iter()
            .find(|p| p.part == part)?;
        Some((
            to_inertial(self, e, port.position, false)?,
            to_inertial(self, e, port.axis, true)?.normalized(),
        ))
    }

    /// The structure's index of `current_craft` part `part` (the structure keeps lost parts' places).
    fn structure_index(&self, e: Entity, part: usize) -> usize {
        let len = self
            .world
            .get::<CraftDoc>(e)
            .map(|c| c.0.parts.len())
            .unwrap_or(0);
        self.present_map(e, len)
            .iter()
            .position(|&m| m == Some(part))
            .unwrap_or(part)
    }

    /// Every pair of fitting free ports on vessels under power whose faces are within `PORT_LOOK_M`, apart
    /// from pairs kept apart after undocking.
    fn port_pairs(&mut self, defs: &[PartDef]) -> Vec<Latch> {
        let vessels: Vec<Entity> = {
            let mut q = self
                .world
                .query_filtered::<Entity, (With<CraftDoc>, With<flight::Powered>)>();
            q.iter(&self.world).collect()
        };
        let mut out = Vec::new();
        for (i, &a) in vessels.iter().enumerate() {
            for &b in &vessels[i + 1..] {
                let Some(d) = self.distance_between(a, b) else {
                    continue;
                };
                if d > 200.0 {
                    continue;
                }
                let blocked = |x: Entity, y: Entity, px: usize, py: usize| {
                    self.world.get::<Undocked>(x).is_some_and(|u| {
                        u.from
                            .iter()
                            .any(|g| g.other == y && g.port == px && g.other_port == py)
                    })
                };
                let (Some(ca), Some(cb)) = (self.current_craft(a), self.current_craft(b)) else {
                    continue;
                };
                let (Some(pa), Some(pb)) = (
                    self.world.get::<flight::Powered>(a),
                    self.world.get::<flight::Powered>(b),
                ) else {
                    continue;
                };
                if pa.crashed || pb.crashed {
                    continue;
                }
                let rel_speed = (pa.v - pb.v).length();
                for port_a in free_ports(&ca, defs) {
                    for port_b in free_ports(&cb, defs) {
                        if !port_a
                            .def
                            .sizes
                            .iter()
                            .any(|s| port_b.def.sizes.contains(s))
                        {
                            continue;
                        }
                        if blocked(a, b, port_a.part, port_b.part)
                            || blocked(b, a, port_b.part, port_a.part)
                        {
                            continue;
                        }
                        let (Some(wa), Some(wb), Some(xa), Some(xb)) = (
                            to_inertial(self, a, port_a.position, false),
                            to_inertial(self, b, port_b.position, false),
                            to_inertial(self, a, port_a.axis, true),
                            to_inertial(self, b, port_b.axis, true),
                        ) else {
                            continue;
                        };
                        let dist = (wa - wb).length();
                        if dist > PORT_LOOK_M {
                            continue;
                        }
                        out.push(Latch {
                            a,
                            port_a: port_a.clone(),
                            b,
                            port_b: port_b.clone(),
                            distance_m: dist,
                            face_a: wa,
                            face_b: wb,
                            axis_a: xa.normalized(),
                            axis_b: xb.normalized(),
                            rel_speed_mps: rel_speed,
                        });
                    }
                }
            }
        }
        out
    }

    /// Let undocked port pairs dock again once their faces are their `min_distance_m` apart (or either port is
    /// gone).
    fn refresh_undock_blocks(&mut self, defs: &[PartDef]) {
        let blocked: Vec<(Entity, Vec<Disengaged>)> = {
            let mut q = self.world.query::<(Entity, &Undocked)>();
            q.iter(&self.world)
                .map(|(e, u)| (e, u.from.clone()))
                .collect()
        };
        for (e, from) in blocked {
            let keep: Vec<Disengaged> = from
                .into_iter()
                .filter(|g| {
                    match (
                        self.port_now(e, g.port, defs),
                        self.port_now(g.other, g.other_port, defs),
                    ) {
                        (Some((x, _)), Some((y, _))) => (x - y).length() <= g.min_distance_m,
                        _ => false,
                    }
                })
                .collect();
            if keep.is_empty() {
                self.world.entity_mut(e).remove::<Undocked>();
            } else {
                self.world.entity_mut(e).insert(Undocked { from: keep });
            }
        }
    }

    /// Keep port `pa` of `a` and port `pb` of `b` from latching until they are `min_distance_m` apart.
    fn block_relatch(&mut self, a: Entity, pa: usize, b: Entity, pb: usize, min_distance_m: f64) {
        for (x, px, y, py) in [(a, pa, b, pb), (b, pb, a, pa)] {
            let mut u = self.world.get::<Undocked>(x).cloned().unwrap_or_default();
            u.from.push(Disengaged {
                other: y,
                port: px,
                other_port: py,
                min_distance_m,
            });
            self.world.entity_mut(x).insert(u);
        }
    }

    pub fn set_docked(&mut self, e: Entity, links: Vec<(usize, usize)>) {
        self.world.entity_mut(e).insert(Docked { links });
    }

    /// The body a vessel is around.
    pub fn body_of(&self, e: Entity) -> Option<Entity> {
        self.world.get::<bodies::ReferenceBody>(e).map(|b| b.0)
    }

    /// Give a vessel the flight state, craft, crew and structure of `craft` (the stage already lit).
    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_vessel(
        &mut self,
        e: Entity,
        craft: &Craft,
        r: DVec3,
        v: DVec3,
        facing: DVec3,
        crew: Vec<(usize, String)>,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) {
        let mut p = flight::Powered::new(spec_for(craft), r, v, false);
        p.facing = facing;
        p.ignited = true;
        self.world.entity_mut(e).remove::<flight::Dormant>();
        self.set_powered(e, p);
        self.set_craft(e, craft.clone());
        // crew first, so the structure does not seat a fresh one
        self.set_crew(e, crew);
        self.set_structure(e, structure::Structure::build(craft, defs, resources));
    }

    /// Old-to-new part indices of a vessel's craft once its lost parts are left out (`current_craft`).
    fn present_map(&self, e: Entity, len: usize) -> Vec<Option<usize>> {
        let present: Vec<bool> = self
            .world
            .get::<structure::Structure>(e)
            .map(|st| st.rigid.present.clone())
            .unwrap_or_else(|| vec![true; len]);
        let mut n = 0;
        present
            .iter()
            .map(|&p| {
                p.then(|| {
                    n += 1;
                    n - 1
                })
            })
            .collect()
    }

    /// The docking step, as the original's `Module_DockingNode` runs it each physics tick: port pairs
    /// facing each other within the acquire angle are left out of hull contacts and stop each other at their
    /// faces; within acquire range and faster than the capture speed they pull together and turn to face
    /// each other (`ApplyAcquireForces`); slower than that they latch (`DockToModule`) and the second vessel
    /// becomes part of the first (its crew, tanks and reports go along). The forces act for the time since
    /// the step last ran. Returns (kept, absorbed) pairs; the absorbed vessels are gone.
    pub fn dock_latched(
        &mut self,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> Vec<(Entity, Entity)> {
        let now = self.tick().0;
        let dt = self
            .world
            .get_resource::<LastDockTick>()
            .map(|t| now.saturating_sub(t.0) as f64 / crate::TICKS_PER_SECOND as f64)
            .unwrap_or(0.0);
        self.world.insert_resource(LastDockTick(now));
        self.refresh_undock_blocks(defs);
        let mut pairs = self.port_pairs(defs);
        pairs.sort_by(|x, y| x.distance_m.total_cmp(&y.distance_m));
        let aligned: Vec<(Entity, usize, Entity, usize)> = pairs
            .iter()
            .filter(|l| l.aligned())
            .map(|l| {
                (
                    l.a,
                    self.structure_index(l.a, l.port_a.part),
                    l.b,
                    self.structure_index(l.b, l.port_b.part),
                )
            })
            .collect();
        self.world.insert_resource(AlignedPorts(aligned));
        for l in pairs.iter().filter(|l| l.aligned()) {
            self.port_faces_meet(l);
        }
        if dt > 0.0 {
            for l in pairs.iter().filter(|l| l.acquiring() && !l.captures()) {
                self.pull_ports(l, dt);
            }
        }
        let mut used = Vec::new();
        let mut out = Vec::new();
        for l in pairs.into_iter().filter(Latch::captures) {
            if used.contains(&l.a) || used.contains(&l.b) {
                continue;
            }
            if self.join(&l, defs, resources, spec_for) {
                used.push(l.a);
                used.push(l.b);
                out.push((l.a, l.b));
            }
        }
        out
    }

    /// Each port pulls toward the other with half its acquire force and turns toward facing it with half its
    /// acquire torque, both growing as the faces close to 5 cm (`ApplyAcquireForces`), for `dt` seconds. The
    /// roll torque is left out: every stock port's `AcquireTorqueRoll` is zero.
    fn pull_ports(&mut self, l: &Latch, dt: f64) {
        let (Some(mut pa), Some(mut pb)) = (self.powered(l.a).cloned(), self.powered(l.b).cloned())
        else {
            return;
        };
        let (Some(ia), Some(ib)) = (self.blow_inertia(l.a), self.blow_inertia(l.b)) else {
            return;
        };
        let gap = l.face_b - l.face_a;
        let num = 1.0 / gap.length().max(0.05);
        let turn = l.axis_a.cross(l.axis_b * -1.0);
        let (da, db) = (&l.port_a.def, &l.port_b.def);
        let fa = gap * (num * da.acquire_force_kn * 0.5);
        let fb = gap * (-num * db.acquire_force_kn * 0.5);
        let ta = turn * (num * da.acquire_torque_knm * 0.5);
        let tb = turn * (-num * db.acquire_torque_knm * 0.5);
        let (ra, rb) = (l.face_a - pa.r, l.face_b - pb.r);
        push(&mut pa, fa * dt, ra, ta * dt, ia);
        push(&mut pb, fb * dt, rb, tb * dt, ib);
        self.set_powered(l.a, pa);
        self.set_powered(l.b, pb);
    }

    /// Ports facing each other stop at their faces: when `b`'s face has come through `a`'s (within the
    /// port's width and depth) and they are still closing, an equal and opposite blow along `a`'s axis, with
    /// the hull contacts' restitution, and they are pushed back to touching. A port struck past its crash
    /// tolerance breaks off.
    fn port_faces_meet(&mut self, l: &Latch) {
        let n = l.axis_a;
        let d = l.face_b - l.face_a;
        let gap = d.dot(n);
        if gap >= 0.0 {
            return;
        }
        let size = |e: Entity, part: usize| {
            self.world
                .get::<structure::Structure>(e)
                .map(|st| st.part_box(self.structure_index(e, part)).1)
                .unwrap_or(DVec3::new(0.3, 0.3, 0.3))
        };
        let (sa, sb) = (size(l.a, l.port_a.part), size(l.b, l.port_b.part));
        let width = sa.x.max(sa.z).max(sb.x).max(sb.z).max(0.1);
        let depth = sa.y.max(sb.y).max(0.05) * 2.0;
        if (d - n * gap).length() > width || gap < -depth {
            return;
        }
        let (Some(pa), Some(pb)) = (self.powered(l.a).cloned(), self.powered(l.b).cloned()) else {
            return;
        };
        let (Some(ia), Some(ib)) = (self.blow_inertia(l.a), self.blow_inertia(l.b)) else {
            return;
        };
        let inv = |p: &flight::Powered, i: f64| {
            if p.landed {
                (0.0, 0.0)
            } else {
                (1.0 / p.mass_t.max(0.001), 1.0 / i.max(1e-6))
            }
        };
        let ((inv_ma, inv_ia), (inv_mb, inv_ib)) = (inv(&pa, ia), inv(&pb, ib));
        if inv_ma + inv_mb <= 0.0 {
            return;
        }
        let (ra, rb) = (l.face_a - pa.r, l.face_b - pb.r);
        let spin = |p: &flight::Powered| p.angvel + p.facing * p.roll_rate;
        let va = pa.v + spin(&pa).cross(ra);
        let vb = pb.v + spin(&pb).cross(rb);
        let vn = (vb - va).dot(n);
        let (mut na, mut nb) = (pa.clone(), pb.clone());
        let (si, sj) = (
            self.structure_index(l.a, l.port_a.part),
            self.structure_index(l.b, l.port_b.part),
        );
        let mut broken = Vec::new();
        if vn < 0.0 {
            let k = inv_ma
                + inv_mb
                + ra.cross(n).length_squared() * inv_ia
                + rb.cross(n).length_squared() * inv_ib;
            let j = n * (-(1.0 + crate::collide::RESTITUTION) * vn / k);
            push(&mut na, j * -1.0, ra, DVec3::ZERO, ia);
            push(&mut nb, j, rb, DVec3::ZERO, ib);
            self.world
                .get_resource_or_insert_with(crate::collide::Contacts::default)
                .0
                .push(crate::collide::Contact {
                    a: l.a,
                    part_a: si,
                    b: l.b,
                    part_b: sj,
                    speed_mps: -vn,
                });
            // a port struck harder than it can take breaks off, as hulls do
            for (e, part) in [(l.a, si), (l.b, sj)] {
                let tolerance = self
                    .world
                    .get::<structure::Structure>(e)
                    .and_then(|st| st.crash_tolerance.get(part).copied())
                    .filter(|t| *t > 0.0)
                    .unwrap_or(flight::CRASH_SPEED_MPS);
                if -vn > tolerance {
                    broken.push((e, part));
                }
            }
        }
        let share = -gap / (inv_ma + inv_mb);
        if !na.landed {
            na.r = na.r - n * (share * inv_ma);
        }
        if !nb.landed {
            nb.r = nb.r + n * (share * inv_mb);
        }
        self.set_powered(l.a, na);
        self.set_powered(l.b, nb);
        for (e, part) in broken {
            self.break_struck_part(e, part);
        }
    }

    /// Latch `l.b` onto `l.a` at their ports. `a` stays where it is; `b` is set onto `a`'s port, as the
    /// original aligns the docking part to the other (`AlignToTransform`). Momentum is kept.
    fn join(
        &mut self,
        l: &Latch,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> bool {
        let (Some(ca), Some(cb)) = (self.current_craft(l.a), self.current_craft(l.b)) else {
            return false;
        };
        let (Some(pa), Some(pb)) = (self.powered(l.a).cloned(), self.powered(l.b).cloned()) else {
            return false;
        };
        let doc_len = |u: &Universe, e: Entity| {
            u.world
                .get::<CraftDoc>(e)
                .map(|c| c.0.parts.len())
                .unwrap_or(0)
        };
        let map_a = self.present_map(l.a, doc_len(self, l.a));
        let map_b = self.present_map(l.b, doc_len(self, l.b));
        let off = ca.parts.len();
        let merged = merge(&ca, &l.port_a, &cb, &l.port_b);
        let pick = |m: &Vec<Option<usize>>, i: usize| m.get(i).copied().flatten();
        let mut crew: Vec<(usize, String)> = self
            .crew(l.a)
            .into_iter()
            .filter_map(|(i, n)| Some((pick(&map_a, i)?, n)))
            .collect();
        crew.extend(
            self.crew(l.b)
                .into_iter()
                .filter_map(|(i, n)| Some((pick(&map_b, i)? + off, n))),
        );
        let mut links: Vec<(usize, usize)> = self
            .docked_links(l.a)
            .into_iter()
            .filter_map(|(x, y)| Some((pick(&map_a, x)?, pick(&map_a, y)?)))
            .collect();
        links.extend(
            self.docked_links(l.b)
                .into_iter()
                .filter_map(|(x, y)| Some((pick(&map_b, x)? + off, pick(&map_b, y)? + off))),
        );
        links.push((l.port_a.part, l.port_b.part + off));
        let m = pa.mass_t + pb.mass_t;
        let v = (pa.v * pa.mass_t + pb.v * pb.mass_t) * (1.0 / m);
        let mut science = self.science_carried(l.a);
        science.extend(self.science_carried(l.b));
        let frame_a = self.frame_of(l.a);
        self.rebuild_vessel(
            l.a, &merged, pa.r, v, pa.facing, crew, defs, resources, spec_for,
        );
        self.keep_attitude(l.a, &pa, frame_a.as_ref());
        // the joined centre where `a`'s parts still are
        if let (Some(old), Some(new)) = (&frame_a, self.world.get::<structure::Structure>(l.a)) {
            let shift = old.offset(new.rigid.rest_centre() - old.centre);
            if let Some(mut p) = self.powered(l.a).cloned() {
                p.r = pa.r + shift;
                self.set_powered(l.a, p);
            }
        }
        self.set_docked(l.a, links);
        self.set_science_carried(l.a, science);
        self.despawn_vessel(l.b);
        true
    }

    /// Vessel `e`'s structure frame now: its axes, rest centre and roll reference.
    fn frame_of(&self, e: Entity) -> Option<Frame> {
        let facing = self.powered(e)?.facing;
        let st = self.world.get::<structure::Structure>(e)?;
        let axis = |v: DVec3| st.offset_inertial(facing, v);
        Some(Frame {
            axes: [
                axis(DVec3::new(1.0, 0.0, 0.0)),
                axis(DVec3::new(0.0, 1.0, 0.0)),
                axis(DVec3::new(0.0, 0.0, 1.0)),
            ],
            centre: st.rigid.rest_centre(),
            top: st.frame_top,
        })
    }

    /// After a rebuild, the vessel turns and spins as `old` did, in the frame `frame` gave its parts.
    fn keep_attitude(&mut self, e: Entity, old: &flight::Powered, frame: Option<&Frame>) {
        let top = frame
            .map(|f| f.top)
            .filter(|t| t.length_squared() > 0.0)
            .unwrap_or_else(|| flight::vessel_top(old));
        if let Some(mut p) = self.powered(e).cloned() {
            p.own_top = top;
            p.angvel = old.angvel;
            p.roll_rate = old.roll_rate;
            self.set_powered(e, p);
        }
        if let Some(mut st) = self.world.get_mut::<structure::Structure>(e) {
            st.frame_top = top;
        }
    }

    /// Undock at one of a vessel's docked joints (the last one docked when `port` is `None`, else the joint
    /// with that port part on either side): what docked comes away as a new vessel. Both halves start where
    /// their parts were, turning as the whole did, and are pushed apart along the port's axis by its ejection
    /// force for a second; the two ports do not latch again until they are the port's re-engage distance
    /// apart. Returns the new vessel.
    pub fn undock(
        &mut self,
        e: Entity,
        port: Option<usize>,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> Result<Entity, String> {
        let links = self.docked_links(e);
        let link = match port {
            Some(p) => links.iter().position(|&(a, b)| a == p || b == p),
            None => (!links.is_empty()).then(|| links.len() - 1),
        }
        .ok_or("not docked there")?;
        let (pa, pb) = links[link];
        let craft = self.current_craft(e).ok_or("vessel has no craft")?;
        let p = self.powered(e).cloned().ok_or("vessel is not in flight")?;
        let ((stay, map_stay), (go, map_go)) = split(&craft, pb);
        let pick = |m: &Vec<Option<usize>>, i: usize| m.get(i).copied().flatten();
        let crew = self.crew(e);
        let crew_of = |m: &Vec<Option<usize>>| -> Vec<(usize, String)> {
            crew.iter()
                .filter_map(|(i, n)| Some((pick(m, *i)?, n.clone())))
                .collect()
        };
        let links_of = |m: &Vec<Option<usize>>| -> Vec<(usize, usize)> {
            links
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != link)
                .filter_map(|(_, &(a, b))| Some((pick(m, a)?, pick(m, b)?)))
                .collect()
        };
        // apart along the docked port's axis
        let port_def = defs.iter().find(|d| d.id == craft.parts[pb].part_id);
        let dock = port_def.and_then(|d| d.docking.clone());
        let axis = port_def
            .zip(dock.as_ref())
            .and_then(|(d, k)| d.attach_nodes.iter().find(|a| a.id == k.node))
            .map(|n| q_rotate(unit(craft.parts[pb].local_rotation), n.orientation))
            .unwrap_or(DVec3::new(0.0, 1.0, 0.0));
        // the port of the part that docked points back at the vessel it docked to: away is the other way
        let dir = to_inertial(self, e, axis, true)
            .map(|d| d.normalized() * -1.0)
            .unwrap_or(p.facing);
        let ejection = dock.as_ref().map(|k| k.undock_ejection_kn).unwrap_or(0.4);
        let reengage = dock
            .as_ref()
            .map(|k| k.min_distance_to_reengage_m)
            .unwrap_or(1.0);
        let m_go = go.mass(defs, resources).wet_t().max(0.01);
        let m_stay = stay.mass(defs, resources).wet_t().max(0.01);
        let body = self.body_of(e).ok_or("no body")?;
        // each half's centre where its parts are now
        let frame = self.frame_of(e);
        let at = |c: &Craft| match &frame {
            Some(f) => {
                let centre = structure::Structure::build(c, defs, resources)
                    .rigid
                    .rest_centre();
                p.r + f.offset(centre - f.centre)
            }
            None => p.r,
        };
        let (r_stay, r_go) = (at(&stay), at(&go));
        let name = format!("{} (undocked)", go.name);
        let v_go = p.v + dir * (ejection / m_go);
        let ne = self.spawn_vessel(&name, body, r_go, v_go);
        self.rebuild_vessel(
            e,
            &stay,
            r_stay,
            p.v - dir * (ejection / m_stay),
            p.facing,
            crew_of(&map_stay),
            defs,
            resources,
            spec_for,
        );
        self.keep_attitude(e, &p, frame.as_ref());
        self.set_docked(e, links_of(&map_stay));
        self.rebuild_vessel(
            ne,
            &go,
            r_go,
            v_go,
            p.facing,
            crew_of(&map_go),
            defs,
            resources,
            spec_for,
        );
        self.keep_attitude(ne, &p, frame.as_ref());
        self.set_docked(ne, links_of(&map_go));
        if let (Some(sa), Some(sb)) = (pick(&map_stay, pa), pick(&map_go, pb)) {
            self.block_relatch(e, sa, ne, sb, reengage);
        }
        Ok(ne)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> Vec<PartDef> {
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap()
    }

    fn part(
        id: &str,
        parent: Option<usize>,
        own: Option<&str>,
        theirs: Option<&str>,
        y: f64,
    ) -> CraftPart {
        CraftPart {
            symmetry: 0,
            actions: Vec::new(),
            flight: None,
            colors: None,
            guid: id.into(),
            part_id: id.into(),
            parent,
            own_node: own.map(String::from),
            parent_node: theirs.map(String::from),
            local_position: DVec3::new(0.0, y, 0.0),
            local_rotation: Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            stage_index: 0,
            fuel_crossfeed: true,
            resources: vec![],
            fuel_line_target: None,
        }
    }

    /// probe core with a docking port on top
    fn ship(name: &str) -> Craft {
        Craft {
            colors: None,
            loose: Vec::new(),
            name: name.into(),
            description: String::new(),
            parts: vec![
                part("probe_1v_disk_electricity", None, None, None, 0.0),
                part(
                    "dockingport_1v_inline",
                    Some(0),
                    Some("bottom"),
                    Some("top"),
                    0.3,
                ),
            ],
            stages: vec![],
        }
    }

    fn spec(c: &Craft) -> flight::FlightSpec {
        let defs = defs();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let report = ose_data::staging::delta_v_by_stage(c, &defs, &res);
        flight::FlightSpec::from_report(&report, &[], &[], 1.0)
    }

    fn resources() -> Vec<ose_data::ResourceDef> {
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap()
    }

    /// Ship A in a circular Kerbin orbit pointing +y, ship B ahead of it facing `b_facing`, B's port face
    /// `gap_m` beyond A's along +y and B closing on A at `closing_mps`.
    fn pair(gap_m: f64, closing_mps: f64, b_facing: DVec3) -> (Universe, Entity, Entity) {
        let d = defs();
        let res = resources();
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
        kerbin.parent = None;
        kerbin.orbit = None;
        kerbin.soi_radius_m = None;
        let mut u = Universe::with_bodies(&[kerbin]);
        let body = u.body_entity("Kerbin").unwrap();
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let r0 = DVec3::new(700_000.0, 0.0, 0.0);
        let v0 = DVec3::new(0.0, (mu / 700_000.0).sqrt(), 0.0);
        let place = |u: &mut Universe, c: &Craft, r: DVec3, facing: DVec3| {
            let e = u.spawn_vessel("ship", body, r, v0);
            u.rebuild_vessel(e, c, r, v0, facing, vec![], &d, &res, &spec);
            e
        };
        let (a_craft, b_craft) = (ship("A"), ship("B"));
        let a = place(&mut u, &a_craft, r0, DVec3::new(0.0, 1.0, 0.0));
        let pa = free_ports(&a_craft, &d)[0].clone();
        let pb = free_ports(&b_craft, &d)[0].clone();
        let wa = to_inertial(&u, a, pa.position, false).unwrap();
        let b = place(&mut u, &b_craft, r0, b_facing.normalized());
        let wb = to_inertial(&u, b, pb.position, false).unwrap();
        let mut p = u.powered(b).unwrap().clone();
        p.r = p.r + (wa - wb) + DVec3::new(0.0, gap_m, 0.0);
        p.v = p.v - DVec3::new(0.0, closing_mps, 0.0);
        u.set_powered(b, p);
        (u, a, b)
    }

    /// Vessel `e`'s free port: its face and outward axis now.
    fn port_of(u: &Universe, e: Entity) -> (DVec3, DVec3) {
        let d = defs();
        let part = free_ports(&u.current_craft(e).unwrap(), &d)[0].part;
        u.port_now(e, part, &d).unwrap()
    }

    /// The faces of A's and B's ports and how far B's is in front of A's along A's axis.
    fn faces(u: &Universe, a: Entity, b: Entity) -> (DVec3, DVec3, f64) {
        let ((fa, xa), (fb, _)) = (port_of(u, a), port_of(u, b));
        (fa, fb, (fb - fa).dot(xa))
    }

    #[test]
    fn two_ships_drifting_port_to_port_dock_and_undock() {
        let d = defs();
        let res = resources();
        // B's port 0.5 m beyond A's, drifting with it
        let (mut u, a, b) = pair(0.5, 0.0, DVec3::new(0.0, -1.0, 0.0));
        let joined = u.dock_latched(&d, &res, &spec);
        assert_eq!(joined, vec![(a, b)]);
        assert!(u.vessel(b).is_none(), "B is part of A now");
        let c = u.current_craft(a).unwrap();
        assert_eq!(c.parts.len(), 4);
        assert_eq!(u.docked_links(a), vec![(1, 3)]);
        // and apart again
        let n = u.undock(a, None, &d, &res, &spec).unwrap();
        assert_eq!(u.current_craft(a).unwrap().parts.len(), 2);
        assert_eq!(u.current_craft(n).unwrap().parts.len(), 2);
        assert!(u.docked_links(a).is_empty());
        let sep = u.powered(n).unwrap().v - u.powered(a).unwrap().v;
        assert!(sep.length() > 0.01, "pushed apart ({sep:?})");
        // they do not latch again straight away
        assert!(u.dock_latched(&d, &res, &spec).is_empty());
    }

    #[test]
    fn docking_and_undocking_leave_every_part_where_it_was() {
        let d = defs();
        let res = resources();
        let (mut u, a, b) = pair(0.3, 0.0, DVec3::new(0.0, -1.0, 0.0));
        let a_probe = to_inertial(&u, a, ship("A").parts[0].local_position, false).unwrap();
        let b_probe = to_inertial(&u, b, ship("B").parts[0].local_position, false).unwrap();
        assert_eq!(u.dock_latched(&d, &res, &spec), vec![(a, b)]);
        let m = u.current_craft(a).unwrap();
        let at = |u: &Universe, e: Entity, c: &Craft, i: usize| {
            to_inertial(u, e, c.parts[i].local_position, false).unwrap()
        };
        // A's parts stay put; B's are set onto A's port (they were 0.3 m off it)
        assert!((at(&u, a, &m, 0) - a_probe).length() < 1e-6);
        assert!(((at(&u, a, &m, 2) - b_probe).length() - 0.3).abs() < 1e-6);
        let before: Vec<DVec3> = (0..4).map(|i| at(&u, a, &m, i)).collect();
        let n = u.undock(a, None, &d, &res, &spec).unwrap();
        let (stay, go) = (u.current_craft(a).unwrap(), u.current_craft(n).unwrap());
        // the half that stays holds parts 0 and 1, the half that leaves B's port (3) and probe (2)
        for (e, c, i, was) in [
            (a, &stay, 0, 0),
            (a, &stay, 1, 1),
            (n, &go, 0, 3),
            (n, &go, 1, 2),
        ] {
            let now = at(&u, e, c, i);
            assert!(
                (now - before[was]).length() < 1e-6,
                "part {was} moved {:?} on undocking",
                now - before[was]
            );
        }
    }

    #[test]
    fn undocked_ports_latch_again_only_past_their_reengage_distance() {
        let d = defs();
        let res = resources();
        let (mut u, a, b) = pair(0.0, 0.0, DVec3::new(0.0, -1.0, 0.0));
        assert_eq!(u.dock_latched(&d, &res, &spec), vec![(a, b)]);
        let n = u.undock(a, None, &d, &res, &spec).unwrap();
        let reengage = defs()
            .into_iter()
            .find(|p| p.id == "dockingport_1v_inline")
            .unwrap()
            .docking
            .unwrap()
            .min_distance_to_reengage_m;
        // drift apart, then come back slowly
        let mut cleared_at = None;
        for _ in 0..(10 * crate::TICKS_PER_SECOND) {
            u.step();
            assert!(
                u.dock_latched(&d, &res, &spec).is_empty(),
                "relatched while leaving"
            );
            if u.world.get::<Undocked>(a).is_none() {
                cleared_at = Some(faces(&u, a, n).2);
                break;
            }
        }
        let gap = cleared_at.expect("the block clears once they are apart");
        assert!(gap > reengage, "cleared at {gap} m");
        let pa = u.powered(a).unwrap().clone();
        let mut p = u.powered(n).unwrap().clone();
        p.v = pa.v - (p.r - pa.r).normalized() * 0.1;
        u.set_powered(n, p);
        let mut joined = Vec::new();
        for _ in 0..(60 * crate::TICKS_PER_SECOND) {
            u.step();
            joined = u.dock_latched(&d, &res, &spec);
            if !joined.is_empty() {
                break;
            }
        }
        assert!(
            joined == vec![(a, n)] || joined == vec![(n, a)],
            "docked again on the way back: {joined:?}"
        );
    }

    #[test]
    fn a_fast_approach_bounces_off_the_faces_then_the_ports_pull_together_and_latch() {
        let d = defs();
        let res = resources();
        // 0.9 m apart inside the 1 m acquire range, closing at 0.6 m/s: too fast to capture (0.3 m/s)
        let (mut u, a, b) = pair(0.9, 0.6, DVec3::new(0.0, -1.0, 0.0));
        let mut fastest: f64 = 0.0;
        let mut deepest: f64 = 0.0;
        let mut bounced = false;
        let mut joined = Vec::new();
        u.dock_latched(&d, &res, &spec);
        for _ in 0..(20 * crate::TICKS_PER_SECOND) {
            u.step();
            let rel = u.powered(b).unwrap().v - u.powered(a).unwrap().v;
            fastest = fastest.max(rel.length());
            joined = u.dock_latched(&d, &res, &spec);
            if !joined.is_empty() {
                break;
            }
            // once the docking step has run the faces are touching at most
            deepest = deepest.min(faces(&u, a, b).2);
            bounced |= rel.y > 0.0;
        }
        assert!(
            fastest > 0.6,
            "the acquire force pulled them faster ({fastest})"
        );
        assert!(bounced, "B came off A's face before latching");
        assert!(
            deepest > -1e-6,
            "the faces did not pass through ({deepest} m)"
        );
        assert_eq!(joined, vec![(a, b)]);
    }

    #[test]
    fn the_acquire_torque_turns_a_skewed_port_toward_facing_and_misaligned_ports_strike() {
        let d = defs();
        let res = resources();
        // 30 degrees off head on (inside the acquire angle) and drifting apart at 0.5 m/s: no latch, but a
        // turn toward facing
        let skew = DVec3::new(0.5, -(3.0f64).sqrt() / 2.0, 0.0);
        let (mut u, a, b) = pair(0.5, -0.5, skew);
        let rel = (u.powered(a).unwrap().v - u.powered(b).unwrap().v).length();
        let l = u.find_latches(&d);
        assert_eq!(l.len(), 1, "acquiring");
        assert!(!l[0].captures(), "{rel} m/s, facing {}", l[0].facing());
        assert!(u.dock_latched(&d, &res, &spec).is_empty());
        u.step();
        let face = |u: &Universe| -port_of(u, a).1.dot(port_of(u, b).1);
        let f0 = face(&u);
        // one tick of acquire torque, then a tick of turning
        assert!(u.dock_latched(&d, &res, &spec).is_empty());
        u.step();
        assert!(
            face(&u) > f0,
            "turned toward facing: {} -> {}",
            f0,
            face(&u)
        );
        // side by side (B turned 90 degrees) its port strikes A's as a hull does
        let (mut u, a, b) = pair(-0.1, 0.2, DVec3::new(1.0, 0.0, 0.0));
        u.dock_latched(&d, &res, &spec);
        assert!(u
            .world
            .get_resource::<AlignedPorts>()
            .is_some_and(|x| x.0.is_empty()));
        u.step();
        let hits = u.take_contacts();
        assert!(
            hits.iter()
                .any(|h| (h.a == a && h.b == b) || (h.a == b && h.b == a)),
            "struck: {hits:?}"
        );
    }

    #[test]
    fn a_free_port_points_out_of_the_craft() {
        let d = defs();
        let s = ship("A");
        let ports = free_ports(&s, &d);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].part, 1);
        assert!(ports[0].axis.y > 0.9, "{:?}", ports[0].axis);
    }

    #[test]
    fn merging_turns_the_second_ship_round_and_splitting_gives_it_back() {
        let d = defs();
        let (a, b) = (ship("A"), ship("B"));
        let (pa, pb) = (free_ports(&a, &d)[0].clone(), free_ports(&b, &d)[0].clone());
        let m = merge(&a, &pa, &b, &pb);
        assert_eq!(m.parts.len(), 4);
        assert!(m.is_tree());
        // b's port hangs off a's port, b's probe beyond it, upside down
        assert_eq!(m.parts[3].parent, Some(1));
        assert_eq!(m.parts[2].parent, Some(3));
        assert!(m.parts[2].local_position.y > m.parts[3].local_position.y);
        assert!(free_ports(&m, &d).is_empty(), "both ports are taken");
        // undock at b's port
        let ((stay, _), (go, _)) = split(&m, 3);
        assert_eq!(stay.parts.len(), 2);
        assert_eq!(go.parts.len(), 2);
        assert!(stay.is_tree() && go.is_tree());
        assert_eq!(go.parts[0].part_id, "dockingport_1v_inline");
        assert_eq!(free_ports(&go, &d).len(), 1);
        assert_eq!(free_ports(&stay, &d).len(), 1);
    }
}

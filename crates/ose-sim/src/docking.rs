//! Docking: finding docking ports that meet, and the craft surgery that joins two vessels into one and takes
//! them apart again (see `docs/reference-notes/docking.md`).
//!
//! A vessel keeps its craft ([`CraftDoc`]) so it can be merged and split. Ports latch when they are within
//! their acquire range, facing each other within the acquire angle and drifting slower than 1 m/s (the
//! original pulls them together from there with its acquire force and latches at the capture range; we
//! latch at once). The docked vessel's parts are re-rooted under the port they docked to, turned so the ports
//! face each other, in the first vessel's craft frame.

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

/// After undocking, these two vessels do not latch again until they have moved apart.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct Undocked {
    pub from: Vec<Entity>,
}

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

/// Where a craft-frame point of vessel `e` is now, body-centred inertial.
fn to_inertial(u: &Universe, e: Entity, craft_point: DVec3, is_dir: bool) -> Option<DVec3> {
    let r = u.position_of(e)?;
    let facing = u
        .world
        .get::<flight::Powered>(e)
        .map(|p| p.facing)
        .or_else(|| u.world.get::<flight::Dormant>(e).map(|d| d.0.facing))
        .unwrap_or(DVec3::new(0.0, 0.0, 1.0));
    let (bx, by, bz) = vessel_axes(facing);
    let vf = |p: DVec3| DVec3::new(p.x, p.y, -p.z);
    let local = if is_dir {
        vf(craft_point)
    } else {
        let centre = u
            .world
            .get::<structure::Structure>(e)
            .map(|s| s.rigid.rest_centre())
            .unwrap_or(DVec3::ZERO);
        vf(craft_point) - centre
    };
    let w = bx * local.x + by * local.y + bz * local.z;
    Some(if is_dir { w } else { r + w })
}

/// A pair of ports that can latch now: (vessel a, port a, vessel b, port b).
#[derive(Clone, Debug, PartialEq)]
pub struct Latch {
    pub a: Entity,
    pub port_a: Port,
    pub b: Entity,
    pub port_b: Port,
    pub distance_m: f64,
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

    /// Port pairs between nearby vessels that would latch now.
    pub fn find_latches(&mut self, defs: &[PartDef]) -> Vec<Latch> {
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
                let blocked = |x: Entity, y: Entity| {
                    self.world
                        .get::<Undocked>(x)
                        .is_some_and(|u| u.from.contains(&y))
                };
                if blocked(a, b) || blocked(b, a) {
                    continue;
                }
                let (Some(ca), Some(cb)) = (self.current_craft(a), self.current_craft(b)) else {
                    continue;
                };
                let (Some(pa), Some(pb)) = (
                    self.world.get::<flight::Powered>(a),
                    self.world.get::<flight::Powered>(b),
                ) else {
                    continue;
                };
                if (pa.v - pb.v).length() > 1.0 || pa.crashed || pb.crashed {
                    continue;
                }
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
                        let (Some(wa), Some(wb), Some(xa), Some(xb)) = (
                            to_inertial(self, a, port_a.position, false),
                            to_inertial(self, b, port_b.position, false),
                            to_inertial(self, a, port_a.axis, true),
                            to_inertial(self, b, port_b.axis, true),
                        ) else {
                            continue;
                        };
                        let dist = (wa - wb).length();
                        let range = port_a.def.acquire_range_m.max(port_b.def.acquire_range_m);
                        let facing = -xa.normalized().dot(xb.normalized());
                        let min_dot = port_a
                            .def
                            .acquire_min_fwd_dot
                            .max(port_b.def.acquire_min_fwd_dot);
                        if dist <= range && facing >= min_dot {
                            out.push(Latch {
                                a,
                                port_a: port_a.clone(),
                                b,
                                port_b: port_b.clone(),
                                distance_m: dist,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Clear undock blocks between vessels that are far enough apart again.
    pub fn refresh_undock_blocks(&mut self) {
        let blocked: Vec<(Entity, Vec<Entity>)> = {
            let mut q = self.world.query::<(Entity, &Undocked)>();
            q.iter(&self.world)
                .map(|(e, u)| (e, u.from.clone()))
                .collect()
        };
        for (e, from) in blocked {
            let keep: Vec<Entity> = from
                .into_iter()
                .filter(|&o| self.distance_between(e, o).is_some_and(|d| d < 3.0))
                .collect();
            if keep.is_empty() {
                self.world.entity_mut(e).remove::<Undocked>();
            } else {
                self.world.entity_mut(e).insert(Undocked { from: keep });
            }
        }
    }

    /// Block latching between two vessels until they are 3 m apart.
    pub fn block_relatch(&mut self, a: Entity, b: Entity) {
        for (x, y) in [(a, b), (b, a)] {
            let mut u = self.world.get::<Undocked>(x).cloned().unwrap_or_default();
            u.from.push(y);
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

    /// Join every pair of vessels whose ports latch now: the second becomes part of the first (its crew,
    /// tanks and reports go along). Returns (kept, absorbed) pairs; the absorbed vessels are gone.
    pub fn dock_latched(
        &mut self,
        defs: &[PartDef],
        resources: &[ose_data::ResourceDef],
        spec_for: &dyn Fn(&Craft) -> flight::FlightSpec,
    ) -> Vec<(Entity, Entity)> {
        let latches = self.find_latches(defs);
        let mut used = Vec::new();
        let mut out = Vec::new();
        for l in latches {
            if used.contains(&l.a) || used.contains(&l.b) {
                continue;
            }
            let (Some(ca), Some(cb)) = (self.current_craft(l.a), self.current_craft(l.b)) else {
                continue;
            };
            let (Some(pa), Some(pb)) = (self.powered(l.a).cloned(), self.powered(l.b).cloned())
            else {
                continue;
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
            let r = (pa.r * pa.mass_t + pb.r * pb.mass_t) * (1.0 / m);
            let v = (pa.v * pa.mass_t + pb.v * pb.mass_t) * (1.0 / m);
            let mut science = self.science_carried(l.a);
            science.extend(self.science_carried(l.b));
            self.rebuild_vessel(
                l.a, &merged, r, v, pa.facing, crew, defs, resources, spec_for,
            );
            self.set_docked(l.a, links);
            self.set_science_carried(l.a, science);
            self.despawn_vessel(l.b);
            used.push(l.a);
            used.push(l.b);
            out.push((l.a, l.b));
        }
        self.refresh_undock_blocks();
        out
    }

    /// Undock at one of a vessel's docked joints (the last one docked when `port` is `None`, else the joint
    /// with that port part on either side): what docked comes away as a new vessel, pushed off by the port's
    /// ejection force for a second. Returns the new vessel.
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
        let (_, pb) = links[link];
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
        let ejection = dock.map(|k| k.undock_ejection_kn).unwrap_or(0.4);
        let m_go = go.mass(defs, resources).wet_t().max(0.01);
        let m_stay = stay.mass(defs, resources).wet_t().max(0.01);
        let body = self.body_of(e).ok_or("no body")?;
        let name = format!("{} (undocked)", go.name);
        let r_go = p.r + dir * 2.0;
        let v_go = p.v + dir * (ejection / m_go);
        let ne = self.spawn_vessel(&name, body, r_go, v_go);
        self.rebuild_vessel(
            e,
            &stay,
            p.r,
            p.v - dir * (ejection / m_stay),
            p.facing,
            crew_of(&map_stay),
            defs,
            resources,
            spec_for,
        );
        self.set_docked(e, links_of(&map_stay));
        self.rebuild_vessel(
            ne,
            &go,
            r_go,
            v_go,
            p.facing * -1.0,
            crew_of(&map_go),
            defs,
            resources,
            spec_for,
        );
        self.set_docked(ne, links_of(&map_go));
        self.block_relatch(e, ne);
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

    #[test]
    fn two_ships_drifting_port_to_port_dock_and_undock() {
        let d = defs();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
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
        // ship A points +y (along its orbit), ship B ahead of it pointing back at it
        let a_craft = ship("A");
        let b_craft = ship("B");
        let place = |u: &mut Universe, c: &Craft, r: DVec3, facing: DVec3| {
            let e = u.spawn_vessel("ship", body, r, v0);
            u.rebuild_vessel(e, c, r, v0, facing, vec![], &d, &res, &spec);
            e
        };
        let a = place(&mut u, &a_craft, r0, DVec3::new(0.0, 1.0, 0.0));
        let pa = free_ports(&a_craft, &d)[0].clone();
        let pb = free_ports(&b_craft, &d)[0].clone();
        // B's port 0.5 m beyond A's port
        let wa = to_inertial(&u, a, pa.position, false).unwrap();
        let b = place(&mut u, &b_craft, r0, DVec3::new(0.0, -1.0, 0.0));
        let wb = to_inertial(&u, b, pb.position, false).unwrap();
        let mut p = u.powered(b).unwrap().clone();
        p.r = p.r + (wa - wb) + DVec3::new(0.0, 0.5, 0.0);
        u.set_powered(b, p);
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

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

impl Universe {
    pub fn set_craft(&mut self, e: Entity, craft: Craft) {
        self.world.entity_mut(e).insert(CraftDoc(craft));
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

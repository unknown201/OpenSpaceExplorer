//! Vehicle Assembly Building: the editor model. A `Craft` part tree built by attaching catalog parts to a
//! selected part on matching attach nodes (stack top/bottom first, surface otherwise), subtree removal,
//! staging, and full tanks on placement. Pure data, unit-tested natively; `lib.rs` owns the screen.

use ose_data::craft::{Craft, CraftPart, Quat, StoredResource};
use ose_math::DVec3;
use std::collections::HashMap;

/// One part of GET /parts.json as the editor needs it.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct PartSpec {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub size_category: String,
    #[serde(default)]
    pub mass_t: f64,
    #[serde(default)]
    pub crew_capacity: u32,
    #[serde(default)]
    pub stage_type: String,
    #[serde(default)]
    pub size_m: Option<DVec3>,
    #[serde(default)]
    pub center_m: Option<DVec3>,
    #[serde(default)]
    pub attach_nodes: Vec<NodeSpec>,
    #[serde(default)]
    pub has_engine: bool,
    #[serde(default)]
    pub is_decoupler: bool,
    /// `(resource, capacity_units)`; filled to capacity when the part is placed.
    #[serde(default)]
    pub resource_containers: Vec<ContainerSpec>,
    /// Engine modes (thrust and specific impulse), for the part tooltip.
    #[serde(default)]
    pub engines: Vec<EngineSpec>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct EngineSpec {
    #[serde(default)]
    pub max_thrust_kn: f64,
    #[serde(default)]
    pub isp_vacuum_s: f64,
    #[serde(default)]
    pub isp_sea_level_s: f64,
    #[serde(default)]
    pub propellant_mixture: String,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct NodeSpec {
    pub id: String,
    #[serde(default)]
    pub node_type: String,
    #[serde(default)]
    pub position: DVec3,
    #[serde(default)]
    pub orientation: DVec3,
    #[serde(default)]
    pub size: i32,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ContainerSpec {
    pub resource: String,
    #[serde(default)]
    pub capacity_units: f64,
}

impl PartSpec {
    pub fn is_stack_node(n: &NodeSpec) -> bool {
        n.node_type.eq_ignore_ascii_case("stack")
    }
    pub fn size(&self) -> DVec3 {
        self.size_m.unwrap_or(DVec3::new(
            crate::thumb::DEFAULT_SIZE_M,
            crate::thumb::DEFAULT_SIZE_M,
            crate::thumb::DEFAULT_SIZE_M,
        ))
    }
    pub fn centre(&self) -> DVec3 {
        self.center_m.unwrap_or(DVec3::ZERO)
    }
    /// Parts that take part in staging (engines, decouplers, parachutes).
    pub fn is_stageable(&self) -> bool {
        !(self.stage_type.is_empty() || self.stage_type == "None")
    }
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() || self.title.starts_with('#') {
            &self.id
        } else {
            &self.title
        }
    }
}

pub type Catalog = HashMap<String, PartSpec>;

const IDENTITY: Quat = Quat {
    x: 0.0,
    y: 0.0,
    z: 0.0,
    w: 1.0,
};

/// Rotate `v` by the unit quaternion `q`.
fn quat_rotate(q: Quat, v: DVec3) -> DVec3 {
    let u = DVec3::new(q.x, q.y, q.z);
    let t = u.cross(v) * 2.0;
    v + t * q.w + u.cross(t)
}

/// The craft under construction plus the selection.
#[derive(Clone, Debug)]
pub struct Editor {
    pub craft: Craft,
    pub selected: Option<usize>,
    /// A new craft starts as a plane (its first part lying along the floor).
    pub plane_new: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new("Untitled craft")
    }
}

impl Editor {
    pub fn new(name: &str) -> Editor {
        Editor {
            craft: Craft {
                name: name.into(),
                description: String::new(),
                parts: Vec::new(),
                stages: Vec::new(),
            },
            selected: None,
            plane_new: false,
        }
    }

    pub fn from_craft(craft: Craft) -> Editor {
        let selected = if craft.parts.is_empty() {
            None
        } else {
            Some(0)
        };
        let mut e = Editor {
            craft,
            selected,
            plane_new: false,
        };
        e.plane_new = e.is_plane();
        e
    }

    pub fn is_empty(&self) -> bool {
        self.craft.parts.is_empty()
    }

    /// Attach node ids already used on part `i`: the one linking to its parent plus those its children hang on.
    fn used_nodes(&self, i: usize) -> Vec<String> {
        let mut used: Vec<String> = Vec::new();
        if let Some(own) = &self.craft.parts[i].own_node {
            used.push(own.clone());
        }
        for p in &self.craft.parts {
            if p.parent == Some(i) {
                if let Some(n) = &p.parent_node {
                    used.push(n.clone());
                }
            }
        }
        used
    }

    /// Add `spec` to the craft. The first part becomes the root; later parts attach to the selected part
    /// (or the last part) on the first free stack node pair, else on the surface. Returns the new index.
    pub fn add_part(&mut self, spec: &PartSpec, catalog: &Catalog) -> Result<usize, String> {
        self.add_part_with(spec, catalog, None)
    }

    /// Add `spec` on part `target`, preferring the parent node `node` (a drop on that node in the 3D VAB).
    pub fn add_part_on(
        &mut self,
        spec: &PartSpec,
        catalog: &Catalog,
        target: usize,
        node: &str,
    ) -> Result<usize, String> {
        if target < self.craft.parts.len() {
            self.selected = Some(target);
        }
        self.add_part_with(spec, catalog, Some(node))
    }

    /// Free stack attach nodes of the craft: (part index, node id, craft-frame position, outward direction),
    /// in the craft's stored (Unity) frame. The drop targets of the 3D VAB.
    pub fn free_nodes(&self, catalog: &Catalog) -> Vec<(usize, String, DVec3, DVec3)> {
        let mut out = Vec::new();
        for (i, p) in self.craft.parts.iter().enumerate() {
            let Some(spec) = catalog.get(&p.part_id) else {
                continue;
            };
            let used = self.used_nodes(i);
            for n in &spec.attach_nodes {
                if !PartSpec::is_stack_node(n) || used.contains(&n.id) {
                    continue;
                }
                let q = p.local_rotation;
                out.push((
                    i,
                    n.id.clone(),
                    p.local_position + quat_rotate(q, n.position),
                    quat_rotate(q, n.orientation),
                ));
            }
        }
        out
    }

    fn add_part_with(
        &mut self,
        spec: &PartSpec,
        catalog: &Catalog,
        prefer: Option<&str>,
    ) -> Result<usize, String> {
        let resources = spec
            .resource_containers
            .iter()
            .map(|c| StoredResource {
                resource: c.resource.clone(),
                stored_units: c.capacity_units,
                capacity_units: c.capacity_units,
            })
            .collect();
        let stage_index = if spec.is_stageable() {
            self.next_stage_index()
        } else {
            0
        };
        if self.craft.parts.is_empty() {
            self.craft.parts.push(CraftPart {
                guid: format!("p{}", 1),
                part_id: spec.id.clone(),
                parent: None,
                own_node: None,
                parent_node: None,
                local_position: DVec3::ZERO,
                local_rotation: if self.plane_new {
                    axis_turn(0, PLANE_TURN_DEG)
                } else {
                    IDENTITY
                },
                stage_index,
                fuel_crossfeed: true,
                resources,
                fuel_line_target: None,
            });
            self.selected = Some(0);
            self.rebuild_stages(catalog);
            return Ok(0);
        }
        let target = self.selected.unwrap_or(self.craft.parts.len() - 1);
        let target_spec = catalog
            .get(&self.craft.parts[target].part_id)
            .ok_or_else(|| format!("unknown part {}", self.craft.parts[target].part_id))?;
        let used = self.used_nodes(target);
        let free_stack: Vec<&NodeSpec> = target_spec
            .attach_nodes
            .iter()
            .filter(|n| PartSpec::is_stack_node(n) && !used.contains(&n.id))
            .collect();
        let new_stack: Vec<&NodeSpec> = spec
            .attach_nodes
            .iter()
            .filter(|n| PartSpec::is_stack_node(n))
            .collect();
        // Prefer the requested node, then the parent's bottom node (building downwards), then its top, then
        // anything.
        let mut pairs: Vec<(&NodeSpec, &NodeSpec)> = Vec::new();
        for pref in prefer.into_iter().chain(["bottom", "top"]) {
            for tn in free_stack.iter().filter(|n| n.id == pref) {
                if let Some(nn) = new_stack
                    .iter()
                    .find(|n| n.orientation.dot(tn.orientation) < -0.5)
                {
                    pairs.push((tn, nn));
                }
            }
        }
        if pairs.is_empty() {
            for tn in &free_stack {
                if let Some(nn) = new_stack
                    .iter()
                    .find(|n| n.orientation.dot(tn.orientation) < -0.5)
                {
                    pairs.push((tn, nn));
                }
            }
        }
        let (own_node, parent_node, local_position) = if let Some((tn, nn)) = pairs.first() {
            (
                Some(nn.id.clone()),
                Some(tn.id.clone()),
                tn.position - nn.position,
            )
        } else {
            // Surface attach on the parent's +x side, centred on the parent.
            let surface = spec
                .attach_nodes
                .iter()
                .find(|n| !PartSpec::is_stack_node(n))
                .map(|n| n.id.clone())
                .unwrap_or_else(|| "srfAttach".into());
            let dx = target_spec.centre().x + target_spec.size().x * 0.5 - spec.centre().x
                + spec.size().x * 0.5;
            (
                Some(surface),
                None,
                DVec3::new(dx, target_spec.centre().y - spec.centre().y, 0.0),
            )
        };
        // Positions are stored relative to the craft root, as KSP2 assemblies (and the stock crafts) store
        // `partState.localPosition`; the new part takes its parent's rotation (a plane's parts lie along it, a
        // part under a radial decoupler hangs off its side), its node offset turned with it.
        let pq = unit_or_identity(self.craft.parts[target].local_rotation);
        let local_position =
            self.craft.parts[target].local_position + quat_rotate(pq, local_position);
        let idx = self.craft.parts.len();
        self.craft.parts.push(CraftPart {
            guid: format!("p{}", idx + 1),
            part_id: spec.id.clone(),
            parent: Some(target),
            own_node,
            parent_node,
            local_position,
            local_rotation: pq,
            stage_index,
            fuel_crossfeed: true,
            resources,
            fuel_line_target: None,
        });
        self.selected = Some(idx);
        self.rebuild_stages(catalog);
        Ok(idx)
    }

    /// Stage for a newly placed stageable part: after the current last stage.
    fn next_stage_index(&self) -> i32 {
        self.craft
            .parts
            .iter()
            .filter(|p| p.stage_index > 0)
            .map(|p| p.stage_index)
            .max()
            .map(|m| m + 1)
            .unwrap_or(1)
    }

    /// Remove the selected part and everything attached to it; the selection moves to its parent.
    pub fn remove_selected(&mut self, catalog: &Catalog) -> Option<usize> {
        let sel = self.selected?;
        let mut doomed = vec![false; self.craft.parts.len()];
        doomed[sel] = true;
        // Children may come after or before their parent; iterate to a fixed point.
        loop {
            let mut changed = false;
            for (i, p) in self.craft.parts.iter().enumerate() {
                if !doomed[i] && p.parent.is_some_and(|pi| doomed[pi]) {
                    doomed[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let parent = self.craft.parts[sel].parent;
        let mut remap: Vec<Option<usize>> = Vec::with_capacity(self.craft.parts.len());
        let mut kept = Vec::new();
        for (i, p) in self.craft.parts.iter().enumerate() {
            if doomed[i] {
                remap.push(None);
            } else {
                remap.push(Some(kept.len()));
                kept.push(p.clone());
            }
        }
        for p in &mut kept {
            p.parent = p.parent.and_then(|pi| remap[pi]);
        }
        let removed = doomed.iter().filter(|d| **d).count();
        self.craft.parts = kept;
        self.selected = parent
            .and_then(|pi| remap[pi])
            .or(if self.craft.parts.is_empty() {
                None
            } else {
                Some(0)
            });
        self.rebuild_stages(catalog);
        Some(removed)
    }

    /// Move the selected stageable part to a higher stage (`+1`, fires earlier: the highest stage fires
    /// first, as `ose_data::staging` runs them) or a lower one (`-1`, later).
    pub fn shift_stage(&mut self, delta: i32, catalog: &Catalog) {
        let Some(sel) = self.selected else {
            return;
        };
        let stageable = catalog
            .get(&self.craft.parts[sel].part_id)
            .is_some_and(|s| s.is_stageable());
        if !stageable {
            return;
        }
        let p = &mut self.craft.parts[sel];
        p.stage_index = (p.stage_index + delta).max(1);
        self.rebuild_stages(catalog);
    }

    /// Put stageable part `i` into the stage at `stages` index `stage` (the drop of a part on a stage card).
    pub fn move_to_stage(&mut self, i: usize, stage: usize, catalog: &Catalog) {
        let stageable = self
            .craft
            .parts
            .get(i)
            .and_then(|p| catalog.get(&p.part_id))
            .is_some_and(|s| s.is_stageable());
        if !stageable {
            return;
        }
        // After rebuild_stages the stage at index n has stage_index n + 1.
        self.craft.parts[i].stage_index = stage as i32 + 1;
        self.rebuild_stages(catalog);
    }

    /// `Craft::stages`: stageable parts grouped by `stage_index`, lowest first, renumbered densely. The last
    /// stage (highest index) fires first.
    pub fn rebuild_stages(&mut self, catalog: &Catalog) {
        let mut by_stage: Vec<(i32, usize)> = self
            .craft
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| catalog.get(&p.part_id).is_some_and(|s| s.is_stageable()))
            .map(|(i, p)| (p.stage_index, i))
            .collect();
        by_stage.sort();
        let mut stages: Vec<Vec<usize>> = Vec::new();
        let mut last = None;
        for (s, i) in by_stage {
            if last != Some(s) {
                stages.push(Vec::new());
                last = Some(s);
            }
            stages.last_mut().unwrap().push(i);
        }
        // Dense renumbering so the indices are 1..=n.
        for (n, stage) in stages.iter().enumerate() {
            for &i in stage {
                self.craft.parts[i].stage_index = n as i32 + 1;
            }
        }
        self.craft.stages = stages;
    }

    /// Dry mass from the catalog, tonnes; unknown parts count as zero.
    pub fn dry_mass_t(&self, catalog: &Catalog) -> f64 {
        self.craft
            .parts
            .iter()
            .map(|p| catalog.get(&p.part_id).map(|s| s.mass_t).unwrap_or(0.0))
            .sum()
    }
}

/// A part and everything attached below it, lifted off the craft (the original's grab in the VAB). `parts[0]` is
/// the fragment's root; parents index into `parts`; positions are relative to the root.
#[derive(Clone, Debug)]
pub struct Fragment {
    pub parts: Vec<CraftPart>,
}

impl Fragment {
    pub fn root_id(&self) -> &str {
        &self.parts[0].part_id
    }
}

/// Undo and redo for the editor (`ObjectyAssemblyBuilderHistorian`): whole-craft snapshots, newest last.
#[derive(Default)]
pub struct History {
    undo: Vec<(Craft, Option<usize>)>,
    redo: Vec<(Craft, Option<usize>)>,
}

impl History {
    const DEPTH: usize = 100;

    /// Remember the editor as it is before a change; a new change forgets the redo list.
    pub fn record(&mut self, e: &Editor) {
        self.undo.push((e.craft.clone(), e.selected));
        if self.undo.len() > Self::DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, e: &mut Editor) -> bool {
        let Some((craft, sel)) = self.undo.pop() else {
            return false;
        };
        let now = std::mem::replace(&mut e.craft, craft);
        self.redo.push((now, e.selected));
        e.selected = sel;
        true
    }

    pub fn redo(&mut self, e: &mut Editor) -> bool {
        let Some((craft, sel)) = self.redo.pop() else {
            return false;
        };
        let now = std::mem::replace(&mut e.craft, craft);
        self.undo.push((now, e.selected));
        e.selected = sel;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

impl Editor {
    /// Parts in the subtree under `i` (itself included), as a mask.
    fn subtree(&self, i: usize) -> Vec<bool> {
        let mut inside = vec![false; self.craft.parts.len()];
        inside[i] = true;
        loop {
            let mut changed = false;
            for (k, p) in self.craft.parts.iter().enumerate() {
                if !inside[k] && p.parent.is_some_and(|pi| inside[pi]) {
                    inside[k] = true;
                    changed = true;
                }
            }
            if !changed {
                return inside;
            }
        }
    }

    /// The offset tool: move part `i` and everything under it by `delta` (craft frame, metres). The root
    /// stays put (moving it would move the whole craft).
    pub fn offset(&mut self, i: usize, delta: DVec3) -> bool {
        if self.craft.parts.get(i).is_none_or(|p| p.parent.is_none()) {
            return false;
        }
        let inside = self.subtree(i);
        for (k, p) in self.craft.parts.iter_mut().enumerate() {
            if inside[k] {
                p.local_position = p.local_position + delta;
            }
        }
        true
    }

    /// Lift part `i` and everything under it off the craft. Grabbing the root lifts the whole craft.
    pub fn detach(&mut self, i: usize, catalog: &Catalog) -> Option<Fragment> {
        if i >= self.craft.parts.len() {
            return None;
        }
        let inside = self.subtree(i);
        let origin = self.craft.parts[i].local_position;
        // Fragment order: the grabbed part first, then the rest in craft order.
        let order: Vec<usize> = std::iter::once(i)
            .chain((0..self.craft.parts.len()).filter(|&k| inside[k] && k != i))
            .collect();
        let mut to_frag = vec![None; self.craft.parts.len()];
        for (n, &k) in order.iter().enumerate() {
            to_frag[k] = Some(n);
        }
        let parts = order
            .iter()
            .map(|&k| {
                let mut p = self.craft.parts[k].clone();
                if k == i {
                    p.parent = None;
                    p.parent_node = None;
                } else {
                    p.parent = p.parent.and_then(|pi| to_frag[pi]);
                }
                p.local_position = p.local_position - origin;
                p.fuel_line_target = p.fuel_line_target.and_then(|t| to_frag[t]);
                p
            })
            .collect();
        let mut remap = vec![None; self.craft.parts.len()];
        let mut kept = Vec::new();
        for (k, p) in self.craft.parts.iter().enumerate() {
            if !inside[k] {
                remap[k] = Some(kept.len());
                kept.push(p.clone());
            }
        }
        for p in &mut kept {
            p.parent = p.parent.and_then(|pi| remap[pi]);
            p.fuel_line_target = p.fuel_line_target.and_then(|t| remap[t]);
        }
        let parent = self.craft.parts[i].parent.and_then(|pi| remap[pi]);
        self.craft.parts = kept;
        self.selected = parent;
        self.rebuild_stages(catalog);
        Some(Fragment { parts })
    }

    /// Turn a stack-attached part by `q` about its own attach node, so it stays on the node it hangs from.
    pub fn turn_on_own_node(&mut self, i: usize, q: Quat, catalog: &Catalog) {
        let Some(p) = self.craft.parts.get(i) else {
            return;
        };
        let own = catalog.get(&p.part_id).and_then(|s| {
            s.attach_nodes
                .iter()
                .find(|n| Some(&n.id) == p.own_node.as_ref())
                .map(|n| n.position)
        });
        let n = own.unwrap_or_default();
        let p = &mut self.craft.parts[i];
        p.local_position = p.local_position + n - quat_rotate(q, n);
        p.local_rotation = q;
    }

    /// A copy of part `i` and everything under it, the craft left as it is (Ctrl+C).
    pub fn copy(&self, i: usize, catalog: &Catalog) -> Option<Fragment> {
        self.clone().detach(i, catalog)
    }

    fn next_guid(&self) -> usize {
        self.craft
            .parts
            .iter()
            .filter_map(|p| {
                p.guid
                    .strip_prefix('p')
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Put a lifted fragment back: on `target`'s node (as a picker drop), or as the whole craft when the craft
    /// is empty. Keeps the fragment's stages, resources and shape. Returns the new root's index.
    pub fn attach(
        &mut self,
        frag: &Fragment,
        catalog: &Catalog,
        target: Option<(usize, &str)>,
    ) -> Result<usize, String> {
        let spec = catalog
            .get(frag.root_id())
            .ok_or_else(|| format!("unknown part {}", frag.root_id()))?;
        let root = match target {
            Some((t, node)) => self.add_part_on(spec, catalog, t, node)?,
            None if self.is_empty() => self.add_part(spec, catalog)?,
            None => return Err("drop the part on a green attach point".into()),
        };
        // The placed root keeps the fragment's state; only the link to its new parent is new.
        let placed = self.craft.parts[root].clone();
        let mut guid = self.next_guid();
        let mut r = frag.parts[0].clone();
        r.parent = placed.parent;
        r.own_node = placed.own_node;
        r.parent_node = placed.parent_node;
        r.local_position = placed.local_position;
        r.guid = format!("p{guid}");
        guid += 1;
        self.craft.parts[root] = r;
        let base = self.craft.parts.len();
        let origin = self.craft.parts[root].local_position;
        // Fragment index n >= 1 lands at base + n - 1; the fragment root is `root`.
        let at = |n: usize| if n == 0 { root } else { base + n - 1 };
        for p in &frag.parts[1..] {
            let mut p = p.clone();
            p.parent = p.parent.map(at);
            p.fuel_line_target = p.fuel_line_target.map(at);
            p.local_position = p.local_position + origin;
            p.guid = format!("p{guid}");
            guid += 1;
            self.craft.parts.push(p);
        }
        self.selected = Some(root);
        self.rebuild_stages(catalog);
        Ok(root)
    }
}

/// A spot on a part's side for surface attachment: the part, the angle around its vertical axis (radians,
/// 0 = +X, towards +Z) and the height in the craft frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceSpot {
    pub target: usize,
    pub azimuth: f64,
    pub height: f64,
    /// The held part's own turn (W/A/S/D/Q/E in the original), applied after it is turned to face the side.
    pub turn: Quat,
}

/// No rotation.
pub const NO_TURN: Quat = IDENTITY;

/// A turn of `deg` degrees about craft axis 0 (X, pitch), 1 (Y, yaw) or 2 (Z, roll).
pub fn axis_turn(axis: usize, deg: f64) -> Quat {
    let h = deg.to_radians() * 0.5;
    let (s, c) = (h.sin(), h.cos());
    match axis {
        0 => Quat {
            x: s,
            y: 0.0,
            z: 0.0,
            w: c,
        },
        1 => Quat {
            x: 0.0,
            y: s,
            z: 0.0,
            w: c,
        },
        _ => Quat {
            x: 0.0,
            y: 0.0,
            z: s,
            w: c,
        },
    }
}

/// `a` then... in quaternion terms `a * b` (apply `b` first, then `a`).
pub fn turn_then(a: Quat, b: Quat) -> Quat {
    quat_mul(a, b)
}

/// `q`, or no rotation for a zero (unset) quaternion.
fn unit_or_identity(q: Quat) -> Quat {
    let n = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if n < 1e-9 {
        IDENTITY
    } else {
        Quat {
            x: q.x / n,
            y: q.y / n,
            z: q.z / n,
            w: q.w / n,
        }
    }
}

/// `v` turned by `q` (an unset quaternion turns nothing).
pub fn rotate_by(q: Quat, v: DVec3) -> DVec3 {
    quat_rotate(unit_or_identity(q), v)
}

/// The turn that lays a rocket-built craft down as a plane: its +Y (the nose) to +Z, along the hangar floor.
pub const PLANE_TURN_DEG: f64 = 90.0;

impl Editor {
    /// Whether the craft is laid out as a plane (its root's +Y along the floor), as the server tells planes.
    pub fn is_plane(&self) -> bool {
        self.craft
            .parts
            .iter()
            .find(|p| p.parent.is_none())
            .map(|p| {
                quat_rotate(
                    unit_or_identity(p.local_rotation),
                    DVec3::new(0.0, 1.0, 0.0),
                )
                .y
                .abs()
                    < 0.5
            })
            .unwrap_or(false)
    }

    /// Turn the whole craft about its root between standing (a rocket) and lying along the floor (a plane),
    /// the original's VAB orientation switch.
    pub fn set_plane(&mut self, plane: bool) {
        if plane == self.is_plane() || self.craft.parts.is_empty() {
            self.plane_new = plane;
            return;
        }
        self.plane_new = plane;
        let r = axis_turn(
            0,
            if plane {
                PLANE_TURN_DEG
            } else {
                -PLANE_TURN_DEG
            },
        );
        let origin = self
            .craft
            .parts
            .iter()
            .find(|p| p.parent.is_none())
            .map(|p| p.local_position)
            .unwrap_or_default();
        for p in &mut self.craft.parts {
            p.local_position = origin + quat_rotate(r, p.local_position - origin);
            p.local_rotation = quat_mul(r, unit_or_identity(p.local_rotation));
        }
    }
}

/// Symmetry counts the original offers (`BuilderSymmetryMode` X1..X8), in X-key order.
pub const SYMMETRY: [u32; 6] = [1, 2, 3, 4, 6, 8];

/// The part's surface attach node, if it can go on another part's side.
pub fn surface_node(spec: &PartSpec) -> Option<&NodeSpec> {
    spec.attach_nodes
        .iter()
        .find(|n| !PartSpec::is_stack_node(n))
}

fn quat_mul(a: Quat, b: Quat) -> Quat {
    Quat {
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
    }
}

fn quat_conj(q: Quat) -> Quat {
    Quat {
        x: -q.x,
        y: -q.y,
        z: -q.z,
        w: q.w,
    }
}

/// The mirror image of a pose across the plane through `centre` with unit `normal`: the position reflected,
/// and the rotation that reflects every part axis but `own` (a unit axis of the part's frame across which the
/// part looks the same); a rotation cannot flip handedness, so that one turns the other way. As reflections
/// are a half turn and an inversion, that is a half turn about the normal after the part's own rotation and a
/// half turn about `own` before it.
fn mirror_pose(pos: DVec3, q: Quat, centre: DVec3, normal: DVec3, own: DVec3) -> (DVec3, Quat) {
    let d = (pos - centre).dot(normal);
    let half_n = Quat {
        x: normal.x,
        y: normal.y,
        z: normal.z,
        w: 0.0,
    };
    let half_own = Quat {
        x: own.x,
        y: own.y,
        z: own.z,
        w: 0.0,
    };
    (
        pos - normal * (2.0 * d),
        quat_mul(quat_mul(half_n, q), half_own),
    )
}

/// Rotation about the craft's vertical axis that turns a part's +X (where surface nodes face, toward what
/// they hang on) to point inward from the outward direction at `azimuth`.
fn facing_in(azimuth: f64) -> Quat {
    let phi = std::f64::consts::PI - azimuth;
    Quat {
        x: 0.0,
        y: (phi * 0.5).sin(),
        z: 0.0,
        w: (phi * 0.5).cos(),
    }
}

impl Editor {
    /// Where a surface attachment at craft-frame point `hit` on part `target` goes: the angle around the
    /// part's axis (snapped to `snap_deg` steps when given) and the height of the hit.
    pub fn surface_spot(
        &self,
        target: usize,
        hit: DVec3,
        catalog: &Catalog,
        snap_deg: Option<f64>,
    ) -> Option<SurfaceSpot> {
        let p = self.craft.parts.get(target)?;
        let spec = catalog.get(&p.part_id)?;
        // In the part's own frame, where its long axis is +Y (a plane's fuselage lies along the floor).
        let pq = unit_or_identity(p.local_rotation);
        let h = quat_rotate(quat_conj(pq), hit - p.local_position);
        let axis = spec.centre();
        let mut azimuth = (h.z - axis.z).atan2(h.x - axis.x);
        if let Some(step) = snap_deg.filter(|s| *s > 0.0) {
            let s = step.to_radians();
            azimuth = (azimuth / s).round() * s;
        }
        Some(SurfaceSpot {
            target,
            azimuth,
            height: h.y,
            turn: IDENTITY,
        })
    }

    /// Poses (craft frame) of `spec` placed at `spot` and its symmetry copies around the target's axis.
    pub fn surface_poses(
        &self,
        spec: &PartSpec,
        catalog: &Catalog,
        spot: SurfaceSpot,
        symmetry: u32,
    ) -> Vec<(DVec3, Quat)> {
        let Some(p) = self.craft.parts.get(spot.target) else {
            return Vec::new();
        };
        let Some(tspec) = catalog.get(&p.part_id) else {
            return Vec::new();
        };
        let node = surface_node(spec).map(|n| n.position).unwrap_or_default();
        // A node that faces along the part's +Z (wings, which span away from it) is first turned to face +X,
        // the way the placement below turns toward the part hung on.
        let o = surface_node(spec)
            .map(|n| n.orientation)
            .unwrap_or_default();
        let align = if o.y.abs() < 0.9 && (o.x.abs() + o.z.abs()) > 1e-9 {
            let th = o.z.atan2(o.x);
            Quat {
                x: 0.0,
                y: (th * 0.5).sin(),
                z: 0.0,
                w: (th * 0.5).cos(),
            }
        } else {
            IDENTITY
        };
        // Worked out in the target's own frame (`spot.height` is along its +Y), then turned into the craft's.
        let pq = unit_or_identity(p.local_rotation);
        let axis = tspec.centre();
        let radius = tspec.size().x.min(tspec.size().z) * 0.5;
        // A plane's pair is a mirror image across its vertical middle (the original's mirror symmetry), not a
        // copy turned half way round the part it hangs on.
        if symmetry == 2 && self.is_plane() {
            let one = self.surface_poses(spec, catalog, spot, 1);
            let Some(&(pos, q)) = one.first() else {
                return one;
            };
            let (centre, normal) = self.mirror_plane();
            // The part's own face it is alike on both sides of: a wing (its node faces along +Z, the way it
            // spans) across its thickness (+Y); a booster or decoupler (node along +X, long along +Y) across +Z.
            let node_dir = surface_node(spec)
                .map(|n| n.orientation)
                .unwrap_or_default();
            let own = if node_dir.z.abs() > node_dir.x.abs().max(node_dir.y.abs()) {
                DVec3::new(0.0, 1.0, 0.0)
            } else {
                DVec3::new(0.0, 0.0, 1.0)
            };
            return vec![(pos, q), mirror_pose(pos, q, centre, normal, own)];
        }
        (0..symmetry.max(1))
            .map(|k| {
                let az = spot.azimuth + std::f64::consts::TAU * k as f64 / symmetry.max(1) as f64;
                let out = DVec3::new(az.cos(), 0.0, az.sin());
                let contact = DVec3::new(axis.x, spot.height, axis.z) + out * radius;
                let q = quat_mul(quat_mul(facing_in(az), spot.turn), align);
                let local = contact - quat_rotate(q, node);
                (p.local_position + quat_rotate(pq, local), quat_mul(pq, q))
            })
            .collect()
    }

    /// A plane's middle: a point on it (the root part) and its normal, across both the nose and the craft's up.
    fn mirror_plane(&self) -> (DVec3, DVec3) {
        let root = self.craft.parts.iter().find(|p| p.parent.is_none());
        let centre = root.map(|p| p.local_position).unwrap_or_default();
        let nose = root
            .map(|p| {
                quat_rotate(
                    unit_or_identity(p.local_rotation),
                    DVec3::new(0.0, 1.0, 0.0),
                )
            })
            .unwrap_or(DVec3::new(0.0, 0.0, 1.0));
        let n = DVec3::new(0.0, 1.0, 0.0).cross(nose);
        let n = if n.length() > 1e-9 {
            n * (1.0 / n.length())
        } else {
            DVec3::new(1.0, 0.0, 0.0)
        };
        (centre, n)
    }

    /// Put `spec` on the side of `spot.target`, with `symmetry` copies around it sharing one stage. Returns
    /// the new parts' indices.
    pub fn add_surface(
        &mut self,
        spec: &PartSpec,
        catalog: &Catalog,
        spot: SurfaceSpot,
        symmetry: u32,
    ) -> Result<Vec<usize>, String> {
        let node = surface_node(spec)
            .ok_or_else(|| format!("{} cannot go on another part's side", spec.display_title()))?
            .id
            .clone();
        let poses = self.surface_poses(spec, catalog, spot, symmetry);
        if poses.is_empty() {
            return Err("no part to attach to there".into());
        }
        let stage_index = if spec.is_stageable() {
            self.next_stage_index()
        } else {
            0
        };
        let mut out = Vec::new();
        for (guid, (pos, q)) in (self.next_guid()..).zip(poses) {
            let resources = spec
                .resource_containers
                .iter()
                .map(|c| StoredResource {
                    resource: c.resource.clone(),
                    stored_units: c.capacity_units,
                    capacity_units: c.capacity_units,
                })
                .collect();
            out.push(self.craft.parts.len());
            self.craft.parts.push(CraftPart {
                guid: format!("p{guid}"),
                part_id: spec.id.clone(),
                parent: Some(spot.target),
                own_node: Some(node.clone()),
                parent_node: None,
                local_position: pos,
                local_rotation: q,
                stage_index,
                fuel_crossfeed: true,
                resources,
                fuel_line_target: None,
            });
        }
        self.selected = out.first().copied();
        self.rebuild_stages(catalog);
        Ok(out)
    }

    /// Put a lifted fragment on the side of `spot.target` (no symmetry): the fragment turns with its root.
    pub fn attach_surface(
        &mut self,
        frag: &Fragment,
        catalog: &Catalog,
        spot: SurfaceSpot,
    ) -> Result<usize, String> {
        let spec = catalog
            .get(frag.root_id())
            .ok_or_else(|| format!("unknown part {}", frag.root_id()))?;
        let root = *self
            .add_surface(spec, catalog, spot, 1)?
            .first()
            .ok_or("nothing placed")?;
        let placed = self.craft.parts[root].clone();
        let turn = quat_mul(
            placed.local_rotation,
            quat_conj(frag.parts[0].local_rotation),
        );
        let mut r = frag.parts[0].clone();
        r.parent = placed.parent;
        r.own_node = placed.own_node;
        r.parent_node = None;
        r.local_position = placed.local_position;
        r.local_rotation = placed.local_rotation;
        r.guid = placed.guid;
        self.craft.parts[root] = r;
        let base = self.craft.parts.len();
        let at = |n: usize| if n == 0 { root } else { base + n - 1 };
        for (guid, p) in (self.next_guid()..).zip(&frag.parts[1..]) {
            let mut p = p.clone();
            p.parent = p.parent.map(at);
            p.fuel_line_target = p.fuel_line_target.map(at);
            p.local_position = placed.local_position + quat_rotate(turn, p.local_position);
            p.local_rotation = quat_mul(turn, p.local_rotation);
            p.guid = format!("p{guid}");
            self.craft.parts.push(p);
        }
        self.selected = Some(root);
        self.rebuild_stages(catalog);
        Ok(root)
    }
}

/// One finding of the engineer's report (`ObjectAssemblyEngineerReport`): severity 2 stops a launch going
/// well, 1 is worth a look, 0 is fine.
#[derive(Clone, Debug, PartialEq)]
pub struct Flaw {
    pub severity: u8,
    pub text: String,
}

/// The original's flaw checkers that the editor can run with what it knows: a command part, engines and the
/// propellant they need, thrust to lift off, a parachute for crew, an antenna for probes, charge storage.
/// `stage_twr` and `stage_dv` are the server's readout per `Craft::stages` index (empty before it arrives).
pub fn engineer_report(
    craft: &Craft,
    catalog: &Catalog,
    stage_twr: &[(usize, f64)],
    stage_dv: &[(usize, f64)],
) -> Vec<Flaw> {
    let mut out = Vec::new();
    if craft.parts.is_empty() {
        return out;
    }
    let specs: Vec<&PartSpec> = craft
        .parts
        .iter()
        .filter_map(|p| catalog.get(&p.part_id))
        .collect();
    let crewed = specs.iter().any(|s| s.crew_capacity > 0);
    let command = specs.iter().any(|s| s.category == "Pods");
    let probe = command && !crewed;
    let has = |f: &dyn Fn(&PartSpec) -> bool| specs.iter().any(|s| f(s));
    let flaw = |severity: u8, text: &str| Flaw {
        severity,
        text: text.to_string(),
    };
    if !command {
        out.push(flaw(
            2,
            "No command part: add a pod or a probe core to control the vessel.",
        ));
    }
    if !has(&|s| s.has_engine) {
        out.push(flaw(
            1,
            "No engines: the vessel cannot move under its own power.",
        ));
    }
    // The first stage to fire is the highest index.
    if let Some(first) = craft.stages.len().checked_sub(1) {
        if let Some((_, twr)) = stage_twr.iter().find(|(i, _)| *i == first) {
            if *twr > 0.0 && *twr < 1.0 {
                out.push(Flaw {
                    severity: 2,
                    text: format!(
                        "Not enough thrust to lift off: the first stage's TWR is {twr:.2} at Kerbin's sea level."
                    ),
                });
            }
        }
    }
    for (i, stage) in craft.stages.iter().enumerate() {
        let engines = stage.iter().any(|&p| {
            craft
                .parts
                .get(p)
                .and_then(|p| catalog.get(&p.part_id))
                .is_some_and(|s| s.has_engine)
        });
        if engines && stage_dv.iter().any(|(k, dv)| *k == i && *dv <= 0.5) {
            out.push(Flaw {
                severity: 1,
                text: format!(
                    "Stage {}: its engines have no propellant they can reach.",
                    craft.stages.len() - i
                ),
            });
        }
    }
    if crewed && !has(&|s| s.stage_type.contains("Parachute")) {
        out.push(flaw(1, "Crewed, but no parachute to come home with."));
    }
    if probe && !has(&|s| s.category == "Communication") && !has(&|s| s.id.contains("antenna")) {
        out.push(flaw(
            1,
            "Probe without an antenna: out of the KSC's range it loses control and cannot transmit.",
        ));
    }
    if probe
        && !has(&|s| {
            s.resource_containers
                .iter()
                .any(|c| c.resource.eq_ignore_ascii_case("ElectricCharge"))
        })
    {
        out.push(flaw(
            2,
            "Probe without electric charge storage: it cannot be controlled.",
        ));
    }
    if out.is_empty() {
        out.push(flaw(0, "No problems found."));
    }
    out
}

/// The part manager window for one part (`PartsManagerCore`, right click in the VAB): its name, stage,
/// crossfeed switch and a fill slider per resource.
pub fn pam_html(craft: &Craft, i: usize, catalog: &Catalog) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('"', "&quot;")
    };
    let Some(p) = craft.parts.get(i) else {
        return String::new();
    };
    let spec = catalog.get(&p.part_id);
    let title = spec.map(|s| s.display_title()).unwrap_or(&p.part_id);
    let stage = craft
        .stages
        .iter()
        .position(|s| s.contains(&i))
        .map(|n| {
            format!(
                "<div class=\"pam-sub\">Stage {}</div>",
                craft.stages.len() - n
            )
        })
        .unwrap_or_default();
    let mut rows = String::new();
    for (k, r) in p.resources.iter().enumerate() {
        if r.capacity_units <= 0.0 {
            continue;
        }
        rows.push_str(&format!(
            "<label class=\"pam-row\"><span>{name}</span><input type=\"range\" data-res=\"{k}\" min=\"0\" max=\"{cap}\" step=\"any\" value=\"{v}\" /><b>{v:.0} / {cap:.0}</b></label>",
            name = esc(&r.resource),
            cap = r.capacity_units,
            v = r.stored_units,
        ));
    }
    format!(
        "<div class=\"pam-head\"><b>{title}</b><button id=\"pam-close\" title=\"Close (Esc)\">&#10005;</button></div>{stage}\
         <label class=\"pam-row\"><span>Fuel crossfeed</span><input type=\"checkbox\" id=\"pam-crossfeed\"{cf} /></label>{rows}",
        title = esc(title),
        cf = if p.fuel_crossfeed { " checked" } else { "" },
    )
}

/// Categories in the order the original's picker shows them; anything else goes at the end.
pub const CATEGORY_ORDER: [&str; 13] = [
    "Pods",
    "FuelTank",
    "Engine",
    "Coupling",
    "Structural",
    "Aero",
    "Ground",
    "Thermal",
    "Electrical",
    "Communication",
    "Science",
    "Utility",
    "Payload",
];

pub fn categories(catalog: &Catalog) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for c in CATEGORY_ORDER {
        if catalog.values().any(|p| p.category == c) {
            seen.push(c.to_string());
        }
    }
    let mut extra: Vec<String> = catalog
        .values()
        .map(|p| p.category.clone())
        .filter(|c| !seen.contains(c) && c != "none")
        .collect();
    extra.sort();
    extra.dedup();
    seen.extend(extra);
    seen
}

/// Surface pressure of the bodies with an atmosphere, atm (the original's: Kerbin 1, Eve 5, Duna 0.0666,
/// Laythe 0.6); every other surface is vacuum.
pub fn surface_pressure_atm(body: &str) -> f64 {
    match body {
        "Kerbin" => 1.0,
        "Eve" => 5.0,
        "Duna" => 0.0666,
        "Laythe" => 0.6,
        _ => 0.0,
    }
}

/// A stage's delta-v and thrust-to-weight in a situation (the delta-v tool): thrust and Isp move from their
/// vacuum to their sea-level values with the pressure (`p_atm`, on past 1 atm), and the propellant flow
/// stays the same, so delta-v scales with the thrust. `g` is the surface gravity, m/s^2.
pub fn stage_in_situation(
    dv_vacuum_mps: f64,
    thrust_vacuum_kn: f64,
    thrust_asl_kn: f64,
    start_mass_t: f64,
    g: f64,
    p_atm: f64,
) -> (f64, f64) {
    let thrust = (thrust_vacuum_kn + (thrust_asl_kn - thrust_vacuum_kn) * p_atm).max(0.0);
    let dv = if thrust_vacuum_kn > 0.0 {
        dv_vacuum_mps * thrust / thrust_vacuum_kn
    } else {
        dv_vacuum_mps
    };
    let twr = if start_mass_t > 0.0 && g > 0.0 {
        thrust / (start_mass_t * g)
    } else {
        0.0
    };
    (dv.max(0.0) + 0.0, twr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_v_at_sea_level_and_in_vacuum() {
        // An engine with 215 kN in vacuum and 168 kN at sea level.
        let (dv, twr) = stage_in_situation(3000.0, 215.0, 168.0, 10.0, 9.81, 1.0);
        assert!((dv - 3000.0 * 168.0 / 215.0).abs() < 1e-9);
        assert!((twr - 168.0 / 98.1).abs() < 1e-9);
        let (dv, _) = stage_in_situation(3000.0, 215.0, 168.0, 10.0, 1.63, 0.0);
        assert_eq!(dv, 3000.0);
        // Eve's 5 atm takes more than sea level does.
        let (eve, _) = stage_in_situation(3000.0, 215.0, 168.0, 10.0, 16.7, 5.0);
        assert!(eve < 3000.0 * 168.0 / 215.0);
        assert_eq!(surface_pressure_atm("Mun"), 0.0);
    }

    fn node(id: &str, y: f64, dir: f64) -> NodeSpec {
        NodeSpec {
            id: id.into(),
            node_type: "Stack".into(),
            position: DVec3::new(0.0, y, 0.0),
            orientation: DVec3::new(0.0, dir, 0.0),
            size: 1,
        }
    }

    fn catalog() -> Catalog {
        let mut c = Catalog::new();
        c.insert(
            "pod".into(),
            PartSpec {
                id: "pod".into(),
                title: "Pod".into(),
                category: "Pods".into(),
                mass_t: 0.8,
                attach_nodes: vec![node("top", 0.6, 1.0), node("bottom", -0.6, -1.0)],
                ..Default::default()
            },
        );
        c.insert(
            "tank".into(),
            PartSpec {
                id: "tank".into(),
                category: "FuelTank".into(),
                mass_t: 0.5,
                size_m: Some(DVec3::new(1.25, 2.0, 1.25)),
                attach_nodes: vec![node("top", 1.0, 1.0), node("bottom", -1.0, -1.0)],
                resource_containers: vec![ContainerSpec {
                    resource: "Methane".into(),
                    capacity_units: 100.0,
                }],
                ..Default::default()
            },
        );
        c.insert(
            "engine".into(),
            PartSpec {
                id: "engine".into(),
                category: "Engine".into(),
                mass_t: 0.45,
                stage_type: "LiquidEngine".into(),
                has_engine: true,
                attach_nodes: vec![node("top", 0.9, 1.0), node("bottom", -1.0, -1.0)],
                ..Default::default()
            },
        );
        c.insert(
            "decoupler".into(),
            PartSpec {
                id: "decoupler".into(),
                category: "Coupling".into(),
                mass_t: 0.05,
                stage_type: "DecouplerVertical".into(),
                is_decoupler: true,
                attach_nodes: vec![node("top", 0.1, 1.0), node("bottom", -0.1, -1.0)],
                ..Default::default()
            },
        );
        c.insert(
            "light".into(),
            PartSpec {
                id: "light".into(),
                category: "Utility".into(),
                mass_t: 0.01,
                attach_nodes: vec![NodeSpec {
                    id: "srfAttach".into(),
                    node_type: "Surface".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        c
    }

    #[test]
    fn stack_builds_downwards_on_matching_nodes() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        assert_eq!(e.craft.parts[t].parent, Some(0));
        assert_eq!(e.craft.parts[t].parent_node.as_deref(), Some("bottom"));
        assert_eq!(e.craft.parts[t].own_node.as_deref(), Some("top"));
        // pod bottom at -0.6, tank top at +1.0: tank sits 1.6 below the pod
        assert!((e.craft.parts[t].local_position.y + 1.6).abs() < 1e-9);
        // root-relative: the engine sits 1.9 below the tank, 3.5 below the root
        assert!((e.craft.parts[en].local_position.y + 3.5).abs() < 1e-9);
        assert_eq!(e.craft.parts[t].resources[0].stored_units, 100.0);
        assert_eq!(e.craft.stages, vec![vec![en]]);
        assert!((e.dry_mass_t(&cat) - 1.75).abs() < 1e-9);
        assert!(e.craft.is_tree());
    }

    #[test]
    fn drop_on_a_chosen_node_attaches_there() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        // The tank's free nodes include its bottom; the pod has its top free.
        let free = e.free_nodes(&cat);
        assert!(free.iter().any(|(i, n, ..)| *i == t && n == "bottom"));
        assert!(free.iter().any(|(i, n, ..)| *i == 0 && n == "top"));
        // Dropping a tank on the pod's top node puts it above the root.
        let up = e.add_part_on(&cat["tank"], &cat, 0, "top").unwrap();
        assert_eq!(e.craft.parts[up].parent, Some(0));
        assert_eq!(e.craft.parts[up].parent_node.as_deref(), Some("top"));
        assert!(e.craft.parts[up].local_position.y > 0.0);
    }

    #[test]
    fn surface_parts_go_on_the_side_and_removal_prunes_subtrees() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        e.add_part(&cat["tank"], &cat).unwrap();
        e.selected = Some(1);
        let l = e.add_part(&cat["light"], &cat).unwrap();
        assert_eq!(e.craft.parts[l].parent_node, None);
        assert!(e.craft.parts[l].local_position.x > 0.6);
        e.selected = Some(1);
        e.add_part(&cat["engine"], &cat).unwrap();
        assert_eq!(e.craft.parts.len(), 4);
        e.selected = Some(1);
        assert_eq!(e.remove_selected(&cat), Some(3)); // tank, light, engine
        assert_eq!(e.craft.parts.len(), 1);
        assert_eq!(e.selected, Some(0));
        assert!(e.craft.stages.is_empty());
    }

    #[test]
    fn engineer_report_flags_what_a_craft_lacks() {
        let cat = catalog();
        let mut e = Editor::new("test");
        assert!(engineer_report(&e.craft, &cat, &[], &[]).is_empty());
        e.add_part(&cat["pod"], &cat).unwrap();
        let r = engineer_report(&e.craft, &cat, &[], &[]);
        let text: Vec<&str> = r.iter().map(|f| f.text.as_str()).collect();
        assert!(text.iter().any(|t| t.starts_with("No engines")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("antenna")));
        e.add_part(&cat["tank"], &cat).unwrap();
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        let stage = e.craft.stages.iter().position(|s| s.contains(&en)).unwrap();
        let r = engineer_report(&e.craft, &cat, &[(stage, 0.6)], &[(stage, 900.0)]);
        assert!(r
            .iter()
            .any(|f| f.severity == 2 && f.text.contains("TWR is 0.60")));
        assert!(!r.iter().any(|f| f.text.starts_with("No engines")));
    }

    #[test]
    fn a_part_moves_onto_another_stage() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        e.add_part(&cat["decoupler"], &cat).unwrap();
        e.add_part(&cat["tank"], &cat).unwrap();
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        assert_eq!(e.craft.stages, vec![vec![1], vec![en]]);
        e.move_to_stage(en, 0, &cat);
        assert_eq!(e.craft.stages, vec![vec![1, en]]);
        e.move_to_stage(0, 0, &cat); // the pod is not stageable: nothing changes
        assert_eq!(e.craft.stages, vec![vec![1, en]]);
    }

    #[test]
    fn grab_and_reattach_a_subtree() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        let mut h = History::default();
        h.record(&e);
        let frag = e.detach(t, &cat).unwrap();
        assert_eq!(frag.parts.len(), 2);
        assert_eq!(frag.root_id(), "tank");
        assert_eq!(frag.parts[1].parent, Some(0));
        assert_eq!(e.craft.parts.len(), 1);
        assert!(e.craft.stages.is_empty());
        // Hang it on the pod's top instead: the engine comes along, 1.9 m under the tank.
        let r = e.attach(&frag, &cat, Some((0, "top"))).unwrap();
        assert_eq!(e.craft.parts.len(), 3);
        assert_eq!(e.craft.parts[r].parent_node.as_deref(), Some("top"));
        let engine = e
            .craft
            .parts
            .iter()
            .position(|p| p.part_id == "engine")
            .unwrap();
        assert_eq!(e.craft.parts[engine].parent, Some(r));
        let gap = e.craft.parts[r].local_position.y - e.craft.parts[engine].local_position.y;
        assert!((gap - 1.9).abs() < 1e-9);
        assert!(e.craft.is_tree());
        assert_eq!(e.craft.stages.len(), 1);
        let guids: std::collections::HashSet<_> = e.craft.parts.iter().map(|p| &p.guid).collect();
        assert_eq!(guids.len(), 3);
        // Undo brings back the craft as it was before the grab.
        assert!(h.undo(&mut e));
        assert_eq!(e.craft.parts[en].part_id, "engine");
        assert_eq!(e.craft.parts[t].parent_node.as_deref(), Some("bottom"));
        assert!(h.redo(&mut e));
        assert_eq!(e.craft.parts.len(), 3);
    }

    #[test]
    fn held_turns_apply_to_surface_and_stack_parts() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        let mut spot = e
            .surface_spot(t, DVec3::new(1.0, -1.6, 0.0), &cat, None)
            .unwrap();
        spot.turn = axis_turn(2, 90.0);
        let l = e.add_surface(&cat["light"], &cat, spot, 1).unwrap()[0];
        // Turned, but its attach node still touches the tank's side.
        let p = &e.craft.parts[l];
        assert!((p.local_rotation.w - 1.0).abs() > 1e-6);
        let node = p.local_position + quat_rotate(p.local_rotation, DVec3::ZERO);
        let tank_x = e.craft.parts[t].local_position.x;
        assert!(((node.x - tank_x).hypot(node.z) - 0.625).abs() < 1e-9);
        // A stack part yawed about its own top node stays on the node.
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        let before = e.craft.parts[en].local_position;
        e.turn_on_own_node(en, axis_turn(1, 90.0), &cat);
        let after = e.craft.parts[en].local_position;
        assert!((after.y - before.y).abs() < 1e-9 && (after.x - before.x).abs() < 1e-9);
        let w = turn_then(axis_turn(1, 90.0), axis_turn(1, 90.0));
        assert!((quat_rotate(w, DVec3::new(1.0, 0.0, 0.0)).x + 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_offset_tool_moves_a_part_with_what_hangs_on_it() {
        let cat = catalog();
        let mut e = Editor::new("test");
        let pod = e.add_part(&cat["pod"], &cat).unwrap();
        let tank = e.add_part(&cat["tank"], &cat).unwrap();
        let engine = e.add_part(&cat["engine"], &cat).unwrap();
        let (t0, e0) = (
            e.craft.parts[tank].local_position,
            e.craft.parts[engine].local_position,
        );
        assert!(e.offset(tank, DVec3::new(0.1, 0.0, 0.0)));
        assert!((e.craft.parts[tank].local_position.x - t0.x - 0.1).abs() < 1e-12);
        assert!((e.craft.parts[engine].local_position.x - e0.x - 0.1).abs() < 1e-12);
        // The root does not move.
        assert!(!e.offset(pod, DVec3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn a_plane_pairs_side_parts_as_mirror_images() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        e.set_plane(true);
        let p = e.craft.parts[t].local_position;
        // A side part a little above the fuselage's middle on its right.
        let spot = e
            .surface_spot(t, DVec3::new(0.5, 0.3, p.z), &cat, None)
            .unwrap();
        let made = e.add_surface(&cat["light"], &cat, spot, 2).unwrap();
        let (a, b) = (&e.craft.parts[made[0]], &e.craft.parts[made[1]]);
        // The pair sits at the same height and along the nose, on opposite sides.
        assert!(
            (a.local_position.y - b.local_position.y).abs() < 1e-9,
            "{a:?} {b:?}"
        );
        assert!((a.local_position.z - b.local_position.z).abs() < 1e-9);
        assert!((a.local_position.x + b.local_position.x).abs() < 1e-9);
        // Each faces the fuselage: the node direction (+X) and the part's up (+Y) mirrored.
        for axis in [DVec3::new(1.0, 0.0, 0.0), DVec3::new(0.0, 1.0, 0.0)] {
            let fa = quat_rotate(a.local_rotation, axis);
            let fb = quat_rotate(b.local_rotation, axis);
            assert!(
                (fa.x + fb.x).abs() < 1e-9
                    && (fa.y - fb.y).abs() < 1e-9
                    && (fa.z - fb.z).abs() < 1e-9,
                "{fa:?} {fb:?}"
            );
        }
        // A rocket keeps turning its pairs round the axis.
        e.set_plane(false);
        assert_eq!(e.mirror_plane().1, DVec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn a_plane_lies_along_the_floor_and_its_parts_follow() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        assert!(!e.is_plane());
        let before = e.craft.parts[t].local_position;
        e.set_plane(true);
        assert!(e.is_plane());
        // The tank hung 1.6 m below the pod; now 1.6 m behind it along -Z, turned the same way.
        let p = e.craft.parts[t].local_position;
        assert!(
            (p.y).abs() < 1e-9 && (p.z + before.y.abs()).abs() < 1e-9,
            "{p:?}"
        );
        // A part added under the tank keeps going along the fuselage.
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        let q = e.craft.parts[en].local_position;
        assert!(q.z < p.z - 1.0 && q.y.abs() < 1e-9, "{q:?}");
        // A side part on the fuselage still faces it.
        let spot = e
            .surface_spot(t, DVec3::new(0.625, 0.0, p.z), &cat, Some(15.0))
            .unwrap();
        let l = e.add_surface(&cat["light"], &cat, spot, 1).unwrap()[0];
        let lp = e.craft.parts[l].local_position;
        assert!(
            (lp.x - 0.625).abs() < 1e-6 && (lp.z - p.z).abs() < 1e-6,
            "{lp:?}"
        );
        e.set_plane(false);
        assert!(!e.is_plane());
        assert!((e.craft.parts[t].local_position.y - before.y).abs() < 1e-9);
    }

    #[test]
    fn copy_leaves_the_craft_and_pastes_a_second_set() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        e.add_part(&cat["engine"], &cat).unwrap();
        let frag = e.copy(t, &cat).unwrap();
        assert_eq!(e.craft.parts.len(), 3);
        assert_eq!(frag.parts.len(), 2);
        e.attach(&frag, &cat, Some((0, "top"))).unwrap();
        assert_eq!(e.craft.parts.len(), 5);
        assert!(e.craft.is_tree());
        // The copy keeps its stage: both engines fire together.
        assert_eq!(e.craft.stages.len(), 1);
        assert_eq!(e.craft.stages[0].len(), 2);
    }

    #[test]
    fn grabbing_the_root_lifts_the_craft() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        e.add_part(&cat["tank"], &cat).unwrap();
        let frag = e.detach(0, &cat).unwrap();
        assert!(e.is_empty());
        assert!(e.attach(&frag, &cat, None).is_ok());
        assert_eq!(e.craft.parts.len(), 2);
        assert!(e.craft.is_tree());
    }

    #[test]
    fn surface_parts_face_in_and_copy_around_the_axis() {
        let mut cat = catalog();
        cat.insert(
            "booster".into(),
            PartSpec {
                id: "booster".into(),
                category: "Engine".into(),
                mass_t: 1.0,
                stage_type: "SolidEngine".into(),
                has_engine: true,
                size_m: Some(DVec3::new(0.6, 2.0, 0.6)),
                attach_nodes: vec![NodeSpec {
                    id: "srfAttach".into(),
                    node_type: "Surface".into(),
                    position: DVec3::new(0.3, 0.0, 0.0),
                    orientation: DVec3::new(1.0, 0.0, 0.0),
                    size: 1,
                }],
                ..Default::default()
            },
        );
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        let t = e.add_part(&cat["tank"], &cat).unwrap();
        // A hit on the tank's +X side, slightly off: snapped to 15 degree steps it is straight +X.
        let spot = e
            .surface_spot(t, DVec3::new(0.62, -1.5, 0.05), &cat, Some(15.0))
            .unwrap();
        assert!(spot.azimuth.abs() < 1e-9);
        let made = e.add_surface(&cat["booster"], &cat, spot, 4).unwrap();
        assert_eq!(made.len(), 4);
        let tank_x = e.craft.parts[t].local_position.x;
        for (k, &i) in made.iter().enumerate() {
            let p = &e.craft.parts[i];
            assert_eq!(p.parent, Some(t));
            // The node (+X in the part frame) points back at the tank's axis.
            let face = quat_rotate(p.local_rotation, DVec3::new(1.0, 0.0, 0.0));
            let node_world =
                p.local_position + quat_rotate(p.local_rotation, DVec3::new(0.3, 0.0, 0.0));
            let out = DVec3::new(node_world.x - tank_x, 0.0, node_world.z);
            assert!(
                (out.length() - 0.625).abs() < 1e-9,
                "copy {k} touches the tank side"
            );
            assert!(face.dot(out) < -0.6, "copy {k} faces the tank");
        }
        // One stage for the whole symmetry group.
        assert_eq!(e.craft.stages.len(), 1);
        assert_eq!(e.craft.stages[0].len(), 4);
        assert!(e.craft.is_tree());
        // A booster lifted and hung back on the side keeps its kids turned with it.
        let frag = e.detach(made[1], &cat).unwrap();
        let spot = e
            .surface_spot(t, DVec3::new(0.0, -1.0, 0.7), &cat, None)
            .unwrap();
        let r = e.attach_surface(&frag, &cat, spot).unwrap();
        let face = quat_rotate(e.craft.parts[r].local_rotation, DVec3::new(1.0, 0.0, 0.0));
        assert!(face.z < -0.99);
    }

    #[test]
    fn stages_group_and_shift() {
        let cat = catalog();
        let mut e = Editor::new("test");
        e.add_part(&cat["pod"], &cat).unwrap();
        e.add_part(&cat["decoupler"], &cat).unwrap();
        e.add_part(&cat["tank"], &cat).unwrap();
        let en = e.add_part(&cat["engine"], &cat).unwrap();
        assert_eq!(e.craft.stages, vec![vec![1], vec![en]]);
        e.selected = Some(en);
        e.shift_stage(-5, &cat);
        assert_eq!(e.craft.stages, vec![vec![1, en]]);
        assert_eq!(
            categories(&cat),
            vec!["Pods", "FuelTank", "Engine", "Coupling", "Utility"]
        );
    }
}

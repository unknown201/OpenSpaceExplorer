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
        }
    }

    pub fn from_craft(craft: Craft) -> Editor {
        let selected = if craft.parts.is_empty() {
            None
        } else {
            Some(0)
        };
        Editor { craft, selected }
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
                local_rotation: IDENTITY,
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
        // `partState.localPosition`; the editor never rotates parts, so the parent's position is its frame.
        let local_position = self.craft.parts[target].local_position + local_position;
        let idx = self.craft.parts.len();
        self.craft.parts.push(CraftPart {
            guid: format!("p{}", idx + 1),
            part_id: spec.id.clone(),
            parent: Some(target),
            own_node,
            parent_node,
            local_position,
            local_rotation: IDENTITY,
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
        let axis = p.local_position + quat_rotate(p.local_rotation, spec.centre());
        let mut azimuth = (hit.z - axis.z).atan2(hit.x - axis.x);
        if let Some(step) = snap_deg.filter(|s| *s > 0.0) {
            let s = step.to_radians();
            azimuth = (azimuth / s).round() * s;
        }
        Some(SurfaceSpot {
            target,
            azimuth,
            height: hit.y,
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
        let axis = p.local_position + quat_rotate(p.local_rotation, tspec.centre());
        let radius = tspec.size().x.min(tspec.size().z) * 0.5;
        (0..symmetry.max(1))
            .map(|k| {
                let az = spot.azimuth + std::f64::consts::TAU * k as f64 / symmetry.max(1) as f64;
                let out = DVec3::new(az.cos(), 0.0, az.sin());
                let contact = DVec3::new(axis.x, spot.height, axis.z) + out * radius;
                let q = quat_mul(facing_in(az), spot.turn);
                (contact - quat_rotate(q, node), q)
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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

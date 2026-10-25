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

    /// Move the selected stageable part one stage earlier (`-1`) or later (`+1`).
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

    /// `Craft::stages`: stageable parts grouped by `stage_index`, first-firing stage first, renumbered densely.
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
        // Dense renumbering so the indices are 1..=n in firing order.
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

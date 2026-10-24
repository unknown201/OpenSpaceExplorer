//! The resource flow graph and request processor (see `docs/reference-notes/resource-flow.md`).
//!
//! Parts are nodes; every attachment adds an edge each way whose kind comes from the part the edge leaves:
//! leaving a decoupler crosses a structure boundary, and an edge carries crossfeed when the part it leaves has
//! crossfeed on. A request drains its resource from groups of tanks chosen by the resource's flow mode:
//!
//! - `NoFlow`: the requesting part only (solid boosters).
//! - `AllVessel`: every tank, one group.
//! - `StagePriorityFlow`: groups by the number of decouplers between tank and requester, nearest first.
//! - `StackPrioritySearch` / `StageStackFlowBalance`: the tanks reachable over crossfeed edges, one group
//!   (fuel lines, which would add further groups, are not modelled yet).
//!
//! Draining takes everything from one group before the next; inside a group each tank gives in proportion to
//! what it holds, so tanks in a stack empty together. A request's instructions share one delivered fraction,
//! so an engine short of oxidizer also cuts its methane draw. Requests run in descending priority.

use std::collections::{HashMap, VecDeque};

use ose_data::craft::Craft;
use ose_data::{FlowMode, PartDef, ResourceDef, ResourceKind};

#[derive(Clone, Debug, PartialEq)]
pub struct Tank {
    pub part: usize,
    /// Index into `FlowNet::resources`.
    pub resource: usize,
    pub stored: f64,
    pub capacity: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge {
    to: usize,
    crossfeed: bool,
    boundary: bool,
}

/// One resource the request moves out of the tanks, units per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instruction {
    pub resource: usize,
    pub units_per_s: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub part: usize,
    pub instructions: Vec<Instruction>,
    pub priority: i32,
}

#[derive(Clone, Debug)]
pub struct FlowNet {
    /// Basic resource ids, with their unit mass (t) and flow mode.
    pub resources: Vec<String>,
    pub unit_mass_t: Vec<f64>,
    pub flow_mode: Vec<FlowMode>,
    pub tanks: Vec<Tank>,
    /// Dry mass of each part, t.
    pub dry_t: Vec<f64>,
    pub present: Vec<bool>,
    edges: Vec<Vec<Edge>>,
    recipes: HashMap<String, Vec<(usize, f64)>>,
}

impl FlowNet {
    pub fn build(craft: &Craft, defs: &[PartDef], resources: &[ResourceDef]) -> FlowNet {
        let mut names = Vec::new();
        let mut unit_mass_t = Vec::new();
        let mut flow_mode = Vec::new();
        for r in resources {
            if let ResourceKind::Basic {
                mass_per_unit_t,
                flow_mode: m,
                ..
            } = &r.kind
            {
                names.push(r.id.clone());
                unit_mass_t.push(*mass_per_unit_t);
                flow_mode.push(*m);
            }
        }
        let index_of =
            |id: &str| -> Option<usize> { names.iter().position(|n| n.eq_ignore_ascii_case(id)) };
        let mut recipes = HashMap::new();
        for r in resources {
            if let ResourceKind::Recipe { ingredients } = &r.kind {
                let ing: Vec<(usize, f64)> = ingredients
                    .iter()
                    .filter_map(|i| Some((index_of(&i.resource)?, i.units_per_recipe_unit)))
                    .collect();
                recipes.insert(r.id.to_ascii_lowercase(), ing);
            }
        }
        let def_of = |i: usize| defs.iter().find(|d| d.id == craft.parts[i].part_id);
        let mut tanks = Vec::new();
        for (i, p) in craft.parts.iter().enumerate() {
            for r in &p.resources {
                if let Some(ri) = index_of(&r.resource) {
                    tanks.push(Tank {
                        part: i,
                        resource: ri,
                        stored: r.stored_units,
                        capacity: r.capacity_units,
                    });
                }
            }
        }
        let n = craft.parts.len();
        let decoupler = |i: usize| {
            def_of(i).is_some_and(|d| d.stage_type.to_ascii_lowercase().contains("decoupler"))
        };
        let edge_from = |from: usize, to: usize| Edge {
            to,
            crossfeed: craft.parts[from].fuel_crossfeed,
            boundary: decoupler(from),
        };
        let mut edges = vec![Vec::new(); n];
        for (i, p) in craft.parts.iter().enumerate() {
            if let Some(parent) = p.parent {
                edges[i].push(edge_from(i, parent));
                edges[parent].push(edge_from(parent, i));
            }
        }
        FlowNet {
            resources: names,
            unit_mass_t,
            flow_mode,
            tanks,
            dry_t: (0..n)
                .map(|i| def_of(i).map(|d| d.mass_t).unwrap_or(0.0))
                .collect(),
            present: vec![true; n],
            edges,
            recipes,
        }
    }

    pub fn resource(&self, id: &str) -> Option<usize> {
        self.resources
            .iter()
            .position(|n| n.eq_ignore_ascii_case(id))
    }

    /// Ingredients (resource, units per unit of mixture) of an engine mixture; a basic resource is itself.
    pub fn ingredients(&self, mixture: &str) -> Vec<(usize, f64)> {
        match self.recipes.get(&mixture.to_ascii_lowercase()) {
            Some(v) => v.clone(),
            None => self
                .resource(mixture)
                .map(|r| vec![(r, 1.0)])
                .unwrap_or_default(),
        }
    }

    /// Mass of one unit of a mixture, t.
    pub fn mixture_unit_mass_t(&self, mixture: &str) -> f64 {
        self.ingredients(mixture)
            .iter()
            .map(|&(r, u)| u * self.unit_mass_t[r])
            .sum()
    }

    /// Take parts out of the graph (staged away, broken off). Their tanks go with them.
    pub fn remove_parts(&mut self, parts: &[usize]) {
        for &i in parts {
            if let Some(p) = self.present.get_mut(i) {
                *p = false;
            }
        }
    }

    /// Mass of a set of parts with what their tanks hold now, t.
    pub fn mass_of(&self, parts: &[usize]) -> f64 {
        let dry: f64 = parts.iter().map(|&i| self.dry_t[i]).sum();
        let wet: f64 = self
            .tanks
            .iter()
            .filter(|t| parts.contains(&t.part))
            .map(|t| t.stored * self.unit_mass_t[t.resource])
            .sum();
        dry + wet
    }

    /// Mass of the parts still in the graph, t.
    pub fn mass_t(&self) -> f64 {
        let parts: Vec<usize> = (0..self.present.len())
            .filter(|&i| self.present[i])
            .collect();
        self.mass_of(&parts)
    }

    /// Tank groups a request from `part` for `resource` drains, in draining order.
    pub fn groups(&self, part: usize, resource: usize) -> Vec<Vec<usize>> {
        let tanks_in = |parts: &[usize]| -> Vec<usize> {
            self.tanks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.resource == resource && parts.contains(&t.part))
                .map(|(i, _)| i)
                .collect()
        };
        if !self.present.get(part).copied().unwrap_or(false) {
            return Vec::new();
        }
        match self.flow_mode[resource] {
            FlowMode::NoFlow | FlowMode::Null => vec![tanks_in(&[part])],
            FlowMode::AllVessel => {
                let all: Vec<usize> = (0..self.present.len())
                    .filter(|&i| self.present[i])
                    .collect();
                vec![tanks_in(&all)]
            }
            FlowMode::StagePriorityFlow => {
                // 0-1 breadth-first search: crossing out of a decoupler costs one
                let n = self.present.len();
                let mut dist = vec![usize::MAX; n];
                let mut q = VecDeque::new();
                dist[part] = 0;
                q.push_back(part);
                while let Some(i) = q.pop_front() {
                    for e in &self.edges[i] {
                        if !self.present[e.to] {
                            continue;
                        }
                        let d = dist[i] + usize::from(e.boundary);
                        if d < dist[e.to] {
                            dist[e.to] = d;
                            if e.boundary {
                                q.push_back(e.to);
                            } else {
                                q.push_front(e.to);
                            }
                        }
                    }
                }
                let max = dist
                    .iter()
                    .filter(|&&d| d != usize::MAX)
                    .max()
                    .copied()
                    .unwrap_or(0);
                (0..=max)
                    .map(|d| {
                        let parts: Vec<usize> = (0..n).filter(|&i| dist[i] == d).collect();
                        tanks_in(&parts)
                    })
                    .filter(|g| !g.is_empty())
                    .collect()
            }
            FlowMode::StackPrioritySearch | FlowMode::StageStackFlowBalance => {
                let n = self.present.len();
                let mut seen = vec![false; n];
                let mut stack = vec![part];
                seen[part] = true;
                while let Some(i) = stack.pop() {
                    for e in &self.edges[i] {
                        if e.crossfeed && self.present[e.to] && !seen[e.to] {
                            seen[e.to] = true;
                            stack.push(e.to);
                        }
                    }
                }
                let parts: Vec<usize> = (0..n).filter(|&i| seen[i]).collect();
                vec![tanks_in(&parts)]
            }
        }
    }

    /// Units of `resource` a request from `part` can reach.
    pub fn available(&self, part: usize, resource: usize) -> f64 {
        self.groups(part, resource)
            .iter()
            .flatten()
            .map(|&t| self.tanks[t].stored)
            .sum()
    }

    /// Run the requests for one step; returns each request's delivered fraction (0..1), in input order.
    pub fn process(&mut self, requests: &[Request], dt: f64) -> Vec<f64> {
        let mut order: Vec<usize> = (0..requests.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(requests[i].priority));
        let mut fractions = vec![0.0; requests.len()];
        for i in order {
            let r = &requests[i];
            let mut fraction: f64 = 1.0;
            let mut plans = Vec::with_capacity(r.instructions.len());
            for ins in &r.instructions {
                let want = ins.units_per_s * dt;
                let groups = self.groups(r.part, ins.resource);
                if want > 0.0 {
                    let have: f64 = groups.iter().flatten().map(|&t| self.tanks[t].stored).sum();
                    fraction = fraction.min(have / want);
                }
                plans.push((want, groups));
            }
            let fraction = fraction.clamp(0.0, 1.0);
            fractions[i] = fraction;
            if fraction <= 0.0 {
                continue;
            }
            for (want, groups) in plans {
                let mut left = want * fraction;
                for g in groups {
                    if left <= 0.0 {
                        break;
                    }
                    let stored: f64 = g.iter().map(|&t| self.tanks[t].stored).sum();
                    if stored <= 0.0 {
                        continue;
                    }
                    let take = left.min(stored);
                    let share = take / stored;
                    for &t in &g {
                        let tank = &mut self.tanks[t];
                        tank.stored = (tank.stored - tank.stored * share).max(0.0);
                    }
                    left -= take;
                }
            }
        }
        fractions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> (Vec<Craft>, Vec<PartDef>, Vec<ResourceDef>) {
        (
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
        )
    }

    fn k1() -> (Craft, FlowNet) {
        let (crafts, defs, res) = assets();
        let c = crafts.into_iter().find(|c| c.name == "Kerbal K1").unwrap();
        let net = FlowNet::build(&c, &defs, &res);
        (c, net)
    }

    fn methalox_request(net: &FlowNet, part: usize, units_per_s: f64) -> Request {
        Request {
            part,
            instructions: net
                .ingredients("Methalox")
                .into_iter()
                .map(|(resource, u)| Instruction {
                    resource,
                    units_per_s: u * units_per_s,
                })
                .collect(),
            priority: 0,
        }
    }

    fn stored(net: &FlowNet, part: usize, res: &str) -> f64 {
        let r = net.resource(res).unwrap();
        net.tanks
            .iter()
            .filter(|t| t.part == part && t.resource == r)
            .map(|t| t.stored)
            .sum()
    }

    #[test]
    fn stacked_tanks_drain_together_and_the_decoupler_isolates_the_upper_stage() {
        // K1: core engine 36 under tanks 18 (1x4) and 19 (1x8); decoupler 17 above them, then the Terrier
        // stage with tank 7
        let (_, mut net) = k1();
        let ox = net.resource("Oxidizer").unwrap();
        let g = net.groups(36, ox);
        assert_eq!(g.len(), 1);
        let parts: Vec<usize> = g[0].iter().map(|&t| net.tanks[t].part).collect();
        assert!(parts.contains(&18) && parts.contains(&19), "{parts:?}");
        assert!(!parts.contains(&7), "the decoupler blocks crossfeed");
        let (a0, b0) = (stored(&net, 18, "Oxidizer"), stored(&net, 19, "Oxidizer"));
        let req = methalox_request(&net, 36, 1.0);
        for _ in 0..100 {
            let f = net.process(std::slice::from_ref(&req), 0.02);
            assert_eq!(f, vec![1.0]);
        }
        let (a1, b1) = (stored(&net, 18, "Oxidizer"), stored(&net, 19, "Oxidizer"));
        assert!(((a1 / a0) - (b1 / b0)).abs() < 1e-9, "same fill ratio");
        assert!(
            ((a0 + b0) - (a1 + b1) - 1.6).abs() < 1e-9,
            "0.8 units/s of oxidizer for 2 s"
        );
        assert_eq!(stored(&net, 7, "Oxidizer"), stored(&k1().1, 7, "Oxidizer"));
    }

    #[test]
    fn solid_fuel_stays_in_its_booster() {
        let (c, net) = k1();
        let sf = net.resource("SolidFuel").unwrap();
        let hammer = c
            .parts
            .iter()
            .position(|p| p.part_id == "booster_1v_solid_hammer")
            .unwrap();
        let g = net.groups(hammer, sf);
        assert_eq!(g.len(), 1);
        assert!(g[0].iter().all(|&t| net.tanks[t].part == hammer));
    }

    #[test]
    fn stage_priority_takes_the_nearest_group_first() {
        // electric charge flows by stage priority: the probe core's own stage first
        let (_, net) = k1();
        let ec = net.resource("ElectricCharge").unwrap();
        let g = net.groups(0, ec);
        assert!(!g.is_empty());
        // the pod's group holds no part beyond the first decoupler
        for &t in &g[0] {
            let p = net.tanks[t].part;
            assert!(p == 0 || p == 37 || p == 1 || p < 17, "{p}");
        }
    }

    #[test]
    fn a_starved_engine_gets_the_fraction_left_and_shares_it_across_ingredients() {
        let (_, mut net) = k1();
        let ox = net.resource("Oxidizer").unwrap();
        let me = net.resource("Methane").unwrap();
        // empty the core's oxidizer down to 0.1 units
        let mut left = 0.1;
        for t in net
            .tanks
            .iter_mut()
            .filter(|t| t.resource == ox && (t.part == 18 || t.part == 19))
        {
            t.stored = left;
            left = 0.0;
        }
        let m0 = stored(&net, 18, "Methane") + stored(&net, 19, "Methane");
        let req = methalox_request(&net, 36, 1.0);
        let f = net.process(&[req], 1.0); // wants 0.8 oxidizer, has 0.1
        assert!((f[0] - 0.125).abs() < 1e-9, "{f:?}");
        let m1 = stored(&net, 18, "Methane") + stored(&net, 19, "Methane");
        assert!(
            (m0 - m1 - 0.2 * 0.125).abs() < 1e-9,
            "methane draw cut to the same fraction"
        );
        let _ = me;
    }

    #[test]
    fn removed_parts_take_their_tanks() {
        let (_, mut net) = k1();
        let before = net.mass_t();
        net.remove_parts(&[18]);
        let ox = net.resource("Oxidizer").unwrap();
        assert!(net.groups(36, ox)[0]
            .iter()
            .all(|&t| net.tanks[t].part != 18));
        assert!(net.mass_t() < before - 2.0);
    }
}

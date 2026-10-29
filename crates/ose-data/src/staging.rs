//! Per-stage delta-v for a craft, the way the editor readout works.
//!
//! Stages fire from the highest stage id down to 0 (the KSP convention; in Kerbal K2 stage 4 holds the
//! boosters and the main engine, stage 0 the parachute). Firing a stage first jettisons the subtrees hanging
//! below its decouplers, then ignites its engines. Running engines keep burning across later stages until
//! their part is jettisoned. Each stage burns until the first propellant any running engine needs runs out
//! in the containers it can reach over crossfeed, then the next stage fires.
//!
//! Crossfeed follows the craft tree: a link carries propellant when both parts allow crossfeed
//! (`CraftPart::fuel_crossfeed` and the part definition's `fuel_cross_feed`); decouplers normally do not.

use crate::craft::Craft;
use crate::{PartDef, ResourceDef, ResourceKind, G0};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub struct StageDeltaV {
    pub stage_id: usize,
    /// Part indices still attached while this stage burns (after its jettison).
    pub alive: Vec<usize>,
    pub start_mass_t: f64,
    pub end_mass_t: f64,
    pub delta_v_vacuum_mps: f64,
    pub burn_time_s: f64,
    /// Vacuum thrust of the engines running during this stage, kN.
    pub thrust_kn: f64,
    /// Part indices running engines during this stage.
    pub engines: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeltaVReport {
    pub stages: Vec<StageDeltaV>,
    pub total_vacuum_mps: f64,
}

struct Catalog<'a> {
    parts: HashMap<&'a str, &'a PartDef>,
    unit_mass: HashMap<&'a str, f64>,
    recipes: HashMap<&'a str, Vec<(&'a str, f64)>>,
}

impl<'a> Catalog<'a> {
    fn new(parts: &'a [PartDef], resources: &'a [ResourceDef]) -> Self {
        let mut unit_mass = HashMap::new();
        let mut recipes = HashMap::new();
        for r in resources {
            match &r.kind {
                ResourceKind::Basic {
                    mass_per_unit_t, ..
                } => {
                    unit_mass.insert(r.id.as_str(), *mass_per_unit_t);
                }
                ResourceKind::Recipe { ingredients } => {
                    recipes.insert(
                        r.id.as_str(),
                        ingredients
                            .iter()
                            .map(|i| (i.resource.as_str(), i.units_per_recipe_unit))
                            .collect(),
                    );
                }
            }
        }
        Catalog {
            parts: parts.iter().map(|p| (p.id.as_str(), p)).collect(),
            unit_mass,
            recipes,
        }
    }

    /// Ingredients (resource id, units per unit of mixture) of an engine mixture; a basic resource is itself.
    fn ingredients(&self, mixture: &'a str) -> Vec<(&'a str, f64)> {
        match self.recipes.get(mixture) {
            Some(v) => v.clone(),
            None => vec![(mixture, 1.0)],
        }
    }
}

fn resource_key_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Compute the per-stage delta-v report.
pub fn delta_v_by_stage(
    craft: &Craft,
    parts: &[PartDef],
    resources: &[ResourceDef],
) -> DeltaVReport {
    let cat = Catalog::new(parts, resources);
    let n = craft.parts.len();
    // stored units per part per resource (mutable copy)
    let mut stored: Vec<HashMap<String, f64>> = craft
        .parts
        .iter()
        .map(|p| {
            p.resources
                .iter()
                .map(|r| (r.resource.clone(), r.stored_units))
                .collect()
        })
        .collect();
    let def_of = |i: usize| cat.parts.get(craft.parts[i].part_id.as_str()).copied();
    let dry_mass = |i: usize| def_of(i).map(|d| craft.parts[i].dry_mass(d)).unwrap_or(0.0);
    let crossfeed_ok =
        |i: usize| craft.parts[i].fuel_crossfeed && def_of(i).is_some_and(|d| d.fuel_cross_feed);

    // undirected adjacency from the tree
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in craft.parts.iter().enumerate() {
        if let Some(par) = p.parent {
            adj[i].push(par);
            adj[par].push(i);
        }
    }
    // a fuel line (the part, hung on its source tank) lets the part it feeds draw from that tank, one way
    let mut lines: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (i, p) in craft.parts.iter().enumerate() {
        if let (Some(target), Some(source)) = (p.fuel_line_target, p.parent) {
            if target < n {
                lines[target].push((source, i));
            }
        }
    }

    let mut alive: Vec<bool> = vec![true; n];
    let mut running: HashSet<usize> = HashSet::new();
    let mut stages_out = Vec::new();

    let total_mass = |alive: &[bool], stored: &[HashMap<String, f64>]| -> f64 {
        let mut m = 0.0;
        for i in 0..n {
            if !alive[i] {
                continue;
            }
            m += dry_mass(i);
            for (res, units) in &stored[i] {
                let um = cat
                    .unit_mass
                    .iter()
                    .find(|(k, _)| resource_key_eq(k, res))
                    .map(|(_, v)| *v)
                    .unwrap_or(0.0);
                m += units * um;
            }
        }
        m
    };

    let mut order: Vec<usize> = (0..craft.stages.len()).collect();
    order.sort_by(|a, b| b.cmp(a));
    for stage_id in order {
        // 1. jettison below decouplers in this stage
        for &pi in &craft.stages[stage_id] {
            if !alive[pi] {
                continue;
            }
            let Some(def) = def_of(pi) else { continue };
            let is_decoupler = matches!(
                def.stage_type.as_str(),
                "DecouplerHorizontal" | "DecouplerVertical"
            ) || def.category == "Coupling";
            if !is_decoupler {
                continue;
            }
            // drop the decoupler's subtree (children side) and the decoupler itself
            let mut q = VecDeque::from([pi]);
            while let Some(x) = q.pop_front() {
                if !alive[x] {
                    continue;
                }
                alive[x] = false;
                running.remove(&x);
                for c in craft.children(x) {
                    q.push_back(c);
                }
            }
        }
        // 2. ignite engines in this stage
        for &pi in &craft.stages[stage_id] {
            if alive[pi] && def_of(pi).is_some_and(|d| !d.engines.is_empty()) {
                running.insert(pi);
            }
        }
        let start_mass = total_mass(&alive, &stored);
        if running.is_empty() {
            stages_out.push(StageDeltaV {
                alive: (0..n).filter(|&i| alive[i]).collect(),
                stage_id,
                start_mass_t: start_mass,
                end_mass_t: start_mass,
                delta_v_vacuum_mps: 0.0,
                burn_time_s: 0.0,
                thrust_kn: 0.0,
                engines: Vec::new(),
            });
            continue;
        }
        // 3. burn: each running engine draws its mixture from containers reachable over crossfeed
        let mut thrust_sum = 0.0;
        let mut flow_sum = 0.0; // t/s
        let mut demand: Vec<(usize, Vec<(String, f64)>)> = Vec::new(); // engine, (resource, t/s of that resource)
        for &e in &running {
            let def = def_of(e).expect("engine def");
            for mode in def.engines.iter().take(1) {
                let isp = mode.isp_curve.evaluate(0.0);
                if isp <= 0.0 || mode.max_thrust_kn <= 0.0 {
                    continue;
                }
                let flow = mode.max_thrust_kn / (isp * G0); // t/s
                thrust_sum += mode.max_thrust_kn;
                flow_sum += flow;
                let ings = cat.ingredients(mode.propellant_mixture.as_str());
                // mass fraction of each ingredient in the mixture
                let per_unit_mass: f64 = ings
                    .iter()
                    .map(|(r, u)| u * cat.unit_mass.get(r).copied().unwrap_or(0.0))
                    .sum();
                let mut d = Vec::new();
                for (r, u) in ings {
                    let frac = if per_unit_mass > 0.0 {
                        u * cat.unit_mass.get(r).copied().unwrap_or(0.0) / per_unit_mass
                    } else {
                        0.0
                    };
                    d.push((r.to_string(), flow * frac));
                }
                demand.push((e, d));
            }
        }
        // reachable containers per engine
        let reach = |from: usize, alive: &[bool]| -> Vec<usize> {
            let mut seen = vec![false; n];
            let mut out = Vec::new();
            let mut q = VecDeque::from([from]);
            seen[from] = true;
            while let Some(x) = q.pop_front() {
                out.push(x);
                for &y in &adj[x] {
                    if !seen[y] && alive[y] && crossfeed_ok(x) && crossfeed_ok(y) {
                        seen[y] = true;
                        q.push_back(y);
                    }
                }
                for &(y, line) in &lines[x] {
                    if !seen[y] && alive[y] && alive[line] {
                        seen[y] = true;
                        q.push_back(y);
                    }
                }
            }
            out
        };
        // burn time until the first engine starves of any ingredient
        let mut burn_time = f64::INFINITY;
        let mut per_engine_reach: Vec<(usize, Vec<usize>)> = Vec::new();
        for (e, d) in &demand {
            let r = reach(*e, &alive);
            for (res, rate_t) in d {
                let um = cat.unit_mass.get(res.as_str()).copied().unwrap_or(0.0);
                if um <= 0.0 || *rate_t <= 0.0 {
                    continue;
                }
                let avail_t: f64 = r
                    .iter()
                    .map(|&i| {
                        stored[i]
                            .iter()
                            .filter(|(k, _)| resource_key_eq(k, res))
                            .map(|(_, u)| u * um)
                            .sum::<f64>()
                    })
                    .sum();
                burn_time = burn_time.min(avail_t / rate_t);
            }
            per_engine_reach.push((*e, r));
        }
        if !burn_time.is_finite() {
            burn_time = 0.0;
        }
        // consume proportionally from reachable containers (same fraction from each, like the original's
        // group draining) for the burn duration
        for (e, d) in &demand {
            let r = &per_engine_reach
                .iter()
                .find(|(x, _)| x == e)
                .expect("reach")
                .1;
            for (res, rate_t) in d {
                let um = cat.unit_mass.get(res.as_str()).copied().unwrap_or(0.0);
                if um <= 0.0 {
                    continue;
                }
                let need_units = rate_t * burn_time / um;
                let total_units: f64 = r
                    .iter()
                    .map(|&i| {
                        stored[i]
                            .iter()
                            .filter(|(k, _)| resource_key_eq(k, res))
                            .map(|(_, u)| *u)
                            .sum::<f64>()
                    })
                    .sum();
                if total_units <= 0.0 {
                    continue;
                }
                let frac = (need_units / total_units).min(1.0);
                for &i in r {
                    for (k, u) in stored[i].iter_mut() {
                        if resource_key_eq(k, res) {
                            *u *= 1.0 - frac;
                        }
                    }
                }
            }
        }
        let end_mass = total_mass(&alive, &stored);
        let isp_eff = if flow_sum > 0.0 {
            thrust_sum / (flow_sum * G0)
        } else {
            0.0
        };
        let dv = if end_mass > 0.0 && start_mass > end_mass {
            isp_eff * G0 * (start_mass / end_mass).ln()
        } else {
            0.0
        };
        let mut engines: Vec<usize> = running.iter().copied().collect();
        engines.sort_unstable();
        stages_out.push(StageDeltaV {
            alive: (0..n).filter(|&i| alive[i]).collect(),
            stage_id,
            start_mass_t: start_mass,
            end_mass_t: end_mass,
            delta_v_vacuum_mps: dv,
            burn_time_s: burn_time,
            thrust_kn: thrust_sum,
            engines,
        });
    }
    let total = stages_out.iter().map(|s| s.delta_v_vacuum_mps).sum();
    DeltaVReport {
        stages: stages_out,
        total_vacuum_mps: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::craft::{CraftPart, Quat, StoredResource};
    use crate::{AttachRules, Curve, CurveKey, EngineMode};
    use ose_math::DVec3;

    fn part(
        id: &str,
        mass: f64,
        crossfeed: bool,
        stage_type: &str,
        engine: Option<(f64, f64, &str)>,
    ) -> PartDef {
        PartDef {
            flight_geometry: Default::default(),
            id: id.into(),
            title_key: id.into(),
            category: if stage_type.starts_with("Decoupler") {
                "Coupling".into()
            } else {
                "x".into()
            },
            family: String::new(),
            size_category: String::new(),
            stage_type: stage_type.into(),
            mass_t: mass,
            mass_offset_m: DVec3::ZERO,
            pressure_offset_m: DVec3::ZERO,
            lift_offset_m: DVec3::ZERO,
            drag_properties: Default::default(),
            cost: 0.0,
            crew_capacity: 0,
            max_temp_k: 0.0,
            crash_tolerance: 0.0,
            breaking_force: 0.0,
            breaking_torque: 0.0,
            fuel_cross_feed: crossfeed,
            physics_mode: String::new(),
            attach_rules: AttachRules::default(),
            attach_nodes: vec![],
            resource_containers: vec![],
            engines: engine
                .map(|(thrust, isp, mix)| {
                    vec![EngineMode {
                        thrust_transforms: Vec::new(),
                        id: "e".into(),
                        engine_type: "x".into(),
                        propellant_mixture: mix.into(),
                        min_thrust_kn: 0.0,
                        max_thrust_kn: thrust,
                        isp_curve: Curve {
                            keys: vec![CurveKey {
                                t: 0.0,
                                v: isp,
                                in_tangent: 0.0,
                                out_tangent: 0.0,
                            }],
                        },
                        throttle_locked: false,
                        allow_restart: true,
                        allow_shutdown: true,
                        gimbal_range_deg: None,
                        air_breathing: false,
                        vel_curve: None,
                        atm_curve: None,
                    }]
                })
                .unwrap_or_default(),
            drag_cubes: vec![],
            bounds: None,
            reaction_wheel_torque_knm: 0.0,
            reaction_wheel_axes_knm: None,
            electric: Default::default(),
            experiments: vec![],
            docking: None,
            parachute: None,
            thermal_mass_modifier: 1.0,
            heatshield: None,
            landing_gear: None,
            converter: None,
            ejection_force: 0.0,
            fairing: false,
            cargo_bay: false,
            rcs: None,
            jetpack: None,
            lifting_surface: None,
            ctrl_range_deg: 0.0,
            ctrl_area: 0.0,
            ctrl_pressure_offset_m: DVec3::ZERO,
            intake_area_m2: 0.0,
            module_names: vec![],
            raw_modules: Default::default(),
        }
    }

    fn cp(
        guid: &str,
        part: &str,
        parent: Option<usize>,
        stage: i32,
        res: &[(&str, f64)],
    ) -> CraftPart {
        CraftPart {
            flight: None,
            colors: None,
            guid: guid.into(),
            part_id: part.into(),
            parent,
            own_node: None,
            parent_node: None,
            local_position: DVec3::ZERO,
            local_rotation: Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            stage_index: stage,
            fuel_crossfeed: true,
            resources: res
                .iter()
                .map(|(r, u)| StoredResource {
                    resource: r.to_string(),
                    stored_units: *u,
                    capacity_units: *u,
                })
                .collect(),
            fuel_line_target: None,
        }
    }

    #[test]
    fn a_fuel_line_feeds_the_lower_engine_from_the_upper_tank() {
        let parts = vec![
            part("pod", 1.0, true, "None", None),
            part("tank", 0.5, true, "None", None),
            part(
                "eng",
                0.5,
                true,
                "LiquidEngine",
                Some((60.0, 300.0, "Fuel")),
            ),
            part("dec", 0.1, false, "DecouplerVertical", None),
            part("line", 0.05, true, "None", None),
        ];
        let resources = vec![ResourceDef {
            id: "Fuel".into(),
            display_name_key: String::new(),
            abbreviation_key: String::new(),
            kind: ResourceKind::Basic {
                mass_per_unit_t: 0.005,
                volume_per_unit: 1.0,
                specific_heat_capacity_per_unit: 0.0,
                flow_mode: crate::FlowMode::StackPrioritySearch,
                pumpable: true,
                cost_per_unit: 0.0,
                non_stageable: false,
            },
        }];
        let mut craft = Craft {
            colors: None,
            loose: Vec::new(),
            name: "lined".into(),
            description: String::new(),
            parts: vec![
                cp("a", "pod", None, -1, &[]),
                cp("b", "tank", Some(0), -1, &[("Fuel", 400.0)]),
                cp("c", "eng", Some(1), 1, &[]),
                cp("d", "dec", Some(2), 1, &[]),
                cp("e", "tank", Some(3), -1, &[("Fuel", 800.0)]),
                cp("f", "eng", Some(4), 2, &[]),
                cp("g", "line", Some(1), -1, &[]),
            ],
            stages: vec![vec![], vec![2, 3], vec![5]],
        };
        let lower = |c: &Craft| delta_v_by_stage(c, &parts, &resources).stages[0].end_mass_t;
        // without the line the decoupler keeps the upper tank to itself
        let alone = lower(&craft);
        assert!((alone - 5.15).abs() < 1e-9, "{alone}");
        // the line from the upper tank to the lower one: the lower engine burns the upper tank too
        craft.parts[6].fuel_line_target = Some(4);
        let fed = lower(&craft);
        assert!((fed - 3.15).abs() < 1e-9, "burns all 6 t: {fed}");
    }

    #[test]
    fn two_stage_rocket_matches_tsiolkovsky_per_stage() {
        let parts = vec![
            part("pod", 1.0, true, "None", None),
            part("tank", 0.5, true, "None", None),
            part(
                "eng",
                0.5,
                true,
                "LiquidEngine",
                Some((60.0, 300.0, "Fuel")),
            ),
            part("dec", 0.1, false, "DecouplerVertical", None),
        ];
        let resources = vec![ResourceDef {
            id: "Fuel".into(),
            display_name_key: String::new(),
            abbreviation_key: String::new(),
            kind: ResourceKind::Basic {
                mass_per_unit_t: 0.005,
                volume_per_unit: 1.0,
                specific_heat_capacity_per_unit: 0.0,
                flow_mode: crate::FlowMode::StackPrioritySearch,
                pumpable: true,
                cost_per_unit: 0.0,
                non_stageable: false,
            },
        }];
        // pod(0) - tank(1, 400 units = 2 t) - eng(2) - dec(3) - tank(4, 800 units = 4 t) - eng(5)
        let craft = Craft {
            colors: None,
            loose: Vec::new(),
            name: "two".into(),
            description: String::new(),
            parts: vec![
                cp("a", "pod", None, -1, &[]),
                cp("b", "tank", Some(0), -1, &[("Fuel", 400.0)]),
                cp("c", "eng", Some(1), 1, &[]),
                cp("d", "dec", Some(2), 1, &[]),
                cp("e", "tank", Some(3), -1, &[("Fuel", 800.0)]),
                cp("f", "eng", Some(4), 2, &[]),
            ],
            // stage 2: lower engine; stage 1: decoupler + upper engine; stage 0: nothing
            stages: vec![vec![], vec![2, 3], vec![5]],
        };
        let r = delta_v_by_stage(&craft, &parts, &resources);
        assert_eq!(r.stages.len(), 3);
        let s2 = &r.stages[0];
        assert_eq!(s2.stage_id, 2);
        // total mass: dry 1+0.5+0.5+0.1+0.5+0.5 = 3.1, fuel 6 t -> 9.1 t; lower stage burns its 4 t
        // (decoupler blocks crossfeed), so end mass 5.1 t
        assert!((s2.start_mass_t - 9.1).abs() < 1e-9, "{}", s2.start_mass_t);
        assert!((s2.end_mass_t - 5.1).abs() < 1e-9, "{}", s2.end_mass_t);
        let expect2 = 300.0 * G0 * (9.1f64 / 5.1).ln();
        assert!((s2.delta_v_vacuum_mps - expect2).abs() < 1e-6);
        let s1 = &r.stages[1];
        assert_eq!(s1.stage_id, 1);
        // after jettison: pod 1 + tank 0.5 + eng 0.5 + 2 t fuel = 4.0 -> 2.0
        assert!((s1.start_mass_t - 4.0).abs() < 1e-9, "{}", s1.start_mass_t);
        assert!((s1.end_mass_t - 2.0).abs() < 1e-9, "{}", s1.end_mass_t);
        assert!((s1.delta_v_vacuum_mps - 300.0 * G0 * 2f64.ln()).abs() < 1e-6);
        assert_eq!(r.stages[2].delta_v_vacuum_mps, 0.0);
        assert!((r.total_vacuum_mps - (s1.delta_v_vacuum_mps + expect2)).abs() < 1e-9);
        // burn time of the lower stage: 4 t at 60 kN / (300 g0) t/s
        let flow = 60.0 / (300.0 * G0);
        assert!((s2.burn_time_s - 4.0 / flow).abs() < 1e-6);
    }
}

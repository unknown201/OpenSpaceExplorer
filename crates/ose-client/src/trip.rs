//! Trip planner (`KSP.game/TripPlanner.cs`): the original's delta-v map between the surfaces, orbits and
//! intercepts of every body (assets/ui/trip_planner.json, exported by tools/export_trip_planner.py), summed
//! along the one path between two places.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TripMap {
    pub trip_nodes: Vec<TripNode>,
    pub trip_edges: Vec<TripEdge>,
    #[serde(default)]
    pub trip_angles: Vec<TripAngle>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TripNode {
    pub name: String,
    pub reference_body: String,
    pub node_type: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TripEdge {
    pub start_node: String,
    pub end_node: String,
    pub cost: f64,
    #[serde(default)]
    pub inclination_cost: f64,
    #[serde(default)]
    pub can_aerobrake: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TripAngle {
    pub start_body: String,
    pub end_body: String,
    pub phase_angle: f64,
    pub ejection_angle: f64,
}

/// The budget for a trip: delta-v there, the plane-change allowance, the part of it the atmosphere can take
/// off on the way down (aerobraking), and the transfer window when it crosses between planets.
#[derive(Clone, Debug, PartialEq)]
pub struct TripPlan {
    pub delta_v_mps: f64,
    pub inclination_mps: f64,
    pub aerobrake_mps: f64,
    pub phase_angle: Option<f64>,
}

impl TripMap {
    pub fn parse(text: &str) -> Option<TripMap> {
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
    }

    /// Readable name of a node: `Mun low orbit`.
    pub fn label(&self, name: &str) -> String {
        let Some(n) = self.trip_nodes.iter().find(|n| n.name == name) else {
            return name.to_string();
        };
        let kind = match n.node_type.as_str() {
            "Surface" => "surface",
            "LowOrbit" => "low orbit",
            "Stationary" => "stationary orbit",
            "Capture" => "capture",
            "Intercept" => "intercept",
            other => other,
        };
        format!("{} {kind}", n.reference_body)
    }

    /// Places worth starting or ending at: surfaces and orbits (intercepts and captures are waypoints).
    pub fn places(&self) -> Vec<&TripNode> {
        self.trip_nodes
            .iter()
            .filter(|n| matches!(n.node_type.as_str(), "Surface" | "LowOrbit" | "Stationary"))
            .collect()
    }

    /// The trip from `from` to `to` (node names), there and back again when `round_trip`.
    pub fn plan(&self, from: &str, to: &str, round_trip: bool) -> Option<TripPlan> {
        // The map is a tree: walk from `from` remembering how each node was reached.
        let mut adj: HashMap<&str, Vec<(&str, &TripEdge, bool)>> = HashMap::new();
        for e in &self.trip_edges {
            adj.entry(&e.start_node)
                .or_default()
                .push((&e.end_node, e, true));
            adj.entry(&e.end_node)
                .or_default()
                .push((&e.start_node, e, false));
        }
        let mut came: HashMap<&str, (&str, &TripEdge, bool)> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([from]);
        let mut seen = std::collections::HashSet::from([from]);
        while let Some(n) = queue.pop_front() {
            if n == to {
                break;
            }
            for (m, e, forward) in adj.get(n).into_iter().flatten() {
                if seen.insert(m) {
                    came.insert(m, (n, e, *forward));
                    queue.push_back(m);
                }
            }
        }
        if from != to && !came.contains_key(to) {
            return None;
        }
        let mut plan = TripPlan {
            delta_v_mps: 0.0,
            inclination_mps: 0.0,
            aerobrake_mps: 0.0,
            phase_angle: None,
        };
        let mut n = to;
        while n != from {
            let (prev, e, forward) = came[n];
            plan.delta_v_mps += e.cost;
            plan.inclination_mps += e.inclination_cost;
            // Aerobraking helps going down into an atmosphere (toward the edge's start, the lower node).
            if e.can_aerobrake && !forward {
                plan.aerobrake_mps += e.cost;
            }
            n = prev;
        }
        if round_trip {
            if let Some(back) = self.plan(to, from, false) {
                plan.delta_v_mps += back.delta_v_mps;
                plan.inclination_mps += back.inclination_mps;
                plan.aerobrake_mps += back.aerobrake_mps;
            }
        }
        let body = |name: &str| {
            self.trip_nodes
                .iter()
                .find(|x| x.name == name)
                .map(|x| x.reference_body.clone())
                .unwrap_or_default()
        };
        let (a, b) = (body(from), body(to));
        plan.phase_angle = self
            .trip_angles
            .iter()
            .find(|t| t.start_body == a && t.end_body == b)
            .map(|t| t.phase_angle);
        Some(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = r#"{"TripNodes":[
        {"Name":"kerbin-surf","ReferenceBody":"Kerbin","NodeType":"Surface"},
        {"Name":"kerbin-low","ReferenceBody":"Kerbin","NodeType":"LowOrbit"},
        {"Name":"mun-int","ReferenceBody":"Mun","NodeType":"Intercept"},
        {"Name":"mun-low","ReferenceBody":"Mun","NodeType":"LowOrbit"},
        {"Name":"mun-surf","ReferenceBody":"Mun","NodeType":"Surface"}],
      "TripEdges":[
        {"StartNode":"kerbin-surf","EndNode":"kerbin-low","Cost":3400,"InclinationCost":0,"CanAerobrake":true},
        {"StartNode":"kerbin-low","EndNode":"mun-int","Cost":860,"InclinationCost":0,"CanAerobrake":false},
        {"StartNode":"mun-int","EndNode":"mun-low","Cost":310,"InclinationCost":0,"CanAerobrake":false},
        {"StartNode":"mun-low","EndNode":"mun-surf","Cost":580,"InclinationCost":0,"CanAerobrake":false}],
      "TripAngles":[]}"#;

    #[test]
    fn mun_landing_and_back() {
        let m = TripMap::parse(MAP).unwrap();
        let p = m.plan("kerbin-surf", "mun-surf", false).unwrap();
        assert_eq!(p.delta_v_mps, 3400.0 + 860.0 + 310.0 + 580.0);
        assert_eq!(p.aerobrake_mps, 0.0);
        let r = m.plan("kerbin-surf", "mun-surf", true).unwrap();
        assert_eq!(r.delta_v_mps, 2.0 * p.delta_v_mps);
        // On the way home, the drop from orbit to Kerbin's surface can be aerobraked.
        assert_eq!(r.aerobrake_mps, 3400.0);
        assert_eq!(m.label("mun-low"), "Mun low orbit");
        assert_eq!(m.places().len(), 4);
        assert!(m.plan("kerbin-surf", "nowhere", false).is_none());
    }
}

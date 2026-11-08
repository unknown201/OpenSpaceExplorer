//! CommNet as the original builds it (`GetConnectedNodesJob`, `TelemetryComponent`): every vessel with an
//! antenna is a node with the range of its best antenna, the KSC is the source with
//! `KSC_COMM_NET_RANGE_METERS` (8e11 m), and two nodes link when each is within the other's range (bodies do
//! not block the line). A vessel is connected when a chain of links reaches the KSC; every vessel relays.
//! The route is the original's: Dijkstra from the KSC with each link costing its squared length, so a chain
//! of short links wins over one long one. The graph is rebuilt every `REBUILD_S` of real time
//! (`CommNetManager.REBUILD_GRAPH_TIMER_SECONDS`), so a link is lost or found up to 3 s late.
//! Without a connection probe cores that need one cannot be flown (with the `CommNetRequired` difficulty
//! option) and reports cannot be transmitted.

use bevy_ecs::prelude::*;
use ose_math::DVec3;

use crate::{bodies, crew, flight, structure, Universe};

/// `PhysicsSettings._kscCommNetRangeMeters`.
pub const KSC_RANGE_M: f64 = 8.0e11;
/// `CommNetManager.REBUILD_GRAPH_TIMER_SECONDS`: how often the graph is rebuilt, s of real time.
pub const REBUILD_S: f64 = 3.0;

/// The ground station: a body-fixed site and its range.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GroundStation {
    pub body: Entity,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub altitude_m: f64,
    pub range_m: f64,
}

/// Whether the `CommNetRequired` difficulty option is on.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct CommNetRequired(pub bool);

/// A vessel's CommNet state, refreshed every step.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct CommLink {
    pub connected: bool,
    /// Links on the way to the KSC (1: in direct contact).
    pub hops: u32,
    pub range_m: f64,
}

impl Universe {
    pub fn set_ground_station(&mut self, station: GroundStation) {
        self.world.insert_resource(station);
    }

    pub fn set_commnet_required(&mut self, on: bool) {
        self.world.insert_resource(CommNetRequired(on));
    }

    pub fn comm_link(&self, e: Entity) -> CommLink {
        self.world.get::<CommLink>(e).copied().unwrap_or_default()
    }

    /// Star-space position of a vessel now.
    fn star_position(&self, e: Entity, ut: f64) -> Option<DVec3> {
        let body = self.world.get::<bodies::ReferenceBody>(e)?.0;
        Some(bodies::star_space_state(&self.world, body, ut).0 + self.position_of(e)?)
    }

    /// Antenna range of a vessel: its best attached antenna, a deployable one only while extended
    /// (`PartComponentModule_DataTransmitter.IsTransmitterActive`); EVA Kerbals carry none and do not relay.
    fn antenna_range(&self, e: Entity) -> f64 {
        self.world
            .get::<structure::Structure>(e)
            .map(|st| {
                (0..st.rigid.len())
                    .filter(|&i| st.rigid.present[i])
                    .filter(|&i| !st.antenna_deployable[i] || st.deployed[i])
                    .map(|i| st.electric[i].comm_range_m)
                    .fold(0.0, f64::max)
            })
            .unwrap_or(0.0)
    }

    /// Rebuild the network and every vessel's link.
    pub fn update_commnet(&mut self) {
        let ut = self.ut();
        let Some(station) = self.world.get_resource::<GroundStation>().cloned() else {
            return;
        };
        let Some(b) = self.world.get::<bodies::Body>(station.body).cloned() else {
            return;
        };
        let (r_site, _) =
            flight::surface_state_at(&b, station.lat_deg, station.lon_deg, station.altitude_m, ut);
        let ksc = bodies::star_space_state(&self.world, station.body, ut).0 + r_site;
        let vessels = self.vessels();
        let mut nodes: Vec<(Option<Entity>, DVec3, f64)> = vec![(None, ksc, station.range_m)];
        for &e in &vessels {
            if self.world.get::<crew::EvaKerbal>(e).is_some() {
                continue;
            }
            let range = self.antenna_range(e);
            if range <= 0.0 {
                continue;
            }
            if let Some(p) = self.star_position(e, ut) {
                nodes.push((Some(e), p, range));
            }
        }
        // breadth first from the KSC: a link needs each end within the other's range
        let hops = route_hops(&nodes.iter().map(|n| (n.1, n.2)).collect::<Vec<_>>());
        for &e in &vessels {
            let link = match nodes.iter().position(|nd| nd.0 == Some(e)) {
                Some(k) => CommLink {
                    connected: hops[k] != u32::MAX,
                    hops: hops[k].min(u32::MAX - 1),
                    range_m: nodes[k].2,
                },
                None => CommLink::default(),
            };
            self.world.entity_mut(e).insert(link);
        }
    }

    /// Whether a vessel's probe cores are cut off: they need CommNet, the option is on, no crew and no link.
    pub fn commnet_blocks_control(&self, e: Entity) -> bool {
        if !self
            .world
            .get_resource::<CommNetRequired>()
            .is_some_and(|r| r.0)
        {
            return false;
        }
        let Some(st) = self.world.get::<structure::Structure>(e) else {
            return false;
        };
        let needs = (0..st.rigid.len())
            .any(|i| st.rigid.present[i] && st.electric[i].command_requires_commnet);
        needs && self.crew(e).is_empty() && !self.comm_link(e).connected
    }
}

/// `GetConnectedNodesJob`: Dijkstra from node 0 (the KSC) where two nodes link when each is within the
/// other's range and a link costs its squared length; the hops along each node's cheapest route
/// (`u32::MAX` when it has none).
pub fn route_hops(nodes: &[(DVec3, f64)]) -> Vec<u32> {
    let n = nodes.len();
    let mut cost = vec![f64::MAX; n];
    let mut hops = vec![u32::MAX; n];
    let mut done = vec![false; n];
    if n == 0 {
        return hops;
    }
    cost[0] = 0.0;
    hops[0] = 0;
    let mut open: Vec<usize> = (0..n).collect();
    while !open.is_empty() {
        let (k, &i) = open
            .iter()
            .enumerate()
            .min_by(|a, b| cost[*a.1].total_cmp(&cost[*b.1]))
            .expect("open");
        open.swap_remove(k);
        done[i] = true;
        if cost[i] == f64::MAX {
            continue;
        }
        let reach = nodes[i].1 * nodes[i].1;
        for j in 0..n {
            if j == i || done[j] {
                continue;
            }
            let d2 = (nodes[i].0 - nodes[j].0).length_squared();
            if d2 < reach && d2 < nodes[j].1 * nodes[j].1 && cost[i] + d2 < cost[j] {
                cost[j] = cost[i] + d2;
                hops[j] = hops[i] + 1;
            }
        }
    }
    hops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_route_is_the_cheapest_chain_of_squared_lengths_not_the_fewest_hops() {
        // KSP2's GetConnectedNodesJob: B can hear the KSC directly (1,900 m, cost 3.61e6 m^2), but through A
        // (1,000 m then 900 m, cost 1.81e6 m^2) is cheaper, so its route has two hops
        let nodes = [
            (DVec3::ZERO, KSC_RANGE_M),
            (DVec3::new(1000.0, 0.0, 0.0), 1500.0),
            (DVec3::new(1900.0, 0.0, 0.0), 2000.0),
            (DVec3::new(9000.0, 0.0, 0.0), 2000.0),
        ];
        assert_eq!(route_hops(&nodes), vec![0, 1, 2, u32::MAX]);
        // a link needs each end within the other's range
        let one_way = [(DVec3::ZERO, 10.0), (DVec3::new(50.0, 0.0, 0.0), 100.0)];
        assert_eq!(route_hops(&one_way), vec![0, u32::MAX]);
    }

    #[test]
    fn a_lost_link_shows_at_the_next_rebuild_three_seconds_on() {
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
        kerbin.parent = None;
        kerbin.orbit = None;
        kerbin.soi_radius_m = None;
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        let drone = crafts.iter().find(|c| c.name == "Stock03_Drone").unwrap();
        let mut u = Universe::with_bodies(&[kerbin]);
        let body = u.body_entity("Kerbin").unwrap();
        u.set_ground_station(GroundStation {
            body,
            lat_deg: -0.6274,
            lon_deg: -74.8305,
            altitude_m: 221.7,
            range_m: KSC_RANGE_M,
        });
        let probe = u.spawn_vessel_in_circular_orbit("probe", body, 100_000.0);
        u.set_structure(probe, structure::Structure::build(drone, &defs, &res));
        u.step();
        assert!(u.comm_link(probe).connected);
        // carried far beyond its antenna's range: the link holds until the graph is rebuilt
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let r = 4.0e8;
        let far = ose_math::Orbit::from_state_vectors(
            mu,
            DVec3::new(r, 0.0, 0.0),
            DVec3::new(0.0, (mu / r).sqrt(), 0.0),
            u.ut(),
        );
        u.world.entity_mut(probe).insert(bodies::OnRails(far));
        let mut lost_after = None;
        for i in 1..=(4 * crate::TICKS_PER_SECOND) {
            u.step();
            if lost_after.is_none() && !u.comm_link(probe).connected {
                lost_after = Some(i as f64 / crate::TICKS_PER_SECOND as f64);
            }
        }
        let t = lost_after.expect("the link is lost");
        assert!(
            t > 2.5 && t <= 3.0 + 1e-9,
            "lost after {t} s (KSP2 rebuilds every 3 s)"
        );
    }

    #[test]
    fn a_probe_near_kerbin_hears_the_ksc_and_one_far_away_does_not() {
        let bodies: Vec<ose_data::CelestialBody> =
            serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
        let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
        kerbin.parent = None;
        kerbin.orbit = None;
        kerbin.soi_radius_m = None;
        let crafts: Vec<ose_data::craft::Craft> =
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
        let defs: Vec<ose_data::PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let res: Vec<ose_data::ResourceDef> =
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
        // the drone: a probe core with the original's 200,000 km antenna built in
        let drone = crafts.iter().find(|c| c.name == "Stock03_Drone").unwrap();
        let mut u = Universe::with_bodies(&[kerbin]);
        let body = u.body_entity("Kerbin").unwrap();
        u.set_ground_station(GroundStation {
            body,
            lat_deg: -0.6274,
            lon_deg: -74.8305,
            altitude_m: 221.7,
            range_m: KSC_RANGE_M,
        });
        u.set_commnet_required(true);
        let near = u.spawn_vessel_in_circular_orbit("near", body, 100_000.0);
        u.set_structure(near, structure::Structure::build(drone, &defs, &res));
        let far = u.spawn_vessel_in_circular_orbit("far", body, 4.0e8);
        u.set_structure(far, structure::Structure::build(drone, &defs, &res));
        u.update_commnet();
        assert!(u.comm_link(near).connected);
        assert_eq!(u.comm_link(near).hops, 1);
        assert!(
            !u.comm_link(far).connected,
            "beyond its antenna's 200,000 km"
        );
        assert!(u.commnet_blocks_control(far));
        assert!(!u.commnet_blocks_control(near));
        // a report goes home packet by packet from the near drone; the far one waits for a link
        let report = ose_data::science::ResearchReport {
            experiment: "CrewReport".into(),
            sample: false,
            location: "Kerbin_InSpaceLow".into(),
            value: 8.0,
        };
        for v in [near, far] {
            u.set_science_carried(v, vec![report.clone()]);
            assert_eq!(u.start_transmission(v), Ok(1));
        }
        let mut arrived = Vec::new();
        for _ in 0..(20 * crate::TICKS_PER_SECOND) {
            u.step();
            arrived.extend(u.take_transmitted());
        }
        assert_eq!(arrived.len(), 1, "{arrived:?}");
        assert_eq!(arrived[0].0, near);
        assert_eq!(
            u.transmission(far).map(|t| t.3),
            Some(Some("no CommNet connection to the KSC"))
        );
        assert!(u.science_carried(near).is_empty());
        // the truck's whip antennas fold out: folded, they reach nobody
        let truck = crafts.iter().find(|c| c.name == "Stock09_Truck").unwrap();
        let t = u.spawn_vessel_in_circular_orbit("truck", body, 100_000.0);
        let mut st = structure::Structure::build(truck, &defs, &res);
        // leave out the cab's built-in antenna
        for i in 0..st.electric.len() {
            if !st.antenna_deployable[i] {
                st.electric[i].comm_range_m = 0.0;
            }
        }
        u.set_structure(t, st);
        assert_eq!(u.antenna_range(t), 0.0, "antennas folded");
        assert_eq!(u.deploy_antennas(t, true), Some(2));
        assert!(u.antenna_range(t) > 0.0, "antennas out");
    }
}

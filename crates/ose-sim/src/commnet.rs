//! CommNet as the original builds it (`GetConnectedNodesJob`, `TelemetryComponent`): every vessel with an
//! antenna is a node with the range of its best antenna, the KSC is the source with
//! `KSC_COMM_NET_RANGE_METERS` (8e11 m), and two nodes link when each is within the other's range (bodies do
//! not block the line). A vessel is connected when a chain of links reaches the KSC; every vessel relays.
//! Without a connection probe cores that need one cannot be flown (with the `CommNetRequired` difficulty
//! option) and reports cannot be transmitted.

use bevy_ecs::prelude::*;
use ose_math::DVec3;
use std::collections::VecDeque;

use crate::{bodies, crew, flight, structure, Universe};

/// `PhysicsSettings._kscCommNetRangeMeters`.
pub const KSC_RANGE_M: f64 = 8.0e11;

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

    /// Antenna range of a vessel: its best attached antenna (EVA Kerbals carry none and do not relay).
    fn antenna_range(&self, e: Entity) -> f64 {
        self.world
            .get::<structure::Structure>(e)
            .map(|st| {
                (0..st.rigid.len())
                    .filter(|&i| st.rigid.present[i])
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
        let n = nodes.len();
        let mut hops = vec![u32::MAX; n];
        hops[0] = 0;
        let mut q = VecDeque::from([0usize]);
        while let Some(i) = q.pop_front() {
            for j in 0..n {
                if hops[j] != u32::MAX {
                    continue;
                }
                let d2 = (nodes[i].1 - nodes[j].1).length_squared();
                if d2 < nodes[i].2 * nodes[i].2 && d2 < nodes[j].2 * nodes[j].2 {
                    hops[j] = hops[i] + 1;
                    q.push_back(j);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}

//! Docking, undocking and the vessel registry: the sim joins and splits the vessels (`ose_sim::docking`); here
//! the registry follows and the owners hear about it.

use bevy_ecs::entity::Entity;
use ose_net::ServerMessage;

use crate::{flight_spec_for, Shared};

/// Join every pair of vessels whose ports latch now. Returns messages for the owners.
pub fn dock_latched(s: &Shared) -> Vec<(u64, ServerMessage)> {
    let mut out = Vec::new();
    let joined = {
        let mut u = s.universe.lock().expect("sim lock");
        u.dock_latched(&s.parts, &s.resources, &|c| flight_spec_for(s, c))
    };
    if joined.is_empty() {
        return out;
    }
    let mut list = s.vessels.lock().expect("vessels lock");
    for (kept, absorbed) in joined {
        let ida = list.iter().find(|x| x.0 == kept).map(|x| (x.1, x.2));
        let idb = list.iter().find(|x| x.0 == absorbed).map(|x| (x.1, x.2));
        list.retain(|x| x.0 != absorbed);
        if let (Some((ida, oa)), Some((idb, ob))) = (ida, idb) {
            tracing::info!(vessel = ida, absorbed = idb, "docked");
            let mut owners = vec![oa, ob];
            owners.dedup();
            for owner in owners.into_iter().flatten() {
                out.push((
                    owner,
                    ServerMessage::Docked {
                        vessel: ida,
                        absorbed: idb,
                    },
                ));
            }
        }
    }
    out
}

/// Undock at a docked joint; returns the new vessel's entity.
pub fn undock(s: &Shared, e: Entity, port: Option<usize>) -> Result<Entity, String> {
    s.universe
        .lock()
        .expect("sim lock")
        .undock(e, port, &s.parts, &s.resources, &|c| flight_spec_for(s, c))
}

/// Register a new vessel for `owner`; returns its id.
pub fn register(s: &Shared, e: Entity, owner: Option<u64>) -> u64 {
    let mut list = s.vessels.lock().expect("vessels lock");
    let id = list.iter().map(|&(_, id, _)| id).max().unwrap_or(0) + 1;
    list.push((e, id, owner));
    id
}

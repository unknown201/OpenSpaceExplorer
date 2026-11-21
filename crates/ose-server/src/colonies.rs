//! Colonies on the server (`docs/colonies-design.md`, step 1). The state is `ose_colonies::Colonies`, kept
//! in the campaign (`save::Campaign::colonies`), so the save, the quicksave and revert carry it. A colony is
//! a site: the vessels landed round it are its stock and its residents, read from the universe whenever they
//! are needed. Each colony adds a launch pad to the launch sites (`Colony_<id>`); a craft launched there is
//! paid for out of the colony's tanks and crewed by its residents (`launch_on_pad_now`).

use bevy_ecs::entity::Entity;
use ose_net::{ClientMessage, ColonyInfo, LaunchSite, ServerMessage};
use ose_sim::Universe;

use crate::{display_name_for, owned_vessel, Shared};

/// The launch site id of a colony's pad.
pub fn site_id(colony: u64) -> String {
    format!("Colony_{colony}")
}

/// Every colony's launch pad, unoccupied.
pub fn sites(s: &Shared, u: &Universe) -> Vec<LaunchSite> {
    let c = s.campaign.lock().expect("campaign lock");
    c.colonies
        .colonies
        .iter()
        .filter_map(|col| {
            let (lat, lon) = ose_colonies::pad(u, col)?;
            Some(LaunchSite {
                id: site_id(col.id),
                name: format!("{} pad ({})", col.name, col.body),
                occupied_by: None,
                occupant: None,
                occupant_name: None,
                runway: false,
                lat_deg: lat,
                lon_deg: lon,
                heading_deg: 90.0,
                body: Some(col.body.clone()),
                colony: Some(col.id),
            })
        })
        .collect()
}

/// The campaign's colonies, with `reason` for a refusal.
pub fn info(s: &Shared, reason: Option<String>) -> ServerMessage {
    // locks in the order the snapshot takes them
    let mut u = s.universe.lock().expect("sim lock");
    let colonies = s
        .campaign
        .lock()
        .expect("campaign lock")
        .colonies
        .colonies
        .clone();
    let found: Vec<_> = colonies
        .iter()
        .map(|c| {
            (
                ose_colonies::members(&mut u, c),
                ose_colonies::residents(&mut u, c),
                ose_colonies::stock(&mut u, c),
            )
        })
        .collect();
    drop(u);
    let list = s.vessels.lock().expect("vessels lock").clone();
    let id_of = |e: Entity| list.iter().find(|v| v.0 == e).map(|v| v.1);
    let colonies = colonies
        .iter()
        .zip(found)
        .map(|(c, (members, residents, stock))| ColonyInfo {
            id: c.id,
            name: c.name.clone(),
            owner: c.owner,
            owner_name: display_name_for(s, c.owner),
            body: c.body.clone(),
            lat_deg: c.lat_deg,
            lon_deg: c.lon_deg,
            founded_ut: c.founded_ut,
            launches: c.launches,
            vessels: members.into_iter().filter_map(id_of).collect(),
            residents: residents.into_iter().map(|(_, n)| n).collect(),
            stock: stock
                .into_iter()
                .map(|x| (x.resource, x.stored, x.capacity))
                .collect(),
        })
        .collect();
    ServerMessage::Colonies { colonies, reason }
}

/// Answer one of the colony messages. A founding or an abandonment is told to everyone in the campaign.
pub fn handle(s: &Shared, player: u64, msg: &ClientMessage) -> ServerMessage {
    let result: Result<bool, String> = match *msg {
        ClientMessage::FoundColony { vessel, ref name } => match owned_vessel(s, vessel, player) {
            None => Err("not your vessel".into()),
            Some(e) => {
                let mut u = s.universe.lock().expect("sim lock");
                let mut c = s.campaign.lock().expect("campaign lock");
                let near_ksc = {
                    let ksc = crate::launch_sites("Kerbin");
                    near_home(&u, e, &ksc)
                };
                if near_ksc {
                    Err("too close to the KSC: found colonies away from home".into())
                } else {
                    c.colonies.found(&mut u, e, player, name).map(|id| {
                        tracing::info!(player, vessel, colony = id, name = %name, "colony founded");
                        true
                    })
                }
            }
        },
        ClientMessage::AbandonColony { id } => {
            let mut c = s.campaign.lock().expect("campaign lock");
            c.colonies.abandon(id, player).map(|()| {
                tracing::info!(player, colony = id, "colony abandoned");
                true
            })
        }
        _ => Ok(false),
    };
    let (changed, reason) = match result {
        Ok(changed) => (changed, None),
        Err(why) => (false, Some(why)),
    };
    if changed {
        tell_everyone(s, player);
    }
    info(s, reason)
}

/// Within `KSC_CLEARANCE_M` of the home launch sites.
fn near_home(u: &Universe, e: Entity, ksc: &[LaunchSite]) -> bool {
    const KSC_CLEARANCE_M: f64 = 5_000.0;
    let Some(p) = u.powered(e).or_else(|| u.dormant(e)) else {
        return false;
    };
    let Some(b) = u
        .world
        .get::<ose_sim::bodies::ReferenceBody>(e)
        .map(|b| b.0)
    else {
        return false;
    };
    if u.body_id(b) != Some("Kerbin") {
        return false;
    }
    let Some(body) = u.world.get::<ose_sim::bodies::Body>(b) else {
        return false;
    };
    let here = ose_sim::flight::lat_lon(body, p.r, u.ut());
    ksc.iter().any(|site| {
        ose_colonies::surface_distance(body.radius_m, here, (site.lat_deg, site.lon_deg))
            < KSC_CLEARANCE_M
    })
}

/// Tell the campaign's other connected players that its colonies changed.
fn tell_everyone(s: &Shared, by: u64) {
    let players: Vec<u64> = s
        .warp_requests
        .lock()
        .expect("warp lock")
        .keys()
        .copied()
        .filter(|&p| p != by)
        .collect();
    if players.is_empty() {
        return;
    }
    let msg = info(s, None);
    let mut q = s.pending.lock().expect("pending lock");
    for p in players {
        q.push((p, msg.clone()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn colony_pads_are_named_by_their_colony() {
        assert_eq!(super::site_id(12), "Colony_12");
    }
}

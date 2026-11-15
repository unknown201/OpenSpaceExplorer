//! Several campaigns on one server: one is live (its universe runs), the others wait on the shelf as save
//! files. Creating a campaign or picking another from the list puts the live one on the shelf and brings
//! the chosen one in, as long as nobody else is playing the live one.

use ose_net::{ServerMessage, SessionMode};
use ose_sim::Universe;

use crate::save;
use crate::{Shared, KSC_LAT_DEG, KSC_LON_DEG};

/// What every campaign's universe is built from.
pub struct WorldSetup {
    pub bodies: Vec<ose_data::CelestialBody>,
    pub terrains: Vec<(String, std::sync::Arc<ose_data::terrain::TerrainMap>)>,
    /// The KSC (CommNet source and where the day starts): latitude, longitude, altitude.
    pub ksc: (f64, f64, f64),
    pub kerbal_experiments: Vec<ose_data::science::PartExperiment>,
    pub science: ose_data::science::ScienceCatalog,
    pub regions: Vec<u8>,
}

/// How high the sun stands over the KSC when a new campaign starts, degrees.
pub const MORNING_SUN_DEG: f64 = 15.0;

impl WorldSetup {
    /// A universe at time zero with the bodies, terrain, KSC ground station and science in place.
    pub fn universe(&self) -> Universe {
        let mut u = Universe::with_bodies(&self.bodies);
        for (id, map) in &self.terrains {
            u.set_terrain_shared(id, map.clone());
        }
        if let Some(kerbin) = u.body_entity("Kerbin") {
            let (lat, lon, alt) = self.ksc;
            u.set_ground_station(ose_sim::commnet::GroundStation {
                body: kerbin,
                lat_deg: lat,
                lon_deg: lon,
                altitude_m: alt,
                range_m: ose_sim::commnet::KSC_RANGE_M,
            });
        }
        u.set_kerbal_part(self.kerbal_experiments.clone());
        // craft at the KSC stand on the campus the client draws
        u.set_site("Kerbin", crate::ksc_site());
        u.set_science(self.science.clone(), self.regions.clone());
        u
    }

    /// A universe for a new campaign: the clock starts on the first morning at the KSC.
    pub fn new_campaign_universe(&self) -> Universe {
        let mut u = self.universe();
        if let Some(kerbin) = u.body_entity("Kerbin") {
            let (lat, lon, _) = self.ksc;
            let t = u.next_morning_ut(kerbin, lat, lon, MORNING_SUN_DEG, 0.0);
            u.set_tick(ose_sim::Tick(
                (t * ose_sim::TICKS_PER_SECOND as f64).round() as u64,
            ));
        }
        u
    }
}

impl Default for WorldSetup {
    fn default() -> Self {
        WorldSetup {
            bodies: Vec::new(),
            terrains: Vec::new(),
            ksc: (KSC_LAT_DEG, KSC_LON_DEG, 100.0),
            kerbal_experiments: Vec::new(),
            science: Default::default(),
            regions: Vec::new(),
        }
    }
}

/// A new campaign's settings.
pub struct NewCampaign {
    pub name: String,
    pub mode: SessionMode,
    pub difficulty: Vec<ose_net::DifficultyOption>,
}

/// Which campaign to make live.
pub enum Target {
    Shelved(String),
    New(NewCampaign),
}

/// Put the live campaign on the shelf and make `target` live. Refused while
/// another player is playing the live campaign.
pub fn switch(s: &Shared, player_id: u64, target: Target) -> Result<(), String> {
    {
        let playing = s.playing.lock().expect("playing lock");
        if playing.iter().any(|&p| p != player_id) {
            return Err("another player is playing a different campaign; try again after they leave".into());
        }
    }
    let mut u = s.universe.lock().expect("sim lock");
    let mut v = s.vessels.lock().expect("vessels lock");
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut shelf = s.shelf.lock().expect("shelf lock");
    if let Target::Shelved(id) = &target {
        if !shelf.iter().any(|x| &x.id == id && can_access(&x.save.campaign, player_id)) {
            return Err("join this campaign with a share code first".into());
        }
    }
    // the live campaign goes on the shelf, even one nobody has joined yet (it stays listed)
    {
        let id = c.id.clone();
        let file = save::capture(&u, &v, &c, &[]);
        shelf.retain(|x| x.id != id);
        shelf.push(save::ShelvedCampaign { id, save: file });
    }
    match target {
        Target::Shelved(id) => {
            let i = shelf.iter().position(|x| x.id == id).expect("checked");
            let back = shelf.remove(i);
            let mut nu = s.setup.universe();
            let registry = save::restore(&mut nu, &back.save, &s.parts, &s.resources);
            *u = nu;
            *v = registry;
            *c = back.save.campaign;
            c.id = back.id;
        }
        Target::New(n) => {
            let nu = s.setup.new_campaign_universe();
            let tick = nu.tick();
            *u = nu;
            v.clear();
            let mut id = format!("c{}", crate::now_unix());
            while shelf.iter().any(|x| x.id == id) || id == c.id {
                id.push('x');
            }
            *c = save::Campaign {
                id,
                name: n.name,
                mode: n.mode,
                difficulty: n.difficulty,
                started_tick: tick,
                owner: Some(player_id),
                ..Default::default()
            };
        }
    }
    u.set_warp(1);
    for clock in u.timelines.values_mut() { clock.warp = 1; }
    *s.world_changed_at.lock().expect("world change lock") = std::time::Instant::now();
    drop((u, v, c, shelf));
    s.pending.lock().expect("pending lock").clear();
    s.playing.lock().expect("playing lock").clear();
    s.revert.lock().expect("revert lock").clear();
    s.warp_requests
        .lock()
        .expect("warp lock")
        .values_mut()
        .for_each(|w| *w = 1);
    Ok(())
}

/// The live campaign as a save, without its quicksave.
pub fn snapshot(s: &Shared) -> save::SaveFile {
    let u = s.universe.lock().expect("sim lock");
    let v = s.vessels.lock().expect("vessels lock");
    let c = s.campaign.lock().expect("campaign lock");
    let mut file = save::capture(&u, &v, &c, &[]);
    file.campaign.quicksave = None;
    file
}

/// Where the player's last launch can be reverted to.
pub struct RevertPoint {
    pub local_valid: bool,
    pub editor: save::SaveFile,
    pub launch: save::SaveFile,
    pub craft: ose_data::craft::Craft,
}

/// Local reversion replaces only this player's fleet and clock. Agency progress is shared.
pub fn revert(s: &Shared, player: u64, file: &save::SaveFile, global: bool) -> Result<(), String> {
    if global {
        reload_global(s, file)?;
        for factor in s.warp_requests.lock().expect("warp lock").values_mut() { *factor = 1; }
        return Ok(());
    }
    let mut u = s.universe.lock().expect("sim lock");
    let mut registry = s.vessels.lock().expect("vessels lock");
    let saved: Vec<_> = file.vessels.iter().filter(|v| v.owner == Some(player)).cloned().collect();
    // A craft transferred by docking cannot be recreated while its crew/parts live in another fleet.
    if saved.iter().any(|v| registry.iter().any(|r| r.1 == v.id && r.2 != Some(player))) {
        return Err("a reverted vessel now belongs to another player; use global revert".into());
    }
    let foreign_crew: Vec<_> = registry.iter().filter(|v| v.2 != Some(player))
        .flat_map(|v| u.crew(v.0).into_iter().map(|(_, name)| name)).collect();
    if saved.iter().any(|v| v.crew.iter().any(|(_, name)| foreign_crew.contains(name))) {
        return Err("revert would duplicate crew docked with another player; use global revert".into());
    }
    for &(e, _, owner) in registry.iter() {
        if owner == Some(player) { u.despawn_vessel(e); }
    }
    registry.retain(|v| v.2 != Some(player));
    let tick = file.timelines.get(&player).map_or(file.tick, |c| c.tick);
    u.timelines.insert(player, ose_sim::timelines::TimelineClock { tick, warp: 1 });
    registry.extend(save::restore_vessels(&mut u, &saved, &s.parts, &s.resources));
    drop(registry);
    drop(u);
    s.warp_requests.lock().expect("warp lock").insert(player, 1);
    Ok(())
}

/// Put the live campaign back to `file` (a quicksave or revert point): its universe, vessels and progress.
/// It keeps its id, quicksave and members. Refused while another player is playing it.
pub fn reload(s: &Shared, player_id: u64, file: &save::SaveFile) -> Result<(), String> {
    if s.playing
        .lock()
        .expect("playing lock")
        .iter()
        .any(|&p| p != player_id)
    {
        return Err("another player is playing this campaign".into());
    }
    reload_global(s, file)
}

fn reload_global(s: &Shared, file: &save::SaveFile) -> Result<(), String> {
    let mut nu = s.setup.universe();
    let registry = save::restore(&mut nu, file, &s.parts, &s.resources);
    let mut u = s.universe.lock().expect("sim lock");
    let mut v = s.vessels.lock().expect("vessels lock");
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut back = file.campaign.clone();
    back.id = c.id.clone();
    back.quicksave = c.quicksave.take();
    back.members = c.members.clone();
    back.owner = c.owner;
    back.share_code = c.share_code.clone();
    *s.world_changed_at.lock().expect("world change lock") = std::time::Instant::now();
    *u = nu;
    *v = registry;
    *c = back;
    u.set_warp(1);
    for clock in u.timelines.values_mut() { clock.warp = 1; }
    Ok(())
}

/// Whether a difficulty option of the live campaign is on (on when the campaign does not set it).
pub fn allowed(s: &Shared, id: &str) -> bool {
    let c = s.campaign.lock().expect("campaign lock");
    !matches!(
        c.difficulty.iter().find(|d| d.id == id).map(|d| &d.value),
        Some(ose_net::DifficultyValue::Bool(false))
    )
}

/// Membership and ownership are persisted on the server, never inferred from browser history.
pub fn can_access(c: &save::Campaign, player: u64) -> bool {
    c.owner == Some(player) || c.members.iter().any(|m| m.player_id == player)
}

pub fn live_access(s: &Shared, player: u64) -> bool {
    can_access(&s.campaign.lock().expect("campaign lock"), player)
}

pub fn access(s: &Shared, player: u64, id: &str) -> bool {
    let c = s.campaign.lock().expect("campaign lock");
    if c.id == id { return can_access(&c, player); }
    s.shelf.lock().expect("shelf lock").iter()
        .any(|x| x.id == id && can_access(&x.save.campaign, player))
}

/// Any member can invite a friend, including while another campaign is live.
pub fn share(s: &Shared, player: u64, id: &str) -> Result<ServerMessage, String> {
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut shelf = s.shelf.lock().expect("shelf lock");
    let c = if c.id == id { &mut *c } else {
        &mut shelf.iter_mut().find(|x| x.id == id)
            .ok_or("campaign unavailable")?.save.campaign
    };
    if !can_access(c, player) { return Err("join this campaign with a share code first".into()); }
    if c.share_code.is_none() {
        c.share_code = Some(crate::accounts::random_token().map_err(|e| e.to_string())?);
    }
    Ok(ServerMessage::SessionShareCode { id: id.into(), code: c.share_code.clone().unwrap() })
}

/// Redeeming an invitation grants membership; it never interrupts other players or changes ownership.
pub fn join_code(s: &Shared, player: u64, code: &str) -> Result<ServerMessage, String> {
    let tick = s.universe.lock().expect("sim lock").tick();
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut shelf = s.shelf.lock().expect("shelf lock");
    let code = code.trim();
    let c = if c.share_code.as_deref() == Some(code) { &mut *c } else {
        &mut shelf.iter_mut().find(|x| x.save.campaign.share_code.as_deref() == Some(code))
            .ok_or("invalid share code")?.save.campaign
    };
    if !can_access(c, player) {
        c.members.push(save::CampaignMember {
            player_id: player, display_name: crate::display_name_for(s, player), joined_tick: tick, agency: None,
        });
    }
    Ok(ServerMessage::SessionJoined { id: c.id.clone(), name: c.name.clone() })
}

/// Accessible campaigns: the live one first, then the shelf, most recently played first.

pub fn list(s: &Shared, player: u64) -> ServerMessage {
    let ut_s = s.universe.lock().expect("sim lock").ut();
    let c = s.campaign.lock().expect("campaign lock");
    let shelf = s.shelf.lock().expect("shelf lock");
    let playing = s.playing.lock().expect("playing lock");
    let owner_name = |campaign: &save::Campaign| {
        campaign.owner.map(|id| {
            campaign
                .members
                .iter()
                .find(|m| m.player_id == id)
                .map(|m| m.display_name.clone())
                .unwrap_or_else(|| crate::display_name_for(s, id))
        })
    };
    let mut sessions = Vec::new();
    if can_access(&c, player) { sessions.push(ose_net::SessionInfo {
        id: c.id.clone(),
        name: c.name.clone(),
        mode: c.mode,
        ut_s,
        last_played_unix: c.last_played_unix,
        players: playing.len() as u32,
        owner: c.owner,
        owner_name: owner_name(&c),
    }); }
    let mut rest: Vec<ose_net::SessionInfo> = shelf
        .iter()
        .filter(|x| can_access(&x.save.campaign, player))
        .map(|x| ose_net::SessionInfo {
            id: x.id.clone(),
            name: x.save.campaign.name.clone(),
            mode: x.save.campaign.mode,
            ut_s: x.save.tick.seconds(),
            last_played_unix: x.save.campaign.last_played_unix,
            players: 0,
            owner: x.save.campaign.owner,
            owner_name: owner_name(&x.save.campaign),
        })
        .collect();
    rest.sort_by_key(|x| std::cmp::Reverse(x.last_played_unix));
    sessions.extend(rest);
    ServerMessage::SessionList { sessions }
}

/// Ownership is checked on the server, including for shelved campaigns.
pub fn manage(s: &Shared, player: u64, id: &str, delete: bool) -> Result<(), String> {
    let mut u = s.universe.lock().expect("sim lock");
    let mut v = s.vessels.lock().expect("vessels lock");
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut shelf = s.shelf.lock().expect("shelf lock");
    let live = c.id == id;
    let campaign = if live {
        &mut *c
    } else {
        &mut shelf
            .iter_mut()
            .find(|x| x.id == id)
            .ok_or_else(|| format!("no campaign {id}"))?
            .save
            .campaign
    };
    if !delete {
        if !can_access(campaign, player) {
            return Err("only a campaign member can claim an unowned session".into());
        }
        match campaign.owner {
            None => campaign.owner = Some(player),
            Some(owner) if owner == player => {}
            _ => return Err("this session already has an owner".into()),
        }
        if !campaign.members.iter().any(|m| m.player_id == player) {
            campaign.members.push(save::CampaignMember {
                player_id: player,
                display_name: crate::display_name_for(s, player),
                joined_tick: u.tick(),
                agency: None,
            });
        }
        return Ok(());
    }
    if campaign.owner != Some(player) {
        return Err("only the session owner can delete it".into());
    }
    if live && !s.playing.lock().expect("playing lock").is_empty() {
        return Err("leave the session before deleting it; all players must leave first".into());
    }
    if live {
        *s.world_changed_at.lock().expect("world change lock") = std::time::Instant::now();
        s.pending.lock().expect("pending lock").clear();
        // Replace the deleted live world without shelving it again.
        if let Some(back) = shelf.pop() {
            let mut nu = s.setup.universe();
            *v = save::restore(&mut nu, &back.save, &s.parts, &s.resources);
            *u = nu;
            *c = back.save.campaign;
            c.id = back.id;
        } else {
            *u = s.setup.new_campaign_universe();
            v.clear();
            *c = save::Campaign::default();
            // Never reuse the deleted id for its replacement.
            c.id = format!("empty-{}", crate::now_unix());
            while c.id == id {
                c.id.push('x');
            }
        }
        u.set_warp(1);
    for clock in u.timelines.values_mut() { clock.warp = 1; }
        s.revert.lock().expect("revert lock").clear();
    } else {
        shelf.retain(|x| x.id != id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_campaign_starts_on_a_morning_at_the_ksc() {
        let setup = WorldSetup {
            bodies: serde_json::from_str(crate::EMBEDDED_BODIES).unwrap(),
            ..Default::default()
        };
        let u = setup.new_campaign_universe();
        let kerbin = u.body_entity("Kerbin").unwrap();
        let (lat, lon, _) = setup.ksc;
        let sun = u.sun_elevation_deg(kerbin, lat, lon, u.ut());
        assert!(
            (10.0..20.0).contains(&sun),
            "sun {sun} degrees up at {} s",
            u.ut()
        );
        assert!(
            u.sun_elevation_deg(kerbin, lat, lon, u.ut() + 1_800.0) > sun,
            "rising"
        );
    }
}

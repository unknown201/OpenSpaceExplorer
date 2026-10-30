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
    pub jetpack: ose_data::Jetpack,
    pub science: ose_data::science::ScienceCatalog,
    pub regions: Vec<u8>,
}

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
        u.set_jetpack(self.jetpack.clone());
        // craft at the KSC stand on the campus the client draws
        u.set_site("Kerbin", crate::ksc_site());
        u.set_science(self.science.clone(), self.regions.clone());
        u
    }

    /// A universe for a new campaign: the clock starts where the original's does, at Y1 D1 00:00 (UT 0),
    /// with the sun high over the KSC.
    pub fn new_campaign_universe(&self) -> Universe {
        self.universe()
    }
}

impl Default for WorldSetup {
    fn default() -> Self {
        WorldSetup {
            bodies: Vec::new(),
            terrains: Vec::new(),
            ksc: (KSC_LAT_DEG, KSC_LON_DEG, 100.0),
            kerbal_experiments: Vec::new(),
            jetpack: Default::default(),
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
            let name = s.campaign.lock().expect("campaign lock").name.clone();
            return Err(format!("another player is playing {name}"));
        }
    }
    let mut u = s.universe.lock().expect("sim lock");
    let mut v = s.vessels.lock().expect("vessels lock");
    let mut c = s.campaign.lock().expect("campaign lock");
    let mut shelf = s.shelf.lock().expect("shelf lock");
    if let Target::Shelved(id) = &target {
        if !shelf.iter().any(|x| &x.id == id) {
            return Err(format!("no campaign {id}"));
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
    drop((u, v, c, shelf));
    s.playing.lock().expect("playing lock").clear();
    *s.revert.lock().expect("revert lock") = None;
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
    pub player: u64,
    pub editor: save::SaveFile,
    pub launch: save::SaveFile,
    pub craft: ose_data::craft::Craft,
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
    *u = nu;
    *v = registry;
    *c = back;
    u.set_warp(1);
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

/// Every campaign: the live one first, then the shelf, most recently played first.
pub fn list(s: &Shared) -> ServerMessage {
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
    let mut sessions = vec![ose_net::SessionInfo {
        id: c.id.clone(),
        name: c.name.clone(),
        mode: c.mode,
        ut_s,
        last_played_unix: c.last_played_unix,
        players: playing.len() as u32,
        owner: c.owner,
        owner_name: owner_name(&c),
    }];
    let mut rest: Vec<ose_net::SessionInfo> = shelf
        .iter()
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
        *s.revert.lock().expect("revert lock") = None;
    } else {
        shelf.retain(|x| x.id != id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_campaign_starts_where_ksp2s_clock_does() {
        let setup = WorldSetup {
            bodies: serde_json::from_str(crate::EMBEDDED_BODIES).unwrap(),
            ..Default::default()
        };
        let u = setup.new_campaign_universe();
        assert_eq!(u.ut(), 0.0, "Y1 D1 00:00");
        let kerbin = u.body_entity("Kerbin").unwrap();
        let (lat, lon, _) = setup.ksc;
        let sun = u.sun_elevation_deg(kerbin, lat, lon, u.ut());
        assert!(sun > 60.0, "the sun high over the KSC: {sun} degrees");
    }
}

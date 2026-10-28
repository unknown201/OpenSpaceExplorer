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
        u.set_jetpack(self.jetpack.clone());
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
    let mut sessions = vec![ose_net::SessionInfo {
        id: c.id.clone(),
        name: c.name.clone(),
        mode: c.mode,
        ut_s,
        last_played_unix: c.last_played_unix,
        players: c.members.len() as u32,
    }];
    let mut rest: Vec<ose_net::SessionInfo> = shelf
        .iter()
        .map(|x| ose_net::SessionInfo {
            id: x.id.clone(),
            name: x.save.campaign.name.clone(),
            mode: x.save.campaign.mode,
            ut_s: x.save.tick.seconds(),
            last_played_unix: x.save.campaign.last_played_unix,
            players: x.save.campaign.members.len() as u32,
        })
        .collect();
    rest.sort_by_key(|x| std::cmp::Reverse(x.last_played_unix));
    sessions.extend(rest);
    ServerMessage::SessionList { sessions }
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

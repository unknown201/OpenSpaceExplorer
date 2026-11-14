//! Several campaigns live at once. Each live campaign is a room: its own universe, clock, streams and
//! players (`Shared`). The first room is the server's primary one (it is what the save file's live
//! campaign is, and where a player arriving lands); more rooms open when a player picks or starts a
//! campaign while someone else is playing the room they are in, and close again (their campaign goes back
//! on the shelf) when the last player leaves them.
//!
//! Before rooms there was one live universe per server, so a player picking another campaign while anyone
//! else was playing got "another player is playing ...".

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::campaigns::Target;
use crate::{save, AppState, Shared};

#[derive(Default)]
pub struct Rooms {
    /// Every live room; the primary one first.
    list: Mutex<Vec<AppState>>,
    /// The room each player is in. A player not listed is in the primary room.
    of: Mutex<HashMap<u64, AppState>>,
    /// Held while players move between rooms and rooms open or close, so two of those never interleave.
    pub(super) gate: Mutex<()>,
}

impl Rooms {
    pub fn set_primary(&self, room: AppState) {
        self.list.lock().expect("rooms lock").insert(0, room);
    }

    pub fn primary(&self) -> AppState {
        self.list.lock().expect("rooms lock")[0].clone()
    }

    /// Every live room, the primary one first.
    pub fn all(&self) -> Vec<AppState> {
        self.list.lock().expect("rooms lock").clone()
    }

    fn is_primary(&self, room: &Shared) -> bool {
        std::ptr::eq(&*self.list.lock().expect("rooms lock")[0], room)
    }
}

/// The room `player` is in.
pub fn room_of(s: &Shared, player: u64) -> AppState {
    let found = s.rooms.of.lock().expect("rooms lock").get(&player).cloned();
    found.unwrap_or_else(|| s.rooms.primary())
}

/// The live room playing campaign `id`, if one is.
pub fn live(s: &Shared, id: &str) -> Option<AppState> {
    s.rooms
        .all()
        .into_iter()
        .find(|r| r.campaign.lock().expect("campaign lock").id == id)
}

/// The ids of every live campaign.
pub fn live_ids(s: &Shared) -> Vec<String> {
    s.rooms
        .all()
        .iter()
        .map(|r| r.campaign.lock().expect("campaign lock").id.clone())
        .collect()
}

fn others_playing(room: &Shared, player: u64) -> bool {
    room.playing
        .lock()
        .expect("playing lock")
        .iter()
        .any(|&p| p != player)
}

/// Take `player` to `target`: into the room already playing it, or load it into a room nobody else is
/// playing (theirs, an idle one, or a new one). Answers the room they are now in.
pub fn enter(s: &Shared, player: u64, target: Target) -> Result<AppState, String> {
    let _gate = s.rooms.gate.lock().expect("rooms gate");
    let current = room_of(s, player);
    if let Target::Shelved(id) = &target {
        if !crate::campaigns::access(s, player, id) {
            return Err("join this campaign with a share code first".into());
        }
        if let Some(room) = live(s, id) {
            move_player(s, player, &current, &room);
            return Ok(room);
        }
    }
    let idle = || {
        s.rooms
            .all()
            .into_iter()
            .find(|r| r.playing.lock().expect("playing lock").is_empty())
    };
    let (room, opened) = if !others_playing(&current, player) {
        (current.clone(), false)
    } else if let Some(room) = idle() {
        (room, false)
    } else {
        (open(s), true)
    };
    if let Err(reason) = crate::campaigns::switch(&room, player, target) {
        if opened {
            close(s, &room);
        }
        return Err(reason);
    }
    move_player(s, player, &current, &room);
    Ok(room)
}

/// The player left their campaign (back to the session list): its room closes if nobody else plays it.
pub fn left_campaign(s: &Shared, player: u64) {
    let _gate = s.rooms.gate.lock().expect("rooms gate");
    let room = room_of(s, player);
    close_if_idle(s, &room);
}

/// The player's last connection closed.
pub fn disconnected(s: &Shared, player: u64) {
    let _gate = s.rooms.gate.lock().expect("rooms gate");
    let room = room_of(s, player);
    s.rooms.of.lock().expect("rooms lock").remove(&player);
    close_if_idle(s, &room);
}

/// Delete room `room`'s campaign: the room closes without shelving it. Not the primary room.
pub fn delete(s: &Shared, room: &AppState) -> bool {
    let _gate = s.rooms.gate.lock().expect("rooms gate");
    if s.rooms.is_primary(room) || !room.playing.lock().expect("playing lock").is_empty() {
        return false;
    }
    room.campaign.lock().expect("campaign lock").id.clear();
    close(s, room);
    true
}

fn move_player(s: &Shared, player: u64, from: &AppState, to: &AppState) {
    if Arc::ptr_eq(from, to) {
        return;
    }
    let was_playing = from.playing.lock().expect("playing lock").remove(&player);
    from.warp_requests
        .lock()
        .expect("warp lock")
        .remove(&player);
    if was_playing {
        crate::push_campaign(from, player);
    }
    crate::apply_warp(from);
    to.warp_requests
        .lock()
        .expect("warp lock")
        .entry(player)
        .or_insert(1);
    s.rooms
        .of
        .lock()
        .expect("rooms lock")
        .insert(player, to.clone());
    close_if_idle(s, from);
}

/// A new, empty room (its campaign is switched in next).
fn open(s: &Shared) -> AppState {
    let room = Arc::new(Shared {
        universe: Mutex::new(s.setup.new_campaign_universe()),
        streams: crate::streaming::Streams::default(),
        pending: Mutex::new(Vec::new()),
        science: s.science.clone(),
        catalog: s.catalog.clone(),
        spins: s.spins.clone(),
        parts: s.parts.clone(),
        resources: s.resources.clone(),
        accounts: s.accounts.clone(),
        otp: s.otp.clone(),
        save_path: s.save_path.clone(),
        save_lock: s.save_lock.clone(),
        world_changed_at: Mutex::new(crate::streaming::mark()),
        vessels: Mutex::new(Vec::new()),
        next_vessel_id: s.next_vessel_id.clone(),
        // no id: nothing to shelve when the chosen campaign replaces it
        campaign: Mutex::new(save::Campaign {
            id: String::new(),
            ..Default::default()
        }),
        difficulty: s.difficulty.clone(),
        stock_crafts: s.stock_crafts.clone(),
        crafts: s.crafts.clone(),
        names: s.names.clone(),
        warp_requests: Mutex::new(HashMap::new()),
        connections: s.connections.clone(),
        setup: s.setup.clone(),
        shelf: s.shelf.clone(),
        playing: Mutex::new(Default::default()),
        revert: Mutex::new(None),
        kerbals: s.kerbals.clone(),
        last_roster: Mutex::new(None),
        flying: Mutex::new(HashMap::new()),
        viewing: Mutex::new(HashMap::new()),
        test_probe: s.test_probe,
        allow_guest_accounts: s.allow_guest_accounts,
        steps: std::sync::atomic::AtomicU64::new(0),
        recorder: Mutex::new(crate::replay::Recorder::default()),
        rooms: s.rooms.clone(),
        closed: std::sync::atomic::AtomicBool::new(false),
    });
    s.rooms.list.lock().expect("rooms lock").push(room.clone());
    // a replay steps its one room itself, with no runtime to run clocks on
    if tokio::runtime::Handle::try_current().is_ok() {
        crate::start_room(room.clone());
    }
    tracing::info!(rooms = s.rooms.all().len(), "room opened");
    room
}

fn close_if_idle(s: &Shared, room: &AppState) {
    if s.rooms.is_primary(room) || !room.playing.lock().expect("playing lock").is_empty() {
        return;
    }
    close(s, room);
}

/// Put a room away: its campaign goes back on the shelf (unless it has no id: new or deleted), its clock
/// and streams stop, and anyone still looking at it is back in the primary room.
fn close(s: &Shared, room: &AppState) {
    {
        let u = room.universe.lock().expect("sim lock");
        let v = room.vessels.lock().expect("vessels lock");
        let c = room.campaign.lock().expect("campaign lock");
        if !c.id.is_empty() {
            let file = save::capture(&u, &v, &c, &[]);
            let mut shelf = room.shelf.lock().expect("shelf lock");
            shelf.retain(|x| x.id != c.id);
            shelf.push(save::ShelvedCampaign {
                id: c.id.clone(),
                save: file,
            });
        }
    }
    room.closed.store(true, Ordering::SeqCst);
    s.rooms
        .list
        .lock()
        .expect("rooms lock")
        .retain(|r| !Arc::ptr_eq(r, room));
    let primary = s.rooms.primary();
    let viewers: Vec<u64> = {
        let mut of = s.rooms.of.lock().expect("rooms lock");
        let viewers = of
            .iter()
            .filter(|(_, r)| Arc::ptr_eq(r, room))
            .map(|(&p, _)| p)
            .collect();
        of.retain(|_, r| !Arc::ptr_eq(r, room));
        viewers
    };
    let mut warp = primary.warp_requests.lock().expect("warp lock");
    for p in viewers {
        warp.entry(p).or_insert(1);
    }
    let name = room.campaign.lock().expect("campaign lock").name.clone();
    tracing::info!(campaign = %name, rooms = s.rooms.all().len(), "room closed");
}

/// The save file: the primary room's campaign live, with the saved crafts and the shelf; the other live
/// rooms' campaigns are saved on the shelf (after a restart they are resumed from the session list).
pub fn save_file(s: &Shared) -> save::SaveFile {
    let _gate = s.rooms.gate.lock().expect("rooms gate");
    let rooms = s.rooms.all();
    let mut file = {
        let p = &rooms[0];
        let u = p.universe.lock().expect("sim lock");
        let v = p.vessels.lock().expect("vessels lock");
        let c = p.campaign.lock().expect("campaign lock");
        let k = p.crafts.lock().expect("crafts lock");
        let mut file = save::capture(&u, &v, &c, &k);
        file.shelf = p.shelf.lock().expect("shelf lock").clone();
        file
    };
    for room in &rooms[1..] {
        let u = room.universe.lock().expect("sim lock");
        let v = room.vessels.lock().expect("vessels lock");
        let c = room.campaign.lock().expect("campaign lock");
        if c.id.is_empty() {
            continue;
        }
        let save = save::capture(&u, &v, &c, &[]);
        file.shelf.retain(|x| x.id != c.id);
        file.shelf.push(save::ShelvedCampaign {
            id: c.id.clone(),
            save,
        });
    }
    file
}

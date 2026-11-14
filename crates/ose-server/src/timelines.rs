//! Each player's own time warp (`ose_sim::timelines`).
//!
//! Warp used to be one clock for the whole room: the lowest request won, so one player at the launch pad held
//! everyone else at 1x. Now a player who asks for warp while sharing their time with other players gets a time
//! of their own (a timeline), and takes the vessel they pilot with them (and the players riding along in it);
//! everyone else keeps theirs. Players drift apart in time that way, as in the KSP multiplayer mods' subspaces:
//! each sees the others' vessels where their orbits put them at their own time, and nothing in one time
//! touches anything in another. A player behind can catch up with one ahead (`ClientMessage::SyncTime`): their
//! time runs at the fastest warp their vessels allow until the clocks meet, and from then on they share one.
//! Nothing ever goes back in time.
//!
//! A room where nobody has warped apart has no timelines at all and runs as it always did. Players in one time
//! still share its warp as before (lowest wins), and the server shows each player's time before it handles
//! their message (`enter_for`), so everything they do happens at their time.

use std::collections::{HashMap, HashSet};

use bevy_ecs::entity::Entity;
use ose_net::{PlayerTime, ServerMessage};
use ose_sim::Universe;

use crate::Shared;

/// The timeline `player` is in (a newcomer joins the one furthest ahead); 0 without timelines.
pub fn of(u: &mut Universe, player: u64) -> u32 {
    if !u.timelines_enabled() {
        return 0;
    }
    match u.member_timeline(player) {
        Some(t) => t,
        None => {
            let t = u.newest_timeline();
            u.set_member_timeline(player, t);
            t
        }
    }
}

/// Show `player`'s time, before handling what they sent.
pub fn enter_for(s: &Shared, player: u64) {
    let mut u = s.universe.lock().expect("sim lock");
    if u.timelines_enabled() {
        let t = of(&mut u, player);
        u.enter(t);
    }
}

fn playing(s: &Shared) -> HashSet<u64> {
    s.playing.lock().expect("playing lock").clone()
}

/// What goes with `player` into a time of their own: the vessel they pilot, and the players riding along in
/// it (following it with their view and piloting nothing).
struct Party {
    vessel: Option<(u64, Entity)>,
    riders: Vec<u64>,
    /// Other players flying the same vessel (it stays where they are then).
    copilots: Vec<u64>,
}

fn party(s: &Shared, player: u64) -> Party {
    let flying = s.flying.lock().expect("flying lock").clone();
    let viewing = s.viewing.lock().expect("viewing lock").clone();
    let vessel = flying.get(&player).and_then(|&id| {
        let list = s.vessels.lock().expect("vessels lock");
        list.iter().find(|x| x.1 == id).map(|x| (id, x.0))
    });
    let playing = playing(s);
    let (mut riders, mut copilots) = (Vec::new(), Vec::new());
    if let Some((id, _)) = vessel {
        for &p in playing.iter().filter(|&&p| p != player) {
            match flying.get(&p) {
                Some(&f) if f == id => copilots.push(p),
                None if viewing.get(&p) == Some(&id) => riders.push(p),
                _ => {}
            }
        }
    }
    Party {
        vessel,
        riders,
        copilots,
    }
}

/// Give `player` a time of their own when they share theirs with other players, taking their vessel and its
/// riders along (their warp requests become `player`'s). Returns their timeline.
pub fn split_off(s: &Shared, player: u64) -> u32 {
    let (t, riders) = split_off_with(s, player);
    let mut reqs = s.warp_requests.lock().expect("warp lock");
    let w = reqs.get(&player).copied().unwrap_or(1);
    for r in riders {
        reqs.insert(r, w);
    }
    t
}

fn split_off_with(s: &Shared, player: u64) -> (u32, Vec<u64>) {
    let party = party(s, player);
    let playing = playing(s);
    let mut u = s.universe.lock().expect("sim lock");
    let mine = of(&mut u, player);
    let others: Vec<u64> = playing
        .iter()
        .copied()
        .filter(|&p| p != player && !party.riders.contains(&p))
        .filter(|&p| !u.timelines_enabled() || u.member_timeline(p).is_none_or(|t| t == mine))
        .collect();
    if others.is_empty() {
        return (mine, Vec::new());
    }
    u.enable_timelines();
    for &p in playing.iter() {
        of(&mut u, p);
    }
    let back = u.active_timeline();
    u.enter(mine);
    let Some(new) = u.split() else {
        u.enter(back);
        return (mine, Vec::new());
    };
    if let Some((_, e)) = party.vessel.filter(|_| party.copilots.is_empty()) {
        if u.timeline_of(e) == mine {
            u.move_to_timeline(&[e], new);
        }
    }
    u.set_member_timeline(player, new);
    for &r in &party.riders {
        u.set_member_timeline(r, new);
    }
    u.enter(back);
    tracing::info!(
        player,
        timeline = new,
        riders = party.riders.len(),
        "a time of their own"
    );
    (new, party.riders)
}

/// The other playing players sharing `player`'s time.
pub fn sharing(s: &Shared, player: u64) -> Vec<u64> {
    let playing = playing(s);
    let mut u = s.universe.lock().expect("sim lock");
    if !u.timelines_enabled() {
        return playing.into_iter().filter(|&p| p != player).collect();
    }
    let mine = of(&mut u, player);
    playing
        .into_iter()
        .filter(|&p| p != player && of(&mut u, p) == mine)
        .collect()
}

/// `player` asks for warp `factor`. Players sharing one time share its warp; asking for less slows everyone
/// in it down with them, asking for more than any of them takes a time of one's own (`split_off`).
pub fn request_warp(s: &Shared, player: u64, factor: u32) {
    let others = sharing(s, player);
    let lowest = {
        let reqs = s.warp_requests.lock().expect("warp lock");
        others
            .iter()
            .map(|p| reqs.get(p).copied().unwrap_or(1))
            .min()
    };
    s.warp_requests
        .lock()
        .expect("warp lock")
        .insert(player, factor);
    match lowest {
        Some(lo) if factor > lo => {
            split_off(s, player);
        }
        Some(_) => {
            let mut reqs = s.warp_requests.lock().expect("warp lock");
            for p in others {
                reqs.insert(p, factor);
            }
        }
        None => {}
    }
}

/// Players sharing a time who want different warps part (a player who joined a time while it was warping,
/// say): those asking for more than the lowest each take a time of their own.
pub fn settle(s: &Shared) {
    let playing = playing(s);
    if playing.len() < 2 {
        return;
    }
    let reqs = s.warp_requests.lock().expect("warp lock").clone();
    let req = |p: &u64| reqs.get(p).copied().unwrap_or(1);
    let groups: Vec<Vec<u64>> = {
        let mut u = s.universe.lock().expect("sim lock");
        if u.timelines_enabled() {
            let mut by: HashMap<u32, Vec<u64>> = HashMap::new();
            for &p in &playing {
                by.entry(of(&mut u, p)).or_default().push(p);
            }
            by.into_values().collect()
        } else {
            vec![playing.iter().copied().collect()]
        }
    };
    for g in groups.into_iter().filter(|g| g.len() > 1) {
        let lo = g.iter().map(req).min().unwrap_or(1);
        let mut faster: Vec<u64> = g.into_iter().filter(|p| req(p) > lo).collect();
        faster.sort();
        let mut moved: HashSet<u64> = HashSet::new();
        for p in faster {
            if !moved.contains(&p) {
                let (_, riders) = split_off_with(s, p);
                let mut r = s.warp_requests.lock().expect("warp lock");
                for x in riders {
                    r.insert(x, req(&p));
                    moved.insert(x);
                }
            }
        }
    }
}

/// Catch `player`'s time up with `other`'s (`ClientMessage::SyncTime`).
pub fn sync(s: &Shared, player: u64, other: u64) -> Result<(), String> {
    if player == other || !playing(s).contains(&other) {
        return Err("that player is not playing this campaign".into());
    }
    let name = crate::display_name_for(s, other);
    {
        let mut u = s.universe.lock().expect("sim lock");
        if !u.timelines_enabled() {
            return Err(format!("you and {name} share one time already"));
        }
        let (a, b) = (of(&mut u, player), of(&mut u, other));
        if a == b {
            return Err(format!("you and {name} share one time already"));
        }
        if u.timeline_tick(b) < u.timeline_tick(a) {
            return Err(format!(
                "{name} is behind you in time: nothing goes back, so they can sync to you instead"
            ));
        }
    }
    // only this player (and their vessel and riders) catch up, not the others sharing their time
    let mine = split_off(s, player);
    let mut u = s.universe.lock().expect("sim lock");
    let to = of(&mut u, other);
    u.catch_up(mine, to)?;
    drop(u);
    tracing::info!(player, other, "catching up in time");
    Ok(())
}

/// Players whose time caught up with another's (`movers`, now in `to`) warp as the players they joined do.
pub fn joined(s: &Shared, movers: &[u64], to: u32) {
    let playing = playing(s);
    let theirs: Vec<u64> = {
        let u = s.universe.lock().expect("sim lock");
        u.timeline_members(to)
            .filter(|p| playing.contains(p) && !movers.contains(p))
            .collect()
    };
    let mut reqs = s.warp_requests.lock().expect("warp lock");
    let w = theirs
        .iter()
        .map(|p| reqs.get(p).copied().unwrap_or(1))
        .min()
        .unwrap_or(1);
    for &p in movers {
        reqs.insert(p, w);
    }
}

/// A vessel `player` commands from another time: one on rails behind their time comes forward to it (its orbit
/// says where it is then); one flying in another time, or ahead of theirs, stays out of reach until they
/// sync with it.
pub fn reach(s: &Shared, u: &mut Universe, player: u64, e: Entity) -> Result<(), String> {
    if !u.timelines_enabled() {
        return Ok(());
    }
    let mine = of(u, player);
    if u.timeline_of(e) == mine {
        return Ok(());
    }
    let theirs = u.timeline_of(e);
    let holders: Vec<u64> = u
        .timeline_members(theirs)
        .filter(|p| playing(s).contains(p))
        .collect();
    u.bring_forward(e, mine)
        .map_err(|why| match holders.first() {
            Some(&p) => format!("{why}: sync with {} first", crate::display_name_for(s, p)),
            None => why,
        })
}

/// Each timeline's players now (every timeline, with nobody in some), newcomers placed.
pub fn parts(s: &Shared) -> Vec<(u32, Vec<u64>)> {
    let playing = playing(s);
    let mut u = s.universe.lock().expect("sim lock");
    if !u.timelines_enabled() {
        return vec![(0, Vec::new())];
    }
    let mut by: HashMap<u32, Vec<u64>> = HashMap::new();
    for &p in &playing {
        by.entry(of(&mut u, p)).or_default().push(p);
    }
    u.timeline_ids()
        .into_iter()
        .map(|t| (t, by.remove(&t).unwrap_or_default()))
        .collect()
}

/// Run `f` with timeline `id` shown (the universe locked throughout), and show the one before again.
pub fn within<R>(s: &Shared, id: u32, f: impl FnOnce(&mut Universe) -> R) -> R {
    let mut u = s.universe.lock().expect("sim lock");
    let back = u.active_timeline();
    u.enter(id);
    let r = f(&mut u);
    u.enter(back);
    r
}

/// Where the other players are in time, to each playing player (once a second, while there are timelines).
pub fn push_times(s: &Shared) {
    let playing = playing(s);
    let msgs: Vec<(u64, ServerMessage)> = {
        let mut u = s.universe.lock().expect("sim lock");
        if !u.timelines_enabled() {
            return;
        }
        let at: HashMap<u64, u32> = playing.iter().map(|&p| (p, of(&mut u, p))).collect();
        playing
            .iter()
            .map(|&p| {
                let mine = at[&p];
                let my_tick = u.timeline_tick(mine).map_or(0, |t| t.0);
                let clock = u.timeline_clock(mine);
                let players = playing
                    .iter()
                    .filter(|&&q| q != p)
                    .filter_map(|&q| {
                        let c = u.timeline_clock(at[&q])?;
                        Some(PlayerTime {
                            player: q,
                            ahead_s: (c.tick.0 as f64 - my_tick as f64)
                                / ose_sim::TICKS_PER_SECOND as f64,
                            warp: c.warp,
                        })
                    })
                    .collect();
                let syncing_to = clock.and_then(|c| c.catching_up).and_then(|to| {
                    let mut m: Vec<u64> = u
                        .timeline_members(to)
                        .filter(|q| playing.contains(q))
                        .collect();
                    m.sort();
                    m.first().copied()
                });
                (
                    p,
                    ServerMessage::PlayerTimes {
                        players,
                        syncing_to,
                    },
                )
            })
            .collect()
    };
    s.pending.lock().expect("pending lock").extend(msgs);
}

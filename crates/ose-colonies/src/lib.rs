//! Colonies (`docs/colonies-design.md`), step 1: found a colony with a landed, crewed vessel that carries a
//! colony trunk, and launch new craft from it on fuel and crew the colony holds.
//!
//! KSP2 promised colonies after science and never shipped them. Its install has the colony trunk parts and a
//! prototype in `KSP.Game.Colonies` (a construction resource provider that pays for what a colony builds,
//! population vacancies, a colony as a CommNet source). In OSE every vessel stays real, so a colony is a
//! **site**: a place on a body where landed vessels belong to it. What those vessels' tanks hold is the
//! colony's stock and the Kerbals aboard are its residents. Nothing is abstracted: a tanker that lands next
//! to the colony adds its fuel to the stock, and a craft launched from the colony's pad is paid for out of
//! the tanks round it and crewed by Kerbals who leave the colony's vessels.
use bevy_ecs::entity::Entity;
use ose_data::craft::Craft;
use ose_sim::{bodies::ReferenceBody, Universe};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The parts that found a colony: KSP2's colony trunks (`transport_3v_colony_trunk_small`, `_large`).
pub const COLONY_PART_PREFIX: &str = "transport_3v_colony_trunk";
/// Landed vessels this close to a colony's site belong to it, m.
pub const RADIUS_M: f64 = 300.0;
/// Two colonies on one body stand at least this far apart, m.
pub const SPACING_M: f64 = 2_000.0;
/// A colony's launch pad stands this far east of its site, m: outside the colony, so a craft waiting on the
/// pad is not part of it (its tanks are not the colony's stock, its crew are not residents).
pub const PAD_DISTANCE_M: f64 = RADIUS_M + 100.0;
/// Resources a launch never pays for: batteries charge on their own and air is not stored.
pub const FREE: &[&str] = &["ElectricCharge", "IntakeAir"];

/// Whether a part founds a colony.
pub fn is_colony_part(part_id: &str) -> bool {
    part_id.starts_with(COLONY_PART_PREFIX)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Colony {
    pub id: u64,
    pub name: String,
    /// The player who founded it.
    pub owner: u64,
    /// The body it stands on (`CelestialBody::id`).
    pub body: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// When it was founded (universal time, s).
    pub founded_ut: f64,
    /// Crafts launched from its pad.
    #[serde(default)]
    pub launches: u32,
    /// When the last newcomer moved in (universal time, s; the founding until one has).
    #[serde(default)]
    pub last_arrival_ut: Option<f64>,
    /// Kerbals who have moved in on their own (`settle`).
    #[serde(default)]
    pub arrivals: u32,
}

/// A campaign's colonies. Kept in the campaign save.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Colonies {
    #[serde(default)]
    pub colonies: Vec<Colony>,
    #[serde(default)]
    next_id: u64,
}

/// One resource a colony holds: units stored and room for them, over all its vessels.
#[derive(Clone, Debug, PartialEq)]
pub struct Stock {
    pub resource: String,
    pub stored: f64,
    pub capacity: f64,
}

/// Where a vessel is: its body, latitude and longitude, and whether it stands on the ground.
fn whereabouts(u: &Universe, e: Entity) -> Option<(Entity, f64, f64, bool)> {
    let p = u.powered(e).or_else(|| u.dormant(e))?;
    let b = u.world.get::<ReferenceBody>(e)?.0;
    let body = u.world.get::<ose_sim::bodies::Body>(b)?;
    let (lat, lon) = ose_sim::flight::lat_lon(body, p.r, u.ut());
    Some((b, lat, lon, p.landed && !p.crashed))
}

fn radius_of(u: &Universe, body: Entity) -> f64 {
    u.world
        .get::<ose_sim::bodies::Body>(body)
        .map(|b| b.radius_m)
        .unwrap_or(1.0)
}

/// Great-circle distance between two places on a sphere of radius `r`, m.
pub fn surface_distance(r: f64, (lat1, lon1): (f64, f64), (lat2, lon2): (f64, f64)) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().min(1.0).asin()
}

/// The place `d` m from `(lat, lon)` on a sphere of radius `r`, setting out on compass heading `heading_deg`.
pub fn travel(r: f64, (lat, lon): (f64, f64), heading_deg: f64, d: f64) -> (f64, f64) {
    let (p1, l1, th, dl) = (
        lat.to_radians(),
        lon.to_radians(),
        heading_deg.to_radians(),
        d / r,
    );
    let p2 = (p1.sin() * dl.cos() + p1.cos() * dl.sin() * th.cos()).asin();
    let l2 = l1 + (th.sin() * dl.sin() * p1.cos()).atan2(dl.cos() - p1.sin() * p2.sin());
    let lon2 = (l2.to_degrees() + 540.0).rem_euclid(360.0) - 180.0;
    (p2.to_degrees(), lon2)
}

/// Kerbals on EVA and planted flags are not part of a colony.
fn is_vessel(u: &Universe, e: Entity) -> bool {
    u.eva_kerbal(e).is_none() && !u.is_flag(e)
}

impl Colonies {
    pub fn get(&self, id: u64) -> Option<&Colony> {
        self.colonies.iter().find(|c| c.id == id)
    }

    /// Found a colony where vessel `e` stands. It must be landed (not afloat, not wrecked), carry a colony
    /// trunk, and have a Kerbal aboard; it must also stand clear of every other colony on that body.
    pub fn found(
        &mut self,
        u: &mut Universe,
        e: Entity,
        owner: u64,
        name: &str,
    ) -> Result<u64, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("a colony needs a name of 1 to 64 characters".into());
        }
        if self
            .colonies
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
        {
            return Err(format!("there is already a colony named {name}"));
        }
        if !is_vessel(u, e) {
            return Err("only a vessel can found a colony".into());
        }
        let (body, lat, lon, landed) = whereabouts(u, e).ok_or("no such vessel")?;
        if !landed {
            return Err("land first: a colony is founded on the ground".into());
        }
        let craft = u.current_craft(e).ok_or("the vessel has no parts")?;
        if !craft.parts.iter().any(|p| is_colony_part(&p.part_id)) {
            return Err("the vessel needs a colony trunk".into());
        }
        if u.crew(e).is_empty() {
            return Err("a colony needs a Kerbal aboard to found it".into());
        }
        let body_id = u.body_id(body).ok_or("unknown body")?.to_string();
        let r = radius_of(u, body);
        if let Some(near) = self.colonies.iter().find(|c| {
            c.body == body_id && surface_distance(r, (c.lat_deg, c.lon_deg), (lat, lon)) < SPACING_M
        }) {
            return Err(format!(
                "too close to {}: colonies stand at least {:.0} km apart",
                near.name,
                SPACING_M / 1000.0
            ));
        }
        self.next_id = self
            .next_id
            .max(self.colonies.iter().map(|c| c.id).max().unwrap_or(0))
            + 1;
        let id = self.next_id;
        self.colonies.push(Colony {
            id,
            name: name.to_string(),
            owner,
            body: body_id,
            lat_deg: lat,
            lon_deg: lon,
            founded_ut: u.ut(),
            launches: 0,
            last_arrival_ut: None,
            arrivals: 0,
        });
        Ok(id)
    }

    /// Give up a colony: its vessels stay where they are. Only its founder may.
    pub fn abandon(&mut self, id: u64, player: u64) -> Result<(), String> {
        let c = self.get(id).ok_or("no such colony")?;
        if c.owner != player {
            return Err("only its founder can abandon a colony".into());
        }
        self.colonies.retain(|c| c.id != id);
        Ok(())
    }
}

/// The vessels that belong to a colony: every vessel landed within `RADIUS_M` of its site.
pub fn members(u: &mut Universe, c: &Colony) -> Vec<Entity> {
    let Some(body) = u.body_entity(&c.body) else {
        return Vec::new();
    };
    let r = radius_of(u, body);
    let all = u.vessels();
    all.into_iter()
        .filter(|&e| is_vessel(u, e))
        .filter(|&e| {
            whereabouts(u, e).is_some_and(|(b, lat, lon, landed)| {
                b == body
                    && landed
                    && surface_distance(r, (c.lat_deg, c.lon_deg), (lat, lon)) <= RADIUS_M
            })
        })
        .collect()
}

/// What a colony's vessels hold, by resource, in name order.
pub fn stock(u: &mut Universe, c: &Colony) -> Vec<Stock> {
    let mut all: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for e in members(u, c) {
        let Some(st) = u.structure(e) else { continue };
        for t in st.net.tanks.iter().filter(|t| st.net.present[t.part]) {
            let x = all.entry(st.net.resources[t.resource].clone()).or_default();
            x.0 += t.stored.max(0.0);
            x.1 += t.capacity;
        }
    }
    all.into_iter()
        .map(|(resource, (stored, capacity))| Stock {
            resource,
            stored,
            capacity,
        })
        .collect()
}

/// The Kerbals living at a colony: the crew of its vessels, with the vessel each sits in.
pub fn residents(u: &mut Universe, c: &Colony) -> Vec<(Entity, String)> {
    members(u, c)
        .into_iter()
        .flat_map(|e| u.crew(e).into_iter().map(move |(_, n)| (e, n)))
        .collect()
}

/// A newcomer moves into a colony with room this long after the last one, or after its founding (s): two
/// Kerbin days. KSP2's population component fills vacancies after a delay; OSE's room is the empty seats.
pub const VACANCY_DELAY_S: f64 = 2.0 * 21_600.0;

/// The empty seats in a colony's vessels, as (vessel, part), in vessel then seat order. A colony is
/// home to as many Kerbals as its vessels seat.
pub fn vacancies(u: &mut Universe, c: &Colony) -> Vec<(Entity, usize)> {
    let mut out = Vec::new();
    for e in members(u, c) {
        let mut taken = u.crew(e);
        for part in u.crew_seats(e) {
            match taken.iter().position(|(p, _)| *p == part) {
                Some(k) => {
                    taken.remove(k);
                }
                None => out.push((e, part)),
            }
        }
    }
    out
}

/// When the next newcomer is due in a colony, if it has someone living there to welcome them and room.
pub fn next_arrival(u: &mut Universe, c: &Colony) -> Option<f64> {
    if residents(u, c).is_empty() || vacancies(u, c).is_empty() {
        return None;
    }
    Some(c.last_arrival_ut.unwrap_or(c.founded_ut) + VACANCY_DELAY_S)
}

impl Colonies {
    /// Population (design step 6): each colony with residents and an empty seat takes in a newcomer every
    /// `VACANCY_DELAY_S`, named by `name_for` (the next Kerbal the campaign has not hired). Returns the
    /// (colony, Kerbal) pairs who moved in. A colony without room or residents only restarts its wait.
    pub fn settle(
        &mut self,
        u: &mut Universe,
        name_for: &mut dyn FnMut() -> String,
    ) -> Vec<(u64, String)> {
        let now = u.ut();
        let mut out = Vec::new();
        for c in &mut self.colonies {
            loop {
                let due = c.last_arrival_ut.unwrap_or(c.founded_ut) + VACANCY_DELAY_S;
                if now < due {
                    break;
                }
                let room = if residents(u, c).is_empty() {
                    None
                } else {
                    vacancies(u, c).first().copied()
                };
                let Some((e, part)) = room else {
                    // nobody to welcome them or no room: the wait starts over from now
                    c.last_arrival_ut = Some(now);
                    break;
                };
                let name = name_for();
                let mut crew = u.crew(e);
                crew.push((part, name.clone()));
                u.set_crew(e, crew);
                c.last_arrival_ut = Some(due);
                c.arrivals += 1;
                out.push((c.id, name));
            }
        }
        out
    }
}

/// Where a colony's launch pad stands: `PAD_DISTANCE_M` east of its site.
pub fn pad(u: &Universe, c: &Colony) -> Option<(f64, f64)> {
    let body = u.body_entity(&c.body)?;
    let r = radius_of(u, body);
    Some(travel(r, (c.lat_deg, c.lon_deg), 90.0, PAD_DISTANCE_M))
}

/// What launching `craft` from a colony costs: every resource its tanks hold, but those in `FREE`, and its
/// parts' dry mass `dry_t` in Metal (`ose_data::colony`, a unit a kilogram) shipped from Kerbin.
pub fn bill(craft: &Craft, dry_t: f64) -> BTreeMap<String, f64> {
    let mut all: BTreeMap<String, f64> = BTreeMap::new();
    for p in &craft.parts {
        for r in &p.resources {
            if r.stored_units > 0.0 && !FREE.iter().any(|f| f.eq_ignore_ascii_case(&r.resource)) {
                *all.entry(r.resource.clone()).or_default() += r.stored_units;
            }
        }
    }
    if dry_t > 0.0 {
        *all.entry(ose_data::colony::METAL.to_string()).or_default() +=
            dry_t / ose_data::colony::METAL_UNIT_T;
    }
    all
}

/// What the colony lacks to pay `bill`, by resource (units short), empty when it can pay.
pub fn shortfall(stock: &[Stock], bill: &BTreeMap<String, f64>) -> Vec<(String, f64)> {
    bill.iter()
        .filter_map(|(res, &need)| {
            let have = stock
                .iter()
                .find(|s| s.resource.eq_ignore_ascii_case(res))
                .map(|s| s.stored)
                .unwrap_or(0.0);
            (have + 1e-6 < need).then(|| (res.clone(), need - have))
        })
        .collect()
}

/// Say what a colony lacks, for a refusal.
pub fn describe_shortfall(short: &[(String, f64)]) -> String {
    let list: Vec<String> = short.iter().map(|(r, n)| format!("{n:.1} {r}")).collect();
    format!("the colony is short of {}", list.join(", "))
}

/// Take `bill` out of the colony's tanks: each tank gives in proportion to what it holds, so they empty
/// together. Nothing is taken unless all of it can be.
pub fn draw(u: &mut Universe, c: &Colony, bill: &BTreeMap<String, f64>) -> Result<(), String> {
    let short = shortfall(&stock(u, c), bill);
    if !short.is_empty() {
        return Err(describe_shortfall(&short));
    }
    let vessels = members(u, c);
    for (res, &need) in bill {
        let total: f64 = stock(u, c)
            .iter()
            .find(|s| s.resource.eq_ignore_ascii_case(res))
            .map(|s| s.stored)
            .unwrap_or(0.0);
        if total <= 0.0 {
            continue;
        }
        let share = (need / total).min(1.0);
        for &e in &vessels {
            let Some(mut st) = u.world.get_mut::<ose_sim::structure::Structure>(e) else {
                continue;
            };
            let st = &mut *st;
            let Some(ri) = st
                .net
                .resources
                .iter()
                .position(|r| r.eq_ignore_ascii_case(res))
            else {
                continue;
            };
            let present = st.net.present.clone();
            for t in st
                .net
                .tanks
                .iter_mut()
                .filter(|t| t.resource == ri && present[t.part])
            {
                t.stored = (t.stored - t.stored.max(0.0) * share).max(0.0);
            }
        }
    }
    Ok(())
}

/// Seat Kerbals of the colony in vessel `e`, just launched from its pad: the `chosen` residents, or the
/// residents in order while seats last. They leave the colony's vessels. Returns who boarded.
pub fn board(
    u: &mut Universe,
    c: &Colony,
    e: Entity,
    chosen: Option<&[String]>,
) -> Result<Vec<String>, String> {
    u.set_crew(e, Vec::new());
    let seats = u.crew_seats(e);
    let living = residents(u, c);
    let picked: Vec<(Entity, String)> = match chosen {
        None => living.into_iter().take(seats.len()).collect(),
        Some(names) => {
            let names: Vec<&String> = names.iter().filter(|n| !n.is_empty()).collect();
            if names.len() > seats.len() {
                return Err(format!("only {} seats", seats.len()));
            }
            let mut out: Vec<(Entity, String)> = Vec::new();
            for n in names {
                let Some(found) = living.iter().find(|(_, r)| r == n) else {
                    return Err(format!("{n} does not live at {}", c.name));
                };
                if out.iter().any(|(_, r)| r == n) {
                    return Err(format!("{n} is picked twice"));
                }
                out.push(found.clone());
            }
            out
        }
    };
    for (from, name) in &picked {
        let rest: Vec<(usize, String)> = u
            .crew(*from)
            .into_iter()
            .filter(|(_, n)| n != name)
            .collect();
        u.set_crew(*from, rest);
    }
    let members: Vec<(usize, String)> = seats
        .into_iter()
        .zip(picked.iter().map(|(_, n)| n.clone()))
        .collect();
    let names = members.iter().map(|(_, n)| n.clone()).collect();
    u.set_crew(e, members);
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travelling_and_measuring_agree() {
        let r = 200_000.0;
        for (lat, lon) in [(0.0, 0.0), (45.0, 170.0), (-80.0, -179.9)] {
            for h in [0.0, 90.0, 135.0, 270.0] {
                let p = travel(r, (lat, lon), h, 150.0);
                let d = surface_distance(r, (lat, lon), p);
                assert!((d - 150.0).abs() < 1e-6, "{lat} {lon} {h}: {d}");
            }
        }
        // heading east keeps the latitude on the equator
        let (lat, lon) = travel(r, (0.0, 10.0), 90.0, 1000.0);
        assert!(lat.abs() < 1e-9 && lon > 10.0);
    }

    #[test]
    fn a_launch_pays_for_its_fuel_but_not_its_charge() {
        let craft: Craft = serde_json::from_value(serde_json::json!({
            "name": "x",
            "parts": []
        }))
        .unwrap_or_default();
        assert!(bill(&craft, 0.0).is_empty());
        assert_eq!(bill(&craft, 1.5)["Metal"], 1500.0);
        let stock = vec![Stock {
            resource: "Methane".into(),
            stored: 10.0,
            capacity: 20.0,
        }];
        let mut want = BTreeMap::new();
        want.insert("Methane".to_string(), 12.0);
        want.insert("Oxidizer".to_string(), 1.0);
        let short = shortfall(&stock, &want);
        assert_eq!(short.len(), 2);
        assert!((short[0].1 - 2.0).abs() < 1e-9);
        want.insert("Methane".to_string(), 10.0);
        want.remove("Oxidizer");
        assert!(shortfall(&stock, &want).is_empty());
    }
}

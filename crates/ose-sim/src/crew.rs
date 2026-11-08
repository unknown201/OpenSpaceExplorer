//! Crew and EVA: who sits in which crewed part, Kerbals stepping out as their own little vessel with the
//! original's jetpack (`Data_Kerbal`: 0.25 kN move force), boarding back in, and planting flags.

use bevy_ecs::prelude::*;
use ose_data::science::PartExperiment;
use ose_math::{DVec3, Orbit};
use serde::{Deserialize, Serialize};

use crate::{bodies, flight, structure, vessels, Universe};

/// The original four, then the rest of the roster in order.
pub const VETERANS: [&str; 4] = [
    "Jebediah Kerman",
    "Bill Kerman",
    "Bob Kerman",
    "Valentina Kerman",
];

/// A Kerbal with suit and pack (`eva_kerbal` mass), t, and the pack's propellant, t.
pub const KERBAL_MASS_T: f64 = 0.07;
pub const JETPACK_PROPELLANT_T: f64 = 0.025;
/// How close a Kerbal has to be to board, m.
pub const BOARD_RANGE_M: f64 = 6.0;

/// The crew aboard a vessel: (part index, name) per seat taken.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Crew {
    pub members: Vec<(usize, String)>,
}

/// A Kerbal on EVA.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaKerbal {
    pub name: String,
    pub experiments: Vec<PartExperiment>,
}

/// A planted flag (a landed vessel that does nothing).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    pub planted_by: String,
}

/// A Kerbal holding on to a ladder: the vessel and part, and how far up it they are, m.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct OnLadder {
    pub vessel: Entity,
    pub part: usize,
    pub along_m: f64,
    /// Climb input, -1..1 (W up, S down), held.
    pub climb: f64,
}

/// A Kerbal climbing in through the hatch (their seat is taken already); they leave when `until_ut` comes.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Boarding {
    pub vessel: Entity,
    pub until_ut: f64,
}

/// How fast a Kerbal climbs a ladder (`ClimbMoveSpeed`), m/s; how near one they must be to grab it, m (our
/// own figure); how far off its axis they hang, m; how long climbing in takes, s (our own figure).
pub const CLIMB_SPEED_MPS: f64 = 0.8;
pub const GRAB_RANGE_M: f64 = 1.5;
pub const LADDER_STANDOFF_M: f64 = 0.4;
/// How far up a crewed part's ladder a Kerbal steps out of its hatch (share of the ladder): KSP2 trace run 14
/// hangs the Kerbal 0.43 m above the Mk1 pod's origin, three quarters of the way up its hull.
pub const HATCH_ALONG: f64 = 0.75;
pub const BOARDING_S: f64 = 1.0;

/// A resource transfer running on a vessel (the Resource Manager): from these tanks into those.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Transfer {
    pub resource: String,
    pub from: Vec<usize>,
    pub to: Vec<usize>,
    /// Units a second it moves.
    pub rate: f64,
}

/// How long a transfer takes to move what it can, s (our own figure: the original's flow requests
/// deliver over a few seconds), and the least it moves a second.
pub const TRANSFER_TIME_S: f64 = 10.0;
pub const TRANSFER_MIN_RATE: f64 = 0.01;

impl Universe {
    /// Move a Kerbal aboard to another crewed part of the same vessel (part index into the craft).
    pub fn move_crew(
        &mut self,
        e: Entity,
        kerbal: &str,
        to_part: usize,
    ) -> Result<(), &'static str> {
        let st = self
            .world
            .get::<structure::Structure>(e)
            .ok_or("vessel has no parts")?;
        if to_part >= st.rigid.len() || !st.rigid.present[to_part] {
            return Err("no such part");
        }
        let seats = st.crew[to_part] as usize;
        if seats == 0 {
            return Err("that part carries no crew");
        }
        let mut crew = self.crew(e);
        let Some(k) = crew.iter().position(|(_, n)| n == kerbal) else {
            return Err("no such Kerbal aboard");
        };
        if crew[k].0 == to_part {
            return Ok(());
        }
        if crew.iter().filter(|(p, _)| *p == to_part).count() >= seats {
            return Err("that part is full");
        }
        crew[k].0 = to_part;
        self.set_crew(e, crew);
        Ok(())
    }

    /// Start moving `resource` from the `from` tanks into the `to` tanks (part indices); empty lists stop
    /// the vessel's transfer. Like the original's Resource Manager it fills the receiving tanks with what
    /// the giving ones hold, then stops.
    pub fn transfer_resource(
        &mut self,
        e: Entity,
        resource: &str,
        from: &[usize],
        to: &[usize],
    ) -> Result<(), &'static str> {
        if from.is_empty() && to.is_empty() {
            self.world.entity_mut(e).remove::<Transfer>();
            return Ok(());
        }
        if from.iter().any(|p| to.contains(p)) {
            return Err("a tank cannot give and take at once");
        }
        let st = self
            .world
            .get::<structure::Structure>(e)
            .ok_or("vessel has no parts")?;
        let ri = st
            .net
            .resources
            .iter()
            .position(|r| r.eq_ignore_ascii_case(resource))
            .ok_or("the vessel holds none of that")?;
        let tanks = |parts: &[usize]| {
            st.net
                .tanks
                .iter()
                .filter(|t| t.resource == ri && parts.contains(&t.part) && st.rigid.present[t.part])
                .cloned()
                .collect::<Vec<_>>()
        };
        let (out, into) = (tanks(from), tanks(to));
        if out.is_empty() || into.is_empty() {
            return Err("pick tanks of that resource on both sides");
        }
        let held: f64 = out.iter().map(|t| t.stored).sum();
        let room: f64 = into.iter().map(|t| (t.capacity - t.stored).max(0.0)).sum();
        let amount = held.min(room);
        if amount <= 1e-9 {
            return Err("nothing to move");
        }
        let resource = st.net.resources[ri].clone();
        self.world.entity_mut(e).insert(Transfer {
            resource,
            from: from.to_vec(),
            to: to.to_vec(),
            rate: (amount / TRANSFER_TIME_S).max(TRANSFER_MIN_RATE),
        });
        Ok(())
    }

    /// The vessel's running transfer, if any.
    pub fn transfer(&self, e: Entity) -> Option<&Transfer> {
        self.world.get::<Transfer>(e)
    }

    /// Run the vessels' transfers for `dt` seconds: the giving tanks drain in proportion to what they hold,
    /// the receiving ones fill in proportion to their room; a transfer ends when one side runs out.
    pub(crate) fn run_transfers(&mut self, dt: f64) {
        let running: Vec<(Entity, Transfer)> = self
            .world
            .query::<(Entity, &Transfer)>()
            .iter(&self.world)
            .map(|(e, t)| (e, t.clone()))
            .collect();
        for (e, t) in running {
            let done = match self.world.get_mut::<structure::Structure>(e) {
                None => true,
                Some(mut st) => {
                    let st = &mut *st;
                    let Some(ri) = st.net.resources.iter().position(|r| r == &t.resource) else {
                        self.world.entity_mut(e).remove::<Transfer>();
                        continue;
                    };
                    let present = st.rigid.present.clone();
                    let pick = |t2: &crate::resources::Tank, parts: &[usize]| {
                        t2.resource == ri && parts.contains(&t2.part) && present[t2.part]
                    };
                    let held: f64 = st
                        .net
                        .tanks
                        .iter()
                        .filter(|x| pick(x, &t.from))
                        .map(|x| x.stored)
                        .sum();
                    let room: f64 = st
                        .net
                        .tanks
                        .iter()
                        .filter(|x| pick(x, &t.to))
                        .map(|x| (x.capacity - x.stored).max(0.0))
                        .sum();
                    let step = (t.rate * dt).min(held).min(room);
                    if step > 0.0 {
                        for x in st.net.tanks.iter_mut() {
                            if pick(x, &t.from) && held > 0.0 {
                                x.stored = (x.stored - step * x.stored / held).max(0.0);
                            } else if pick(x, &t.to) && room > 0.0 {
                                let free = (x.capacity - x.stored).max(0.0);
                                x.stored = (x.stored + step * free / room).min(x.capacity);
                            }
                        }
                    }
                    held - step <= 1e-9 || room - step <= 1e-9
                }
            };
            if done {
                self.world.entity_mut(e).remove::<Transfer>();
            }
        }
    }

    pub fn crew(&self, e: Entity) -> Vec<(usize, String)> {
        self.world
            .get::<Crew>(e)
            .map(|c| c.members.clone())
            .unwrap_or_default()
    }

    pub fn set_crew(&mut self, e: Entity, members: Vec<(usize, String)>) {
        let mass = self
            .world
            .get_mut::<structure::Structure>(e)
            .map(|mut st| st.set_occupied_seats(&members));
        if let Some(mass) = mass {
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
                p.mass_t = mass;
            }
            if let Some(mut p) = self.world.get_mut::<flight::Dormant>(e) {
                p.0.mass_t = mass;
            }
        }
        self.world.entity_mut(e).insert(Crew { members });
    }

    pub fn eva_kerbal(&self, e: Entity) -> Option<&EvaKerbal> {
        self.world.get::<EvaKerbal>(e)
    }

    pub fn is_flag(&self, e: Entity) -> bool {
        self.world.get::<Flag>(e).is_some()
    }

    /// Every Kerbal name in use: aboard vessels or on EVA.
    pub fn kerbals_in_use(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let mut q = self.world.query::<&Crew>();
        for c in q.iter(&self.world) {
            out.extend(c.members.iter().map(|(_, n)| n.clone()));
        }
        let mut q = self.world.query::<&EvaKerbal>();
        for k in q.iter(&self.world) {
            out.push(k.name.clone());
        }
        out
    }

    /// Fill a vessel's crewed seats from `roster`, skipping Kerbals already flying.
    /// The vessel's seats, one entry (its part) per seat, in craft order.
    pub fn crew_seats(&self, e: Entity) -> Vec<usize> {
        self.world
            .get::<structure::Structure>(e)
            .map(|st| {
                (0..st.rigid.len())
                    .filter(|&i| st.rigid.present[i])
                    .flat_map(|i| std::iter::repeat_n(i, st.crew[i] as usize))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn crew_vessel(&mut self, e: Entity, roster: &[String]) -> Vec<(usize, String)> {
        let Some(seats) = self.world.get::<structure::Structure>(e).map(|st| {
            (0..st.rigid.len())
                .filter(|&i| st.rigid.present[i])
                .flat_map(|i| std::iter::repeat_n(i, st.crew[i] as usize))
                .collect::<Vec<usize>>()
        }) else {
            return Vec::new();
        };
        let busy = self.kerbals_in_use();
        let mut free = roster.iter().filter(|n| !busy.contains(n));
        let mut members = Vec::new();
        let mut spare = busy.len() + roster.len();
        for part in seats {
            let name = match free.next() {
                Some(n) => n.clone(),
                None => {
                    spare += 1;
                    format!("Kerbal {spare} Kerman")
                }
            };
            members.push((part, name));
        }
        self.set_crew(e, members.clone());
        members
    }

    /// A Kerbal steps out of `e`. On the ground they stand beside it; in flight they float a couple of metres
    /// off it with its velocity. Returns the Kerbal's vessel.
    pub fn go_eva(&mut self, e: Entity, name: Option<&str>) -> Result<Entity, String> {
        let mut crew = self.crew(e);
        let idx = match name {
            Some(n) => crew.iter().position(|(_, m)| m == n),
            None => (!crew.is_empty()).then_some(0),
        }
        .ok_or_else(|| "nobody of that name aboard".to_string())?;
        let body = self
            .world
            .get::<bodies::ReferenceBody>(e)
            .ok_or("vessel has no body")?
            .0;
        let ut = self.ut();
        let b = self
            .world
            .get::<bodies::Body>(body)
            .ok_or("no body")?
            .clone();
        let (r, v, landed, facing) = match self.world.get::<flight::Powered>(e) {
            Some(p) => (p.r, p.v, p.landed && !p.crashed, p.facing),
            None => {
                let (r, v) = self
                    .world
                    .get::<bodies::OnRails>(e)
                    .ok_or("no orbit")?
                    .0
                    .state_at(ut);
                (r, v, false, r.normalized())
            }
        };
        let (seat, kname) = crew.remove(idx);
        self.set_crew(e, crew);
        let jetpack = self
            .world
            .get_resource::<KerbalPart>()
            .map(|k| k.jetpack.clone())
            .unwrap_or_default();
        let isp_vac = jetpack.isp_curve.evaluate(0.0);
        let isp_asl = jetpack.isp_curve.evaluate(1.0);
        let spec = flight::FlightSpec {
            stages: vec![flight::StageSpec {
                stage_id: 0,
                thrust_vac_kn: jetpack.move_force_kn,
                thrust_asl_kn: jetpack.move_force_kn * isp_asl / isp_vac.max(1e-9),
                isp_vac_s: isp_vac,
                isp_asl_s: isp_asl,
                propellant_t: JETPACK_PROPELLANT_T,
                start_mass_t: KERBAL_MASS_T + JETPACK_PROPELLANT_T,
                ..Default::default()
            }],
            drag_area_m2: 0.6,
            drag_cd: flight::DEFAULT_CD,
        };
        let kerbal = if landed {
            // a few metres east of the vessel on the ground
            let (lat, lon) = flight::lat_lon(&b, r, ut);
            let dlon = 4.0 / (b.radius_m * lat.to_radians().cos().max(0.01)) * 57.29578;
            self.spawn_powered_on_surface(&kname, body, spec, lat, lon + dlon)
        } else {
            let side = {
                let s = facing.cross(DVec3::new(0.0, 0.0, 1.0));
                if s.length() > 1e-6 {
                    s.normalized()
                } else {
                    DVec3::new(1.0, 0.0, 0.0)
                }
            };
            let r2 = r + side * 2.5;
            let mut p = flight::Powered::new(spec, r2, v, false);
            p.facing = facing;
            let orbit = Orbit::from_state_vectors(b.mu, r2, v, ut);
            self.world
                .spawn((
                    vessels::Vessel {
                        name: kname.clone(),
                    },
                    bodies::ReferenceBody(body),
                    bodies::OnRails(orbit),
                    p,
                ))
                .id()
        };
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(kerbal) {
            // a jetpack fires on the throttle without staging; on the ground the Kerbal walks
            p.ignited = true;
            p.walker = true;
            // the jetpack also translates (Space/Shift up, Ctrl down, sideways) like RCS, always on
            p.rcs.thrust_kn = jetpack.move_force_kn;
            p.rcs.isp_vac_s = isp_vac;
            p.rcs.isp_asl_s = isp_asl;
            p.jetpack = Some(jetpack);
            p.groups.rcs = true;
            // a Kerbal has no SAS: it keeps the attitude it stepped out in rather than turning to a mode
            p.attitude = flight::Attitude::Free;
            p.angvel = DVec3::ZERO;
            p.roll_rate = 0.0;
            p.ground_clearance_m = 0.55;
            p.footprint_m = 0.3;
            p.crash_tolerance_mps = 50.0;
        }
        let experiments = self
            .world
            .get_resource::<KerbalPart>()
            .map(|k| k.experiments.clone())
            .unwrap_or_default();
        self.world.entity_mut(kerbal).insert(EvaKerbal {
            name: kname,
            experiments,
        });
        // As in the original, a Kerbal steps out holding the hatch's ladder rather than dropping off it.
        if self.world.get::<flight::Powered>(e).is_none() {
            self.wake(e);
        }
        if let Some((_, foot, top)) = self.ladders(e).into_iter().find(|l| l.0 == seat) {
            self.world.entity_mut(kerbal).insert(OnLadder {
                vessel: e,
                part: seat,
                along_m: (top - foot).length() * HATCH_ALONG,
                climb: 0.0,
            });
            self.run_ladders(0.0);
        }
        Ok(kerbal)
    }

    /// Craft standing on the ground (or the sea) within `radius_m` of a Kerbal on EVA: what is recovered with
    /// the Kerbal when it is recovered beside its landed craft.
    pub fn landed_craft_near(&mut self, kerbal: Entity, radius_m: f64) -> Vec<Entity> {
        if self.world.get::<EvaKerbal>(kerbal).is_none() {
            return Vec::new();
        }
        self.vessels()
            .into_iter()
            .filter(|&v| v != kerbal && self.world.get::<EvaKerbal>(v).is_none())
            .filter(|&v| {
                self.world
                    .get::<flight::Powered>(v)
                    .is_some_and(|p| (p.landed || p.splashed) && !p.crashed)
            })
            .filter(|&v| {
                self.distance_between(kerbal, v)
                    .is_some_and(|d| d <= radius_m)
            })
            .collect()
    }

    /// A Kerbal on EVA climbs into a vessel within reach that has a free seat.
    pub fn board(&mut self, kerbal: Entity, target: Entity) -> Result<(), String> {
        let name = self
            .world
            .get::<EvaKerbal>(kerbal)
            .ok_or("not a Kerbal on EVA")?
            .name
            .clone();
        let d = self
            .distance_between(kerbal, target)
            .ok_or("not near each other")?;
        // from the nearest hatch (a crewed part) when the vessel has parts
        let me = self.position_of(kerbal).ok_or("not near each other")?;
        let at = self.position_of(target).ok_or("not near each other")?;
        let hatch = self
            .ladders(target)
            .into_iter()
            .filter(|l| {
                self.world
                    .get::<structure::Structure>(target)
                    .is_some_and(|st| st.crew[l.0] > 0)
            })
            .map(|(_, a, b)| (at + (a + b) * 0.5 - me).length())
            .fold(f64::INFINITY, f64::min);
        let d = d.min(hatch);
        if d > BOARD_RANGE_M {
            return Err(format!("{d:.0} m away; get within {BOARD_RANGE_M:.0} m"));
        }
        let st = self
            .world
            .get::<structure::Structure>(target)
            .ok_or("that vessel has no seats")?;
        let mut crew = self.crew(target);
        let seat = (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i] && st.crew[i] > 0)
            .find(|&i| crew.iter().filter(|(p, _)| *p == i).count() < st.crew[i] as usize)
            .ok_or("no free seat")?;
        crew.push((seat, name));
        self.set_crew(target, crew);
        // they climb in for a moment before they are gone
        let until_ut = self.ut() + BOARDING_S;
        self.world
            .entity_mut(kerbal)
            .remove::<OnLadder>()
            .insert(Boarding {
                vessel: target,
                until_ut,
            });
        Ok(())
    }

    /// A Kerbal climbing in: the vessel they are boarding.
    pub fn boarding(&self, kerbal: Entity) -> Option<Entity> {
        self.world.get::<Boarding>(kerbal).map(|b| b.vessel)
    }

    /// The ladders of a vessel: (part, foot, top), inertial offsets from the vessel's position, m. Ladder
    /// parts run along their own axis, and so does the built-in ladder of a crewed part (a pod's hatch
    /// side is not in the data: the whole height of the part counts).
    fn ladders(&self, e: Entity) -> Vec<(usize, DVec3, DVec3)> {
        let (Some(st), Some(p)) = (
            self.world.get::<structure::Structure>(e),
            self.world.get::<flight::Powered>(e),
        ) else {
            return Vec::new();
        };
        let c = st.rigid.rest_centre();
        (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i])
            .filter(|&i| st.part_ids[i].starts_with("ladder_") || st.crew[i] > 0)
            .map(|i| {
                let (centre, half) = st.part_box(i);
                let axis = st.axis_inertial(
                    p.facing,
                    crate::aero::rotate(st.part_rot[i], DVec3::new(0.0, 1.0, 0.0)),
                );
                let mut mid = st.offset_inertial(p.facing, centre - c);
                // the box's extent along the part's axis
                let a = crate::aero::rotate_inverse(st.part_rot[i], DVec3::new(0.0, 1.0, 0.0));
                let reach = (half.x * a.x.abs() + half.y * a.y.abs() + half.z * a.z.abs()).max(0.3);
                if !st.part_ids[i].starts_with("ladder_") {
                    // A pod's built-in ladder is on its hull, not down its middle: out from the vessel's
                    // axis, or on the craft's top side (the side the flight camera starts on) for a pod
                    // on the axis.
                    let radial = mid - p.facing * mid.dot(p.facing);
                    let out = if radial.length() > 0.1 {
                        radial.normalized()
                    } else {
                        st.offset_inertial(p.facing, DVec3::new(0.0, 0.0, 1.0))
                            .normalized()
                    };
                    // beside the hull itself at that height (a Mk1 pod narrows to its top), else its box
                    let across = st
                        .hull_taper(i)
                        .map(|t| t.radius_at((centre - st.rigid.rest_position(i)).dot(t.axis)))
                        .unwrap_or(
                            (half.x * (1.0 - a.x.abs()))
                                .max(half.y * (1.0 - a.y.abs()))
                                .max(half.z * (1.0 - a.z.abs())),
                        );
                    mid = mid + out * across;
                }
                (i, mid - axis * reach, mid + axis * reach)
            })
            .collect()
    }

    /// Grab the nearest ladder within reach (F), or let go of the one held.
    pub fn grab_ladder(&mut self, kerbal: Entity) -> Result<bool, &'static str> {
        if self.world.get::<EvaKerbal>(kerbal).is_none() {
            return Err("not a Kerbal on EVA");
        }
        if self.world.get::<OnLadder>(kerbal).is_some() {
            self.world.entity_mut(kerbal).remove::<OnLadder>();
            return Ok(false);
        }
        let me = self.position_of(kerbal).ok_or("not a Kerbal on EVA")?;
        let mut best: Option<(f64, Entity, usize, f64)> = None;
        for v in self.vessels() {
            if v == kerbal || self.world.get::<EvaKerbal>(v).is_some() {
                continue;
            }
            if self.distance_between(kerbal, v).is_none_or(|d| d > 60.0) {
                continue;
            }
            let Some(at) = self.position_of(v) else {
                continue;
            };
            for (part, foot, top) in self.ladders(v) {
                let (a, b) = (at + foot, at + top);
                let ab = b - a;
                let t = ((me - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let d = (a + ab * t - me).length();
                if best.is_none_or(|x| d < x.0) {
                    best = Some((d, v, part, t * ab.length()));
                }
            }
        }
        match best {
            Some((d, vessel, part, along_m)) if d <= GRAB_RANGE_M + LADDER_STANDOFF_M => {
                self.world.entity_mut(kerbal).insert(OnLadder {
                    vessel,
                    part,
                    along_m,
                    climb: 0.0,
                });
                Ok(true)
            }
            _ => Err("no ladder within reach"),
        }
    }

    /// The ladder a Kerbal holds: (vessel, part).
    pub fn on_ladder(&self, kerbal: Entity) -> Option<(Entity, usize)> {
        self.world
            .get::<OnLadder>(kerbal)
            .map(|l| (l.vessel, l.part))
    }

    /// Climb input on a ladder (-1..1); false when not on one.
    pub fn set_climb(&mut self, kerbal: Entity, climb: f64) -> bool {
        match self.world.get_mut::<OnLadder>(kerbal) {
            Some(mut l) => {
                l.climb = climb.clamp(-1.0, 1.0);
                true
            }
            None => false,
        }
    }

    /// Kerbals on ladders move with them and climb; those climbing in leave when they are through.
    pub(crate) fn run_ladders(&mut self, dt: f64) {
        let ut = self.ut();
        let done: Vec<Entity> = self
            .world
            .query::<(Entity, &Boarding)>()
            .iter(&self.world)
            .filter(|(_, b)| b.until_ut <= ut)
            .map(|(e, _)| e)
            .collect();
        for e in done {
            self.world.despawn(e);
        }
        let holding: Vec<(Entity, OnLadder)> = self
            .world
            .query::<(Entity, &OnLadder)>()
            .iter(&self.world)
            .map(|(e, l)| (e, *l))
            .collect();
        for (k, mut l) in holding {
            let ladder = self.ladders(l.vessel).into_iter().find(|x| x.0 == l.part);
            let (Some((_, foot, top)), Some(vp)) = (
                ladder,
                self.world
                    .get::<flight::Powered>(l.vessel)
                    .map(|p| (p.r, p.v, p.facing)),
            ) else {
                // the ladder went (staged, broken, on rails): let go
                self.world.entity_mut(k).remove::<OnLadder>();
                continue;
            };
            let (r, v, facing) = vp;
            let axis = top - foot;
            let len = axis.length().max(1e-6);
            let up = axis * (1.0 / len);
            l.along_m = (l.along_m + l.climb * CLIMB_SPEED_MPS * dt).clamp(0.0, len);
            // hang off the ladder on the side away from the vessel's axis
            let point = foot + up * l.along_m;
            let side = point - facing * point.dot(facing);
            let out = if side.length() > 1e-3 {
                side.normalized()
            } else if facing.cross(up).length() > 1e-3 {
                facing.cross(up).normalized()
            } else {
                crate::structure::basis(facing).0
            };
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(k) {
                p.r = r + point + out * LADDER_STANDOFF_M;
                p.v = v;
                p.facing = up;
                p.landed = false;
                p.angvel = DVec3::ZERO;
            }
            if let Some(mut h) = self.world.get_mut::<OnLadder>(k) {
                h.along_m = l.along_m;
            }
        }
    }

    /// Distance between two vessels around the same body, m.
    pub fn distance_between(&self, a: Entity, b: Entity) -> Option<f64> {
        let ba = self.world.get::<bodies::ReferenceBody>(a)?.0;
        let bb = self.world.get::<bodies::ReferenceBody>(b)?.0;
        if ba != bb {
            return None;
        }
        Some((self.position_of(a)? - self.position_of(b)?).length())
    }

    /// Body-centred position of a vessel now.
    pub fn position_of(&self, e: Entity) -> Option<DVec3> {
        match self.world.get::<flight::Powered>(e) {
            Some(p) => Some(p.r),
            None => Some(
                self.world
                    .get::<bodies::OnRails>(e)?
                    .0
                    .state_at(self.ut())
                    .0,
            ),
        }
    }

    /// A Kerbal standing on the ground plants a flag beside them.
    pub fn plant_flag(&mut self, kerbal: Entity, label: &str) -> Result<Entity, String> {
        let name = self
            .world
            .get::<EvaKerbal>(kerbal)
            .ok_or("only a Kerbal on EVA can plant a flag")?
            .name
            .clone();
        let p = self
            .world
            .get::<flight::Powered>(kerbal)
            .ok_or("no flight state")?
            .clone();
        if !p.landed || p.crashed {
            return Err("land first".into());
        }
        let body = self
            .world
            .get::<bodies::ReferenceBody>(kerbal)
            .ok_or("no body")?
            .0;
        let b = self
            .world
            .get::<bodies::Body>(body)
            .ok_or("no body")?
            .clone();
        let (lat, lon) = flight::lat_lon(&b, p.r, self.ut());
        let spec = flight::FlightSpec {
            stages: vec![flight::StageSpec {
                start_mass_t: 0.01,
                ..Default::default()
            }],
            drag_area_m2: 0.1,
            drag_cd: flight::DEFAULT_CD,
        };
        let dlon = 1.5 / (b.radius_m * lat.to_radians().cos().max(0.01)) * 57.29578;
        let flag = self.spawn_powered_on_surface(label, body, spec, lat, lon + dlon);
        self.world
            .entity_mut(flag)
            .insert(Flag { planted_by: name });
        let tick = self.tick().0;
        if let Some(mut log) = self.log_mut(kerbal) {
            log.event(tick, ose_data::missions::MissionEvent::FlagPlanted);
        }
        Ok(flag)
    }

    pub fn flag_planter(&self, e: Entity) -> Option<String> {
        self.world.get::<Flag>(e).map(|f| f.planted_by.clone())
    }

    /// Mark a restored vessel as a Kerbal on EVA / a flag.
    pub fn restore_eva_kerbal(&mut self, e: Entity, name: &str) {
        let settings = self
            .world
            .get_resource::<KerbalPart>()
            .map(|k| k.jetpack.clone())
            .unwrap_or_default();
        if let Some(mut p) = self.world.get_mut::<flight::Powered>(e) {
            if p.jetpack.is_none() {
                let vacuum = settings.isp_curve.evaluate(0.0);
                let sea_level = settings.isp_curve.evaluate(1.0);
                p.rcs.thrust_kn = settings.move_force_kn;
                p.rcs.isp_vac_s = vacuum;
                p.rcs.isp_asl_s = sea_level;
                if let Some(stage) = p.spec.stages.first_mut() {
                    stage.isp_vac_s = vacuum;
                    stage.isp_asl_s = sea_level;
                    stage.thrust_vac_kn = settings.move_force_kn;
                    stage.thrust_asl_kn = settings.move_force_kn * sea_level / vacuum.max(1e-9);
                }
                p.jetpack = Some(settings);
            }
        }

        let experiments = self
            .world
            .get_resource::<KerbalPart>()
            .map(|k| k.experiments.clone())
            .unwrap_or_default();
        self.world.entity_mut(e).insert(EvaKerbal {
            name: name.to_string(),
            experiments,
        });
    }

    pub fn restore_flag(&mut self, e: Entity, by: &str) {
        self.world.entity_mut(e).insert(Flag {
            planted_by: by.to_string(),
        });
    }

    pub fn set_jetpack(&mut self, jetpack: ose_data::Jetpack) {
        self.world
            .get_resource_or_insert_with(KerbalPart::default)
            .jetpack = jetpack;
    }

    /// The experiments an EVA Kerbal carries (from the `eva_kerbal` part).
    pub fn set_kerbal_part(&mut self, experiments: Vec<PartExperiment>) {
        let jetpack = self
            .world
            .get_resource::<KerbalPart>()
            .map(|k| k.jetpack.clone())
            .unwrap_or_default();
        self.world.insert_resource(KerbalPart {
            experiments,
            jetpack,
        });
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct KerbalPart {
    pub jetpack: ose_data::Jetpack,
    pub experiments: Vec<PartExperiment>,
}

/// So `missions` can see whether a vessel is a Kerbal.
pub(crate) fn is_eva(world: &World, e: Entity) -> bool {
    world.get::<EvaKerbal>(e).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k1_on_pad() -> (Universe, Entity) {
        craft_on_pad("Kerbal K1")
    }

    fn craft_on_pad(name: &str) -> (Universe, Entity) {
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
        let k1 = crafts.iter().find(|c| c.name == name).unwrap();
        let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let mut u = Universe::with_bodies(&[kerbin]);
        u.set_science(
            serde_json::from_str(include_str!("../../../assets/own/science/index.json")).unwrap(),
            include_bytes!("../../../assets/own/science/regions.bin").to_vec(),
        );
        let kerbal = defs.iter().find(|d| d.id == "eva_kerbal").unwrap();
        u.set_kerbal_part(kerbal.experiments.clone());
        let body = u.body_entity("Kerbin").unwrap();
        let v = u.spawn_powered_on_surface(name, body, spec, -0.6836, -74.8198);
        u.set_structure(v, structure::Structure::build(k1, &defs, &res));
        (u, v)
    }

    /// Let go of the ladder the Kerbal stepped out onto, and wait until they are on the ground: they land on
    /// the pod below the hatch, so they walk off it, away from the craft, and fall to the ground.
    fn let_go_and_land(u: &mut Universe, jeb: Entity) {
        let craft = u.on_ladder(jeb).expect("on a ladder").0;
        assert_eq!(
            u.grab_ladder(jeb),
            Ok(false),
            "stepped out holding the ladder"
        );
        for _ in 0..(20 * crate::TICKS_PER_SECOND) {
            let p = u.powered(jeb).unwrap();
            if p.landed && p.deck.is_none() {
                u.set_controls(jeb, 0.0, flight::Attitude::Free);
                return;
            }
            if p.landed {
                // on the craft: walk out from its axis
                let c = u.powered(craft).unwrap().r;
                let up = c.normalized();
                let d = p.r - c;
                let out = (d - up * d.dot(up)).normalized();
                u.set_controls(jeb, 1.0, flight::Attitude::Fixed(out));
            } else {
                u.set_controls(jeb, 0.0, flight::Attitude::Free);
            }
            u.step();
        }
        panic!("never reached the ground");
    }

    /// Height of a Kerbal's centre above the ground under it, m.
    fn height(u: &Universe, k: Entity) -> f64 {
        let body = u.body_entity("Kerbin").unwrap();
        let b = u.world.get::<bodies::Body>(body).unwrap();
        let r = u.powered(k).unwrap().r;
        r.length() - flight::ground_radius(b, None, r, u.ut())
    }

    #[test]
    fn letting_go_of_the_pod_ladder_lands_on_the_pod_and_walking_off_its_edge_falls_to_the_pad() {
        // KSP2 trace run 14 (reference/ksp2-run14-eva/eva-trace.csv): Valentina lets go of the Mk1 pod's
        // ladder at 112.32 m, drops 0.31 m onto the pod and stands there Landed at 0 m/s for 5 s; walking on,
        // she goes off its edge and falls 3.7 m to the pad. The same pod on a tank and a Terrier here.
        let (mut u, v) = craft_on_pad("Lesson 3.2 rocket");
        let jeb = u.go_eva(v, None).unwrap();
        u.step();
        let hang = height(&u, jeb);
        assert_eq!(u.grab_ladder(jeb), Ok(false));
        for _ in 0..(2 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(jeb).unwrap().clone();
        assert!(p.landed && !p.crashed, "came to rest");
        assert!(p.deck.is_some(), "on the craft");
        let on_pod = height(&u, jeb);
        let drop = hang - on_pod;
        assert!(
            (0.15..0.5).contains(&drop),
            "dropped {drop:.2} m onto the pod (KSP2: 0.31 m)"
        );
        // the pod stands on a 1.9 m tank and a 0.76 m engine: well above the pad
        assert!(on_pod > 2.5, "{on_pod:.2} m up: on the pad, not the pod");
        // standing still on it for 5 s, with the craft
        let rel = |u: &Universe| u.powered(jeb).unwrap().r - u.powered(v).unwrap().r;
        let r0 = rel(&u);
        for _ in 0..(5 * crate::TICKS_PER_SECOND) {
            u.step();
            assert!(u.powered(jeb).unwrap().landed, "stays standing");
        }
        let moved = (rel(&u) - r0).length();
        assert!(moved < 0.02, "slid {moved:.3} m on the pod");
        let pk = u.powered(jeb).unwrap();
        let pc = u.powered(v).unwrap();
        assert!((pk.v - pc.v).length() < 0.01, "moves with the craft");
        // walk out from the craft's axis until the pod is no longer under her
        let mut walked_off = false;
        for _ in 0..(5 * crate::TICKS_PER_SECOND) {
            let c = u.powered(v).unwrap().r;
            let up = c.normalized();
            let d = rel(&u);
            let out = (d - up * d.dot(up)).normalized();
            u.set_controls(jeb, 1.0, flight::Attitude::Fixed(out));
            u.step();
            if !u.powered(jeb).unwrap().landed {
                walked_off = true;
                break;
            }
        }
        assert!(walked_off, "walked off the pod's edge");
        u.set_controls(jeb, 0.0, flight::Attitude::Free);
        let mut fell = 0.0_f64;
        let top = height(&u, jeb);
        for _ in 0..(5 * crate::TICKS_PER_SECOND) {
            u.step();
            fell = fell.max(top - height(&u, jeb));
            if u.powered(jeb).unwrap().landed {
                break;
            }
        }
        let p = u.powered(jeb).unwrap();
        assert!(p.landed && !p.crashed, "landed on the pad");
        assert!(p.deck.is_none(), "on the ground, not the craft");
        assert!(fell > 2.0, "fell {fell:.2} m to the pad (KSP2: 3.7 m)");
        let on_pad = height(&u, jeb);
        assert!(
            (on_pad - 0.55).abs() < 0.01,
            "standing on the pad: {on_pad:.2} m"
        );
    }

    #[test]
    fn a_kerbal_standing_on_a_craft_rides_along_with_it() {
        let (mut u, v) = craft_on_pad("Lesson 3.2 rocket");
        let jeb = u.go_eva(v, None).unwrap();
        u.grab_ladder(jeb).unwrap();
        for _ in 0..(2 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        assert!(u.powered(jeb).unwrap().deck.is_some());
        // the deck moves at 2 m/s across the ground (as a rolling craft's hull would): she goes with it
        let mut p = u.powered(jeb).unwrap().clone();
        let up = p.r.normalized();
        let east = DVec3::new(0.0, 0.0, 1.0).cross(up).normalized();
        let deck = p.deck.unwrap();
        let start = p.r;
        let mut k = p.clone();
        for i in 0..crate::TICKS_PER_SECOND {
            k.deck = Some(flight::Deck {
                radius_m: deck.radius_m,
                v: deck.v + east * 2.0,
            });
            let b = u
                .world
                .get::<bodies::Body>(u.body_entity("Kerbin").unwrap())
                .unwrap();
            flight::integrate(&mut k, b, None, None, u.ut() + i as f64 / 50.0, 1.0 / 50.0);
        }
        p = k;
        assert!(p.landed);
        // the body turns under her too: compare with the ground's own turn
        let along = (p.r - start).dot(east) - deck.v.dot(east);
        assert!((along - 2.0).abs() < 0.05, "carried {along:.2} m");
    }

    #[test]
    fn in_orbit_the_kerbal_holds_on_while_the_craft_coasts_and_keeps_its_own_attitude() {
        let (mut u, v) = k1_on_pad();
        let body = u.body_entity("Kerbin").unwrap();
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let mut p = u.powered(v).unwrap().clone();
        p.r = DVec3::new(700_000.0, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / 700_000.0_f64).sqrt(), 0.0);
        p.landed = false;
        p.attitude = flight::Attitude::Free;
        u.set_powered(v, p);
        let jeb = u.go_eva(v, None).unwrap();
        // a Kerbal has no SAS to turn it toward a mode
        assert_eq!(u.powered(jeb).unwrap().attitude, flight::Attitude::Free);
        // a quiet coast would put the craft on rails after ten seconds; with a Kerbal on its ladder it stays
        for _ in 0..(40 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        assert_eq!(u.on_ladder(jeb), Some((v, 0)), "still holding on");
        let (pv, pj) = (u.powered(v).unwrap(), u.powered(jeb).unwrap());
        assert!(
            (pv.angvel + pv.facing * pv.roll_rate).length() < 1e-4,
            "the craft does not turn"
        );
        assert!((pv.r - pj.r).length() < 15.0, "together");
    }

    #[test]
    fn stepping_out_does_not_turn_the_craft() {
        for orbit in [false, true] {
            let (mut u, v) = k1_on_pad();
            if orbit {
                let body = u.body_entity("Kerbin").unwrap();
                let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
                let mut p = u.powered(v).unwrap().clone();
                p.r = DVec3::new(700_000.0, 0.0, 0.0);
                p.v = DVec3::new(0.0, (mu / 700_000.0_f64).sqrt(), 0.0);
                p.landed = false;
                p.attitude = flight::Attitude::Fixed(p.facing);
                u.set_powered(v, p);
                // long enough to be put on rails
                for _ in 0..(20 * crate::TICKS_PER_SECOND) {
                    u.step();
                }
            }
            let jeb = u.go_eva(v, None).unwrap();
            let f0 = u.powered(v).unwrap().facing;
            for _ in 0..(5 * crate::TICKS_PER_SECOND) {
                u.step();
            }
            let p = u.powered(v).unwrap();
            let w = (p.angvel + p.facing * p.roll_rate).length();
            assert!(w < 1e-3, "orbit {orbit}: spinning at {w} rad/s");
            // on the pad the body's turn shows in the inertial facing: 5 s of it is a few hundredths
            let turned = p.facing.dot(f0).clamp(-1.0, 1.0).acos().to_degrees();
            assert!(turned < 0.2, "orbit {orbit}: turned {turned} degrees");
            assert!(u.on_ladder(jeb).is_some());
        }
    }

    #[test]
    fn a_kerbal_steps_out_onto_the_hatch_ladder_and_holds_on() {
        let (mut u, v) = k1_on_pad();
        let jeb = u.go_eva(v, None).unwrap();
        assert_eq!(u.on_ladder(jeb), Some((v, 0)), "on the pod's ladder");
        u.step();
        // beside the pod's hull, level with it, not inside it or down by the engines
        let (k, p) = (u.powered(jeb).unwrap().r, u.powered(v).unwrap().clone());
        let st = u.structure(v).unwrap();
        let (pod, half) = st.part_box(0);
        let pod_up = (pod - st.rigid.rest_centre()).y;
        let d = k - p.r;
        let up = d.dot(p.facing);
        let out = (d - p.facing * up).length();
        assert!(
            (up - pod_up).abs() < half.y + 0.1,
            "level with the pod ({up} vs {pod_up} m)"
        );
        assert!(
            out > half.x.max(half.z),
            "outside the hull ({out} m from the axis)"
        );
        let d0 = u.distance_between(jeb, v).unwrap();
        for _ in 0..(3 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        assert_eq!(u.on_ladder(jeb), Some((v, 0)), "still holding on");
        let d = u.distance_between(jeb, v).unwrap();
        assert!(
            (d - d0).abs() < 0.05,
            "stays with the craft ({d0} -> {d} m)"
        );
        assert!(!u.powered(jeb).unwrap().landed, "not dropped to the ground");
        // and in orbit, beside a craft on rails
        let body = u.body_entity("Kerbin").unwrap();
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let mut p = u.powered(v).unwrap().clone();
        u.grab_ladder(jeb).unwrap();
        u.board(jeb, v).unwrap();
        for _ in 0..crate::TICKS_PER_SECOND {
            u.step();
        }
        p.r = DVec3::new(700_000.0, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / 700_000.0_f64).sqrt(), 0.0);
        p.landed = false;
        u.set_powered(v, p);
        let jeb = u.go_eva(v, None).unwrap();
        let d0 = u.distance_between(jeb, v).unwrap();
        for _ in 0..(3 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        assert_eq!(u.on_ladder(jeb), Some((v, 0)));
        let d = u.distance_between(jeb, v).unwrap();
        assert!(
            (d - d0).abs() < 0.05,
            "floats with the craft ({d0} -> {d} m)"
        );
    }

    #[test]
    fn a_kerbal_beside_his_landed_craft_finds_it_for_recovery() {
        let (mut u, v) = k1_on_pad();
        for _ in 0..crate::TICKS_PER_SECOND {
            u.step();
        }
        let jeb = u.go_eva(v, None).unwrap();
        let_go_and_land(&mut u, jeb);
        assert_eq!(u.landed_craft_near(jeb, 50.0), vec![v]);
        assert!(
            u.landed_craft_near(v, 50.0).is_empty(),
            "only a Kerbal on EVA"
        );
        // out of reach once the craft stands 100 m away
        let mut p = u.powered(v).unwrap().clone();
        let r = p.r;
        let side = r.cross(DVec3::new(0.0, 0.0, 1.0)).normalized();
        p.r = (r + side * 100.0).normalized() * r.length();
        u.set_powered(v, p);
        assert!(u.landed_craft_near(jeb, 50.0).is_empty());
    }

    #[test]
    fn jeb_walks_out_plants_a_flag_takes_a_sample_and_climbs_back_in() {
        let (mut u, v) = k1_on_pad();
        assert_eq!(u.crew(v), vec![(0, "Jebediah Kerman".to_string())]);
        let jeb = u.go_eva(v, None).unwrap();
        assert!(u.crew(v).is_empty());
        assert_eq!(u.eva_kerbal(jeb).unwrap().name, "Jebediah Kerman");
        let_go_and_land(&mut u, jeb);
        let d = u.distance_between(jeb, v).unwrap();
        assert!(d < 12.0, "at the foot of the craft ({d} m)");
        // a flag on the KSC
        let flag = u.plant_flag(jeb, "Agency flag").unwrap();
        assert!(u.is_flag(flag));
        let log = u.flight_log(jeb).unwrap();
        assert!(log
            .events
            .iter()
            .any(|(_, e)| *e == ose_data::missions::MissionEvent::FlagPlanted));
        // his surface sample takes its 10 s
        let out = u.run_experiments(jeb);
        assert!(
            out.started.iter().any(|r| r.experiment == "SurfaceSurvey"),
            "{out:?}"
        );
        for _ in 0..11 {
            u.advance_experiments(1.0);
        }
        assert!(u
            .science_carried(jeb)
            .iter()
            .any(|r| r.sample && r.location == "Kerbin_Landed_KerbinKSC"));
        // too far from the pod's ladder up on top of the rocket
        assert!(u.grab_ladder(jeb).is_err());
        // up beside it (as he would be stepping out of the hatch): grab it, climb, let go
        let (_, foot, _) = u.ladders(v).into_iter().find(|l| l.0 == 0).unwrap();
        let at = u.position_of(v).unwrap() + foot;
        let out = at
            .normalized()
            .cross(DVec3::new(0.0, 0.0, 1.0))
            .normalized();
        u.world.get_mut::<flight::Powered>(jeb).unwrap().r = at + out * 1.0;
        assert!(u.grab_ladder(jeb).unwrap(), "the pod is within reach");
        assert_eq!(u.on_ladder(jeb).map(|l| l.0), Some(v));
        u.step();
        let low = u.position_of(jeb).unwrap() - u.position_of(v).unwrap();
        assert!(u.set_climb(jeb, 1.0));
        for _ in 0..crate::TICKS_PER_SECOND {
            u.step();
        }
        let high = u.position_of(jeb).unwrap() - u.position_of(v).unwrap();
        let up = u.position_of(v).unwrap().normalized();
        assert!((high - low).dot(up) > 0.3, "climbed {:?}", high - low);
        assert!(u.set_eva(jeb, false, true), "Space lets go");
        assert!(u.on_ladder(jeb).is_none());
        // and back in: the seat is taken at once, the Kerbal is gone once through the hatch
        u.board(jeb, v).unwrap();
        assert_eq!(u.crew(v).len(), 1);
        assert_eq!(u.boarding(jeb), Some(v));
        for _ in 0..(2 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        assert!(u.vessel(jeb).is_none());
    }

    #[test]
    fn a_kerbal_walking_into_its_craft_stops_at_the_hull() {
        let (mut u, v) = k1_on_pad();
        let jeb = u.go_eva(v, None).unwrap();
        let_go_and_land(&mut u, jeb);
        let level = |x: DVec3, up: DVec3| x - up * x.dot(up);
        // walk straight at the rocket for a while
        for _ in 0..(20 * crate::TICKS_PER_SECOND) {
            let craft = u.powered(v).unwrap().r;
            let k = u.powered(jeb).unwrap().r;
            let toward = level(craft - k, craft.normalized()).normalized();
            u.set_controls(jeb, 1.0, flight::Attitude::Fixed(toward));
            u.step();
        }
        let craft = u.powered(v).unwrap().r;
        let k = u.powered(jeb).unwrap();
        assert!(k.landed && !k.crashed);
        let apart = level(k.r - craft, craft.normalized()).length();
        // the K1's pod and tanks are 1.25 m across: it stands outside them
        assert!(
            apart > 0.6,
            "walked into the craft: {apart:.2} m from its axis"
        );
        assert!(apart < 2.0, "it did get to the hull: {apart:.2} m");
    }

    #[test]
    fn a_kerbal_walks_across_the_ground() {
        let (mut u, v) = k1_on_pad();
        let jeb = u.go_eva(v, None).unwrap();
        let_go_and_land(&mut u, jeb);
        let fuel = u.powered(jeb).unwrap().propellant_t;
        let body = u.body_entity("Kerbin").unwrap();
        let b = u.world.get::<bodies::Body>(body).unwrap().clone();
        let here = |u: &Universe| flight::lat_lon(&b, u.position_of(jeb).unwrap(), u.ut());
        let start = here(&u);
        // heading north at a level pitch, full throttle: walking, no propellant used
        u.set_controls(
            jeb,
            1.0,
            flight::Attitude::PitchHeading {
                pitch_deg: 0.0,
                heading_deg: 0.0,
            },
        );
        for _ in 0..(4 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(jeb).unwrap();
        assert!(p.landed);
        assert_eq!(p.propellant_t, fuel, "walking uses no propellant");
        // across the surface (the body turns under it too): KSP2 trace run 14 walks at 0.65 m/s (its 0.8 m/s
        // pace less the ground's friction each tick), about 2.6 m north
        let end = here(&u);
        let north_m = (end.0 - start.0).to_radians() * b.radius_m;
        let east_m = (end.1 - start.1).to_radians() * b.radius_m;
        assert!((2.4..2.8).contains(&north_m), "walked {north_m} m north");
        assert!(east_m.abs() < 1.0, "{east_m} m east");
        // and its velocity says so, for the client to carry it between snapshots
        let omega = std::f64::consts::TAU / b.rotation_period_s;
        let over_ground = p.v - DVec3::new(0.0, 0.0, omega).cross(p.r);
        assert!(
            (over_ground.length() - 0.65).abs() < 0.02,
            "{over_ground:?}"
        );
        // running: 2.05 m/s in the trace once it has eased up from the walk over a quarter second
        assert!(u.set_eva(jeb, true, false));
        for _ in 0..(4 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        let ran = (here(&u).0 - end.0).to_radians() * b.radius_m;
        assert!((7.6..8.4).contains(&ran), "ran {ran} m");
        // a jump leaves the ground, carries on forward and comes back down: KSP2's push of 1.36 kN lifts a
        // Kerbal 0.277 m in trace run 14
        let ground = u.powered(jeb).unwrap().r.length();
        assert!(u.set_eva(jeb, true, true));
        u.step();
        u.step();
        assert!(!u.powered(jeb).unwrap().landed, "in the air");
        let mut top: f64 = 0.0;
        for _ in 0..(3 * crate::TICKS_PER_SECOND) {
            u.step();
            top = top.max(u.powered(jeb).unwrap().r.length() - ground);
        }
        assert!((0.26..0.30).contains(&top), "jumped {top} m");
        let p = u.powered(jeb).unwrap();
        assert!(p.landed && !p.crashed);
        assert_eq!(p.propellant_t, fuel, "walking and jumping burn nothing");
        // the jetpack lifts it straight up (Translate along the Kerbal's axis)
        u.set_controls(jeb, 0.0, flight::Attitude::Up);
        assert!(u.set_translate(jeb, DVec3::new(0.0, 1.0, 0.0)));
        for _ in 0..crate::TICKS_PER_SECOND {
            u.step();
        }
        let p = u.powered(jeb).unwrap();
        assert!(p.propellant_t < JETPACK_PROPELLANT_T, "the pack fires");
        // 0.25 kN on a Kerbal is too weak to lift off Kerbin (as in the original); it is for low gravity
        assert!(p.landed);
    }

    #[test]
    fn eva_universe_resolves_partial_fuel_once_and_old_saves_keep_their_remaining_pack() {
        let (mut u, vessel) = k1_on_pad();
        let body = u.body_entity("Kerbin").unwrap();
        let mut carrier = u.powered(vessel).unwrap().clone();
        carrier.r = DVec3::new(700000.0, 0.0, 0.0);
        carrier.v = DVec3::new(0.0, 100.0, 0.0);
        carrier.landed = false;
        carrier.facing = DVec3::new(1.0, 0.0, 0.0);
        u.set_powered(vessel, carrier);
        let eva = u.go_eva(vessel, Some("Jebediah Kerman")).unwrap();
        u.grab_ladder(eva).unwrap();
        let mut p = u.powered(eva).unwrap().clone();
        let dt = 1.0 / crate::TICKS_PER_SECOND as f64;
        let fuel = 0.25 / (240.0 * ose_data::G0) * dt * 0.25;
        p.r = DVec3::new(700000.0, 0.0, 0.0);
        p.v = DVec3::new(0.0, 100.0, 0.0);
        p.facing = DVec3::new(1.0, 0.0, 0.0);
        p.attitude = flight::Attitude::Free;
        p.throttle = 1.0;
        p.rcs.translate = DVec3::new(0.0, 1.0, 0.0);
        p.propellant_t = fuel;
        p.mass_t = KERBAL_MASS_T + fuel;
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let expected = (0.25 * 0.25 / KERBAL_MASS_T - mu / 700000.0_f64.powi(2)) * dt;
        u.set_powered(eva, p);
        u.step();
        let after = u.powered(eva).unwrap();
        assert!((after.v.x - expected).abs() < 1e-10);
        assert!((after.mass_t - KERBAL_MASS_T).abs() < 1e-12 && after.propellant_t < 1e-12);
        u.step();
        assert!((u.powered(eva).unwrap().mass_t - KERBAL_MASS_T).abs() < 1e-12);
        let mut old = serde_json::to_value(u.powered(eva).unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("jetpack");
        let mut restored: flight::Powered = serde_json::from_value(old).unwrap();
        restored.propellant_t = 0.0123;
        restored.mass_t = 0.0823;
        u.set_powered(eva, restored);
        u.restore_eva_kerbal(eva, "Jebediah Kerman");
        let after = u.powered(eva).unwrap();
        assert_eq!(after.propellant_t, 0.0123);
        assert_eq!(after.mass_t, 0.0823);
        assert_eq!(
            after.jetpack.as_ref().unwrap().isp_curve.evaluate(0.0),
            240.0
        );
        assert_eq!(after.rcs.isp_vac_s, 240.0);
    }

    #[test]
    fn the_jetpack_pushes_a_kerbal_in_orbit() {
        let (mut u, v) = k1_on_pad();
        let body = u.body_entity("Kerbin").unwrap();
        let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
        let mut p = u.powered(v).unwrap().clone();
        let r = 700_000.0;
        p.r = DVec3::new(r, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / r).sqrt(), 0.0);
        p.landed = false;
        u.set_powered(v, p);
        let jeb = u.go_eva(v, Some("Jebediah Kerman")).unwrap();
        assert_eq!(u.grab_ladder(jeb), Ok(false), "lets go of the ladder");
        let v0 = u.powered(jeb).unwrap().v;
        u.set_controls(jeb, 1.0, flight::Attitude::OrbitPrograde);
        for _ in 0..(5 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(jeb).unwrap();
        let dv = (p.v - v0).length();
        // 0.25 kN on 0.095 t is about 2.6 m/s^2; gravity turns the velocity too, so just check it pushed
        assert!(p.propellant_t < JETPACK_PROPELLANT_T, "used propellant");
        assert!(dv > 5.0, "dv {dv}");
        // too far from the craft now to board
        assert!(u.board(jeb, v).is_err());
    }
}

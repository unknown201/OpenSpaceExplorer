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

/// Jetpack thrust (`JetpackMoveForce`), kN.
pub const JETPACK_THRUST_KN: f64 = 0.25;
/// Legacy constant-Isp approximation. The reference JetpackResourceManager uses
/// Data_Kerbal.JetpackAtmosphereCurve (240 s vacuum, 100 s at one atmosphere).
pub const JETPACK_ISP_S: f64 = 100.0;
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
        let (_, kname) = crew.remove(idx);
        self.set_crew(e, crew);
        let spec = flight::FlightSpec {
            stages: vec![flight::StageSpec {
                stage_id: 0,
                thrust_vac_kn: JETPACK_THRUST_KN,
                thrust_asl_kn: JETPACK_THRUST_KN,
                isp_vac_s: JETPACK_ISP_S,
                isp_asl_s: JETPACK_ISP_S,
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
            p.rcs.thrust_kn = JETPACK_THRUST_KN;
            p.rcs.isp_vac_s = JETPACK_ISP_S;
            p.rcs.isp_asl_s = JETPACK_ISP_S;
            p.groups.rcs = true;
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
        Ok(kerbal)
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
                let mid = st.offset_inertial(p.facing, centre - c);
                // the box's extent along the part's axis
                let a = crate::aero::rotate_inverse(st.part_rot[i], DVec3::new(0.0, 1.0, 0.0));
                let reach = (half.x * a.x.abs() + half.y * a.y.abs() + half.z * a.z.abs()).max(0.3);
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
            } else {
                facing.cross(up).normalized()
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

    /// The experiments an EVA Kerbal carries (from the `eva_kerbal` part).
    pub fn set_kerbal_part(&mut self, experiments: Vec<PartExperiment>) {
        self.world.insert_resource(KerbalPart { experiments });
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct KerbalPart {
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
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
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
        let v = u.spawn_powered_on_surface("K1", body, spec, -0.6836, -74.8198);
        u.set_structure(v, structure::Structure::build(k1, &defs, &res));
        (u, v)
    }

    #[test]
    fn jeb_walks_out_plants_a_flag_takes_a_sample_and_climbs_back_in() {
        let (mut u, v) = k1_on_pad();
        assert_eq!(u.crew(v), vec![(0, "Jebediah Kerman".to_string())]);
        let jeb = u.go_eva(v, None).unwrap();
        assert!(u.crew(v).is_empty());
        assert_eq!(u.eva_kerbal(jeb).unwrap().name, "Jebediah Kerman");
        let d = u.distance_between(jeb, v).unwrap();
        assert!((2.0..8.0).contains(&d), "beside the craft ({d} m)");
        assert!(u.powered(jeb).unwrap().landed);
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
    fn a_kerbal_walks_across_the_ground() {
        let (mut u, v) = k1_on_pad();
        let jeb = u.go_eva(v, None).unwrap();
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
        assert_eq!(p.propellant_t, JETPACK_PROPELLANT_T);
        // across the surface (the body turns under it too): 0.8 m/s, about 3.2 m north
        let end = here(&u);
        let north_m = (end.0 - start.0).to_radians() * b.radius_m;
        let east_m = (end.1 - start.1).to_radians() * b.radius_m;
        assert!((2.8..3.6).contains(&north_m), "walked {north_m} m north");
        assert!(east_m.abs() < 1.0, "{east_m} m east");
        // running: 2.2 m/s
        assert!(u.set_eva(jeb, true, false));
        for _ in 0..(4 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        let ran = (here(&u).0 - end.0).to_radians() * b.radius_m;
        assert!((8.0..9.6).contains(&ran), "ran {ran} m");
        // a jump leaves the ground, carries on forward and comes back down
        assert!(u.set_eva(jeb, true, true));
        u.step();
        u.step();
        assert!(!u.powered(jeb).unwrap().landed, "in the air");
        for _ in 0..(3 * crate::TICKS_PER_SECOND) {
            u.step();
        }
        let p = u.powered(jeb).unwrap();
        assert!(p.landed && !p.crashed);
        assert_eq!(
            p.propellant_t, JETPACK_PROPELLANT_T,
            "walking and jumping burn nothing"
        );
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

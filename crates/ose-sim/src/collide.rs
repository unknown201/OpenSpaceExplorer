//! Vessels and debris striking each other: each part a sphere round its box, contacts between the parts of
//! two nearby vessels resolved with equal and opposite impulses (restitution and friction), and the parts
//! that take a blow harder than their crash tolerance (and than KSP2's `IsSurvivableCrash`, speed times the
//! striking part's mass, allows) break off. Docking ports facing each other are left
//! to docking, which stops them at their faces. Our own first model: no reference traces of the original's PhysX contacts yet.

use bevy_ecs::prelude::*;
use ose_math::DVec3;

use crate::{bodies, flight, structure, Universe};

/// Bounce of a part-to-part contact (our own figure).
pub const RESTITUTION: f64 = 0.2;
/// Sliding friction of a part-to-part contact (our own figure).
pub const FRICTION: f64 = 0.5;
/// How much of a part's box the sphere round it covers (the half diagonal reaches the corners; parts are
/// rarely their whole box).
const SPHERE_SHARE: f64 = 0.8;

/// Whether a part struck by another at `speed`, m/s, breaks (`PartBehavior`): faster than its crash tolerance,
/// and with the speed times the mass of the part that struck it, t, over that tolerance too
/// (`IsSurvivableCrash`).
pub fn crash_breaks(speed: f64, other_mass_t: f64, tolerance: f64) -> bool {
    speed > tolerance && speed * other_mass_t > tolerance
}

/// One part as the contact test sees it: (part, centre inertial offset from the vessel, radius).
fn part_spheres(st: &structure::Structure, facing: DVec3) -> Vec<(usize, DVec3, f64)> {
    let c = st.rigid.rest_centre();
    (0..st.rigid.len())
        .filter(|&i| st.rigid.present[i])
        .map(|i| {
            let (centre, half) = st.part_box(i);
            (
                i,
                st.offset_inertial(facing, centre - c),
                (half.length() * SPHERE_SHARE).max(0.1),
            )
        })
        .collect()
}

/// A vessel's moment of inertia for a blow, t·m²: its stage's figure at its mass, or a ball of its size.
fn inertia(p: &flight::Powered, reach: f64) -> f64 {
    let moi = p
        .current_stage()
        .or_else(|| p.spec.stages.last())
        .map(|s| {
            let ratio = if s.start_mass_t > 0.0 {
                (p.mass_t / s.start_mass_t).clamp(0.05, 1.5)
            } else {
                1.0
            };
            s.moi_tm2 * ratio
        })
        .unwrap_or(0.0);
    if moi > 0.0 {
        moi
    } else {
        0.4 * p.mass_t.max(0.01) * reach.max(0.5).powi(2)
    }
}

impl Universe {
    /// Vessel `e`'s moment of inertia for a blow, t·m² (as a contact uses it).
    pub(crate) fn blow_inertia(&self, e: Entity) -> Option<f64> {
        let p = self.world.get::<flight::Powered>(e)?;
        let st = self.world.get::<structure::Structure>(e)?;
        let reach = part_spheres(st, p.facing)
            .iter()
            .map(|(_, o, r)| o.length() + r)
            .fold(0.0, f64::max);
        Some(inertia(p, reach))
    }
}

/// A piece that has just come off a vessel does not strike it for a moment (they start out overlapping).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct SeparatedFrom {
    pub other: Entity,
    pub until_ut: f64,
}

/// How long a separated piece and its vessel pass through each other, s (our own figure).
pub const SEPARATION_GRACE_S: f64 = 2.0;

/// A vessel's parts as spheres: (part, inertial offset from the vessel, radius).
type Spheres = Vec<(usize, DVec3, f64)>;

/// What a contact did: the vessels, the parts that touched, and how hard (m/s along the normal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub a: Entity,
    pub part_a: usize,
    pub b: Entity,
    pub part_b: usize,
    pub speed_mps: f64,
}

/// Contacts since the last `take_contacts`.
#[derive(Resource, Default)]
pub struct Contacts(pub Vec<Contact>);

impl Universe {
    /// Vessels under power near each other strike part to part: their velocities and spins change by equal
    /// and opposite impulses, they are pushed apart, and parts hit harder than they can take break off
    /// (a vessel whose root breaks is lost). Landed vessels hold their ground.
    pub(crate) fn collide_vessels(&mut self) {
        let list: Vec<(Entity, Entity)> = {
            let mut q = self
                .world
                .query::<(Entity, &flight::Powered, &bodies::ReferenceBody)>();
            q.iter(&self.world)
                .filter(|(_, p, _)| !p.crashed)
                .map(|(e, _, b)| (e, b.0))
                .collect()
        };
        if list.len() < 2 {
            return;
        }
        // each vessel's parts and reach
        // vessels with parts (a Kerbal on EVA does not strike hulls yet)
        let shapes: Vec<(Entity, Entity, Spheres, f64)> = list
            .iter()
            .filter_map(|&(e, body)| {
                let p = self.world.get::<flight::Powered>(e)?;
                let spheres = part_spheres(self.world.get::<structure::Structure>(e)?, p.facing);
                let reach = spheres
                    .iter()
                    .map(|(_, o, r)| o.length() + r)
                    .fold(0.0, f64::max);
                Some((e, body, spheres, reach))
            })
            .collect();
        let mut broken: Vec<(Entity, usize)> = Vec::new();
        let mut events = Vec::new();
        for i in 0..shapes.len() {
            for j in (i + 1)..shapes.len() {
                let (a, body_a, ref sa, reach_a) = shapes[i];
                let (b, body_b, ref sb, reach_b) = shapes[j];
                if body_a != body_b {
                    continue;
                }
                let ut = self.ut();
                let apart = |x: Entity, y: Entity| {
                    self.world
                        .get::<SeparatedFrom>(x)
                        .is_some_and(|s| s.other == y && s.until_ut > ut)
                };
                // pieces that came off the same vessel together, too
                let siblings = match (
                    self.world.get::<SeparatedFrom>(a),
                    self.world.get::<SeparatedFrom>(b),
                ) {
                    (Some(x), Some(y)) => x.other == y.other && x.until_ut > ut && y.until_ut > ut,
                    _ => false,
                };
                if apart(a, b) || apart(b, a) || siblings {
                    continue;
                }
                let (Some(pa), Some(pb)) = (
                    self.world.get::<flight::Powered>(a).cloned(),
                    self.world.get::<flight::Powered>(b).cloned(),
                ) else {
                    continue;
                };
                if (pa.r - pb.r).length() > reach_a + reach_b {
                    continue;
                }
                // the deepest overlap between their parts
                let mut best: Option<(f64, usize, usize, DVec3, DVec3)> = None;
                for &(ia, oa, ra) in sa {
                    for &(ib, ob, rb) in sb {
                        let d = (pa.r + oa) - (pb.r + ob);
                        let depth = ra + rb - d.length();
                        if depth > 0.0 && best.is_none_or(|x| depth > x.0) {
                            best = Some((depth, ia, ib, oa, ob));
                        }
                    }
                }
                let Some((depth, ia, ib, oa, ob)) = best else {
                    continue;
                };
                // ports facing each other meet at their faces in the docking step instead
                if self
                    .world
                    .get_resource::<crate::docking::AlignedPorts>()
                    .is_some_and(|x| x.contains(a, ia, b, ib))
                {
                    continue;
                }
                let d = (pa.r + oa) - (pb.r + ob);
                let n = if d.length() > 1e-9 {
                    d.normalized()
                } else {
                    (pa.r - pb.r).normalized()
                };
                // contact point offsets from each centre: where the spheres meet
                let ra_c = oa - n * (depth * 0.5);
                let rb_c = ob + n * (depth * 0.5);
                let spin = |p: &flight::Powered| p.angvel + p.facing * p.roll_rate;
                let va = pa.v + spin(&pa).cross(ra_c);
                let vb = pb.v + spin(&pb).cross(rb_c);
                let rel = va - vb;
                let vn = rel.dot(n);
                // landed vessels hold their ground
                let inv_ma = if pa.landed {
                    0.0
                } else {
                    1.0 / pa.mass_t.max(0.001)
                };
                let inv_mb = if pb.landed {
                    0.0
                } else {
                    1.0 / pb.mass_t.max(0.001)
                };
                let inv_ia = if pa.landed {
                    0.0
                } else {
                    1.0 / inertia(&pa, reach_a)
                };
                let inv_ib = if pb.landed {
                    0.0
                } else {
                    1.0 / inertia(&pb, reach_b)
                };
                if inv_ma + inv_mb <= 0.0 {
                    continue;
                }
                let mut new_a = pa.clone();
                let mut new_b = pb.clone();
                if vn < 0.0 {
                    let k = inv_ma
                        + inv_mb
                        + ra_c.cross(n).length_squared() * inv_ia
                        + rb_c.cross(n).length_squared() * inv_ib;
                    let jn = -(1.0 + RESTITUTION) * vn / k;
                    // friction against the sliding, at most FRICTION of the normal blow
                    let slide = rel - n * vn;
                    let jt = if slide.length() > 1e-6 {
                        let t = slide.normalized();
                        let kt = inv_ma
                            + inv_mb
                            + ra_c.cross(t).length_squared() * inv_ia
                            + rb_c.cross(t).length_squared() * inv_ib;
                        -t * (slide.length() / kt).min(FRICTION * jn)
                    } else {
                        DVec3::ZERO
                    };
                    let impulse = n * jn + jt;
                    let apply =
                        |p: &mut flight::Powered, j: DVec3, r: DVec3, inv_m: f64, inv_i: f64| {
                            p.v = p.v + j * inv_m;
                            let dw = r.cross(j) * inv_i;
                            let axial = dw.dot(p.facing);
                            p.roll_rate += axial;
                            p.angvel = p.angvel + (dw - p.facing * axial);
                        };
                    apply(&mut new_a, impulse, ra_c, inv_ma, inv_ia);
                    apply(&mut new_b, -impulse, rb_c, inv_mb, inv_ib);
                    events.push(Contact {
                        a,
                        part_a: ia,
                        b,
                        part_b: ib,
                        speed_mps: -vn,
                    });
                    // what the blow breaks (`PartBehavior.OnCollisionEnter` / `IsSurvivableCrash`): a part struck
                    // faster than its crash tolerance breaks unless the speed times the mass of the part that
                    // struck it is within that tolerance too, so light debris cannot break a heavy part
                    let speed = rel.length();
                    let part_mass = |e: Entity, part: usize, p: &flight::Powered| {
                        self.world
                            .get::<structure::Structure>(e)
                            .and_then(|st| st.rigid.masses_t.get(part).copied())
                            .unwrap_or(p.mass_t)
                    };
                    let (mass_a, mass_b) = (part_mass(a, ia, &pa), part_mass(b, ib, &pb));
                    for (e, part, other_mass) in [(a, ia, mass_b), (b, ib, mass_a)] {
                        let tolerance = self
                            .world
                            .get::<structure::Structure>(e)
                            .and_then(|st| st.crash_tolerance.get(part).copied())
                            .filter(|t| *t > 0.0)
                            .unwrap_or(flight::CRASH_SPEED_MPS);
                        if crash_breaks(speed, other_mass, tolerance) {
                            broken.push((e, part));
                        }
                    }
                }
                // and they no longer overlap
                let push = depth / (inv_ma + inv_mb);
                new_a.r = new_a.r + n * (push * inv_ma);
                new_b.r = new_b.r - n * (push * inv_mb);
                if new_a.landed {
                    new_a.r = pa.r;
                }
                if new_b.landed {
                    new_b.r = pb.r;
                }
                self.world.entity_mut(a).insert(new_a);
                self.world.entity_mut(b).insert(new_b);
            }
        }
        if !events.is_empty() {
            self.world
                .get_resource_or_insert_with(Contacts::default)
                .0
                .extend(events);
        }
        for (e, part) in broken {
            self.break_struck_part(e, part);
        }
    }

    /// A part struck too hard leaves its vessel; what only it held breaks off as debris. The vessel is lost
    /// when it is the root.
    pub(crate) fn break_struck_part(&mut self, e: Entity, part: usize) {
        let Some(p) = self.world.get::<flight::Powered>(e).cloned() else {
            return;
        };
        if part == usize::MAX {
            if let Some(mut q) = self.world.get_mut::<flight::Powered>(e) {
                q.crashed = true;
                q.throttle = 0.0;
            }
            return;
        }
        let Some(mut st) = self.world.entity_mut(e).take::<structure::Structure>() else {
            return;
        };
        if !st.rigid.present.get(part).copied().unwrap_or(false) {
            self.world.entity_mut(e).insert(st);
            return;
        }
        let mut q = p.clone();
        let centre = st.mass_properties().centre_m;
        match st.burn_up(&[part], p.facing) {
            Some(pieces) => {
                st.keep_in_place(&mut q, centre);
                st.sync(&mut q);
                self.world.entity_mut(e).insert(st).insert(q.clone());
                let area = q.spec.drag_area_m2;
                for piece in pieces {
                    if let Some(d) = self.spawn_debris_piece(e, &piece, q.r, q.v, q.facing, area) {
                        self.broken_off.push((e, d));
                    }
                }
            }
            None => {
                q.crashed = true;
                q.throttle = 0.0;
                self.world.entity_mut(e).insert(st).insert(q);
            }
        }
    }

    /// Spawn a broken-off piece of `e` as debris, tumbling away.
    fn spawn_debris_piece(
        &mut self,
        e: Entity,
        piece: &structure::Piece,
        r: DVec3,
        v: DVec3,
        facing: DVec3,
        area: f64,
    ) -> Option<Entity> {
        self.spawn_debris(
            e,
            piece.mass_t,
            r + piece.offset_m,
            v + piece.dv_mps,
            facing,
            area,
            &piece.parts,
            piece.spin,
            None,
        )
    }

    /// Contacts between vessels since the last call (for the client's effects and sounds).
    pub fn take_contacts(&mut self) -> Vec<Contact> {
        self.world
            .get_resource_mut::<Contacts>()
            .map(|mut c| std::mem::take(&mut c.0))
            .unwrap_or_default()
    }
}

/// A Kerbal on EVA as the hull test sees it: spheres this big round points along its body, m (our own
/// figures for a 1 m Kerbal standing on its centre).
const KERBAL_RADIUS_M: f64 = 0.25;
const KERBAL_POINTS_M: [f64; 2] = [-0.25, 0.3];

/// A part's hull as a Kerbal meets it, inertial offsets from its craft's position: the part's box, or for a
/// stack part the frustum between its stack nodes (a Mk1 pod's cone, a tank's cylinder).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Hull {
    Box {
        centre: DVec3,
        axes: [DVec3; 3],
        half: DVec3,
    },
    Frustum {
        origin: DVec3,
        axis: DVec3,
        lo: f64,
        hi: f64,
        taper: structure::Taper,
    },
}

impl Hull {
    /// Signed distance from `x` to the hull's surface, m (negative inside).
    pub(crate) fn distance(&self, x: DVec3) -> f64 {
        match *self {
            Hull::Box { centre, axes, half } => {
                let d = x - centre;
                let q = DVec3::new(
                    d.dot(axes[0]).abs() - half.x,
                    d.dot(axes[1]).abs() - half.y,
                    d.dot(axes[2]).abs() - half.z,
                );
                let outside = DVec3::new(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0)).length();
                outside + q.x.max(q.y).max(q.z).min(0.0)
            }
            Hull::Frustum {
                origin,
                axis,
                lo,
                hi,
                taper,
            } => {
                // in the plane through the axis: a trapezoid from the axis out to the hull
                let v = x - origin;
                let h = v.dot(axis);
                let d = (v - axis * h).length();
                let (rl, rh) = (taper.radius_at(lo), taper.radius_at(hi));
                let edge = |a: (f64, f64), b: (f64, f64)| {
                    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                    let t = (((d - a.0) * ex + (h - a.1) * ey) / (ex * ex + ey * ey).max(1e-12))
                        .clamp(0.0, 1.0);
                    ((d - a.0 - ex * t).powi(2) + (h - a.1 - ey * t).powi(2)).sqrt()
                };
                let gap = edge((0.0, lo), (rl, lo))
                    .min(edge((rl, lo), (rh, hi)))
                    .min(edge((rh, hi), (0.0, hi)));
                if (lo..=hi).contains(&h) && d <= taper.radius_at(h) {
                    -gap
                } else {
                    gap
                }
            }
        }
    }

    /// The way out of the hull at `x` (its surface normal there).
    pub(crate) fn normal(&self, x: DVec3) -> DVec3 {
        let e = 1e-4;
        let g = |a: DVec3| self.distance(x + a * e) - self.distance(x - a * e);
        let n = DVec3::new(
            g(DVec3::new(1.0, 0.0, 0.0)),
            g(DVec3::new(0.0, 1.0, 0.0)),
            g(DVec3::new(0.0, 0.0, 1.0)),
        );
        if n.length() > 1e-12 {
            n.normalized()
        } else {
            DVec3::ZERO
        }
    }
}

/// A craft's parts as hulls (inertial offsets from its position).
pub(crate) fn hulls(st: &structure::Structure, facing: DVec3) -> Vec<Hull> {
    let c = st.rigid.rest_centre();
    let (bx, by, bz) = st.frame(facing);
    let to = |v: DVec3| bx * v.x + by * v.y + bz * v.z;
    (0..st.rigid.len())
        .filter(|&i| st.rigid.present[i])
        .map(|i| {
            let (centre, half) = st.part_box(i);
            match st.hull_taper(i) {
                Some(taper) => {
                    let origin = st.rigid.rest_position(i);
                    let a = taper.axis;
                    let mid = (centre - origin).dot(a);
                    let reach = half.x * a.x.abs() + half.y * a.y.abs() + half.z * a.z.abs();
                    Hull::Frustum {
                        origin: to(origin - c),
                        axis: to(a),
                        lo: mid - reach,
                        hi: mid + reach,
                        taper,
                    }
                }
                None => Hull::Box {
                    centre: to(centre - c),
                    axes: [bx, by, bz],
                    half,
                },
            }
        })
        .collect()
}

/// The steepest hull a Kerbal stands on (the up share of its surface normal): KSP2 trace run 14 lets go of
/// the Mk1 pod's ladder and stands on its cone, which leans some 70 degrees.
const DECK_MIN_UP: f64 = 0.2;
/// How far up a standing Kerbal steps onto a hull, and how far down it follows one before it falls, m (our
/// own figures).
pub const DECK_STEP_UP_M: f64 = 0.3;

/// The highest radius from the body centre the Kerbal's centre can come down to between `hi` and `lo` before
/// its feet (a sphere at the bottom of its body) rest on one of a craft's hulls facing up enough to stand on.
fn deck_on(hulls: &[Hull], k: &flight::Powered, craft_r: DVec3, hi: f64, lo: f64) -> Option<f64> {
    let up = k.r.normalized();
    let drop = k.ground_clearance_m - KERBAL_RADIUS_M;
    let foot = |z: f64| up * (z - drop) - craft_r;
    let mut best: Option<f64> = None;
    for h in hulls {
        let gap = |z: f64| h.distance(foot(z)) - KERBAL_RADIUS_M;
        // already in it at the top of the window: a wall, which `hull_push` keeps it out of
        if gap(hi) <= 0.0 {
            continue;
        }
        let mut z = hi;
        let mut bracket = None;
        while z > lo {
            let next = (z - 0.05).max(lo);
            if gap(next) <= 0.0 {
                bracket = Some((next, z));
                break;
            }
            z = next;
        }
        let Some((mut a, mut b)) = bracket else {
            continue;
        };
        for _ in 0..30 {
            let m = 0.5 * (a + b);
            if gap(m) <= 0.0 {
                a = m;
            } else {
                b = m;
            }
        }
        if h.normal(foot(b)).dot(up) < DECK_MIN_UP {
            continue;
        }
        best = Some(best.map_or(b, |x: f64| x.max(b)));
    }
    best
}

impl Universe {
    /// What each Kerbal on EVA stands on besides the ground: the top of a nearby craft's hull under its feet,
    /// within a step up of where it is and the fall it can make this step. It lands on that, stands on it and
    /// rides along (KSP2 `KerbalBehavior.SetContactObjectRelativeVelocity`), and falls once it walks off.
    pub(crate) fn kerbals_find_decks(&mut self, dt: f64) {
        let walkers: Vec<(Entity, Entity)> = {
            let mut q = self.world.query::<(
                Entity,
                &flight::Powered,
                &bodies::ReferenceBody,
                Has<crate::crew::OnLadder>,
                Has<crate::crew::Boarding>,
            )>();
            q.iter(&self.world)
                .filter(|(_, p, _, ladder, boarding)| p.walker && !ladder && !boarding)
                .map(|(e, _, b, _, _)| (e, b.0))
                .collect()
        };
        if walkers.is_empty() {
            return;
        }
        let crafts: Vec<(Entity, Entity)> = {
            let mut q = self.world.query_filtered::<(
                Entity,
                &flight::Powered,
                &bodies::ReferenceBody,
            ), With<structure::Structure>>();
            q.iter(&self.world)
                .filter(|(_, p, _)| !p.walker && !p.crashed)
                .map(|(e, _, b)| (e, b.0))
                .collect()
        };
        for (k, body) in walkers {
            let Some(pk) = self.world.get::<flight::Powered>(k) else {
                continue;
            };
            let deck = if pk.crashed {
                None
            } else {
                let r = pk.r.length();
                let (hi, lo) = if pk.landed {
                    (r + DECK_STEP_UP_M, r - flight::WALK_OFF_M)
                } else {
                    (r + 0.05, r - (pk.v.length() * dt + 0.5))
                };
                let mut best: Option<flight::Deck> = None;
                for &(c, craft_body) in &crafts {
                    if craft_body != body || c == k {
                        continue;
                    }
                    let (Some(pc), Some(st)) = (
                        self.world.get::<flight::Powered>(c),
                        self.world.get::<structure::Structure>(c),
                    ) else {
                        continue;
                    };
                    let reach = part_spheres(st, pc.facing)
                        .iter()
                        .map(|(_, o, r)| o.length() + r)
                        .fold(0.0, f64::max);
                    if (pk.r - pc.r).length() > reach + 2.0 {
                        continue;
                    }
                    let Some(z) = deck_on(&hulls(st, pc.facing), pk, pc.r, hi, lo) else {
                        continue;
                    };
                    if best.is_none_or(|d| z > d.radius_m) {
                        let spin = pc.angvel + pc.facing * pc.roll_rate;
                        best = Some(flight::Deck {
                            radius_m: z,
                            v: pc.v + spin.cross(pk.r - pc.r),
                        });
                    }
                }
                best
            };
            if let Some(mut p) = self.world.get_mut::<flight::Powered>(k) {
                p.deck = deck;
            }
        }
    }
}

impl Universe {
    /// A Kerbal on EVA does not pass through a craft: it is pushed out of the parts' boxes, sideways (a hull
    /// is a wall to it, not a floor: it does not come to rest on a fin), and stops going into them. Kerbals
    /// on a ladder are left to the ladder.
    pub(crate) fn kerbals_meet_hulls(&mut self) {
        let ut = self.ut();
        let (kerbals, crafts): (Vec<_>, Vec<_>) = {
            let mut q = self.world.query::<(
                Entity,
                &flight::Powered,
                &bodies::ReferenceBody,
                Has<crate::crew::OnLadder>,
                Has<structure::Structure>,
            )>();
            q.iter(&self.world)
                .filter(|(_, p, _, ladder, _)| !p.crashed && !ladder)
                .map(|(e, p, b, _, st)| (e, b.0, p.walker, st))
                .partition(|&(_, _, walker, _)| walker)
        };
        for &(k, body, _, _) in &kerbals {
            for &(c, craft_body, _, has_parts) in &crafts {
                if craft_body != body || !has_parts {
                    continue;
                }
                let grace = |x: Entity, y: Entity| {
                    self.world
                        .get::<SeparatedFrom>(x)
                        .is_some_and(|s| s.other == y && s.until_ut > ut)
                };
                if grace(k, c) || grace(c, k) {
                    continue;
                }
                let (Some(pk), Some(pc), Some(st)) = (
                    self.world.get::<flight::Powered>(k),
                    self.world.get::<flight::Powered>(c),
                    self.world.get::<structure::Structure>(c),
                ) else {
                    continue;
                };
                let Some(push) = hull_push(pk, pc, st) else {
                    continue;
                };
                let craft_v = pc.v;
                let mut pk = pk.clone();
                let n = push.normalized();
                pk.r = pk.r + push;
                let into = (pk.v - craft_v).dot(n);
                if into < 0.0 {
                    pk.v = pk.v - n * into;
                }
                self.world.entity_mut(k).insert(pk);
            }
        }
    }
}

/// How far to move a Kerbal `pk` (inertial) to clear the parts of craft `pc`, or None when it is clear.
fn hull_push(
    pk: &flight::Powered,
    pc: &flight::Powered,
    st: &structure::Structure,
) -> Option<DVec3> {
    let (bx, by, bz) = st.frame(pc.facing);
    let c = st.rigid.rest_centre();
    let up = pk.r.normalized();
    let mut total = DVec3::ZERO;
    let shapes = hulls(st, pc.facing);
    for along in KERBAL_POINTS_M {
        let d = pk.r + up * along + total - pc.r;
        // the point in craft coordinates
        let local = DVec3::new(d.dot(bx), d.dot(by), d.dot(bz)) + c;
        let mut deepest: Option<DVec3> = None;
        for (n, i) in (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i])
            .enumerate()
        {
            if let Hull::Frustum { .. } = shapes[n] {
                // a stack part's own shape: pushed out of it along its surface, sideways
                let gap = shapes[n].distance(d);
                if gap >= KERBAL_RADIUS_M {
                    continue;
                }
                let world = shapes[n].normal(d) * (KERBAL_RADIUS_M - gap);
                let world = world - up * world.dot(up);
                if world.length() < 1e-9 {
                    continue;
                }
                let push = DVec3::new(world.dot(bx), world.dot(by), world.dot(bz));
                if deepest.is_none_or(|x| push.length() > x.length()) {
                    deepest = Some(push);
                }
                continue;
            }
            let (centre, half) = st.part_box(i);
            let rel = local - centre;
            let q = DVec3::new(
                rel.x.clamp(-half.x, half.x),
                rel.y.clamp(-half.y, half.y),
                rel.z.clamp(-half.z, half.z),
            );
            let out = rel - q;
            let gap = out.length();
            let push = if gap > 1e-9 {
                if gap >= KERBAL_RADIUS_M {
                    continue;
                }
                out * ((KERBAL_RADIUS_M - gap) / gap)
            } else {
                // inside the box: out through the nearest side face
                let faces = [
                    (half.x - rel.x.abs(), DVec3::new(rel.x.signum(), 0.0, 0.0)),
                    (half.y - rel.y.abs(), DVec3::new(0.0, rel.y.signum(), 0.0)),
                    (half.z - rel.z.abs(), DVec3::new(0.0, 0.0, rel.z.signum())),
                ];
                let Some((depth, dir)) = faces
                    .into_iter()
                    .filter(|(_, d)| (bx * d.x + by * d.y + bz * d.z).dot(up).abs() < 0.7)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                else {
                    continue;
                };
                dir * (depth + KERBAL_RADIUS_M)
            };
            // sideways only
            let world = bx * push.x + by * push.y + bz * push.z;
            let world = world - up * world.dot(up);
            if world.length() < 1e-9 {
                continue;
            }
            let push = DVec3::new(world.dot(bx), world.dot(by), world.dot(bz));
            if deepest.is_none_or(|x| push.length() > x.length()) {
                deepest = Some(push);
            }
        }
        if let Some(p) = deepest {
            total = total + bx * p.x + by * p.y + bz * p.z;
        }
    }
    (total.length() > 1e-9).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICKS_PER_SECOND;

    fn two_k1s(closing_mps: f64) -> (Universe, Entity, Entity) {
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
        let body = u.body_entity("Kerbin").unwrap();
        let r0 = DVec3::new(700_000.0, 0.0, 0.0);
        let v0 = DVec3::new(0.0, 2_246.0, 0.0);
        let mut out = Vec::new();
        // side by side across the orbit (x), noses along it, closing along x
        for (k, side) in [(0, 1.0), (1, -1.0)] {
            let v = u.spawn_vessel_in_circular_orbit(&format!("K1 {k}"), body, 100_000.0);
            let mut p = flight::Powered::new(
                spec.clone(),
                r0 + DVec3::new(side * 8.0, 0.0, 0.0),
                v0 + DVec3::new(-side * closing_mps * 0.5, 0.0, 0.0),
                false,
            );
            p.facing = DVec3::new(0.0, 1.0, 0.0);
            p.attitude = flight::Attitude::Free;
            p.throttle = 0.0;
            u.set_powered(v, p);
            u.set_craft(v, k1.clone());
            u.set_structure(v, structure::Structure::build(k1, &defs, &res));
            out.push(v);
        }
        (u, out[0], out[1])
    }

    #[test]
    fn two_vessels_bump_and_part_keeping_their_momentum() {
        let (mut u, a, b) = two_k1s(2.0);
        let m = |u: &Universe, e| u.powered(e).unwrap().mass_t;
        let mom =
            |u: &Universe| u.powered(a).unwrap().v * m(u, a) + u.powered(b).unwrap().v * m(u, b);
        // the contact itself conserves momentum exactly; the run around it adds gravity only
        let separating = |u: &Universe| {
            let pa = u.powered(a).unwrap();
            let pb = u.powered(b).unwrap();
            (pa.v - pb.v).dot((pa.r - pb.r).normalized())
        };
        let gap = |u: &Universe| (u.powered(a).unwrap().r - u.powered(b).unwrap().r).length();
        let mut touched = false;
        let mut step_change = DVec3::ZERO;
        let mut sep = f64::NAN;
        let mut closest = f64::MAX;
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
            closest = closest.min(gap(&u));
            if !touched {
                let before = mom(&u);
                u.collide_vessels();
                if !u.take_contacts().is_empty() {
                    touched = true;
                    step_change = mom(&u) - before;
                    sep = separating(&u);
                }
            }
        }
        assert!(touched, "they met");
        // the blow turns them apart across the orbit (an off-centre blow also sets them turning, so they
        // may lean on each other afterwards, but never pass through)
        assert!(sep > 0.0, "moving apart at {sep} m/s");
        assert!(sep < 2.0, "and slower than they came: {sep}");
        assert!(
            closest > 4.0,
            "the two K1s came within {closest} m of each other's centre"
        );
        let (pa, pb) = (u.powered(a).unwrap(), u.powered(b).unwrap());
        let g_step = 8.2 * (m(&u, a) + m(&u, b)) / TICKS_PER_SECOND as f64;
        assert!(
            step_change.length() < 1.5 * g_step,
            "{step_change:?} vs gravity {g_step}"
        );
        assert!(!pa.crashed && !pb.crashed, "a bump breaks nothing");
    }

    #[test]
    fn light_parts_cannot_break_heavy_ones_as_in_ksp2() {
        // the Mk1 pod (15 m/s): struck by a 1 t part at 20 m/s it breaks; by a 0.04 t decoupler at 20 m/s
        // (0.8 t*m/s) it does not, and the decoupler would need 375 m/s
        assert!(crash_breaks(20.0, 1.0, 15.0));
        assert!(!crash_breaks(20.0, 0.04, 15.0));
        assert!(!crash_breaks(370.0, 0.04, 15.0));
        assert!(crash_breaks(380.0, 0.04, 15.0));
        // never below the tolerance itself, however heavy the other part
        assert!(!crash_breaks(14.0, 100.0, 15.0));
    }

    #[test]
    fn a_fast_collision_breaks_parts_off() {
        let (mut u, a, b) = two_k1s(80.0);
        let parts = |u: &Universe, e| {
            u.structure(e)
                .map(|s| s.rigid.present.iter().filter(|x| **x).count())
                .unwrap_or(0)
        };
        let (na, nb) = (parts(&u, a), parts(&u, b));
        for _ in 0..(2 * TICKS_PER_SECOND) {
            u.step();
        }
        let lost = (na - parts(&u, a)) + (nb - parts(&u, b));
        let wrecked = u.powered(a).unwrap().crashed || u.powered(b).unwrap().crashed;
        assert!(lost > 0 || wrecked, "an 80 m/s blow breaks something");
    }
}

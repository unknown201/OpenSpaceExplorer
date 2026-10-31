//! Vessels and debris striking each other: each part a sphere round its box, contacts between the parts of
//! two nearby vessels resolved with equal and opposite impulses (restitution and friction), and the parts
//! that take a blow harder than their crash tolerance break off. Docking ports facing each other are left
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
                    // what the blow breaks
                    for (e, part) in [(a, ia), (b, ib)] {
                        let tolerance = self
                            .world
                            .get::<structure::Structure>(e)
                            .and_then(|st| st.crash_tolerance.get(part).copied())
                            .filter(|t| *t > 0.0)
                            .unwrap_or(flight::CRASH_SPEED_MPS);
                        if -vn > tolerance {
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
        let mut touched = false;
        let mut step_change = DVec3::ZERO;
        for _ in 0..(10 * TICKS_PER_SECOND) {
            u.step();
            if !touched {
                let before = mom(&u);
                u.collide_vessels();
                if !u.take_contacts().is_empty() {
                    touched = true;
                    step_change = mom(&u) - before;
                }
            }
        }
        assert!(touched, "they met");
        let pa = u.powered(a).unwrap();
        let pb = u.powered(b).unwrap();
        // now moving apart across the orbit
        let sep = (pa.v - pb.v).dot((pa.r - pb.r).normalized());
        assert!(sep > 0.0, "moving apart at {sep} m/s");
        assert!(sep < 2.0, "and slower than they came: {sep}");
        let g_step = 8.2 * (m(&u, a) + m(&u, b)) / TICKS_PER_SECOND as f64;
        assert!(
            step_change.length() < 1.5 * g_step,
            "{step_change:?} vs gravity {g_step}"
        );
        assert!(!pa.crashed && !pb.crashed, "a bump breaks nothing");
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

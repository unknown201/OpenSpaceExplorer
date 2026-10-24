//! Per-part rigid bodies linked by the original's part joints (see `docs/reference-notes/physics-joints-drag.md`).
//!
//! One rapier body per part, one joint per parent link. Units follow the original's PhysX setup: tonnes, metres,
//! seconds, so forces are kN and torques kN*m. Joints are fixed, reduced-coordinate (multibody) links: a craft
//! is a tree, and a multibody stays exact across the 100:1 mass ratios of radial decouplers between tanks and
//! boosters, which an iterative solver cannot hold. Each joint's load comes from Newton-Euler over the subtree
//! it carries.
//!
//! Wobble is a layer on top: every non-rigid joint bends by a rotation `theta` driven by the torque the rigid
//! solution says it carries, with the original's angular spring (`JointLimits::stiffness`, damping
//! `stiffness * JointDampingFactor`) and the subtree's moment of inertia about the anchor:
//! `I theta'' = -tau - k theta - c theta'`, integrated semi-implicitly in substeps so it stays stable at any
//! stiffness. Statically a joint bends by `tau / k`, so a tall stack under thrust or a booster on a radial
//! decoupler visibly flexes, and a sudden load rings. A joint breaks when its force, or its spring torque
//! `k |theta|` (the load itself for rigid joints), exceeds the original's limits; the craft then splits into
//! the connected groups that remain. (Rapier's own multibody motors for three free rotations track integrated
//! angular velocity rather than the true rotation, and wander, so the spring is ours.)
//!
//! The vessel frame is the craft frame made right-handed: Unity Y-up with Z negated, so engines push along +Y.

use std::collections::HashMap;

use ose_data::craft::{Craft, Quat};
use ose_data::{PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use rapier3d_f64::math::{Pose, Rotation, Vector};
use rapier3d_f64::prelude::{
    CCDSolver, ColliderSet, DefaultBroadPhase, GenericJointBuilder, ImpulseJointSet,
    IntegrationParameters, IslandManager, JointAxesMask, MassProperties, MultibodyJointHandle,
    MultibodyJointSet, NarrowPhase, PhysicsPipeline, RigidBodyBuilder, RigidBodyHandle,
    RigidBodySet, SoftBodySet,
};

/// `PhysicsSettings` defaults.
pub const JOINT_RIGIDITY: f64 = 1500.0;
pub const JOINT_STACK_NODE_FACTOR: f64 = 50.0;
pub const JOINT_SURFACE_NODE_FACTOR: f64 = 0.8;
pub const JOINT_ATTACH_STIFFNESS: f64 = 10.0;
pub const JOINT_DAMPING_FACTOR: f64 = 0.01;
pub const JOINT_BREAK_FORCE_FACTOR: f64 = 50.0;
pub const JOINT_BREAK_TORQUE_FACTOR: f64 = 50.0;
/// Spring substeps per step for the wobble layer.
const WOBBLE_SUBSTEPS: usize = 8;

/// Spring and break limits of one part joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointLimits {
    /// Angular spring, kN*m/rad.
    pub stiffness: f64,
    pub damping: f64,
    /// kN; infinite for unbreakable joints.
    pub break_force_kn: f64,
    /// kN*m.
    pub break_torque_knm: f64,
    pub rigid: bool,
}

/// The original's `CreateJoint` / `GetJointBreakLimits` for a single (non multi-) joint.
pub fn joint_limits(
    node_size: i32,
    stack: bool,
    rigid: bool,
    breaking_force: (f64, f64),
    breaking_torque: (f64, f64),
) -> JointLimits {
    let size = node_size.max(0) as f64;
    let node_factor = (size + 1.0)
        * if stack {
            JOINT_STACK_NODE_FACTOR
        } else {
            JOINT_SURFACE_NODE_FACTOR
        };
    let stiffness = JOINT_RIGIDITY * node_factor * JOINT_ATTACH_STIFFNESS;
    let size_div = if node_size < 2 { 1.0 } else { 3.0 };
    // a part authored without a breaking force is not breakable
    let limit = |a: f64, b: f64, factor: f64| {
        let m = a.min(b);
        if m <= 0.0 {
            f64::INFINITY
        } else {
            m * node_factor / size_div * factor
        }
    };
    JointLimits {
        stiffness,
        damping: stiffness * JOINT_DAMPING_FACTOR,
        break_force_kn: limit(breaking_force.0, breaking_force.1, JOINT_BREAK_FORCE_FACTOR),
        break_torque_knm: limit(
            breaking_torque.0,
            breaking_torque.1,
            JOINT_BREAK_TORQUE_FACTOR,
        ),
        rigid,
    }
}

/// Unity Y-up left-handed to the vessel frame (Z negated).
pub fn to_vessel(v: DVec3) -> DVec3 {
    DVec3::new(v.x, v.y, -v.z)
}

fn quat_to_vessel(q: Quat) -> Rotation {
    // mirroring Z flips the sense of rotation about X and Y
    let r = Rotation::from_xyzw(-q.x, -q.y, q.z, q.w);
    if r.length_squared() < 1e-12 {
        Rotation::IDENTITY
    } else if r.w < 0.0 {
        // one sign for every part, so joint frames never start a full turn apart
        -r.normalize()
    } else {
        r.normalize()
    }
}

fn v(d: DVec3) -> Vector {
    Vector::new(d.x, d.y, d.z)
}

fn d(v: Vector) -> DVec3 {
    DVec3::new(v.x, v.y, v.z)
}

/// One parent link.
#[derive(Clone, Debug)]
pub struct Link {
    pub child: usize,
    pub parent: usize,
    pub limits: JointLimits,
    /// Last step's load, for readouts and break checks.
    pub force_kn: f64,
    pub torque_knm: f64,
    /// Torque the joint carries (vessel frame), kN*m.
    pub torque_vec_knm: DVec3,
    /// Bend of the child away from the parent (rotation vector, vessel frame, about the anchor), rad, and
    /// its rate.
    pub bend: DVec3,
    pub bend_rate: DVec3,
    handle: Option<MultibodyJointHandle>,
    /// The joint anchor in the child's frame, and in the vessel frame at rest.
    anchor_child: Vector,
    anchor_rest: DVec3,
}

/// Turning loads the rigid bodies do not feel themselves: the vessel's angular acceleration and rate (vessel
/// frame) about its centre of mass, and the control torques (at parts) and forces that drive it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turning {
    pub alpha: DVec3,
    pub omega: DVec3,
    pub torques: Vec<(usize, DVec3)>,
    pub forces: Vec<PartForce>,
}

/// Unit quaternion (x, y, z, w) helpers for the bend chain.
fn q_from_rotvec(r: DVec3) -> [f64; 4] {
    let a = r.length();
    if a < 1e-12 {
        return [r.x * 0.5, r.y * 0.5, r.z * 0.5, 1.0];
    }
    let (s, c) = (a * 0.5).sin_cos();
    let u = r * (s / a);
    [u.x, u.y, u.z, c]
}

fn q_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn q_conj(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

fn q_rotate(q: [f64; 4], v: DVec3) -> DVec3 {
    let u = DVec3::new(q[0], q[1], q[2]);
    let t = u.cross(v) * 2.0;
    v + t * q[3] + u.cross(t)
}

fn q_to_rotvec(q: [f64; 4]) -> DVec3 {
    let (q, sign) = if q[3] < 0.0 { (q, -1.0) } else { (q, 1.0) };
    let u = DVec3::new(q[0], q[1], q[2]) * sign;
    let s = u.length();
    if s < 1e-12 {
        return u * 2.0;
    }
    let angle = 2.0 * s.atan2(q[3] * sign);
    u * (angle / s)
}

/// How a part sits away from its rest pose because of the bends between it and the root.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartFlex {
    pub part: usize,
    /// Rotation vector (vessel frame), rad.
    pub rotation: DVec3,
    /// Displacement of the part's origin (vessel frame), m.
    pub offset_m: DVec3,
}

impl Link {
    pub fn intact(&self) -> bool {
        self.handle.is_some()
    }
}

/// A joint that broke this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Broken {
    pub child: usize,
    pub parent: usize,
    pub force_kn: f64,
    pub torque_knm: f64,
}

/// A force on a part, in the vessel frame, applied at the part's position plus `offset_m`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartForce {
    pub part: usize,
    pub force_kn: DVec3,
    pub offset_m: DVec3,
}

pub struct RigidVessel {
    /// Wet mass at build (dry plus stored resources), t.
    pub masses_t: Vec<f64>,
    pub dry_masses_t: Vec<f64>,
    /// Box size of each part (vessel frame axes), m, as used for its inertia.
    pub sizes_m: Vec<DVec3>,
    /// Parts still simulated (removed ones were staged away or broke off).
    pub present: Vec<bool>,
    pub links: Vec<Link>,
    /// Part origins at rest, vessel frame.
    rest_pos: Vec<DVec3>,
    /// The root body's rotation at build: the vessel frame is the root's rotation since then.
    root_rot0: [f64; 4],
    root: usize,
    bodies_of: Vec<RigidBodyHandle>,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse: ImpulseJointSet,
    joints: MultibodyJointSet,
    prev_vel: Vec<DVec3>,
    soft: SoftBodySet,
    islands: IslandManager,
    broad: DefaultBroadPhase,
    narrow: NarrowPhase,
    ccd: CCDSolver,
    pipeline: PhysicsPipeline,
    params: IntegrationParameters,
}

fn part_mass_t(
    craft: &Craft,
    i: usize,
    def: Option<&PartDef>,
    unit_mass: &HashMap<&str, f64>,
) -> f64 {
    let dry = def.map(|d| d.mass_t).unwrap_or(0.0);
    let wet: f64 = craft.parts[i]
        .resources
        .iter()
        .map(|r| r.stored_units * unit_mass.get(r.resource.as_str()).copied().unwrap_or(0.0))
        .sum();
    // a body needs some mass; the lightest stock parts are a few kg
    (dry + wet).max(0.001)
}

impl RigidVessel {
    /// Build from a craft, every part at its authored placement, at rest.
    pub fn build(craft: &Craft, defs: &[PartDef], resources: &[ResourceDef]) -> RigidVessel {
        let by_id: HashMap<&str, &PartDef> = defs.iter().map(|p| (p.id.as_str(), p)).collect();
        let unit_mass: HashMap<&str, f64> = resources
            .iter()
            .filter_map(|r| match &r.kind {
                ResourceKind::Basic {
                    mass_per_unit_t, ..
                } => Some((r.id.as_str(), *mass_per_unit_t)),
                ResourceKind::Recipe { .. } => None,
            })
            .collect();
        let mut bodies = RigidBodySet::new();
        let mut joints = MultibodyJointSet::new();
        let mut masses_t = Vec::with_capacity(craft.parts.len());
        let mut dry_masses_t = Vec::with_capacity(craft.parts.len());
        let mut sizes_m = Vec::with_capacity(craft.parts.len());
        let mut bodies_of = Vec::with_capacity(craft.parts.len());
        let mut rots = Vec::with_capacity(craft.parts.len());
        for (i, p) in craft.parts.iter().enumerate() {
            let def = by_id.get(p.part_id.as_str()).copied();
            let m = part_mass_t(craft, i, def, &unit_mass);
            // solid box over the part's bounds; deployed shapes (parachutes) are capped
            let size = def
                .and_then(|d| d.bounds)
                .map(|b| b.size_m)
                .unwrap_or(DVec3::new(1.0, 1.0, 1.0));
            let (sx, sy, sz) = (
                size.x.clamp(0.1, 5.0),
                size.y.clamp(0.1, 5.0),
                size.z.clamp(0.1, 5.0),
            );
            let inertia = Vector::new(
                m * (sy * sy + sz * sz) / 12.0,
                m * (sx * sx + sz * sz) / 12.0,
                m * (sx * sx + sy * sy) / 12.0,
            );
            let rot = quat_to_vessel(p.local_rotation);
            let rb = RigidBodyBuilder::dynamic()
                .pose(Pose::from_parts(v(to_vessel(p.local_position)), rot))
                .additional_mass_properties(MassProperties::new(Vector::ZERO, m, inertia))
                .can_sleep(false)
                .build();
            bodies_of.push(bodies.insert(rb));
            masses_t.push(m);
            dry_masses_t.push(def.map(|d| d.mass_t).unwrap_or(0.0).min(m));
            sizes_m.push(DVec3::new(sx, sy, sz));
            rots.push(rot);
        }
        let mut links = Vec::new();
        for (i, p) in craft.parts.iter().enumerate() {
            let Some(parent) = p.parent else { continue };
            let def = by_id.get(p.part_id.as_str()).copied();
            let pdef = by_id.get(craft.parts[parent].part_id.as_str()).copied();
            let own = def.and_then(|d| {
                p.own_node
                    .as_deref()
                    .and_then(|n| d.attach_nodes.iter().find(|a| a.id == n))
            });
            let other = pdef.and_then(|d| {
                p.parent_node
                    .as_deref()
                    .and_then(|n| d.attach_nodes.iter().find(|a| a.id == n))
            });
            let node = own.or(other);
            let stack = node.is_none_or(|n| n.node_type.eq_ignore_ascii_case("stack"));
            let size = node.map(|n| n.size).unwrap_or(1);
            let rigid = own.is_some_and(|n| n.is_rigid) || other.is_some_and(|n| n.is_rigid);
            let bf = |d: Option<&PartDef>| d.map(|d| d.breaking_force).unwrap_or(0.0);
            let bt = |d: Option<&PartDef>| d.map(|d| d.breaking_torque).unwrap_or(0.0);
            let limits = joint_limits(size, stack, rigid, (bf(def), bf(pdef)), (bt(def), bt(pdef)));
            // anchor at the child's attach node, else midway between the parts
            let cpos = to_vessel(p.local_position);
            let ppos = to_vessel(craft.parts[parent].local_position);
            let anchor = match own {
                Some(n) => cpos + d(rots[i] * v(to_vessel(n.position))),
                None => (cpos + ppos) * 0.5,
            };
            let a1 = rots[parent].inverse() * v(anchor - ppos);
            let a2 = rots[i].inverse() * v(anchor - cpos);
            let b = GenericJointBuilder::new(JointAxesMask::LOCKED_FIXED_AXES)
                .local_frame1(Pose::from_parts(a1, rots[parent].inverse()))
                .local_frame2(Pose::from_parts(a2, rots[i].inverse()))
                .contacts_enabled(false);
            let h = joints.insert(bodies_of[parent], bodies_of[i], b.build(), true);
            links.push(Link {
                child: i,
                parent,
                limits,
                force_kn: 0.0,
                torque_knm: 0.0,
                torque_vec_knm: DVec3::ZERO,
                bend: DVec3::ZERO,
                bend_rate: DVec3::ZERO,
                handle: h,
                anchor_child: a2,
                anchor_rest: anchor,
            });
        }
        let params = IntegrationParameters::default();
        let n = masses_t.len();
        let rest_pos = craft
            .parts
            .iter()
            .map(|p| to_vessel(p.local_position))
            .collect();
        RigidVessel {
            masses_t,
            dry_masses_t,
            sizes_m,
            present: vec![true; n],
            links,
            rest_pos,
            root_rot0: {
                let r = craft
                    .parts
                    .iter()
                    .position(|p| p.parent.is_none())
                    .unwrap_or(0);
                let q = rots.get(r).copied().unwrap_or(Rotation::IDENTITY);
                [q.x, q.y, q.z, q.w]
            },
            root: craft
                .parts
                .iter()
                .position(|p| p.parent.is_none())
                .unwrap_or(0),
            bodies_of,
            bodies,
            colliders: ColliderSet::new(),
            impulse: ImpulseJointSet::new(),
            joints,
            prev_vel: vec![DVec3::ZERO; n],
            soft: SoftBodySet::new(),
            islands: IslandManager::new(),
            broad: DefaultBroadPhase::new(),
            narrow: NarrowPhase::new(),
            ccd: CCDSolver::new(),
            pipeline: PhysicsPipeline::new(),
            params,
        }
    }

    /// How far the structure has turned since it was built (rotation from the vessel frame to the solver's
    /// world): the root's rotation now against its rotation then.
    pub fn frame(&self) -> [f64; 4] {
        match self.bodies.get(self.bodies_of[self.root]) {
            Some(b) if self.present[self.root] => {
                let r = b.rotation();
                q_mul([r.x, r.y, r.z, r.w], q_conj(self.root_rot0))
            }
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    pub fn len(&self) -> usize {
        self.bodies_of.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies_of.is_empty()
    }

    /// Zero for a removed part.
    pub fn position(&self, part: usize) -> DVec3 {
        self.bodies
            .get(self.bodies_of[part])
            .map(|b| d(b.translation()))
            .unwrap_or(DVec3::ZERO)
    }

    pub fn velocity(&self, part: usize) -> DVec3 {
        self.bodies
            .get(self.bodies_of[part])
            .map(|b| d(b.linvel()))
            .unwrap_or(DVec3::ZERO)
    }

    /// Rotation of a part as a quaternion (x, y, z, w) in the vessel frame.
    pub fn rotation(&self, part: usize) -> [f64; 4] {
        match self.bodies.get(self.bodies_of[part]) {
            Some(b) => {
                let r = b.rotation();
                [r.x, r.y, r.z, r.w]
            }
            None => [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Mass-weighted centre and velocity of a set of parts.
    pub fn centre_of(&self, parts: &[usize]) -> (DVec3, DVec3) {
        let m: f64 = parts.iter().map(|&i| self.masses_t[i]).sum();
        let (mut c, mut vel) = (DVec3::ZERO, DVec3::ZERO);
        for &i in parts {
            c = c + self.position(i) * self.masses_t[i];
            vel = vel + self.velocity(i) * self.masses_t[i];
        }
        (c * (1.0 / m), vel * (1.0 / m))
    }

    /// Advance by `dt` under a uniform gravitational acceleration and the given part forces; returns the
    /// joints that broke (already removed). Forces and `turning` are in the vessel frame, which turns with
    /// the structure (it may drift slowly under unbalanced loads); `turning` adds the loads of the vessel
    /// being turned.
    pub fn step(&mut self, dt: f64, gravity: DVec3, forces: &[PartForce]) -> Vec<Broken> {
        self.step_turning(dt, gravity, forces, None)
    }

    pub fn step_turning(
        &mut self,
        dt: f64,
        gravity: DVec3,
        forces: &[PartForce],
        turning: Option<&Turning>,
    ) -> Vec<Broken> {
        let n = self.len();
        for &h in &self.bodies_of {
            if let Some(b) = self.bodies.get_mut(h) {
                b.reset_forces(false);
            }
        }
        // external force and its moment about the origin, per part, for the joint loads below
        let frame = self.frame();
        let to_world = |x: DVec3| q_rotate(frame, x);
        let gravity = to_world(gravity);
        let mut ext = vec![(DVec3::ZERO, DVec3::ZERO); n];
        for f in forces {
            let f = PartForce {
                part: f.part,
                force_kn: to_world(f.force_kn),
                offset_m: to_world(f.offset_m),
            };
            let Some(&h) = self.bodies_of.get(f.part) else {
                continue;
            };
            if !self.present[f.part] {
                continue;
            }
            let rb = &mut self.bodies[h];
            let at = rb.translation() + v(f.offset_m);
            rb.add_force_at_point(v(f.force_kn), at, true);
            ext[f.part].0 = ext[f.part].0 + f.force_kn;
            ext[f.part].1 = ext[f.part].1 + d(at).cross(f.force_kn);
        }
        let present: Vec<usize> = (0..n).filter(|&i| self.present[i]).collect();
        for i in (0..n).filter(|&i| self.present[i]) {
            self.prev_vel[i] = self.velocity(i);
        }
        self.params.dt = dt;
        self.pipeline.step(
            v(gravity),
            &self.params,
            &mut self.islands,
            &mut self.broad,
            &mut self.narrow,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse,
            &mut self.joints,
            &mut self.soft,
            &mut self.ccd,
            &(),
            &(),
        );
        // Joint loads by Newton-Euler over each child subtree: the joint supplies whatever the subtree's
        // motion needs beyond gravity and the external forces on it (rotational inertia neglected).
        let mut sum_f = vec![DVec3::ZERO; n];
        let mut sum_m = vec![DVec3::ZERO; n];
        // subtree mass moments for each joint's moment of inertia: sum m, sum m r, sum m |r|^2 + own boxes
        let mut sm = vec![0.0; n];
        let mut smr = vec![DVec3::ZERO; n];
        let mut smr2 = vec![0.0; n];
        let com = if present.is_empty() {
            DVec3::ZERO
        } else {
            self.centre_of(&present).0
        };
        for &i in &present {
            let a = (self.velocity(i) - self.prev_vel[i]) * (1.0 / dt);
            let m = self.masses_t[i];
            let r = self.position(i);
            let mut need = (a - gravity) * m;
            if let Some(t) = turning {
                // the part is carried round the centre of mass: tangential and centripetal acceleration
                let rel = r - com;
                let (alpha, omega) = (to_world(t.alpha), to_world(t.omega));
                need = need + (alpha.cross(rel) + omega.cross(omega.cross(rel))) * m;
            }
            sum_f[i] = need - ext[i].0;
            sum_m[i] = r.cross(need) - ext[i].1;
            let s = self.sizes_m[i];
            sm[i] = m;
            smr[i] = r * m;
            smr2[i] = m * r.length_squared() + m * (s.x * s.x + s.y * s.y + s.z * s.z) / 24.0;
        }
        if let Some(t) = turning {
            // the control that turns the vessel: wheel torques and gimbal forces
            for &(part, tq) in &t.torques {
                if self.present.get(part).copied().unwrap_or(false) {
                    sum_m[part] = sum_m[part] - to_world(tq);
                }
            }
            for f in &t.forces {
                if self.present.get(f.part).copied().unwrap_or(false) {
                    let at = self.position(f.part) + to_world(f.offset_m);
                    let fw = to_world(f.force_kn);
                    sum_f[f.part] = sum_f[f.part] - fw;
                    sum_m[f.part] = sum_m[f.part] - at.cross(fw);
                }
            }
        }
        for &c in self.subtree_order().iter().rev() {
            if let Some(l) = self.links.iter().find(|l| l.intact() && l.child == c) {
                let p = l.parent;
                sum_f[p] = sum_f[p] + sum_f[c];
                sum_m[p] = sum_m[p] + sum_m[c];
                sm[p] += sm[c];
                smr[p] = smr[p] + smr[c];
                smr2[p] += smr2[c];
            }
        }
        let mut broken = Vec::new();
        for l in &mut self.links {
            let Some(h) = l.handle else { continue };
            let body = &self.bodies[self.bodies_of[l.child]];
            let anchor = d(body.position().transform_point(l.anchor_child));
            l.force_kn = sum_f[l.child].length();
            let tau_world = sum_m[l.child] - anchor.cross(sum_f[l.child]);
            // bends live in the vessel frame
            let tau = q_rotate(q_conj(frame), tau_world);
            l.torque_vec_knm = tau;
            l.torque_knm = if l.limits.rigid {
                tau.length()
            } else {
                // the subtree's moment of inertia about the anchor, then the spring
                let c = l.child;
                let inertia = (smr2[c] - 2.0 * anchor.dot(smr[c])
                    + sm[c] * anchor.length_squared())
                .max(1e-6);
                let (k, damp) = (l.limits.stiffness, l.limits.damping);
                let h = dt / WOBBLE_SUBSTEPS as f64;
                for _ in 0..WOBBLE_SUBSTEPS {
                    let accel = (tau * -1.0 - l.bend * k) * (1.0 / inertia);
                    l.bend_rate = (l.bend_rate + accel * h)
                        * (1.0 / (1.0 + h * damp / inertia + h * h * k / inertia));
                    l.bend = l.bend + l.bend_rate * h;
                }
                k * l.bend.length()
            };
            if l.force_kn > l.limits.break_force_kn || l.torque_knm > l.limits.break_torque_knm {
                self.joints.remove(h, true);
                l.handle = None;
                broken.push(Broken {
                    child: l.child,
                    parent: l.parent,
                    force_kn: l.force_kn,
                    torque_knm: l.torque_knm,
                });
            }
        }
        broken
    }

    /// Let the joints ring down with no load (a coast: nothing pushes the structure).
    pub fn relax(&mut self, dt: f64) {
        let n = self.len();
        let present: Vec<usize> = (0..n).filter(|&i| self.present[i]).collect();
        if self
            .links
            .iter()
            .all(|l| l.bend.length() < 1e-7 && l.bend_rate.length() < 1e-6)
        {
            return;
        }
        let mut inertia = vec![0.0; n];
        for &i in &present {
            let s = self.sizes_m[i];
            inertia[i] = self.masses_t[i] * (s.x * s.x + s.y * s.y + s.z * s.z) / 24.0;
        }
        for l in self
            .links
            .iter_mut()
            .filter(|l| l.intact() && !l.limits.rigid)
        {
            // a rough subtree inertia is enough to ring down
            let i = (inertia[l.child] + self.masses_t[l.child]).max(1e-6);
            let (k, damp) = (l.limits.stiffness, l.limits.damping);
            let h = dt / WOBBLE_SUBSTEPS as f64;
            for _ in 0..WOBBLE_SUBSTEPS {
                let accel = (l.bend * -k) * (1.0 / i);
                l.bend_rate =
                    (l.bend_rate + accel * h) * (1.0 / (1.0 + h * damp / i + h * h * k / i));
                l.bend = l.bend + l.bend_rate * h;
            }
            l.torque_knm = k * l.bend.length();
            l.torque_vec_knm = DVec3::ZERO;
        }
    }

    /// Rest position of a part (vessel frame) and the attached parts' centre of mass at rest.
    pub fn rest_position(&self, part: usize) -> DVec3 {
        self.rest_pos[part]
    }

    pub fn rest_centre(&self) -> DVec3 {
        let (mut m, mut c) = (0.0, DVec3::ZERO);
        for i in (0..self.len()).filter(|&i| self.present[i]) {
            m += self.masses_t[i];
            c = c + self.rest_pos[i] * self.masses_t[i];
        }
        if m > 0.0 {
            c * (1.0 / m)
        } else {
            c
        }
    }

    /// Torque (vessel frame, kN*m) that gives the attached parts angular acceleration `alpha` about their
    /// centre of mass: the point masses' `sum m (|r|^2 alpha - (r . alpha) r)` plus their own boxes.
    pub fn torque_for(&self, alpha: DVec3) -> DVec3 {
        let c = self.rest_centre();
        let mut t = DVec3::ZERO;
        for i in (0..self.len()).filter(|&i| self.present[i]) {
            let m = self.masses_t[i];
            let r = self.rest_pos[i] - c;
            let s = self.sizes_m[i];
            let own = m * (s.x * s.x + s.y * s.y + s.z * s.z) / 18.0;
            t = t + (alpha * r.length_squared() - r * r.dot(alpha)) * m + alpha * own;
        }
        t
    }

    /// Worst joint load as a fraction of its break limit (force or torque), 0 when unloaded.
    pub fn max_stress(&self) -> f64 {
        self.links
            .iter()
            .filter(|l| l.intact())
            .map(|l| {
                let f = if l.limits.break_force_kn.is_finite() {
                    l.force_kn / l.limits.break_force_kn
                } else {
                    0.0
                };
                let t = if l.limits.break_torque_knm.is_finite() {
                    l.torque_knm / l.limits.break_torque_knm
                } else {
                    0.0
                };
                f.max(t)
            })
            .fold(0.0, f64::max)
    }

    /// Each attached part's pose away from rest from the bends between it and the root; parts that moved
    /// less than `min_rad` / `min_m` are left out.
    pub fn flex(&self, min_rad: f64, min_m: f64) -> Vec<PartFlex> {
        let n = self.len();
        let mut rot = vec![[0.0, 0.0, 0.0, 1.0]; n];
        let mut off = vec![DVec3::ZERO; n];
        let mut out = Vec::new();
        for i in self.subtree_order() {
            let Some(l) = self.links.iter().find(|l| l.intact() && l.child == i) else {
                continue;
            };
            let p = l.parent;
            // the anchor rides on the parent, then the child turns about it by its bend
            let anchor =
                l.anchor_rest + off[p] + q_rotate(rot[p], l.anchor_rest - self.rest_pos[p])
                    - (l.anchor_rest - self.rest_pos[p]);
            let q = q_mul(q_from_rotvec(l.bend), rot[p]);
            let pos = anchor + q_rotate(q, self.rest_pos[i] - l.anchor_rest);
            rot[i] = q;
            off[i] = pos - self.rest_pos[i];
            let rv = q_to_rotvec(q);
            if rv.length() >= min_rad || off[i].length() >= min_m {
                out.push(PartFlex {
                    part: i,
                    rotation: rv,
                    offset_m: off[i],
                });
            }
        }
        out
    }

    /// Parts in depth-first order over the intact joints, parents before children.
    fn subtree_order(&self) -> Vec<usize> {
        let n = self.len();
        let mut children = vec![Vec::new(); n];
        let mut has_parent = vec![false; n];
        for l in self.links.iter().filter(|l| l.intact()) {
            children[l.parent].push(l.child);
            has_parent[l.child] = true;
        }
        let mut order = Vec::with_capacity(n);
        let mut stack: Vec<usize> = (0..n)
            .filter(|&i| self.present[i] && !has_parent[i])
            .collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            stack.extend(children[i].iter().copied());
        }
        order
    }

    /// Take parts out of the simulation (staged away, broken off): their bodies and joints are removed.
    pub fn remove_parts(&mut self, parts: &[usize]) {
        for &i in parts {
            if !self.present.get(i).copied().unwrap_or(false) {
                continue;
            }
            for l in self
                .links
                .iter_mut()
                .filter(|l| l.child == i || l.parent == i)
            {
                l.handle = None;
            }
            self.bodies.remove(
                self.bodies_of[i],
                &mut self.islands,
                &mut self.colliders,
                &mut self.impulse,
                &mut self.joints,
                &mut self.soft,
                true,
            );
            self.present[i] = false;
        }
    }

    /// Connected groups of parts over the intact joints, each sorted, the group holding part 0 first.
    pub fn groups(&self) -> Vec<Vec<usize>> {
        let n = self.len();
        let mut root: Vec<usize> = (0..n).collect();
        fn find(r: &mut [usize], mut i: usize) -> usize {
            while r[i] != i {
                r[i] = r[r[i]];
                i = r[i];
            }
            i
        }
        for l in self.links.iter().filter(|l| l.intact()) {
            let (a, b) = (find(&mut root, l.child), find(&mut root, l.parent));
            if a != b {
                root[a] = b;
            }
        }
        let mut by_root: Vec<(usize, Vec<usize>)> = Vec::new();
        for i in (0..n).filter(|&i| self.present[i]) {
            let r = find(&mut root, i);
            match by_root.iter_mut().find(|g| g.0 == r) {
                Some(g) => g.1.push(i),
                None => by_root.push((r, vec![i])),
            }
        }
        by_root.into_iter().map(|g| g.1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> (Vec<Craft>, Vec<PartDef>, Vec<ResourceDef>) {
        (
            serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
            serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
        )
    }

    #[test]
    fn joint_limits_match_the_original_formula() {
        // the reference notes' example: two size-1 stack parts with breaking force and torque 50
        let l = joint_limits(1, true, false, (50.0, 50.0), (50.0, 80.0));
        assert_eq!(l.stiffness, 1.5e6);
        assert_eq!(l.damping, 1.5e4);
        assert_eq!(l.break_force_kn, 250_000.0);
        assert_eq!(l.break_torque_knm, 250_000.0);
        // surface nodes are far weaker; size 2 divides by 3
        let s = joint_limits(0, false, false, (15.0, 50.0), (15.0, 50.0));
        assert!((s.stiffness - 1500.0 * 0.8 * 10.0).abs() < 1e-9);
        assert!((s.break_force_kn - 15.0 * 0.8 * 50.0).abs() < 1e-9);
        let big = joint_limits(2, true, false, (50.0, 50.0), (50.0, 50.0));
        assert!((big.break_force_kn - 50.0 * 150.0 / 3.0 * 50.0).abs() < 1e-6);
        assert!(joint_limits(1, true, false, (0.0, 50.0), (0.0, 50.0))
            .break_force_kn
            .is_infinite());
    }

    fn engine_force(craft: &Craft, defs: &[PartDef]) -> PartForce {
        // the lowest engine at full thrust, pushing up the stack
        let by_id: HashMap<&str, &PartDef> = defs.iter().map(|p| (p.id.as_str(), p)).collect();
        let (i, thrust) = craft
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let e = by_id.get(p.part_id.as_str())?.engines.first()?;
                Some((i, e.max_thrust_kn, p.local_position.y))
            })
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
            .map(|(i, t, _)| (i, t))
            .unwrap();
        PartForce {
            part: i,
            force_kn: DVec3::new(0.0, thrust, 0.0),
            offset_m: DVec3::ZERO,
        }
    }

    fn holds_under_thrust(name: &str) {
        let (crafts, defs, res) = assets();
        let craft = crafts.iter().find(|c| c.name == name).unwrap();
        let mut rv = RigidVessel::build(craft, &defs, &res);
        let f = engine_force(craft, &defs);
        let all: Vec<usize> = (0..rv.len()).collect();
        let start: Vec<DVec3> = all.iter().map(|&i| rv.position(i)).collect();
        let total: f64 = rv.masses_t.iter().sum();
        let g = DVec3::new(0.0, -9.81, 0.0);
        let dt = 0.02;
        for _ in 0..100 {
            let broken = rv.step(dt, g, &[f]);
            assert!(broken.is_empty(), "{name}: {broken:?}");
        }
        assert_eq!(rv.groups().len(), 1, "{name} stays one vessel");
        // the engine's joint carries the thrust less what accelerating the engine itself takes
        let mount = rv.links.iter().find(|l| l.child == f.part).unwrap();
        let expect_load = f.force_kn.y * (1.0 - rv.masses_t[f.part] / total);
        assert!(
            (mount.force_kn - expect_load).abs() < 0.03 * expect_load,
            "{name}: engine mount {} kN, expected {expect_load}",
            mount.force_kn
        );
        // every part moved with the whole craft: the shape held to within a few centimetres
        let (c, vel) = rv.centre_of(&all);
        let c0 = all
            .iter()
            .fold(DVec3::ZERO, |a, &i| a + start[i] * rv.masses_t[i])
            * (1.0 / total);
        let shift = c - c0;
        for &i in &all {
            let drift = (rv.position(i) - start[i] - shift).length();
            assert!(drift < 0.05, "{name}: part {i} drifted {drift} m");
        }
        // and the craft accelerated as one body: v = (F/m - g) t
        let expect = (f.force_kn.y / total - 9.81) * 2.0;
        assert!(
            (vel.y - expect).abs() < 0.02 * expect.abs().max(1.0),
            "{name}: v {} expected {expect}",
            vel.y
        );
    }

    #[test]
    fn small_rocket_holds_together_under_thrust() {
        holds_under_thrust("Stock12_StartRocket");
    }

    #[test]
    fn kerbal_k1_holds_together_under_thrust() {
        holds_under_thrust("Kerbal K1");
    }

    #[test]
    fn overloaded_joint_breaks_and_the_craft_splits() {
        let (crafts, defs, res) = assets();
        let craft = crafts
            .iter()
            .find(|c| c.name == "Stock12_StartRocket")
            .unwrap();
        let mut rv = RigidVessel::build(craft, &defs, &res);
        // yank the pod sideways far past what its joint takes
        let pod = 0;
        let limit = rv
            .links
            .iter()
            .filter(|l| l.parent == pod || l.child == pod)
            .map(|l| l.limits.break_force_kn)
            .fold(0.0, f64::max);
        let yank = PartForce {
            part: pod,
            force_kn: DVec3::new(limit * 8.0, 0.0, 0.0),
            offset_m: DVec3::ZERO,
        };
        let mut broken = Vec::new();
        for _ in 0..20 {
            broken.extend(rv.step(0.02, DVec3::ZERO, &[yank]));
            if !broken.is_empty() {
                break;
            }
        }
        assert!(!broken.is_empty(), "a joint should break");
        let groups = rv.groups();
        assert!(groups.len() >= 2, "{groups:?}");
        // the pieces fly apart afterwards
        let a = groups[0].clone();
        let b = groups[1].clone();
        let gap0 = (rv.centre_of(&a).0 - rv.centre_of(&b).0).length();
        for _ in 0..25 {
            rv.step(0.02, DVec3::ZERO, &[yank]);
        }
        let gap1 = (rv.centre_of(&a).0 - rv.centre_of(&b).0).length();
        assert!(gap1 > gap0 + 1.0, "gap {gap0} -> {gap1}");
    }
}

//! Per-part rigid bodies linked by the original's part joints (see `docs/reference-notes/physics-joints-drag.md`).
//!
//! One rapier body per part, one joint per parent link. Units follow the original's PhysX setup: tonnes, metres,
//! seconds, so forces are kN and torques kN*m. Translation is locked at the joint (the original holds it with a
//! 1e20 spring); rotation is an angular spring of the original's stiffness with damping
//! `stiffness * JointDampingFactor`, or locked for rigid nodes. A joint breaks when the force it carries or its
//! spring torque exceeds the original's limits, and the craft splits into the connected groups that remain.
//!
//! The vessel frame is the craft frame made right-handed: Unity Y-up with Z negated, so engines push along +Y.

use std::collections::HashMap;

use ose_data::craft::{Craft, Quat};
use ose_data::{PartDef, ResourceDef, ResourceKind};
use ose_math::DVec3;
use rapier3d_f64::math::{Pose, Rotation, Vector};
use rapier3d_f64::prelude::{
    CCDSolver, ColliderSet, DefaultBroadPhase, GenericJointBuilder, ImpulseJointSet,
    IntegrationParameters, IslandManager, JointAxesMask, JointAxis, MassProperties, MotorModel,
    MultibodyJointHandle, MultibodyJointSet, NarrowPhase, PhysicsPipeline, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, SoftBodySet,
};

/// `PhysicsSettings` defaults.
pub const JOINT_RIGIDITY: f64 = 1500.0;
pub const JOINT_STACK_NODE_FACTOR: f64 = 50.0;
pub const JOINT_SURFACE_NODE_FACTOR: f64 = 0.8;
pub const JOINT_ATTACH_STIFFNESS: f64 = 10.0;
pub const JOINT_DAMPING_FACTOR: f64 = 0.01;
pub const JOINT_BREAK_FORCE_FACTOR: f64 = 50.0;
pub const JOINT_BREAK_TORQUE_FACTOR: f64 = 50.0;

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
    handle: Option<MultibodyJointHandle>,
    /// Parent-relative child rotation at rest.
    rest: Rotation,
    /// The joint anchor in the child's frame.
    anchor_child: Vector,
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
    pub masses_t: Vec<f64>,
    pub links: Vec<Link>,
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
            let mask = if rigid {
                JointAxesMask::LOCKED_FIXED_AXES
            } else {
                JointAxesMask::LIN_AXES
            };
            let mut b = GenericJointBuilder::new(mask)
                .local_frame1(Pose::from_parts(a1, rots[parent].inverse()))
                .local_frame2(Pose::from_parts(a2, rots[i].inverse()))
                .contacts_enabled(false);
            if !rigid {
                for axis in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ] {
                    b = b.motor_model(axis, MotorModel::ForceBased).motor_position(
                        axis,
                        0.0,
                        limits.stiffness,
                        limits.damping,
                    );
                }
            }
            // reduced-coordinate (multibody) joints: a craft is a tree, and these stay stiff across the
            // 100:1 mass ratios of radial decouplers between tanks and boosters, which an iterative solver
            // cannot hold
            let h = joints.insert(bodies_of[parent], bodies_of[i], b.build(), true);
            links.push(Link {
                child: i,
                parent,
                limits,
                force_kn: 0.0,
                torque_knm: 0.0,
                handle: h,
                rest: rots[parent].inverse() * rots[i],
                anchor_child: a2,
            });
        }
        let params = IntegrationParameters::default();
        let n = masses_t.len();
        RigidVessel {
            masses_t,
            links,
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

    pub fn len(&self) -> usize {
        self.bodies_of.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies_of.is_empty()
    }

    pub fn position(&self, part: usize) -> DVec3 {
        d(self.bodies[self.bodies_of[part]].translation())
    }

    pub fn velocity(&self, part: usize) -> DVec3 {
        d(self.bodies[self.bodies_of[part]].linvel())
    }

    /// Rotation of a part as a quaternion (x, y, z, w) in the vessel frame.
    pub fn rotation(&self, part: usize) -> [f64; 4] {
        let r = self.bodies[self.bodies_of[part]].rotation();
        [r.x, r.y, r.z, r.w]
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
    /// joints that broke (already removed).
    pub fn step(&mut self, dt: f64, gravity: DVec3, forces: &[PartForce]) -> Vec<Broken> {
        let n = self.len();
        for &h in &self.bodies_of {
            self.bodies[h].reset_forces(false);
        }
        // external force and its moment about the origin, per part, for the joint loads below
        let mut ext = vec![(DVec3::ZERO, DVec3::ZERO); n];
        for f in forces {
            let Some(&h) = self.bodies_of.get(f.part) else {
                continue;
            };
            let rb = &mut self.bodies[h];
            let at = rb.translation() + v(f.offset_m);
            rb.add_force_at_point(v(f.force_kn), at, true);
            ext[f.part].0 = ext[f.part].0 + f.force_kn;
            ext[f.part].1 = ext[f.part].1 + d(at).cross(f.force_kn);
        }
        for i in 0..n {
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
        for i in 0..n {
            let a = (self.velocity(i) - self.prev_vel[i]) * (1.0 / dt);
            let need = (a - gravity) * self.masses_t[i];
            sum_f[i] = need - ext[i].0;
            sum_m[i] = self.position(i).cross(need) - ext[i].1;
        }
        for &c in self.subtree_order().iter().rev() {
            if let Some(l) = self.links.iter().find(|l| l.intact() && l.child == c) {
                let p = l.parent;
                sum_f[p] = sum_f[p] + sum_f[c];
                sum_m[p] = sum_m[p] + sum_m[c];
            }
        }
        let mut broken = Vec::new();
        for l in &mut self.links {
            let Some(h) = l.handle else { continue };
            let body = &self.bodies[self.bodies_of[l.child]];
            let anchor = d(body.position().transform_point(l.anchor_child));
            l.force_kn = sum_f[l.child].length();
            l.torque_knm = if l.limits.rigid {
                (sum_m[l.child] - anchor.cross(sum_f[l.child])).length()
            } else {
                let qp = *self.bodies[self.bodies_of[l.parent]].rotation();
                let qc = *body.rotation();
                let rel = (qp.inverse() * qc * l.rest.inverse()).normalize();
                let angle = 2.0 * rel.w.abs().min(1.0).acos();
                l.limits.stiffness * angle
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
        let mut stack: Vec<usize> = (0..n).filter(|&i| !has_parent[i]).collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            stack.extend(children[i].iter().copied());
        }
        order
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
        for i in 0..n {
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
            .fold(f64::INFINITY, f64::min);
        let yank = PartForce {
            part: pod,
            force_kn: DVec3::new(limit * 4.0, 0.0, 0.0),
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

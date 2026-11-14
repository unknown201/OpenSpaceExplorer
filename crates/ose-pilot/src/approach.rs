//! Final approach and docking with RCS: fly to a staging point on the target port's axis, then down
//! the axis at a capped closing speed until the simulation's port capture latches. There is no
//! collision avoidance: if something else is in the way, the vessel hits it.
use crate::{body, state, Status};
use bevy_ecs::entity::Entity;
use ose_data::PartDef;
use ose_math::DVec3;
use ose_sim::{docking, flight, flight::Attitude, Universe};

/// Distance of the staging point out along the target port's axis (m).
pub const STAGING_M: f64 = 120.0;

/// A port's face position (inertial, relative to the vessel's body) and outward axis. `None` if
/// the part is gone or the port is already in use.
pub fn port_pose(u: &Universe, e: Entity, guid: &str, defs: &[PartDef]) -> Option<(DVec3, DVec3)> {
    let craft = u.current_craft(e)?;
    let port = docking::free_ports(&craft, defs)
        .into_iter()
        .find(|p| craft.parts[p.part].guid == guid)?;
    let p = u.powered(e).or_else(|| u.dormant(e))?;
    let st = u.structure(e)?;
    let centre = st.rigid.rest_centre();
    let r = state(u, e)?.0;
    let position = r + st.offset_inertial(
        p.facing,
        DVec3::new(port.position.x, port.position.y, -port.position.z) - centre,
    );
    Some((position, st.axis_inertial(p.facing, port.axis)))
}

pub struct Approach {
    pub target: Entity,
    pub target_port: String,
    pub own_port: String,
    final_leg: bool,
    started: Option<f64>,
    pulses: [f64; 3],
    pub timeout_s: f64,
}

impl Approach {
    pub fn new(target: Entity, target_port: &str, own_port: &str) -> Self {
        Self {
            target,
            target_port: target_port.into(),
            own_port: own_port.into(),
            final_leg: false,
            started: None,
            pulses: [0.0; 3],
            timeout_s: 2_500.0,
        }
    }

    /// Whether the two vessels have become one.
    fn docked(&self, u: &Universe, e: Entity) -> bool {
        let gone = |x: Entity| u.world.get_entity(x).is_err() || u.current_craft(x).is_none();
        if gone(e) || gone(self.target) {
            return true;
        }
        u.current_craft(e)
            .is_some_and(|c| c.parts.iter().any(|p| p.guid == self.target_port))
    }

    pub fn update(&mut self, u: &mut Universe, e: Entity, defs: &[PartDef]) -> Status {
        if self.docked(u, e) {
            return Status::Done;
        }
        let started = *self.started.get_or_insert_with(|| {
            u.set_target(e, Some(self.target));
            u.action_group(e, ose_sim::ActionGroup::Rcs, Some(true));
            u.ut()
        });
        u.wake(self.target);
        u.wake(e);
        let (Some(ps), Some(pc)) = (u.powered(self.target).cloned(), u.powered(e).cloned()) else {
            return Status::Failed("vessel left physics".into());
        };
        if pc.crashed {
            return Status::Failed("destroyed during the approach".into());
        }
        let Some((a, axis)) = port_pose(u, self.target, &self.target_port, defs) else {
            return Status::Failed(format!("target port {} is not free", self.target_port));
        };
        let Some((b, own_axis)) = port_pose(u, e, &self.own_port, defs) else {
            return Status::Failed(format!("own port {} is not free", self.own_port));
        };
        let Some((_, mu, _)) = body(u, e) else {
            return Status::Failed("no body".into());
        };
        let goal = if self.final_leg {
            a + axis * 0.15
        } else {
            a + axis * STAGING_M
        };
        let error = goal - b;
        let station_spin = ps.angvel + ps.facing * ps.roll_rate;
        let own_spin = pc.angvel + pc.facing * pc.roll_rate;
        let rel_v = pc.v + own_spin.cross(b - pc.r) - ps.v - station_spin.cross(goal - ps.r);
        let distance = error.length();
        if !self.final_leg && distance < 15.0 && rel_v.length() < 0.3 {
            self.final_leg = true;
        }
        // Face our port down the target port's axis.
        let top = flight::vessel_top(&pc);
        let right = pc.facing.cross(top);
        let turn = glam::DQuat::from_rotation_arc(
            glam::DVec3::new(own_axis.x, own_axis.y, own_axis.z),
            -glam::DVec3::new(axis.x, axis.y, axis.z),
        );
        let face = turn * glam::DVec3::new(pc.facing.x, pc.facing.y, pc.facing.z);
        u.set_controls(e, 0.0, Attitude::Fixed(DVec3::new(face.x, face.y, face.z)));
        let accel = pc.rcs.thrust_kn / pc.mass_t;
        if accel <= 0.001 {
            return Status::Failed("no RCS thrust".into());
        }
        let max_speed = if self.final_leg {
            if distance < 5.0 {
                0.12
            } else {
                0.3
            }
        } else {
            (distance * 0.03).clamp(0.4, 15.0)
        };
        let desired = error.normalized() * (distance * 0.12).min(max_speed);
        let differential =
            pc.r * (-mu / pc.r.length().powi(3)) - ps.r * (-mu / ps.r.length().powi(3));
        let command = ((desired - rel_v) * 0.6 - differential) * (1.0 / accel);
        let command = if command.length() > 0.5 {
            command.normalized() * 0.5
        } else {
            command
        };
        // The RCS mixer has a 0.05 input deadband: pulse small inputs like a player tapping keys.
        let axes = [command.dot(right), command.dot(pc.facing), command.dot(top)];
        let pulses = &mut self.pulses;
        let axes: [f64; 3] = std::array::from_fn(|i| {
            if axes[i].abs() >= 0.05 {
                return axes[i];
            }
            pulses[i] += axes[i];
            if pulses[i].abs() >= 0.1 {
                let pulse = pulses[i].signum() * 0.1;
                pulses[i] -= pulse;
                pulse
            } else {
                0.0
            }
        });
        u.set_translate(e, DVec3::new(axes[0], axes[1], axes[2]));
        if u.ut() - started > self.timeout_s {
            u.set_translate(e, DVec3::ZERO);
            return Status::Failed(format!(
                "no capture after {:.0} s: {:.1} m from the port at {:.2} m/s",
                self.timeout_s,
                (a - b).length(),
                rel_v.length()
            ));
        }
        Status::Running
    }

    pub fn suggested_warp(&self) -> u32 {
        if self.final_leg {
            4
        } else {
            10
        }
    }
}

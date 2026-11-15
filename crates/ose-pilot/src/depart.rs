//! Leave a station: undock at our port, then back straight off with RCS to a safe distance and stop
//! relative to the station, ready for the transfer to plan from a clean orbit.
use crate::{
    approach::port_pose, control_axes, mission::Catalog, pulsed, translation_input, Status,
};
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{flight::Attitude, Universe};

pub struct Depart {
    pub own_port: String,
    /// Distance to back off before the transfer (m).
    pub back_off_m: f64,
    /// Our vessel after undocking, and the station we left.
    pub vessel: Option<Entity>,
    pub station: Option<Entity>,
    /// The station port's position and outward axis at undocking, kept in the station's frame.
    port_axis: DVec3,
    /// Attitude held while backing off, captured at undocking.
    hold: DVec3,
    started: f64,
    stopping: bool,
    pulses: [f64; 3],
}

impl Depart {
    pub fn new(own_port: &str) -> Self {
        Self {
            own_port: own_port.into(),
            back_off_m: 200.0,
            vessel: None,
            station: None,
            port_axis: DVec3::ZERO,
            hold: DVec3::ZERO,
            started: 0.0,
            stopping: false,
            pulses: [0.0; 3],
        }
    }

    pub fn update(&mut self, u: &mut Universe, docked: Entity, catalog: &Catalog) -> Status {
        let (vessel, station) = match (self.vessel, self.station) {
            (Some(v), Some(s)) => (v, s),
            _ => match self.undock(u, docked, catalog) {
                Ok(pair) => pair,
                Err(why) => return Status::Failed(why),
            },
        };
        u.wake(vessel);
        u.wake(station);
        let (Some(pv), Some(ps)) = (u.powered(vessel).cloned(), u.powered(station).cloned()) else {
            return Status::Failed("vessel left physics while departing".into());
        };
        if pv.crashed {
            return Status::Failed("destroyed while departing".into());
        }
        let away = pv.r - ps.r;
        let rel_v = pv.v - ps.v;
        let accel = pv.rcs.thrust_kn / pv.mass_t;
        if accel <= 0.001 {
            return Status::Failed("no RCS thrust to back away".into());
        }
        u.set_controls(vessel, 0.0, Attitude::Fixed(self.hold));
        if !self.stopping && away.length() > self.back_off_m {
            self.stopping = true;
        }
        let desired = if self.stopping {
            DVec3::ZERO
        } else {
            self.port_axis * 1.0
        };
        let error = desired - rel_v;
        if self.stopping && error.length() < 0.1 {
            u.set_translate(vessel, DVec3::ZERO);
            u.action_group(vessel, ose_sim::ActionGroup::Rcs, Some(false));
            return Status::Done;
        }
        let command = error * (0.8 / accel);
        let command = if command.length() > 1.0 {
            command.normalized()
        } else {
            command
        };
        let input = pulsed(&mut self.pulses, translation_input(&pv, command));
        u.set_translate(vessel, input);
        if u.ut() - self.started > 1_200.0 {
            u.set_translate(vessel, DVec3::ZERO);
            return Status::Failed("could not back away from the station".into());
        }
        Status::Running
    }

    fn undock(
        &mut self,
        u: &mut Universe,
        docked: Entity,
        catalog: &Catalog,
    ) -> Result<(Entity, Entity), String> {
        let craft = u.current_craft(docked).ok_or("vessel is gone")?;
        let index = craft
            .parts
            .iter()
            .position(|p| p.guid == self.own_port)
            .ok_or_else(|| format!("no port {}", self.own_port))?;
        u.wake(docked);
        let other = u.undock(
            docked,
            Some(index),
            catalog.defs,
            catalog.resources,
            catalog.spec,
        )?;
        let ours = |e: Entity| {
            u.current_craft(e)
                .is_some_and(|c| c.parts.iter().any(|p| p.guid == self.own_port))
        };
        let (vessel, station) = if ours(other) {
            (other, docked)
        } else {
            (docked, other)
        };
        // Back off along our freed port's outward axis, reversed: away from the station.
        let axis = port_pose(u, vessel, &self.own_port, catalog.defs)
            .map(|(_, axis)| -axis)
            .unwrap_or(DVec3::ZERO);
        if axis == DVec3::ZERO {
            return Err("could not find our port after undocking".into());
        }
        // Only our half keeps our controls; the station is left with none held.
        u.set_translate(station, DVec3::ZERO);
        u.action_group(vessel, ose_sim::ActionGroup::Rcs, Some(true));
        self.port_axis = axis;
        self.hold = u
            .powered(vessel)
            .or_else(|| u.dormant(vessel))
            .map(|p| control_axes(p).1)
            .ok_or("vessel has no flight state")?;
        self.vessel = Some(vessel);
        self.station = Some(station);
        self.started = u.ut();
        Ok((vessel, station))
    }

    pub fn suggested_warp(&self) -> u32 {
        4
    }
}

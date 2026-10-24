//! What missions look at: a log of each vessel's flight (highest point, landings, activated parts, events) and
//! the per-vessel facts the mission checks read (`ose_data::missions::VesselFacts`).

use bevy_ecs::prelude::*;
use ose_data::missions::{MissionEvent, VesselFacts};
use serde::{Deserialize, Serialize};

use crate::{bodies, flight, science, structure, Universe};

/// A vessel's flight history for missions.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightLog {
    /// Highest altitude above the ground reached, m.
    pub max_ground_altitude_m: f64,
    /// Bodies it has landed on.
    pub landed_on: Vec<String>,
    /// Part ids that have been activated (engines fired, experiments run).
    pub activated_parts: Vec<String>,
    /// Events with the tick they happened.
    pub events: Vec<(u64, MissionEvent)>,
}

impl FlightLog {
    pub fn event(&mut self, tick: u64, e: MissionEvent) {
        self.events.push((tick, e));
        // keep the log bounded: missions only look at events since their stage began
        if self.events.len() > 256 {
            self.events.drain(..self.events.len() - 256);
        }
    }

    pub fn activated(&mut self, part_id: &str) {
        if !self.activated_parts.iter().any(|p| p == part_id) {
            self.activated_parts.push(part_id.to_string());
        }
    }
}

/// The last vessel destroyed on impact: its highest altitude and the body it hit.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct LastDestroyed {
    pub max_ground_altitude_m: f64,
    pub body: String,
}

impl Universe {
    pub fn flight_log(&self, e: Entity) -> Option<&FlightLog> {
        self.world.get::<FlightLog>(e)
    }

    pub(crate) fn log_mut(&mut self, e: Entity) -> Option<Mut<'_, FlightLog>> {
        if self.world.get::<FlightLog>(e).is_none() {
            self.world
                .get_entity_mut(e)
                .ok()?
                .insert(FlightLog::default());
        }
        self.world.get_mut::<FlightLog>(e)
    }

    pub fn last_destroyed(&self) -> LastDestroyed {
        self.world
            .get_resource::<LastDestroyed>()
            .cloned()
            .unwrap_or_default()
    }

    /// Record one flight step for missions: altitude, a touchdown, a crash.
    pub(crate) fn log_flight_step(
        &mut self,
        e: Entity,
        body: &bodies::Body,
        terrain: Option<&ose_data::terrain::TerrainMap>,
        p: &flight::Powered,
        was_landed: bool,
        ut: f64,
    ) {
        // debris is not flown: it neither counts for missions nor replaces the last destroyed vessel
        if self.world.get::<crate::vessels::Debris>(e).is_some() {
            return;
        }
        let tick = self.tick().0;
        let ground = flight::ground_radius(body, terrain, p.r, ut);
        let alt = p.r.length() - ground;
        let (lat, lon) = flight::lat_lon(body, p.r, ut);
        let on_water = terrain.is_some_and(|t| t.has_ocean && t.height_m(lat, lon) < 0.0);
        let body_id = body.id.clone();
        let Some(mut log) = self.log_mut(e) else {
            return;
        };
        if !p.landed {
            log.max_ground_altitude_m = log.max_ground_altitude_m.max(alt);
        }
        if p.crashed && !was_landed {
            let max = log.max_ground_altitude_m;
            log.event(tick, MissionEvent::VesselDestroyed);
            self.world.insert_resource(LastDestroyed {
                max_ground_altitude_m: max,
                body: body_id,
            });
            return;
        }
        if p.landed && !was_landed {
            log.event(
                tick,
                if on_water {
                    MissionEvent::LandedOnWater
                } else {
                    MissionEvent::LandedOnGround
                },
            );
            log.event(tick, MissionEvent::LandedAfterFlight);
            if !log.landed_on.contains(&body_id) {
                log.landed_on.push(body_id);
            }
        }
    }

    /// The mission facts of a vessel, with events since `since_tick`.
    pub fn mission_facts(&self, e: Entity, since_tick: u64) -> Option<VesselFacts> {
        let (_, body_e, orbit) = self.vessel(e)?;
        let body = self.world.get::<bodies::Body>(body_e)?;
        let ut = self.ut();
        let terrain = self
            .world
            .get::<flight::Terrain>(body_e)
            .map(|t| t.0.clone());
        let powered = self.world.get::<flight::Powered>(e);
        let (r, v) = match powered {
            Some(p) => (p.r, p.v),
            None => orbit.state_at(ut),
        };
        let ground = flight::ground_radius(body, terrain.as_deref(), r, ut);
        let mut f = VesselFacts {
            soi: body.id.clone(),
            ground_altitude_m: r.length() - ground,
            sea_level_altitude_m: r.length() - body.radius_m,
            periapsis_altitude_m: orbit.periapsis() - body.radius_m,
            apoapsis_altitude_m: if orbit.eccentricity() < 1.0 {
                orbit.apoapsis() - body.radius_m
            } else {
                f64::INFINITY
            },
            orbit_speed_mps: v.length(),
            mass_t: powered.map(|p| p.mass_t).unwrap_or(0.0),
            ..Default::default()
        };
        f.crew = self
            .world
            .get::<crate::crew::Crew>(e)
            .map(|c| c.members.len() as i64)
            .unwrap_or(0);
        f.eva = crate::crew::is_eva(&self.world, e);
        if f.eva {
            f.crew = 1;
            f.parts.push("eva_kerbal".into());
        }
        if let Some(st) = self.world.get::<structure::Structure>(e) {
            for i in (0..st.rigid.len()).filter(|&i| st.rigid.present[i]) {
                f.parts.push(st.part_ids[i].clone());
                f.part_families.push(st.families[i].clone());
                if st.families[i].ends_with("-Wheel") {
                    f.wheels += 1;
                }
            }
        }
        f.stored_reports = self
            .world
            .get::<science::ScienceStorage>(e)
            .map(|s| s.reports.iter().map(|r| r.key()).collect())
            .unwrap_or_default();
        if let Some(loc) = self.science_location(e) {
            f.research_location = match &loc.region {
                Some(r) => format!("{}_{}_{}", loc.body, loc.situation.name(), r),
                None => format!("{}_{}", loc.body, loc.situation.name()),
            };
        }
        for patch in self.predict(e) {
            if let Some(id) = self.body_id(patch.body) {
                if id != body.id && !f.predicted_sois.iter().any(|x| x == id) {
                    f.predicted_sois.push(id.to_string());
                }
            }
        }
        if let Some(log) = self.world.get::<FlightLog>(e) {
            f.max_ground_altitude_m = log.max_ground_altitude_m;
            f.landed_on = log.landed_on.clone();
            f.activated_parts = log.activated_parts.clone();
            f.events = log
                .events
                .iter()
                .filter(|(t, _)| *t >= since_tick)
                .map(|(_, ev)| ev.clone())
                .collect();
        }
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_data::missions::{AgencyFacts, MissionDef, MissionProgress};

    #[test]
    fn the_first_mission_completes_when_a_k1_goes_above_10_km_and_comes_down() {
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
        let missions: Vec<MissionDef> =
            serde_json::from_str(include_str!("../../../assets/own/missions.json")).unwrap();
        let first = missions.iter().find(|m| m.id == "Main_Kerbin_01").unwrap();
        assert!(first.playable() && first.active_at_start);
        let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
        let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
        let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
        let mut u = Universe::with_bodies(&[kerbin]);
        let body = u.body_entity("Kerbin").unwrap();
        let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
        u.set_structure(v, structure::Structure::build(k1, &defs, &res));
        assert!(u.set_controls(v, 1.0, flight::Attitude::Up));
        assert!(
            u.stage(v).debris.is_none(),
            "the first press lights the engines"
        );
        let mut progress = MissionProgress::default();
        let mut stage_start = 0;
        let mut earned = 0.0;
        for t in 0..(600 * crate::TICKS_PER_SECOND) {
            u.step();
            // climb to 12 km, then cut the engines and let it fall
            if u.powered(v)
                .is_some_and(|p| p.r.length() - 600_000.0 > 12_000.0)
            {
                u.set_controls(v, 0.0, flight::Attitude::Up);
            }
            if t % 25 == 0 {
                let facts = u.mission_facts(v, stage_start).unwrap();
                let last = u.last_destroyed();
                let agency = AgencyFacts {
                    scored_reports: vec![],
                    last_destroyed_max_ground_altitude_m: last.max_ground_altitude_m,
                    last_destroyed_hit: last.body,
                };
                let (got, moved) = progress.advance(first, &[(1, facts)], &agency);
                earned += got;
                if moved {
                    stage_start = u.tick().0;
                }
                if progress.complete {
                    break;
                }
            }
        }
        assert!(progress.complete, "stuck at stage {}", progress.stage);
        assert_eq!(earned, first.science_reward());
        assert!(u.flight_log(v).unwrap().max_ground_altitude_m > 10_000.0);
    }
}

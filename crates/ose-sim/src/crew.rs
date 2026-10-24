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
/// Jetpack specific impulse. The original does not use an Isp for the jetpack; with this the Kerbal's
/// 0.025 t of monopropellant lasts about 100 s at full thrust.
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

impl Universe {
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
            // a jetpack fires on the throttle without staging
            p.ignited = true;
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
        self.world.despawn(kerbal);
        Ok(())
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
        // and back in
        u.board(jeb, v).unwrap();
        assert_eq!(u.crew(v).len(), 1);
        assert!(u.vessel(jeb).is_none());
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

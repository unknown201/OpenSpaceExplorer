//! Science on vessels: where a vessel is for science, running the experiments on its parts and the reports
//! it carries until they are transmitted or recovered (see `ose_data::science` for the rules).

use bevy_ecs::prelude::*;
use ose_data::science::{
    run_experiment, BodyScience, ResearchReport, ScienceCatalog, ScienceSituation,
};
use ose_math::DVec3;
use serde::{Deserialize, Serialize};

use crate::{bodies, flight, structure, Universe};

/// The science definitions and region maps, as a world resource.
#[derive(Resource, Clone, Debug, Default)]
pub struct Science {
    pub catalog: ScienceCatalog,
    pub regions: Vec<u8>,
}

/// Reports a vessel carries.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScienceStorage {
    pub reports: Vec<ResearchReport>,
}

/// Reports going home over the vessel's best antenna, one at a time (`ScienceStorageComponent`): each
/// with its size and how much of it is sent, science data units.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct Transmission {
    pub queue: Vec<(ResearchReport, f64, f64)>,
    /// Why it is waiting, when it is (no link, no power, no antenna).
    pub stalled: Option<&'static str>,
}

/// Reports that finished sending, waiting for the server to score them: (vessel, report).
#[derive(Resource, Default)]
pub struct Transmitted(pub Vec<(Entity, ResearchReport)>);

/// Where a vessel is for science.
#[derive(Clone, Debug, PartialEq)]
pub struct ScienceLocation {
    pub body: String,
    pub situation: ScienceSituation,
    pub situation_scalar: f64,
    pub region: Option<String>,
}

/// Why an experiment on a part did not give a report.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub part: usize,
    pub experiment: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunOutcome {
    pub reports: Vec<ResearchReport>,
    pub skipped: Vec<Skipped>,
    /// Experiments that take time and are now running.
    pub started: Vec<Run>,
}

/// An experiment taking its time (`TimeToComplete`): it finishes where it started, drawing its charge while it
/// runs; moving to another situation or region interrupts it, and without charge it waits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub part: usize,
    pub experiment: String,
    /// Where it started (`{Body}_{Situation}` plus region).
    pub location: String,
    pub remaining_s: f64,
    pub electric_rate: f64,
}

#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunningExperiments {
    pub runs: Vec<Run>,
}

/// Experiments that finished or were interrupted since the last `take_experiment_events`.
#[derive(Resource, Clone, Debug, Default)]
pub struct ExperimentEvents {
    pub finished: Vec<(Entity, Run, Vec<ResearchReport>)>,
    pub interrupted: Vec<(Entity, Run)>,
}

fn location_id(loc: &ScienceLocation) -> String {
    match &loc.region {
        Some(r) => format!("{}_{}_{}", loc.body, loc.situation.name(), r),
        None => format!("{}_{}", loc.body, loc.situation.name()),
    }
}

/// A body-fixed position in the original's body frame (Unity Y-up): x towards longitude 0, z towards 90 E.
pub fn body_frame_position(lat_deg: f64, lon_deg: f64, radius_m: f64) -> DVec3 {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    DVec3::new(
        radius_m * lat.cos() * lon.cos(),
        radius_m * lat.sin(),
        radius_m * lat.cos() * lon.sin(),
    )
}

impl Universe {
    pub fn set_science(&mut self, catalog: ScienceCatalog, regions: Vec<u8>) {
        self.world.insert_resource(Science { catalog, regions });
    }

    pub fn science(&self) -> Option<&Science> {
        self.world.get_resource::<Science>()
    }

    /// Where a vessel is for science, from its flight state when it has one, else its orbit.
    pub fn science_location(&self, e: Entity) -> Option<ScienceLocation> {
        let sci = self.world.get_resource::<Science>()?;
        let body_e = self.world.get::<bodies::ReferenceBody>(e)?.0;
        let body = self.world.get::<bodies::Body>(body_e)?;
        let bs: &BodyScience = sci.catalog.body(&body.id)?;
        let ut = self.ut();
        let (r, landed) = match self.world.get::<flight::Powered>(e) {
            Some(p) => (p.r, p.landed && !p.crashed),
            None => (
                self.world.get::<bodies::OnRails>(e)?.0.state_at(ut).0,
                false,
            ),
        };
        let alt = r.length() - body.radius_m;
        let (lat, lon) = flight::lat_lon(body, r, ut);
        // on the sea rather than the ground
        let splashed = landed
            && self
                .world
                .get::<flight::Terrain>(body_e)
                .is_some_and(|t| t.0.has_ocean && t.0.height_m(lat, lon) < 0.0);
        let (situation, situation_scalar) = bs.situation(alt, landed, splashed)?;
        let at = landed.then(|| body_frame_position(lat, lon, r.length()));
        let region = bs
            .region_at(&sci.regions, lat, lon, at)
            .map(|r| r.id.clone());
        Some(ScienceLocation {
            body: body.id.clone(),
            situation,
            situation_scalar,
            region,
        })
    }

    /// Run every experiment on the vessel's attached parts here. New reports (not already carried) are
    /// stored on the vessel and returned; experiments that need crew or charge it lacks, or that do not
    /// work here, are listed as skipped. Experiment run times are not waited for yet.
    pub fn run_experiments(&mut self, e: Entity) -> RunOutcome {
        let mut out = RunOutcome::default();
        let Some(loc) = self.science_location(e) else {
            return out;
        };
        // a vessel's parts, or a Kerbal on EVA with the experiments of the eva_kerbal part
        let mut st = self.world.entity_mut(e).take::<structure::Structure>();
        let kerbal = self.world.get::<crate::crew::EvaKerbal>(e).cloned();
        let (experiments, crewed) = match (&st, &kerbal) {
            (Some(st), _) => (
                st.experiments.clone(),
                (0..st.rigid.len()).any(|i| st.rigid.present[i] && st.crew[i] > 0)
                    && !self.crew(e).is_empty(),
            ),
            (None, Some(k)) => (k.experiments.iter().map(|x| (0, x.clone())).collect(), true),
            (None, None) => return out,
        };
        let part_id = |st: &Option<structure::Structure>, part: usize| match st {
            Some(st) => st.part_ids[part].clone(),
            None => "eva_kerbal".to_string(),
        };
        let sci = self.world.resource::<Science>().clone();
        let mut storage = self
            .world
            .get::<ScienceStorage>(e)
            .cloned()
            .unwrap_or_default();
        let bs = sci
            .catalog
            .body(&loc.body)
            .expect("located on a body with science");
        let region = loc
            .region
            .as_deref()
            .and_then(|id| bs.regions.iter().find(|r| r.id == id));
        let ec = st.as_ref().and_then(|s| s.net.resource("ElectricCharge"));
        let mut activated = Vec::new();
        for (part, pe) in experiments {
            if st.as_ref().is_some_and(|s| !s.rigid.present[part]) {
                continue;
            }
            let skip = |reason: &str| Skipped {
                part,
                experiment: pe.experiment.clone(),
                reason: reason.to_string(),
            };
            let Some(def) = sci.catalog.experiment(&pe.experiment) else {
                out.skipped.push(skip("unknown experiment"));
                continue;
            };
            if pe.crew_required > 0 && !crewed {
                out.skipped.push(skip("needs crew"));
                continue;
            }
            let reports = run_experiment(def, bs, loc.situation, loc.situation_scalar, region);
            if reports.is_empty() {
                out.skipped.push(skip("not here"));
                continue;
            }
            let fresh: Vec<ResearchReport> = reports
                .into_iter()
                .filter(|r| storage.reports.iter().all(|s| s.key() != r.key()))
                .collect();
            if fresh.is_empty() {
                out.skipped.push(skip("already stored"));
                continue;
            }
            if pe.time_to_complete_s > 0.0 {
                let mut runs = self
                    .world
                    .get::<RunningExperiments>(e)
                    .cloned()
                    .unwrap_or_default();
                if runs
                    .runs
                    .iter()
                    .any(|r| r.part == part && r.experiment == pe.experiment)
                {
                    out.skipped.push(skip("running"));
                    continue;
                }
                let run = Run {
                    part,
                    experiment: pe.experiment.clone(),
                    location: location_id(&loc),
                    remaining_s: pe.time_to_complete_s,
                    electric_rate: pe.electric_rate,
                };
                runs.runs.push(run.clone());
                self.world.entity_mut(e).insert(runs);
                out.started.push(run);
                activated.push(part_id(&st, part));
                continue;
            }
            if let (true, Some(st)) = (pe.electric_rate > 0.0, st.as_mut()) {
                let need = pe.electric_rate;
                let have = ec.map(|r| st.net.available(part, r)).unwrap_or(0.0);
                if have < need {
                    out.skipped.push(skip("not enough charge"));
                    continue;
                }
                let r = ec.expect("charge");
                st.net.process(
                    &[crate::resources::Request {
                        part,
                        instructions: vec![crate::resources::Instruction {
                            resource: r,
                            units_per_s: need,
                        }],
                        priority: 0,
                    }],
                    1.0,
                );
            }
            storage.reports.extend(fresh.iter().cloned());
            out.reports.extend(fresh);
            activated.push(part_id(&st, part));
        }
        if let Some(st) = st {
            self.world.entity_mut(e).insert(st);
        }
        self.world.entity_mut(e).insert(storage);
        let tick = self.tick().0;
        if let Some(mut log) = self.log_mut(e) {
            for id in &activated {
                log.activated(id);
            }
            if !out.reports.is_empty() {
                log.event(tick, ose_data::missions::MissionEvent::ReportAcquired);
            }
        }
        out
    }

    /// Advance running experiments by `dt` seconds: charge drawn, interrupted on leaving their location,
    /// stored when done.
    pub fn advance_experiments(&mut self, dt: f64) {
        let vessels: Vec<Entity> = {
            let mut q = self
                .world
                .query_filtered::<Entity, With<RunningExperiments>>();
            q.iter(&self.world).collect()
        };
        for e in vessels {
            let Some(mut runs) = self.world.get::<RunningExperiments>(e).cloned() else {
                continue;
            };
            if runs.runs.is_empty() {
                continue;
            }
            let here = self.science_location(e);
            let mut st = self.world.entity_mut(e).take::<structure::Structure>();
            let mut done = Vec::new();
            let mut interrupted = Vec::new();
            runs.runs.retain_mut(|run| {
                let Some(loc) = &here else {
                    interrupted.push(run.clone());
                    return false;
                };
                if location_id(loc) != run.location
                    || st.as_ref().is_some_and(|s| !s.rigid.present[run.part])
                {
                    interrupted.push(run.clone());
                    return false;
                }
                // without the charge it needs it waits
                let mut powered = true;
                if run.electric_rate > 0.0 {
                    powered = false;
                    if let Some(s) = &mut st {
                        if let Some(ec) = s.net.resource("ElectricCharge") {
                            let need = run.electric_rate * dt;
                            if s.net.available(run.part, ec) >= need {
                                s.net.process(
                                    &[crate::resources::Request {
                                        part: run.part,
                                        instructions: vec![crate::resources::Instruction {
                                            resource: ec,
                                            units_per_s: run.electric_rate,
                                        }],
                                        priority: 3,
                                    }],
                                    dt,
                                );
                                powered = true;
                            }
                        }
                    }
                }
                if powered {
                    run.remaining_s -= dt;
                }
                if run.remaining_s <= 0.0 {
                    done.push(run.clone());
                    return false;
                }
                true
            });
            if let Some(s) = st {
                self.world.entity_mut(e).insert(s);
            }
            self.world.entity_mut(e).insert(runs);
            let mut finished = Vec::new();
            if let (Some(loc), false) = (&here, done.is_empty()) {
                let sci = self.world.resource::<Science>().clone();
                let Some(bs) = sci.catalog.body(&loc.body) else {
                    continue;
                };
                let region = loc
                    .region
                    .as_deref()
                    .and_then(|id| bs.regions.iter().find(|r| r.id == id));
                let mut storage = self
                    .world
                    .get::<ScienceStorage>(e)
                    .cloned()
                    .unwrap_or_default();
                for run in done {
                    let Some(def) = sci.catalog.experiment(&run.experiment) else {
                        continue;
                    };
                    let fresh: Vec<ResearchReport> =
                        run_experiment(def, bs, loc.situation, loc.situation_scalar, region)
                            .into_iter()
                            .filter(|r| storage.reports.iter().all(|s| s.key() != r.key()))
                            .collect();
                    storage.reports.extend(fresh.iter().cloned());
                    finished.push((run, fresh));
                }
                self.world.entity_mut(e).insert(storage);
            }
            let tick = self.tick().0;
            if finished.iter().any(|(_, r)| !r.is_empty()) {
                if let Some(mut log) = self.log_mut(e) {
                    log.event(tick, ose_data::missions::MissionEvent::ReportAcquired);
                }
            }
            let mut ev = self
                .world
                .get_resource_or_insert_with::<ExperimentEvents>(Default::default);
            for (run, reports) in finished {
                ev.finished.push((e, run, reports));
            }
            for run in interrupted {
                ev.interrupted.push((e, run));
            }
        }
    }

    /// Experiments that finished or were interrupted since the last call.
    pub fn take_experiment_events(&mut self) -> ExperimentEvents {
        self.world
            .get_resource_mut::<ExperimentEvents>()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }

    pub fn set_running_experiments(&mut self, e: Entity, runs: Vec<Run>) {
        self.world.entity_mut(e).insert(RunningExperiments { runs });
    }

    pub fn running_experiments(&self, e: Entity) -> Vec<Run> {
        self.world
            .get::<RunningExperiments>(e)
            .map(|r| r.runs.clone())
            .unwrap_or_default()
    }

    /// Reports a vessel carries.
    pub fn science_carried(&self, e: Entity) -> Vec<ResearchReport> {
        self.world
            .get::<ScienceStorage>(e)
            .map(|s| s.reports.clone())
            .unwrap_or_default()
    }

    /// Take every report off the vessel (transmitted or recovered).
    pub fn take_science(&mut self, e: Entity) -> Vec<ResearchReport> {
        match self.world.get_mut::<ScienceStorage>(e) {
            Some(mut s) => std::mem::take(&mut s.reports),
            None => Vec::new(),
        }
    }

    /// The antenna that sends: the active one with the biggest packet (part, packet units, s per packet,
    /// EC/s), as the original picks it.
    fn best_transmitter(&self, e: Entity) -> Option<(usize, f64, f64, f64)> {
        let st = self.world.get::<structure::Structure>(e)?;
        (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i] && (!st.antenna_deployable[i] || st.deployed[i]))
            .map(|i| (i, st.electric[i]))
            .filter(|(_, x)| x.transmit_packet > 0.0 && x.transmit_interval_s > 0.0)
            .max_by(|a, b| a.1.transmit_packet.total_cmp(&b.1.transmit_packet))
            .map(|(i, x)| {
                (
                    i,
                    x.transmit_packet,
                    x.transmit_interval_s,
                    x.transmit_ec_per_s,
                )
            })
    }

    /// Start sending the vessel's data reports home (samples have to be recovered), each of its
    /// experiment's `TransmissionSize`; they finish one by one in `take_transmitted`. Returns how many were
    /// queued.
    pub fn start_transmission(&mut self, e: Entity) -> Result<usize, &'static str> {
        if self.best_transmitter(e).is_none() {
            return Err("no working antenna (a folded one has to be extended)");
        }
        let data: Vec<ResearchReport> = match self.world.get_mut::<ScienceStorage>(e) {
            Some(mut s) => {
                let (data, samples): (Vec<_>, Vec<_>) = std::mem::take(&mut s.reports)
                    .into_iter()
                    .partition(|r| !r.sample);
                s.reports = samples;
                data
            }
            None => Vec::new(),
        };
        if data.is_empty() {
            return Err("no data reports to send (samples have to be recovered)");
        }
        let n = data.len();
        let size_of = |r: &ResearchReport| {
            self.world
                .get_resource::<Science>()
                .and_then(|s| {
                    s.catalog
                        .experiments
                        .iter()
                        .find(|x| x.id == r.experiment)
                        .map(|x| x.transmission_size)
                })
                .unwrap_or(5.0)
        };
        let mut t = self
            .world
            .get::<Transmission>(e)
            .cloned()
            .unwrap_or_default();
        t.queue.extend(data.into_iter().map(|r| {
            let size = size_of(&r).max(0.0);
            (r, size, 0.0)
        }));
        self.world.entity_mut(e).insert(t);
        Ok(n)
    }

    /// What the vessel is sending: the report going now, the share of it sent, how many wait, and why it
    /// is held up if it is.
    pub fn transmission(
        &self,
        e: Entity,
    ) -> Option<(&ResearchReport, f64, usize, Option<&'static str>)> {
        let t = self.world.get::<Transmission>(e)?;
        let (r, size, done) = t.queue.first()?;
        let share = if *size > 0.0 {
            (done / size).min(1.0)
        } else {
            1.0
        };
        Some((r, share, t.queue.len() - 1, t.stalled))
    }

    /// Reports that finished sending since the last call.
    pub fn take_transmitted(&mut self) -> Vec<(Entity, ResearchReport)> {
        self.world
            .get_resource_mut::<Transmitted>()
            .map(|mut t| std::mem::take(&mut t.0))
            .unwrap_or_default()
    }

    /// Send for `dt` seconds: a packet each interval while the vessel has its link to the KSC and the
    /// antenna its power; held up otherwise (it carries on when they come back).
    pub(crate) fn run_transmissions(&mut self, dt: f64) {
        let sending: Vec<Entity> = self
            .world
            .query_filtered::<Entity, With<Transmission>>()
            .iter(&self.world)
            .collect();
        for e in sending {
            let stalled = if !self.comm_link(e).connected {
                Some("no CommNet connection to the KSC")
            } else if let Some((part, packet, interval, ec_rate)) = self.best_transmitter(e) {
                let power = match self.world.get_mut::<structure::Structure>(e) {
                    Some(mut st) if ec_rate > 0.0 => match st.net.resource("ElectricCharge") {
                        Some(ec) => st.net.process(
                            &[crate::resources::Request {
                                part,
                                instructions: vec![crate::resources::Instruction {
                                    resource: ec,
                                    units_per_s: ec_rate,
                                }],
                                priority: 0,
                            }],
                            dt,
                        )[0],
                        None => 0.0,
                    },
                    _ => 1.0,
                };
                if power < 0.999 {
                    Some("out of electric charge")
                } else {
                    let mut t = self.world.get_mut::<Transmission>(e).expect("sending");
                    let mut left = packet * dt / interval;
                    let mut done = Vec::new();
                    while left > 0.0 {
                        let Some(first) = t.queue.first_mut() else {
                            break;
                        };
                        // whole packets: a report of size s takes ceil(s / packet) of them
                        let need = (first.1 / packet).ceil() * packet - first.2;
                        if left >= need {
                            left -= need;
                            done.push(t.queue.remove(0).0);
                        } else {
                            first.2 += left;
                            left = 0.0;
                        }
                    }
                    if !done.is_empty() {
                        let mut out = self.world.get_resource_or_insert_with(Transmitted::default);
                        out.0.extend(done.into_iter().map(|r| (e, r)));
                    }
                    None
                }
            } else {
                Some("no working antenna")
            };
            let empty = match self.world.get_mut::<Transmission>(e) {
                Some(mut t) => {
                    t.stalled = stalled;
                    t.queue.is_empty()
                }
                None => true,
            };
            if empty {
                self.world.entity_mut(e).remove::<Transmission>();
            }
        }
    }

    /// Put reports back on a vessel (a restored save).
    pub fn set_science_carried(&mut self, e: Entity, reports: Vec<ResearchReport>) {
        self.world.entity_mut(e).insert(ScienceStorage { reports });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn science() -> (ScienceCatalog, Vec<u8>) {
        (
            serde_json::from_str(include_str!("../../../assets/own/science/index.json")).unwrap(),
            include_bytes!("../../../assets/own/science/regions.bin").to_vec(),
        )
    }

    #[test]
    fn the_body_frame_matches_the_original_landmarks() {
        // KSC's landmark centre (kerbin discoverables) sits at the pad's latitude/longitude
        let (cat, _) = science();
        let ksc = cat
            .body("Kerbin")
            .unwrap()
            .discoverables
            .iter()
            .find(|d| d.region == "KerbinKSC")
            .unwrap()
            .clone();
        let pad = body_frame_position(-0.6836, -74.8198, 600_115.0);
        assert!(
            (pad - ksc.position).length() < ksc.radius_m,
            "pad {pad:?} vs KSC {:?} r {}",
            ksc.position,
            ksc.radius_m
        );
    }

    fn k1_universe() -> (Universe, Entity, Entity) {
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
        let (cat, regions) = science();
        u.set_science(cat, regions);
        let body = u.body_entity("Kerbin").unwrap();
        let v = u.spawn_powered_on_surface("K1", body, spec, -0.6836, -74.8198);
        u.set_structure(v, structure::Structure::build(k1, &defs, &res));
        (u, v, body)
    }

    #[test]
    fn a_crewed_pod_on_the_pad_observes_the_ksc_once() {
        let (mut u, v, _) = k1_universe();
        let loc = u.science_location(v).unwrap();
        assert_eq!(loc.situation, ScienceSituation::Landed);
        assert_eq!(loc.region.as_deref(), Some("KerbinKSC"));
        let first = u.run_experiments(v);
        let obs = first
            .reports
            .iter()
            .find(|r| r.experiment == "CrewObservation")
            .expect("the pod's crew observation");
        assert_eq!(obs.location, "Kerbin_Landed_KerbinKSC");
        assert!(obs.value > 0.0);
        assert_eq!(u.science_carried(v).len(), first.reports.len());
        // a second run here adds nothing
        let again = u.run_experiments(v);
        assert!(again.reports.is_empty());
        assert!(again.skipped.iter().any(|s| s.reason == "already stored"));
        // transmitting empties the storage
        assert_eq!(u.take_science(v).len(), first.reports.len());
        assert!(u.science_carried(v).is_empty());
    }

    #[test]
    fn a_timed_experiment_runs_for_its_time_on_charge_and_stops_if_moved() {
        let (mut u, v, _) = k1_universe();
        // give the K1 the original's 60 s, 6 charge/s radiation experiment on its pod
        let mut st = u
            .world
            .entity_mut(v)
            .take::<structure::Structure>()
            .unwrap();
        st.experiments = vec![(
            0,
            ose_data::science::PartExperiment {
                experiment: "CrewObservation".into(),
                crew_required: 0,
                time_to_complete_s: 60.0,
                electric_rate: 0.5,
            },
        )];
        u.world.entity_mut(v).insert(st);
        let out = u.run_experiments(v);
        assert!(out.reports.is_empty());
        assert_eq!(out.started.len(), 1);
        assert_eq!(u.run_experiments(v).skipped[0].reason, "running");
        for _ in 0..59 {
            u.advance_experiments(1.0);
        }
        assert!(u.science_carried(v).is_empty(), "not done before its time");
        u.advance_experiments(1.0);
        let ev = u.take_experiment_events();
        assert_eq!(ev.finished.len(), 1);
        assert_eq!(ev.finished[0].2[0].location, "Kerbin_Landed_KerbinKSC");
        assert_eq!(u.science_carried(v).len(), 1);
        // a second run, then the vessel leaves the pad: interrupted
        u.take_science(v);
        u.run_experiments(v);
        let mut p = u.powered(v).unwrap().clone();
        p.landed = false;
        p.r = p.r * (1.0 + 20_000.0 / p.r.length());
        u.set_powered(v, p);
        u.advance_experiments(1.0);
        let ev = u.take_experiment_events();
        assert_eq!(ev.interrupted.len(), 1);
        assert!(u.running_experiments(v).is_empty());
    }

    #[test]
    fn orbit_is_low_orbit_and_worth_more() {
        let (mut u, v, _) = k1_universe();
        let landed = u.run_experiments(v);
        let landed_obs = landed
            .reports
            .iter()
            .find(|r| r.experiment == "CrewObservation")
            .unwrap()
            .value;
        // put it in a 100 km orbit
        let mut p = u.powered(v).unwrap().clone();
        let r = 700_000.0;
        let mu = u
            .world
            .get::<bodies::Body>(u.body_entity("Kerbin").unwrap())
            .unwrap()
            .mu;
        p.r = DVec3::new(r, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / r).sqrt(), 0.0);
        p.landed = false;
        u.set_powered(v, p);
        let loc = u.science_location(v).unwrap();
        assert_eq!(loc.situation, ScienceSituation::LowOrbit);
        let orbit = u.run_experiments(v);
        let obs = orbit
            .reports
            .iter()
            .find(|r| r.experiment == "CrewObservation")
            .unwrap();
        assert!(
            obs.location.starts_with("Kerbin_LowOrbit_"),
            "{}",
            obs.location
        );
        assert!(obs.value > landed_obs, "{} vs {landed_obs}", obs.value);
    }
}

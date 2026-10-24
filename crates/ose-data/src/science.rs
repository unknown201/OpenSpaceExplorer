//! Science and the tech tree, as the original defines them (see `docs/reference-notes/science.md`).
//!
//! Experiments come from the `scienceExperiment` assets, science regions (biomes) and situation altitudes
//! from `science_region`, landmark sites from `science_region_discoverables`, the baked region maps from
//! `science_region_map` (downsampled to 512 x 256 for `assets/own/science/regions.bin`) and the tech nodes
//! from `techNodeData`.

use serde::{Deserialize, Serialize};

/// Where a vessel is for science purposes (`ScienceSitutation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScienceSituation {
    Landed,
    Splashed,
    Atmosphere,
    LowOrbit,
    HighOrbit,
}

impl ScienceSituation {
    pub fn name(self) -> &'static str {
        match self {
            ScienceSituation::Landed => "Landed",
            ScienceSituation::Splashed => "Splashed",
            ScienceSituation::Atmosphere => "Atmosphere",
            ScienceSituation::LowOrbit => "LowOrbit",
            ScienceSituation::HighOrbit => "HighOrbit",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "Landed" => ScienceSituation::Landed,
            "Splashed" => ScienceSituation::Splashed,
            "Atmosphere" => ScienceSituation::Atmosphere,
            "LowOrbit" => ScienceSituation::LowOrbit,
            "HighOrbit" => ScienceSituation::HighOrbit,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExperimentKind {
    Data,
    Sample,
    Both,
}

/// One entry of an experiment's `ValidLocations`; empty body or region and `situation: None` match anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidLocation {
    pub body: String,
    pub situation: Option<ScienceSituation>,
    pub region: String,
    pub requires_region: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentDef {
    pub id: String,
    pub display_name_key: String,
    pub kind: ExperimentKind,
    pub valid: Vec<ValidLocation>,
    pub data_value: f64,
    pub sample_value: f64,
    pub transmission_size: f64,
    pub requires_eva: bool,
}

impl ExperimentDef {
    /// The original's `IsLocationValid`: the first matching entry, and whether it wants the region.
    pub fn location(&self, body: &str, situation: ScienceSituation, region: &str) -> Option<bool> {
        self.valid
            .iter()
            .find(|l| {
                (l.body.is_empty() || l.body.eq_ignore_ascii_case(body))
                    && l.situation.is_none_or(|s| s == situation)
                    && (l.region.is_empty() || l.region.eq_ignore_ascii_case(region))
            })
            .map(|l| l.requires_region)
    }
}

/// Situation altitudes and multipliers of a body (`CBSituationData`); a scalar of 0 or less disables it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SituationData {
    pub high_orbit_max_m: f64,
    pub low_orbit_max_m: f64,
    pub atmosphere_max_m: f64,
    pub body_scalar: f64,
    pub high_orbit_scalar: f64,
    pub low_orbit_scalar: f64,
    pub atmosphere_scalar: f64,
    pub splashed_scalar: f64,
    pub landed_scalar: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionDef {
    pub id: String,
    pub map_id: u8,
    pub atmosphere_scalar: f64,
    pub splashed_scalar: f64,
    pub landed_scalar: f64,
}

/// A landmark region around a point (body frame, Unity Y-up metres), only when landed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Discoverable {
    pub region: String,
    pub position: ose_math::DVec3,
    pub radius_m: f64,
}

/// A region map in `regions.bin`: `width x height` region map ids, equirectangular like the terrain maps
/// (`u = lon / 360 + 0.5`, `v = lat / 180 + 0.5`, row 0 at the south pole).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionMapRef {
    pub offset: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyScience {
    pub body: String,
    pub situations: SituationData,
    pub regions: Vec<RegionDef>,
    pub discoverables: Vec<Discoverable>,
    pub map: Option<RegionMapRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScienceCatalog {
    pub experiments: Vec<ExperimentDef>,
    pub bodies: Vec<BodyScience>,
}

impl ScienceCatalog {
    pub fn experiment(&self, id: &str) -> Option<&ExperimentDef> {
        self.experiments
            .iter()
            .find(|e| e.id.eq_ignore_ascii_case(id))
    }

    pub fn body(&self, id: &str) -> Option<&BodyScience> {
        self.bodies.iter().find(|b| b.body.eq_ignore_ascii_case(id))
    }
}

impl BodyScience {
    /// The situation for a vessel `alt_m` above the body's radius, landed or splashed when on the surface
    /// (`GetScienceSituation`), with its multiplier; `None` beyond high orbit or where disabled.
    pub fn situation(
        &self,
        alt_m: f64,
        landed: bool,
        splashed: bool,
    ) -> Option<(ScienceSituation, f64)> {
        let s = &self.situations;
        let pick = |sit, scalar: f64| (scalar > 0.0).then_some((sit, scalar));
        if splashed {
            return pick(ScienceSituation::Splashed, s.splashed_scalar);
        }
        if landed {
            return pick(ScienceSituation::Landed, s.landed_scalar);
        }
        if s.atmosphere_scalar > 0.0 && alt_m <= s.atmosphere_max_m {
            Some((ScienceSituation::Atmosphere, s.atmosphere_scalar))
        } else if s.low_orbit_scalar > 0.0 && alt_m <= s.low_orbit_max_m {
            Some((ScienceSituation::LowOrbit, s.low_orbit_scalar))
        } else if s.high_orbit_scalar > 0.0 && alt_m <= s.high_orbit_max_m {
            Some((ScienceSituation::HighOrbit, s.high_orbit_scalar))
        } else {
            None
        }
    }

    /// The region under a body-fixed latitude/longitude (`GetScienceRegionAt`): a landmark when landed
    /// within its radius of `position_m` (body frame, Unity Y-up), else the region map.
    pub fn region_at(
        &self,
        blob: &[u8],
        lat_deg: f64,
        lon_deg: f64,
        landed_at: Option<ose_math::DVec3>,
    ) -> Option<&RegionDef> {
        if let Some(p) = landed_at {
            for d in &self.discoverables {
                if (d.position - p).length() < d.radius_m {
                    if let Some(r) = self.regions.iter().find(|r| r.id == d.region) {
                        return Some(r);
                    }
                }
            }
        }
        let m = self.map?;
        let u = (lon_deg / 360.0 + 0.5).rem_euclid(1.0);
        let v = (lat_deg / 180.0 + 0.5).clamp(0.0, 1.0);
        let x = ((u * m.width as f64).floor() as usize) % m.width;
        let y = ((v * m.height as f64).floor() as usize).min(m.height - 1);
        let id = *blob.get(m.offset + y * m.width + x)?;
        self.regions.iter().find(|r| r.map_id == id)
    }

    /// Region multiplier for a situation (`GetScienceRegionForVessel`): orbits ignore the region.
    pub fn region_scalar(region: Option<&RegionDef>, situation: ScienceSituation) -> f64 {
        match (region, situation) {
            (_, ScienceSituation::LowOrbit | ScienceSituation::HighOrbit) => 1.0,
            (Some(r), ScienceSituation::Atmosphere) => r.atmosphere_scalar,
            (Some(r), ScienceSituation::Landed) => r.landed_scalar,
            (Some(r), ScienceSituation::Splashed) => r.splashed_scalar,
            (None, _) => -1.0,
        }
    }
}

/// The report a run produces, before checking whether it was already submitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchReport {
    pub experiment: String,
    pub sample: bool,
    /// `"{Body}_{Situation}"` plus `"_{Region}"` when the experiment wants the region (`ResearchLocationId`).
    pub location: String,
    pub value: f64,
}

impl ResearchReport {
    /// `ResearchReportKey`: one submission per experiment, report type and location.
    pub fn key(&self) -> String {
        format!(
            "{}_{}_{}",
            self.experiment,
            if self.sample {
                "SampleType"
            } else {
                "DataType"
            },
            self.location
        )
    }
}

/// Reports an experiment gives at a place (`CreateScienceReports`), empty where it cannot run:
/// `value = DataValue (or SampleValue) * body scalar * situation scalar * region scalar if the region counts`.
pub fn run_experiment(
    exp: &ExperimentDef,
    body: &BodyScience,
    situation: ScienceSituation,
    situation_scalar: f64,
    region: Option<&RegionDef>,
) -> Vec<ResearchReport> {
    let region_id = region.map(|r| r.id.as_str()).unwrap_or("");
    let Some(requires_region) = exp.location(&body.body, situation, region_id) else {
        return Vec::new();
    };
    let region_scalar = BodyScience::region_scalar(region, situation);
    if situation_scalar <= 0.0 || region_scalar <= 0.0 {
        return Vec::new();
    }
    let mut scale = body.situations.body_scalar * situation_scalar;
    if requires_region {
        scale *= region_scalar;
    }
    let location = if requires_region && !region_id.is_empty() {
        format!("{}_{}_{}", body.body, situation.name(), region_id)
    } else {
        format!("{}_{}", body.body, situation.name())
    };
    let mut out = Vec::new();
    if matches!(exp.kind, ExperimentKind::Data | ExperimentKind::Both) && exp.data_value > 0.0 {
        out.push(ResearchReport {
            experiment: exp.id.clone(),
            sample: false,
            location: location.clone(),
            value: exp.data_value * scale,
        });
    }
    if matches!(exp.kind, ExperimentKind::Sample | ExperimentKind::Both) && exp.sample_value > 0.0 {
        out.push(ResearchReport {
            experiment: exp.id.clone(),
            sample: true,
            location,
            value: exp.sample_value * scale,
        });
    }
    out
}

/// An experiment a part can run (`Data_ScienceExperiment.Experiments`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartExperiment {
    pub experiment: String,
    pub crew_required: u32,
    pub time_to_complete_s: f64,
    /// Electric charge per second while it runs.
    pub electric_rate: f64,
}

/// A tech tree node (`TechNodeData`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TechNode {
    pub id: String,
    pub name_key: String,
    pub description_key: String,
    pub icon: String,
    pub required_science: u32,
    pub parts: Vec<String>,
    /// Every one must be unlocked first.
    pub requires: Vec<String>,
    /// Hidden until this node is unlocked (empty: always shown).
    pub hidden_by: String,
    pub tier: u32,
    pub position: (f64, f64),
}

impl TechNode {
    /// Free starting nodes (`IsAlwaysUnlockedTechNode`).
    pub fn always_unlocked(&self) -> bool {
        self.required_science == 0 && self.requires.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kerbin() -> BodyScience {
        BodyScience {
            body: "Kerbin".into(),
            situations: SituationData {
                high_orbit_max_m: 84_159_286.0,
                low_orbit_max_m: 150_000.0,
                atmosphere_max_m: 70_000.0,
                body_scalar: 1.0,
                high_orbit_scalar: 12.0,
                low_orbit_scalar: 6.0,
                atmosphere_scalar: 1.0,
                splashed_scalar: 1.0,
                landed_scalar: 1.0,
            },
            regions: vec![
                RegionDef {
                    id: "KerbinGrasslands".into(),
                    map_id: 0,
                    atmosphere_scalar: 1.0,
                    splashed_scalar: -1.0,
                    landed_scalar: 1.0,
                },
                RegionDef {
                    id: "KerbinDevilsTower".into(),
                    map_id: 9,
                    atmosphere_scalar: -1.0,
                    splashed_scalar: -1.0,
                    landed_scalar: 2.0,
                },
            ],
            discoverables: vec![Discoverable {
                region: "KerbinDevilsTower".into(),
                position: ose_math::DVec3::new(1000.0, 0.0, 0.0),
                radius_m: 100.0,
            }],
            map: Some(RegionMapRef {
                offset: 0,
                width: 4,
                height: 2,
            }),
        }
    }

    fn crew_observation() -> ExperimentDef {
        let at = |s, r| ValidLocation {
            body: String::new(),
            situation: Some(s),
            region: String::new(),
            requires_region: r,
        };
        ExperimentDef {
            id: "CrewObservation".into(),
            display_name_key: String::new(),
            kind: ExperimentKind::Data,
            valid: vec![
                at(ScienceSituation::HighOrbit, false),
                at(ScienceSituation::LowOrbit, false),
                at(ScienceSituation::Landed, true),
            ],
            data_value: 4.0,
            sample_value: 0.0,
            transmission_size: 5.0,
            requires_eva: false,
        }
    }

    #[test]
    fn situations_follow_the_body_altitudes() {
        let k = kerbin();
        assert_eq!(
            k.situation(0.0, true, false).unwrap().0,
            ScienceSituation::Landed
        );
        assert_eq!(
            k.situation(10_000.0, false, false).unwrap().0,
            ScienceSituation::Atmosphere
        );
        assert_eq!(
            k.situation(100_000.0, false, false).unwrap(),
            (ScienceSituation::LowOrbit, 6.0)
        );
        assert_eq!(
            k.situation(1.0e6, false, false).unwrap(),
            (ScienceSituation::HighOrbit, 12.0)
        );
        assert!(k.situation(1.0e9, false, false).is_none());
    }

    #[test]
    fn report_values_and_keys_follow_the_original() {
        let k = kerbin();
        let blob = vec![0u8; 8];
        let exp = crew_observation();
        // low orbit: 4 * 1 * 6, no region in the location
        let r = run_experiment(
            &exp,
            &k,
            ScienceSituation::LowOrbit,
            6.0,
            k.region_at(&blob, 0.0, 0.0, None),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].value, 24.0);
        assert_eq!(r[0].key(), "CrewObservation_DataType_Kerbin_LowOrbit");
        // landed at the landmark: its region and its x2
        let here = Some(ose_math::DVec3::new(1050.0, 0.0, 0.0));
        let reg = k.region_at(&blob, 0.0, 0.0, here);
        assert_eq!(reg.unwrap().id, "KerbinDevilsTower");
        let r = run_experiment(&exp, &k, ScienceSituation::Landed, 1.0, reg);
        assert_eq!(r[0].value, 8.0);
        assert_eq!(r[0].location, "Kerbin_Landed_KerbinDevilsTower");
        // not valid in the atmosphere
        assert!(run_experiment(&exp, &k, ScienceSituation::Atmosphere, 1.0, reg).is_empty());
    }
}

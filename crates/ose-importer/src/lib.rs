//! Converts the extracted KSP2 JSON (see `tools/extract_bundles.py`) into `ose_data` types.
//!
//! The original files are `{ "version": ..., "data": { ... } }` wrappers. Fields are read loosely through
//! `serde_json::Value` so that a missing or renamed field produces a coverage note instead of a hard failure.

#![forbid(unsafe_code)]

pub mod craft;
pub mod missions;
pub mod science;

use ose_data::{
    Atmosphere, AttachNode, AttachRules, CelestialBody, Curve, CurveKey, DragCube, EngineMode,
    FlowMode, GameData, Ocean, Orbit, PartDef, RecipeIngredient, ResourceContainer, ResourceDef,
    ResourceKind, BIG_G, G0, SCHEMA_VERSION,
};
use ose_math::DVec3;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Fields the converter looked at. Everything else in a source file is reported as unmapped.
#[derive(Debug, Default)]
pub struct Coverage {
    pub unmapped: BTreeMap<String, BTreeSet<String>>,
    pub warnings: Vec<String>,
    /// Informational notes that are expected (for example bodies the galaxy places at runtime).
    pub notes: Vec<String>,
}

impl Coverage {
    fn note_unmapped(&mut self, kind: &str, obj: &Value, used: &[&str]) {
        if let Some(map) = obj.as_object() {
            for k in map.keys() {
                if !used.contains(&k.as_str()) {
                    self.unmapped
                        .entry(kind.to_string())
                        .or_default()
                        .insert(k.clone());
                }
            }
        }
    }

    pub fn report(&self) -> String {
        let mut s = String::new();
        for (kind, fields) in &self.unmapped {
            s.push_str(&format!("{kind}: {} unmapped fields\n", fields.len()));
            for f in fields {
                s.push_str(&format!("  {f}\n"));
            }
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        s
    }
}

pub(crate) fn f(v: &Value, key: &str) -> f64 {
    match &v[key] {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}
pub(crate) fn b(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
pub(crate) fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}
pub(crate) fn vec3(v: &Value) -> DVec3 {
    DVec3::new(f(v, "x"), f(v, "y"), f(v, "z"))
}

/// Reads a Unity `AnimationCurve` in either of the two serialised shapes found in the data
/// (`fCurve.keys[]` with `inTangent`/`outTangent`, or `fCurve.m_Curve[]` with `inSlope`/`outSlope`).
pub fn curve(v: &Value) -> Curve {
    let fc = &v["fCurve"];
    let keys = fc["keys"].as_array().or_else(|| fc["m_Curve"].as_array());
    let mut out = Curve::default();
    if let Some(keys) = keys {
        for k in keys {
            out.keys.push(CurveKey {
                t: f(k, "time"),
                v: f(k, "value"),
                in_tangent: if k.get("inTangent").is_some() {
                    f(k, "inTangent")
                } else {
                    f(k, "inSlope")
                },
                out_tangent: if k.get("outTangent").is_some() {
                    f(k, "outTangent")
                } else {
                    f(k, "outSlope")
                },
            });
        }
    }
    out.keys
        .sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn unwrap_data(v: &Value) -> &Value {
    if v.get("data").is_some() {
        &v["data"]
    } else {
        v
    }
}

/// Galaxy definition: body id -> (parent id, orbit).
pub fn galaxy(v: &Value) -> BTreeMap<String, (Option<String>, Option<Orbit>)> {
    let mut out = BTreeMap::new();
    if let Some(bodies) = v["CelestialBodies"].as_array() {
        for body in bodies {
            let id = s(body, "GUID");
            let op = &body["OrbitProperties"];
            let parent = op["referenceBodyGuid"].as_str().map(|p| p.to_string());
            let orbit = parent.as_ref().map(|_| Orbit {
                semi_major_axis_m: f(op, "semiMajorAxis"),
                eccentricity: f(op, "eccentricity"),
                inclination_deg: f(op, "inclination"),
                longitude_of_ascending_node_deg: f(op, "longitudeOfAscendingNode"),
                argument_of_periapsis_deg: f(op, "argumentOfPeriapsis"),
                mean_anomaly_at_epoch_rad: f(op, "meanAnomalyAtEpoch"),
                epoch_s: f(op, "epoch"),
            });
            out.insert(id, (parent, orbit));
        }
    }
    out
}

pub fn celestial_body(
    v: &Value,
    parent: Option<String>,
    orbit: Option<Orbit>,
    cov: &mut Coverage,
) -> CelestialBody {
    let d = unwrap_data(v);
    let radius = f(d, "radius");
    let gravity_asl = f(d, "gravityASL");
    let mu = gravity_asl * G0 * radius * radius;
    let atmosphere = b(d, "hasAtmosphere").then(|| Atmosphere {
        depth_m: f(d, "atmosphereDepth"),
        contains_oxygen: b(d, "atmosphereContainsOxygen"),
        sea_level_pressure_kpa: f(d, "atmospherePressureSeaLevel"),
        sea_level_temperature_k: f(d, "atmosphereTemperatureSeaLevel"),
        molar_mass_kg_per_mol: f(d, "atmosphereMolarMass"),
        adiabatic_index: f(d, "atmosphereAdiabaticIndex"),
        pressure_curve: curve(&d["atmospherePressureCurve"]),
        temperature_curve: curve(&d["BodyAltitudeTemperatureCurve"]),
    });
    let used = [
        "bodyName",
        "bodyDisplayName",
        "isStar",
        "isHomeWorld",
        "radius",
        "gravityASL",
        "rotationPeriod",
        "initialRotation",
        "isTidallyLocked",
        "hasSolidSurface",
        "hasOcean",
        "oceanAltitude",
        "oceanDensity",
        "hasAtmosphere",
        "atmosphereDepth",
        "atmosphereContainsOxygen",
        "atmospherePressureSeaLevel",
        "atmosphereTemperatureSeaLevel",
        "atmosphereMolarMass",
        "atmosphereAdiabaticIndex",
        "atmospherePressureCurve",
        "BodyAltitudeTemperatureCurve",
        "StarLuminosity",
        "SphereOfInfluenceCalculationType",
    ];
    cov.note_unmapped("celestial_body", d, &used);
    CelestialBody {
        id: s(d, "bodyName"),
        display_name: s(d, "bodyDisplayName"),
        parent,
        is_star: b(d, "isStar"),
        is_home_world: b(d, "isHomeWorld"),
        radius_m: radius,
        gravity_asl,
        mu,
        mass_kg: mu / BIG_G,
        soi_radius_m: None, // filled in by `resolve_soi` once every body is known
        rotation_period_s: f(d, "rotationPeriod"),
        initial_rotation_deg: f(d, "initialRotation"),
        is_tidally_locked: b(d, "isTidallyLocked"),
        has_solid_surface: b(d, "hasSolidSurface"),
        ocean: b(d, "hasOcean").then(|| Ocean {
            altitude_m: f(d, "oceanAltitude"),
            density: f(d, "oceanDensity"),
        }),
        orbit,
        atmosphere,
        star_luminosity: b(d, "isStar").then(|| f(d, "StarLuminosity")),
    }
}

/// Sphere of influence as the original computes it: `a * (m / M_parent)^0.4`, infinite for the root.
pub fn resolve_soi(bodies: &mut [CelestialBody]) {
    let masses: BTreeMap<String, f64> = bodies.iter().map(|b| (b.id.clone(), b.mass_kg)).collect();
    for body in bodies.iter_mut() {
        body.soi_radius_m = match (&body.parent, &body.orbit) {
            (Some(p), Some(o)) => masses
                .get(p)
                .map(|pm| o.semi_major_axis_m * (body.mass_kg / pm).powf(0.4)),
            _ => None,
        };
    }
}

/// Tidally locked moons turn once per orbit. The body JSON carries a placeholder `rotationPeriod` for some
/// (100 s for the Mun, Tylo and Vall), which would make their equators move at kilometres per second; the
/// original keeps them facing their parent, so the period is the orbital period `2 pi sqrt(a^3 / mu_parent)`.
pub fn resolve_tidal_lock(bodies: &mut [CelestialBody]) {
    let mus: BTreeMap<String, f64> = bodies.iter().map(|b| (b.id.clone(), b.mu)).collect();
    for body in bodies.iter_mut().filter(|b| b.is_tidally_locked) {
        if let (Some(p), Some(o)) = (&body.parent, &body.orbit) {
            if let Some(mu) = mus.get(p).filter(|m| **m > 0.0) {
                body.rotation_period_s =
                    std::f64::consts::TAU * (o.semi_major_axis_m.powi(3) / mu).sqrt();
            }
        }
    }
}

pub fn resource(v: &Value, cov: &mut Coverage) -> ResourceDef {
    if v["isRecipe"].as_bool().unwrap_or(false) {
        let r = &v["recipeData"];
        let ingredients = r["ingredients"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| RecipeIngredient {
                        resource: s(i, "name"),
                        units_per_recipe_unit: f(i, "unitsPerRecipeUnit"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        cov.note_unmapped(
            "resource_recipe",
            r,
            &["name", "displayNameKey", "abbreviationKey", "ingredients"],
        );
        return ResourceDef {
            id: s(r, "name"),
            display_name_key: s(r, "displayNameKey"),
            abbreviation_key: s(r, "abbreviationKey"),
            kind: ResourceKind::Recipe { ingredients },
        };
    }
    let d = unwrap_data(v);
    cov.note_unmapped(
        "resource",
        d,
        &[
            "name",
            "displayNameKey",
            "abbreviationKey",
            "massPerUnit",
            "volumePerUnit",
            "specificHeatCapacityPerUnit",
            "flowMode",
            "transferMode",
            "costPerUnit",
            "NonStageable",
        ],
    );
    ResourceDef {
        id: s(d, "name"),
        display_name_key: s(d, "displayNameKey"),
        abbreviation_key: s(d, "abbreviationKey"),
        kind: ResourceKind::Basic {
            mass_per_unit_t: f(d, "massPerUnit"),
            volume_per_unit: f(d, "volumePerUnit"),
            specific_heat_capacity_per_unit: f(d, "specificHeatCapacityPerUnit"),
            flow_mode: FlowMode::from_code(d["flowMode"].as_i64().unwrap_or(0)),
            pumpable: d["transferMode"].as_i64().unwrap_or(0) == 1,
            cost_per_unit: f(d, "costPerUnit"),
            non_stageable: b(d, "NonStageable"),
        },
    }
}

fn module_data(m: &Value) -> &Value {
    &m["ModuleData"][0]["DataObject"]
}

pub fn part(v: &Value, cov: &mut Coverage) -> PartDef {
    let d = unwrap_data(v);
    let ar = &d["attachRules"];
    let attach_rules = AttachRules {
        stack: b(ar, "stack"),
        srf_attach: b(ar, "srfAttach"),
        allow_stack: b(ar, "allowStack"),
        allow_srf_attach: b(ar, "allowSrfAttach"),
        allow_collision: b(ar, "allowCollision"),
        allow_dock: b(ar, "allowDock"),
        allow_rotate: b(ar, "allowRotate"),
        allow_root: b(ar, "allowRoot"),
    };
    let attach_nodes: Vec<AttachNode> = d["attachNodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|n| AttachNode {
                    id: s(n, "nodeID"),
                    node_type: s(n, "nodeType"),
                    attach_method: s(n, "attachMethod"),
                    position: vec3(&n["position"]),
                    orientation: vec3(&n["orientation"]),
                    size: n["size"].as_i64().unwrap_or(0) as i32,
                    is_resource_crossfeed: b(n, "isResourceCrossfeed"),
                    is_rigid: b(n, "isRigid"),
                    angular_strength_multiplier: f(n, "angularStrengthMultiplier"),
                })
                .collect()
        })
        .unwrap_or_default();
    let resource_containers = d["resourceContainers"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| ResourceContainer {
                    resource: s(c, "name"),
                    capacity_units: f(c, "capacityUnits"),
                    initial_units: f(c, "initialUnits"),
                })
                .collect()
        })
        .unwrap_or_default();

    let mut engines = Vec::new();
    let mut drag_cubes = Vec::new();
    let mut module_names = Vec::new();
    let mut raw_modules = BTreeMap::new();
    let mut gimbal_range = None;
    let mut wheel_torque: f64 = 0.0;
    let mut electric = ose_data::Electric::default();
    let mut experiments = Vec::new();
    let mut docking = None;
    if let Some(mods) = d["serializedPartModules"].as_array() {
        for m in mods {
            if let Some(entries) = m["ModuleData"].as_array() {
                for entry in entries.iter().filter(|x| x["Name"] == "Data_DockingNode") {
                    let o = &entry["DataObject"];
                    docking = Some(ose_data::DockingPort {
                        node: s(o, "AttachmentNodeId"),
                        sizes: o["NodeTypes"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        acquire_range_m: f(o, "AcquireRange"),
                        acquire_min_fwd_dot: f(o, "AcquireMinFwdDot"),
                        capture_range_m: f(o, "CaptureRange"),
                        capture_min_fwd_dot: f(o, "CaptureMinFwdDot"),
                        capture_max_rvel_mps: f(o, "CaptureMaxRvel"),
                        undock_ejection_kn: f(o, "UndockEjectionForce"),
                        min_distance_to_reengage_m: f(o, "MinDistanceToReengage"),
                    });
                }
            }
            electric_from_module(m, &mut electric);
            science::part_experiments(m, &mut experiments);
            let name = s(m, "Name");
            let data = module_data(m);
            match name.as_str() {
                "PartComponentModule_Engine" => {
                    if let Some(modes) = data["engineModes"].as_array() {
                        for em in modes {
                            engines.push(EngineMode {
                                id: s(em, "engineID"),
                                engine_type: s(em, "engineType"),
                                propellant_mixture: s(&em["propellant"], "mixtureName"),
                                min_thrust_kn: f(em, "minThrust"),
                                max_thrust_kn: f(em, "maxThrust"),
                                isp_curve: curve(&em["atmosphereCurve"]),
                                throttle_locked: b(em, "throttleLocked"),
                                allow_restart: b(em, "allowRestart"),
                                allow_shutdown: b(em, "allowShutdown"),
                                gimbal_range_deg: None,
                            });
                        }
                    }
                }
                "PartComponentModule_Gimbal" => gimbal_range = Some(f(data, "gimbalRange")),
                "PartComponentModule_ReactionWheel" => {
                    wheel_torque = f(data, "PitchTorque").max(f(data, "YawTorque"));
                }
                "PartComponentModule_Drag" => {
                    if let Some(cubes) = data["cubes"].as_array() {
                        for c in cubes {
                            let arr6 = |k: &str| {
                                let mut out = [0.0; 6];
                                if let Some(a) = c[k].as_array() {
                                    for (i, x) in a.iter().take(6).enumerate() {
                                        out[i] = x.as_f64().unwrap_or(0.0);
                                    }
                                }
                                out
                            };
                            drag_cubes.push(DragCube {
                                name: s(c, "Name"),
                                area: arr6("Area"),
                                drag: arr6("Drag"),
                                depth: arr6("Depth"),
                                center: vec3(&c["Center"]),
                                size: vec3(&c["Size"]),
                            });
                        }
                    }
                }
                _ => {
                    raw_modules.insert(name.clone(), data.clone());
                }
            }
            module_names.push(name);
        }
    }
    for e in &mut engines {
        e.gimbal_range_deg = gimbal_range;
    }
    if d["mass"].as_f64().is_none() {
        cov.warnings
            .push(format!("part {} has no mass", s(d, "partName")));
    }
    cov.note_unmapped(
        "part",
        d,
        &[
            "partName",
            "Title",
            "category",
            "family",
            "sizeCategory",
            "stageType",
            "mass",
            "cost",
            "crewCapacity",
            "maxTemp",
            "crashTolerance",
            "breakingForce",
            "breakingTorque",
            "fuelCrossFeed",
            "physicsMode",
            "attachRules",
            "attachNodes",
            "resourceContainers",
            "serializedPartModules",
        ],
    );
    // the stowed shape first (parachutes list DEPLOYED first, gear and panels RETRACTED/EXTENDED): it is the
    // part's default drag and extent
    if let Some(i) = drag_cubes
        .iter()
        .position(|c| matches!(c.name.as_str(), "PACKED" | "RETRACTED"))
    {
        let stowed = drag_cubes.remove(i);
        drag_cubes.insert(0, stowed);
    }
    let bounds = part_bounds(&drag_cubes, &attach_nodes);
    PartDef {
        id: s(d, "partName"),
        title_key: s(d, "Title"),
        category: s(d, "category"),
        family: s(d, "family"),
        size_category: s(d, "sizeCategory"),
        stage_type: s(d, "stageType"),
        mass_t: f(d, "mass"),
        cost: f(d, "cost"),
        crew_capacity: d["crewCapacity"].as_u64().unwrap_or(0) as u32,
        max_temp_k: f(d, "maxTemp"),
        crash_tolerance: f(d, "crashTolerance"),
        breaking_force: f(d, "breakingForce"),
        breaking_torque: f(d, "breakingTorque"),
        fuel_cross_feed: b(d, "fuelCrossFeed"),
        physics_mode: s(d, "physicsMode"),
        attach_rules,
        attach_nodes,
        resource_containers,
        engines,
        bounds,
        reaction_wheel_torque_knm: wheel_torque,
        electric,
        experiments,
        docking,
        drag_cubes,
        module_names,
        raw_modules,
    }
}

/// Electric charge rate of a module resource setting (`{ResourceName, Rate}`), 0 for other resources.
fn ec_rate(setting: &Value) -> f64 {
    if setting["ResourceName"].as_str() == Some("ElectricCharge") {
        f(setting, "Rate")
    } else {
        0.0
    }
}

/// Pick the electric charge figures out of every data entry of one serialized module.
fn electric_from_module(m: &Value, e: &mut ose_data::Electric) {
    let Some(entries) = m["ModuleData"].as_array() else {
        return;
    };
    for entry in entries {
        let o = &entry["DataObject"];
        match entry["Name"].as_str().unwrap_or("") {
            "Data_Command" => {
                if let Some(r) = o["requiredResources"].as_array() {
                    e.command_use += r.iter().map(ec_rate).sum::<f64>();
                }
            }
            "Data_ReactionWheel" => {
                if let Some(r) = o["RequiredResources"].as_array() {
                    e.wheel_use_per_knm = r.iter().map(ec_rate).sum::<f64>();
                }
                let t = |k: &str| f(o, k);
                e.wheel_torque_vec_knm =
                    (t("PitchTorque").powi(2) + t("YawTorque").powi(2) + t("RollTorque").powi(2))
                        .sqrt();
            }
            "Data_SolarPanel" => e.solar_output += ec_rate(&o["ResourceSettings"]),
            "Data_Deployable" => {
                if o["isTracking"].as_bool() == Some(true) {
                    e.solar_tracking = true;
                }
            }
            "Data_ModuleGenerator" => e.generator_output += ec_rate(&o["ResourceSetting"]),
            _ => {}
        }
    }
}

/// First drag cube's size and centre, else the attach-node bounding box padded to at least 0.1 m, else None.
fn part_bounds(cubes: &[DragCube], nodes: &[AttachNode]) -> Option<ose_data::PartBounds> {
    if let Some(c) = cubes.first() {
        if c.size.length() > 0.0 {
            return Some(ose_data::PartBounds {
                size_m: c.size,
                center_m: c.center,
            });
        }
    }
    if nodes.is_empty() {
        return None;
    }
    let mut lo = DVec3::new(f64::MAX, f64::MAX, f64::MAX);
    let mut hi = DVec3::new(f64::MIN, f64::MIN, f64::MIN);
    for n in nodes {
        lo = DVec3::new(
            lo.x.min(n.position.x),
            lo.y.min(n.position.y),
            lo.z.min(n.position.z),
        );
        hi = DVec3::new(
            hi.x.max(n.position.x),
            hi.y.max(n.position.y),
            hi.z.max(n.position.z),
        );
    }
    let size = DVec3::new(
        (hi.x - lo.x).max(0.1),
        (hi.y - lo.y).max(0.1),
        (hi.z - lo.z).max(0.1),
    );
    Some(ose_data::PartBounds {
        size_m: size,
        center_m: (lo + hi) * 0.5,
    })
}

/// The original files were written for Newtonsoft and sometimes carry trailing commas; fall back to json5.
pub(crate) fn read_json(path: &Path) -> anyhow::Result<Value> {
    let text = std::fs::read_to_string(path)?;
    match serde_json::from_str(&text) {
        Ok(v) => Ok(v),
        Err(e) => json5::from_str(&text)
            .map_err(|e5| anyhow::anyhow!("{}: {e}; json5: {e5}", path.display())),
    }
}

pub(crate) fn json_files(dir: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    if dir.is_dir() {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json") {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Converts an extracted data directory (`assets/ksp2/data`) into a `GameData`.
pub fn import_dir(data_dir: &Path, cov: &mut Coverage) -> anyhow::Result<GameData> {
    let gal = galaxy(&read_json(&data_dir.join("GalaxyDefinition_Default.json"))?);
    let mut bodies = Vec::new();
    for p in json_files(&data_dir.join("celestialbody_json"))? {
        let v = read_json(&p)?;
        let id = s(unwrap_data(&v), "bodyName");
        let (parent, orbit) = gal.get(&id).cloned().unwrap_or((None, None));
        if !gal.contains_key(&id) {
            cov.notes.push(format!(
                "body {id} is not placed by the galaxy definition (spawned at runtime)"
            ));
        }
        bodies.push(celestial_body(&v, parent, orbit, cov));
    }
    resolve_soi(&mut bodies);
    resolve_tidal_lock(&mut bodies);

    let mut resources = Vec::new();
    for p in json_files(&data_dir.join("resources-json"))? {
        resources.push(resource(&read_json(&p)?, cov));
    }
    let mut parts = Vec::new();
    for p in json_files(&data_dir.join("parts"))? {
        parts.push(part(&read_json(&p)?, cov));
    }
    Ok(GameData {
        schema_version: SCHEMA_VERSION,
        bodies,
        resources,
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_mu_and_soi_match_original_formulas() {
        let kerbol = json!({"data": {"bodyName": "Kerbol", "isStar": true, "radius": 261600000.0, "gravityASL": 1.7467}});
        let kerbin = json!({"data": {"bodyName": "Kerbin", "radius": 600000.0, "gravityASL": 1.00034160493135,
            "rotationPeriod": 21549.425, "hasAtmosphere": true, "atmosphereDepth": 70000.0,
            "atmospherePressureSeaLevel": 101.325, "atmospherePressureCurve": {"fCurve": {"m_Curve": [
                {"time": 0.0, "value": 101.325, "inSlope": 0.0, "outSlope": -0.015},
                {"time": 70000.0, "value": 0.0, "inSlope": 0.0, "outSlope": 0.0}]}}}});
        let mut cov = Coverage::default();
        let orbit = Orbit {
            semi_major_axis_m: 13_599_840_256.0,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            longitude_of_ascending_node_deg: 0.0,
            argument_of_periapsis_deg: 0.0,
            mean_anomaly_at_epoch_rad: 3.0,
            epoch_s: 0.0,
        };
        let mut bodies = vec![
            celestial_body(&kerbol, None, None, &mut cov),
            celestial_body(&kerbin, Some("Kerbol".into()), Some(orbit), &mut cov),
        ];
        resolve_soi(&mut bodies);
        let k = &bodies[1];
        // KSP1/2 Kerbin: mu ~ 3.5316e12, SOI ~ 84,159 km.
        assert!((k.mu - 3.5316e12).abs() / 3.5316e12 < 1e-3, "mu = {}", k.mu);
        let soi = k.soi_radius_m.unwrap();
        assert!(
            (soi - 84_159_286.0).abs() / 84_159_286.0 < 0.01,
            "soi = {soi}"
        );
        assert!(bodies[0].soi_radius_m.is_none());
        let atm = k.atmosphere.as_ref().unwrap();
        assert_eq!(atm.pressure_curve.keys.len(), 2);
        assert_eq!(atm.pressure_curve.evaluate(0.0), 101.325);
    }

    #[test]
    fn galaxy_orbits_parse_numeric_strings() {
        let g = json!({"CelestialBodies": [
            {"GUID": "Kerbol", "OrbitProperties": {}},
            {"GUID": "Moho", "OrbitProperties": {"referenceBodyGuid": "Kerbol", "inclination": "7",
              "eccentricity": "0.2", "semiMajorAxis": "5263138304", "longitudeOfAscendingNode": "70",
              "argumentOfPeriapsis": "15", "meanAnomalyAtEpoch": "3.14", "epoch": "0"}}]});
        let gal = galaxy(&g);
        assert_eq!(gal["Kerbol"], (None, None));
        let (p, o) = &gal["Moho"];
        assert_eq!(p.as_deref(), Some("Kerbol"));
        assert_eq!(o.as_ref().unwrap().semi_major_axis_m, 5_263_138_304.0);
    }

    #[test]
    fn engine_part_maps_thrust_isp_and_gimbal() {
        let p = json!({"data": {"partName": "engine_x", "category": "Engine", "mass": 0.5, "attachRules": {"stack": true},
        "attachNodes": [{"nodeID": "top", "nodeType": "Stack", "position": {"x": 0, "y": 0.264, "z": 0},
                         "orientation": {"x": 0, "y": 1, "z": 0}, "size": 1, "isResourceCrossfeed": true}],
        "serializedPartModules": [
          {"Name": "PartComponentModule_Engine", "ModuleData": [{"DataObject": {"engineModes": [
             {"engineID": "Engine", "engineType": "Methalox", "minThrust": 0.0, "maxThrust": 60.0,
              "propellant": {"mixtureName": "Methalox"}, "allowRestart": true,
              "atmosphereCurve": {"fCurve": {"keys": [
                 {"time": 0.0, "value": 335.0, "inTangent": 0, "outTangent": 0},
                 {"time": 1.0, "value": 170.0, "inTangent": 0, "outTangent": 0}]}}}]}}]},
          {"Name": "PartComponentModule_Gimbal", "ModuleData": [{"DataObject": {"gimbalRange": 4.0}}]},
          {"Name": "PartComponentModule_Color", "ModuleData": [{"DataObject": {"Base": 1}}]}
        ]}});
        let mut cov = Coverage::default();
        let part = part(&p, &mut cov);
        assert_eq!(part.engines.len(), 1);
        let e = &part.engines[0];
        assert_eq!(e.max_thrust_kn, 60.0);
        assert_eq!(e.isp_curve.evaluate(0.0), 335.0);
        assert_eq!(e.isp_curve.evaluate(1.0), 170.0);
        assert_eq!(e.gimbal_range_deg, Some(4.0));
        assert_eq!(part.attach_nodes[0].position.y, 0.264);
        assert_eq!(part.module_names.len(), 3);
        assert!(part.raw_modules.contains_key("PartComponentModule_Color"));
    }

    #[test]
    fn tidally_locked_moons_turn_once_per_orbit() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/own/kerbol-system.json");
        let bodies: Vec<CelestialBody> =
            serde_json::from_str(&std::fs::read_to_string(data).unwrap()).unwrap();
        let mun = bodies.iter().find(|b| b.id == "Mun").unwrap();
        let kerbin = bodies.iter().find(|b| b.id == "Kerbin").unwrap();
        let a = mun.orbit.as_ref().unwrap().semi_major_axis_m;
        let orbital = std::f64::consts::TAU * (a.powi(3) / kerbin.mu).sqrt();
        assert!(
            (mun.rotation_period_s - orbital).abs() < 1.0,
            "{}",
            mun.rotation_period_s
        );
        // the Mun's equator then moves at about 9 m/s, not kilometres per second
        let v = std::f64::consts::TAU * mun.radius_m / mun.rotation_period_s;
        assert!(v < 20.0, "{v}");
        for b in bodies.iter().filter(|b| b.is_tidally_locked) {
            assert!(
                b.rotation_period_s > 10_000.0,
                "{} {}",
                b.id,
                b.rotation_period_s
            );
        }
    }
}

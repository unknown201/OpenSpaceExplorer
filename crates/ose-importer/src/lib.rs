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

/// Attach prefab-derived geometry to the corresponding typed engine modes, rejecting stale exports.
pub fn apply_engine_geometry(
    parts: &mut [PartDef],
    geometry: &BTreeMap<String, Vec<Vec<ose_data::ThrustTransform>>>,
) -> Result<(), String> {
    for (id, modes) in geometry {
        let part = parts
            .iter_mut()
            .find(|p| &p.id == id)
            .ok_or_else(|| format!("Unknown geometry part: {id}"))?;
        if part.engines.len() != modes.len() {
            return Err(format!("Engine mode count changed for {id}"));
        }
        for (engine, points) in part.engines.iter_mut().zip(modes) {
            if points.is_empty()
                || points.iter().any(|p| {
                    !p.weight.is_finite()
                        || p.weight < 0.0
                        || !p.position_m.length_squared().is_finite()
                        || !p.direction.length().is_finite()
                        || (p.direction.length() - 1.0).abs() > 1e-6
                })
            {
                return Err(format!("Invalid thrust geometry for {id}"));
            }
            engine.thrust_transforms = points.clone();
        }
    }
    Ok(())
}

pub fn apply_rcs_geometry(
    parts: &mut [PartDef],
    geometry: &BTreeMap<String, Vec<ose_data::ThrustTransform>>,
) -> Result<(), String> {
    for (id, points) in geometry {
        let rcs = parts
            .iter_mut()
            .find(|p| &p.id == id)
            .and_then(|p| p.rcs.as_mut())
            .ok_or_else(|| format!("Unknown RCS geometry part: {id}"))?;
        if points.is_empty()
            || points.iter().any(|p| {
                (p.weight - 1.0).abs() > 1e-12
                    || !p.weight.is_finite()
                    || !p.position_m.length_squared().is_finite()
                    || !p.direction.length().is_finite()
                    || (p.direction.length() - 1.0).abs() > 1e-6
            })
        {
            return Err(format!("Invalid RCS geometry for {id}"));
        }
        rcs.thrusters = points.clone();
    }
    Ok(())
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct LiftingGeometry {
    pub transform_name: String,
    pub resolved: bool,
    pub frame: ose_data::SurfaceFrame,
}

/// Attach a static prefab frame without overwriting module-local axes or pressure offsets.
pub fn apply_lifting_geometry(
    parts: &mut [PartDef],
    geometry: &BTreeMap<String, LiftingGeometry>,
) -> Result<(), String> {
    for (id, exported) in geometry {
        let surface = parts
            .iter_mut()
            .find(|p| &p.id == id)
            .and_then(|p| p.lifting_surface.as_mut())
            .ok_or_else(|| format!("Unknown lifting geometry part: {id}"))?;
        if surface.transform_name != exported.transform_name {
            return Err(format!("Stale lifting transform name for {id}"));
        }
        let frame = &exported.frame;
        let [x, y, z] = frame.direction_axes;
        let determinant = frame.point_axes[0]
            .cross(frame.point_axes[1])
            .dot(frame.point_axes[2]);
        if !frame.position_m.length_squared().is_finite()
            || frame
                .point_axes
                .iter()
                .any(|v| !v.length_squared().is_finite())
            || !determinant.is_finite()
            || determinant == 0.0
            || frame
                .direction_axes
                .iter()
                .any(|v| !v.length_squared().is_finite() || (v.length_squared() - 1.0).abs() > 1e-6)
            || x.dot(y).abs() > 1e-6
            || x.dot(z).abs() > 1e-6
            || y.dot(z).abs() > 1e-6
            || (x.cross(y).dot(z) - 1.0).abs() > 1e-6
        {
            return Err(format!("Invalid lifting frame for {id}"));
        }
        surface.reference_frame = Some(frame.clone());
    }
    Ok(())
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
                    full_break_strength: b(n, "MultiJointFullBreakStrength"),
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
    let mut drag_properties = ose_data::DragProperties::default();
    let mut module_names = Vec::new();
    let mut raw_modules = BTreeMap::new();
    let mut gimbal_range = None;
    let mut wheel_torque: f64 = 0.0;
    let mut wheel_axes = None;
    let mut electric = ose_data::Electric::default();
    let mut experiments = Vec::new();
    let mut docking = None;
    let mut chute: Option<ose_data::Parachute> = None;
    let mut heatshield: Option<ose_data::Heatshield> = None;
    let mut wheel: Option<(bool, f64)> = None;
    let mut deployable: Option<(bool, bool)> = None;
    let mut wheel_radius = 0.0;
    let mut motor: Option<ose_data::WheelMotor> = None;
    let mut brake_torque = 0.0;
    let mut steerable = false;
    let mut converter: Option<ose_data::Converter> = None;
    let mut ejection_force = 0.0;
    let mut fairing = false;
    let mut cargo_bay = false;
    let mut rcs: Option<ose_data::Rcs> = None;
    let mut jetpack = None;
    let mut lifting_surface: Option<ose_data::LiftingSurface> = None;
    let mut intake_area_m2 = 0.0;
    let mut ctrl = (0.0, 0.0);
    let mut ctrl_pressure_offset_m = DVec3::ZERO;
    if let Some(mods) = d["serializedPartModules"].as_array() {
        for m in mods {
            if let Some(entries) = m["ModuleData"].as_array() {
                for entry in entries.iter() {
                    let o = &entry["DataObject"];
                    if entry["Name"] == "Data_Drag" {
                        drag_properties = ose_data::DragProperties {
                            drag_enabled: o["DragEnabled"].as_bool().unwrap_or(true),
                            body_lift_enabled: o["BodyLiftEnabled"].as_bool().unwrap_or(true),
                            body_lift_multiplier: o["bodyLiftMultiplier"].as_f64().unwrap_or(1.0),
                        };
                    }
                    if entry["Name"] == "Data_LiftingSurface" {
                        let sign = match f(o, "transformSign") {
                            0.0 => 1.0,
                            x => x,
                        };
                        let axis = match o["transformDir"].as_str() {
                            Some("Z") => DVec3::new(0.0, 0.0, sign),
                            Some("Y") => DVec3::new(0.0, sign, 0.0),
                            _ => DVec3::new(sign, 0.0, 0.0),
                        };
                        lifting_surface = Some(ose_data::LiftingSurface {
                            transform_name: s(o, "transformName"),
                            reference_frame: None,
                            node_enabled: b(o, "nodeEnabled"),
                            attach_node_name: s(o, "attachNodeName"),
                            pressure_offset_m: vec3(&o["CoPOffset"]),
                            perpendicular_only: b(o, "perpendicularOnly"),
                            use_internal_drag_model: o["useInternalDragModel"]
                                .as_bool()
                                .unwrap_or(true),
                            deflection_lift_coeff: f(o, "deflectionLiftCoeff"),
                            axis,
                            omnidirectional: b(o, "omnidirectional"),
                            curve_set: s(o, "liftingSurfaceCurve"),
                            lift_curve: curve(&o["liftCurve"]),
                            lift_mach_curve: curve(&o["liftMachCurve"]),
                            drag_curve: curve(&o["dragCurve"]),
                            drag_mach_curve: curve(&o["dragMachCurve"]),
                        });
                    }
                    if entry["Name"] == "Data_ControlSurface" {
                        ctrl_pressure_offset_m = vec3(&o["CoPressureCtrlSurfaceOffset"]);
                        ctrl = (
                            f(o, "CtrlSurfaceRange"),
                            if b(o, "DisableLiftingSurfaceForce") {
                                1.0
                            } else {
                                f(o, "CtrlSurfaceArea").clamp(0.0, 1.0)
                            },
                        );
                    }
                    if entry["Name"] == "Data_ResourceIntake" {
                        intake_area_m2 = f(o, "area");
                    }
                    if entry["Name"] == "Data_Kerbal" {
                        jetpack = Some(ose_data::Jetpack {
                            move_force_kn: f(o, "JetpackMoveForce"),
                            rotate_torque_knm: f(o, "JetpackRotateTorque"),
                            rotate_offset_m: f(o, "JetpackRotateTorqueOffset"),
                            rotate_max_speed_deg_s: f(o, "JetpackRotateMaxAngularSpeed"),
                            reset_enabled: b(o, "JetpackRotateResetEnabled"),
                            reset_speed_deg_s: f(o, "JetpackRotateResetAngularSpeed"),
                            reset_threshold_deg_s: f(o, "JetpackRotateResetAngularSpeedThreshold"),
                            propellant: s(&o["JetpackPropellantDefinition"], "mixtureName"),
                            isp_curve: curve(&o["JetpackAtmosphereCurve"]),
                        });
                    }
                    if entry["Name"] == "Data_RCS" {
                        rcs = Some(ose_data::Rcs {
                            request_minimum: Some(0.5),
                            controls: ose_data::RcsControls {
                                pitch: o["enablePitch"]["storedValue"].as_bool().unwrap_or(true),
                                yaw: o["enableYaw"]["storedValue"].as_bool().unwrap_or(true),
                                roll: o["enableRoll"]["storedValue"].as_bool().unwrap_or(true),
                                x: o["enableX"]["storedValue"].as_bool().unwrap_or(true),
                                y: o["enableY"]["storedValue"].as_bool().unwrap_or(true),
                                z: o["enableZ"]["storedValue"].as_bool().unwrap_or(true),
                                fore_by_throttle: b(&o["foreByThrottle"], "storedValue"),
                                full_thrust: b(&o["fullThrust"], "storedValue"),
                                full_thrust_min: o["fullThrustMin"].as_f64().unwrap_or(0.2),
                            },
                            enabled: o["isEnabled"]["storedValue"].as_bool().unwrap_or(true),
                            thrust_fraction: (o["thrustPercentage"]["storedValue"]
                                .as_f64()
                                .unwrap_or(100.0)
                                / 100.0)
                                .clamp(0.0, 1.0),
                            shielded_can_activate: b(o, "shieldedCanActivate"),
                            thrusters: Vec::new(),
                            thrust_kn: f(o, "maxThrust"),
                            propellant: s(&o["Propellant"], "mixtureName"),
                            isp_curve: curve(&o["atmosphereCurve"]),
                        });
                    }
                    // a fairing part of its own (engines carry Data_Fairing for their shrouds too)
                    if entry["Name"] == "Data_Fairing" && d["stageType"] == "Fairing" {
                        fairing = true;
                    }
                    if entry["Name"] == "Data_CargoBay" {
                        cargo_bay = true;
                    }
                    if entry["Name"] == "Data_Decouple" {
                        ejection_force = f(o, "ejectionForce");
                    }
                    if entry["Name"] == "Data_ResourceConverter" {
                        let pick = o["SelectedFormula"].as_u64().unwrap_or(0) as usize;
                        if let Some(fm) =
                            o["FormulaDefinitions"].as_array().and_then(|a| a.get(pick))
                        {
                            let list = |k: &str| -> Vec<(String, f64)> {
                                fm[k]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .map(|r| (s(r, "ResourceName"), f(r, "Rate")))
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            };
                            converter = Some(ose_data::Converter {
                                formula: s(fm, "InternalName"),
                                inputs: list("InputResources"),
                                outputs: list("OutputResources"),
                                conversion_rate: f(&o["conversionRate"], "storedValue"),
                            });
                        }
                    }
                    if entry["Name"] == "Data_WheelMotor" {
                        motor = Some(ose_data::WheelMotor {
                            torque_curve: curve(&o["torqueCurve"]),
                            ec_per_s: f(&o["requiredResource"], "Rate"),
                        });
                    }
                    if entry["Name"] == "Data_WheelBrakes" {
                        brake_torque = f(o, "MaxBrakeTorque");
                    }
                    if entry["Name"] == "Data_WheelSteering" {
                        steerable = true;
                    }
                    if entry["Name"] == "Data_WheelBase" {
                        wheel_radius = f(o, "Radius");
                        let leg = o["WheelType"] == "LEG";
                        wheel = Some((leg, wheel.map(|w| w.1).unwrap_or(0.0)));
                    }
                    if entry["Name"] == "Data_WheelSuspension" {
                        let leg = wheel.map(|w| w.0).unwrap_or(false);
                        wheel = Some((leg, f(o, "suspensionDistance")));
                    }
                    if entry["Name"] == "Data_Deployable" {
                        let state = o["CurrentDeployState"]["storedValue"]
                            .as_str()
                            .unwrap_or("");
                        deployable = Some((b(o, "retractable"), state == "Extended"));
                    }
                }
                for entry in entries.iter().filter(|x| x["Name"] == "Data_Heatshield") {
                    let o = &entry["DataObject"];
                    let res = &o["requiredResources"][0];
                    heatshield = Some(ose_data::Heatshield {
                        ablation_temp_k: f(o, "AblationTempThreshold"),
                        ablation_max_over_k: f(o, "AblationMaximumOverThreshold"),
                        pyrolysis_loss_factor: f(o, "PyrolysisLossFactor"),
                        shielding_scale: f(o, "ShieldingScale"),
                        shielding_direction: vec3(&o["ShieldingDirection"]),
                        ablator: s(res, "ResourceName"),
                        ablator_rate: f(res, "Rate"),
                    });
                }
                for entry in entries.iter().filter(|x| x["Name"] == "Data_Parachute") {
                    let o = &entry["DataObject"];
                    chute = Some(ose_data::Parachute {
                        min_pressure_atm: f(o, "defaultMinAirPressureToOpen"),
                        deploy_altitude_m: f(o, "defaultDeployAltitude"),
                        auto_cut_speed_mps: f(o, "autoCutSpeed"),
                        semi_deploy_rate: f(o, "semiDeploymentSpeed"),
                        deploy_rate: f(o, "deploymentSpeed"),
                        semi_cube: None,
                        deployed_cube: None,
                        safety: chute_safety(o),
                        rotation_speed_dps: o["rotationSpeedDPS"].as_f64().unwrap_or(90.0),
                        spread_angle_deg: o["spreadAngle"]["storedValue"].as_f64().unwrap_or(7.0),
                        shielded_can_deploy: b(o, "shieldedCanDeploy"),
                        min_speed_to_deploy: o["minimumSpeedToDeploy"].as_f64().unwrap_or(1.0),
                    });
                }
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
                        acquire_force_kn: f(o, "AcquireForce"),
                        acquire_torque_knm: f(o, "AcquireTorque"),
                        acquire_torque_roll_knm: f(o, "AcquireTorqueRoll"),
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
                                thrust_transforms: Vec::new(),
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
                                air_breathing: em["propellant"]["ingredientOverrides"]
                                    .as_array()
                                    .is_some_and(|a| a.iter().any(|i| i["name"] == "IntakeAir")),
                                vel_curve: b(em, "useVelCurve").then(|| curve(&em["velCurve"])),
                                atm_curve: b(em, "useAtmCurve").then(|| curve(&em["atmCurve"])),
                            });
                        }
                    }
                }
                "PartComponentModule_Gimbal" => gimbal_range = Some(f(data, "gimbalRange")),
                "PartComponentModule_ReactionWheel" => {
                    wheel_torque = f(data, "PitchTorque").max(f(data, "YawTorque"));
                    wheel_axes = Some(ose_math::DVec3::new(
                        f(data, "PitchTorque"),
                        f(data, "RollTorque"),
                        f(data, "YawTorque"),
                    ));
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
                                drag_multiplier: if c["DragMultiplier"].is_array() {
                                    arr6("DragMultiplier")
                                } else {
                                    [1.0; 6]
                                },
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
            "thermalMassModifier",
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
    if let Some(c) = chute.as_mut() {
        c.semi_cube = drag_cubes
            .iter()
            .find(|d| d.name == "SEMIDEPLOYED")
            .cloned();
        c.deployed_cube = drag_cubes.iter().find(|d| d.name == "DEPLOYED").cloned();
    }
    let parachute = chute;
    let landing_gear = wheel.map(|(leg, suspension_m)| {
        let extended =
            drag_cubes
                .iter()
                .find(|d| d.name == "EXTENDED")
                .map(|c| ose_data::PartBounds {
                    size_m: c.size,
                    center_m: c.center,
                });
        ose_data::LandingGear {
            leg,
            suspension_m,
            retractable: deployable.is_some_and(|d| d.0) && extended.is_some(),
            starts_extended: deployable.is_none_or(|d| d.1) || extended.is_none(),
            extended: if deployable.is_some() { extended } else { None },
            radius_m: wheel_radius,
            motor: motor.clone(),
            brake_torque_knm: brake_torque,
            steerable,
        }
    });
    let bounds = part_bounds(&drag_cubes, &attach_nodes);
    // Data_Deployable is shared by legs, antennae and bays too. Its settings describe solar
    // behavior only when this part actually contains a solar producer.
    if electric.solar_output <= 0.0 {
        electric.solar_deployable = false;
        electric.solar_tracking = false;
    }
    PartDef {
        flight_geometry: ose_data::flight_geometry::FlightGeometry {
            mass_offset_m: vec3(&d["coMassOffset"]),
            pressure_offset_m: vec3(&d["coPressureOffset"]),
            lift_offset_m: vec3(&d["coLiftOffset"]),
            ..Default::default()
        },
        id: s(d, "partName"),
        title_key: s(d, "Title"),
        category: s(d, "category"),
        family: s(d, "family"),
        size_category: s(d, "sizeCategory"),
        stage_type: s(d, "stageType"),
        mass_t: f(d, "mass"),
        mass_offset_m: vec3(&d["coMassOffset"]),
        pressure_offset_m: vec3(&d["coPressureOffset"]),
        lift_offset_m: vec3(&d["coLiftOffset"]),
        drag_properties,
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
        reaction_wheel_axes_knm: wheel_axes,
        electric,
        experiments,
        docking,
        parachute,
        thermal_mass_modifier: match f(d, "thermalMassModifier") {
            m if m > 0.0 => m,
            _ => 1.0,
        },
        heatshield,
        landing_gear,
        converter,
        ejection_force,
        fairing,
        cargo_bay,
        rcs,
        jetpack,
        lifting_surface,
        ctrl_range_deg: ctrl.0,
        ctrl_area: ctrl.1,
        ctrl_pressure_offset_m,
        intake_area_m2,
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
                if o["requiresCommNet"].as_bool() == Some(true) {
                    e.command_requires_commnet = true;
                }
            }
            "Data_Transmitter" => {
                e.comm_range_m = e.comm_range_m.max(f(o, "CommunicationRange"));
                if f(o, "DataPacketSize") > e.transmit_packet {
                    e.transmit_packet = f(o, "DataPacketSize");
                    e.transmit_interval_s = f(o, "DataTransmissionInterval");
                    e.transmit_ec_per_s = o["RequiredResources"]
                        .as_array()
                        .map(|r| r.iter().map(ec_rate).sum::<f64>())
                        .unwrap_or(0.0);
                }
            }
            "Data_ReactionWheel" => {
                if let Some(r) = o["RequiredResources"].as_array() {
                    e.wheel_use_per_knm = r.iter().map(ec_rate).sum::<f64>();
                    e.wheel_acceptance_threshold = r
                        .iter()
                        .find(|r| r["ResourceName"].as_str() == Some("ElectricCharge"))
                        .and_then(|r| r["AcceptanceThreshold"].as_f64())
                        .filter(|v| v.is_finite())
                        .map(|v| v.clamp(0.0, 1.0));
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
                if o["extendable"].as_bool() == Some(true)
                    && o["IsExtended"].as_bool() != Some(true)
                {
                    e.solar_deployable = true;
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
/// The original's Kerbals (`kerbal_variety_text/*.json`, one Kerbal each): the four veterans in their
/// order, then the rest by name.
pub fn import_kerbals(dir: &Path) -> anyhow::Result<Vec<ose_data::KerbalDef>> {
    let mut out = Vec::new();
    for p in json_files(dir)? {
        let v = read_json(&p)?;
        // one Kerbal per file; the attribute tables beside them are skipped
        let Some((_, k)) = v.as_object().and_then(|o| o.iter().next()) else {
            continue;
        };
        if !k["NAME"].is_string() {
            continue;
        }
        let title = |w: &str| {
            w.split(' ')
                .map(|part| {
                    let mut c = part.chars();
                    match c.next() {
                        Some(f) => {
                            f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let name = format!("{} {}", title(&s(k, "NAME")), title(&s(k, "SURNAME")));
        let num = |key: &str| match &k[key] {
            Value::String(x) => x.parse().unwrap_or(0.0),
            other => other.as_f64().unwrap_or(0.0),
        };
        let veteran = match &k["ISVETERAN"] {
            Value::String(x) => x == "true",
            other => other.as_bool().unwrap_or(false),
        };
        let looks = [
            "TYPE",
            "HAIRSTYLE",
            "HAIRCOLOR",
            "EYES",
            "SKINCOLOR",
            "FACIALHAIR",
            "FACEDECORATION",
            "FACEDETAIL",
            "HELMET",
            "TEAMCOLOR1",
            "TEAMCOLOR2",
        ]
        .iter()
        .filter_map(|key| k[*key].as_str().map(|v| (key.to_string(), v.to_string())))
        .collect();
        out.push(ose_data::KerbalDef {
            name,
            veteran,
            bravery: num("BRAVERY"),
            stupidity: num("STUPIDITY"),
            optimism: num("OPTIMISM"),
            constitution: num("CONSTITUTION"),
            looks,
        });
    }
    const VETERANS: [&str; 4] = [
        "Jebediah Kerman",
        "Bill Kerman",
        "Bob Kerman",
        "Valentina Kerman",
    ];
    out.sort_by(|a, b| {
        let rank = |k: &ose_data::KerbalDef| {
            VETERANS
                .iter()
                .position(|v| *v == k.name)
                .unwrap_or(VETERANS.len())
        };
        rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

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
/// An engine mixture that is no resource of its own (a jet's `MethaneAir`) becomes a recipe of the
/// ingredients the engine lists, without intake air: jets take air from the atmosphere through intakes.
fn mixture_recipes(v: &Value, resources: &mut Vec<ResourceDef>) {
    let d = unwrap_data(v);
    let Some(mods) = d["serializedPartModules"].as_array() else {
        return;
    };
    for m in mods {
        for entry in m["ModuleData"].as_array().into_iter().flatten() {
            if entry["Name"] != "Data_Engine" {
                continue;
            }
            for em in entry["DataObject"]["engineModes"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let mix = s(&em["propellant"], "mixtureName");
                if mix.is_empty() || resources.iter().any(|r| r.id.eq_ignore_ascii_case(&mix)) {
                    continue;
                }
                let ingredients: Vec<ose_data::RecipeIngredient> = em["propellant"]
                    ["ingredientOverrides"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| i["name"] != "IntakeAir")
                    .map(|i| ose_data::RecipeIngredient {
                        resource: s(i, "name"),
                        units_per_recipe_unit: f(i, "unitsPerRecipeUnit"),
                    })
                    .collect();
                if !ingredients.is_empty() {
                    resources.push(ResourceDef {
                        id: mix.clone(),
                        display_name_key: format!("Resource/DisplayName/{mix}"),
                        abbreviation_key: String::new(),
                        kind: ose_data::ResourceKind::Recipe { ingredients },
                    });
                }
            }
        }
    }
}

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
        let v = read_json(&p)?;
        mixture_recipes(&v, &mut resources);
        parts.push(part(&v, cov));
    }
    Ok(GameData {
        schema_version: SCHEMA_VERSION,
        bodies,
        resources,
        parts,
    })
}

/// `Data_Parachute`'s safety fields, the class defaults where a part leaves one out (the EVA chute).
fn chute_safety(o: &serde_json::Value) -> ose_data::ChuteSafety {
    let d = ose_data::ChuteSafety::default();
    let g = |k: &str, default: f64| o[k].as_f64().unwrap_or(default);
    ose_data::ChuteSafety {
        max_temp_k: g("chuteMaxTemp", d.max_temp_k),
        thermal_mass_per_area: g("ChuteThermalMassPerArea", d.thermal_mass_per_area),
        fraction_exposed: g("ChuteFractionExposed", d.fraction_exposed),
        area_deployed_m2: g("areaDeployed", d.area_deployed_m2),
        starting_temp_k: g("startingTemp", d.starting_temp_k),
        seconds_for_risky: g("SecondsForRisky", d.seconds_for_risky),
        seconds_for_unsafe: g("SecondsForUnsafe", d.seconds_for_unsafe),
        mach_safety: g("MachSafetyScalar", d.mach_safety),
        flux_safety: g("FluxSafetyScalar", d.flux_safety),
        atmosphere_flux_safety_lerp: g("AtmosphereFluxSafetyLerp", d.atmosphere_flux_safety_lerp),
    }
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
    fn wheel_import_preserves_each_axis_instead_of_repeating_pitch_torque() {
        let source = json!({"data":{"partName":"wheel","serializedPartModules":[
            {"Name":"PartComponentModule_ReactionWheel","ModuleData":[
                {"Name":"Data_ReactionWheel","DataObject":{"PitchTorque":2.0,"YawTorque":3.0,"RollTorque":0.5}}
            ]}
        ]}});
        let imported = part(&source, &mut Coverage::default());
        assert_eq!(
            imported.reaction_wheel_axes_knm,
            Some(ose_math::DVec3::new(2.0, 0.5, 3.0))
        );
        assert_eq!(imported.reaction_wheel_torque_knm, 3.0);
    }

    #[test]
    fn generic_deployables_are_solar_only_when_the_part_generates_solar_power() {
        let mut p = json!({"data":{"partName":"deployable","serializedPartModules":[
            {"Name":"PartComponentModule_Deployable","ModuleData":[{"Name":"Data_Deployable", "DataObject":{"isTracking":true,"extendable":true}}]}
        ]}});
        let imported = part(&p, &mut Coverage::default());
        assert!(!imported.electric.solar_deployable);
        assert!(!imported.electric.solar_tracking);
        p["data"]["serializedPartModules"].as_array_mut().unwrap().push(json!({
            "Name":"PartComponentModule_SolarPanel", "ModuleData":[{"Name":"Data_SolarPanel","DataObject":{"ResourceSettings":{"ResourceName":"ElectricCharge","Rate":2.0}}}]
        }));
        let imported = part(&p, &mut Coverage::default());
        assert!(imported.electric.solar_deployable);
        assert!(imported.electric.solar_tracking);
        assert_eq!(imported.electric.solar_output, 2.0);
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
    #[test]
    fn prefab_engine_geometry_maps_all_modes_and_rejects_stale_or_invalid_exports() {
        let mut parts: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let geometry: BTreeMap<String, Vec<Vec<ose_data::ThrustTransform>>> =
            serde_json::from_str(include_str!("../../../assets/own/physics-geometry.json"))
                .unwrap();
        assert_eq!(geometry.len(), 36);
        for part in &mut parts {
            for mode in &mut part.engines {
                mode.thrust_transforms.clear();
            }
        }
        apply_engine_geometry(&mut parts, &geometry).unwrap();
        assert_eq!(
            parts
                .iter()
                .flat_map(|p| &p.engines)
                .filter(|m| !m.thrust_transforms.is_empty())
                .count(),
            41
        );
        let id = "engine_1v_methalox_cornet".to_string();
        let mut bad = BTreeMap::from([(id.clone(), Vec::new())]);
        assert!(apply_engine_geometry(&mut parts, &bad)
            .unwrap_err()
            .contains("mode count"));
        bad.insert(id.clone(), geometry[&id].clone());
        bad.get_mut(&id).unwrap()[0][0].direction = DVec3::ZERO;
        assert!(apply_engine_geometry(&mut parts, &bad).is_err());
        bad.get_mut(&id).unwrap()[0][0].direction = DVec3::new(f64::NAN, 0.0, 0.0);
        assert!(apply_engine_geometry(&mut parts, &bad).is_err());
        let mut old =
            serde_json::to_value(&parts.iter().find(|p| p.id == id).unwrap().engines[0]).unwrap();
        old.as_object_mut().unwrap().remove("thrust_transforms");
        let mode: EngineMode = serde_json::from_value(old).unwrap();
        assert!(
            mode.thrust_transforms.is_empty(),
            "old catalogs retain the origin/+Y fallback"
        );
    }
    #[test]
    fn rcs_geometry_import_checks_nozzle_definitions_and_legacy_fallback() {
        let mut parts: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let geometry: BTreeMap<String, Vec<ose_data::ThrustTransform>> =
            serde_json::from_str(include_str!("../../../assets/own/rcs-geometry.json")).unwrap();
        for part in &mut parts {
            if let Some(rcs) = &mut part.rcs {
                rcs.thrusters.clear();
            }
        }
        apply_rcs_geometry(&mut parts, &geometry).unwrap();
        assert_eq!(
            parts
                .iter()
                .filter_map(|p| p.rcs.as_ref())
                .map(|r| r.thrusters.len())
                .sum::<usize>(),
            22
        );
        let id = "rcs_0v_linear_methalox".to_string();
        let mut bad = BTreeMap::from([(id.clone(), geometry[&id].clone())]);
        bad.get_mut(&id).unwrap()[0].weight = 0.5;
        assert!(apply_rcs_geometry(&mut parts, &bad).is_err());
        let mut old = serde_json::to_value(
            parts
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .rcs
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        old.as_object_mut().unwrap().remove("thrusters");
        let rcs: ose_data::Rcs = serde_json::from_value(old).unwrap();
        assert!(rcs.thrusters.is_empty());
    }
    #[test]
    fn authored_mass_offsets_import_and_old_catalogs_default_to_the_origin() {
        let value = serde_json::json!({"data":{"partName":"test","mass":2.0,"coMassOffset":{"x":1.0,"y":2.0,"z":3.0}}});
        let imported = part(&value, &mut Coverage::default());
        assert_eq!(imported.mass_offset_m, DVec3::new(1.0, 2.0, 3.0));
        let mut old = serde_json::to_value(&imported).unwrap();
        old.as_object_mut().unwrap().remove("mass_offset_m");
        assert_eq!(
            serde_json::from_value::<PartDef>(old)
                .unwrap()
                .mass_offset_m,
            DVec3::ZERO
        );
    }
    #[test]
    fn authored_jetpack_curve_and_rotation_settings_survive_the_slim_catalog() {
        let parts: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let pack = parts
            .iter()
            .find(|p| p.id == "eva_kerbal")
            .unwrap()
            .jetpack
            .as_ref()
            .unwrap();
        assert_eq!(pack, &ose_data::Jetpack::default());
        assert!((pack.isp_curve.evaluate(2.5) - 27.50059375).abs() < 1e-9);
        let value = serde_json::json!({"data":{"partName":"eva","serializedPartModules":[{"ModuleData":[{"Name":"Data_Kerbal","DataObject":{"JetpackMoveForce":0.3,"JetpackRotateTorque":0.2,"JetpackRotateTorqueOffset":0.4,"JetpackRotateMaxAngularSpeed":45.0,"JetpackRotateResetEnabled":false,"JetpackRotateResetAngularSpeed":30.0,"JetpackRotateResetAngularSpeedThreshold":0.5,"JetpackPropellantDefinition":{"mixtureName":"MonoPropellant"},"JetpackAtmosphereCurve":{"fCurve":{"keys":[{"time":0.0,"value":150.0,"inTangent":0.0,"outTangent":0.0}]}}}}]}]}});
        let imported = part(&value, &mut Coverage::default()).jetpack.unwrap();
        assert_eq!(imported.move_force_kn, 0.3);
        assert_eq!(imported.rotate_torque_knm, 0.2);
        assert_eq!(imported.rotate_offset_m, 0.4);
        assert_eq!(imported.rotate_max_speed_deg_s, 45.0);
        assert!(!imported.reset_enabled);
        assert_eq!(imported.isp_curve.evaluate(0.0), 150.0);
    }
    #[test]
    fn lifting_frames_validate_geometry_preserve_module_data_and_migrate_old_catalogs() {
        let source = serde_json::json!({"data":{"partName":"wing","serializedPartModules":[{"ModuleData":[{"Name":"Data_LiftingSurface","DataObject":{"transformName":"aero","transformDir":"X","transformSign":1.0,"CoPOffset":{"x":1.0,"y":2.0,"z":3.0}}}]}]}});
        let mut parts = vec![part(&source, &mut Coverage::default())];
        let frame = ose_data::SurfaceFrame {
            position_m: DVec3::new(1.0, 2.0, 3.0),
            point_axes: [
                DVec3::new(0.0, 2.0, 0.0),
                DVec3::new(-3.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 4.0),
            ],
            direction_axes: [
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(-1.0, 0.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ],
        };
        let export = LiftingGeometry {
            transform_name: "aero".into(),
            resolved: true,
            frame,
        };
        let geometry = BTreeMap::from([("wing".into(), export.clone())]);
        apply_lifting_geometry(&mut parts, &geometry).unwrap();
        let surface = parts[0].lifting_surface.as_ref().unwrap();
        assert_eq!(surface.axis, DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(surface.pressure_offset_m, DVec3::new(1.0, 2.0, 3.0));
        let frame = surface.reference_frame.as_ref().unwrap();
        assert_eq!(
            frame.point(surface.pressure_offset_m),
            DVec3::new(-5.0, 4.0, 15.0)
        );
        assert_eq!(frame.direction(surface.axis), DVec3::new(0.0, 1.0, 0.0));
        let mut old = serde_json::to_value(surface).unwrap();
        old.as_object_mut().unwrap().remove("transform_name");
        old.as_object_mut().unwrap().remove("reference_frame");
        let restored: ose_data::LiftingSurface = serde_json::from_value(old).unwrap();
        assert!(restored.transform_name.is_empty() && restored.reference_frame.is_none());
        for case in 0..5 {
            let mut bad = export.clone();
            match case {
                0 => bad.transform_name = "stale".into(),
                1 => bad.frame.position_m.x = f64::NAN,
                2 => bad.frame.point_axes[0] = DVec3::ZERO,
                3 => bad.frame.direction_axes[0] = bad.frame.direction_axes[0] * 2.0,
                _ => bad.frame.direction_axes[0] = bad.frame.direction_axes[0] * -1.0,
            }
            assert!(
                apply_lifting_geometry(&mut parts, &BTreeMap::from([("wing".into(), bad)]))
                    .is_err()
            );
        }
        let mut mirrored = export.clone();
        mirrored.resolved = false; // Missing named transforms export the module-owner fallback.
        mirrored.frame.point_axes[0] = mirrored.frame.point_axes[0] * -1.0;
        apply_lifting_geometry(&mut parts, &BTreeMap::from([("wing".into(), mirrored)])).unwrap();
        assert_eq!(
            parts[0]
                .lifting_surface
                .as_ref()
                .unwrap()
                .reference_frame
                .as_ref()
                .unwrap()
                .direction(DVec3::new(1.0, 0.0, 0.0)),
            DVec3::new(0.0, 1.0, 0.0)
        );
        assert!(
            apply_lifting_geometry(&mut parts, &BTreeMap::from([("unknown".into(), export)]))
                .is_err()
        );
    }

    #[test]
    fn committed_lifting_frames_match_the_thirteen_authored_root_transforms() {
        let mut parts: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let geometry: BTreeMap<String, LiftingGeometry> =
            serde_json::from_str(include_str!("../../../assets/own/lifting-geometry.json"))
                .unwrap();
        assert_eq!(geometry.len(), 13);
        apply_lifting_geometry(&mut parts, &geometry).unwrap();
        for (id, exported) in geometry {
            assert!(exported.transform_name.is_empty() && exported.resolved);
            let frame = parts
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .lifting_surface
                .as_ref()
                .unwrap()
                .reference_frame
                .as_ref()
                .unwrap();
            assert_eq!(frame.point(DVec3::ZERO), DVec3::ZERO);
            for axis in [
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ] {
                assert_eq!(frame.point(axis), axis);
                assert_eq!(frame.direction(axis), axis);
            }
        }
    }

    #[test]
    fn lifting_node_gate_imports_and_old_catalogs_leave_lift_enabled() {
        let value = serde_json::json!({"data":{"partName":"wing","serializedPartModules":[
            {"ModuleData":[{"Name":"Data_LiftingSurface","DataObject":{"nodeEnabled":true,"attachNodeName":"top"}}]}
        ]}});
        let surface = part(&value, &mut Coverage::default())
            .lifting_surface
            .unwrap();
        assert!(surface.node_enabled);
        assert_eq!(surface.attach_node_name, "top");
        let mut old = serde_json::to_value(surface).unwrap();
        old.as_object_mut().unwrap().remove("node_enabled");
        old.as_object_mut().unwrap().remove("attach_node_name");
        let restored: ose_data::LiftingSurface = serde_json::from_value(old).unwrap();
        assert!(!restored.node_enabled && restored.attach_node_name.is_empty());
        let catalog: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        let surfaces: Vec<_> = catalog
            .iter()
            .filter_map(|p| p.lifting_surface.as_ref())
            .collect();
        assert_eq!(surfaces.len(), 13);
        assert!(surfaces
            .iter()
            .all(|s| !s.node_enabled && s.attach_node_name.is_empty()));
    }

    #[test]
    fn aerodynamic_offsets_and_lift_options_import_with_legacy_defaults() {
        let value = serde_json::json!({"data":{"partName":"wing","coPressureOffset":{"x":1.0,"y":2.0,"z":3.0},"serializedPartModules":[{"ModuleData":[{"Name":"Data_LiftingSurface","DataObject":{"CoPOffset":{"x":0.0,"y":4.0,"z":0.0},"perpendicularOnly":true,"useInternalDragModel":false}},{"Name":"Data_ControlSurface","DataObject":{"CtrlSurfaceRange":20.0,"CtrlSurfaceArea":0.5,"CoPressureCtrlSurfaceOffset":{"x":5.0,"y":0.0,"z":0.0}}}]}]}});
        let imported = part(&value, &mut Coverage::default());
        assert_eq!(imported.pressure_offset_m, DVec3::new(1.0, 2.0, 3.0));
        assert_eq!(imported.ctrl_pressure_offset_m, DVec3::new(5.0, 0.0, 0.0));
        let wing = imported.lifting_surface.as_ref().unwrap();
        assert_eq!(wing.pressure_offset_m, DVec3::new(0.0, 4.0, 0.0));
        assert!(wing.perpendicular_only);
        assert!(!wing.use_internal_drag_model);
        let mut old = serde_json::to_value(wing).unwrap();
        for key in [
            "pressure_offset_m",
            "perpendicular_only",
            "use_internal_drag_model",
        ] {
            old.as_object_mut().unwrap().remove(key);
        }
        let legacy: ose_data::LiftingSurface = serde_json::from_value(old).unwrap();
        assert_eq!(legacy.pressure_offset_m, DVec3::ZERO);
        assert!(!legacy.perpendicular_only);
        assert!(legacy.use_internal_drag_model);
    }
    #[test]
    fn body_lift_flags_multiplier_and_authored_lift_point_are_imported() {
        let value = serde_json::json!({"data":{"partName":"body","coLiftOffset":{"x":1.0,"y":0.0,"z":2.0},"serializedPartModules":[{"ModuleData":[{"Name":"Data_Drag","DataObject":{"DragEnabled":false,"BodyLiftEnabled":true,"bodyLiftMultiplier":2.5}}]}]}});
        let imported = part(&value, &mut Coverage::default());
        assert_eq!(imported.lift_offset_m, DVec3::new(1.0, 0.0, 2.0));
        assert!(!imported.drag_properties.drag_enabled);
        assert!(imported.drag_properties.body_lift_enabled);
        assert_eq!(imported.drag_properties.body_lift_multiplier, 2.5);
        let mut old = serde_json::to_value(&imported).unwrap();
        old.as_object_mut().unwrap().remove("drag_properties");
        old.as_object_mut().unwrap().remove("lift_offset_m");
        let legacy: PartDef = serde_json::from_value(old).unwrap();
        assert_eq!(legacy.drag_properties, ose_data::DragProperties::default());
        assert_eq!(legacy.lift_offset_m, DVec3::ZERO);
    }
    #[test]
    fn rcs_source_request_minimum_imports_and_older_catalogues_retain_legacy_allocation() {
        let value = serde_json::json!({"data":{"partName":"rcs","serializedPartModules":[{"ModuleData":[{"Name":"Data_RCS","DataObject":{"maxThrust":1.0}}]}]}});
        let rcs = part(&value, &mut Coverage::default()).rcs.unwrap();
        assert_eq!(rcs.request_minimum, Some(0.5));
        let mut old = serde_json::to_value(rcs).unwrap();
        old.as_object_mut().unwrap().remove("request_minimum");
        assert_eq!(
            serde_json::from_value::<ose_data::Rcs>(old)
                .unwrap()
                .request_minimum,
            None
        );
    }

    #[test]
    fn rcs_module_activation_defaults_and_limits_import_and_migrate() {
        let value = serde_json::json!({"data":{"partName":"rcs","serializedPartModules":[{"ModuleData":[{"Name":"Data_RCS","DataObject":{"isEnabled":{"storedValue":false},"thrustPercentage":{"storedValue":25.0},"shieldedCanActivate":true,"maxThrust":2.0}}]}]}});
        let imported = part(&value, &mut Coverage::default()).rcs.unwrap();
        assert!(!imported.enabled);
        assert_eq!(imported.thrust_fraction, 0.25);
        assert!(imported.shielded_can_activate);
        assert_eq!(imported.limited_thrust_kn(), 0.0);
        let mut old = serde_json::to_value(&imported).unwrap();
        for key in ["enabled", "thrust_fraction", "shielded_can_activate"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let legacy: ose_data::Rcs = serde_json::from_value(old).unwrap();
        assert!(legacy.enabled);
        assert_eq!(legacy.thrust_fraction, 1.0);
        assert!(!legacy.shielded_can_activate);
        assert_eq!(legacy.limited_thrust_kn(), 2.0);
    }
    #[test]
    fn rcs_control_switches_import_and_default_for_older_catalogs() {
        let value = serde_json::json!({"data":{"partName":"rcs","serializedPartModules":[{"ModuleData":[{"Name":"Data_RCS","DataObject":{"enablePitch":{"storedValue":false},"enableYaw":{"storedValue":false},"enableRoll":{"storedValue":true},"enableX":{"storedValue":false},"enableY":{"storedValue":false},"enableZ":{"storedValue":true},"foreByThrottle":{"storedValue":true},"fullThrust":{"storedValue":true},"fullThrustMin":0.3}}]}]}});
        let imported = part(&value, &mut Coverage::default()).rcs.unwrap();
        let c = imported.controls;
        assert_eq!(c.rotation_axes(), DVec3::new(0.0, 1.0, 0.0));
        assert_eq!(
            c.translation(DVec3::new(1.0, 0.0, 1.0), 0.6),
            DVec3::new(0.0, 0.6, 0.0)
        );
        assert_eq!(
            c.translation(DVec3::new(0.0, -0.5, 0.0), 1.0),
            DVec3::new(0.0, -0.5, 0.0)
        );
        assert_eq!(c.nozzle_input(0.2), 0.2);
        assert_eq!(c.nozzle_input(0.3), 1.0);
        assert_eq!(c.nozzle_input(0.0), 0.0);
        let mut old = serde_json::to_value(imported).unwrap();
        old.as_object_mut().unwrap().remove("controls");
        let legacy: ose_data::Rcs = serde_json::from_value(old).unwrap();
        assert_eq!(legacy.controls, ose_data::RcsControls::default());
    }
    #[test]
    fn reaction_wheel_power_acceptance_is_imported_and_legacy_data_keeps_proportional_delivery() {
        let value = serde_json::json!({"data":{"partName":"wheel","serializedPartModules":[{"ModuleData":[{"Name":"Data_ReactionWheel","DataObject":{"PitchTorque":5.0,"RequiredResources":[{"ResourceName":"ElectricCharge","Rate":0.025,"AcceptanceThreshold":0.01}]}}]}]}});
        let imported = part(&value, &mut Coverage::default());
        assert_eq!(imported.electric.wheel_use_per_knm, 0.025);
        assert_eq!(imported.electric.wheel_acceptance_threshold, Some(0.01));
        let mut legacy = serde_json::to_value(imported.electric).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("wheel_acceptance_threshold");
        let legacy: ose_data::Electric = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.wheel_acceptance_threshold, None);
    }

    #[test]
    fn authored_full_break_strength_nodes_import_and_legacy_nodes_default_to_normal() {
        let value = serde_json::json!({"data":{"partName":"wing","attachNodes":[{"nodeID":"srfAttach","nodeType":"Surface","size":2,"MultiJointFullBreakStrength":true}]}});
        let imported = part(&value, &mut Coverage::default());
        assert!(imported.attach_nodes[0].full_break_strength);
        let mut old = serde_json::to_value(&imported.attach_nodes[0]).unwrap();
        old.as_object_mut().unwrap().remove("full_break_strength");
        assert!(
            !serde_json::from_value::<AttachNode>(old)
                .unwrap()
                .full_break_strength
        );
        let parts: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        assert_eq!(
            parts
                .iter()
                .flat_map(|p| &p.attach_nodes)
                .filter(|n| n.full_break_strength)
                .count(),
            9
        );
    }
}

//! T39: fuel flow over a fuel line, on the craft the KSP2 parity pilot flies in its "fuelline" scenario
//! (`tools/ksp2-parity-pilot/craft-fuelline.json`): pod, tank A, an inline decoupler that blocks crossfeed,
//! tank B, Swivel, and a fuel line from A to B. The Swivel burns at full throttle and each tank's stored
//! propellant is traced, as the pilot traces KSP2's.
//!
//! `OSE_FUELLINE_CSV=<path>` writes the trace (t, tank, resource, stored units) for the side-by-side report.

use ose_data::craft::Craft;
use ose_data::{PartDef, ResourceDef};
use ose_sim::flight::EngineEnvironment;
use ose_sim::resources::{Instruction, Request};
use ose_sim::structure::Structure;

fn craft() -> (Craft, Vec<PartDef>, Vec<ResourceDef>) {
    let v: serde_json::Value = serde_json::from_str(
        include_str!("../../../tools/ksp2-parity-pilot/craft-fuelline.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    let craft = ose_importer::craft::craft_from_value(&v).unwrap();
    let defs: Vec<PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    (craft, defs, res)
}

/// Burn the engine stage at full throttle for up to `seconds`; returns, per step of `dt`, the stored units
/// of every tank resource as (t, part, resource name, stored).
fn burn(
    craft: &Craft,
    defs: &[PartDef],
    res: &[ResourceDef],
    seconds: f64,
) -> Vec<(f64, usize, String, f64)> {
    let mut s = Structure::build(craft, defs, res);
    let stage = (0..craft.stages.len().max(1))
        .find(|&k| !s.fed_engines(k).is_empty())
        .expect("a stage with a fed engine");
    let dt = 0.02;
    let mut out = Vec::new();
    let mut t = 0.0;
    while t < seconds {
        let mut requests = Vec::new();
        for d in s.engine_demands(stage, 1.0, EngineEnvironment::default()) {
            let mix = craft.parts[d.part].part_id.as_str();
            let def = defs.iter().find(|x| x.id == mix).unwrap();
            let mixture = &def.engines[0].propellant_mixture;
            let unit = s.net.mixture_unit_mass_t(mixture);
            requests.push(Request {
                part: d.part,
                instructions: s
                    .net
                    .ingredients(mixture)
                    .into_iter()
                    .map(|(resource, units)| Instruction {
                        resource,
                        units_per_s: units * d.mass_flow_tps / unit,
                    })
                    .collect(),
                priority: 0,
            });
        }
        if requests.is_empty() {
            break;
        }
        let before = s.net.mass_t();
        s.net.process(&requests, dt);
        t += dt;
        for tank in &s.net.tanks {
            out.push((
                t,
                tank.part,
                s.net.resources[tank.resource].clone(),
                tank.stored,
            ));
        }
        if (before - s.net.mass_t()).abs() < 1e-12 {
            break;
        }
    }
    out
}

fn stored(trace: &[(f64, usize, String, f64)], t: f64, part: usize, r: &str) -> f64 {
    trace
        .iter()
        .rfind(|x| x.1 == part && x.2 == r && x.0 <= t + 1e-9)
        .map(|x| x.3)
        .unwrap()
}

#[test]
fn the_engine_below_the_decoupler_drains_the_tank_above_through_the_fuel_line_first() {
    let (craft, defs, res) = craft();
    let tanks: Vec<usize> = (0..craft.parts.len())
        .filter(|&i| craft.parts[i].part_id.starts_with("fueltank"))
        .collect();
    let (a, b) = (tanks[0], tanks[1]);
    let first = (0..craft.parts.len())
        .find(|&i| craft.parts[i].fuel_line_target.is_some())
        .unwrap();
    assert_eq!(craft.compound_ends(first), Some((a, b)));

    let trace = burn(&craft, &defs, &res, 400.0);
    if let Ok(path) = std::env::var("OSE_FUELLINE_CSV") {
        let mut csv = String::from("t,part,name,resource,stored_units\n");
        for (t, p, r, v) in &trace {
            csv += &format!("{t:.2},{p},{},{r},{v}\n", craft.parts[*p].part_id);
        }
        std::fs::write(path, csv).unwrap();
    }
    let end = trace.last().unwrap().0;
    let full_b = stored(&trace, 0.02, b, "Oxidizer");
    // early on, A (over the line) is what burns; B waits
    let early = end * 0.25;
    let full_a = stored(&trace, 0.02, a, "Oxidizer");
    assert!(
        stored(&trace, early, a, "Oxidizer") < full_a * 0.7,
        "A drains first"
    );
    assert!(
        (stored(&trace, early, b, "Oxidizer") - full_b).abs() < 1e-6 * full_b.max(1.0),
        "B untouched while A feeds the engine"
    );
    // by the end both are empty
    assert!(stored(&trace, end, a, "Oxidizer") < 1e-6);
    assert!(stored(&trace, end, b, "Oxidizer") < 1e-6);

    // without the line the decoupler keeps A to itself
    let mut cut = craft.clone();
    cut.parts[first].fuel_line_target = None;
    let alone = burn(&cut, &defs, &res, 400.0);
    let end_alone = alone.last().unwrap().0;
    assert!(
        (stored(&alone, end_alone, a, "Oxidizer") - stored(&alone, 0.02, a, "Oxidizer")).abs()
            < 1e-9
    );
    assert!(
        end_alone < end * 0.6,
        "half the propellant: {end_alone} s vs {end} s"
    );
}

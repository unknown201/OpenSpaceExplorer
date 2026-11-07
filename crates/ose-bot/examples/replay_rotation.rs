//! Replay a KSP2 rotation trace into the simulation, tick for tick, and compare the spin.
//!
//! `cargo run -p ose-bot --example replay_rotation -- <craft.json> <ksp2-trace.csv> <out.csv>`
//!
//! The craft (a KSP2 workspace file) goes into a 200 km circular Kerbin orbit. Each trace row is one 0.02 s
//! physics tick: its `pitch`/`yaw`/`roll` columns are the input KSP2 actually applied that tick and `sas`
//! whether SAS (stability) held. Driving our simulation with that exact sequence separates the physics
//! from how the inputs were delivered: the KSP2 capture applies a held key on only part of the ticks.
//! Prints the angular-rate magnitude of both at the end of each input phase and writes both series to
//! `out.csv`.

use ose_math::DVec3;
use ose_sim::{flight, structure, Universe};
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() == 3,
        "usage: replay_rotation <craft.json> <trace.csv> <out.csv>"
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read = |p: &str| std::fs::read_to_string(root.join(p));
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(&read("assets/own/kerbol-system.json")?)?;
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(&read("assets/own/parts-catalog.json")?)?;
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(&read("assets/own/resources.json")?)?;
    let craft_json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args[0])?)?;
    let mut craft = ose_importer::craft::craft_from_value(&craft_json)?;
    fill_mixtures(&mut craft, &res);

    let mut u = Universe::with_bodies(&bodies);
    let kerbin = u.body_entity("Kerbin").expect("Kerbin");
    let v = u.spawn_vessel_in_circular_orbit("replay", kerbin, 200_000.0);
    let (r, vel) = u.vessel(v).expect("vessel").2.state_at(u.ut());
    let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let mut p = flight::Powered::new(spec, r, vel, false);
    p.facing = vel.normalized();
    p.attitude = flight::Attitude::Free;
    // a whisper of throttle keeps it under power (off rails) without lighting anything
    p.throttle = 0.0;
    u.set_powered(v, p);
    u.set_craft(v, craft.clone());
    u.set_structure(v, structure::Structure::build(&craft, &defs, &res));
    u.step();

    let trace: Vec<Row> = csv_rows(&std::fs::read_to_string(&args[1])?)?;
    let mut out = std::fs::File::create(&args[2])?;
    writeln!(out, "t,pitch,yaw,roll,sas,ksp2_w,ose_w")?;
    let mut sas_on = false;
    let mut phase: Option<(f64, f64, f64, bool)> = None;
    let mut last = (0.0, 0.0, 0.0);
    for row in &trace {
        if row.sas != sas_on {
            sas_on = row.sas;
            let facing = u.powered(v).map(|p| p.facing).unwrap_or(DVec3::ZERO);
            let attitude = if sas_on {
                flight::Attitude::Fixed(facing)
            } else {
                flight::Attitude::Free
            };
            u.set_controls(v, 0.0, attitude);
        }
        // with SAS holding, the trace's input columns are what KSP2's SAS commanded, not the pilot: our own
        // SAS flies that part, with the pilot's hands off
        if row.sas {
            u.set_rotation(v, 0.0, 0.0, 0.0);
        } else {
            u.set_rotation(v, row.pitch, row.yaw, row.roll);
        }
        u.step();
        let ose_w = u
            .powered(v)
            .map(|p| (p.angvel + p.facing * p.roll_rate).length())
            .unwrap_or(f64::NAN);
        writeln!(
            out,
            "{},{},{},{},{},{},{}",
            row.t, row.pitch, row.yaw, row.roll, row.sas as u8, row.w, ose_w
        )?;
        // phases: which axis is held (any tick of it within the last 0.2 s) and SAS
        let key = (
            (row.pitch != 0.0) as u8 as f64,
            (row.yaw != 0.0) as u8 as f64,
            (row.roll != 0.0) as u8 as f64,
            row.sas,
        );
        if phase != Some(key) && (key.0 + key.1 + key.2 > 0.0 || row.t - last.0 > 0.2) {
            if let Some(k) = phase {
                println!(
                    "phase {:?} ending t {:.2}: KSP2 |w| {:.4}  ours {:.4}",
                    k, last.0, last.1, last.2
                );
            }
            phase = Some(key);
        }
        if key.0 + key.1 + key.2 > 0.0 || row.sas {
            last = (row.t, row.w, ose_w);
        } else {
            last = (last.0.max(row.t - 0.2).min(row.t), row.w, ose_w);
            last.0 = row.t;
        }
    }
    Ok(())
}

struct Row {
    t: f64,
    pitch: f64,
    yaw: f64,
    roll: f64,
    sas: bool,
    w: f64,
}

fn csv_rows(text: &str) -> anyhow::Result<Vec<Row>> {
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
    let col = |name: &str| {
        head.iter()
            .position(|h| *h == name)
            .ok_or_else(|| anyhow::anyhow!("no column {name}"))
    };
    let (t, p, y, r, s, wx, wy, wz) = (
        col("t")?,
        col("pitch")?,
        col("yaw")?,
        col("roll")?,
        col("sas")?,
        col("wx")?,
        col("wy")?,
        col("wz")?,
    );
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let n = |i: usize| f[i].trim().parse::<f64>().unwrap_or(0.0);
            Ok(Row {
                t: n(t),
                pitch: n(p),
                yaw: n(y),
                roll: n(r),
                sas: f[s].trim() == "1" || f[s].trim().eq_ignore_ascii_case("true"),
                w: (n(wx).powi(2) + n(wy).powi(2) + n(wz).powi(2)).sqrt(),
            })
        })
        .collect()
}

/// Tanks listed by a mixture (Methalox) hold its ingredients, as the server stores them at launch.
fn fill_mixtures(craft: &mut ose_data::craft::Craft, res: &[ose_data::ResourceDef]) {
    for part in &mut craft.parts {
        let mut out = Vec::new();
        for r in part.resources.drain(..) {
            match res.iter().find(|d| d.id == r.resource).map(|d| &d.kind) {
                Some(ose_data::ResourceKind::Recipe { ingredients }) => {
                    for i in ingredients {
                        out.push(ose_data::craft::StoredResource {
                            resource: i.resource.clone(),
                            stored_units: r.stored_units * i.units_per_recipe_unit,
                            capacity_units: r.capacity_units * i.units_per_recipe_unit,
                        });
                    }
                }
                _ => out.push(r),
            }
        }
        part.resources = out;
    }
}

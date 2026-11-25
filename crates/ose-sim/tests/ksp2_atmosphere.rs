//! KSP2 parity for the air on every body (gaps T2, T11): the pilot's `atmosphere` dump (2026-11-25) of KSP2's own
//! `CelestialBodyComponent.GetPressure`, `GetTemperature`, `GetDensity` and `AeroUtilities.GetSoundSpeed` (what
//! flight telemetry, drag and heating read) at every quarter percent of the air's depth on Eve, Kerbin, Duna, Jool
//! and Laythe (`tests/data/ksp2-atmosphere`).
use std::collections::HashMap;

use ose_sim::Universe;

type Row = HashMap<String, String>;

fn csv(text: &str) -> Vec<Row> {
    let mut lines = text.lines();
    let head: Vec<String> = lines
        .next()
        .unwrap()
        .split(',')
        .map(str::to_string)
        .collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            head.iter()
                .cloned()
                .zip(l.split(',').map(str::to_string))
                .collect()
        })
        .collect()
}

fn num(r: &Row, k: &str) -> f64 {
    r[k].parse().unwrap()
}

fn system() -> Universe {
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    Universe::with_bodies(&bodies)
}

/// Relative difference, against a floor so the thin top of the air is judged in absolute terms.
fn off(ours: f64, theirs: f64, floor: f64) -> f64 {
    (ours - theirs).abs() / theirs.abs().max(floor)
}

#[test]
fn every_bodys_air_is_ksp2s_at_every_height() {
    let u = system();
    let rows = csv(include_str!("data/ksp2-atmosphere/atmosphere.csv"));
    let mut worst: HashMap<String, [f64; 4]> = HashMap::new();
    for r in &rows {
        let name = &r["body"];
        let body = u.body_entity(name).unwrap_or_else(|| panic!("{name}"));
        let air = u
            .atmosphere(body)
            .unwrap_or_else(|| panic!("{name} has no air"));
        let alt = num(r, "alt_m");
        if alt >= air.depth_m {
            // the top of the air: KSP2 reads no air there (temperature 0), as OSE's pressure and density do
            assert_eq!(air.pressure_kpa(alt), 0.0, "{name}");
            continue;
        }
        let (p, t, rho, c) = (
            num(r, "pressure_kpa"),
            num(r, "temperature_k"),
            num(r, "density"),
            num(r, "sound_mps"),
        );
        let p0 = air.pressure_kpa(0.0);
        let ours = [
            off(air.pressure_kpa(alt), p, p0 * 1e-4),
            off(air.temperature_k.evaluate(alt.min(air.depth_m)), t, 1.0),
            off(air.density(alt), rho, air.density(0.0) * 1e-4),
            if c > 0.0 {
                off(air.speed_of_sound(alt), c, 1.0)
            } else {
                0.0
            },
        ];
        let w = worst.entry(name.clone()).or_default();
        for k in 0..4 {
            if ours[k] > w[k] {
                w[k] = ours[k];
                if ours[k] > 0.005 {
                    println!(
                        "{name} {alt:.0} m: {} {:.4}% (ours p {:.5} t {:.3} rho {:.6} c {:.2}; KSP2 {p:.5} {t:.3} {rho:.6} {c:.2})",
                        ["pressure", "temperature", "density", "sound"][k],
                        ours[k] * 100.0,
                        air.pressure_kpa(alt),
                        air.temperature_k.evaluate(alt.min(air.depth_m)),
                        air.density(alt),
                        air.speed_of_sound(alt)
                    );
                }
            }
        }
    }
    let mut names: Vec<_> = worst.keys().cloned().collect();
    names.sort();
    for name in names {
        let w = worst[&name];
        println!(
            "{name}: worst pressure {:.3}%, temperature {:.3}%, density {:.3}%, speed of sound {:.3}%",
            w[0] * 100.0,
            w[1] * 100.0,
            w[2] * 100.0,
            w[3] * 100.0
        );
        for (k, what) in ["pressure", "temperature", "density", "speed of sound"]
            .iter()
            .enumerate()
        {
            assert!(w[k] < 0.005, "{name}: {what} off by {:.3}%", w[k] * 100.0);
        }
    }
}

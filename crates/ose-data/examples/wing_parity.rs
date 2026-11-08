//! Compare `ose_data::procedural` with what KSP2 computed for the same wing shapes
//! (tools/ksp2-wing-probe's `wings.csv`): mass, fixed and moving lift coefficients.
//!
//!     cargo run -p ose-data --example wing_parity -- reference/ksp2-wings/wings.csv

use ose_data::procedural::{evaluate, WingShape};

fn main() {
    let path = std::env::args().nth(1).expect("wings.csv");
    let text = std::fs::read_to_string(&path).expect("read wings.csv");
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    let col = |name: &str| header.iter().position(|h| *h == name).expect(name);
    let (label, part, mirrored, option, sliders) = (
        col("label"),
        col("part"),
        col("mirrored"),
        col("option"),
        col("sliders"),
    );
    let (mass, fixed, moving) = (col("mass_t"), col("lifting_ctrl_area"), col("ctrl_area"));
    println!(
        "{:<8} {:<34} {:>9} {:>9} {:>7} | {:>8} {:>8} {:>7} | {:>8} {:>8} {:>7}",
        "label",
        "part",
        "mass ksp2",
        "ose",
        "err%",
        "lift k2",
        "ose",
        "err%",
        "ctrl k2",
        "ose",
        "err%"
    );
    let err = |a: f64, b: f64| {
        if a.abs() > 1e-9 {
            (b - a) / a * 100.0
        } else {
            0.0
        }
    };
    for line in lines {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < header.len() || !ose_data::procedural::is_procedural_wing(f[part]) {
            continue;
        }
        let shape = WingShape {
            option: f[option].parse().unwrap_or(0),
            sliders: f[sliders]
                .split_whitespace()
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    Some((k.to_string(), v.parse().ok()?))
                })
                .collect(),
            toggles: vec![],
        };
        let Some(r) = evaluate(f[part], &shape, f[mirrored] == "1") else {
            continue;
        };
        let num = |i: usize| f[i].parse::<f64>().unwrap_or(f64::NAN);
        let (km, kf, kc) = (num(mass), num(fixed), num(moving));
        println!(
            "{:<8} {:<34} {:>9.5} {:>9.5} {:>6.1}% | {:>8.4} {:>8.4} {:>6.1}% | {:>8.4} {:>8.4} {:>6.1}%",
            f[label],
            f[part],
            km,
            r.mass_t,
            err(km, r.mass_t),
            kf,
            r.fixed_lift_coeff,
            err(kf, r.fixed_lift_coeff),
            kc,
            r.moving_lift_coeff,
            err(kc, r.moving_lift_coeff),
        );
    }
}

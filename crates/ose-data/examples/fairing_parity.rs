//! Compare OSE's procedural fairing drag cubes with KSP2's (gap T38): reads the parity pilot's
//! `fairing-cubes.csv` rows (`tools/ksp2-parity-pilot/run-fairings.ps1`) and renders each wall's cube here.
//!
//! cargo run -p ose-data --example fairing_parity -- reference/ksp2-fairings/<shape>/fairing-cubes.csv ...

use ose_data::fairing::{def, FairingShape};

fn main() {
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).expect("csv");
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().unwrap().split(',').collect();
        for line in lines {
            let row: std::collections::HashMap<&str, &str> =
                head.iter().copied().zip(line.split(',')).collect();
            if row["cube"] != "Fairing" {
                continue;
            }
            // KSP2 appends its nose point to the saved sections; the shape here is the player's wall
            let sections: Vec<(f64, f64)> = row["sections"]
                .split(' ')
                .map(|s| {
                    let (h, r) = s.split_once(':').unwrap();
                    (h.parse().unwrap(), r.parse().unwrap())
                })
                .filter(|&(_, r): &(f64, f64)| r > 0.0)
                .collect();
            let d = def(row["wall"]
                .split("base")
                .nth(1)
                .map_or("fairing_1v_procedural", |s| match &s[..2] {
                    "0v" => "fairing_0v_procedural",
                    "2v" => "fairing_2v_procedural",
                    "3v" => "fairing_3v_procedural",
                    "4v" => "fairing_4v_procedural",
                    _ => "fairing_1v_procedural",
                }))
            .expect("fairing def");
            let shape = FairingShape {
                sections,
                capped: row["capped"] == "1",
                pieces: 2,
                ejection_force: 100.0,
            };
            let c = shape.drag_cube(d);
            let k = |p: &str, i: usize| row[format!("{p}{i}").as_str()].parse::<f64>().unwrap();
            let kv = |name: &str| row[name].parse::<f64>().unwrap();
            println!("{} ({path})", row["wall"]);
            println!("  face   area KSP2/OSE      drag KSP2/OSE    depth KSP2/OSE");
            for i in 0..6 {
                println!(
                    "  {i}   {:7.3} {:7.3}   {:6.3} {:6.3}   {:6.3} {:6.3}",
                    k("a", i),
                    c.area[i],
                    k("d", i),
                    c.drag[i],
                    k("p", i),
                    c.depth[i]
                );
            }
            println!(
                "  centre {:.3} {:.3} {:.3} / {:.3} {:.3} {:.3}   size {:.3} {:.3} {:.3} / {:.3} {:.3} {:.3}",
                kv("cx"),
                kv("cy"),
                kv("cz"),
                c.center.x,
                c.center.y,
                c.center.z,
                kv("sx"),
                kv("sy"),
                kv("sz"),
                c.size.x,
                c.size.y,
                c.size.z
            );
        }
    }
}

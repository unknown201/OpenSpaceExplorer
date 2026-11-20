//! KSP2 parity for procedural fairing drag cubes (gap T38).
//!
//! The KSP2 parity pilot's `fairings` runs (`tools/ksp2-parity-pilot/run-fairings.ps1`, 2026-11-20): the Mk1 pod
//! under each fairing base, its wall written into the craft file, launched to the pad; every drag cube the
//! part holds once KSP2 has built the panels. `tests/data/ksp2-fairings` keeps the `Fairing` cube of each wall:
//! cylinders, a cone and an ogive closed down to the cap radius, wide walls with and without `IsCapped`, a
//! 6 m wall, and a 2 m cylinder on every base size.
//!
//! What KSP2 renders is the part's whole model: the base (down to the bottom of its own drag cube) and the
//! panels, whose heights count from the part's origin; `IsCapped` adds nothing, and only a wall closed down
//! to the cap radius gets a nose. Areas, centre and size follow the shape; the drag and depth of the faces
//! along the wall too. Not matched: the underside's depth and drag (the base's underside is not flat) and the
//! drag looking down an open wall (the floor inside is not flat either).

use std::collections::HashMap;

use ose_data::fairing::{def, FairingShape};

const WALLS: [(&str, &str); 11] = [
    (
        "cylinder-2",
        include_str!("data/ksp2-fairings/cylinder-2.csv"),
    ),
    (
        "cylinder-2-capped",
        include_str!("data/ksp2-fairings/cylinder-2-capped.csv"),
    ),
    ("cone-2", include_str!("data/ksp2-fairings/cone-2.csv")),
    ("ogive-4", include_str!("data/ksp2-fairings/ogive-4.csv")),
    ("wide-3", include_str!("data/ksp2-fairings/wide-3.csv")),
    (
        "wide-3-capped",
        include_str!("data/ksp2-fairings/wide-3-capped.csv"),
    ),
    ("tall-6", include_str!("data/ksp2-fairings/tall-6.csv")),
    (
        "base0v-cyl-2",
        include_str!("data/ksp2-fairings/base0v-cyl-2.csv"),
    ),
    (
        "base2v-cyl-2",
        include_str!("data/ksp2-fairings/base2v-cyl-2.csv"),
    ),
    (
        "base3v-cyl-2",
        include_str!("data/ksp2-fairings/base3v-cyl-2.csv"),
    ),
    (
        "base4v-cyl-2",
        include_str!("data/ksp2-fairings/base4v-cyl-2.csv"),
    ),
];

fn row(text: &str) -> HashMap<String, String> {
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap().split(',').collect();
    head.iter()
        .map(|h| h.to_string())
        .zip(lines.next().unwrap().split(',').map(str::to_string))
        .collect()
}

#[test]
fn fairing_walls_drag_cubes_match_ksp2s() {
    for (name, text) in WALLS {
        let r = row(text);
        let k = |key: &str| r[key].parse::<f64>().unwrap();
        let base = match name.strip_prefix("base").map(|s| &s[..2]) {
            Some("0v") => "fairing_0v_procedural",
            Some("2v") => "fairing_2v_procedural",
            Some("3v") => "fairing_3v_procedural",
            Some("4v") => "fairing_4v_procedural",
            _ => "fairing_1v_procedural",
        };
        let d = def(base).unwrap();
        // the player's wall: KSP2 appends its nose point to the sections it saves
        let sections: Vec<(f64, f64)> = r["sections"]
            .split(' ')
            .map(|s| {
                let (h, rad) = s.split_once(':').unwrap();
                (h.parse().unwrap(), rad.parse().unwrap())
            })
            .filter(|&(_, rad): &(f64, f64)| rad > 0.0)
            .collect();
        let shape = FairingShape {
            sections,
            capped: r["capped"] == "1",
            pieces: 2,
            ejection_force: 100.0,
        };
        let c = shape.drag_cube(d);
        let closed = shape.wall(d).last().is_some_and(|s| s.1 == 0.0);
        println!(
            "{name}: side {:.3} / {:.3}, end {:.3} / {:.3}, size y {:.3} / {:.3}",
            c.area[0],
            k("a0"),
            c.area[2],
            k("a2"),
            c.size.y,
            k("sy")
        );
        for f in 0..6 {
            let a = k(&format!("a{f}"));
            assert!(
                (c.area[f] - a).abs() / a < 0.01,
                "{name} face {f}: area {:.3} vs KSP2 {a:.3}",
                c.area[f]
            );
        }
        for (got, key) in [
            (c.center.x, "cx"),
            (c.center.y, "cy"),
            (c.center.z, "cz"),
            (c.size.x, "sx"),
            (c.size.y, "sy"),
            (c.size.z, "sz"),
        ] {
            assert!(
                (got - k(key)).abs() < 0.01,
                "{name} {key}: {got:.4} vs KSP2 {:.4}",
                k(key)
            );
        }
        // along the wall (faces 0, 1, 4, 5) and down onto a closed nose: drag and depth
        let mut faces = vec![0, 1, 4, 5];
        if closed {
            faces.push(2);
        }
        for f in faces {
            let (drag, depth) = (k(&format!("d{f}")), k(&format!("p{f}")));
            assert!(
                (c.drag[f] - drag).abs() < 0.015,
                "{name} face {f}: drag {:.3} vs KSP2 {drag:.3}",
                c.drag[f]
            );
            assert!(
                (c.depth[f] - depth).abs() < 0.12,
                "{name} face {f}: depth {:.3} vs KSP2 {depth:.3}",
                c.depth[f]
            );
        }
        // the deepest point seen down the wall: the nose's shoulder, or the floor inside an open wall
        assert!(
            (c.depth[2] - k("p2")).abs() < 0.05,
            "{name}: depth from above {:.3} vs KSP2 {:.3}",
            c.depth[2],
            k("p2")
        );
    }
}

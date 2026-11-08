//! KSP2 parity for the centre of mass as fuel drains (gap T3).
//!
//! KSP2 (`PartOwnerComponent.CalculatePhysicsStats`) puts the vessel's centre of mass at the mass-weighted mean
//! of each part's `CenterOfMass` (its origin plus `coMassOffset`), with each part weighing its dry mass, its
//! resources and its crew (`PartComponent.UpdateMass`), and leaves physicsless parts (`PhysicsMode.None`: radial
//! batteries, flat solar panels) out altogether. This replays the stock Kerbal K1 against that rule with the
//! per-part masses KSP2 itself reported (run 10, `tests/data/ksp2-run10-k1-parts/parts.csv`), with every tank
//! full and drained in steps, and requires OSE's centre of mass within 2 cm of KSP2's along and across the
//! stack.
use std::collections::HashMap;

use ose_math::DVec3;
use ose_sim::structure::Structure;

fn csv(text: &str) -> Vec<HashMap<String, String>> {
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

/// Rotate `v` by the unit quaternion `q` (Unity frame).
fn rotate(q: ose_data::craft::Quat, v: DVec3) -> DVec3 {
    let u = DVec3::new(q.x, q.y, q.z);
    let t = u.cross(v) * 2.0;
    v + t * q.w + u.cross(t)
}

#[test]
fn ksp2_k1_centre_of_mass_tracks_ksp2_as_its_tanks_drain() {
    let crafts: Vec<ose_data::craft::Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let k1 = crafts.iter().find(|c| c.name == "Kerbal K1").unwrap();
    let ksp2 = csv(include_str!("data/ksp2-run10-k1-parts/parts.csv"));
    assert_eq!(ksp2.len(), k1.parts.len());
    // on the pad with its pilot in the pod, as KSP2 had it (the 0.095 t of crew mass is run 10's `green_t`)
    let bodies: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let mut u = ose_sim::Universe::with_bodies(&[kerbin]);
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(k1, &defs, &res);
    let spec = ose_sim::flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface("K1", body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(k1, &defs, &res));
    let full: Vec<f64> = u
        .structure(v)
        .unwrap()
        .net
        .tanks
        .iter()
        .map(|t| t.stored)
        .collect();

    for fraction in [1.0, 0.75, 0.5, 0.25, 0.0] {
        // KSP2's rule on KSP2's own part masses
        let (mut m, mut c) = (0.0, DVec3::ZERO);
        for (part, row) in k1.parts.iter().zip(&ksp2) {
            assert_eq!(row["name"], part.part_id);
            if row["physics_mode"] == "None" {
                continue;
            }
            let num = |k: &str| row[k].parse::<f64>().unwrap();
            let mass = num("dry_t") + num("green_t") + num("resource_t") * fraction;
            let def = defs.iter().find(|d| d.id == part.part_id).unwrap();
            let at = part.local_position + rotate(part.local_rotation, def.mass_offset_m);
            m += mass;
            c = c + at * mass;
        }
        let ksp2_centre = ose_sim::rigid::to_vessel(c * (1.0 / m));

        let mut st = u.world.get_mut::<Structure>(v).unwrap();
        for (tank, units) in st.net.tanks.iter_mut().zip(&full) {
            tank.stored = units * fraction;
        }
        let ose = st.mass_properties();
        let d = ose.centre_m - ksp2_centre;
        println!(
            "fuel {fraction:.2}: KSP2 {:.3} t at y {:.3}, OSE {:.3} t at y {:.3}; off by {:.1} mm along, {:.1} mm across",
            m,
            ksp2_centre.y,
            ose.mass_t,
            ose.centre_m.y,
            d.y * 1e3,
            DVec3::new(d.x, 0.0, d.z).length() * 1e3
        );
        assert!(
            d.y.abs() < 0.02,
            "fuel {fraction}: {:.1} mm along",
            d.y * 1e3
        );
        assert!(
            DVec3::new(d.x, 0.0, d.z).length() < 0.02,
            "fuel {fraction}: across by {d:?}"
        );
    }
}

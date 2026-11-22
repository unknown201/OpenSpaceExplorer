//! OSE side of the EVA 1:1 comparison with KSP2 (the parity pilot's `evaorbit` and `evapad` scenarios,
//! `tools/ksp2-parity-pilot/EvaCompare.cs`): the run-1 craft in a 100 km orbit or on the pad, the first Kerbal
//! out holding the hatch ladder 10 s, let go, 5 s, then the jetpack (orbit) or a walk (pad). Writes the same
//! columns as KSP2's `eva-compare.csv` to `target/eva-compare-{orbit,pad}.csv`; run with `--ignored`.
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{bodies, flight, structure::Structure, Universe, TICKS_PER_SECOND};
use std::io::Write;

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;

fn craft_universe(orbit: bool) -> (Universe, Entity) {
    let mut kerbin = serde_json::from_str::<Vec<ose_data::CelestialBody>>(include_str!(
        "../../../assets/own/kerbol-system.json"
    ))
    .unwrap()
    .into_iter()
    .find(|b| b.id == "Kerbin")
    .unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let value: serde_json::Value = serde_json::from_str(
        include_str!("../../../tools/ksp2-parity-pilot/craft-rotation.json")
            .trim_start_matches('\u{feff}'),
    )
    .unwrap();
    let craft = ose_importer::craft::craft_from_value(&value).unwrap();
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let mu = u.world.get::<bodies::Body>(body).unwrap().mu;
    let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface("eva compare", body, spec, -0.0972, -74.5577);
    u.set_structure(v, Structure::build(&craft, &defs, &res));
    if orbit {
        let r = 700_000.0;
        let mut p = u.powered(v).unwrap().clone();
        p.r = DVec3::new(r, 0.0, 0.0);
        p.v = DVec3::new(0.0, (mu / r).sqrt(), 0.0);
        p.landed = false;
        p.attitude = flight::Attitude::Free;
        u.set_powered(v, p);
    }
    for _ in 0..(3 * TICKS_PER_SECOND) {
        u.step();
    }
    (u, v)
}

fn run(orbit: bool) {
    let (mut u, v) = craft_universe(orbit);
    let jeb = u.go_eva(v, None).unwrap();
    let path = format!(
        "{}/../../target/eva-compare-{}.csv",
        env!("CARGO_MANIFEST_DIR"),
        if orbit { "orbit" } else { "pad" }
    );
    let mut w = std::fs::File::create(&path).unwrap();
    writeln!(
        w,
        "phase,t,x,y,z,dist,dvx,dvy,dvz,on_ladder,landed,deck,mass_t,alt_m,jetpack"
    )
    .unwrap();
    let mut t = 0.0;
    let mut basis = flight::vessel_basis(u.powered(v).unwrap());
    let mut hold = |u: &mut Universe, phase: &str, seconds: f64| {
        for _ in 0..(seconds * TICKS_PER_SECOND as f64).round() as u64 {
            u.step();
            t += DT;
            // the Kerbal too, floating free
            let Some(pk) = u.powered(jeb).cloned() else {
                continue;
            };
            // the craft goes back on rails once it has coasted 10 s with nobody holding on
            let (cr, cv) = match u.powered(v) {
                Some(pv) => {
                    basis = flight::vessel_basis(pv);
                    (pv.r, pv.v)
                }
                None => u.vessel(v).unwrap().2.state_at(u.ut()),
            };
            let (x, y, z) = basis;
            let pk = &pk;
            let d = pk.r - cr;
            let dv = pk.v - cv;
            let alt = pk.r.length() - 600_000.0;
            writeln!(
                w,
                "{phase},{t:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{:.6},{alt:.3},{}",
                d.dot(x),
                d.dot(y),
                d.dot(z),
                d.length(),
                dv.dot(x),
                dv.dot(y),
                dv.dot(z),
                u.on_ladder(jeb).is_some(),
                pk.landed,
                pk.deck.is_some(),
                pk.mass_t,
                pk.groups.rcs
            )
            .unwrap();
        }
    };
    hold(&mut u, "ladder", 10.0);
    assert!(u.set_eva(jeb, false, true));
    hold(&mut u, "letgo", 5.3);
    if orbit {
        u.action_group(jeb, ose_sim::ActionGroup::Rcs, Some(true));
        hold(&mut u, "pack-on", 1.0);
        u.set_translate(jeb, DVec3::new(0.0, 1.0, 0.0));
        hold(&mut u, "up", 1.0);
        u.set_translate(jeb, DVec3::ZERO);
        hold(&mut u, "coast1", 3.0);
        u.set_translate(jeb, DVec3::new(0.0, 0.0, 1.0));
        hold(&mut u, "forward", 1.0);
        u.set_translate(jeb, DVec3::ZERO);
        hold(&mut u, "coast2", 3.0);
    } else {
        let k = u.powered(jeb).unwrap();
        let up = k.r.normalized();
        let away = (k.r - u.position_of(v).unwrap()).normalized();
        let out = (away - up * away.dot(up)).normalized();
        u.set_controls(jeb, 1.0, flight::Attitude::Fixed(out));
        hold(&mut u, "walk", 3.0);
        u.set_controls(jeb, 0.0, flight::Attitude::Free);
        hold(&mut u, "stand", 3.0);
    }
    eprintln!("wrote {path}");
}

#[test]
#[ignore]
fn eva_compare_orbit() {
    run(true);
}

#[test]
#[ignore]
fn eva_compare_pad() {
    run(false);
}

//! KSP2 parity for the EVA jetpack in free flight (gap T21), from the pilot's `jetpack` scenario rerun
//! (2026-11-08 18:56Z, `reference/ksp2-jetpack`): a Kerbal out of the pod in a 100 km orbit, moved 50 m clear of
//! the craft, holds "up" for 2 s, coasts 2 s, holds "forward" for 2 s. With gravity (the trace's own g column)
//! taken out, KSP2's Kerbal speeds up at 0.250 kN / mass along its own axes, burns 2.124 g of monopropellant per
//! tick, and does not turn:
//! - up: 5.216 m/s for 212.44 g (100 ticks of fuel, 99 of push: the force lands a tick after the fuel)
//! - forward: 5.280 m/s for 212.44 g (100 and 100), at right angles to "up"
//! - mass 0.095000 -> 0.094575 t
use bevy_ecs::entity::Entity;
use ose_math::DVec3;
use ose_sim::{bodies, flight, structure::Structure, Universe, TICKS_PER_SECOND};

const DT: f64 = 1.0 / TICKS_PER_SECOND as f64;
const KSP2_UP_DV: f64 = 5.2161;
const KSP2_FORWARD_DV: f64 = 5.2801;
const KSP2_PHASE_FUEL_T: f64 = 212.441e-6;
const KSP2_MASS_AFTER_T: f64 = 0.094_575_118_552_112_65;

fn kerbal_clear_of_the_pod() -> (Universe, Entity, f64) {
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
    let v = u.spawn_powered_on_surface("orbiter", body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(&craft, &defs, &res));
    let r = 700_000.0;
    let speed = (mu / r).sqrt();
    let mut p = u.powered(v).unwrap().clone();
    p.r = DVec3::new(r, 0.0, 0.0);
    p.v = DVec3::new(0.0, speed, 0.0);
    p.landed = false;
    u.set_powered(v, p);
    let jeb = u.go_eva(v, None).unwrap();
    assert_eq!(u.grab_ladder(jeb), Ok(false), "lets go of the ladder");
    // 50 m ahead along the same orbit, as the pilot puts it, with its pack still and its attitude free
    let ahead = 50.0 / r;
    let mut k = u.powered(jeb).unwrap().clone();
    k.r = DVec3::new(r * ahead.cos(), r * ahead.sin(), 0.0);
    k.v = DVec3::new(-speed * ahead.sin(), speed * ahead.cos(), 0.0);
    k.angvel = DVec3::ZERO;
    k.roll_rate = 0.0;
    k.attitude = flight::Attitude::Free;
    u.set_powered(jeb, k);
    (u, jeb, mu)
}

/// Hold a jetpack translation for `ticks`, returning the velocity change with gravity taken out and the
/// propellant burned.
fn hold(u: &mut Universe, jeb: Entity, mu: f64, input: DVec3, ticks: u64) -> (DVec3, f64) {
    assert!(u.set_translate(jeb, input));
    let m0 = u.powered(jeb).unwrap().mass_t;
    let mut dv = DVec3::ZERO;
    for _ in 0..ticks {
        let before = u.powered(jeb).unwrap().clone();
        u.step();
        let after = u.powered(jeb).unwrap();
        // gravity at the mid point of the tick, as a symplectic step sees it
        let mid = (before.r + after.r) * 0.5;
        let g = mid * (-mu / mid.length().powi(3));
        dv = dv + (after.v - before.v) - g * DT;
    }
    assert!(u.set_translate(jeb, DVec3::ZERO));
    (dv, m0 - u.powered(jeb).unwrap().mass_t)
}

fn coast(u: &mut Universe, ticks: u64) {
    for _ in 0..ticks {
        u.step();
    }
}

#[test]
fn ksp2_jetpack_up_and_forward_give_the_same_push_and_fuel() {
    let (mut u, jeb, mu) = kerbal_clear_of_the_pod();
    let m0 = u.powered(jeb).unwrap().mass_t;
    println!("kerbal {m0:.6} t (KSP2 0.095000)");
    assert!((m0 - 0.095).abs() < 1e-6, "same Kerbal and full pack");
    coast(&mut u, TICKS_PER_SECOND);
    let ticks = 2 * TICKS_PER_SECOND;
    let (up, up_fuel) = hold(&mut u, jeb, mu, DVec3::new(0.0, 1.0, 0.0), ticks);
    coast(&mut u, ticks);
    let (fwd, fwd_fuel) = hold(&mut u, jeb, mu, DVec3::new(0.0, 0.0, 1.0), ticks);
    coast(&mut u, ticks);
    let p = u.powered(jeb).unwrap();
    println!(
        "up {:.4} m/s {:.2} g (KSP2 {KSP2_UP_DV} m/s, {:.2} g); forward {:.4} m/s {:.2} g (KSP2 {KSP2_FORWARD_DV}); \
         angle {:.2} deg; mass after {:.6} t (KSP2 {KSP2_MASS_AFTER_T:.6}); spin {:.2e} rad/s",
        up.length(),
        up_fuel * 1e6,
        KSP2_PHASE_FUEL_T * 1e6,
        fwd.length(),
        fwd_fuel * 1e6,
        up.normalized().dot(fwd.normalized()).acos().to_degrees(),
        p.mass_t,
        p.angvel.length(),
    );
    // fuel: 0.25 kN at Isp 240 each tick of input, in both games
    assert!((up_fuel / KSP2_PHASE_FUEL_T - 1.0).abs() < 0.005, "up fuel");
    assert!(
        (fwd_fuel / KSP2_PHASE_FUEL_T - 1.0).abs() < 0.005,
        "forward fuel"
    );
    assert!((p.mass_t - KSP2_MASS_AFTER_T).abs() < 1e-6, "mass after");
    // push: KSP2's force lands a tick after its fuel at the start, so allow one tick (0.026 m/s)
    assert!((up.length() - KSP2_UP_DV).abs() < 0.06, "up dv");
    assert!((fwd.length() - KSP2_FORWARD_DV).abs() < 0.06, "forward dv");
    assert!(
        up.normalized().dot(fwd.normalized()).abs() < 1e-3,
        "up and forward are the Kerbal's own perpendicular axes"
    );
    assert!(
        p.angvel.length() < 1e-9,
        "the pack does not turn the Kerbal"
    );
}

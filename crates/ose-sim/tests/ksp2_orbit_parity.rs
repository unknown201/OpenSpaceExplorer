//! KSP2 parity for orbits, time warp and SOI changes (gap T8) and a 100 m/s prograde burn (gap T9), from the
//! pilot's `warp` and `burn` scenarios (`tools/ksp2-parity-pilot/OrbitScenarios.cs`, traces captured
//! 2026-11-08 18:11Z and 18:17Z in `reference/ksp2-warp-soi` and `reference/ksp2-burn-100`). The KSP2 numbers
//! below are copied from those traces, at their full printed precision.
#![allow(clippy::excessive_precision)]
use bevy_ecs::entity::Entity;
use ose_math::{DVec3, Elements};
use ose_sim::{bodies, flight, structure::Structure, vessels, Universe, TICKS_PER_SECOND};

fn system() -> Vec<ose_data::CelestialBody> {
    serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap()
}

fn kerbin_alone() -> ose_data::CelestialBody {
    let mut b = system().into_iter().find(|b| b.id == "Kerbin").unwrap();
    b.parent = None;
    b.orbit = None;
    b.soi_radius_m = None;
    b
}

fn body_of(u: &Universe, e: Entity) -> Entity {
    u.world.get::<bodies::ReferenceBody>(e).unwrap().0
}

#[test]
fn ksp2_warp_is_capped_by_height_as_ksp2_caps_it() {
    let mut u = Universe::with_bodies(&system());
    let kerbin = u.body_entity("Kerbin").unwrap();
    let mun = u.body_entity("Mun").unwrap();
    // KSP2 trace: at 100 km over Kerbin the x1000 request ran at x100 (`warp` column 100, UT advancing 100.9 s
    // per real second); 1,000x needs 50 km over the 70 km atmosphere
    let low = u.spawn_vessel_in_circular_orbit("low", kerbin, 100_000.0);
    u.set_warp_focus(vec![low]);
    u.set_warp(1_000);
    assert_eq!(u.warp(), 100, "KSP2 holds 100x at 100 km");
    for (altitude, allowed) in [
        (119_000.0, 100),
        (121_000.0, 1_000),
        (221_000.0, 10_000),
        (571_000.0, 100_000),
        (4_071_000.0, 1_000_000),
    ] {
        let v = u.spawn_vessel_in_circular_orbit("probe", kerbin, altitude);
        u.set_warp_focus(vec![v]);
        u.set_warp(1_000_000);
        assert_eq!(u.warp(), allowed, "at {altitude} m over Kerbin");
    }
    // airless: measured from the highest terrain (none loaded here), so the Mun orbit's 100 km periapsis
    // allows 1,000x but not 10,000x
    let m = u.spawn_vessel_in_circular_orbit("mun", mun, 100_000.0);
    u.set_warp_focus(vec![m]);
    u.set_warp(1_000_000);
    assert_eq!(u.warp(), 1_000);
    // no focused vessel: no height limit
    u.set_warp_focus(vec![]);
    u.set_warp(1_000_000);
    assert_eq!(u.warp(), 1_000_000);
}

#[test]
fn ksp2_a_100_km_orbit_holds_its_apsides_through_100x_warp() {
    let mut u = Universe::with_bodies(&[kerbin_alone()]);
    let kerbin = u.body_entity("Kerbin").unwrap();
    let v = u.spawn_vessel_in_circular_orbit("orbiter", kerbin, 100_000.0);
    u.set_warp_focus(vec![v]);
    let before = u.vessel(v).unwrap().2;
    // KSP2: 10 s at 1x, 60 s at 100x and 30 s more at "1,000x" (held at 100x), 10 s at 1x: UT 504.6 -> 9,597.8,
    // apsides 100,000.07 / 99,999.91 m on every row
    u.set_warp(1);
    for _ in 0..(10 * TICKS_PER_SECOND) {
        u.step();
    }
    u.set_warp(1_000);
    let t0 = u.ut();
    while u.ut() < t0 + 9_000.0 {
        u.step();
    }
    let after = u.vessel(v).unwrap().2;
    let r = 600_000.0;
    let (ap0, pe0) = (before.apoapsis() - r, before.periapsis() - r);
    let (ap1, pe1) = (after.apoapsis() - r, after.periapsis() - r);
    println!(
        "apsides {ap0:.2}/{pe0:.2} -> {ap1:.2}/{pe1:.2} m after {:.0} s",
        u.ut()
    );
    assert!((ap1 - ap0).abs() < 0.1 && (pe1 - pe0).abs() < 0.1);
    let (r1, _) = after.state_at(u.ut());
    assert!((r1.length() - r - 100_000.0).abs() < 0.5);
}

/// KSP2's Mun orbit, exactly as the trace reports it after the teleport: periapsis 100 km, apoapsis 2,800 km
/// (beyond the 2,429.6 km SOI), at periapsis at UT 9,597.76 (the trace's first Mun row, 8 s later at
/// 100,018.9 m, puts periapsis there).
fn ksp2_mun_orbit(epoch: f64) -> Elements {
    Elements {
        semi_major_axis: 1_649_967.165_220_105,
        eccentricity: 0.818_178_202_276_271_9,
        inclination_deg: 0.0,
        longitude_of_ascending_node_deg: 0.0,
        argument_of_periapsis_deg: 0.0,
        mean_anomaly_at_epoch: 0.0,
        epoch,
    }
}

#[test]
fn ksp2_leaving_the_mun_changes_soi_when_ksp2_does() {
    let mut u = Universe::with_bodies(&system());
    let kerbin = u.body_entity("Kerbin").unwrap();
    let mun = u.body_entity("Mun").unwrap();
    let mu = u.world.get::<bodies::Body>(mun).unwrap().mu;
    let soi = u
        .world
        .get::<bodies::Body>(mun)
        .unwrap()
        .soi_radius_m
        .unwrap();
    // the epoch from the trace's first Mun row (UT 9,605.7598, 100,018.948 m, outbound)
    let probe = ose_math::Orbit::from_elements(mu, ksp2_mun_orbit(0.0));
    let since_pe =
        probe.time_to_true_anomaly(probe.true_anomaly_at_radius(300_018.947_827_991), 0.0);
    let epoch = 9_605.759_785_294_533 - since_pe;
    assert!((epoch - 9_597.76).abs() < 0.01, "epoch {epoch}");
    u.set_tick(ose_sim::Tick((9_597.76 * TICKS_PER_SECOND as f64) as u64));
    let v = u.spawn_vessel_with_elements("munar", mun, ksp2_mun_orbit(epoch));
    u.set_warp_focus(vec![v]);
    u.set_warp(100);
    while body_of(&u, v) == mun && u.ut() < 30_000.0 {
        u.step();
    }
    let exit = u.ut();
    // KSP2 (sampled every 20 s of game time at 100x): still in the Mun's SOI at UT 22,205.76 (2,229.4 km up),
    // around Kerbin at 22,225.51. Its own elements put the crossing at 22,207.4.
    let ksp2_r_last = 200_000.0 + 2_229_403.567_273_565;
    let ksp2_last_ut = 22_205.759_503_662_586;
    let crossing = epoch + probe.time_to_true_anomaly(probe.true_anomaly_at_radius(soi), 0.0);
    let ours_at_last =
        epoch + probe.time_to_true_anomaly(probe.true_anomaly_at_radius(ksp2_r_last), 0.0);
    println!(
        "SOI exit UT {exit:.2} (analytic {crossing:.2}); KSP2 between {ksp2_last_ut:.2} and 22225.51; \
         our time at KSP2's last Mun radius {ours_at_last:.2}"
    );
    assert_eq!(body_of(&u, v), kerbin, "left for Kerbin");
    assert!(
        exit > ksp2_last_ut && exit <= 22_225.51,
        "inside KSP2's bracket"
    );
    assert!((exit - crossing).abs() < 1.0 / TICKS_PER_SECOND as f64 + 1e-6);
    assert!(
        (ours_at_last - ksp2_last_ut).abs() < 1.0,
        "same timing within 1 s"
    );
    // the Kerbin orbit it leaves on: KSP2 sma 40,683 km, e 0.6552, Pe 13,427 km (Mun phase and frame agree)
    let o = u.vessel(v).unwrap().2;
    let (sma, ecc, pe) = (
        o.elements.semi_major_axis,
        o.elements.eccentricity,
        o.periapsis() - 600_000.0,
    );
    println!("after the SOI change: sma {sma:.0} m, e {ecc:.4}, Pe {pe:.0} m (KSP2 40683414, 0.6552, 13427314)");
    assert!((sma / 40_683_414.0 - 1.0).abs() < 0.01, "sma {sma}");
    assert!((ecc - 0.655_208).abs() < 0.01, "ecc {ecc}");
    assert!((pe / 13_427_314.0 - 1.0).abs() < 0.01, "pe {pe}");
}

/// The pilot's orbital craft (`craft-rotation.json`, 4.7425 t in KSP2), staged in a 100 km circular orbit with
/// its nose prograde.
fn rotation_craft_in_orbit() -> (Universe, Entity) {
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
    let mut u = Universe::with_bodies(&[kerbin_alone()]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let v = u.spawn_powered_on_surface("orbiter", body, spec, 0.0, 0.0);
    u.set_structure(v, Structure::build(&craft, &defs, &res));
    let b = u.world.get::<bodies::Body>(body).unwrap().clone();
    let r = b.radius_m + 100_000.0;
    let mut p = u.powered(v).unwrap().clone();
    p.r = DVec3::new(r, 0.0, 0.0);
    p.v = DVec3::new(0.0, (b.mu / r).sqrt(), 0.0);
    p.landed = false;
    p.facing = DVec3::new(0.0, 1.0, 0.0);
    u.set_powered(v, p);
    (u, v)
}

// KSP2 `burn` (craft at 2,246.14 m/s, 4.7425 t, SAS prograde): the Swivel burns 1.370 kg and adds 0.907 m/s
// per 20 ms tick from its first tick at full thrust, no spool-up. Thrust starts on the tick ending t = 0.20 s and
// the speed is 100 m/s up on the tick ending t = 2.38 s: 110 ticks (2.20 s), 0.1507 t, orbit 240,878 x 99,999.9 m.
// The pilot's throttle reaches the engine 4 ticks late both ways (nothing burns for the first 4 ticks after
// "full throttle", and it burns 4 more after "cut"), so the logged burn is 2.28 s long and it ends at 4.5877 t
// on a 246,818 x 99,999.9 m orbit.
const KSP2_THRUST_TICKS: u64 = 110;
const KSP2_FUEL_T: f64 = 4.742_499_994_337_558 - 4.591_773_187_608_871_4;
const KSP2_AP_AT_100_M: f64 = 240_878.476_233_310_07;
const KSP2_LATE_TICKS: u64 = 4;
const KSP2_AP_FINAL_M: f64 = 246_818.119_632_908_26;
const KSP2_PE_M: f64 = 99_999.936_275_024_08;

fn apsides(u: &Universe, v: Entity) -> (f64, f64) {
    let p = u.powered(v).or(u.dormant(v)).unwrap();
    let mu = 3_531_600_000_000.0;
    let o = ose_math::Orbit::from_state_vectors(mu, p.r, p.v, u.ut());
    (o.apoapsis() - 600_000.0, o.periapsis() - 600_000.0)
}

#[test]
fn ksp2_a_100_mps_prograde_burn_takes_the_same_ticks_fuel_and_orbit() {
    let (mut u, v) = rotation_craft_in_orbit();
    let p0 = u.powered(v).unwrap().clone();
    let (v0, m0) = (p0.v.length(), p0.mass_t);
    println!("start: {v0:.2} m/s, {m0:.4} t (KSP2 2246.14, 4.7425)");
    assert!((m0 - 4.7425).abs() < 0.01, "same craft mass");
    u.stage(v);
    u.set_controls(v, 1.0, flight::Attitude::OrbitPrograde);
    let mut ticks = 0;
    while u.powered(v).unwrap().v.length() < v0 + 100.0 && ticks < 30 * TICKS_PER_SECOND {
        u.step();
        ticks += 1;
    }
    let fuel = m0 - u.powered(v).unwrap().mass_t;
    let (ap, pe) = apsides(&u, v);
    println!(
        "100 m/s up after {ticks} ticks (KSP2 {KSP2_THRUST_TICKS}), fuel {fuel:.4} t (KSP2 {KSP2_FUEL_T:.4}), orbit {ap:.0} x {pe:.0} m (KSP2 {KSP2_AP_AT_100_M:.0} x {KSP2_PE_M:.0})"
    );
    assert!(ticks.abs_diff(KSP2_THRUST_TICKS) <= 2, "ticks");
    assert!((fuel / KSP2_FUEL_T - 1.0).abs() < 0.015, "fuel");
    assert!((ap / KSP2_AP_AT_100_M - 1.0).abs() < 0.01, "apoapsis");
    assert!((pe / KSP2_PE_M - 1.0).abs() < 0.01, "periapsis");
    // the pilot's late cutoff: 4 more ticks of thrust give KSP2's final orbit too
    for _ in 0..KSP2_LATE_TICKS {
        u.step();
    }
    u.set_controls(v, 0.0, flight::Attitude::OrbitPrograde);
    let (ap, pe) = apsides(&u, v);
    println!("after {KSP2_LATE_TICKS} late ticks: {ap:.0} x {pe:.0} m (KSP2 {KSP2_AP_FINAL_M:.0})");
    assert!((ap / KSP2_AP_FINAL_M - 1.0).abs() < 0.01, "final apoapsis");
}

#[test]
fn ksp2_a_100_mps_prograde_node_flown_by_its_timer_ends_on_ksp2s_orbit() {
    let (mut u, v) = rotation_craft_in_orbit();
    u.stage(v);
    u.set_controls(v, 0.0, flight::Attitude::OrbitPrograde);
    let m0 = u.powered(v).unwrap().mass_t;
    let node = vessels::ScheduledBurn {
        ut: u.ut() + 60.0,
        prograde_mps: 100.0,
        normal_mps: 0.0,
        radial_mps: 0.0,
    };
    assert!(u.schedule_burn(v, node));
    for _ in 0..(120 * TICKS_PER_SECOND) {
        u.step();
        if u.scheduled_burn(v).is_none() || u.burn_stopped(v).is_some() {
            break;
        }
    }
    assert!(u.burn_stopped(v).is_none(), "{:?}", u.burn_stopped(v));
    assert!(u.scheduled_burn(v).is_none(), "flown");
    let p = u.powered(v).or(u.dormant(v)).unwrap();
    let fuel = m0 - p.mass_t;
    let (ap, pe) = apsides(&u, v);
    println!(
        "node: fuel {fuel:.4} t (KSP2 {KSP2_FUEL_T:.4}), orbit {ap:.0} x {pe:.0} m (KSP2 {KSP2_AP_AT_100_M:.0} x {KSP2_PE_M:.0})"
    );
    // T9: resulting apsides within 1% of KSP2's at 100 m/s
    assert!((ap / KSP2_AP_AT_100_M - 1.0).abs() < 0.01, "apoapsis");
    assert!((pe / KSP2_PE_M - 1.0).abs() < 0.01, "periapsis");
    assert!((fuel / KSP2_FUEL_T - 1.0).abs() < 0.03, "fuel");
}

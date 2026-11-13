//! KSP2 parity for solar power (gap T22), from the original's code: a panel makes its rating x the sun's flux
//! there over `SolarInsolationAtHome` (`PartComponentModule_SolarPanel.CalculateVesselSolarLOS`, flux
//! `StarLuminosity / (4 pi d^2)` from `PartOwnerComponent`), and only the body the craft is at can shade it
//! (`CalculateOcclusionPerStar`).
use ose_math::DVec3;
use ose_sim::{bodies, Universe};

fn system() -> (Universe, Vec<ose_data::CelestialBody>) {
    let defs: Vec<ose_data::CelestialBody> =
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap();
    (Universe::with_bodies(&defs), defs)
}

#[test]
fn ksp2_panels_at_kerbin_make_97_percent_of_their_rating_by_day_and_none_by_night() {
    let (u, defs) = system();
    let kerbin = u.body_entity("Kerbin").unwrap();
    let ut = 0.0;
    let k = bodies::star_space_state(&u.world, kerbin, ut).0;
    let to_sun = (k * -1.0).normalized();
    let r = to_sun * 700_000.0;
    let luminosity = defs
        .iter()
        .find(|d| d.id == "Kerbol")
        .and_then(|d| d.star_luminosity)
        .unwrap();
    let d = (k + r).length();
    let expected = luminosity / (4.0 * std::f64::consts::PI * d * d) / 1360.0;
    let day = u.sunlight(kerbin, r, ut);
    println!("day side: {day:.4} of the rating (KSP2 {expected:.4})");
    assert!((day - expected).abs() < 1e-9);
    assert!((day - 0.968).abs() < 0.002, "{day}");
    assert_eq!(u.sunlight(kerbin, r * -1.0, ut), 0.0, "night side");
    // KSP2's shadow is a cylinder behind the planet: 40 km up, 500 km off the sun line, it is dark on the night
    // side of the terminator and lit on the day side; 650 km off the line it is outside the shadow either way
    let east = DVec3::new(0.0, 0.0, 1.0).cross(to_sun).normalized();
    assert_eq!(
        u.sunlight(kerbin, east * 500_000.0 - to_sun * 400_000.0, ut),
        0.0
    );
    assert!(u.sunlight(kerbin, east * 500_000.0 + to_sun * 400_000.0, ut) > 0.9);
    assert!(u.sunlight(kerbin, east * 650_000.0 - to_sun * 400_000.0, ut) > 0.9);
}

#[test]
fn ksp2_a_craft_at_the_mun_is_not_shaded_by_kerbin() {
    let (u, _) = system();
    let (kerbin, mun) = (
        u.body_entity("Kerbin").unwrap(),
        u.body_entity("Mun").unwrap(),
    );
    let ut = 0.0;
    let k = bodies::star_space_state(&u.world, kerbin, ut).0;
    let m = bodies::star_space_state(&u.world, mun, ut).0;
    // a point 2,000 km behind Kerbin, straight down its shadow, flown from the Mun's sphere of influence
    let behind_kerbin = k + k.normalized() * 2_000_000.0;
    let r = behind_kerbin - m;
    let lit = u.sunlight(mun, r, ut);
    println!("behind Kerbin, at the Mun: {lit:.3}");
    assert!(
        lit > 0.9,
        "KSP2 tests only the Mun for a craft at the Mun ({lit})"
    );
    // the Mun itself still shades its own night side
    let mun_night = (m.normalized()) * 250_000.0;
    assert_eq!(u.sunlight(mun, mun_night, ut), 0.0);
}

#[test]
fn measured_native_orbital_shadow_and_flux_match_at_four_phases() {
    let (u, _) = system();
    let kerbin = u.body_entity("Kerbin").unwrap();
    let evidence: serde_json::Value =
        serde_json::from_str(include_str!("data/services-live.json")).unwrap();
    for case in evidence["solar_orbit_cases"].as_array().unwrap() {
        let vector = |key: &str| {
            let a = case[key].as_array().unwrap();
            DVec3::new(
                a[0].as_f64().unwrap(),
                a[1].as_f64().unwrap(),
                a[2].as_f64().unwrap(),
            )
        };
        let ut = case["ut"].as_f64().unwrap();
        let native_r = vector("r_body_m");
        let native_sun = vector("star_body_m").normalized();
        // Native celestial coordinates and our inertial coordinates differ. Preserve the measured
        // sunward projection and perpendicular distance, which determine the spherical shadow.
        let sun = -bodies::star_space_state(&u.world, kerbin, ut)
            .0
            .normalized();
        let across = sun.cross(DVec3::new(0., 0., 1.)).normalized();
        let along = native_r.dot(native_sun);
        let r = sun * along + across * (native_r - native_sun * along).length();
        let measured = case["illumination_scale"].as_f64().unwrap();
        let actual = u.sunlight(kerbin, r, ut);
        if case["blocker"] == "Kerbin" {
            assert_eq!(actual, 0.);
        } else {
            // Environment event and terminal sample straddle screenshot capture; do not pretend they
            // are synchronous or compare native and OSE camera axes.
            assert!((actual - measured).abs() < 0.0002, "{actual} vs {measured}");
        }
    }
}

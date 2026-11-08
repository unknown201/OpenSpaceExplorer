//! Verified matching subsets. Run tools/check_interaction_parity.py for the full
//! strict comparison, which also reports the currently divergent categories.
use ose_client::{interaction, vab};
use serde_json::Value;

fn reference() -> Value {
    serde_json::from_str(include_str!("../testdata/ksp2-interactions.json")).unwrap()
}

#[test]
fn throttle_ramp_clamps_and_presets_match_recorded_ksp2_equations() {
    let fixture = reference();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["category"] == "throttle")
        .collect();
    assert!(cases.len() >= 20, "throttle coverage was removed");
    for case in cases {
        let input = &case["input"];
        let actual = match case["op"].as_str().unwrap() {
            "throttle" => interaction::throttle_step(
                input["current"].as_f64().unwrap(),
                input["axis"].as_f64().unwrap(),
                input["dt"].as_f64().unwrap(),
            ),
            "preset" => interaction::throttle_preset(input["full"].as_bool().unwrap()),
            op => panic!("unknown throttle reference operation {op}"),
        };
        assert!(
            (actual - case["expected"].as_f64().unwrap()).abs() < 1e-6,
            "{}",
            case["id"]
        );
    }
}

#[test]
fn radial_symmetry_counts_and_angles_match_recorded_ksp2_equations() {
    let fixture = reference();
    let counts: Vec<u32> = fixture["symmetry"]["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap() as u32)
        .collect();
    assert_eq!(vab::SYMMETRY.as_slice(), counts);
    for (count, expected) in counts
        .iter()
        .zip(fixture["symmetry"]["angles_deg"].as_array().unwrap())
    {
        assert!(
            (vab::radial_angle_radians(1, *count).to_degrees() - expected.as_f64().unwrap()).abs()
                < 1e-9
        );
    }
}

/// T27-T31: the flight, map and VAB cameras turn and zoom by KSP2's recorded equations.
#[test]
fn camera_orbit_and_zoom_match_recorded_ksp2_equations() {
    let fixture = reference();
    let mut checked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let category = case["category"].as_str().unwrap();
        if !category.ends_with("_camera") {
            continue;
        }
        let input = &case["input"];
        let n = |k: &str| input[k].as_f64().unwrap();
        let fine = input["fine"].as_bool().unwrap_or(false);
        let actual = match (
            case["op"].as_str().unwrap(),
            input["context"].as_str().unwrap(),
        ) {
            ("orbit", "flight") => interaction::flight_orbit_degrees(n("pixels"), n("dt"), fine),
            ("orbit", "map") => interaction::map_orbit_degrees(n("pixels"), n("pitch"), fine),
            ("orbit", "vab") => interaction::vab_orbit_radians(n("pixels"), fine).to_degrees(),
            ("zoom", "flight") => interaction::flight_zoom(n("distance"), n("axis"), fine),
            ("zoom", "map") => interaction::map_zoom(n("distance"), n("axis")),
            ("zoom", "vab") => interaction::vab_zoom(
                n("distance"),
                n("axis"),
                input["shift"].as_bool().unwrap(),
                fine,
            ),
            other => panic!("unknown camera case {other:?}"),
        };
        let expected = case["expected"].as_f64().unwrap();
        assert!(
            (actual - expected).abs() <= 1e-6 + 2e-6 * expected.abs(),
            "{}: KSP2 {expected}, OSE {actual}",
            case["id"]
        );
        checked += 1;
    }
    assert!(checked >= 40, "camera coverage was removed ({checked})");
}

/// T32: the warp ladder, warp-to's choice of rate and the automatic drop to 1x follow KSP2's `TimeWarp`.
#[test]
fn warp_ladder_warp_to_and_drop_match_recorded_ksp2_equations() {
    use ose_client::kerbin_time;
    let fixture = reference();
    let mut checked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let ok = match case["op"].as_str().unwrap() {
            "ladder" => {
                let want: Vec<u64> = case["expected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap())
                    .collect();
                kerbin_time::WARP_LADDER
                    .iter()
                    .map(|&w| w as u64)
                    .collect::<Vec<_>>()
                    == want
            }
            "warp_to" => {
                kerbin_time::warp_toward(input["remaining"].as_f64().unwrap()) as u64
                    == case["expected"].as_u64().unwrap()
            }
            "drop" => {
                kerbin_time::warp_drop(
                    input["rate"].as_f64().unwrap() as u32,
                    input["remaining"].as_f64().unwrap(),
                ) == case["expected"].as_bool().unwrap()
            }
            _ => continue,
        };
        assert!(ok, "{}", case["id"]);
        checked += 1;
    }
    assert!(checked >= 30, "warp coverage was removed ({checked})");
}

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

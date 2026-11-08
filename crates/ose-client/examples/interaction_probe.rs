//! Evaluate production OSE handlers against externally recorded inputs.
//! No expected values are used by this probe.
use ose_client::{input::Held, interaction, kerbin_time, vab};
use serde_json::{json, Value};

fn number(input: &Value, key: &str) -> f64 {
    input[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing numeric input {key}"))
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: interaction_probe FIXTURE.json");
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let rows: Vec<Value> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let input = &case["input"];
            let fine = input["fine"].as_bool().unwrap_or(false);
            let actual = match case["op"].as_str().unwrap() {
                "throttle" => {
                    let mut held = Held::default();
                    match number(input, "axis") {
                        v if v > 0.0 => held.down("ShiftLeft"),
                        v if v < 0.0 => held.down("ControlLeft"),
                        _ => {}
                    }
                    json!(interaction::throttle_step(
                        number(input, "current"),
                        held.throttle(),
                        number(input, "dt")
                    ))
                }
                "preset" => json!(interaction::throttle_preset(
                    input["full"].as_bool().unwrap()
                )),
                "rotation" => json!(interaction::rotation_step_degrees(
                    input["snap"].as_bool().unwrap(),
                    fine
                )),
                "orbit" => json!(match input["context"].as_str().unwrap() {
                    "flight" => interaction::flight_orbit_degrees(
                        number(input, "pixels"),
                        number(input, "dt"),
                        fine
                    ),
                    "map" => interaction::map_orbit_degrees(
                        number(input, "pixels"),
                        number(input, "pitch"),
                        fine
                    ),
                    "vab" =>
                        interaction::vab_orbit_radians(number(input, "pixels"), fine).to_degrees(),
                    other => panic!("unknown orbit context {other}"),
                }),
                "zoom" => {
                    // Unity's positive zoom axis zooms in, as interaction's notches do.
                    let (axis, distance) = (number(input, "axis"), number(input, "distance"));
                    json!(match input["context"].as_str().unwrap() {
                        "flight" => interaction::flight_zoom(distance, axis, fine),
                        "map" => interaction::map_zoom(distance, axis),
                        "vab" => interaction::vab_zoom(
                            distance,
                            axis,
                            input["shift"].as_bool().unwrap(),
                            fine
                        ),
                        other => panic!("unknown zoom context {other}"),
                    })
                }
                "maneuver" => json!(interaction::maneuver_delta_mps(
                    number(input, "pixels"),
                    fine
                )),
                "ladder" => json!(kerbin_time::WARP_LADDER),
                "warp_to" => json!(kerbin_time::warp_toward(number(input, "remaining"))),
                "drop" => json!(kerbin_time::warp_drop(
                    number(input, "rate") as u32,
                    number(input, "remaining")
                )),
                "symmetry_counts" => json!(vab::SYMMETRY),
                "symmetry_angles" => json!(vab::SYMMETRY
                    .iter()
                    .map(|&n| vab::radial_angle_radians(1, n).to_degrees())
                    .collect::<Vec<_>>()),
                other => panic!("unsupported reference operation {other}"),
            };
            json!({"id":case["id"], "actual":actual})
        })
        .collect();
    println!("{}", json!({"cases":rows}));
}

//! Missions from the extracted `explorationmissions` and `missioncontrol` JSON.

use crate::{f, read_json, s};
use ose_data::missions::{Condition, MissionDef, MissionEvent, MissionStage, Op, Watch};
use serde_json::Value;
use std::path::Path;

/// The short type name out of an assembly-qualified one (`KSP.Messages.X, Assembly-CSharp, ...` -> `X`).
fn type_name(aqn: &str) -> &str {
    aqn.split(',')
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
}

pub fn condition(v: &Value) -> Option<Condition> {
    match v["ConditionType"].as_str()? {
        "ConditionSet" => {
            let children: Vec<Condition> = v["Children"]
                .as_array()
                .map(|a| a.iter().filter_map(condition).collect())
                .unwrap_or_default();
            Some(match v["ConditionMode"].as_str().unwrap_or("AND") {
                "OR" => Condition::Any(children),
                "XOR" => Condition::OneOf(children),
                "NOT" => Condition::Not(children),
                _ => Condition::All(children),
            })
        }
        "PropertyCondition" => Some(Condition::Property {
            watch: Watch::from_type_name(type_name(v["PropertyTypeAQN"].as_str().unwrap_or(""))),
            op: Op::from_name(v["propOperator"].as_str().unwrap_or("EQUAL")),
            value: f(v, "TestWatchedValue"),
            count: v["TestWatchedInt"].as_i64().unwrap_or(0),
            text: s(v, "TestWatchedstring"),
            input: s(v, "Inputstring"),
            expect: v["TestWatchedBool"].as_bool().unwrap_or(true),
            latch: !v["RequireCurrentValue"].as_bool().unwrap_or(true),
        }),
        "EventCondition" => Some(Condition::Event(MissionEvent::from_type_name(type_name(
            v["EventTypeAQN"].as_str().unwrap_or(""),
        )))),
        _ => None,
    }
}

pub fn mission(v: &Value) -> MissionDef {
    let stages = v["missionStages"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|st| MissionStage {
                    id: st["StageID"].as_i64().unwrap_or(0),
                    objective_key: s(st, "Objective"),
                    condition: if st["scriptableCondition"].is_object() {
                        condition(&st["scriptableCondition"])
                    } else {
                        None
                    },
                    science_reward: st["MissionReward"]["MissionRewardDefinitions"]
                        .as_array()
                        .map(|r| {
                            r.iter()
                                .filter(|x| {
                                    x["MissionRewardType"].as_str() == Some("SciencePoints")
                                })
                                .map(|x| f(x, "RewardAmount"))
                                .sum()
                        })
                        .unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();
    let granter = v["MissionGranterKey"].as_str().unwrap_or("");
    MissionDef {
        id: s(v, "ID"),
        name_key: s(v, "name"),
        description_key: s(v, "description"),
        primary: s(v, "type") == "Primary",
        granter: if s(v, "Owner") == "Agency" {
            granter.to_string()
        } else {
            String::new()
        },
        active_at_start: s(v, "state") == "Active",
        stages,
    }
}

pub fn import_missions(data_dir: &Path) -> anyhow::Result<Vec<MissionDef>> {
    let mut out = Vec::new();
    for dir in ["explorationmissions", "missioncontrol"] {
        for p in crate::json_files(&data_dir.join(dir))? {
            out.push(mission(&read_json(&p)?));
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_property_and_an_event_inside_a_set() {
        let v: Value = serde_json::from_str(
            r#"{"ConditionType":"ConditionSet","ConditionMode":"AND","Children":[
                {"ConditionType":"PropertyCondition","PropertyTypeAQN":"KSP.Messages.PropertyWatchers.MaxGroundAltitudeReached, Assembly-CSharp","TestWatchedValue":10000.0,"propOperator":"GREATER","RequireCurrentValue":false},
                {"ConditionType":"EventCondition","EventTypeAQN":"KSP.Messages.VesselLandedAfterFlight, Assembly-CSharp"}]}"#,
        )
        .unwrap();
        let c = condition(&v).unwrap();
        let Condition::All(ch) = &c else { panic!() };
        assert!(matches!(
            &ch[0],
            Condition::Property {
                watch: Watch::MaxGroundAltitude,
                latch: true,
                ..
            }
        ));
        assert_eq!(ch[1], Condition::Event(MissionEvent::LandedAfterFlight));
        assert!(c.supported());
    }
}

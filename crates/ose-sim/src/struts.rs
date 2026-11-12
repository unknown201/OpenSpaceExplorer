//! Strut behavior regressions. The coupled solver lives in `rigid::support`.

#[cfg(test)]
mod tests {
    use crate::rigid::RigidVessel;
    use ose_data::craft::{Craft, CraftPart, Quat};
    use ose_data::PartDef;
    use ose_math::DVec3;

    fn part(id: &str, parent: Option<usize>, y: f64, x: f64) -> CraftPart {
        CraftPart {
            guid: format!("g{y}{x}"),
            part_id: id.into(),
            parent,
            own_node: None,
            parent_node: None,
            local_position: DVec3::new(x, y, 0.0),
            local_rotation: Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            stage_index: 0,
            fuel_crossfeed: true,
            resources: Vec::new(),
            fuel_line_target: None,
            colors: None,
            flight: None,
            actions: Vec::new(),
            symmetry: 0,
            chute: None,
            fairing: Default::default(),
            shape: Default::default(),
        }
    }

    fn defs() -> Vec<PartDef> {
        let all: Vec<PartDef> =
            serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
        all.into_iter()
            .filter(|d| {
                [
                    "fueltank_1v_inline_methalox_1x2",
                    "decoupler_1v_radial",
                    "strut_1v_extensible",
                ]
                .contains(&d.id.as_str())
            })
            .collect()
    }

    /// A tank with a booster tank on a radial decoupler; `strut` adds a strut from the booster to the core.
    fn booster(strut: bool) -> Craft {
        let mut parts = vec![
            part("fueltank_1v_inline_methalox_1x2", None, 0.0, 0.0),
            part("decoupler_1v_radial", Some(0), 0.0, 0.7),
            part("fueltank_1v_inline_methalox_1x2", Some(1), 0.0, 1.4),
        ];
        if strut {
            parts.push(part("strut_1v_extensible", Some(2), 1.0, 0.9));
            parts.push(part("strut_1v_extensible", Some(0), 1.2, 0.6));
            parts[3].fuel_line_target = Some(4);
        }
        Craft {
            name: "braced".into(),
            parts,
            ..Default::default()
        }
    }

    #[test]
    fn brace_does_not_permanently_multiply_parent_joint_strength() {
        let defs = defs();
        let loose = RigidVessel::build(&booster(false), &defs, &[]);
        let mut braced = RigidVessel::build(&booster(true), &defs, &[]);
        for child in [1, 2] {
            assert_eq!(
                loose
                    .links
                    .iter()
                    .find(|l| l.child == child)
                    .unwrap()
                    .limits,
                braced
                    .links
                    .iter()
                    .find(|l| l.child == child)
                    .unwrap()
                    .limits
            );
        }
        assert_eq!(braced.braces.len(), 1);
        braced.remove_parts(&[4]);
        assert!(!braced.braces[0].active);
    }
    #[test]
    fn unlinked_anchor_adds_no_virtual_joint() {
        let defs = defs();
        let mut c = booster(true);
        c.parts[3].fuel_line_target = None;
        assert!(RigidVessel::build(&c, &defs, &[]).braces.is_empty());
    }
}

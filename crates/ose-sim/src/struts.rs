//! Struts in the joint solver.
//!
//! A strut is a two-anchor part: its first anchor hangs on one part and `CraftPart::fuel_line_target` names
//! its far anchor, which hangs on another (`Craft::compound_ends`). In the original (`Module_Strut`) the
//! first anchor adds a joint between the two parts the anchors hang on: a stack-type joint of node size 0,
//! not rigid, that never breaks, kept while both parts stay on one vessel.
//!
//! The craft here is a tree of fixed multibody links, which has no room for a loop, so the strut acts on the
//! links it spans instead: it sits in parallel with the chain of joints on the tree path between its two
//! parts. That chain, springs in series, and the strut share the load by stiffness, so every joint on the path
//! carries only `k_chain / (k_chain + k_strut)` of what it carried alone. Carrying less of the same load is
//! the same as being that much stronger and stiffer, so each link on the path has its spring, damping and
//! break limits scaled up by the inverse of that share. A strut whose ends are gone (staged or broken off)
//! is no longer in the craft the vessel is rebuilt from, so it stops counting.

use std::collections::HashMap;

use ose_data::craft::Craft;
use ose_data::PartDef;

use crate::rigid::{joint_settings, Link};

/// The original strut joint: stack type, node size 0, not rigid, unbreakable.
fn strut_stiffness() -> f64 {
    joint_settings()
        .limits(
            0,
            true,
            false,
            false,
            (f64::INFINITY, f64::INFINITY),
            (f64::INFINITY, f64::INFINITY),
        )
        .stiffness
}

/// Indices into `links` on the tree path between parts `a` and `b`.
fn path(links: &[Link], a: usize, b: usize) -> Vec<usize> {
    let up = |mut p: usize| {
        let mut chain = vec![(p, None)];
        while let Some(k) = links.iter().position(|l| l.child == p) {
            p = links[k].parent;
            if chain.iter().any(|(q, _)| *q == p) {
                break;
            }
            chain.push((p, Some(k)));
        }
        chain
    };
    let (ua, ub) = (up(a), up(b));
    // the lowest part both reach
    let Some(meet) = ua.iter().position(|(p, _)| ub.iter().any(|(q, _)| q == p)) else {
        return Vec::new();
    };
    let common = ua[meet].0;
    let meet_b = ub.iter().position(|(q, _)| *q == common).unwrap_or(0);
    ua[1..=meet]
        .iter()
        .chain(ub[1..=meet_b].iter())
        .filter_map(|(_, k)| *k)
        .collect()
}

/// Strengthen the links each strut of `craft` spans (see the module notes). Returns how many struts braced.
pub(crate) fn brace(links: &mut [Link], craft: &Craft, by_id: &HashMap<&str, &PartDef>) -> usize {
    let k_strut = strut_stiffness();
    let mut braced = 0;
    for (i, p) in craft.parts.iter().enumerate() {
        if !by_id.get(p.part_id.as_str()).is_some_and(|d| d.is_strut()) {
            continue;
        }
        let Some((a, b)) = craft.compound_ends(i) else {
            continue;
        };
        let on = path(links, a, b);
        // springs in series: 1 / k_chain = sum 1 / k
        let compliance: f64 = on
            .iter()
            .map(|&k| 1.0 / links[k].limits.stiffness.max(1e-9))
            .sum();
        if on.is_empty() || compliance <= 0.0 || !k_strut.is_finite() || k_strut <= 0.0 {
            continue;
        }
        let k_chain = 1.0 / compliance;
        let scale = (k_chain + k_strut) / k_chain;
        for k in on {
            let l = &mut links[k].limits;
            l.stiffness *= scale;
            l.damping *= scale;
            l.break_force_kn *= scale;
            l.break_torque_knm *= scale;
        }
        braced += 1;
    }
    braced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rigid::RigidVessel;
    use ose_data::craft::{CraftPart, Quat};
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
    fn a_strut_shares_the_load_of_the_joints_between_its_parts() {
        let defs = defs();
        assert_eq!(defs.len(), 3, "test parts present in the catalog");
        let loose = RigidVessel::build(&booster(false), &defs, &[]);
        let braced = RigidVessel::build(&booster(true), &defs, &[]);
        let link =
            |v: &RigidVessel, c: usize| v.links.iter().find(|l| l.child == c).unwrap().limits;
        // the decoupler's two joints are on the strut's path: stronger and stiffer by the same factor
        for c in [1, 2] {
            let (a, b) = (link(&loose, c), link(&braced, c));
            let s = b.stiffness / a.stiffness;
            assert!(s > 1.05, "link {c} stiffened by {s}");
            assert!((b.break_force_kn / a.break_force_kn - s).abs() < 1e-9 * s);
            assert!((b.break_torque_knm / a.break_torque_knm - s).abs() < 1e-9 * s);
        }
        // the strut's own anchors keep their ordinary joints
        assert_eq!(link(&braced, 3).stiffness, link(&braced, 4).stiffness);
    }

    #[test]
    fn an_unlinked_strut_braces_nothing() {
        let defs = defs();
        let mut c = booster(true);
        c.parts[3].fuel_line_target = None;
        let by_id: HashMap<&str, &PartDef> = defs.iter().map(|d| (d.id.as_str(), d)).collect();
        let mut links = RigidVessel::build(&c, &defs, &[]).links;
        assert_eq!(brace(&mut links, &c, &by_id), 0);
    }

    #[test]
    fn the_strut_path_runs_through_the_lowest_common_part() {
        let defs = defs();
        let v = RigidVessel::build(&booster(true), &defs, &[]);
        let mut on: Vec<usize> = path(&v.links, 2, 0)
            .into_iter()
            .map(|k| v.links[k].child)
            .collect();
        on.sort();
        assert_eq!(on, vec![1, 2]);
        assert!(path(&v.links, 0, 0).is_empty());
    }
}

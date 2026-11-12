//! Linearized two-anchor joints for the existing angular-flex model.
//! The parent tree remains the connectivity graph: an extra connection releases when that tree splits.
use super::*;

#[derive(Clone, Debug)]
pub struct Brace {
    pub part: usize,
    pub far_part: usize,
    pub source: usize,
    pub target: usize,
    pub anchor: DVec3,
    pub active: bool,
}

pub(super) struct Row {
    terms: Vec<(usize, DVec3, f64)>,
    axis: DVec3,
    stiffness: f64,
    linear: bool,
}

impl RigidVessel {
    /// Reference default: each eligible physical part is reinforced to the first
    /// heaviest physical part. Decouplers/clamps never receive an enhanced joint.
    /// This adds coupled constraints; it does not mark the parent links rigid.
    pub fn set_enhanced_joints(&mut self, enabled: bool) {
        self.enhanced_joints = enabled;
        self.rebuild_reinforcements();
    }

    pub(super) fn rebuild_reinforcements(&mut self) {
        self.reinforcements.clear();
        let mut heaviest = None;
        for i in 0..self.len() {
            if self.present[i]
                && self.reinforcement_targets[i]
                && heaviest.is_none_or(|j| self.masses_t[i] > self.masses_t[j])
            {
                heaviest = Some(i);
            }
        }
        let Some(target) = heaviest else { return };
        for source in 0..self.len() {
            use ose_data::ReinforcementTarget::*;
            if !self.present[source] || !self.reinforcement_targets[source] {
                continue;
            }
            let mode = match self.authored_reinforcement[source] {
                Some(mode) => mode,
                None if self.enhanced_joints && self.reinforcement_eligible[source] => Heaviest,
                None => continue,
            };
            let parent = |i| {
                self.links
                    .iter()
                    .find(|l| l.child == i && l.intact())
                    .map(|l| l.parent)
                    .filter(|p| !self.launch_clamps.contains(p))
            };
            let target = match mode {
                Heaviest => target,
                GrandParent => parent(source)
                    .map(|p| parent(p).unwrap_or(p))
                    .unwrap_or(source),
                Root => {
                    if !self.launch_clamps.contains(&self.root) {
                        self.root
                    } else {
                        parent(source).unwrap_or(source)
                    }
                }
            };
            if source != target {
                self.reinforcements.push(Brace {
                    part: source,
                    far_part: target,
                    source,
                    target,
                    anchor: self.rest_pos[source],
                    active: true,
                });
            }
        }
    }

    /// Whether any ground launch clamps are still attached.
    pub fn has_launch_clamps(&self) -> bool {
        self.launch_clamps.iter().any(|&i| self.present[i])
    }

    /// Path signs describe the relative motion of target against source at the target anchor.
    pub(super) fn brace_rows(&mut self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut parent = vec![None; self.len()];
        for (i, l) in self.links.iter().enumerate().filter(|(_, l)| l.intact()) {
            parent[l.child] = Some(i);
        }
        let angular_stiffness = |size| {
            joint_settings()
                .limits(
                    size,
                    true,
                    false,
                    false,
                    (f64::INFINITY, f64::INFINITY),
                    (f64::INFINITY, f64::INFINITY),
                )
                .stiffness
        };
        for (brace, angular_stiffness) in self
            .braces
            .iter_mut()
            .map(|b| (b, angular_stiffness(0)))
            .chain(
                self.reinforcements
                    .iter_mut()
                    .map(|b| (b, angular_stiffness(2))),
            )
        {
            if !brace.active {
                continue;
            }
            if !self.present[brace.far_part]
                || !self.present[brace.part]
                || !self.present[brace.source]
                || !self.present[brace.target]
            {
                brace.active = false;
                continue;
            }
            let path = |mut p: usize| {
                let mut out = Vec::new();
                while let Some(i) = parent[p] {
                    out.push(i);
                    p = self.links[i].parent;
                }
                (p, out)
            };
            let (a, source) = path(brace.source);
            let (b, target) = path(brace.target);
            if a != b {
                brace.active = false;
                continue;
            }
            let mut terms = Vec::new();
            for (path, other, sign) in [(&source, &target, -1.0), (&target, &source, 1.0)] {
                for &i in path.iter().filter(|i| !other.contains(i)) {
                    if !self.links[i].limits.rigid {
                        terms.push((i, sign));
                    }
                }
            }
            for axis in [
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(0.0, 0.0, 1.0),
            ] {
                for linear in [true, false] {
                    rows.push(Row {
                        terms: terms
                            .iter()
                            .map(|&(i, sign)| {
                                let coefficient = if linear {
                                    (brace.anchor - self.links[i].anchor_rest).cross(axis) * sign
                                } else {
                                    axis * sign
                                };
                                (i, coefficient, sign)
                            })
                            .collect(),
                        axis,
                        // Finite translation penalty replaces PhysX's near-infinite linear drive.
                        stiffness: if linear { 1e8 } else { angular_stiffness },
                        linear,
                    });
                }
            }
        }
        rows
    }

    pub(super) fn braced_links(&mut self) -> Vec<bool> {
        let mut involved = vec![false; self.links.len()];
        for row in self.brace_rows() {
            for (i, _, _) in row.terms {
                involved[i] = true;
            }
        }
        involved
    }

    /// Backward Euler, solved by preconditioned conjugate gradients. Coupled constraints let a
    /// braced path bend internally while keeping its endpoints together; no global rigidity toggle.
    pub(super) fn solve_braces(&mut self, dt: f64, inertia: &[f64], forces: &[DVec3]) {
        let rows = self.brace_rows();
        let n = self.links.len();
        let mut involved = vec![false; n];
        for row in &rows {
            for &(i, _, _) in &row.terms {
                involved[i] = true;
            }
        }
        let theta: Vec<_> = self.links.iter().map(|l| l.bend).collect();
        let rate: Vec<_> = self.links.iter().map(|l| l.bend_rate).collect();
        let mut diag = vec![1.0; n];
        let mut rhs = vec![DVec3::ZERO; n];
        for (i, l) in self.links.iter().enumerate() {
            if involved[i] && l.intact() && !l.limits.rigid {
                diag[i] = inertia[i] / dt + l.limits.damping + dt * l.limits.stiffness;
                rhs[i] =
                    rate[i] * (inertia[i] / dt) - l.torque_vec_knm - theta[i] * l.limits.stiffness;
            }
        }
        let mut pre = vec![DVec3::ZERO; n];
        for i in 0..n {
            pre[i] = DVec3::new(diag[i], diag[i], diag[i]);
        }
        for row in &rows {
            let q: f64 = row.terms.iter().map(|&(i, b, _)| b.dot(theta[i])).sum();
            let weight = row.stiffness * (joint_settings().damping_factor + dt);
            for &(i, b, _) in &row.terms {
                rhs[i] = rhs[i] - b * (row.stiffness * q);
                pre[i] = pre[i] + DVec3::new(b.x * b.x, b.y * b.y, b.z * b.z) * weight;
            }
        }
        let apply = |x: &[DVec3]| {
            let mut y: Vec<_> = x.iter().zip(&diag).map(|(&v, &d)| v * d).collect();
            for row in &rows {
                let q: f64 = row.terms.iter().map(|&(i, b, _)| b.dot(x[i])).sum();
                for &(i, b, _) in &row.terms {
                    y[i] = y[i] + b * (q * row.stiffness * (joint_settings().damping_factor + dt));
                }
            }
            y
        };
        let dot = |a: &[DVec3], b: &[DVec3]| a.iter().zip(b).map(|(a, b)| a.dot(*b)).sum::<f64>();
        let condition = |r: &[DVec3]| {
            r.iter()
                .zip(&pre)
                .map(|(r, d)| DVec3::new(r.x / d.x, r.y / d.y, r.z / d.z))
                .collect::<Vec<_>>()
        };
        let mut x = vec![DVec3::ZERO; n];
        let mut r = rhs.clone();
        let mut direction = condition(&r);
        let mut rz = dot(&r, &direction);
        let tolerance = dot(&rhs, &rhs) * 1e-18 + 1e-24;
        for _ in 0..(3 * n).min(256) {
            if dot(&r, &r) <= tolerance {
                break;
            }
            let ad = apply(&direction);
            let denominator = dot(&direction, &ad);
            if denominator <= 0.0 {
                break;
            }
            let alpha = rz / denominator;
            for i in 0..n {
                x[i] = x[i] + direction[i] * alpha;
                r[i] = r[i] - ad[i] * alpha;
            }
            let z = condition(&r);
            let next = dot(&r, &z);
            let beta = next / rz;
            for i in 0..n {
                direction[i] = z[i] + direction[i] * beta;
            }
            rz = next;
        }
        for (i, l) in self.links.iter_mut().enumerate() {
            if involved[i] && l.intact() && !l.limits.rigid {
                l.bend_rate = x[i];
                l.bend = theta[i] + x[i] * dt;
                l.torque_knm = l.limits.stiffness * l.bend.length();
            }
        }
        let mut carried = forces.to_vec();
        for row in rows.iter().filter(|r| r.linear) {
            let q: f64 = row
                .terms
                .iter()
                .map(|&(i, b, _)| {
                    b.dot(
                        self.links[i].bend
                            + self.links[i].bend_rate * joint_settings().damping_factor,
                    )
                })
                .sum();
            for &(i, _, sign) in &row.terms {
                carried[i] = carried[i] + row.axis * (sign * row.stiffness * q);
            }
        }
        for (i, l) in self.links.iter_mut().enumerate() {
            l.force_kn = carried[i].length();
        }
    }
}

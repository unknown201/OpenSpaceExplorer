//! Bounded part actuators in craft axes. Allocation retains the forces/torques for joint loading.
use ose_math::DVec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Other,
    /// Legacy catalog fallback: three bilateral channels share a mount sphere.
    RcsMount,
    /// One authored nozzle: an independent input in 0..1.
    RcsNozzle,
}

impl Kind {
    pub fn is_rcs(self) -> bool {
        matches!(self, Self::RcsMount | Self::RcsNozzle)
    }
}

/// Stock Data_ReactionWheel.TorqueResponseSpeed = 30; Unity Vector3.Lerp clamps t.
pub fn wheel_response(previous: DVec3, desired: DVec3, dt: f64) -> DVec3 {
    previous + (desired - previous) * (30.0 * dt).clamp(0.0, 1.0)
}

/// Module_RCS combines positive rotational and translational nozzle projections,
/// then clamps their sum. Rotation uses a unit lever projected off the rotation
/// axis, so distant mounts do not receive a larger normal-mode command.
pub fn rcs_input(direction: DVec3, lever: DVec3, rotation: DVec3, translation: DVec3) -> f64 {
    let axis = if rotation.length_squared() > 1e-16 {
        rotation.normalized()
    } else {
        DVec3::ZERO
    };
    let radial = lever - axis * lever.dot(axis);
    let tangential = if radial.length_squared() > 1e-16 {
        rotation.cross(radial.normalized())
    } else {
        DVec3::ZERO
    };
    (direction.dot(tangential).max(0.0) + direction.dot(translation).max(0.0)).clamp(0.0, 1.0)
}

pub fn rcs_precision_factor(
    direction: DVec3,
    lever: DVec3,
    precision: bool,
    use_lever: bool,
) -> f64 {
    if !precision {
        1.0
    } else if use_lever {
        1.0 / lever.cross(direction).length().max(1.0)
    } else {
        0.1
    }
}

/// Module_RCS applies full-thrust snapping, precision, then the strict 0.05
/// propellant-request threshold, in that order.
pub fn rcs_power(
    input: f64,
    direction: DVec3,
    lever: DVec3,
    precision: bool,
    tuning: ose_data::flight_geometry::RcsTuning,
) -> f64 {
    let input = if tuning.full_thrust && input >= tuning.full_thrust_min {
        1.0
    } else {
        input
    };
    let power = input * rcs_precision_factor(direction, lever, precision, tuning.use_lever);
    if power > 0.05 {
        power
    } else {
        0.0
    }
}

/// Reference hinge projection, distance-weighted roll and ahead-of-COM reversal.
pub fn surface_mix(
    control_up: DVec3,
    input: DVec3,
    com_from_pivot: DVec3,
    hinge: DVec3,
    normal: DVec3,
    deflection: f64,
) -> f64 {
    let roll = input.dot(control_up);
    let mut action = (input - control_up * roll).dot(hinge);
    let h = glam::DVec3::new(hinge.x, hinge.y, hinge.z);
    let turn = glam::DQuat::from_axis_angle(h, deflection);
    let local = turn * glam::DVec3::new(com_from_pivot.x, com_from_pivot.y, com_from_pivot.z);
    let local = DVec3::new(local.x, local.y, local.z);
    let forward = hinge.cross(normal).normalized();
    let radial = local - forward * local.dot(forward);
    let weight = if radial.length_squared() > 1e-16 {
        0.5 - 0.5 * radial.normalized().dot(control_up).abs()
    } else {
        0.5
    };
    let roll = local.dot(hinge) * weight * roll;
    if control_up.dot(com_from_pivot) < 0.0 {
        action = -action;
    }
    (action - roll).clamp(-1.0, 1.0)
}

/// Joint translation/rotation allocation over real, one-way nozzle columns. Per-part
/// L1 budgets retain the exact propellant reservation after partial resource delivery.
pub fn nozzle_wrench(
    channels: &[Actuator],
    torque: DVec3,
    force: DVec3,
    budgets: &[(usize, f64)],
) -> Allocation {
    let ts = channels
        .iter()
        .map(|a| a.torque_knm.length())
        .sum::<f64>()
        .max(1e-6);
    let fs = channels
        .iter()
        .map(|a| a.force_kn.length())
        .sum::<f64>()
        .max(1e-6);
    let norm = channels
        .iter()
        .map(|a| {
            a.torque_knm.length_squared() / (ts * ts) + a.force_kn.length_squared() / (fs * fs)
        })
        .sum::<f64>()
        .max(1e-12);
    let mut out = Allocation {
        inputs: vec![0.0; channels.len()],
        ..Default::default()
    };
    for _ in 0..128 {
        let rt = torque - out.torque_knm;
        let rf = force - out.force_kn;
        for (a, u) in channels.iter().zip(&mut out.inputs) {
            *u = (*u + (a.torque_knm.dot(rt) / (ts * ts) + a.force_kn.dot(rf) / (fs * fs)) / norm)
                .clamp(0.0, 1.0);
        }
        for &(part, budget) in budgets {
            let sum = channels
                .iter()
                .zip(&out.inputs)
                .filter(|(a, _)| a.part == part)
                .map(|(_, u)| *u)
                .sum::<f64>();
            if sum > budget {
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..32 {
                    let mid = (lo + hi) * 0.5;
                    let sum = channels
                        .iter()
                        .zip(&out.inputs)
                        .filter(|(a, _)| a.part == part)
                        .map(|(_, u)| (*u - mid).max(0.0))
                        .sum::<f64>();
                    if sum > budget {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                for (a, u) in channels.iter().zip(&mut out.inputs) {
                    if a.part == part {
                        *u = (*u - hi).max(0.0);
                    }
                }
            }
        }
        out.force_kn = DVec3::ZERO;
        out.torque_knm = DVec3::ZERO;
        for (a, u) in channels.iter().zip(&out.inputs) {
            out.force_kn = out.force_kn + a.force_kn * *u;
            out.torque_knm = out.torque_knm + a.torque_knm * *u;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Actuator {
    pub kind: Kind,
    pub part: usize,
    /// Torque produced at full positive input. One scalar channel cannot control other axes.
    pub torque_knm: DVec3,
    /// A gimbal/control surface also pushes the vessel; wheels have zero force.
    pub force_kn: DVec3,
    pub offset_m: DVec3,
    /// Channels sharing a group share a unit input disc/sphere.
    pub group: usize,
    /// Engine axial thrust and mount lever, shared by a gimbal's two transverse channels.
    pub axial: Option<(DVec3, DVec3)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Allocation {
    pub torque_knm: DVec3,
    pub force_kn: DVec3,
    pub inputs: Vec<f64>,
    pub axial_corrections: Vec<crate::rigid::PartForce>,
}

pub fn allocate(channels: &[Actuator], request: DVec3) -> Allocation {
    allocate_with_axes(channels, request, &[])
}

/// Restrict which commanded rotation axes may request a channel; retain its full physical torque.
pub fn allocate_with_axes(channels: &[Actuator], request: DVec3, axes: &[DVec3]) -> Allocation {
    let masked = |i: usize, v: DVec3| {
        let a = axes.get(i).copied().unwrap_or(DVec3::new(1.0, 1.0, 1.0));
        DVec3::new(a.x * v.x, a.y * v.y, a.z * v.z)
    };
    let mut out = Allocation {
        inputs: vec![0.0; channels.len()],
        ..Default::default()
    };
    let mut groups: Vec<(usize, Vec<usize>, f64)> = Vec::new();
    for (i, channel) in channels.iter().enumerate() {
        if let Some((_, indices, norm)) = groups.iter_mut().find(|g| g.0 == channel.group) {
            indices.push(i);
            *norm += masked(i, channel.torque_knm).length_squared();
        } else {
            groups.push((
                channel.group,
                vec![i],
                masked(i, channel.torque_knm).length_squared(),
            ));
        }
    }
    let mut next = vec![0.0; channels.len()];
    // Block coordinate descent lets a saturated gimbal rotate its command within its cone.
    for _ in 0..16 {
        for (_, indices, lipschitz) in &groups {
            if *lipschitz < 1e-16 {
                continue;
            }
            let residual = request - out.torque_knm;
            let mut length = 0.0;
            for &i in indices {
                next[i] =
                    out.inputs[i] + masked(i, channels[i].torque_knm).dot(residual) / lipschitz;
                if channels[i].kind == Kind::RcsNozzle {
                    next[i] = next[i].max(0.0);
                }
                length += next[i] * next[i];
            }
            let length = length.sqrt().max(1.0);
            for &i in indices {
                next[i] /= length;
                let change = next[i] - out.inputs[i];
                out.torque_knm = out.torque_knm + channels[i].torque_knm * change;
                out.force_kn = out.force_kn + channels[i].force_kn * change;
                out.inputs[i] = next[i];
            }
        }
    }
    for (_, indices, _) in groups {
        if let Some(a) = indices
            .iter()
            .map(|&i| &channels[i])
            .find(|a| a.axial.is_some())
        {
            let (axial, r) = a.axial.unwrap();
            let side = indices.iter().fold(DVec3::ZERO, |s, &i| {
                s + channels[i].force_kn * out.inputs[i]
            });
            let fraction = (1.0 - side.length_squared() / axial.length_squared())
                .max(0.0)
                .sqrt();
            let correction = axial * (fraction - 1.0);
            out.force_kn = out.force_kn + correction;
            out.torque_knm = out.torque_knm + r.cross(correction);
            out.axial_corrections.push(crate::rigid::PartForce {
                part: a.part,
                force_kn: correction,
                offset_m: a.offset_m,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_axis_masks_do_not_remove_incidental_physical_torque() {
        let channel = Actuator {
            kind: Kind::RcsNozzle,
            part: 0,
            torque_knm: DVec3::new(1.0, 1.0, 0.0),
            force_kn: DVec3::new(0.0, 0.0, 1.0),
            offset_m: DVec3::new(-1.0, 1.0, 0.0),
            group: 0,
            axial: None,
        };
        let out = allocate_with_axes(
            std::slice::from_ref(&channel),
            DVec3::new(1.0, 0.0, 0.0),
            &[DVec3::new(1.0, 0.0, 0.0)],
        );
        assert_eq!(out.torque_knm, DVec3::new(1.0, 1.0, 0.0));
        assert_eq!(out.force_kn, DVec3::new(0.0, 0.0, 1.0));
        let disabled = allocate_with_axes(
            &[channel],
            DVec3::new(1.0, 0.0, 0.0),
            &[DVec3::new(0.0, 1.0, 0.0)],
        );
        assert_eq!(disabled.inputs, vec![0.0]);
    }

    #[test]
    fn rcs_precision_uses_lever_or_factor_and_applies_request_threshold_last() {
        use ose_data::flight_geometry::RcsTuning;
        let d = DVec3::new(0., 1., 0.);
        let lever = DVec3::new(4., 0., 0.);
        let tuning = RcsTuning {
            use_lever: true,
            ..Default::default()
        };
        assert_eq!(rcs_power(1., d, lever, true, tuning), 0.25);
        assert_eq!(rcs_power(1., d, lever, false, tuning), 1.);
        assert_eq!(rcs_power(0.2, d, lever, true, tuning), 0.);
        assert_eq!(
            rcs_power(
                0.1,
                d,
                lever,
                true,
                RcsTuning {
                    full_thrust: true,
                    full_thrust_min: 0.1,
                    ..tuning
                }
            ),
            0.25
        );
        assert_eq!(rcs_power(1., d, lever, true, Default::default()), 0.1);
        assert_eq!(rcs_power(0.05, d, lever, false, tuning), 0.);
        assert_eq!(rcs_precision_factor(d, d * 100., true, true), 1.);
    }
    #[test]
    fn surfaces_reverse_ahead_of_com_and_mix_opposite_roll_without_air() {
        let up = DVec3::new(0., 1., 0.);
        let hinge = DVec3::new(1., 0., 0.);
        let normal = DVec3::new(0., 0., 1.);
        assert_eq!(surface_mix(up, hinge, up * 2., hinge, normal, 0.), 1.);
        assert_eq!(surface_mix(up, hinge, -up * 2., hinge, normal, 0.), -1.);
        assert_eq!(surface_mix(up, up, hinge * 2., hinge, normal, 0.), -1.);
        assert_eq!(surface_mix(up, up, hinge * (-2.), hinge, normal, 0.), 1.);
        assert_eq!(surface_mix(up, hinge * 10., up * 2., hinge, normal, 0.), 1.);
    }
    #[test]
    fn nozzle_commands_combine_rotation_and_translation_without_reverse_thrust() {
        let direction = DVec3::new(0., 1., 0.);
        let lever = DVec3::new(2., 0., 0.);
        assert!(
            (rcs_input(direction, lever, DVec3::new(0., 0., 0.4), direction * 0.3) - 0.7).abs()
                < 1e-12
        );
        assert_eq!(
            rcs_input(direction, lever, DVec3::new(0., 0., -1.), -direction),
            0.
        );
        assert_eq!(
            rcs_input(direction, lever, DVec3::new(0., 0., 1.), direction),
            1.
        );
        assert_eq!(
            rcs_input(direction, lever * 10., DVec3::new(0., 0., 1.), DVec3::ZERO),
            1.
        );
        assert_eq!(
            rcs_input(direction, DVec3::ZERO, DVec3::new(0., 0., 1.), DVec3::ZERO),
            0.
        );
    }
    #[test]
    fn fuel_budget_reallocates_combined_wrench_to_remaining_nozzles() {
        let channels = [1., -1.]
            .into_iter()
            .enumerate()
            .map(|(part, x)| Actuator {
                kind: Kind::RcsNozzle,
                part,
                torque_knm: DVec3::new(0., 0., x),
                force_kn: DVec3::new(0., 1., 0.),
                offset_m: DVec3::new(x, 0., 0.),
                group: part,
                axial: None,
            })
            .collect::<Vec<_>>();
        let out = nozzle_wrench(
            &channels,
            DVec3::ZERO,
            DVec3::new(0., 1., 0.),
            &[(0, 0.25), (1, 1.)],
        );
        assert!((out.inputs[0] - 0.25).abs() < 1e-6);
        assert!(out.inputs[1] > 0.49 && out.inputs[1] < 0.51);
        assert!(out.inputs.iter().all(|u| *u >= 0. && *u <= 1.));
        let empty = nozzle_wrench(
            &channels,
            DVec3::new(0., 0., 1.),
            DVec3::new(0., 1., 0.),
            &[(0, 0.), (1, 0.)],
        );
        assert_eq!(empty.force_kn, DVec3::ZERO);
        assert_eq!(empty.torque_knm, DVec3::ZERO);
    }
    #[test]
    fn wheel_response_matches_reference_lerp_on_press_release_reverse_and_warp() {
        let target = DVec3::new(2.0, -3.0, 4.0);
        let first = wheel_response(DVec3::ZERO, target, 0.02);
        assert!((first - target * 0.6).length() < 1e-12);
        let release = wheel_response(first, DVec3::ZERO, 0.02);
        assert!((release - target * 0.24).length() < 1e-12);
        let reverse = wheel_response(release, -target, 0.02);
        assert!((reverse - target * (-0.504)).length() < 1e-12);
        assert_eq!(wheel_response(reverse, target, 0.08), target);
    }
    #[test]
    fn gimbal_cannot_roll_and_shared_channels_respect_the_force_cone() {
        let channels =
            [DVec3::new(2.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 2.0)].map(|torque_knm| Actuator {
                kind: Kind::Other,
                part: 0,
                torque_knm,
                force_kn: DVec3::ZERO,
                offset_m: DVec3::ZERO,
                group: 0,
                axial: None,
            });
        assert_eq!(
            allocate(&channels, DVec3::new(0.0, 50.0, 0.0)).torque_knm,
            DVec3::ZERO
        );
        let out = allocate(&channels, DVec3::new(50.0, 0.0, 50.0));
        assert!((out.torque_knm.x - out.torque_knm.z).abs() < 1e-10);
        assert!(out.torque_knm.length() <= 2.0 + 1e-10);
        assert!(out.inputs.iter().map(|u| u * u).sum::<f64>() <= 1.0 + 1e-10);
    }

    #[test]
    fn gimbal_deflection_preserves_the_fuel_resolved_thrust_magnitude() {
        let thrust = DVec3::new(0.0, 10.0, 0.0);
        let r = DVec3::new(0.0, -2.0, 0.0);
        let channels =
            [DVec3::new(1.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 1.0)].map(|force_kn| Actuator {
                kind: Kind::Other,
                part: 0,
                torque_knm: r.cross(force_kn),
                force_kn,
                offset_m: DVec3::ZERO,
                group: 0,
                axial: Some((thrust, r)),
            });
        let out = allocate(&channels, DVec3::new(100.0, 0.0, 100.0));
        let force = thrust + out.force_kn;
        assert!((force.length() - 10.0).abs() < 1e-10);
        assert!((out.torque_knm - r.cross(out.force_kn)).length() < 1e-10);
        assert_eq!(out.axial_corrections.len(), 1);
    }
    #[test]
    fn a_nozzle_cannot_reverse_to_meet_a_torque_request() {
        let nozzle = Actuator {
            kind: Kind::RcsNozzle,
            part: 0,
            force_kn: DVec3::new(1.0, 0.0, 0.0),
            torque_knm: DVec3::new(0.0, 0.0, 1.0),
            offset_m: DVec3::new(0.0, -1.0, 0.0),
            group: 0,
            axial: None,
        };
        assert_eq!(
            allocate(std::slice::from_ref(&nozzle), DVec3::new(0.0, 0.0, -100.0)).inputs,
            vec![0.0]
        );
        assert_eq!(
            allocate(&[nozzle], DVec3::new(0.0, 0.0, 100.0)).inputs,
            vec![1.0]
        );
    }
}

//! Bounded part actuators in craft axes. Allocation retains the forces/torques for joint loading.
use ose_math::DVec3;

/// Stock Data_ReactionWheel.TorqueResponseSpeed = 30; Unity Vector3.Lerp clamps t.
pub fn wheel_response(previous: DVec3, desired: DVec3, dt: f64) -> DVec3 {
    previous + (desired - previous) * (30.0 * dt).clamp(0.0, 1.0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Actuator {
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
    let mut out = Allocation {
        inputs: vec![0.0; channels.len()],
        ..Default::default()
    };
    let mut groups: Vec<(usize, Vec<usize>, f64)> = Vec::new();
    for (i, channel) in channels.iter().enumerate() {
        if let Some((_, indices, norm)) = groups.iter_mut().find(|g| g.0 == channel.group) {
            indices.push(i);
            *norm += channel.torque_knm.length_squared();
        } else {
            groups.push((channel.group, vec![i], channel.torque_knm.length_squared()));
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
                next[i] = out.inputs[i] + channels[i].torque_knm.dot(residual) / lipschitz;
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
}

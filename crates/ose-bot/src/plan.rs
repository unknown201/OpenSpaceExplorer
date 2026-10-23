//! Mission planning: a Hohmann transfer from a circular parking orbit to a coplanar circular target orbit
//! (Kerbin parking orbit to the Mun). Pure functions over `f64`, no ECS.

use std::f64::consts::{PI, TAU};

/// A planned prograde burn and the coast that follows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransferPlan {
    /// Universal time of the burn, s.
    pub burn_ut: f64,
    /// Time from `ut0` to the burn, s.
    pub wait_s: f64,
    /// Prograde delta-v, m/s.
    pub dv_mps: f64,
    /// Half-ellipse coast time from burn to arrival at the target radius, s.
    pub transfer_time_s: f64,
    /// Angle the target must lead the vessel by at the burn, rad.
    pub lead_angle_rad: f64,
}

/// Angle of `x` in the plane with normal `n`, measured from `reference` (both unit vectors in the plane).
pub fn in_plane_angle(reference: ose_math::DVec3, n: ose_math::DVec3, x: ose_math::DVec3) -> f64 {
    let s = n.dot(reference.cross(x));
    let c = reference.dot(x);
    s.atan2(c)
}

/// Plan a Hohmann transfer.
///
/// `mu` is the central body's gravitational parameter, `r_park` and `r_target` the circular orbit radii,
/// `phase_now` the target's angle minus the vessel's angle at `ut0` (rad, any range). Both orbits are
/// assumed coplanar and circular, which holds for Kerbin orbit to the Mun.
pub fn plan_hohmann(mu: f64, r_park: f64, r_target: f64, phase_now: f64, ut0: f64) -> TransferPlan {
    let n_vessel = (mu / r_park.powi(3)).sqrt();
    let n_target = (mu / r_target.powi(3)).sqrt();
    let a_transfer = 0.5 * (r_park + r_target);
    let transfer_time_s = PI * (a_transfer.powi(3) / mu).sqrt();
    let lead_angle_rad = PI - n_target * transfer_time_s;
    // phase(t) = phase_now + (n_target - n_vessel) (t - ut0) decreases with time (inner orbit is faster);
    // the first t >= ut0 where phase(t) == lead_angle (mod 2 pi):
    let wait_s = (phase_now - lead_angle_rad).rem_euclid(TAU) / (n_vessel - n_target);
    let v_circ = (mu / r_park).sqrt();
    let dv_mps = v_circ * ((2.0 * r_target / (r_park + r_target)).sqrt() - 1.0);
    TransferPlan {
        burn_ut: ut0 + wait_s,
        wait_s,
        dv_mps,
        transfer_time_s,
        lead_angle_rad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MU_KERBIN: f64 = 3.5316e12;

    #[test]
    fn kerbin_100km_to_mun_matches_the_well_known_numbers() {
        let p = plan_hohmann(MU_KERBIN, 700_000.0, 12_000_000.0, 0.0, 0.0);
        // Players quote roughly 860 m/s for the Mun burn from a 100 km orbit (the ideal Hohmann is a bit
        // less) and a coast of about six to seven hours.
        assert!((p.dv_mps - 842.0).abs() < 5.0, "dv {}", p.dv_mps);
        assert!(
            p.transfer_time_s > 6.0 * 3600.0 && p.transfer_time_s < 8.0 * 3600.0,
            "transfer {}",
            p.transfer_time_s
        );
        // Lead angle a little over 110 degrees, as the KSP community tables say.
        let lead_deg = p.lead_angle_rad.to_degrees();
        assert!((lead_deg - 114.0).abs() < 6.0, "lead {lead_deg}");
    }

    #[test]
    fn waiting_brings_the_phase_to_the_lead_angle() {
        let r1 = 700_000.0;
        let r2 = 12_000_000.0;
        for phase_now in [0.0, 1.0, 3.0, -2.0, 6.0] {
            let p = plan_hohmann(MU_KERBIN, r1, r2, phase_now, 100.0);
            let n_v = (MU_KERBIN / r1.powi(3)).sqrt();
            let n_t = (MU_KERBIN / r2.powi(3)).sqrt();
            let phase_at_burn = phase_now + (n_t - n_v) * p.wait_s;
            let err = (phase_at_burn - p.lead_angle_rad).rem_euclid(TAU);
            assert!(
                err < 1e-9 || (TAU - err) < 1e-9,
                "phase {phase_now}: err {err}"
            );
            assert!(p.wait_s >= 0.0 && p.wait_s <= TAU / (n_v - n_t));
        }
    }

    #[test]
    fn in_plane_angle_is_signed_about_the_normal() {
        use ose_math::DVec3;
        let x = DVec3::new(1.0, 0.0, 0.0);
        let z = DVec3::new(0.0, 0.0, 1.0);
        assert!((in_plane_angle(x, z, DVec3::new(0.0, 1.0, 0.0)) - PI / 2.0).abs() < 1e-12);
        assert!((in_plane_angle(x, z, DVec3::new(0.0, -1.0, 0.0)) + PI / 2.0).abs() < 1e-12);
    }
}

//! The bot's local flight model: a vessel on rails in `ose-sim`, an impulsive burn, and a coast until the
//! Mun's sphere of influence takes over. This is what the server will do once it drives vessels and accepts
//! maneuver intents; until then the bot runs it in-process so the Mun run is exercised today.

use crate::plan::{in_plane_angle, plan_hohmann, TransferPlan};
use anyhow::{anyhow, Context, Result};
use bevy_ecs::prelude::*;
use ose_data::CelestialBody;
use ose_math::{DVec3, Orbit};
use ose_sim::bodies::{spawn_bodies, star_space_state, Body, BodyIndex, OnRails, ReferenceBody};
use ose_sim::vessels::{spawn_vessel_on_rails, update_reference_body, SoiTransition};

/// A universe with the bodies spawned and the handles the Mun run uses.
pub struct RailsWorld {
    pub world: World,
    pub index: BodyIndex,
    pub kerbin: Entity,
    pub mun: Entity,
}

impl RailsWorld {
    pub fn new(bodies: &[CelestialBody]) -> Result<Self> {
        let mut world = World::new();
        let index = spawn_bodies(&mut world, bodies);
        let kerbin = *index
            .by_id
            .get("Kerbin")
            .context("no Kerbin in body data")?;
        let mun = *index.by_id.get("Mun").context("no Mun in body data")?;
        Ok(Self {
            world,
            index,
            kerbin,
            mun,
        })
    }

    pub fn body(&self, e: Entity) -> &Body {
        self.world.get::<Body>(e).expect("body component")
    }

    /// Position and velocity of `e` relative to `about` at `ut`.
    pub fn relative_state(&self, e: Entity, about: Entity, ut: f64) -> (DVec3, DVec3) {
        let (p, v) = star_space_state(&self.world, e, ut);
        let (bp, bv) = star_space_state(&self.world, about, ut);
        (p - bp, v - bv)
    }
}

/// Where the vessel is parked before the transfer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParkingOrbit {
    pub vessel: Entity,
    pub radius_m: f64,
    pub altitude_m: f64,
    pub speed_mps: f64,
    pub period_s: f64,
}

/// Put a vessel in a circular, prograde orbit at `altitude_m` above Kerbin, in the Mun's orbital plane, at
/// `ut`. `phase_offset_rad` is where it sits relative to the Mun's direction (0 = directly below the Mun).
pub fn park_in_kerbin_orbit(
    rw: &mut RailsWorld,
    name: &str,
    altitude_m: f64,
    phase_offset_rad: f64,
    ut: f64,
) -> ParkingOrbit {
    let kerbin = rw.body(rw.kerbin).clone();
    let (mun_p, mun_v) = rw.relative_state(rw.mun, rw.kerbin, ut);
    let normal = mun_p.cross(mun_v).normalized();
    let u_r = mun_p.normalized();
    let u_t = normal.cross(u_r);
    let (s, c) = phase_offset_rad.sin_cos();
    let radius_m = kerbin.radius_m + altitude_m;
    let p_hat = u_r * c + u_t * s;
    let r = p_hat * radius_m;
    let speed_mps = (kerbin.mu / radius_m).sqrt();
    let v = normal.cross(p_hat) * speed_mps;
    let vessel = spawn_vessel_on_rails(&mut rw.world, name, rw.kerbin, r, v, ut);
    let period_s = rw.world.get::<OnRails>(vessel).expect("rails").0.period;
    ParkingOrbit {
        vessel,
        radius_m,
        altitude_m,
        speed_mps,
        period_s,
    }
}

/// The Mun's angle minus the vessel's angle in the Mun's orbital plane at `ut`.
pub fn mun_phase(rw: &RailsWorld, vessel: Entity, ut: f64) -> f64 {
    let (mun_p, mun_v) = rw.relative_state(rw.mun, rw.kerbin, ut);
    let normal = mun_p.cross(mun_v).normalized();
    let reference = mun_p.normalized();
    let (vp, _) = rw.relative_state(vessel, rw.kerbin, ut);
    let vessel_angle = in_plane_angle(reference, normal, vp.normalized());
    -vessel_angle
}

/// Plan the transfer burn for `vessel` (parked around Kerbin) at or after `ut`.
pub fn plan_mun_transfer(rw: &RailsWorld, vessel: Entity, ut: f64) -> Result<TransferPlan> {
    let rails = rw
        .world
        .get::<OnRails>(vessel)
        .ok_or_else(|| anyhow!("vessel is not on rails"))?
        .0;
    let reference = rw
        .world
        .get::<ReferenceBody>(vessel)
        .ok_or_else(|| anyhow!("vessel has no reference body"))?
        .0;
    anyhow::ensure!(reference == rw.kerbin, "vessel is not in Kerbin orbit");
    anyhow::ensure!(
        rails.eccentricity() < 0.01,
        "parking orbit is not circular (e = {})",
        rails.eccentricity()
    );
    let r_target = rw
        .world
        .get::<OnRails>(rw.mun)
        .expect("mun rails")
        .0
        .semi_major_axis();
    Ok(plan_hohmann(
        rails.mu,
        rails.semi_major_axis(),
        r_target,
        mun_phase(rw, vessel, ut),
        ut,
    ))
}

/// Advance `vessel` on rails from `from_ut` to `to_ut` in steps of `dt`, applying SOI handoffs. Returns the
/// first transition seen and the time it happened, if any.
pub fn coast(
    rw: &mut RailsWorld,
    vessel: Entity,
    from_ut: f64,
    to_ut: f64,
    dt: f64,
) -> Option<(SoiTransition, f64)> {
    let mut ut = from_ut;
    while ut < to_ut {
        ut = (ut + dt).min(to_ut);
        let t = update_reference_body(&mut rw.world, &rw.index, vessel, ut);
        if t != SoiTransition::None {
            return Some((t, ut));
        }
    }
    None
}

/// Apply an instantaneous prograde `dv_mps` to `vessel` at `ut`, keeping its reference body.
pub fn prograde_burn(rw: &mut RailsWorld, vessel: Entity, dv_mps: f64, ut: f64) -> Orbit {
    let rails = rw.world.get::<OnRails>(vessel).expect("rails").0;
    let (r, v) = rails.state_at(ut);
    let v_new = v + v.normalized() * dv_mps;
    let orbit = Orbit::from_state_vectors(rails.mu, r, v_new, ut);
    rw.world.entity_mut(vessel).insert(OnRails(orbit));
    orbit
}

/// Everything the Mun run measured, for the report.
#[derive(Clone, Debug, PartialEq)]
pub struct MunRun {
    pub parking: ParkingOrbit,
    pub plan: TransferPlan,
    /// Lead angle actually measured at the burn, rad (should equal `plan.lead_angle_rad`).
    pub phase_at_burn_rad: f64,
    pub transfer_apoapsis_m: f64,
    pub encounter_ut: f64,
    /// Periapsis altitude above the Mun's surface on the arrival hyperbola, m (negative = impact course).
    pub mun_periapsis_alt_m: f64,
    pub arrival_speed_mps: f64,
}

/// Park at `altitude_m` above Kerbin at `ut0`, wait for the window, burn, coast until the Mun's SOI.
pub fn fly_to_mun_encounter(rw: &mut RailsWorld, altitude_m: f64, ut0: f64) -> Result<MunRun> {
    let parking = park_in_kerbin_orbit(rw, "bot", altitude_m, 0.0, ut0);
    let vessel = parking.vessel;
    let plan = plan_mun_transfer(rw, vessel, ut0)?;

    // Wait in the parking orbit; nothing should hand the vessel off while we wait.
    if let Some((t, at)) = coast(rw, vessel, ut0, plan.burn_ut, 60.0) {
        return Err(anyhow!("unexpected {t:?} at UT {at} while parked"));
    }
    let phase_at_burn_rad = mun_phase(rw, vessel, plan.burn_ut);
    let transfer = prograde_burn(rw, vessel, plan.dv_mps, plan.burn_ut);
    let transfer_apoapsis_m = transfer.apoapsis();

    let deadline = plan.burn_ut + 1.5 * plan.transfer_time_s;
    let Some((t, encounter_ut)) = coast(rw, vessel, plan.burn_ut, deadline, 30.0) else {
        return Err(anyhow!(
            "no SOI transition within {:.0} s of the burn (apoapsis {:.0} m)",
            deadline - plan.burn_ut,
            transfer_apoapsis_m
        ));
    };
    match t {
        SoiTransition::Encounter { to, .. } if to == rw.mun => {}
        other => return Err(anyhow!("expected a Mun encounter, got {other:?}")),
    }
    let arrival = rw.world.get::<OnRails>(vessel).expect("rails").0;
    let mun = rw.body(rw.mun);
    let (_, v_arr) = arrival.state_at(encounter_ut);
    Ok(MunRun {
        parking,
        plan,
        phase_at_burn_rad,
        transfer_apoapsis_m,
        encounter_ut,
        mun_periapsis_alt_m: arrival.periapsis() - mun.radius_m,
        arrival_speed_mps: v_arr.length(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::builtin_kerbol;

    #[test]
    fn parking_orbit_is_circular_in_the_muns_plane() {
        let mut rw = RailsWorld::new(&builtin_kerbol()).unwrap();
        let park = park_in_kerbin_orbit(&mut rw, "p", 100_000.0, 0.7, 5000.0);
        let o = rw.world.get::<OnRails>(park.vessel).unwrap().0;
        assert!(o.eccentricity() < 1e-9, "e {}", o.eccentricity());
        assert!((o.semi_major_axis() - 700_000.0).abs() < 1e-3);
        let (mun_p, mun_v) = rw.relative_state(rw.mun, rw.kerbin, 5000.0);
        let (vp, vv) = rw.relative_state(park.vessel, rw.kerbin, 5000.0);
        let n_mun = mun_p.cross(mun_v).normalized();
        let n_v = vp.cross(vv).normalized();
        assert!((n_mun - n_v).length() < 1e-9, "planes differ");
        let phase = mun_phase(&rw, park.vessel, 5000.0);
        assert!((phase + 0.7).abs() < 1e-9, "phase {phase}");
    }

    #[test]
    fn bot_reaches_the_muns_soi_from_a_100km_parking_orbit() {
        let mut rw = RailsWorld::new(&builtin_kerbol()).unwrap();
        let run = fly_to_mun_encounter(&mut rw, 100_000.0, 0.0).unwrap();
        println!("{run:#?}");
        let lead_err = (run.phase_at_burn_rad - run.plan.lead_angle_rad).abs();
        assert!(lead_err < 1e-6, "lead angle error {lead_err}");
        assert!(
            (run.transfer_apoapsis_m - 12_000_000.0).abs() < 1_000.0,
            "apoapsis {}",
            run.transfer_apoapsis_m
        );
        let coast = run.encounter_ut - run.plan.burn_ut;
        assert!(
            coast > 0.6 * run.plan.transfer_time_s && coast < run.plan.transfer_time_s,
            "coast {coast} vs transfer {}",
            run.plan.transfer_time_s
        );
        // An ideal Hohmann aims at the Mun's centre, so the arrival hyperbola dips below the surface.
        assert!(
            run.mun_periapsis_alt_m < 200_000.0,
            "periapsis alt {}",
            run.mun_periapsis_alt_m
        );
        assert!(run.arrival_speed_mps > 200.0 && run.arrival_speed_mps < 1000.0);
    }
}

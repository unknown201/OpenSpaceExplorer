//! OSE side of the KSP2 parity trace T40 (warp under thrust, KSP2 run 19): the pilot's stack craft (pod, six
//! 1x4 tanks, Swivel) lit in a 200 km Kerbin orbit, nose held prograde, full throttle 10 s at 1x then 30 s at
//! 10x. Prints mass and orbital speed at the start, after 10 s and after 40 s, as the pilot's steps.log does.
//!
//! cargo run -p ose-bot --example parity_thrustwarp -- tools/ksp2-parity-pilot/craft-stack.json

use anyhow::{Context, Result};
use ose_math::DVec3;
use ose_sim::{bodies, flight, structure, Universe, TICKS_PER_SECOND};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: parity_thrustwarp <craft.json>")?;
    let text = std::fs::read_to_string(&path)?;
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    let craft = ose_importer::craft::craft_from_value(&value)?;
    let own = |f: &str| std::fs::read_to_string(format!("assets/own/{f}"));
    let defs: Vec<ose_data::PartDef> = serde_json::from_str(&own("parts-catalog.json")?)?;
    let res: Vec<ose_data::ResourceDef> = serde_json::from_str(&own("resources.json")?)?;
    let bodies_def: Vec<ose_data::CelestialBody> =
        serde_json::from_str(&own("kerbol-system.json")?)?;
    let mut kerbin = bodies_def
        .into_iter()
        .find(|b| b.id == "Kerbin")
        .context("no Kerbin")?;
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let report = ose_data::staging::delta_v_by_stage(&craft, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let mut u = Universe::with_bodies(&[kerbin]);
    let body = u.body_entity("Kerbin").context("no Kerbin")?;
    let v = u.spawn_powered_on_surface("stack", body, spec, 0.0, 0.0);
    u.set_structure(v, structure::Structure::build(&craft, &defs, &res));
    let b = u.world.get::<bodies::Body>(body).context("body")?.clone();
    let r = b.radius_m + 200_000.0;
    let mut p = u.powered(v).context("powered")?.clone();
    p.r = DVec3::new(r, 0.0, 0.0);
    p.v = DVec3::new(0.0, (b.mu / r).sqrt(), 0.0);
    p.landed = false;
    p.facing = DVec3::new(0.0, 1.0, 0.0);
    u.set_powered(v, p);
    u.stage(v);
    let say = |u: &Universe, what: &str| {
        let p = u.powered(v).unwrap();
        println!(
            "{what}: t {:.2} mass {:.4} t speed {:.2} m/s warp {}",
            u.ut(),
            p.mass_t,
            p.v.length(),
            u.warp()
        );
    };
    say(&u, "lit");
    u.set_controls(v, 1.0, flight::Attitude::Fixed(DVec3::new(0.0, 1.0, 0.0)));
    let t0 = u.ut();
    for _ in 0..(10 * TICKS_PER_SECOND) {
        u.step();
    }
    say(&u, "after 10 s at 1x");
    u.set_warp(10);
    while u.ut() < t0 + 40.0 - 1e-9 {
        let left = ((t0 + 40.0 - u.ut()) * TICKS_PER_SECOND as f64).round() as u32;
        if left < u.warp() {
            u.set_warp(left);
        }
        u.step();
    }
    say(&u, "after 30 s at 10x");
    Ok(())
}

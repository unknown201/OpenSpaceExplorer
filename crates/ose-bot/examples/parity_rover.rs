//! OSE side of the KSP2 parity trace T16 (wheels, driving): a rover on the pad drives full throttle 15 s,
//! coasts 3 s, brakes 8 s, then turns at half throttle and full steer 10 s, as `tools/ksp2-parity-pilot`
//! does with stock Inquisitive Rover.
//!
//! Old header: T17 (RCS): RCS on, translate fore 2 s, right 2 s, then pitch 1 s,
//! the schedule `tools/ksp2-parity-pilot` flies with stock Kerbal K2 in a 200 km orbit. Monopropellant use
//! shows in `mass_t`.
//!
//! Based on the T5/T7 rotation runner (docs/ksp2-side-by-side-gaps.md): a KSP2 workspace craft in a
//! 200 km orbit with SAS off gets full pitch, then yaw, then roll input for 1 s each (4 s apart), then SAS
//! on for 12 s, the same schedule `tools/ksp2-parity-pilot` flies in KSP2. Logs facing and roll reference
//! of every streamed state; angular rates come from their change between states.
//!
//! `OSE_PARITY_SITE=Runway_1` starts it on the runway instead of the pad. The heading turns with `roll_ref` (the craft's +Z laid on the ground).
//!
//! cargo run -p ose-bot --example parity_rover -- craft.json ose-rover.csv

use anyhow::{bail, Context, Result};
use ose_bot::client::Client;
use ose_net::{ClientMessage, ServerMessage, SessionMode};
use std::io::Write;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let craft_path = args
        .next()
        .context("usage: parity_rover <craft.json> <out.csv>")?;
    let out = args.next().unwrap_or_else(|| "ose-rotation.csv".into());
    let text = std::fs::read_to_string(&craft_path)?;
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    let craft = ose_importer::craft::craft_from_value(&value)?;

    let server = ose_bot::harness::spawn_server(Duration::from_secs(900)).await?;
    let mut c = Client::connect(&server.addr, "parity")
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    c.create_session("parity", SessionMode::Sandbox).await?;
    c.join_session().await?;
    c.send(&ClientMessage::SaveCraft { craft }).await?;
    let saved = c
        .wait_for(Duration::from_secs(20), "CraftSaved", |_, m| match m {
            ServerMessage::CraftSaved { id, reason, .. } => Some((*id, reason.clone())),
            _ => None,
        })
        .await?;
    let Some(id) = saved.0 else {
        bail!("craft not saved: {:?}", saved.1)
    };
    // OSE's launch pad is smaller than KSP2's: the rover runs off its edge before the brakes, so
    // OSE_PARITY_SITE (a site id such as Runway_1) can start it somewhere longer and flat
    let l = match std::env::var("OSE_PARITY_SITE") {
        Ok(site) => c.launch_saved_at(id, Some(site)).await?,
        Err(_) => c.launch_saved_on_pad(id).await?,
    };
    println!("on the pad: vessel {} wet {:.3} t", l.vessel, l.wet_mass_t);
    c.stage(l.vessel).await?;
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(3) {
        c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
    }
    let mut f = std::fs::File::create(&out)?;
    writeln!(
        f,
        "t,throttle,steer,brakes,srf_speed,alt_m,fx,fy,fz,rx,ry,rz,situation,ut"
    )?;
    // (seconds, throttle, steer, brakes)
    let schedule: [(f64, f32, f32, bool); 6] = [
        (1.0, 1.0, 0.0, false),
        (16.0, 0.0, 0.0, false),
        (19.0, 0.0, 0.0, true),
        (27.0, 0.5, 1.0, false),
        (37.0, 0.0, 0.0, false),
        (41.0, 0.0, 0.0, false),
    ];
    let mut next = 0;
    let mut cmd = (0.0f32, 0.0f32, false);
    let mut t0 = None;
    let wall = Instant::now();
    while wall.elapsed() < Duration::from_secs(120) {
        let v = c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
        let ut = c.latest.ut();
        let t = ut - *t0.get_or_insert(ut);
        while next < schedule.len() && t >= schedule[next].0 {
            let (_, throttle, steer, brakes) = schedule[next];
            cmd = (throttle, steer, brakes);
            c.send(&ClientMessage::Drive {
                vessel: l.vessel,
                throttle,
                steer,
                brakes,
            })
            .await?;
            next += 1;
        }
        let Some(r) = v.flight.as_ref() else { continue };
        writeln!(
            f,
            "{t},{},{},{},{},{},{},{},{},{},{},{},{:?},{ut}",
            cmd.0,
            cmd.1,
            cmd.2,
            r.surface_speed_mps,
            r.altitude_m,
            r.facing.x,
            r.facing.y,
            r.facing.z,
            r.roll_ref.x,
            r.roll_ref.y,
            r.roll_ref.z,
            r.situation
        )?;
        if t > 41.0 {
            break;
        }
    }
    println!("trace in {out}");
    drop(server);
    Ok(())
}

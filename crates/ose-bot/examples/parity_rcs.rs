//! OSE side of the KSP2 parity trace T17 (RCS): RCS on, translate fore 2 s, right 2 s, then pitch 1 s,
//! the schedule `tools/ksp2-parity-pilot` flies with stock Kerbal K2 in a 200 km orbit. Monopropellant use
//! shows in `mass_t`.
//!
//! Based on the T5/T7 rotation runner (docs/ksp2-side-by-side-gaps.md): a KSP2 workspace craft in a
//! 200 km orbit with SAS off gets full pitch, then yaw, then roll input for 1 s each (4 s apart), then SAS
//! on for 12 s, the same schedule `tools/ksp2-parity-pilot` flies in KSP2. Logs facing and roll reference
//! of every streamed state; angular rates come from their change between states.
//!
//! cargo run -p ose-bot --example parity_rcs -- craft.json ose-rotation.csv

use anyhow::{bail, Context, Result};
use ose_bot::client::Client;
use ose_net::{AttitudeMode, ClientMessage, ServerMessage, SessionMode};
use std::io::Write;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let craft_path = args
        .next()
        .context("usage: parity_rcs <craft.json> <out.csv>")?;
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
    let l = c.launch_saved_to_orbit(id, 200_000.0).await?;
    println!("in orbit: vessel {} wet {:.3} t", l.vessel, l.wet_mass_t);
    c.set_controls(l.vessel, 0.0, AttitudeMode::Free).await?;
    c.send(&ClientMessage::ActionGroup {
        vessel: l.vessel,
        group: ose_net::ActionGroup::Rcs,
        on: Some(true),
    })
    .await?;

    let mut f = std::fs::File::create(&out)?;
    writeln!(f, "t,tx,ty,pitch,fx,fy,fz,rx,ry,rz,mass_t,ut")?;
    // schedule (seconds from start, axis input) matching the KSP2 pilot: 2 s idle, then 1 s per axis, 4 s apart
    // (seconds, translate x right, y fore, pitch)
    let schedule: [(f64, [f32; 3]); 6] = [
        (2.0, [0.0, 1.0, 0.0]),
        (4.0, [0.0, 0.0, 0.0]),
        (7.0, [1.0, 0.0, 0.0]),
        (9.0, [0.0, 0.0, 0.0]),
        (12.0, [0.0, 0.0, 1.0]),
        (13.0, [0.0, 0.0, 0.0]),
    ];
    let mut next = 0;
    let mut input = [0.0f32; 3];
    let mut t0 = None;
    let wall = Instant::now();
    while wall.elapsed() < Duration::from_secs(120) {
        let v = c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
        let ut = c.latest.ut();
        let t = ut - *t0.get_or_insert(ut);
        while next < schedule.len() && t >= schedule[next].0 {
            input = schedule[next].1;
            c.send(&ClientMessage::Translate {
                vessel: l.vessel,
                x: input[0],
                y: input[1],
                z: 0.0,
            })
            .await?;
            c.send(&ClientMessage::SetRotation {
                vessel: l.vessel,
                pitch: input[2],
                yaw: 0.0,
                roll: 0.0,
            })
            .await?;
            next += 1;
        }
        let Some(r) = v.flight.as_ref() else { continue };
        writeln!(
            f,
            "{t},{},{},{},{},{},{},{},{},{},{},{ut}",
            input[0],
            input[1],
            input[2],
            r.facing.x,
            r.facing.y,
            r.facing.z,
            r.roll_ref.x,
            r.roll_ref.y,
            r.roll_ref.z,
            r.mass_t
        )?;
        if t > 17.0 {
            break;
        }
    }
    println!("trace in {out}");
    drop(server);
    Ok(())
}

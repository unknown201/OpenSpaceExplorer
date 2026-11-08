//! OSE side of the KSP2 parity trace T19/T20 (EVA): the first Kerbal steps out of the pod on the pad, lets
//! go of the ladder, walks 6 s, stands 3 s, runs 6 s, stands 3 s, jumps, then walks back 4 s, as
//! `tools/ksp2-parity-pilot` does.
//!
//! Based on the T17 runner (RCS): RCS on, translate fore 2 s, right 2 s, then pitch 1 s,
//! the schedule `tools/ksp2-parity-pilot` flies with stock Kerbal K2 in a 200 km orbit. Monopropellant use
//! shows in `mass_t`.
//!
//! Based on the T5/T7 rotation runner (docs/ksp2-side-by-side-gaps.md): a KSP2 workspace craft in a
//! 200 km orbit with SAS off gets full pitch, then yaw, then roll input for 1 s each (4 s apart), then SAS
//! on for 12 s, the same schedule `tools/ksp2-parity-pilot` flies in KSP2. Logs facing and roll reference
//! of every streamed state; angular rates come from their change between states.
//!
//! cargo run -p ose-bot --example parity_eva -- craft.json ose-rotation.csv

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
        .context("usage: parity_eva <craft.json> <out.csv>")?;
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
    let l = c.launch_saved_on_pad(id).await?;
    println!("on the pad: vessel {}", l.vessel);
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(3) {
        c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
    }
    c.send(&ClientMessage::GoEva {
        vessel: l.vessel,
        kerbal: None,
    })
    .await?;
    let kerbal = c
        .wait_for(Duration::from_secs(20), "CrewAck", |_, m| match m {
            ServerMessage::CrewAck {
                new_vessel,
                accepted,
                reason,
                ..
            } => Some((*new_vessel, *accepted, reason.clone())),
            _ => None,
        })
        .await?;
    let Some(k) = kerbal.0 else {
        bail!("EVA refused: {:?}", kerbal.2)
    };
    println!("kerbal vessel {k}");
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(5) {
        c.vessel_state(k, Duration::from_secs(10)).await?;
    }
    let mut f = std::fs::File::create(&out)?;
    writeln!(f, "t,phase,srf_speed,alt_m,vspeed,on_ladder,situation,ut")?;
    // (seconds, phase, throttle, heading, run, jump)
    let schedule: [(f64, &str, f64, f64, bool, bool); 9] = [
        (1.0, "let go", 0.0, 90.0, false, true),
        (5.5, "walk", 1.0, 90.0, false, false),
        (11.5, "stand", 0.0, 90.0, false, false),
        (14.5, "run", 1.0, 90.0, true, false),
        (20.5, "stand", 0.0, 90.0, false, false),
        (23.5, "jump", 0.0, 90.0, false, true),
        (28.0, "walk back", 1.0, 270.0, false, false),
        (32.0, "stand", 0.0, 270.0, false, false),
        (34.0, "end", 0.0, 270.0, false, false),
    ];
    let mut next = 0;
    let mut phase = "start";
    let mut t0 = None;
    let wall = Instant::now();
    while wall.elapsed() < Duration::from_secs(120) {
        let v = c.vessel_state(k, Duration::from_secs(10)).await?;
        let ut = c.latest.ut();
        let t = ut - *t0.get_or_insert(ut);
        while next < schedule.len() && t >= schedule[next].0 {
            let (_, p, throttle, heading, run, jump) = schedule[next];
            phase = p;
            c.send(&ClientMessage::EvaControls {
                vessel: k,
                run,
                jump,
                grab: false,
                climb: 0.0,
            })
            .await?;
            c.set_controls(
                k,
                throttle,
                AttitudeMode::PitchHeading {
                    pitch_deg: 0.0,
                    heading_deg: heading,
                },
            )
            .await?;
            next += 1;
        }
        let Some(r) = v.flight.as_ref() else { continue };
        writeln!(
            f,
            "{t},{phase},{},{},{},{},{:?},{ut}",
            r.surface_speed_mps,
            r.altitude_m,
            r.vertical_speed_mps,
            r.on_ladder.is_some(),
            r.situation
        )?;
        if t > 34.5 {
            break;
        }
    }
    println!("trace in {out}");
    drop(server);
    Ok(())
}

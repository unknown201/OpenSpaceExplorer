//! OSE side of the KSP2 parity trace T41 (jet engines: spool-up, intake air, engine modes), the schedule
//! `tools/ksp2-parity-pilot` (scenario `jet`) flies with stock Stock04_Jet on Runway_1:
//!   idle 5 s (staged, throttle 0, brakes on), static-up 20 s (full throttle, brakes on), static-down 10 s
//!   (throttle cut), roll 25 s (brakes off, full throttle), reverse 16 s (cut, Wheesley to Reverse mode,
//!   full throttle after 1 s), stop 8 s (cut, brakes on).
//! Logs the delivered thrust of every streamed state.
//!
//! cargo run -p ose-bot --example parity_jet -- Stock04_Jet.json ose-jet.csv

use anyhow::{bail, Context, Result};
use ose_bot::client::Client;
use ose_net::{AttitudeMode, ClientMessage, ServerMessage, SessionMode};
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Throttle(f64),
    Brakes(bool),
    Reverse,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let craft_path = args
        .next()
        .context("usage: parity_jet <craft.json> <out.csv>")?;
    let out = args.next().unwrap_or_else(|| "ose-jet.csv".into());
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
    let l = c.launch_saved_at(id, Some("Runway_1".into())).await?;
    println!(
        "on the runway: vessel {} wet {:.3} t",
        l.vessel, l.wet_mass_t
    );
    let drive = |brakes: bool| ClientMessage::Drive {
        vessel: l.vessel,
        throttle: 0.0,
        steer: 0.0,
        brakes,
    };
    c.send(&drive(true)).await?;
    let settle = Instant::now();
    let mut facing = None;
    while settle.elapsed() < Duration::from_secs(3) {
        let v = c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
        if let Some(r) = v.flight.as_ref() {
            facing = Some(r.facing);
        }
    }
    let hold = AttitudeMode::Fixed(facing.context("no flight readout")?);
    c.set_controls(l.vessel, 0.0, hold).await?;
    c.stage(l.vessel).await?;

    let mut f = std::fs::File::create(&out)?;
    writeln!(
        f,
        "t,phase,throttle,brakes,thrust_kn,srf_speed,alt_m,firing,situation,ut"
    )?;
    // (seconds after staging, phase, step)
    let schedule: Vec<(f64, &str, Step)> = vec![
        (5.0, "static-up", Step::Throttle(1.0)),
        (25.0, "static-down", Step::Throttle(0.0)),
        (35.0, "roll", Step::Brakes(false)),
        (35.0, "roll", Step::Throttle(1.0)),
        (60.0, "reverse", Step::Throttle(0.0)),
        (60.0, "reverse", Step::Reverse),
        (61.0, "reverse", Step::Throttle(1.0)),
        (76.0, "stop", Step::Throttle(0.0)),
        (76.0, "stop", Step::Brakes(true)),
    ];
    let mut next = 0;
    let (mut phase, mut throttle, mut brakes) = ("idle", 0.0, true);
    let mut engine = None;
    let mut t0 = None;
    let wall = Instant::now();
    while wall.elapsed() < Duration::from_secs(300) {
        let v = c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
        let ut = c.latest.ut();
        let t = ut - *t0.get_or_insert(ut);
        let Some(r) = v.flight.as_ref() else { continue };
        if engine.is_none() {
            engine = r.firing_engines.first().copied();
        }
        while next < schedule.len() && t >= schedule[next].0 {
            let (_, name, step) = schedule[next];
            phase = name;
            match step {
                Step::Throttle(x) => {
                    throttle = x;
                    c.set_controls(l.vessel, x, hold).await?;
                }
                Step::Brakes(on) => {
                    brakes = on;
                    c.send(&drive(on)).await?;
                }
                Step::Reverse => {
                    let part = engine.context("no engine fired")?;
                    c.send(&ClientMessage::PartAction {
                        vessel: l.vessel,
                        part,
                        action: "mode".into(),
                        on: true,
                    })
                    .await?;
                }
            }
            next += 1;
        }
        writeln!(
            f,
            "{t},{phase},{throttle},{},{},{},{},{},{:?},{ut}",
            u8::from(brakes),
            r.thrust_kn,
            r.surface_speed_mps,
            r.altitude_m,
            r.firing_engines.len(),
            r.situation
        )?;
        if t > 84.0 {
            break;
        }
    }
    println!("trace in {out}");
    drop(server);
    Ok(())
}

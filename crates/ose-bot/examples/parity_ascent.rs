//! OSE side of the KSP2 parity trace T1/T2 (docs/ksp2-side-by-side-gaps.md): import a KSP2 workspace craft,
//! put it on the pad, hold attitude up, full throttle, fire the first stage and log the flight readout of
//! every streamed state in the same columns as `tools/ksp2-parity-pilot` writes.
//!
//! cargo run -p ose-bot --example parity_ascent -- craft.json ose-trace.csv

use anyhow::{bail, Context, Result};
use ose_bot::client::Client;
use ose_net::{AttitudeMode, ClientMessage, FlightSituation, ServerMessage, SessionMode};
use std::io::Write;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let craft_path = args
        .next()
        .context("usage: parity_ascent <craft.json> <out.csv>")?;
    let out = args.next().unwrap_or_else(|| "ose-trace.csv".into());
    let text = std::fs::read_to_string(&craft_path)?;
    let text = text.trim_start_matches('\u{feff}');
    let value: serde_json::Value = serde_json::from_str(text)?;
    let craft = ose_importer::craft::craft_from_value(&value)?;
    println!("craft {:?}: {} parts", craft.name, craft.parts.len());

    let server = ose_bot::harness::spawn_server(Duration::from_secs(900)).await?;
    let mut c = Client::connect(&server.addr, "parity")
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    c.create_session("parity", SessionMode::Sandbox).await?;
    c.join_session().await?;
    c.send(&ClientMessage::SaveCraft { craft }).await?;
    let id = c
        .wait_for(Duration::from_secs(20), "CraftSaved", |_, m| match m {
            ServerMessage::CraftSaved { id, reason, .. } => Some((*id, reason.clone())),
            _ => None,
        })
        .await?;
    let Some(id) = id.0 else {
        bail!("craft not saved: {:?}", id.1)
    };
    let l = c.launch_saved_on_pad(id).await?;
    println!(
        "on the pad: vessel {} wet {:.3} t dry {:.3} t",
        l.vessel, l.wet_mass_t, l.dry_mass_t
    );
    // let it settle on the pad as the KSP2 run does
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(5) {
        c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
    }
    c.set_controls(l.vessel, 1.0, AttitudeMode::Up).await?;
    c.stage(l.vessel).await?;

    let mut f = std::fs::File::create(&out)?;
    writeln!(
        f,
        "t,alt_m,vspeed,srf_speed,mach,q_kpa,density,static_kpa,mass_t,throttle,temp_max_k,situation,ut"
    )?;
    let mut t0 = None;
    let mut top: f64 = 0.0;
    let mut falling = false;
    let wall = Instant::now();
    while wall.elapsed() < Duration::from_secs(900) {
        let v = match c.vessel_state(l.vessel, Duration::from_secs(10)).await {
            Ok(v) => v,
            Err(e) => {
                println!("stream ended: {e}");
                break;
            }
        };
        let ut = c.latest.ut();
        let Some(r) = v.flight.as_ref() else { continue };
        let t = ut - *t0.get_or_insert(ut);
        top = top.max(r.altitude_m);
        falling |= r.vertical_speed_mps < 0.0 && r.altitude_m > 1000.0;
        writeln!(
            f,
            "{t},{},{},{},{},{},{},{},{},{},{},{:?},{ut}",
            r.altitude_m,
            r.vertical_speed_mps,
            r.surface_speed_mps,
            r.mach_number,
            r.dynamic_pressure_kpa,
            r.atmosphere_density,
            r.static_pressure_kpa,
            r.mass_t,
            r.throttle,
            r.heat.iter().map(|h| h.1).fold(0.0f32, f32::max),
            r.situation
        )?;
        if falling
            && matches!(
                r.situation,
                FlightSituation::Landed | FlightSituation::Splashed | FlightSituation::Crashed
            )
        {
            break;
        }
    }
    println!("apoapsis {top:.1} m, trace in {out}");
    drop(server);
    Ok(())
}

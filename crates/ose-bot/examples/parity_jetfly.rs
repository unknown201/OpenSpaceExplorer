//! OSE side of the KSP2 parity trace T10 (control surfaces and lift), the schedule `tools/ksp2-parity-pilot`
//! (scenario `jetfly`) flies with stock Stock04_Jet from Runway_1, SAS holding:
//!   roll (full throttle, brakes off) to 60 m/s, rotate (full pitch up) until the nose is 10 degrees up (at
//!   most 10 s), climb 20 s, pitch (-0.5) 1.5 s then 8 s released, bank (roll +1) 1 s then 8 s released.
//! Logs speed, vertical speed, altitude, pitch and roll against the horizon and thrust of every streamed
//! state.
//!
//! cargo run -p ose-bot --example parity_jetfly -- Stock04_Jet.json ose-jetfly.csv

use anyhow::{bail, Context, Result};
use ose_bot::client::Client;
use ose_net::{AttitudeMode, ClientMessage, ServerMessage, SessionMode};
use std::io::Write;
use std::time::{Duration, Instant};

/// Kerbin's gravitational parameter, m³/s².
const KERBIN_MU: f64 = 3.531_6e12;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let craft_path = args
        .next()
        .context("usage: parity_jetfly <craft.json> <out.csv>")?;
    let out = args.next().unwrap_or_else(|| "ose-jetfly.csv".into());
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
    let settle = Instant::now();
    let mut facing = None;
    while settle.elapsed() < Duration::from_secs(3) {
        let v = c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
        if let Some(r) = v.flight.as_ref() {
            facing = Some(r.facing);
        }
    }
    let hold = AttitudeMode::Fixed(facing.context("no flight readout")?);
    c.send(&ClientMessage::Drive {
        vessel: l.vessel,
        throttle: 0.0,
        steer: 0.0,
        brakes: false,
    })
    .await?;
    c.set_controls(l.vessel, 1.0, hold).await?;
    c.stage(l.vessel).await?;

    let rotate = |pitch: f32, roll: f32| ClientMessage::SetRotation {
        vessel: l.vessel,
        pitch,
        yaw: 0.0,
        roll,
    };
    let mut f = std::fs::File::create(&out)?;
    writeln!(
        f,
        "t,phase,srf_speed,vspeed,alt_m,pitch_deg,roll_deg,in_pitch,in_roll,thrust_kn,situation,ut"
    )?;
    let mut phase = "roll";
    let mut since = 0.0;
    let (mut in_pitch, mut in_roll) = (0.0f32, 0.0f32);
    let mut t0 = None;
    let wall = Instant::now();
    while wall.elapsed() < Duration::from_secs(400) {
        let v = c.vessel_state(l.vessel, Duration::from_secs(10)).await?;
        let ut = c.latest.ut();
        let t = ut - *t0.get_or_insert(ut);
        let Some(r) = v.flight.as_ref() else { continue };
        // up from the vessel's place around Kerbin (`position_m` is star space)
        let up = ose_bot::client::orbit_from(KERBIN_MU, &v.orbit)
            .state_at(ut)
            .0
            .normalized();
        let pitch = r.facing.dot(up).clamp(-1.0, 1.0).asin().to_degrees();
        // bank: the craft's belly (+Z, `roll_ref`) against the local down, positive right wing down
        let right = r.facing.cross(up).normalized();
        let roll = r
            .roll_ref
            .dot(right)
            .atan2(-r.roll_ref.dot(up))
            .to_degrees();
        let next = match phase {
            "roll" if r.surface_speed_mps >= 60.0 => Some(("rotate", 1.0, 0.0)),
            "rotate" if pitch >= 10.0 || t - since >= 10.0 => Some(("climb", 0.0, 0.0)),
            "climb" if t - since >= 20.0 => Some(("pitch", -0.5, 0.0)),
            "pitch" if t - since >= 1.5 && in_pitch != 0.0 => Some(("pitch", 0.0, 0.0)),
            "pitch" if t - since >= 9.5 => Some(("bank", 0.0, 1.0)),
            "bank" if t - since >= 1.0 && in_roll != 0.0 => Some(("bank", 0.0, 0.0)),
            "bank" if t - since >= 9.0 => Some(("done", 0.0, 0.0)),
            _ => None,
        };
        if let Some((name, p, ro)) = next {
            if name != phase {
                since = t;
            }
            phase = name;
            in_pitch = p;
            in_roll = ro;
            c.send(&rotate(p, ro)).await?;
        }
        if phase == "done" {
            break;
        }
        writeln!(
            f,
            "{t},{phase},{},{},{},{pitch},{roll},{in_pitch},{in_roll},{},{:?},{ut}",
            r.surface_speed_mps, r.vertical_speed_mps, r.altitude_m, r.thrust_kn, r.situation
        )?;
    }
    println!("trace in {out}");
    drop(server);
    Ok(())
}

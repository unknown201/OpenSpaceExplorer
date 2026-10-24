//! Fly only the ascent against a fresh server and write a CSV trace, for tuning the controller.
//!
//! cargo run -p ose-bot --example ascent -- trace.csv

use ose_bot::ascent::{fly_ascent, AscentParams};
use ose_bot::client::Client;
use ose_net::SessionMode;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let trace = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ascent-trace.csv".into());
    let server = ose_bot::harness::spawn_server(Duration::from_secs(600)).await?;
    let mut c = Client::connect(&server.addr, "ascent-trace")
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    c.create_session("ascent trace", SessionMode::Sandbox)
        .await?;
    c.join_session().await?;
    let list = c.craft_list().await?;
    let k2 = list
        .iter()
        .find(|s| s.stock && s.name == "Kerbal K2")
        .expect("K2 in the stock library");
    let l = c.launch_saved_on_pad(k2.id).await?;
    let kerbin = c.body_index("Kerbin").expect("Kerbin");
    let p = AscentParams {
        trace: Some(trace.clone().into()),
        ..AscentParams::default()
    };
    let r = fly_ascent(&mut c, l.vessel, kerbin, &p).await;
    println!("{r:#?}\ntrace in {trace}");
    drop(server);
    Ok(())
}

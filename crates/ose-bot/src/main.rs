//! `ose-bot`: play the Mun campaign against a server and print how far it got.
//!
//! cargo run -p ose-bot -- --spawn-server          # start a local ose-server, play, report
//! cargo run -p ose-bot -- --server 127.0.0.1:7777  # against a running server
//! cargo run -p ose-bot                             # local steps only
//!
//! Exit status is 1 when any step FAILED; BLOCKED and SKIPPED steps are expected while the game grows.

use clap::Parser;
use ose_bot::{print_report, run_mun_campaign, BotConfig};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "ose-bot")]
struct Args {
    /// `host:port` of a running ose-server.
    #[arg(long, conflicts_with = "spawn_server")]
    server: Option<String>,
    /// Build and start a local ose-server on a free port for this run.
    #[arg(long)]
    spawn_server: bool,
    /// Converted game data (falls back to the built-in Kerbol fixture when absent).
    #[arg(long)]
    game_data: Option<PathBuf>,
    /// Directory of extracted stock assemblies.
    #[arg(long)]
    stock_dir: Option<PathBuf>,
    /// Parking orbit altitude above Kerbin, km.
    #[arg(long, default_value_t = 100.0)]
    parking_altitude_km: f64,
    /// Print the results as JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut cfg = BotConfig {
        server: args.server.clone(),
        parking_altitude_m: args.parking_altitude_km * 1000.0,
        ..BotConfig::default()
    };
    if let Some(p) = args.game_data {
        cfg.game_data = p;
    }
    if let Some(p) = args.stock_dir {
        cfg.stock_dir = p;
    }
    let server = if args.spawn_server {
        let s = ose_bot::harness::spawn_server(Duration::from_secs(300)).await?;
        eprintln!("ose-server listening on {}", s.addr);
        cfg.server = Some(s.addr.clone());
        Some(s)
    } else {
        None
    };

    let results = run_mun_campaign(&cfg).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_report(&results);
    }
    if results.iter().any(|r| r.outcome.is_failed()) {
        // process::exit skips destructors: stop the spawned server first
        drop(server);
        std::process::exit(1);
    }
    Ok(())
}

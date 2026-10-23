//! Authoritative game server. Phase 0: HTTP health endpoint and a ticking universe.

use axum::{routing::get, Router};
use clap::Parser;
use ose_sim::Universe;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(name = "ose-server")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7777")]
    listen: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    let args = Args::parse();

    let universe = Arc::new(Mutex::new(Universe::new()));
    let sim = universe.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(
            1000 / ose_sim::TICKS_PER_SECOND,
        ));
        loop {
            interval.tick().await;
            sim.lock().expect("sim lock").step();
        }
    });

    let app = Router::new().route(
        "/health",
        get({
            let u = universe.clone();
            move || async move { format!("ok tick={}", u.lock().expect("sim lock").tick().0) }
        }),
    );

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, "server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

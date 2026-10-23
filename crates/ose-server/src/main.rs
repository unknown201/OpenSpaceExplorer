//! Authoritative game server. Phase 0: HTTP health endpoint and a ticking universe.
//! With `--static-dir`, it also serves the built web client (`trunk build` output) so one
//! process is the whole deployment (docker/Dockerfile).

use axum::{routing::get, Router};
use clap::Parser;
use ose_sim::Universe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Parser, Debug)]
#[command(name = "ose-server")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7777")]
    listen: String,
    /// Directory with the built web client to serve at `/` (its `index.html` answers unknown paths).
    #[arg(long)]
    static_dir: Option<PathBuf>,
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

    let mut app = Router::new().route(
        "/health",
        get({
            let u = universe.clone();
            move || async move { format!("ok tick={}", u.lock().expect("sim lock").tick().0) }
        }),
    );
    if let Some(dir) = &args.static_dir {
        let index = ServeFile::new(dir.join("index.html"));
        app = app.fallback_service(ServeDir::new(dir).fallback(index));
        tracing::info!(dir = %dir.display(), "serving web client");
    }

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, "server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

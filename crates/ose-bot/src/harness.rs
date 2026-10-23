//! Spawns a local `ose-server` for the bot to play against and tears it down afterwards.

use anyhow::{bail, Context, Result};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running server process, killed on drop.
pub struct ServerProcess {
    child: Child,
    /// `host:port` the server listens on.
    pub addr: String,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Ask the OS for a free loopback port (released before the server binds it, so a small race is possible).
pub fn free_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Start `ose-server` via `cargo run -p ose-server` from the workspace root and wait until `/health`
/// answers. The dev profile artifacts are shared with `cargo test`, so this is a quick relink at most.
pub async fn spawn_server(timeout: Duration) -> Result<ServerProcess> {
    let port = free_port()?;
    let addr = format!("127.0.0.1:{port}");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let child = Command::new(cargo)
        .args([
            "run",
            "-q",
            "-p",
            "ose-server",
            "--",
            "--listen",
            &addr,
            "--save",
            "none",
            "--db",
            ":memory:",
        ])
        .current_dir(crate::data::repo_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn cargo run -p ose-server")?;
    let mut proc = ServerProcess { child, addr };
    let start = Instant::now();
    loop {
        if let Some(status) = proc.child.try_wait()? {
            bail!("ose-server exited early with {status}");
        }
        if crate::client::health(&proc.addr).await.is_ok() {
            return Ok(proc);
        }
        if start.elapsed() > timeout {
            bail!("ose-server did not answer /health within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

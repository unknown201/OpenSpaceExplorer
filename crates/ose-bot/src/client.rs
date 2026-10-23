//! The bot's view of `ose-server`: the HTTP health probe and a WebSocket session speaking the `ose-net`
//! protocol (postcard-encoded binary frames).
//!
//! After `Hello`, the server streams `TimeState`/`Snapshot`/`Vessels` at `SNAPSHOT_HZ` and `Trajectory` at
//! `TRAJECTORY_HZ`. [`Client`] keeps the latest of each in [`Latest`] so steps can ask questions like "what
//! is my vessel's orbit now" or "wait until my vessel is around the Mun" without parsing the stream
//! themselves.

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use ose_data::Tick;
use ose_math::{Elements, Orbit};
use ose_net::{
    BodyInfo, BodyState, ClientMessage, OrbitElements, ServerMessage, TrajectoryPatch, VesselState,
    PROTOCOL_VERSION,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub use ose_net::WS_PATH;

/// What `GET /health` said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub tick: Tick,
}

/// `base` is `host:port`. A hand-rolled HTTP/1.0 request keeps the bot free of an HTTP client dependency.
pub async fn health(base: &str) -> Result<Health> {
    let mut stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(base))
        .await
        .context("connect timed out")?
        .with_context(|| format!("connect to {base}"))?;
    stream
        .write_all(format!("GET /health HTTP/1.0\r\nHost: {base}\r\n\r\n").as_bytes())
        .await?;
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
        .await
        .context("read timed out")??;
    let text = String::from_utf8_lossy(&buf);
    let status = text.lines().next().unwrap_or_default().to_string();
    if !status.contains(" 200 ") {
        bail!("/health answered {status:?}");
    }
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .ok_or_else(|| anyhow!("no body in {text:?}"))?;
    parse_health(body.trim())
}

pub fn parse_health(body: &str) -> Result<Health> {
    let tick = body
        .strip_prefix("ok tick=")
        .ok_or_else(|| anyhow!("unexpected health body {body:?}"))?
        .trim()
        .parse::<u64>()
        .with_context(|| format!("tick in {body:?}"))?;
    Ok(Health { tick: Tick(tick) })
}

/// Why a WebSocket session could not be opened.
#[derive(Debug)]
pub enum ConnectError {
    /// The server answered the upgrade with an HTTP error: it has no WebSocket endpoint at that path.
    NoEndpoint {
        status: u16,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::NoEndpoint { status } => {
                write!(f, "WebSocket upgrade refused with HTTP {status}")
            }
            ConnectError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// `ose_net::OrbitElements` as an `ose_math::Orbit` around a body with parameter `mu`.
pub fn orbit_from(mu: f64, o: &OrbitElements) -> Orbit {
    Orbit::from_elements(
        mu,
        Elements {
            semi_major_axis: o.semi_major_axis_m,
            eccentricity: o.eccentricity,
            inclination_deg: o.inclination_deg,
            longitude_of_ascending_node_deg: o.longitude_of_ascending_node_deg,
            argument_of_periapsis_deg: o.argument_of_periapsis_deg,
            mean_anomaly_at_epoch: o.mean_anomaly_at_epoch_rad,
            epoch: o.epoch_s,
        },
    )
}

/// The most recent state the server streamed.
#[derive(Clone, Debug, Default)]
pub struct Latest {
    pub tick: Tick,
    pub warp: u32,
    pub warp_limited_by: Option<u64>,
    pub bodies: Vec<BodyState>,
    pub vessels: Vec<VesselState>,
    /// Latest trajectory per vessel id, with the tick it was computed at.
    pub trajectories: HashMap<u64, (Tick, Vec<TrajectoryPatch>)>,
    /// Latest `BurnState` per vessel id: (scheduled, reason).
    pub burn_states: HashMap<u64, (bool, Option<String>)>,
}

impl Latest {
    pub fn ut(&self) -> f64 {
        self.tick.seconds()
    }
    pub fn vessel(&self, id: u64) -> Option<&VesselState> {
        self.vessels.iter().find(|v| v.id == id)
    }
}

/// A protocol session.
pub struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pub player_id: u64,
    pub token: String,
    pub catalog: Vec<BodyInfo>,
    pub latest: Latest,
}

impl Client {
    /// Open a WebSocket to `ws://{base}{WS_PATH}` and complete the handshake: `Hello` -> `Welcome` ->
    /// `BodyCatalog`.
    pub async fn connect(base: &str, display_name: &str) -> Result<Client, ConnectError> {
        let url = format!("ws://{base}{WS_PATH}");
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .map_err(|_| ConnectError::Other(anyhow!("connect to {url} timed out")))?;
        let ws = match attempt {
            Ok((ws, _)) => ws,
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                return Err(ConnectError::NoEndpoint {
                    status: resp.status().as_u16(),
                })
            }
            Err(e) => {
                return Err(ConnectError::Other(
                    anyhow!(e).context(format!("connect to {url}")),
                ))
            }
        };
        let mut c = Client {
            ws,
            player_id: 0,
            token: String::new(),
            catalog: Vec::new(),
            latest: Latest::default(),
        };
        c.hello(display_name).await.map_err(ConnectError::Other)?;
        Ok(c)
    }

    async fn hello(&mut self, display_name: &str) -> Result<()> {
        self.send(&ClientMessage::Hello {
            protocol: PROTOCOL_VERSION,
            token: None,
            display_name: display_name.into(),
        })
        .await?;
        match self.recv_raw(Duration::from_secs(10)).await? {
            ServerMessage::Welcome {
                player_id,
                token,
                tick,
            } => {
                self.player_id = player_id;
                self.token = token;
                self.latest.tick = tick;
            }
            ServerMessage::Rejected { reason } => bail!("server rejected hello: {reason}"),
            other => bail!("expected Welcome, got {other:?}"),
        }
        match self.recv_raw(Duration::from_secs(10)).await? {
            ServerMessage::BodyCatalog { bodies } => self.catalog = bodies,
            other => bail!("expected BodyCatalog after Welcome, got {other:?}"),
        }
        Ok(())
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let bytes = ose_net::encode(msg)?;
        self.ws.send(Message::Binary(bytes.into())).await?;
        Ok(())
    }

    /// Next binary protocol frame, skipping pings and text; `Err` on close or timeout.
    async fn recv_raw(&mut self, timeout: Duration) -> Result<ServerMessage> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let next = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .context("timed out waiting for the server")?;
            match next {
                Some(Ok(Message::Binary(b))) => return Ok(ose_net::decode(&b)?),
                Some(Ok(Message::Close(frame))) => bail!("server closed the session: {frame:?}"),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
                None => bail!("stream ended"),
            }
        }
    }

    /// Receive one message, fold it into `latest`, and return it.
    pub async fn poll(&mut self, timeout: Duration) -> Result<ServerMessage> {
        let msg = self.recv_raw(timeout).await?;
        match &msg {
            ServerMessage::TimeState {
                tick,
                warp,
                warp_limited_by,
            } => {
                self.latest.tick = *tick;
                self.latest.warp = *warp;
                self.latest.warp_limited_by = *warp_limited_by;
            }
            ServerMessage::Snapshot { tick, bodies } => {
                self.latest.tick = *tick;
                self.latest.bodies = bodies.clone();
            }
            ServerMessage::Vessels { tick, vessels } => {
                self.latest.tick = *tick;
                self.latest.vessels = vessels.clone();
            }
            ServerMessage::Trajectory {
                tick,
                vessel,
                patches,
            } => {
                self.latest
                    .trajectories
                    .insert(*vessel, (*tick, patches.clone()));
            }
            ServerMessage::BurnState {
                vessel,
                scheduled,
                reason,
            } => {
                self.latest
                    .burn_states
                    .insert(*vessel, (*scheduled, reason.clone()));
            }
            _ => {}
        }
        Ok(msg)
    }

    /// Poll until `pred` returns `Some` or `timeout` (wall clock) passes.
    pub async fn wait_for<T>(
        &mut self,
        timeout: Duration,
        what: &str,
        mut pred: impl FnMut(&Latest, &ServerMessage) -> Option<T>,
    ) -> Result<T> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                bail!("timed out after {timeout:?} waiting for {what}");
            }
            let msg = self
                .poll(remaining)
                .await
                .with_context(|| format!("waiting for {what}"))?;
            if let Some(t) = pred(&self.latest, &msg) {
                return Ok(t);
            }
        }
    }

    /// Catalog index of the body with `id`.
    pub fn body_index(&self, id: &str) -> Option<u16> {
        self.catalog
            .iter()
            .position(|b| b.id == id)
            .map(|i| i as u16)
    }

    pub fn body(&self, index: u16) -> Option<&BodyInfo> {
        self.catalog.get(index as usize)
    }

    /// The first vessel the server streams as owned by this player, once `Vessels` has arrived.
    pub async fn my_vessel(&mut self, timeout: Duration) -> Result<VesselState> {
        let me = self.player_id;
        self.wait_for(timeout, "a vessel owned by this player", |l, _| {
            l.vessels.iter().find(|v| v.owner == Some(me)).cloned()
        })
        .await
    }

    /// The latest streamed state of vessel `id`, waiting for the next `Vessels` message.
    pub async fn vessel_state(&mut self, id: u64, timeout: Duration) -> Result<VesselState> {
        self.wait_for(timeout, "a Vessels message", |l, m| match m {
            ServerMessage::Vessels { .. } => l.vessel(id).cloned(),
            _ => None,
        })
        .await
    }

    pub async fn request_warp(&mut self, factor: u32) -> Result<()> {
        self.send(&ClientMessage::RequestWarp { factor }).await
    }

    /// Schedule a burn and return the server's answer plus the trajectory it re-sends right after.
    pub async fn schedule_burn(
        &mut self,
        vessel: u64,
        ut: f64,
        prograde_mps: f64,
        normal_mps: f64,
        radial_mps: f64,
    ) -> Result<Vec<TrajectoryPatch>> {
        self.send(&ClientMessage::ScheduleBurn {
            vessel,
            ut,
            prograde_mps,
            normal_mps,
            radial_mps,
        })
        .await?;
        let (scheduled, reason) = self
            .wait_for(Duration::from_secs(10), "BurnState", |_, m| match m {
                ServerMessage::BurnState {
                    vessel: v,
                    scheduled,
                    reason,
                } if *v == vessel => Some((*scheduled, reason.clone())),
                _ => None,
            })
            .await?;
        if !scheduled {
            bail!(
                "server refused the burn: {}",
                reason.unwrap_or_else(|| "no reason given".into())
            );
        }
        self.wait_for(
            Duration::from_secs(10),
            "Trajectory after BurnState",
            |_, m| match m {
                ServerMessage::Trajectory {
                    vessel: v, patches, ..
                } if *v == vessel => Some(patches.clone()),
                _ => None,
            },
        )
        .await
    }
}

/// What the server said to `JoinCampaign` / `CampaignInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct Campaign {
    pub name: String,
    pub started_tick: Tick,
    pub members: Vec<ose_net::CampaignMember>,
    pub you_are_member: bool,
    pub vessels: u32,
}

impl Client {
    /// Join the shared campaign (idempotent) and return its state.
    pub async fn join_campaign(&mut self) -> Result<Campaign> {
        self.send(&ClientMessage::JoinCampaign).await?;
        let c = self
            .wait_for(Duration::from_secs(10), "CampaignState", |_, m| match m {
                ServerMessage::CampaignState {
                    name,
                    started_tick,
                    members,
                    you_are_member,
                    vessels,
                } => Some(Campaign {
                    name: name.clone(),
                    started_tick: *started_tick,
                    members: members.clone(),
                    you_are_member: *you_are_member,
                    vessels: *vessels,
                }),
                _ => None,
            })
            .await?;
        if !c.you_are_member {
            bail!("server answered JoinCampaign but did not list this player as a member");
        }
        Ok(c)
    }
}

/// What the server said to `LaunchCraft`.
#[derive(Clone, Debug, PartialEq)]
pub struct Launched {
    pub vessel: u64,
    pub name: String,
    pub parts: u32,
    pub wet_mass_t: f64,
    pub dry_mass_t: f64,
    pub delta_v_vacuum_mps: f64,
}

impl Client {
    /// Upload a craft and have the server spawn it as this player's vessel in a circular orbit.
    pub async fn launch_craft(
        &mut self,
        craft: ose_data::craft::Craft,
        altitude_m: f64,
    ) -> Result<Launched> {
        self.send(&ClientMessage::LaunchCraft { craft, altitude_m })
            .await?;
        let answer = self
            .wait_for(Duration::from_secs(20), "CraftLaunched", |_, m| match m {
                ServerMessage::CraftLaunched {
                    vessel,
                    name,
                    parts,
                    wet_mass_t,
                    dry_mass_t,
                    delta_v_vacuum_mps,
                    reason,
                    ..
                } => Some((
                    *vessel,
                    name.clone(),
                    *parts,
                    *wet_mass_t,
                    *dry_mass_t,
                    *delta_v_vacuum_mps,
                    reason.clone(),
                )),
                _ => None,
            })
            .await?;
        let (vessel, name, parts, wet_mass_t, dry_mass_t, delta_v_vacuum_mps, reason) = answer;
        let Some(vessel) = vessel else {
            bail!(
                "server refused the craft {name:?}: {}",
                reason.unwrap_or_else(|| "no reason given".into())
            );
        };
        Ok(Launched {
            vessel,
            name,
            parts,
            wet_mass_t,
            dry_mass_t,
            delta_v_vacuum_mps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_body_parses() {
        assert_eq!(
            parse_health("ok tick=1234").unwrap(),
            Health { tick: Tick(1234) }
        );
        assert!(parse_health("nope").is_err());
    }

    #[test]
    fn wire_elements_round_trip_through_ose_math() {
        let o = OrbitElements {
            semi_major_axis_m: 700_000.0,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            longitude_of_ascending_node_deg: 0.0,
            argument_of_periapsis_deg: 0.0,
            mean_anomaly_at_epoch_rad: 0.0,
            epoch_s: 0.0,
        };
        let orbit = orbit_from(3.5316e12, &o);
        let (r, v) = orbit.state_at(0.0);
        assert!((r.length() - 700_000.0).abs() < 1e-6);
        assert!((v.length() - (3.5316e12 / 700_000.0f64).sqrt()).abs() < 1e-6);
    }
}

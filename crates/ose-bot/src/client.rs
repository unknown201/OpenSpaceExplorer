//! The bot's view of `ose-server`: the HTTP health probe and a WebSocket session speaking the `ose-net`
//! protocol (postcard-encoded binary frames).

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use ose_data::Tick;
use ose_net::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

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

/// A protocol session.
pub struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

/// The path the server is expected to upgrade WebSocket sessions on.
pub const WS_PATH: &str = "/ws";

impl Client {
    /// Open a WebSocket to `ws://{base}{WS_PATH}`.
    pub async fn connect(base: &str) -> Result<Client, ConnectError> {
        let url = format!("ws://{base}{WS_PATH}");
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .map_err(|_| ConnectError::Other(anyhow!("connect to {url} timed out")))?;
        match attempt {
            Ok((ws, _)) => Ok(Client { ws }),
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                Err(ConnectError::NoEndpoint {
                    status: resp.status().as_u16(),
                })
            }
            Err(e) => Err(ConnectError::Other(
                anyhow!(e).context(format!("connect to {url}")),
            )),
        }
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let bytes = ose_net::encode(msg)?;
        self.ws.send(Message::Binary(bytes.into())).await?;
        Ok(())
    }

    /// Next binary protocol frame, skipping pings and text; `Err` on close or timeout.
    pub async fn recv(&mut self, timeout: Duration) -> Result<ServerMessage> {
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

    /// Say hello and wait for the welcome.
    pub async fn hello(&mut self, display_name: &str) -> Result<Welcome> {
        self.send(&ClientMessage::Hello {
            protocol: PROTOCOL_VERSION,
            token: None,
            display_name: display_name.into(),
        })
        .await?;
        match self.recv(Duration::from_secs(10)).await? {
            ServerMessage::Welcome {
                player_id,
                token,
                tick,
            } => Ok(Welcome {
                player_id,
                token,
                tick,
            }),
            ServerMessage::Rejected { reason } => bail!("server rejected hello: {reason}"),
            other => bail!("expected Welcome, got {other:?}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Welcome {
    pub player_id: u64,
    pub token: String,
    pub tick: Tick,
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
}

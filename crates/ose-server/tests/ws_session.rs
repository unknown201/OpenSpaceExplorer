//! Spawns the server binary and runs one protocol session: Hello -> Welcome, BodyCatalog, then TimeState and
//! Snapshot frames with every body in the catalog, and RequestWarp changing the streamed warp.

use futures_util::{SinkExt, StreamExt};
use ose_net::{ClientMessage, ServerMessage, PROTOCOL_VERSION, WS_PATH};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

async fn spawn_server() -> (Server, String) {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_ose-server"))
        .args(["--listen", &addr])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ose-server");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "server did not start"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (Server(child), addr)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn recv(ws: &mut Ws) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
            .expect("stream")
        {
            Ok(Message::Binary(b)) => return ose_net::decode::<ServerMessage>(&b).expect("decode"),
            Ok(Message::Close(f)) => panic!("closed: {f:?}"),
            Ok(_) => continue,
            Err(e) => panic!("ws error: {e}"),
        }
    }
}

#[tokio::test]
async fn hello_catalog_and_snapshots() {
    let (_server, addr) = spawn_server().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{WS_PATH}"))
        .await
        .expect("connect");
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        token: None,
        display_name: "test".into(),
    };
    ws.send(Message::Binary(ose_net::encode(&hello).unwrap().into()))
        .await
        .unwrap();

    let ServerMessage::Welcome { player_id, .. } = recv(&mut ws).await else {
        panic!("expected Welcome")
    };
    assert_eq!(player_id, 1);
    let ServerMessage::BodyCatalog { bodies } = recv(&mut ws).await else {
        panic!("expected BodyCatalog")
    };
    assert!(bodies.len() >= 17, "catalog has {} bodies", bodies.len());
    let kerbin = bodies
        .iter()
        .position(|b| b.id == "Kerbin")
        .expect("Kerbin in catalog");
    let mun = bodies
        .iter()
        .position(|b| b.id == "Mun")
        .expect("Mun in catalog");
    assert_eq!(bodies[mun].parent, Some(kerbin as u16));
    assert!(bodies[kerbin].has_atmosphere);

    let mut saw_time = None;
    let mut saw_snap = None;
    for _ in 0..6 {
        match recv(&mut ws).await {
            ServerMessage::TimeState { tick, warp, .. } => saw_time = Some((tick, warp)),
            ServerMessage::Snapshot {
                tick,
                bodies: states,
            } => {
                assert_eq!(states.len(), bodies.len());
                let d = (states[mun].position_m - states[kerbin].position_m).length();
                assert!((d - 12_000_000.0).abs() < 1.0, "Mun distance {d}");
                saw_snap = Some(tick);
            }
            other => panic!("unexpected {other:?}"),
        }
        if saw_time.is_some() && saw_snap.is_some() {
            break;
        }
    }
    let (_, warp) = saw_time.expect("TimeState");
    assert_eq!(warp, 1);
    saw_snap.expect("Snapshot");

    // Raise warp and see it reflected.
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::RequestWarp { factor: 100 })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    let mut seen = false;
    for _ in 0..10 {
        if let ServerMessage::TimeState { warp, .. } = recv(&mut ws).await {
            if warp == 100 {
                seen = true;
                break;
            }
        }
    }
    assert!(seen, "warp 100 never streamed");
}

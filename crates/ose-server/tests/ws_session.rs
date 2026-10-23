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
    let mut saw_vessel = false;
    let mut saw_traj = false;
    for _ in 0..40 {
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
            ServerMessage::Vessels { vessels, .. } => {
                let probe = vessels
                    .iter()
                    .find(|v| v.owner == Some(player_id))
                    .expect("my probe");
                assert_eq!(probe.reference_body, kerbin as u16);
                let alt = (probe.orbit.semi_major_axis_m - bodies[kerbin].radius_m).abs();
                assert!((alt - 100_000.0).abs() < 1.0, "probe altitude {alt}");
                saw_vessel = true;
            }
            ServerMessage::Trajectory { patches, .. } => {
                assert_eq!(patches.len(), 1, "{patches:?}");
                assert_eq!(patches[0].end, ose_net::PatchEnd::Final);
                saw_traj = true;
            }
            other => panic!("unexpected {other:?}"),
        }
        if saw_time.is_some() && saw_snap.is_some() && saw_vessel && saw_traj {
            break;
        }
    }
    assert!(
        saw_vessel && saw_traj,
        "vessel {saw_vessel} trajectory {saw_traj}"
    );
    let (_, warp) = saw_time.expect("TimeState");
    assert_eq!(warp, 1);
    saw_snap.expect("Snapshot");

    // Schedule a prograde burn on the probe; the trajectory shows the maneuver and a 12,000 km apoapsis.
    let probe_id = 1;
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::ScheduleBurn {
            vessel: probe_id,
            ut: 1.0e6,
            prograde_mps: 841.6,
            normal_mps: 0.0,
            radial_mps: 0.0,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut burn_ok = false;
    let mut maneuver_seen = false;
    for _ in 0..60 {
        match recv(&mut ws).await {
            ServerMessage::BurnState {
                vessel,
                scheduled,
                reason,
            } => {
                assert_eq!(vessel, probe_id);
                assert!(scheduled, "{reason:?}");
                burn_ok = true;
            }
            ServerMessage::Trajectory { patches, .. } if patches.len() >= 2 => {
                assert_eq!(patches[0].end, ose_net::PatchEnd::Maneuver);
                let o = &patches[1].orbit;
                let ap = o.semi_major_axis_m * (1.0 + o.eccentricity);
                assert!((ap - 12_000_000.0).abs() < 100_000.0, "apoapsis {ap}");
                maneuver_seen = true;
            }
            _ => {}
        }
        if burn_ok && maneuver_seen {
            break;
        }
    }
    assert!(
        burn_ok && maneuver_seen,
        "burn {burn_ok} maneuver {maneuver_seen}"
    );

    // Raise warp and see it reflected.
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::RequestWarp { factor: 100 })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    let mut seen = false;
    for _ in 0..40 {
        if let ServerMessage::TimeState { warp, .. } = recv(&mut ws).await {
            if warp == 100 {
                seen = true;
                break;
            }
        }
    }
    assert!(seen, "warp 100 never streamed");

    // A second player who has not asked for warp holds it at 1 (lowest wins) and is named as the limiter;
    // when they leave, warp returns to 100.
    let (mut ws2, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{WS_PATH}"))
        .await
        .expect("connect 2");
    let hello2 = ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        token: None,
        display_name: "second".into(),
    };
    ws2.send(Message::Binary(ose_net::encode(&hello2).unwrap().into()))
        .await
        .unwrap();
    let ServerMessage::Welcome { player_id: p2, .. } = recv(&mut ws2).await else {
        panic!("expected Welcome 2")
    };
    let mut limited = false;
    for _ in 0..40 {
        if let ServerMessage::TimeState {
            warp,
            warp_limited_by,
            ..
        } = recv(&mut ws).await
        {
            if warp == 1 && warp_limited_by == Some(p2) {
                limited = true;
                break;
            }
        }
    }
    assert!(limited, "second player did not hold warp down");
    drop(ws2);
    let mut restored = false;
    for _ in 0..60 {
        if let ServerMessage::TimeState {
            warp,
            warp_limited_by,
            ..
        } = recv(&mut ws).await
        {
            if warp == 100 && warp_limited_by.is_none() {
                restored = true;
                break;
            }
        }
    }
    assert!(
        restored,
        "warp did not return to 100 after the second player left"
    );
}

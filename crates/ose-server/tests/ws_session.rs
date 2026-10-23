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
        let _ = self.0.wait();
    }
}

/// Wait until nothing accepts on `addr` any more (the killed server has released the port).
async fn wait_port_free(addr: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::net::TcpStream::connect(addr).await.is_ok() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "port {addr} never freed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn spawn_server() -> (Server, String) {
    spawn_server_with(&["--db", ":memory:", "--save", "none"], None).await
}

async fn spawn_server_with(extra: &[&str], port: Option<u16>) -> (Server, String) {
    let port = port.unwrap_or_else(|| {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    });
    let addr = format!("127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_ose-server"))
        .args(["--listen", &addr])
        .args(extra)
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

    let ServerMessage::Welcome {
        player_id, token, ..
    } = recv(&mut ws).await
    else {
        panic!("expected Welcome")
    };
    assert_eq!(player_id, 1);
    assert!(!token.is_empty());
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

    // Launch a tiny craft: a pod with charge and a Terrier; it appears as a second owned vessel.
    let craft = ose_data::craft::Craft {
        name: "Tiny".into(),
        description: String::new(),
        parts: vec![
            ose_data::craft::CraftPart {
                guid: "a".into(),
                part_id: "pod_1v_conical_crew".into(),
                parent: None,
                own_node: None,
                parent_node: None,
                local_position: ose_math::DVec3::ZERO,
                local_rotation: ose_data::craft::Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                stage_index: -1,
                fuel_crossfeed: true,
                resources: vec![],
            },
            ose_data::craft::CraftPart {
                guid: "b".into(),
                part_id: "engine_1v_methalox_terrier".into(),
                parent: Some(0),
                own_node: Some("top".into()),
                parent_node: Some("bottom".into()),
                local_position: ose_math::DVec3::ZERO,
                local_rotation: ose_data::craft::Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                stage_index: 0,
                fuel_crossfeed: true,
                resources: vec![],
            },
        ],
        stages: vec![vec![1]],
    };
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::LaunchCraft {
            craft,
            altitude_m: 120_000.0,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut launched = None;
    for _ in 0..60 {
        if let ServerMessage::CraftLaunched {
            vessel,
            reason,
            parts,
            dry_mass_t,
            ..
        } = recv(&mut ws).await
        {
            assert_eq!(parts, 2);
            assert!(dry_mass_t > 0.5, "dry {dry_mass_t}");
            assert!(reason.is_none(), "{reason:?}");
            launched = vessel;
            break;
        }
    }
    let launched = launched.expect("CraftLaunched");
    let mut seen_two = false;
    for _ in 0..40 {
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut ws).await {
            let mine: Vec<_> = vessels
                .iter()
                .filter(|v| v.owner == Some(player_id))
                .collect();
            if mine.len() == 2 {
                let tiny = mine
                    .iter()
                    .find(|v| v.id == launched)
                    .expect("launched vessel streamed");
                assert_eq!(tiny.name, "Tiny");
                let alt = tiny.orbit.semi_major_axis_m - bodies[kerbin].radius_m;
                assert!((alt - 120_000.0).abs() < 1.0, "alt {alt}");
                seen_two = true;
                break;
            }
        }
    }
    assert!(seen_two, "launched craft not streamed");

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

    // Resume: a new connection with the token is the same player and keeps the same single probe.
    drop(ws);
    let (mut ws3, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{WS_PATH}"))
        .await
        .expect("connect 3");
    let hello3 = ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        token: Some(token),
        display_name: "test again".into(),
    };
    ws3.send(Message::Binary(ose_net::encode(&hello3).unwrap().into()))
        .await
        .unwrap();
    let ServerMessage::Welcome { player_id: p3, .. } = recv(&mut ws3).await else {
        panic!("expected Welcome 3")
    };
    assert_eq!(p3, player_id, "token resumes the same player");
    let mut checked = false;
    for _ in 0..40 {
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut ws3).await {
            let mine: Vec<_> = vessels.iter().filter(|v| v.owner == Some(p3)).collect();
            // the probe and the launched craft, and no second probe on resume
            assert_eq!(
                mine.len(),
                2,
                "resumed player keeps probe + Tiny: {vessels:?}"
            );
            assert_eq!(mine.iter().filter(|v| v.name.ends_with("probe")).count(), 1);
            checked = true;
            break;
        }
    }
    assert!(checked);
}

async fn hello(ws: &mut Ws, token: Option<String>, name: &str) -> (u64, String) {
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        token,
        display_name: name.into(),
    };
    ws.send(Message::Binary(ose_net::encode(&hello).unwrap().into()))
        .await
        .unwrap();
    match recv(ws).await {
        ServerMessage::Welcome {
            player_id, token, ..
        } => (player_id, token),
        other => panic!("expected Welcome, got {other:?}"),
    }
}

#[tokio::test]
async fn restart_keeps_time_vessels_and_burns() {
    let dir = std::env::temp_dir().join(format!("ose-save-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("accounts.json");
    let save = dir.join("save.json");
    let args = [
        "--db",
        db.to_str().unwrap(),
        "--save",
        save.to_str().unwrap(),
        "--save-interval-s",
        "0.3",
    ];
    let (server, addr) = spawn_server_with(&args, None).await;
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{WS_PATH}"))
        .await
        .unwrap();
    let (pid, token) = hello(&mut ws, None, "saver").await;
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::ScheduleBurn {
            vessel: 1,
            ut: 5.0e5,
            prograde_mps: 100.0,
            normal_mps: 0.0,
            radial_mps: 0.0,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::RequestWarp { factor: 1000 })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    // let a couple of saves happen at warp 1000
    tokio::time::sleep(Duration::from_millis(900)).await;
    let before = save::read_for_test(&save);
    assert!(before.tick > 0, "save has advanced time: {}", before.tick);
    assert_eq!(before.vessels.len(), 1);
    assert!(before.vessels[0].burn_ut.is_some());
    drop(ws);
    drop(server);
    wait_port_free(&addr).await;

    let (_server2, addr2) = spawn_server_with(&args, Some(port)).await;
    let (mut ws2, _) = tokio_tungstenite::connect_async(format!("ws://{addr2}{WS_PATH}"))
        .await
        .unwrap();
    let (pid2, _) = hello(&mut ws2, Some(token), "saver").await;
    assert_eq!(pid2, pid, "account survives restart");
    let mut ok = false;
    for _ in 0..40 {
        match recv(&mut ws2).await {
            ServerMessage::TimeState { tick, .. } => {
                assert!(
                    tick.0 >= before.tick,
                    "time went backwards: {} < {}",
                    tick.0,
                    before.tick
                );
            }
            ServerMessage::Vessels { vessels, .. } => {
                let mine: Vec<_> = vessels.iter().filter(|v| v.owner == Some(pid2)).collect();
                assert_eq!(mine.len(), 1, "one restored probe, no new one: {vessels:?}");
                assert_eq!(mine[0].id, 1);
            }
            ServerMessage::Trajectory { patches, .. } => {
                assert!(
                    patches.iter().any(|p| p.end == ose_net::PatchEnd::Maneuver),
                    "burn restored: {patches:?}"
                );
                ok = true;
                break;
            }
            _ => {}
        }
    }
    assert!(ok, "no trajectory with the restored burn");
    let _ = std::fs::remove_dir_all(&dir);
}

mod save {
    //! Minimal reader for the save file so the test does not depend on server internals.
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct Burn {
        ut: f64,
    }
    #[derive(Deserialize)]
    struct Vessel {
        burn: Option<Burn>,
    }
    #[derive(Deserialize)]
    struct File {
        tick: u64,
        vessels: Vec<Vessel>,
    }
    pub struct Seen {
        pub tick: u64,
        pub vessels: Vec<SeenVessel>,
    }
    pub struct SeenVessel {
        pub burn_ut: Option<f64>,
    }
    pub fn read_for_test(p: &std::path::Path) -> Seen {
        let f: File =
            serde_json::from_str(&std::fs::read_to_string(p).expect("save file")).expect("parse");
        Seen {
            tick: f.tick,
            vessels: f
                .vessels
                .into_iter()
                .map(|v| SeenVessel {
                    burn_ut: v.burn.map(|b| b.ut),
                })
                .collect(),
        }
    }
}

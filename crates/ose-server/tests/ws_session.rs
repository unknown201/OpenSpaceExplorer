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
        if health_ok(&addr).await {
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

/// True when `GET /health` answers 200 (the listener is bound and the router is serving).
async fn health_ok(addr: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(Ok(mut s)) = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    else {
        return false;
    };
    if s.write_all(
        format!(
            "GET /health HTTP/1.0
Host: {addr}

"
        )
        .as_bytes(),
    )
    .await
    .is_err()
    {
        return false;
    }
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_millis(500), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).contains(" 200 ")
}

/// Open the protocol WebSocket, retrying briefly: right after a (re)start the first connect can still be
/// refused on some hosts even though `/health` already answered.
async fn connect_ws(addr: &str) -> Ws {
    let url = format!("ws://{addr}{WS_PATH}");
    let mut last = None;
    for _ in 0..50 {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => return ws,
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    panic!("could not connect to {url}: {last:?}");
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
    let mut ws = connect_ws(&addr).await;
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
    let ServerMessage::BodyRotation { bodies: spins } = recv(&mut ws).await else {
        panic!("expected BodyRotation")
    };
    assert_eq!(spins.len(), bodies.len());
    let k = bodies.iter().position(|b| b.id == "Kerbin").unwrap();
    assert!(
        (spins[k].rotation_period_s - 21_549.425).abs() < 1.0,
        "{:?}",
        spins[k]
    );
    let ServerMessage::DifficultyOptions {
        definitions,
        presets,
    } = recv(&mut ws).await
    else {
        panic!("expected DifficultyOptions")
    };
    assert!(definitions.iter().any(|d| d.id == "InfiniteFuel"));
    assert!(presets.iter().any(|p| p.name == "Normal"));
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

    // Session browser: one shared session; creating configures it before anyone joined; agency is echoed.
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::ListSessions)
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    let mut listed = false;
    for _ in 0..40 {
        if let ServerMessage::SessionList { sessions } = recv(&mut ws).await {
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, "shared");
            assert_eq!(sessions[0].players, 0);
            listed = true;
            break;
        }
    }
    assert!(listed, "no SessionList");
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::CreateSession {
            name: "Mun or bust".into(),
            mode: ose_net::SessionMode::Sandbox,
            difficulty: vec![ose_net::DifficultyOption {
                id: "InfiniteFuel".into(),
                value: ose_net::DifficultyValue::Bool(false),
            }],
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut created_ok = false;
    for _ in 0..40 {
        if let ServerMessage::CampaignState {
            name,
            created,
            mode,
            difficulty,
            you_are_member,
            ..
        } = recv(&mut ws).await
        {
            assert!(created);
            assert_eq!(name, "Mun or bust");
            // sandbox: this flight test launches stock craft without researching their parts (the
            // exploration tech tree has its own test)
            assert_eq!(mode, ose_net::SessionMode::Sandbox);
            assert_eq!(difficulty.len(), 1);
            assert!(you_are_member);
            created_ok = true;
            break;
        }
    }
    assert!(created_ok, "no CampaignState after CreateSession");

    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::SetAgency {
            agency: ose_net::Agency {
                flag_id: "flag_AGY_orbit".into(),
                primary_rgb: [10, 20, 30],
                secondary_rgb: [200, 210, 220],
            },
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut agency_ok = false;
    for _ in 0..40 {
        if let ServerMessage::CampaignState { members, .. } = recv(&mut ws).await {
            let me = members.iter().find(|m| m.player_id == player_id).unwrap();
            assert_eq!(
                me.agency.as_ref().map(|a| a.flag_id.as_str()),
                Some("flag_AGY_orbit")
            );
            agency_ok = true;
            break;
        }
    }
    assert!(agency_ok, "agency not echoed");

    // Join the shared campaign; the state lists us as a member and is idempotent.
    for _ in 0..2 {
        ws.send(Message::Binary(
            ose_net::encode(&ClientMessage::JoinCampaign)
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
        let mut seen = false;
        for _ in 0..40 {
            if let ServerMessage::CampaignState {
                members,
                you_are_member,
                name,
                ..
            } = recv(&mut ws).await
            {
                assert!(you_are_member);
                assert_eq!(members.len(), 1, "{members:?}");
                assert_eq!(members[0].player_id, player_id);
                assert_eq!(members[0].display_name, "test");
                assert!(!name.is_empty());
                seen = true;
                break;
            }
        }
        assert!(seen, "no CampaignState");
    }

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
                fuel_line_target: None,
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
                fuel_line_target: None,
            },
        ],
        stages: vec![vec![1]],
    };
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::LaunchCraft {
            craft: craft.clone(),
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

    // A craft launched straight into orbit keeps a flight state: throttle up wakes it under power.
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::SetControls {
            vessel: launched,
            throttle: 0.5,
            attitude: ose_net::AttitudeMode::OrbitPrograde,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut woke = false;
    for _ in 0..60 {
        match recv(&mut ws).await {
            ServerMessage::FlightAck {
                vessel,
                accepted,
                reason,
            } if vessel == launched => {
                assert!(accepted, "{reason:?}");
            }
            ServerMessage::Vessels { vessels, .. } => {
                if let Some(f) = vessels
                    .iter()
                    .find(|v| v.id == launched)
                    .and_then(|v| v.flight.as_ref())
                {
                    assert_eq!(f.situation, ose_net::FlightSituation::Orbit);
                    assert_eq!(f.throttle, 0.5);
                    woke = true;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(woke, "orbiting craft was not woken by SetControls");
    // the bare probe has no flight state and is refused
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::SetControls {
            vessel: 1,
            throttle: 1.0,
            attitude: ose_net::AttitudeMode::OrbitPrograde,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut refused = false;
    for _ in 0..60 {
        if let ServerMessage::FlightAck {
            vessel: 1,
            accepted,
            ..
        } = recv(&mut ws).await
        {
            assert!(!accepted);
            refused = true;
            break;
        }
    }
    assert!(refused, "probe controls were not refused");

    // Craft library: stock crafts are listed; saving adds ours; a saved id launches.
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::CraftList).unwrap().into(),
    ))
    .await
    .unwrap();
    let mut stock_count = 0;
    for _ in 0..40 {
        if let ServerMessage::CraftList { crafts } = recv(&mut ws).await {
            stock_count = crafts.iter().filter(|c| c.stock).count();
            assert!(
                crafts
                    .iter()
                    .any(|c| c.name.contains("Kerbal") && c.parts >= 40),
                "{crafts:?}"
            );
            break;
        }
    }
    assert!(stock_count >= 5, "stock crafts {stock_count}");
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::SaveCraft {
            craft: craft.clone(),
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut saved_id = None;
    for _ in 0..40 {
        if let ServerMessage::CraftSaved { id, reason, .. } = recv(&mut ws).await {
            assert!(reason.is_none(), "{reason:?}");
            saved_id = id;
            break;
        }
    }
    let saved_id = saved_id.expect("CraftSaved");
    assert!(saved_id >= 1000);
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::LaunchSavedCraft {
            id: saved_id,
            altitude_m: 150_000.0,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let mut launched2 = false;
    for _ in 0..40 {
        if let ServerMessage::CraftLaunched { vessel, reason, .. } = recv(&mut ws).await {
            assert!(reason.is_none(), "{reason:?}");
            assert!(vessel.is_some());
            launched2 = true;
            break;
        }
    }
    assert!(launched2, "saved craft did not launch");
    let mut seen_two = false;
    for _ in 0..40 {
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut ws).await {
            let mine: Vec<_> = vessels
                .iter()
                .filter(|v| v.owner == Some(player_id))
                .collect();
            if mine.len() >= 2 {
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

    // Launch pad: a stock craft on the pad is landed; full throttle straight up lifts it off.
    let k2 = {
        ws.send(Message::Binary(
            ose_net::encode(&ClientMessage::CraftList).unwrap().into(),
        ))
        .await
        .unwrap();
        let mut id = None;
        for _ in 0..40 {
            if let ServerMessage::CraftList { crafts } = recv(&mut ws).await {
                id = crafts
                    .iter()
                    .find(|c| c.name.contains("Kerbal") && c.stock)
                    .map(|c| c.id);
                break;
            }
        }
        id.expect("a stock Kerbal craft")
    };
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::LaunchSavedOnPad { id: k2 })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    let mut pad_vessel = None;
    for _ in 0..40 {
        if let ServerMessage::CraftLaunched { vessel, reason, .. } = recv(&mut ws).await {
            assert!(reason.is_none(), "{reason:?}");
            pad_vessel = vessel;
            break;
        }
    }
    let pad_vessel = pad_vessel.expect("pad vessel");
    let mut landed_seen = false;
    for _ in 0..40 {
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut ws).await {
            let v = vessels
                .iter()
                .find(|v| v.id == pad_vessel)
                .expect("pad vessel streamed");
            let f = v.flight.as_ref().expect("flight readout");
            assert_eq!(f.situation, ose_net::FlightSituation::Landed);
            // KSP2's launch pad 1 stands 115.2 m above sea level (Kerbin LocalSimObjectsData); the altitude
            // is the craft's centre of mass, which stands above the pad on its lowest part
            assert!(
                f.altitude_m > 115.2 && f.altitude_m < 135.0,
                "pad alt {}",
                f.altitude_m
            );
            assert!(f.twr > 1.0, "twr {}", f.twr);
            // standing on the pad: the axis points up, the roll reference lies across it
            assert!((f.facing.length() - 1.0).abs() < 1e-6, "{:?}", f.facing);
            assert!((f.roll_ref.length() - 1.0).abs() < 1e-6);
            assert!(f.facing.dot(f.roll_ref).abs() < 1e-6);
            assert!(f.has_control);
            assert!(f.joint_stress < 1.0);
            landed_seen = true;
            break;
        }
    }
    assert!(landed_seen);
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::SetControls {
            vessel: pad_vessel,
            throttle: 1.0,
            attitude: ose_net::AttitudeMode::Up,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    // the first stage press lights the engines (the throttle alone does not, on the pad)
    send_msg(&mut ws, &ClientMessage::Stage { vessel: pad_vessel }).await;
    let mut flying = false;
    for _ in 0..200 {
        match recv(&mut ws).await {
            ServerMessage::FlightAck {
                accepted, reason, ..
            } => assert!(accepted, "{reason:?}"),
            ServerMessage::Vessels { vessels, .. } => {
                let v = vessels.iter().find(|v| v.id == pad_vessel).unwrap();
                let f = v.flight.as_ref().unwrap();
                if f.situation == ose_net::FlightSituation::Flying && f.altitude_m > 140.0 {
                    assert!(f.vertical_speed_mps > 0.0);
                    assert!(f.throttle == 1.0);
                    flying = true;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(flying, "the craft did not lift off");
    ws.send(Message::Binary(
        ose_net::encode(&ClientMessage::Stage { vessel: pad_vessel })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    let mut staged = false;
    let mut debris_seen = false;
    for _ in 0..80 {
        match recv(&mut ws).await {
            ServerMessage::FlightAck {
                vessel,
                accepted,
                reason,
            } if vessel == pad_vessel => {
                assert!(accepted, "{reason:?}");
                staged = true;
            }
            ServerMessage::Vessels { vessels, .. }
                if staged
                    && vessels
                        .iter()
                        .any(|v| v.name.ends_with("debris") && v.owner == Some(player_id)) =>
            {
                debris_seen = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        staged && debris_seen,
        "staged {staged} debris {debris_seen}"
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
            // a vessel is under powered flight, so warp is capped at physics warp
            if warp == 4 {
                seen = true;
                break;
            }
        }
    }
    assert!(seen, "physics-warp cap never streamed");

    // A second player who has not asked for warp holds it at 1 (lowest wins) and is named as the limiter;
    // when they leave, warp returns to 100.
    let mut ws2 = connect_ws(&addr).await;
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
            if warp == 4 && warp_limited_by.is_none() {
                restored = true;
                break;
            }
        }
    }
    assert!(
        restored,
        "warp did not return to the physics cap after the second player left"
    );

    // Resume: a new connection with the token is the same player and keeps the same single probe.
    drop(ws);
    let mut ws3 = connect_ws(&addr).await;
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
            let mine: Vec<_> = vessels
                .iter()
                .filter(|v| v.owner == Some(p3) && !v.name.ends_with("debris"))
                .collect();
            // the probe, two Tiny launches and the pad craft (debris comes and goes), no second probe
            assert_eq!(
                mine.len(),
                4,
                "resumed player keeps probe, two Tiny launches and the pad craft: {vessels:?}"
            );
            assert_eq!(mine.iter().filter(|v| v.name.ends_with("probe")).count(), 1);
            checked = true;
            break;
        }
    }
    assert!(checked);
}

async fn send_msg(ws: &mut Ws, m: &ClientMessage) {
    ws.send(Message::Binary(ose_net::encode(m).unwrap().into()))
        .await
        .unwrap();
}

/// Wait for the first message `pick` accepts (other traffic, like snapshots, is skipped).
async fn wait_for<T>(ws: &mut Ws, mut pick: impl FnMut(ServerMessage) -> Option<T>) -> T {
    for _ in 0..400 {
        if let Some(t) = pick(recv(ws).await) {
            return t;
        }
    }
    panic!("expected message never came");
}

fn starter_craft() -> ose_data::craft::Craft {
    // only parts of the free starting tech node: pod, tank, Swivel
    let part = |guid: &str,
                id: &str,
                parent: Option<usize>,
                own: Option<&str>,
                other: Option<&str>,
                y: f64| {
        ose_data::craft::CraftPart {
            guid: guid.into(),
            part_id: id.into(),
            parent,
            own_node: own.map(String::from),
            parent_node: other.map(String::from),
            local_position: ose_math::DVec3::new(0.0, y, 0.0),
            local_rotation: ose_data::craft::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            stage_index: 0,
            fuel_crossfeed: true,
            resources: vec![],
            fuel_line_target: None,
        }
    };
    ose_data::craft::Craft {
        name: "Starter".into(),
        description: String::new(),
        parts: vec![
            part("a", "pod_1v_conical_crew", None, None, None, 0.0),
            part(
                "b",
                "fueltank_1v_inline_methalox_1x2",
                Some(0),
                Some("top"),
                Some("bottom"),
                -1.0,
            ),
            part(
                "c",
                "engine_1v_methalox_swivel",
                Some(1),
                Some("top"),
                Some("bottom"),
                -2.2,
            ),
        ],
        stages: vec![vec![2]],
    }
}

#[tokio::test]
async fn exploration_science_tech_tree_and_recovery() {
    let (_server, addr) = spawn_server().await;
    let mut ws = connect_ws(&addr).await;
    let (player_id, _) = hello(&mut ws, None, "scientist").await;
    send_msg(&mut ws, &ClientMessage::JoinCampaign).await;
    wait_for(&mut ws, |m| {
        matches!(m, ServerMessage::CampaignState { .. }).then_some(())
    })
    .await;

    // a fresh exploration campaign: the tech tree is on, nothing earned, the first mission offered
    send_msg(&mut ws, &ClientMessage::ScienceInfo).await;
    let (tech_tree, capacity, unlocked, missions) = wait_for(&mut ws, |m| match m {
        ServerMessage::ScienceState {
            tech_tree,
            capacity,
            unlocked,
            missions,
            ..
        } => Some((tech_tree, capacity, unlocked, missions)),
        _ => None,
    })
    .await;
    assert!(tech_tree);
    assert_eq!(capacity, 0);
    assert!(unlocked.contains(&"tNode_1v_start".to_string()));
    assert!(
        missions
            .iter()
            .any(|m| m.id == "Main_Kerbin_01" && !m.complete),
        "{missions:?}"
    );

    // unresearched parts stay in the VAB
    let mut locked = starter_craft();
    locked.parts[2].part_id = "engine_1v_methalox_terrier".into();
    send_msg(&mut ws, &ClientMessage::LaunchOnPad { craft: locked }).await;
    let reason = wait_for(&mut ws, |m| match m {
        ServerMessage::CraftLaunched { reason, .. } => Some(reason),
        _ => None,
    })
    .await;
    assert!(reason.unwrap_or_default().contains("not researched"));

    // a starter craft on the pad: the crew observes the KSC
    send_msg(
        &mut ws,
        &ClientMessage::LaunchOnPad {
            craft: starter_craft(),
        },
    )
    .await;
    let vessel = wait_for(&mut ws, |m| match m {
        ServerMessage::CraftLaunched { vessel, reason, .. } => {
            assert!(reason.is_none(), "{reason:?}");
            vessel
        }
        _ => None,
    })
    .await;
    send_msg(&mut ws, &ClientMessage::RunExperiments { vessel }).await;
    let (new, carried) = wait_for(&mut ws, |m| match m {
        ServerMessage::ScienceReports {
            new,
            carried,
            reason,
            ..
        } => {
            assert!(reason.is_none(), "{reason:?}");
            Some((new, carried))
        }
        _ => None,
    })
    .await;
    let obs = new
        .iter()
        .find(|r| r.experiment == "CrewObservation")
        .expect("crew observation");
    assert_eq!(obs.location, "Kerbin_Landed_KerbinKSC");
    assert_eq!(carried.len(), new.len());

    // transmitting scores it; the state follows
    send_msg(&mut ws, &ClientMessage::TransmitScience { vessel }).await;
    let earned = wait_for(&mut ws, |m| match m {
        ServerMessage::ScienceSubmitted { earned, reason, .. } => {
            assert!(reason.is_none(), "{reason:?}");
            Some(earned)
        }
        _ => None,
    })
    .await;
    assert!(earned > 0.0);
    let capacity = wait_for(&mut ws, |m| match m {
        ServerMessage::ScienceState { capacity, .. } => Some(capacity),
        _ => None,
    })
    .await;
    assert_eq!(capacity as f64, earned);

    // research: a node we cannot afford is refused with the reason
    send_msg(
        &mut ws,
        &ClientMessage::UnlockTechNode {
            id: "tNode_1v_rockets_03".into(),
        },
    )
    .await;
    let (accepted, reason) = wait_for(&mut ws, |m| match m {
        ServerMessage::TechUnlock {
            accepted, reason, ..
        } => Some((accepted, reason)),
        _ => None,
    })
    .await;
    assert!(!accepted && reason.is_some());

    // the same report again is worth nothing
    send_msg(&mut ws, &ClientMessage::RunExperiments { vessel }).await;
    wait_for(&mut ws, |m| {
        matches!(m, ServerMessage::ScienceReports { .. }).then_some(())
    })
    .await;
    send_msg(&mut ws, &ClientMessage::RecoverVessel { vessel }).await;
    let (earned, recovered) = wait_for(&mut ws, |m| match m {
        ServerMessage::ScienceSubmitted {
            earned,
            recovered,
            reason,
            ..
        } => {
            assert!(reason.is_none(), "{reason:?}");
            Some((earned, recovered))
        }
        _ => None,
    })
    .await;
    assert!(recovered && earned == 0.0, "{earned}");
    // the recovered vessel is gone
    for _ in 0..40 {
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut ws).await {
            assert!(vessels.iter().all(|v| v.id != vessel));
            break;
        }
    }
    let _ = player_id;
    // the tech tree and missions are served for the R&D and Mission Control screens
    let tree = http_get_json(&addr, "/tech-tree.json").await;
    assert!(tree.as_array().is_some_and(|a| a.len() > 50));
    let missions = http_get_json(&addr, "/missions.json").await;
    assert!(missions.as_array().is_some_and(|a| a.len() > 40));
}

#[tokio::test]
async fn a_pad_launch_lights_while_a_teammate_is_connected() {
    let (_server, addr) = spawn_server().await;
    // a teammate joins the campaign and sits there
    let mut mate = connect_ws(&addr).await;
    hello(&mut mate, None, "teammate").await;
    send_msg(&mut mate, &ClientMessage::JoinCampaign).await;
    wait_for(&mut mate, |m| {
        matches!(m, ServerMessage::CampaignState { .. }).then_some(())
    })
    .await;
    let mut ws = connect_ws(&addr).await;
    hello(&mut ws, None, "pilot").await;
    send_msg(&mut ws, &ClientMessage::JoinCampaign).await;
    wait_for(&mut ws, |m| {
        matches!(m, ServerMessage::CampaignState { .. }).then_some(())
    })
    .await;
    send_msg(
        &mut ws,
        &ClientMessage::LaunchOnPad {
            craft: starter_craft(),
        },
    )
    .await;
    let vessel = wait_for(&mut ws, |m| match m {
        ServerMessage::CraftLaunched { vessel, reason, .. } => {
            assert!(reason.is_none(), "{reason:?}");
            vessel
        }
        _ => None,
    })
    .await;
    // throttle up, let the server run a moment, then the first stage press
    send_msg(
        &mut ws,
        &ClientMessage::SetControls {
            vessel,
            throttle: 1.0,
            attitude: ose_net::AttitudeMode::Up,
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_msg(&mut ws, &ClientMessage::Stage { vessel }).await;
    let mut lifted = false;
    for _ in 0..300 {
        // keep the teammate's socket drained
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(1), mate.next()).await {}
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut ws).await {
            let f = vessels
                .iter()
                .find(|v| v.id == vessel)
                .and_then(|v| v.flight.clone())
                .expect("flight readout");
            assert_eq!(f.stage, 0, "the press must not drop the engine stage");
            eprintln!(
                "TT {:?} alt {:.1} thr {} thrust {:.1} prop {:.2} ctl {}",
                f.situation, f.altitude_m, f.throttle, f.thrust_kn, f.propellant_t, f.has_control
            );
            if f.situation == ose_net::FlightSituation::Flying && f.altitude_m > 130.0 {
                assert!(f.thrust_kn > 0.0);
                lifted = true;
                break;
            }
        }
    }
    assert!(lifted, "the starter rocket did not lift off");
}

async fn http_get_json(addr: &str, path: &str) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").expect("http response");
    assert!(
        head.starts_with("HTTP/1.0 200") || head.starts_with("HTTP/1.1 200"),
        "{head}"
    );
    assert!(
        head.to_ascii_lowercase().contains("application/json"),
        "{head}"
    );
    serde_json::from_str(body).expect("json body")
}

async fn http_get_raw(addr: &str, path: &str) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    match text.split_once("\r\n\r\n") {
        Some((h, b)) => (h.to_string(), b.to_string()),
        None => (text, String::new()),
    }
}

#[tokio::test]
async fn assets_dir_is_served_with_long_cache() {
    let dir = std::env::temp_dir().join(format!("ose-assets-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("parts")).unwrap();
    std::fs::write(dir.join("parts/pod.glb"), b"glTF-binary-bytes").unwrap();
    let dir_s = dir.to_str().unwrap().to_string();
    let args = ["--db", ":memory:", "--save", "none", "--assets-dir", &dir_s];
    let (_server, addr) = spawn_server_with(&args, None).await;
    let (head, body) = http_get_raw(&addr, "/assets/parts/pod.glb").await;
    assert!(head.contains(" 200 "), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("max-age=86400"),
        "{head}"
    );
    assert_eq!(body, "glTF-binary-bytes");
    let (head, _) = http_get_raw(&addr, "/assets/parts/missing.glb").await;
    assert!(head.contains(" 404 "), "{head}");
    let (head, _) = http_get_raw(&addr, "/assets/").await;
    assert!(
        !head.contains(" 200 ") || head.contains(" 404 "),
        "no index fallback: {head}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn parts_catalog_route_serves_geometry() {
    let (_server, addr) = spawn_server().await;
    let parts = http_get_json(&addr, "/parts.json").await;
    let arr = parts.as_array().expect("array");
    assert!(arr.len() >= 400, "parts {}", arr.len());
    let terrier = arr
        .iter()
        .find(|p| p["id"] == "engine_1v_methalox_terrier")
        .expect("terrier");
    assert_eq!(terrier["has_engine"], true);
    assert_eq!(terrier["is_decoupler"], false);
    assert!(terrier["size_m"]["x"].as_f64().unwrap() > 0.5, "{terrier}");
    assert!(terrier["attach_nodes"].as_array().unwrap().len() >= 2);
    assert_eq!(terrier["engines"][0]["max_thrust_kn"], 60.0);
    assert_eq!(terrier["engines"][0]["isp_vacuum_s"], 335.0);
    assert_eq!(terrier["attach_rules"]["stack"], true);
    let tank = arr
        .iter()
        .find(|p| {
            !p["resource_containers"].as_array().unwrap().is_empty() && p["category"] == "FuelTank"
        })
        .expect("a fuel tank with containers");
    assert!(
        tank["resource_containers"][0]["capacity_units"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(arr.iter().any(|p| p["is_decoupler"] == true));
    let res = http_get_json(&addr, "/resources.json").await;
    assert!(res
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == "Methalox"));
}

#[tokio::test]
async fn stale_guests_are_swept_but_members_stay() {
    let args = [
        "--db",
        ":memory:",
        "--save",
        "none",
        "--guest-ttl-s",
        "1",
        "--guest-sweep-s",
        "0.3",
    ];
    let (_server, addr) = spawn_server_with(&args, None).await;
    let mut g = connect_ws(&addr).await;
    let (guest_id, _) = hello(&mut g, None, "e2e pilot").await;
    drop(g);
    let mut m = connect_ws(&addr).await;
    let (member_id, _) = hello(&mut m, None, "josh").await;
    m.send(Message::Binary(
        ose_net::encode(&ClientMessage::JoinCampaign)
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let mut checked = false;
    for _ in 0..60 {
        if let ServerMessage::Vessels { vessels, .. } = recv(&mut m).await {
            if vessels.iter().all(|v| v.owner != Some(guest_id)) {
                assert!(vessels.iter().any(|v| v.owner == Some(member_id)));
                checked = true;
                break;
            }
        }
    }
    assert!(checked, "guest probe was not swept");
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
    let mut ws = connect_ws(&addr).await;
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
    let mut ws2 = connect_ws(&addr2).await;
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

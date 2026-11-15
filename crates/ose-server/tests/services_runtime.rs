//! Isolated live WebSocket evidence; assertions validate capture integrity, not KSP2 parity.
use futures_util::{SinkExt, StreamExt};
use ose_net::{ClientMessage as C, ServerMessage as S};
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Capture {
    file: std::fs::File,
    start: Instant,
}
impl Capture {
    fn event(&mut self, kind: &str, value: serde_json::Value) {
        writeln!(
            self.file,
            "{}",
            json!({"real_s":self.start.elapsed().as_secs_f64(),"kind":kind,"value":value})
        )
        .unwrap();
        self.file.flush().unwrap();
    }
    async fn send(&mut self, ws: &mut Ws, m: C) {
        self.event("send", serde_json::to_value(&m).unwrap());
        ws.send(Message::Binary(ose_net::encode(&m).unwrap().into()))
            .await
            .unwrap();
    }
    async fn recv(&mut self, ws: &mut Ws) -> S {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(20), ws.next())
                .await
                .expect("message timeout")
                .expect("closed")
                .unwrap();
            if let Message::Binary(b) = m {
                let m = ose_net::decode::<S>(&b).unwrap();
                // Resume tokens are deliberately excluded from the evidence file.
                if !matches!(
                    m,
                    S::Welcome { .. } | S::BodyCatalog { .. } | S::BodyRotation { .. }
                ) {
                    self.event("recv", serde_json::to_value(&m).unwrap());
                }
                return m;
            }
        }
    }
    async fn wait<T>(&mut self, ws: &mut Ws, mut f: impl FnMut(S) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "condition timeout");
            if let Some(x) = f(self.recv(ws).await) {
                return x;
            }
        }
    }
    async fn flight(&mut self, ws: &mut Ws, id: u64) -> ose_net::FlightReadout {
        self.wait(ws, |m| {
            if let S::Vessels { vessels, .. } = m {
                vessels
                    .into_iter()
                    .find(|v| v.id == id)
                    .and_then(|v| v.flight)
            } else {
                None
            }
        })
        .await
    }
}

#[tokio::test]
async fn capture_services_over_live_websocket() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let out = root.join(".cache/services-parity");
    std::fs::create_dir_all(&out).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let server_log = std::fs::File::create(out.join("ose-server.log")).unwrap();
    let _server = Process(
        Command::new(env!("CARGO_BIN_EXE_ose-server"))
            .current_dir(&root)
            .args(["--listen", &addr, "--db", ":memory:", "--save", "none"])
            .stdout(Stdio::null())
            .stderr(Stdio::from(server_log))
            .spawn()
            .unwrap(),
    );
    let mut cap = Capture {
        file: std::fs::File::create(out.join("ose-ws.jsonl")).unwrap(),
        start: Instant::now(),
    };
    let mut ws = loop {
        match tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await {
            Ok((s, _)) => break s,
            Err(e) => {
                assert!(cap.start.elapsed() < Duration::from_secs(25), "{e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    cap.send(
        &mut ws,
        C::Hello {
            protocol: ose_net::PROTOCOL_VERSION,
            token: None,
            display_name: "Services audit".into(),
        },
    )
    .await;
    let token = cap
        .wait(&mut ws, |m| {
            if let S::Welcome { token, .. } = m {
                Some(token)
            } else {
                None
            }
        })
        .await;
    let base: ose_data::craft::Craft = serde_json::from_slice(
        &std::fs::read(out.join("ose-craft.json")).expect("Run services_probe first"),
    )
    .unwrap();
    let mut results = vec![];
    for fraction in [1., 0.] {
        cap.send(
            &mut ws,
            C::CreateSession {
                name: format!("Services audit charge {fraction}"),
                mode: ose_net::SessionMode::Sandbox,
                difficulty: vec![],
            },
        )
        .await;
        cap.wait(&mut ws, |m| {
            matches!(m, S::CampaignState { .. }).then_some(())
        })
        .await;
        let mut craft = base.clone();
        craft.parts.retain(|p| !p.part_id.starts_with("solar"));
        craft.parts[2].parent = Some(1);
        let mut sampler = craft.parts[2].clone();
        sampler.guid = "services-sampler".into();
        sampler.part_id = "collector_0v_radial_science_sampler".into();
        sampler.local_position = ose_math::DVec3::new(-1., -1., 0.);
        sampler.resources.clear();
        craft.parts.push(sampler);
        for p in &mut craft.parts {
            for r in &mut p.resources {
                if r.resource == "ElectricCharge" {
                    r.stored_units = r.capacity_units * fraction;
                }
            }
        }
        cap.send(&mut ws, C::LaunchOnPad { craft }).await;
        let vessel = cap
            .wait(&mut ws, |m| {
                if let S::CraftLaunched { vessel, reason, .. } = m {
                    assert!(reason.is_none(), "{reason:?}");
                    vessel
                } else {
                    None
                }
            })
            .await;
        cap.send(
            &mut ws,
            C::DeployAntennas {
                vessel,
                deploy: true,
            },
        )
        .await;
        let before = cap.flight(&mut ws, vessel).await;
        cap.send(&mut ws, C::RunExperiments { vessel }).await;
        cap.wait(&mut ws, |m| {
            matches!(m, S::ScienceReports { .. }).then_some(())
        })
        .await;
        cap.wait(&mut ws, |m| {
            if let S::ExperimentFinished {
                experiment,
                interrupted,
                ..
            } = m
            {
                (experiment == "SurfaceSurvey" && !interrupted).then_some(())
            } else {
                None
            }
        })
        .await;
        let before_tx = cap.flight(&mut ws, vessel).await;
        let start = Instant::now();
        cap.send(&mut ws, C::TransmitScience { vessel }).await;
        let (reports, earned, reason) = cap
            .wait(&mut ws, |m| {
                if let S::ScienceSubmitted {
                    reports,
                    earned,
                    reason,
                    ..
                } = m
                {
                    Some((reports, earned, reason))
                } else {
                    None
                }
            })
            .await;
        let response_s = start.elapsed().as_secs_f64();
        let after = cap.flight(&mut ws, vessel).await;
        results.push(json!({"case":"transmission","starting_charge_fraction":fraction,"vessel":vessel,"link":before_tx.comm_connected,
            "ec_before_experiment":before.electric_charge,"ec_before_tx":before_tx.electric_charge,"ec_after_tx":after.electric_charge,
            "response_real_s":response_s,"submitted_reports":reports,"earned":earned,"reason":reason}));
        // Vessel recovery is a separate production command, exercised after transmission.
        cap.send(&mut ws, C::RecoverVessel { vessel }).await;
        cap.wait(&mut ws, |m| {
            if let S::ScienceSubmitted {
                recovered, reason, ..
            } = m
            {
                assert!(recovered && reason.is_none(), "{reason:?}");
                Some(())
            } else {
                None
            }
        })
        .await;
    }
    cap.send(&mut ws, C::LaunchOnPad { craft: base }).await;
    let pod = cap
        .wait(&mut ws, |m| {
            if let S::CraftLaunched { vessel, .. } = m {
                vessel
            } else {
                None
            }
        })
        .await;
    cap.send(
        &mut ws,
        C::GoEva {
            vessel: pod,
            kerbal: None,
        },
    )
    .await;
    let eva = cap
        .wait(&mut ws, |m| {
            if let S::CrewAck {
                accepted,
                new_vessel,
                reason,
                ..
            } = m
            {
                assert!(accepted, "{reason:?}");
                new_vessel
            } else {
                None
            }
        })
        .await;
    let mut flags = vec![];
    for trial in 0..2 {
        let start = Instant::now();
        cap.send(&mut ws, C::PlantFlag { vessel: eva }).await;
        let (accepted, id, reason) = cap
            .wait(&mut ws, |m| {
                if let S::CrewAck {
                    accepted,
                    new_vessel,
                    reason,
                    ..
                } = m
                {
                    Some((accepted, new_vessel, reason))
                } else {
                    None
                }
            })
            .await;
        let response_s = start.elapsed().as_secs_f64();
        let mut distance = None;
        if let Some(id) = id {
            flags.push(id);
            distance = Some(
                cap.wait(&mut ws, |m| {
                    if let S::Vessels { vessels, .. } = m {
                        let e = vessels.iter().find(|v| v.id == eva)?;
                        let f = vessels.iter().find(|v| v.id == id)?;
                        Some((e.position_m - f.position_m).length())
                    } else {
                        None
                    }
                })
                .await,
            );
        }
        results.push(json!({"case":"flag_placement","trial":trial,"accepted":accepted,"reason":reason,"distance_m":distance,"response_real_s":response_s}));
    }
    assert!(!flags.is_empty(), "No flag captured");
    cap.send(&mut ws, C::QuickSave).await;
    cap.wait(&mut ws, |m| {
        if let S::SaveAck {
            accepted, reason, ..
        } = m
        {
            assert!(accepted, "{reason:?}");
            Some(())
        } else {
            None
        }
    })
    .await;
    cap.send(&mut ws, C::RecoverVessel { vessel: flags[0] })
        .await;
    let recovered = cap
        .wait(&mut ws, |m| {
            if let S::ScienceSubmitted { recovered, .. } = m {
                Some(recovered)
            } else {
                None
            }
        })
        .await;
    results.push(json!({"case":"flag_vessel_recovery","recovered":recovered,"note":"RecoverVessel; OSE has no Kerbal flag pickup intent"}));
    cap.send(&mut ws, C::QuickLoad).await;
    cap.wait(&mut ws, |m| {
        if let S::SaveAck {
            accepted, reason, ..
        } = m
        {
            assert!(accepted, "{reason:?}");
            Some(())
        } else {
            None
        }
    })
    .await;
    let restored = cap
        .wait(&mut ws, |m| {
            if let S::Vessels { vessels, .. } = m {
                vessels.iter().any(|v| v.id == flags[0]).then_some(true)
            } else {
                None
            }
        })
        .await;
    results.push(json!({"case":"flag_save_restore","restored":restored}));
    ws.close(None).await.unwrap();
    let reconnect_start = Instant::now();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    cap.send(
        &mut ws,
        C::Hello {
            protocol: ose_net::PROTOCOL_VERSION,
            token: Some(token),
            display_name: "Services audit".into(),
        },
    )
    .await;
    cap.wait(&mut ws, |m| matches!(m, S::Welcome { .. }).then_some(()))
        .await;
    let latency = reconnect_start.elapsed().as_secs_f64();
    cap.wait(&mut ws, |m| {
        if let S::Vessels { vessels, .. } = m {
            vessels.iter().any(|v| v.id == flags[0]).then_some(())
        } else {
            None
        }
    })
    .await;
    results.push(json!({"case":"websocket_reconnect","welcome_real_s":latency,"flag_retained":true,"note":"Transport reconnection; separate from CommNet"}));
    std::fs::write(
        out.join("ose-ws-summary.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    println!("Runtime evidence: {}", out.display());
}

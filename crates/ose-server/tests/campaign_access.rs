//! Exercise account identity and campaign access over independent WebSocket connections and restarts.
use futures_util::{SinkExt, StreamExt};
use ose_net::{ClientMessage as C, ServerMessage as S, PROTOCOL_VERSION};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn start(dir: &Path) -> (Server, String) {
    start_with_mail(dir, None).await
}
async fn start_with_mail(dir: &Path, mail: Option<u16>) -> (Server, String) {
    start_config(dir, mail, true).await
}
async fn start_config(dir: &Path, mail: Option<u16>, allow_guests: bool) -> (Server, String) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_ose-server"));
    if allow_guests {
        command.arg("--allow-guest-accounts");
    }
    if let Some(port) = mail {
        command
            .env("OSE_SMTP_HOST", "127.0.0.1")
            .env("OSE_SMTP_PORT", port.to_string())
            .env("OSE_SMTP_FROM", "OpenSpaceExplorer <ose@example.com>");
    }
    let child = command
        .args(["--listen", &addr, "--save-interval-s", "3600", "--db"])
        .arg(dir.join("accounts.json"))
        .arg("--save")
        .arg(dir.join("save.json"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let server = Server(child);
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            return (server, addr);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not start");
}
async fn connect(addr: &str, token: Option<String>) -> (Ws, S) {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    send(
        &mut ws,
        C::Hello {
            protocol: PROTOCOL_VERSION,
            token,
            display_name: "Pilot".into(),
        },
    )
    .await;
    let welcome = recv(&mut ws).await;
    (ws, welcome)
}
async fn send(ws: &mut Ws, m: C) {
    ws.send(Message::Binary(ose_net::encode(&m).unwrap().into()))
        .await
        .unwrap();
}
async fn recv(ws: &mut Ws) -> S {
    match tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    {
        Message::Binary(b) => ose_net::decode(&b).unwrap(),
        m => panic!("unexpected {m:?}"),
    }
}
async fn wait<T>(ws: &mut Ws, mut f: impl FnMut(S) -> Option<T>) -> T {
    for _ in 0..100 {
        if let Some(v) = f(recv(ws).await) {
            return v;
        }
    }
    panic!("condition did not arrive");
}
async fn list(ws: &mut Ws) -> Vec<ose_net::SessionInfo> {
    send(ws, C::ListSessions).await;
    wait(ws, |m| match m {
        S::SessionList { sessions } => Some(sessions),
        _ => None,
    })
    .await
}
async fn create(ws: &mut Ws, name: &str) -> String {
    send(
        ws,
        C::CreateSession {
            name: name.into(),
            mode: ose_net::SessionMode::Sandbox,
            difficulty: vec![],
        },
    )
    .await;
    wait(ws, |m| match m {
        S::CampaignState {
            id, created: true, ..
        } => Some(id),
        S::Rejected { reason } => panic!("{reason}"),
        _ => None,
    })
    .await
}
async fn share(ws: &mut Ws, id: &str) -> String {
    send(ws, C::ShareSession { id: id.into() }).await;
    wait(ws, |m| match m {
        S::SessionShareCode { code, .. } => Some(code),
        S::Rejected { reason } => panic!("{reason}"),
        _ => None,
    })
    .await
}
async fn reject(ws: &mut Ws, m: C) {
    send(ws, m).await;
    wait(ws, |m| match m {
        S::Rejected { .. } => Some(()),
        _ => None,
    })
    .await;
}
async fn join(ws: &mut Ws, code: &str, expected: &str) {
    send(ws, C::JoinSessionCode { code: code.into() }).await;
    let id = wait(ws, |m| match m {
        S::SessionJoined { id, .. } => Some(id),
        S::Rejected { reason } => panic!("{reason}"),
        _ => None,
    })
    .await;
    assert_eq!(id, expected);
    // Drain the list sent after SessionJoined before issuing another request.
    wait(ws, |m| matches!(m, S::SessionList { .. }).then_some(())).await;
}
#[tokio::test]
async fn private_campaigns_and_portable_accounts_survive_restart() {
    let dir = std::env::temp_dir().join(format!("ose-access-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (server, addr) = start(&dir).await;
    let (mut owner, welcome) = connect(&addr, None).await;
    let S::Welcome {
        player_id: owner_id,
        token: owner_token,
        ..
    } = welcome
    else {
        panic!("welcome");
    };
    assert!(list(&mut owner).await.is_empty());
    let alpha = create(&mut owner, "Alpha").await;
    let alpha_code = share(&mut owner, &alpha).await;
    assert_ne!(alpha_code, owner_token);
    let (mut guest, welcome) = connect(&addr, None).await;
    let S::Welcome {
        player_id: guest_id,
        token: guest_token,
        ..
    } = welcome
    else {
        panic!("welcome");
    };
    assert_ne!(guest_id, owner_id);
    assert!(list(&mut guest).await.is_empty());
    reject(&mut guest, C::SelectSession { id: alpha.clone() }).await;
    reject(&mut guest, C::JoinCampaign).await;
    reject(&mut guest, C::CampaignInfo).await;
    reject(&mut guest, C::ShareSession { id: alpha.clone() }).await;
    reject(&mut guest, C::ClaimSession { id: alpha.clone() }).await;
    reject(
        &mut guest,
        C::JoinSessionCode {
            code: owner_token.clone(),
        },
    )
    .await;
    reject(&mut guest, C::JoinSessionCode { code: "bad".into() }).await;
    reject(
        &mut guest,
        C::SetAgency {
            agency: ose_net::Agency {
                flag_id: "default".into(),
                primary_rgb: [0; 3],
                secondary_rgb: [0; 3],
            },
        },
    )
    .await;
    // Unjoined visitors receive no private game state, including periodic presence pushes.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1200);
    while let Ok(Some(Ok(Message::Binary(b)))) =
        tokio::time::timeout_at(deadline, guest.next()).await
    {
        let m: S = ose_net::decode(&b).unwrap();
        match m {
            S::SessionPresence {
                sessions,
                campaign_id,
                members,
            } => {
                assert!(sessions.is_empty() && campaign_id.is_empty() && members.is_empty());
            }
            S::BodyCatalog { .. }
            | S::BodyRotation { .. }
            | S::DifficultyOptions { .. }
            | S::LaunchSites { .. } => {}
            other => panic!("private data leaked: {other:?}"),
        }
    }
    // Joining grants membership while the owner keeps playing; duplicate redemption is idempotent.
    join(&mut guest, &alpha_code, &alpha).await;
    join(&mut guest, &alpha_code, &alpha).await;
    let games = list(&mut guest).await;
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].owner, Some(owner_id));
    assert_eq!(games[0].players, 1);
    // Members can share, too.
    assert_eq!(share(&mut guest, &alpha).await, alpha_code);
    let beta = create(&mut owner, "Beta").await;
    assert_eq!(share(&mut owner, &alpha).await, alpha_code); // shelved sharing
    assert_eq!(list(&mut guest).await[0].id, alpha);
    reject(&mut guest, C::SelectSession { id: beta.clone() }).await;
    let beta_code = share(&mut owner, &beta).await;
    // Game invitations cannot be used to log in to somebody's account.
    let (_bad, m) = connect(&addr, Some(beta_code.clone())).await;
    assert!(matches!(m, S::Rejected { .. }));
    let (_bad, m) = connect(&addr, Some("invalid login".into())).await;
    assert!(matches!(m, S::Rejected { .. }));
    // No periodic save has run: acknowledged metadata must already be durable.
    drop(owner);
    drop(guest);
    drop(server);
    let (server, addr) = start(&dir).await;
    let (mut owner, welcome) = connect(&addr, Some(owner_token.clone())).await;
    assert!(matches!(welcome, S::Welcome { player_id, .. } if player_id == owner_id));
    let (mut second_machine, welcome) = connect(&addr, Some(owner_token)).await;
    assert!(matches!(welcome, S::Welcome { player_id, .. } if player_id == owner_id));
    assert_eq!(list(&mut owner).await.len(), 2);
    assert_eq!(list(&mut second_machine).await.len(), 2);
    let (mut guest, welcome) = connect(&addr, Some(guest_token)).await;
    assert!(matches!(welcome, S::Welcome { player_id, .. } if player_id == guest_id));
    assert_eq!(list(&mut guest).await.len(), 1);
    join(&mut guest, &beta_code, &beta).await;
    assert_eq!(list(&mut guest).await.len(), 2);
    let (mut newcomer, _) = connect(&addr, None).await;
    join(&mut newcomer, &alpha_code, &alpha).await; // invitation survived on shelf
    assert_eq!(list(&mut newcomer).await[0].id, alpha);
    assert_eq!(share(&mut owner, &alpha).await, alpha_code);
    drop(owner);
    drop(second_machine);
    drop(guest);
    drop(newcomer);
    drop(server);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn invitations_work_across_concurrent_rooms() {
    let dir = std::env::temp_dir().join(format!("ose-access-rooms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (server, addr) = start(&dir).await;
    let (mut owner, _) = connect(&addr, None).await;
    let alpha = create(&mut owner, "Alpha room").await;
    let code = share(&mut owner, &alpha).await;
    let (mut friend, _) = connect(&addr, None).await;
    join(&mut friend, &code, &alpha).await;
    send(&mut friend, C::SelectSession { id: alpha.clone() }).await;
    wait(&mut friend, |m| {
        matches!(
            m,
            S::CampaignState {
                you_are_member: true,
                ..
            }
        )
        .then_some(())
    })
    .await;
    // A second room opens; the friend's game keeps running.
    let beta = create(&mut owner, "Beta room").await;
    assert_eq!(share(&mut owner, &alpha).await, code);
    assert_eq!(share(&mut friend, &alpha).await, code);
    let beta_code = share(&mut owner, &beta).await;
    assert_eq!(list(&mut friend).await.len(), 1);
    reject(&mut friend, C::SelectSession { id: beta.clone() }).await;
    let (mut stranger, _) = connect(&addr, None).await;
    assert!(list(&mut stranger).await.is_empty());
    join(&mut stranger, &beta_code, &beta).await;
    send(&mut stranger, C::SelectSession { id: beta.clone() }).await;
    wait(&mut stranger, |m| {
        matches!(
            m,
            S::CampaignState {
                you_are_member: true,
                ..
            }
        )
        .then_some(())
    })
    .await;
    let a = list(&mut friend).await;
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].id, alpha);
    assert_eq!(a[0].players, 1);
    let b = list(&mut stranger).await;
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].id, beta);
    assert_eq!(b[0].players, 2);
    drop(owner);
    drop(friend);
    drop(stranger);
    drop(server);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A real SMTP socket captures only this test's outbound messages; no external email is sent.
async fn mail_sink() -> (
    u16,
    tokio::sync::mpsc::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::channel(20);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                stream
                    .get_mut()
                    .write_all(b"220 test SMTP\r\n")
                    .await
                    .unwrap();
                let mut line = String::new();
                loop {
                    line.clear();
                    if stream.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    let response: &[u8] = if line.starts_with("DATA") {
                        stream
                            .get_mut()
                            .write_all(b"354 send message\r\n")
                            .await
                            .unwrap();
                        let mut message = String::new();
                        loop {
                            line.clear();
                            stream.read_line(&mut line).await.unwrap();
                            if line == ".\r\n" {
                                break;
                            }
                            message.push_str(&line);
                        }
                        tx.send(message).await.unwrap();
                        b"250 queued\r\n"
                    } else if line.starts_with("QUIT") {
                        stream.get_mut().write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        b"250 test\r\n"
                    };
                    stream.get_mut().write_all(response).await.unwrap();
                }
            });
        }
    });
    (port, rx, task)
}
async fn email_code(
    ws: &mut Ws,
    inbox: &mut tokio::sync::mpsc::Receiver<String>,
    email: &str,
) -> (String, String) {
    send(
        ws,
        C::RequestEmailOtp {
            email: email.into(),
        },
    )
    .await;
    let challenge = wait(ws, |m| match m {
        S::EmailOtpSent {
            challenge,
            expires_in_secs,
            retry_after_secs,
            ..
        } => {
            assert_eq!(expires_in_secs, 600);
            assert_eq!(retry_after_secs, 60);
            Some(challenge)
        }
        S::AuthError { reason } => panic!("{reason}"),
        _ => None,
    })
    .await;
    let mail = tokio::time::timeout(Duration::from_secs(3), inbox.recv())
        .await
        .unwrap()
        .unwrap();
    let code = mail
        .split("code is: ")
        .nth(1)
        .unwrap()
        .chars()
        .take(6)
        .collect::<String>();
    assert!(code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()));
    (challenge, code)
}
async fn fresh_socket(addr: &str) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap()
        .0
}
fn verify_email(email: &str, challenge: &str, code: &str) -> C {
    C::EmailSignIn {
        protocol: PROTOCOL_VERSION,
        email: email.into(),
        challenge: challenge.into(),
        code: code.into(),
        display_name: "Email pilot".into(),
    }
}
#[tokio::test]
async fn email_otp_restores_games_on_new_machines_and_links_legacy_accounts() {
    let dir = std::env::temp_dir().join(format!("ose-email-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (port, mut inbox, mail_task) = mail_sink().await;
    let (server, addr) = start_with_mail(&dir, Some(port)).await;
    let mut first = fresh_socket(&addr).await;
    let (challenge, code) = email_code(&mut first, &mut inbox, "pilot@example.com").await;
    // Resend limits apply before mail delivery, and invalid codes do not sign in.
    send(
        &mut first,
        C::RequestEmailOtp {
            email: "pilot@example.com".into(),
        },
    )
    .await;
    assert!(matches!(recv(&mut first).await, S::AuthError { .. }));
    send(
        &mut first,
        verify_email("pilot@example.com", &challenge, "bad"),
    )
    .await;
    assert!(matches!(recv(&mut first).await, S::AuthError { .. }));
    send(
        &mut first,
        verify_email("pilot@example.com", &challenge, &code),
    )
    .await;
    let S::Welcome {
        player_id: owner,
        token,
        ..
    } = recv(&mut first).await
    else {
        panic!("welcome");
    };
    let game = create(&mut first, "Email-owned game").await;
    // Used codes are rejected even from another browser's connection.
    let mut replay = fresh_socket(&addr).await;
    send(
        &mut replay,
        verify_email("pilot@example.com", &challenge, &code),
    )
    .await;
    assert!(matches!(recv(&mut replay).await, S::AuthError { .. }));
    // A signed-in player links an email to their existing identity and campaign.
    let (mut legacy, m) = connect(&addr, None).await;
    let S::Welcome {
        player_id: legacy_id,
        token: legacy_token,
        ..
    } = m
    else {
        panic!("welcome");
    };
    let legacy_game = create(&mut legacy, "Existing game").await;
    let (challenge, code) = email_code(&mut legacy, &mut inbox, "legacy@example.com").await;
    send(
        &mut legacy,
        verify_email("legacy@example.com", &challenge, &code),
    )
    .await;
    wait(&mut legacy, |m| match m {
        S::AccountState { email: Some(email) } => {
            assert_eq!(email, "legacy@example.com");
            Some(())
        }
        _ => None,
    })
    .await;
    assert_eq!(list(&mut legacy).await[0].owner, Some(legacy_id));
    // Restart discards pending codes, but verified emails, sessions and membership are durable.
    drop(first);
    drop(legacy);
    drop(replay);
    drop(server);
    let (server, addr) = start_with_mail(&dir, Some(port)).await;
    let mut second = fresh_socket(&addr).await;
    let (challenge, code) = email_code(&mut second, &mut inbox, "PILOT@example.com").await;
    send(
        &mut second,
        verify_email("pilot@example.com", &challenge, &code),
    )
    .await;
    let S::Welcome {
        player_id,
        token: second_token,
        ..
    } = recv(&mut second).await
    else {
        panic!("welcome");
    };
    assert_eq!(player_id, owner);
    assert_ne!(second_token, token);
    assert_eq!(list(&mut second).await[0].id, game);
    let (mut remembered, m) = connect(&addr, Some(token)).await;
    assert!(matches!(m,S::Welcome { player_id,.. } if player_id==owner));
    assert_eq!(list(&mut remembered).await[0].id, game);
    let mut recovered = fresh_socket(&addr).await;
    let (challenge, code) = email_code(&mut recovered, &mut inbox, "legacy@example.com").await;
    send(
        &mut recovered,
        verify_email("legacy@example.com", &challenge, &code),
    )
    .await;
    assert!(
        matches!(recv(&mut recovered).await,S::Welcome { player_id,.. } if player_id==legacy_id)
    );
    assert_eq!(list(&mut recovered).await[0].id, legacy_game);
    let (_remembered, m) = connect(&addr, Some(legacy_token)).await;
    assert!(matches!(m,S::Welcome { player_id,.. } if player_id==legacy_id));
    drop(second);
    drop(remembered);
    drop(recovered);
    drop(server);
    mail_task.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn public_server_requires_verified_email_for_new_accounts() {
    let dir = std::env::temp_dir().join(format!("ose-public-auth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (server, addr) = start_config(&dir, None, false).await;
    let (_socket, reply) = connect(&addr, None).await;
    assert!(matches!(reply, S::Rejected { .. }));
    drop(server);
    std::fs::remove_dir_all(dir).unwrap();
}

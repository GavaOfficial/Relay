use std::{collections::HashMap, time::Duration};

use futures_util::{SinkExt, StreamExt};
use relay_common::{AgentState, ClientMsg, PlayerStatus, ServerMsg};
use relay_server::{app, state::AppState};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn spawn_server() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let tokens: HashMap<String, String> = [("ta", "alice"), ("tb", "bob"), ("tc", "carol")]
        .into_iter()
        .map(|(t, u)| (t.to_string(), u.to_string()))
        .collect();
    let st = AppState::new(dir.path().to_path_buf(), tokens)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app(st)).await.unwrap() });
    (format!("127.0.0.1:{}", addr.port()), dir)
}

async fn connect(host: &str, id: &str, token: &str) -> Ws {
    let mut req = format!("ws://{host}/api/matches/{id}/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    connect_async(req).await.unwrap().0
}

async fn expect<F: Fn(&ServerMsg) -> bool>(ws: &mut Ws, pred: F) -> ServerMsg {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Message::Text(t) = ws.next().await.expect("socket chiuso").unwrap() {
                let m: ServerMsg = serde_json::from_str(&t).unwrap();
                if pred(&m) {
                    return m;
                }
            }
        }
    })
    .await
    .expect("timeout in attesa del messaggio")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test]
async fn start_stop_are_synchronised() {
    let (host, _dir) = spawn_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{host}");

    let m: serde_json::Value = http
        .post(format!("{base}/api/matches"))
        .bearer_auth("ta")
        .json(&serde_json::json!({"players": ["alice", "bob"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = m["id"].as_str().unwrap().to_string();

    let mut req = format!("ws://{host}/api/matches/{id}/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", "Bearer tc".parse().unwrap());
    assert!(connect_async(req).await.is_err());

    let start = |force: bool| {
        let (http, base, id) = (http.clone(), base.clone(), id.clone());
        async move {
            http.post(format!(
                "{base}/api/matches/{id}/start{}",
                if force { "?force=true" } else { "" }
            ))
            .bearer_auth("ta")
            .send()
            .await
            .unwrap()
        }
    };

    let mut wa = connect(&host, &id, "ta").await;
    expect(&mut wa, |m| matches!(m, ServerMsg::State { .. })).await;
    assert_eq!(start(false).await.status(), 409);

    let t0 = now_ms();
    wa.send(Message::Text(
        serde_json::to_string(&ClientMsg::Ping { t0 })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    match expect(&mut wa, |m| matches!(m, ServerMsg::Pong { .. })).await {
        ServerMsg::Pong {
            t0: echoed,
            server_ms,
        } => {
            assert_eq!(echoed, t0);
            assert!(server_ms.abs_diff(now_ms()) < 1000);
        }
        _ => unreachable!(),
    }

    let mut wb = connect(&host, &id, "tb").await;
    expect(
        &mut wa,
        |m| matches!(m, ServerMsg::Presence { connected } if connected.len() == 2),
    )
    .await;

    let r = http
        .post(format!("{base}/api/matches/{id}/start"))
        .bearer_auth("tb")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    assert_eq!(start(false).await.status(), 409);

    let send_status = |s: PlayerStatus| serde_json::to_string(&ClientMsg::Status(s)).unwrap();
    let ready = PlayerStatus {
        state: AgentState::Waiting,
        window_found: Some(true),
        rtt_ms: Some(20),
        window: Some("game.exe - Titolo privato".into()),
        ..Default::default()
    };
    wa.send(Message::Text(send_status(ready.clone()).into()))
        .await
        .unwrap();
    wb.send(Message::Text(
        send_status(PlayerStatus {
            window_found: Some(false),
            ..ready.clone()
        })
        .into(),
    ))
    .await
    .unwrap();
    let health = expect(&mut wa, |m| {
        matches!(m, ServerMsg::Health { players } if players.iter().any(|p| p.id == "bob" && p.issue.as_deref() == Some("finestra non trovata")))
    })
    .await;
    match health {
        ServerMsg::Health { players } => {
            let alice = players.iter().find(|p| p.id == "alice").unwrap();
            assert!(alice.ready, "alice e' pronta: {alice:?}");
            assert!(!players.iter().find(|p| p.id == "bob").unwrap().ready);
        }
        _ => unreachable!(),
    }
    assert_eq!(start(false).await.status(), 409);

    wb.send(Message::Text(send_status(ready.clone()).into()))
        .await
        .unwrap();
    let all_ready = expect(
        &mut wa,
        |m| matches!(m, ServerMsg::Health { players } if players.iter().all(|p| p.ready)),
    )
    .await;

    match all_ready {
        ServerMsg::Health { players } => {
            let w = players
                .iter()
                .find(|p| p.id == "bob")
                .unwrap()
                .status
                .as_ref()
                .unwrap()
                .window
                .clone();
            assert_eq!(w.as_deref(), Some("game.exe - Titolo privato"));
        }
        _ => unreachable!(),
    }

    match expect(&mut wb, |m| matches!(m, ServerMsg::Health { players } if players.iter().any(|p| p.id == "alice" && p.status.is_some()))).await {
        ServerMsg::Health { players } => {
            let a = players.iter().find(|p| p.id == "alice").unwrap().status.as_ref().unwrap();
            assert_eq!(a.window, None, "un giocatore non deve vedere il titolo della finestra degli altri");
            assert_eq!(a.window_found, Some(true));
        }
        _ => unreachable!(),
    }

    let before = now_ms();
    assert_eq!(start(false).await.status(), 200);
    let ta = match expect(&mut wa, |m| matches!(m, ServerMsg::Start { .. })).await {
        ServerMsg::Start { at_ms } => at_ms,
        _ => unreachable!(),
    };
    let tb = match expect(&mut wb, |m| matches!(m, ServerMsg::Start { .. })).await {
        ServerMsg::Start { at_ms } => at_ms,
        _ => unreachable!(),
    };
    assert_eq!(ta, tb);
    assert_eq!(ta % 4000, 0);
    assert!(ta >= before + 5000 && ta < before + 5000 + 4000 + 500);
    assert_eq!(start(false).await.status(), 409);

    drop(wb);
    let mut wb2 = connect(&host, &id, "tb").await;
    match expect(&mut wb2, |m| matches!(m, ServerMsg::State { .. })).await {
        ServerMsg::State { start_at_ms, .. } => assert_eq!(start_at_ms, Some(ta)),
        _ => unreachable!(),
    }

    let r = http
        .post(format!("{base}/api/matches/{id}/stop"))
        .bearer_auth("ta")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let sa = match expect(&mut wa, |m| matches!(m, ServerMsg::Stop { .. })).await {
        ServerMsg::Stop { at_ms } => at_ms,
        _ => unreachable!(),
    };
    let sb = match expect(&mut wb2, |m| matches!(m, ServerMsg::Stop { .. })).await {
        ServerMsg::Stop { at_ms } => at_ms,
        _ => unreachable!(),
    };
    assert_eq!(sa, sb);
    assert_eq!((sa - ta) % 4000, 0);
}

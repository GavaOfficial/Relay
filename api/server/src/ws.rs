use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
};
use relay_common::{ClientMsg, MatchInfo, ServerMsg};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, routes::now_ms, state::AppState};

pub async fn match_ws(
    State(st): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let m = st
        .matches
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or(AppError::NotFound)?;
    if !m.players.contains(&user) && m.coordinator != user {
        return Err(AppError::Forbidden);
    }
    Ok(ws.on_upgrade(move |socket| handle(st, id, user, socket)))
}

fn state_msg(m: &MatchInfo) -> ServerMsg {
    ServerMsg::State {
        status: m.status,
        start_at_ms: m.started_at_ms,
        stop_at_ms: m.stopped_at_ms,
    }
}

async fn handle(st: Arc<AppState>, id: Uuid, user: String, mut socket: WebSocket) {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let conn = st.hub_register(id, &user, tx.clone());
    tracing::info!("ws: {user} connesso alla partita {id}");
    let mut reason = "chiuso dal client";

    if let Some(m) = st.matches.read().await.get(&id) {
        let _ = tx.send(state_msg(m));
    }
    st.hub_broadcast(
        id,
        &ServerMsg::Presence {
            connected: st.hub_connected(id),
        },
    );
    push_health(&st, id).await;

    loop {
        tokio::select! {
            out = rx.recv() => {
                let Some(msg) = out else { break };
                let Ok(text) = serde_json::to_string(&msg) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    reason = "invio non riuscito (rete o proxy)";
                    break;
                }
            }
            inc = socket.recv() => {
                match inc {
                    Some(Ok(Message::Text(t))) => {
                        match serde_json::from_str::<ClientMsg>(&t) {
                            Ok(ClientMsg::Ping { t0 }) => {
                                let _ = tx.send(ServerMsg::Pong { t0, server_ms: now_ms() });
                            }
                            Ok(ClientMsg::Status(s)) => {
                                if is_player(&st, id, &user).await {
                                    st.status_set(id, &user, s);
                                    push_health(&st, id).await;
                                }
                            }
                            Err(_) => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    None => break,
                    Some(Err(e)) => {
                        tracing::info!("ws: {user} errore di lettura: {e}");
                        reason = "errore di lettura";
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    tracing::info!("ws: {user} scollegato dalla partita {id} ({reason})");
    st.hub_unregister(id, &user, conn);
    st.hub_broadcast(
        id,
        &ServerMsg::Presence {
            connected: st.hub_connected(id),
        },
    );
    push_health(&st, id).await;
}

async fn is_player(st: &AppState, id: Uuid, user: &str) -> bool {
    st.matches
        .read()
        .await
        .get(&id)
        .is_some_and(|m| m.players.iter().any(|p| p == user))
}

pub async fn push_health(st: &AppState, id: Uuid) {
    let m = st
        .matches
        .read()
        .await
        .get(&id)
        .map(|m| (m.players.clone(), m.coordinator.clone()));
    if let Some((players, coordinator)) = m {
        st.broadcast_health(id, &players, &coordinator);
    }
}

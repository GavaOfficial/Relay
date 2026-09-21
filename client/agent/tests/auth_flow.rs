use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use relay_agent::{
    credentials::{logout, resolve_session},
    login::{login_with, LoginOptions},
    pkce,
    tokenstore::{MemoryStore, TokenStore},
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Default)]
struct Inner {
    codes: HashMap<String, (String, String)>,
    refresh_valid: HashSet<String>,
    counter: u32,
    tamper_state: bool,
    deny: bool,

    store_at_session: Option<Option<String>>,
    logout_calls: Vec<String>,
}

#[derive(Clone)]
struct Mock {
    inner: Arc<Mutex<Inner>>,
    store: Arc<MemoryStore>,
}

async fn m_login(State(m): State<Mock>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut i = m.inner.lock().unwrap();
    if q.get("client_id").map(String::as_str) != Some("relay")
        || q.get("code_challenge_method").map(String::as_str) != Some("S256")
        || q.get("response_type").map(String::as_str) != Some("code")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let redirect = q["redirect_uri"].clone();
    let state = if i.tamper_state {
        "manomesso".to_string()
    } else {
        q["state"].clone()
    };
    if i.deny {
        return Redirect::temporary(&format!(
            "{redirect}?error=access_denied&error_description=L%27utente+ha+negato&state={state}"
        ))
        .into_response();
    }
    i.counter += 1;
    let code = format!("code-{}", i.counter);
    i.codes.insert(
        code.clone(),
        (q["code_challenge"].clone(), redirect.clone()),
    );
    Redirect::temporary(&format!("{redirect}?code={code}&state={state}")).into_response()
}

async fn m_token(State(m): State<Mock>, Json(b): Json<Value>) -> Response {
    let mut i = m.inner.lock().unwrap();
    let s = |k: &str| b[k].as_str().unwrap_or("").to_string();
    match s("grant_type").as_str() {
        "authorization_code" => {
            let Some((chal, redir)) = i.codes.remove(&s("code")) else {
                return (StatusCode::BAD_REQUEST, "codice non valido").into_response();
            };
            if s("client_id") != "relay"
                || s("redirect_uri") != redir
                || pkce::challenge(&s("code_verifier")) != chal
            {
                return (StatusCode::BAD_REQUEST, "pkce/redirect non valido").into_response();
            }
        }
        "refresh_token" => {
            if !i.refresh_valid.remove(&s("refresh_token")) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "invalid_grant"})),
                )
                    .into_response();
            }
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    }
    i.counter += 1;
    let refresh = format!("ref-{}", i.counter);
    i.refresh_valid.insert(refresh.clone());
    Json(json!({
        "access_token": format!("acc-{}", i.counter),
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": 900
    }))
    .into_response()
}

async fn m_logout(State(m): State<Mock>, Json(b): Json<Value>) -> StatusCode {
    let rt = b["refresh_token"].as_str().unwrap_or("").to_string();
    let mut i = m.inner.lock().unwrap();
    i.refresh_valid.remove(&rt);
    i.logout_calls.push(rt);
    StatusCode::NO_CONTENT
}

async fn m_session(State(m): State<Mock>, h: HeaderMap) -> Response {
    let auth = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !auth.starts_with("Bearer acc-") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    m.inner.lock().unwrap().store_at_session = Some(m.store.get().unwrap());
    Json(json!({"session": "sess-1", "expires_in": 86400, "user": "user-1", "name": "Mario"}))
        .into_response()
}

async fn start() -> (String, Mock) {
    let m = Mock {
        inner: Arc::default(),
        store: Arc::new(MemoryStore::new()),
    };
    let app = Router::new()
        .route("/login", get(m_login))
        .route("/token", post(m_token))
        .route("/logout", post(m_logout))
        .route("/api/session", post(m_session))
        .with_state(m.clone());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, m)
}

fn opts(ports: Vec<u16>, secs: u64) -> LoginOptions {
    LoginOptions {
        ports,
        timeout: Duration::from_secs(secs),
    }
}

fn browser(seen: Arc<Mutex<Option<String>>>) -> impl FnOnce(&str) -> anyhow::Result<()> {
    move |u: &str| {
        *seen.lock().unwrap() = Some(u.to_string());
        let u = u.to_string();
        tokio::spawn(async move {
            let _ = reqwest::get(u).await;
        });
        Ok(())
    }
}

fn no_seen() -> Arc<Mutex<Option<String>>> {
    Arc::default()
}

#[tokio::test]
async fn login_completo() {
    let (url, m) = start().await;
    let t = login_with(&url, "relay", browser(no_seen()), opts(vec![0], 10))
        .await
        .unwrap();
    assert!(t.access_token.starts_with("acc-"));
    assert!(t.refresh_token.starts_with("ref-"));
    assert_eq!(t.expires_in, 900);
    assert!(m.inner.lock().unwrap().codes.is_empty());
}

#[tokio::test]
async fn state_manomesso() {
    let (url, m) = start().await;
    m.inner.lock().unwrap().tamper_state = true;
    let e = login_with(&url, "relay", browser(no_seen()), opts(vec![0], 10))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("state"), "{e}");
}

#[tokio::test]
async fn accesso_negato() {
    let (url, m) = start().await;
    m.inner.lock().unwrap().deny = true;
    let e = login_with(&url, "relay", browser(no_seen()), opts(vec![0], 10))
        .await
        .unwrap_err();
    let s = e.to_string();
    assert!(s.contains("access_denied") && s.contains("negato"), "{s}");
}

#[tokio::test]
async fn timeout() {
    let (url, _m) = start().await;
    let e = login_with(&url, "relay", |_: &str| Ok(()), opts(vec![0], 1))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("non completato"), "{e}");
}

fn redirect_port(login_url: &str) -> u16 {
    let u = reqwest::Url::parse(login_url).unwrap();
    let (_, r) = u.query_pairs().find(|(k, _)| k == "redirect_uri").unwrap();
    reqwest::Url::parse(&r).unwrap().port().unwrap()
}

#[tokio::test]
async fn porta_occupata_usa_la_successiva() {
    let (url, _m) = start().await;
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let seen = no_seen();
    login_with(
        &url,
        "relay",
        browser(seen.clone()),
        opts(vec![busy_port, 0], 10),
    )
    .await
    .unwrap();
    let used = redirect_port(seen.lock().unwrap().as_ref().unwrap());
    assert_ne!(used, busy_port);
}

#[tokio::test]
async fn tutte_occupate() {
    let (url, _m) = start().await;
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = busy.local_addr().unwrap().port();
    let e = login_with(&url, "relay", |_: &str| Ok(()), opts(vec![p], 5))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("nessuna porta libera"), "{e}");
}

#[tokio::test]
async fn richieste_spurie_e_malformate_ignorate() {
    let (url, _m) = start().await;
    let opener = |u: &str| {
        let u = u.to_string();
        tokio::spawn(async move {
            let port = redirect_port(&u);
            let addr = format!("127.0.0.1:{port}");
            let _ = reqwest::get(format!("http://{addr}/favicon.ico")).await;

            let mut s = TcpStream::connect(&addr).await.unwrap();
            s.write_all(b"spazzatura\r\n\r\n").await.unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out).await;
            assert!(out.starts_with("HTTP/1.1 400"), "{out}");

            let _idle = TcpStream::connect(&addr).await.unwrap();
            let _ = reqwest::get(u).await;
        });
        Ok(())
    };
    let t = login_with(&url, "relay", opener, opts(vec![0], 10))
        .await
        .unwrap();
    assert!(t.refresh_token.starts_with("ref-"));
}

#[tokio::test]
async fn sessione_con_rotazione_del_refresh() {
    let (url, m) = start().await;
    m.inner.lock().unwrap().refresh_valid.insert("ref-0".into());
    m.store.set("ref-0").unwrap();
    let s = resolve_session(&url, &url, "relay", &*m.store)
        .await
        .unwrap();
    assert_eq!(
        (s.token.as_str(), s.user.as_str(), s.name.as_deref()),
        ("sess-1", "user-1", Some("Mario"))
    );
    let saved = m.store.get().unwrap().unwrap();
    assert_ne!(saved, "ref-0");

    assert_eq!(
        m.inner.lock().unwrap().store_at_session,
        Some(Some(saved.clone()))
    );

    assert!(!m.inner.lock().unwrap().refresh_valid.contains("ref-0"));
    resolve_session(&url, &url, "relay", &*m.store)
        .await
        .unwrap();
}

#[tokio::test]
async fn refresh_rifiutato_svuota_lo_store() {
    let (url, m) = start().await;
    m.store.set("ref-revocato").unwrap();
    let e = resolve_session(&url, &url, "relay", &*m.store)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("relay-agent login"), "{e}");
    assert_eq!(m.store.get().unwrap(), None);
}

#[tokio::test]
async fn rete_assente_store_intatto() {
    let store = MemoryStore::new();
    store.set("ref-1").unwrap();
    let dead = "http://127.0.0.1:1";
    let e = resolve_session(dead, dead, "relay", &store)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("rete"), "{e}");
    assert_eq!(store.get().unwrap().as_deref(), Some("ref-1"));
}

#[tokio::test]
async fn store_vuoto_chiede_login() {
    let store = MemoryStore::new();
    let e = resolve_session("http://x", "http://x", "relay", &store)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("relay-agent login"), "{e}");
}

#[tokio::test]
async fn logout_revoca_e_svuota() {
    let (url, m) = start().await;
    m.store.set("ref-9").unwrap();
    logout(&url, &*m.store).await.unwrap();
    assert_eq!(m.store.get().unwrap(), None);
    assert_eq!(
        m.inner.lock().unwrap().logout_calls,
        vec!["ref-9".to_string()]
    );
}

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};

use crate::pkce;

pub const PORTS: [u16; 5] = [53682, 53683, 53684, 53685, 53686];

#[derive(Debug, Clone, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Clone)]
pub struct LoginOptions {
    pub ports: Vec<u16>,
    pub timeout: Duration,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            ports: PORTS.to_vec(),
            timeout: Duration::from_secs(300),
        }
    }
}

pub async fn login(
    auth_url: &str,
    client_id: &str,
    opener: impl FnOnce(&str) -> Result<()>,
) -> Result<Tokens> {
    login_with(auth_url, client_id, opener, LoginOptions::default()).await
}

async fn bind_first(ports: &[u16]) -> Result<TcpListener> {
    for &p in ports {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)).await {
            return Ok(l);
        }
    }
    bail!(
        "nessuna porta libera per il login (provate: {:?}): chiudi i programmi che le usano e riprova",
        ports
    )
}

pub async fn login_with(
    auth_url: &str,
    client_id: &str,
    opener: impl FnOnce(&str) -> Result<()>,
    opts: LoginOptions,
) -> Result<Tokens> {
    let auth = auth_url.trim_end_matches('/');
    let listener = bind_first(&opts.ports).await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let verifier = pkce::verifier();
    let state = pkce::state();
    let mut url = reqwest::Url::parse(&format!("{auth}/login")).context("--auth-url non valido")?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("code_challenge", &pkce::challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    opener(url.as_str())?;

    let code = tokio::select! {
        r = tokio::time::timeout(opts.timeout, wait_callback(&listener, &state)) => {
            r.map_err(|_| anyhow!("login non completato entro {} secondi", opts.timeout.as_secs()))??
        }
        _ = tokio::signal::ctrl_c() => bail!("login annullato"),
    };
    drop(listener);

    exchange_code(auth, client_id, &redirect_uri, &code, &verifier).await
}

async fn wait_callback(listener: &TcpListener, state: &str) -> Result<String> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Result<String>>();
    loop {
        tokio::select! {
            acc = listener.accept() => {
                let Ok((sock, peer)) = acc else { continue };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let (tx, state) = (tx.clone(), state.to_string());
                tokio::spawn(async move {
                    if let Some(r) = handle_conn(sock, &state).await {
                        let _ = tx.send(r);
                    }
                });
            }
            r = rx.recv() => return r.unwrap_or_else(|| Err(anyhow!("canale callback chiuso"))),
        }
    }
}

const OK_PAGE: &str =
    "<!doctype html><html lang=\"it\"><meta charset=\"utf-8\"><title>Relay</title>\
<body style=\"font-family:sans-serif;text-align:center;margin-top:20vh\">\
<h2>Accesso completato</h2><p>Puoi chiudere questa scheda e tornare all'agente.</p></body></html>";

fn err_page(msg: &str) -> String {
    let msg = msg
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><html lang=\"it\"><meta charset=\"utf-8\"><title>Relay</title>\
<body style=\"font-family:sans-serif;text-align:center;margin-top:20vh\">\
<h2>Accesso non riuscito</h2><p>{msg}</p></body></html>"
    )
}

async fn respond(sock: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

async fn handle_conn(mut sock: TcpStream, state: &str) -> Option<Result<String>> {
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), async {
        let mut chunk = [0u8; 1024];
        while buf.len() < 8192 && !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        true
    })
    .await;
    if !matches!(read, Ok(true)) {
        if !buf.is_empty() {
            respond(
                &mut sock,
                "400 Bad Request",
                &err_page("richiesta non valida"),
            )
            .await;
        }
        return None;
    }

    let head = String::from_utf8_lossy(&buf);
    let line = head.lines().next().unwrap_or("");
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 3 || !parts[2].starts_with("HTTP/") || !parts[1].starts_with('/') {
        respond(
            &mut sock,
            "400 Bad Request",
            &err_page("richiesta non valida"),
        )
        .await;
        return None;
    }
    if parts[0] != "GET" {
        respond(
            &mut sock,
            "405 Method Not Allowed",
            &err_page("metodo non consentito"),
        )
        .await;
        return None;
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{}", parts[1])) else {
        respond(
            &mut sock,
            "400 Bad Request",
            &err_page("richiesta non valida"),
        )
        .await;
        return None;
    };
    if url.path() != "/callback" {
        respond(&mut sock, "404 Not Found", &err_page("pagina non trovata")).await;
        return None;
    }

    let (mut code, mut got_state, mut error, mut desc) = (None, None, None, None);
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => got_state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            "error_description" => desc = Some(v.into_owned()),
            _ => {}
        }
    }
    if code.is_none() && error.is_none() {
        respond(
            &mut sock,
            "400 Bad Request",
            &err_page("parametri mancanti"),
        )
        .await;
        return None;
    }

    if !got_state.as_deref().is_some_and(|s| pkce::ct_eq(s, state)) {
        respond(&mut sock, "400 Bad Request", &err_page("state non valido")).await;
        return Some(Err(anyhow!(
            "state della callback non valido: possibile tentativo di attacco, login interrotto"
        )));
    }
    if let Some(e) = error {
        let d = desc.map(|d| format!(": {d}")).unwrap_or_default();
        let msg = format!("accesso rifiutato dal provider ({e}{d})");
        respond(&mut sock, "200 OK", &err_page(&msg)).await;
        return Some(Err(anyhow!(msg)));
    }
    respond(&mut sock, "200 OK", OK_PAGE).await;
    Some(Ok(code.unwrap()))
}

async fn exchange_code(
    auth: &str,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens> {
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(format!("{auth}/token"))
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": client_id,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }))
        .send()
        .await
        .context("impossibile contattare GavaAuth per lo scambio del codice")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("GavaAuth ha rifiutato lo scambio del codice ({status}): {body}");
    }
    resp.json().await.context("risposta di GavaAuth non valida")
}

pub fn open_browser(url: &str) -> Result<()> {
    println!("Apri questo indirizzo nel browser per accedere:\n{url}\n");
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        tracing::warn!("impossibile aprire il browser ({e}): usa l'indirizzo qui sopra");
    }
    Ok(())
}

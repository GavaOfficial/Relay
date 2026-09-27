mod store;
mod update;

use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use relay_common::storage::{
    encode, Frame, Hello, Request, Response, DATA_FRAME, KIND_DATA, KIND_END, KIND_HELLO,
    KIND_REQUEST, KIND_RESPONSE, PROTOCOL_VERSION, TUNNEL_PATH,
};
use tokio::{io::AsyncReadExt, sync::mpsc};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use store::Store;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Config {
    server: String,
    key: String,
    dir: std::path::PathBuf,
    conns: u32,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn config() -> Result<Config, String> {
    Ok(Config {
        server: var("RELAY_SERVER").unwrap_or_else(|| "https://relay.gavatech.org".into()).trim_end_matches('/').to_string(),
        key: var("RELAY_STORAGE_KEY").ok_or("manca RELAY_STORAGE_KEY")?,
        dir: var("RELAY_STORAGE_DIR").ok_or("manca RELAY_STORAGE_DIR")?.into(),
        conns: var("RELAY_STORAGE_CONNS").and_then(|v| v.parse().ok()).unwrap_or(4).clamp(1, 16),
    })
}

fn tunnel_url(server: &str) -> String {
    let ws = if let Some(rest) = server.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = server.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{server}")
    };
    format!("{ws}{TUNNEL_PATH}")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "relay_storage=info".into()))
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = match config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("relay-storage: {e}");
            std::process::exit(2);
        }
    };
    let store = match Store::open(&cfg.dir) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("relay-storage: non riesco ad aprire {}: {e}", cfg.dir.display());
            std::process::exit(2);
        }
    };
    let s = store.stats();
    tracing::info!(
        "relay-storage {VERSION}: {} ({} file, {:.1} GB usati, {:.1} GB liberi), {} connessioni verso {}",
        cfg.dir.display(),
        s.blobs,
        s.used as f64 / 1e9,
        s.free as f64 / 1e9,
        cfg.conns,
        cfg.server
    );
    tokio::spawn(update::run(cfg.server.clone()));
    let url = tunnel_url(&cfg.server);
    let mut tasks = Vec::new();
    for conn in 0..cfg.conns {
        tasks.push(tokio::spawn(keep_connected(url.clone(), cfg.key.clone(), conn, store.clone())));
    }
    for t in tasks {
        let _ = t.await;
    }
}

async fn keep_connected(url: String, key: String, conn: u32, store: Arc<Store>) {
    let mut wait = 1;
    loop {
        match session(&url, &key, conn, &store).await {
            Ok(()) => {
                tracing::info!("connessione {conn} chiusa dal centrale");
                wait = 1;
            }
            Err(e) => tracing::warn!("connessione {conn}: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(wait)).await;
        wait = (wait * 2).min(30);
    }
}

async fn session(url: &str, key: &str, conn: u32, store: &Arc<Store>) -> Result<(), String> {
    let mut req = url.into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert("authorization", format!("Bearer {key}").parse().map_err(|_| "chiave non valida")?);
    let (ws, _) = tokio_tungstenite::connect_async(req).await.map_err(|e| match e {
        tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 401 => "chiave rifiutata dal centrale".to_string(),
        e => e.to_string(),
    })?;
    let (mut sink, mut stream) = ws.split();
    let hello = Hello { version: VERSION.into(), protocol: PROTOCOL_VERSION, conn, stats: store.stats() };
    sink.send(Message::Binary(encode(KIND_HELLO, 0, &hello, &[]).into())).await.map_err(|e| e.to_string())?;
    if conn == 0 {
        tracing::info!("collegato a {url}");
    }

    let (tx, mut rx) = mpsc::channel::<Message>(64);
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });
    let pinger = {
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(20)).await;
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        })
    };

    let result = loop {
        let msg = match tokio::time::timeout(Duration::from_secs(90), stream.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => break Err(e.to_string()),
            Ok(None) => break Ok(()),
            Err(_) => break Err("nessuna risposta dal centrale da 90 secondi".into()),
        };
        let bytes = match msg {
            Message::Binary(b) => b,
            Message::Close(_) => break Ok(()),
            _ => continue,
        };
        let Some(f) = Frame::parse(&bytes) else { continue };
        if f.kind != KIND_REQUEST {
            continue;
        }
        let id = f.id;
        let Some(req) = f.header::<Request>() else {
            let _ = tx.send(reply(id, Response::Error { message: "richiesta non valida".into() })).await;
            continue;
        };
        let payload = f.payload.to_vec();
        let (store, tx) = (store.clone(), tx.clone());
        tokio::spawn(async move { handle(req, id, payload, &store, &tx).await });
    };
    pinger.abort();
    drop(tx);
    let _ = writer.await;
    result
}

fn reply(id: u64, r: Response) -> Message {
    Message::Binary(encode(KIND_RESPONSE, id, &r, &[]).into())
}

fn done(r: Result<(), String>) -> Response {
    match r {
        Ok(()) => Response::Ok,
        Err(message) => Response::Error { message },
    }
}

async fn handle(req: Request, id: u64, payload: Vec<u8>, store: &Store, tx: &mpsc::Sender<Message>) {
    let resp = match req {
        Request::Put { blob, offset } => done(store.put(&blob, offset, &payload).await),
        Request::Commit { blob, size, sha256 } => {
            let r = store.commit(&blob, size, &sha256).await;
            match &r {
                Ok(()) => tracing::info!("ricevuto {blob} ({:.1} MB)", size as f64 / 1e6),
                Err(e) => tracing::warn!("{blob} rifiutato: {e}"),
            }
            done(r)
        }
        Request::Delete { blob } => done(store.delete(&blob).await),
        Request::Abort { blob } => done(store.abort(&blob).await),
        Request::Stat => Response::Stats(store.stats()),
        Request::Get { blob, offset, len } => {
            match store.open_range(&blob, offset, len).await {
                Ok(mut r) => {
                    let mut buf = vec![0u8; DATA_FRAME];
                    loop {
                        match r.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                let m = Message::Binary(encode(KIND_DATA, id, &(), &buf[..n]).into());
                                if tx.send(m).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(reply(id, Response::Error { message: e.to_string() })).await;
                                return;
                            }
                        }
                    }
                    let _ = tx.send(Message::Binary(encode(KIND_END, id, &(), &[]).into())).await;
                    return;
                }
                Err(message) => Response::Error { message },
            }
        }
    };
    let _ = tx.send(reply(id, resp)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_urls() {
        assert_eq!(tunnel_url("https://relay.gavatech.org"), "wss://relay.gavatech.org/api/storage/tunnel");
        assert_eq!(tunnel_url("http://127.0.0.1:18080"), "ws://127.0.0.1:18080/api/storage/tunnel");
    }
}

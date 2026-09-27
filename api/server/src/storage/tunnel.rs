use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::HeaderMap,
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use relay_common::storage::{
    encode, Frame, Hello, NodeStats, Request, Response as NodeResponse, KIND_DATA, KIND_END,
    KIND_HELLO, KIND_REQUEST, KIND_RESPONSE,
};
use tokio::sync::{mpsc, oneshot};

use crate::{error::AppError, routes::now_ms, state::AppState};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

enum Waiter {
    Reply(oneshot::Sender<NodeResponse>),
    Stream(mpsc::Sender<Result<Bytes, String>>),
}

struct Conn {
    tx: mpsc::Sender<Message>,
    waiters: Mutex<HashMap<u64, Waiter>>,
    closed: AtomicBool,
}

impl Conn {
    fn fail_all(&self) {
        self.closed.store(true, Ordering::Relaxed);
        for (_, w) in self.waiters.lock().unwrap().drain() {
            match w {
                Waiter::Reply(tx) => {
                    let _ = tx.send(NodeResponse::Error { message: "connessione chiusa".into() });
                }
                Waiter::Stream(tx) => {
                    let _ = tx.try_send(Err("connessione chiusa".into()));
                }
            }
        }
    }
}

pub struct NodeLink {
    conns: Mutex<Vec<Arc<Conn>>>,
    turn: AtomicUsize,
    pub stats: Mutex<NodeStats>,
    pub version: Mutex<String>,
    pub since: AtomicU64,
    pub up_rate: AtomicU64,
    pub down_rate: AtomicU64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl NodeLink {
    fn new() -> Self {
        Self {
            conns: Mutex::new(Vec::new()),
            turn: AtomicUsize::new(0),
            stats: Mutex::new(NodeStats::default()),
            version: Mutex::new(String::new()),
            since: AtomicU64::new(now_ms()),
            up_rate: AtomicU64::new(0),
            down_rate: AtomicU64::new(0),
        }
    }

    pub fn connections(&self) -> usize {
        self.conns.lock().unwrap().iter().filter(|c| !c.closed.load(Ordering::Relaxed)).count()
    }

    pub fn online(&self) -> bool {
        self.connections() > 0
    }

    fn pick(&self) -> Option<Arc<Conn>> {
        let conns = self.conns.lock().unwrap();
        let open: Vec<&Arc<Conn>> = conns.iter().filter(|c| !c.closed.load(Ordering::Relaxed)).collect();
        if open.is_empty() {
            return None;
        }
        let i = self.turn.fetch_add(1, Ordering::Relaxed) % open.len();
        Some(open[i].clone())
    }

    pub async fn request(&self, req: &Request, payload: &[u8]) -> Result<NodeResponse, String> {
        let conn = self.pick().ok_or("server di archivio offline")?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        conn.waiters.lock().unwrap().insert(id, Waiter::Reply(tx));
        let msg = Message::Binary(encode(KIND_REQUEST, id, req, payload).into());
        if conn.tx.send(msg).await.is_err() {
            conn.waiters.lock().unwrap().remove(&id);
            return Err("connessione chiusa".into());
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(NodeResponse::Error { message })) => Err(message),
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Err("connessione chiusa".into()),
            Err(_) => {
                conn.waiters.lock().unwrap().remove(&id);
                Err("il server di archivio non risponde".into())
            }
        }
    }

    pub async fn get(&self, blob: &str, offset: u64, len: u64) -> Result<mpsc::Receiver<Result<Bytes, String>>, String> {
        let conn = self.pick().ok_or("server di archivio offline")?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(32);
        conn.waiters.lock().unwrap().insert(id, Waiter::Stream(tx));
        let req = Request::Get { blob: blob.to_string(), offset, len };
        let msg = Message::Binary(encode(KIND_REQUEST, id, &req, &[]).into());
        if conn.tx.send(msg).await.is_err() {
            conn.waiters.lock().unwrap().remove(&id);
            return Err("connessione chiusa".into());
        }
        Ok(rx)
    }
}

#[derive(Default)]
pub struct Links(Mutex<HashMap<String, Arc<NodeLink>>>);

impl Links {
    pub fn get(&self, node: &str) -> Option<Arc<NodeLink>> {
        self.0.lock().unwrap().get(node).cloned().filter(|l| l.online())
    }

    pub fn any(&self, node: &str) -> Option<Arc<NodeLink>> {
        self.0.lock().unwrap().get(node).cloned()
    }

    fn entry(&self, node: &str) -> Arc<NodeLink> {
        self.0.lock().unwrap().entry(node.to_string()).or_insert_with(|| Arc::new(NodeLink::new())).clone()
    }
}

pub async fn tunnel(State(st): State<Arc<AppState>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Result<Response, AppError> {
    let key = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;
    let storage = st.storage.clone();
    let node = storage.node_for_key(key.trim()).ok_or(AppError::Unauthorized)?;
    Ok(ws.max_message_size(64 << 20).on_upgrade(move |socket| run(storage, node, socket)))
}

async fn run(storage: Arc<super::Storage>, node: String, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(64);
    let conn = Arc::new(Conn { tx, waiters: Mutex::new(HashMap::new()), closed: AtomicBool::new(false) });
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let hello = match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        Ok(Some(Ok(Message::Binary(b)))) => Frame::parse(&b).filter(|f| f.kind == KIND_HELLO).and_then(|f| f.header::<Hello>()),
        _ => None,
    };
    let Some(hello) = hello else {
        tracing::warn!("archivio {node}: collegamento senza saluto, chiuso");
        writer.abort();
        return;
    };
    let link = storage.links.entry(&node);
    if !link.online() {
        link.since.store(now_ms(), Ordering::Relaxed);
        tracing::info!("archivio {node} collegato (versione {})", hello.version);
    }
    *link.stats.lock().unwrap() = hello.stats.clone();
    *link.version.lock().unwrap() = hello.version.clone();
    link.conns.lock().unwrap().push(conn.clone());
    storage.wake();

    while let Some(Ok(msg)) = stream.next().await {
        let bytes = match msg {
            Message::Binary(b) => b,
            Message::Close(_) => break,
            _ => continue,
        };
        let Some(f) = Frame::parse(&bytes) else { continue };
        match f.kind {
            KIND_RESPONSE => {
                let resp = f.header::<NodeResponse>().unwrap_or(NodeResponse::Error { message: "risposta non valida".into() });
                if let NodeResponse::Stats(s) = &resp {
                    *link.stats.lock().unwrap() = s.clone();
                }
                match conn.waiters.lock().unwrap().remove(&f.id) {
                    Some(Waiter::Reply(tx)) => {
                        let _ = tx.send(resp);
                    }
                    Some(Waiter::Stream(tx)) => {
                        let msg = match resp {
                            NodeResponse::Error { message } => message,
                            _ => "risposta inattesa".into(),
                        };
                        let _ = tx.try_send(Err(msg));
                    }
                    None => {}
                }
            }
            KIND_DATA => {
                let tx = match conn.waiters.lock().unwrap().get(&f.id) {
                    Some(Waiter::Stream(tx)) => Some(tx.clone()),
                    _ => None,
                };
                if let Some(tx) = tx {
                    if tx.send(Ok(Bytes::copy_from_slice(f.payload))).await.is_err() {
                        conn.waiters.lock().unwrap().remove(&f.id);
                    }
                }
            }
            KIND_END => {
                conn.waiters.lock().unwrap().remove(&f.id);
            }
            _ => {}
        }
    }
    conn.fail_all();
    writer.abort();
    link.conns.lock().unwrap().retain(|c| !Arc::ptr_eq(c, &conn));
    if !link.online() {
        tracing::info!("archivio {node} scollegato");
    }
}

pub async fn refresh_stats(storage: Arc<super::Storage>) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let links: Vec<Arc<NodeLink>> = storage.links.0.lock().unwrap().values().cloned().collect();
        for link in links.into_iter().filter(|l| l.online()) {
            let _ = link.request(&Request::Stat, &[]).await;
        }
    }
}

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use relay_common::{
    not_ready_reason, redact_health, MatchInfo, PlayerHealth, PlayerStatus, ServerMsg,
};
use tokio::sync::{mpsc::UnboundedSender, RwLock};
use uuid::Uuid;

use crate::auth::Authenticator;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Processing {
    pub stage: &'static str,

    pub pct: u8,
}

type Conns = HashMap<String, (u64, UnboundedSender<ServerMsg>)>;
type Statuses = HashMap<String, (PlayerStatus, Instant)>;

pub struct AppState {
    pub data_dir: PathBuf,
    pub auth: Authenticator,
    pub matches: RwLock<HashMap<Uuid, MatchInfo>>,

    hubs: Mutex<HashMap<Uuid, Conns>>,

    statuses: Mutex<HashMap<Uuid, Statuses>>,
    next_conn: AtomicU64,

    processing: Mutex<HashMap<(Uuid, String), Processing>>,

    pub ffmpeg: Option<PathBuf>,

    pub finalize_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    pub async fn new(
        data_dir: PathBuf,
        dev_tokens: HashMap<String, String>,
    ) -> std::io::Result<Arc<Self>> {
        Self::with_auth(data_dir, Authenticator::dev(dev_tokens)).await
    }

    pub async fn with_auth(data_dir: PathBuf, auth: Authenticator) -> std::io::Result<Arc<Self>> {
        let ffmpeg = std::env::var("RELAY_FFMPEG")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        Self::with_ffmpeg(data_dir, auth, ffmpeg.map(PathBuf::from)).await
    }

    pub async fn with_ffmpeg(
        data_dir: PathBuf,
        auth: Authenticator,
        ffmpeg: Option<PathBuf>,
    ) -> std::io::Result<Arc<Self>> {
        let mut matches = HashMap::new();
        let root = data_dir.join("matches");
        tokio::fs::create_dir_all(&root).await?;
        let mut rd = tokio::fs::read_dir(&root).await?;
        while let Some(e) = rd.next_entry().await? {
            if let Ok(bytes) = tokio::fs::read(e.path().join("meta.json")).await {
                if let Ok(m) = serde_json::from_slice::<MatchInfo>(&bytes) {
                    matches.insert(m.id, m);
                }
            }
        }

        let ffmpeg = ffmpeg.map(|p| {
            if p.components().count() > 1 {
                std::fs::canonicalize(&p).unwrap_or(p)
            } else {
                p
            }
        });
        Ok(Arc::new(Self {
            data_dir,
            auth,
            matches: RwLock::new(matches),
            hubs: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            processing: Mutex::new(HashMap::new()),
            ffmpeg,
            finalize_lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn set_processing(&self, id: Uuid, player: &str, stage: &'static str, pct: u8) {
        self.processing.lock().unwrap().insert(
            (id, player.to_string()),
            Processing {
                stage,
                pct: pct.min(100),
            },
        );
    }

    pub fn clear_processing(&self, id: Uuid, player: &str) {
        self.processing
            .lock()
            .unwrap()
            .remove(&(id, player.to_string()));
    }

    pub fn processing_of(&self, id: Uuid) -> HashMap<String, Processing> {
        self.processing
            .lock()
            .unwrap()
            .iter()
            .filter(|((m, _), _)| *m == id)
            .map(|((_, p), v)| (p.clone(), v.clone()))
            .collect()
    }

    pub fn match_dir(&self, id: Uuid) -> PathBuf {
        self.data_dir.join("matches").join(id.to_string())
    }

    pub fn player_dir(&self, id: Uuid, player: &str) -> PathBuf {
        self.match_dir(id).join("players").join(player)
    }

    pub async fn persist(&self, m: &MatchInfo) -> std::io::Result<()> {
        let dir = self.match_dir(m.id);
        tokio::fs::create_dir_all(&dir).await?;
        let tmp = dir.join(format!(".meta.{}.tmp", Uuid::new_v4()));
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(m)?).await?;
        tokio::fs::rename(&tmp, dir.join("meta.json")).await
    }

    pub fn hub_register(&self, id: Uuid, user: &str, tx: UnboundedSender<ServerMsg>) -> u64 {
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        self.hubs
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .insert(user.to_string(), (conn, tx));
        conn
    }

    pub fn hub_unregister(&self, id: Uuid, user: &str, conn: u64) {
        let mut hubs = self.hubs.lock().unwrap();
        if let Some(h) = hubs.get_mut(&id) {
            if h.get(user).map(|(c, _)| *c) == Some(conn) {
                h.remove(user);
            }
            if h.is_empty() {
                hubs.remove(&id);
            }
        }
    }

    pub fn hub_connected(&self, id: Uuid) -> Vec<String> {
        let mut v: Vec<String> = self
            .hubs
            .lock()
            .unwrap()
            .get(&id)
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    pub fn hub_broadcast(&self, id: Uuid, msg: &ServerMsg) {
        if let Some(h) = self.hubs.lock().unwrap().get(&id) {
            for (_, tx) in h.values() {
                let _ = tx.send(msg.clone());
            }
        }
    }

    pub fn status_set(&self, id: Uuid, user: &str, status: PlayerStatus) {
        self.statuses
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .insert(user.to_string(), (status, Instant::now()));
    }

    pub fn health(&self, id: Uuid, players: &[String]) -> Vec<PlayerHealth> {
        let connected = self.hub_connected(id);
        let statuses = self.statuses.lock().unwrap();
        let of = statuses.get(&id);
        players
            .iter()
            .map(|p| {
                let is_conn = connected.contains(p);
                let (status, age) = match of.and_then(|m| m.get(p)) {
                    Some((s, at)) => (Some(s.clone()), Some(at.elapsed().as_millis() as u64)),
                    None => (None, None),
                };
                let issue = not_ready_reason(is_conn, status.as_ref(), age);
                PlayerHealth {
                    id: p.clone(),
                    connected: is_conn,
                    ready: issue.is_none(),
                    status,
                    age_ms: age,
                    issue,
                }
            })
            .collect()
    }

    pub fn broadcast_health(&self, id: Uuid, players: &[String], coordinator: &str) {
        let full = self.health(id, players);
        if let Some(h) = self.hubs.lock().unwrap().get(&id) {
            for (user, (_, tx)) in h.iter() {
                let msg = ServerMsg::Health {
                    players: redact_health(full.clone(), user == coordinator),
                };
                let _ = tx.send(msg);
            }
        }
    }
}

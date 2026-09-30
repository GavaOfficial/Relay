pub mod crypto;
pub mod routes;
pub mod tunnel;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use relay_common::storage::{cipher_len, Request, Response as NodeResponse, PLAIN_CHUNK};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, sync::Notify};

use crate::{error::AppError, state::AppState};
use crypto::{chunks, cipher_chunk_len, cipher_offset, BlobCipher};

pub const GB: u64 = 1_000_000_000;
const NODE_MARGIN: u64 = 5 * GB;
const CLIP_AGE: Duration = Duration::from_secs(3 * 60);
const MATCH_AGE: Duration = Duration::from_secs(15 * 60);
const PARALLEL_CHUNKS: usize = 16;
const READ_AHEAD: usize = 8;
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCfg {
    pub id: String,
    pub name: String,
    key_sha256: String,
    #[serde(default)]
    pub limit: Option<u64>,
    pub created_at: u64,
    #[serde(default)]
    pub draining: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    nodes: Vec<NodeCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub node: String,
    pub blob: String,
    pub size: u64,
    pub sha256: String,
    pub stored_at: u64,
    #[serde(default)]
    pub last_access: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    files: BTreeMap<String, Entry>,
    #[serde(default)]
    deletes: Vec<(String, String)>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Activity {
    pub queued: u64,
    pub queued_bytes: u64,
    pub current: Option<String>,
    pub last_error: Option<String>,
    pub local_only_bytes: u64,
}

pub struct Storage {
    data_dir: PathBuf,
    dir: PathBuf,
    master: [u8; 32],
    config: Mutex<Config>,
    index: Mutex<Index>,
    pub links: tunnel::Links,
    wake: Notify,
    rush: std::sync::atomic::AtomicBool,
    pub activity: Mutex<Activity>,
}

static GLOBAL: OnceLock<Arc<Storage>> = OnceLock::new();

pub fn global() -> Option<&'static Arc<Storage>> {
    GLOBAL.get()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    let tmp = path.with_extension("json.tmp");
    let ok = serde_json::to_vec_pretty(value)
        .ok()
        .map(|b| std::fs::write(&tmp, b).and_then(|_| std::fs::rename(&tmp, path)));
    if !matches!(ok, Some(Ok(()))) {
        tracing::error!("archivio: non riesco a salvare {}", path.display());
    }
}

pub fn hash_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

async fn fetch_chunk(link: &tunnel::NodeLink, e: &Entry, index: u64) -> Result<Vec<u8>, String> {
    let need = cipher_chunk_len(e.size, index) as usize;
    let mut rx = link.get(&e.blob, cipher_offset(index), need as u64).await?;
    let mut buf = Vec::with_capacity(need);
    let read = async {
        while buf.len() < need {
            match rx.recv().await {
                Some(Ok(b)) => buf.extend_from_slice(&b),
                Some(Err(m)) => return Err(m),
                None => return Err("trasferimento interrotto".to_string()),
            }
        }
        Ok(())
    };
    tokio::time::timeout(CHUNK_TIMEOUT, read)
        .await
        .map_err(|_| "il server di archivio non risponde".to_string())??;
    buf.truncate(need);
    Ok(buf)
}

impl Storage {
    pub fn open(data_dir: &Path) -> std::io::Result<Arc<Self>> {
        let dir = data_dir.join("storage");
        std::fs::create_dir_all(&dir)?;
        let key_path = dir.join("master.key");
        let master = match std::fs::read(&key_path) {
            Ok(b) if b.len() == 32 => b.try_into().unwrap(),
            _ => {
                let mut k = [0u8; 32];
                getrandom::getrandom(&mut k).map_err(|e| std::io::Error::other(e.to_string()))?;
                std::fs::write(&key_path, k)?;
                tracing::info!("archivio: creata la chiave di cifratura dei video");
                k
            }
        };
        let s = Arc::new(Self {
            data_dir: data_dir.to_path_buf(),
            config: Mutex::new(read_json(&dir.join("config.json"))),
            index: Mutex::new(read_json(&dir.join("index.json"))),
            dir,
            master,
            links: tunnel::Links::default(),
            wake: Notify::new(),
            rush: Default::default(),
            activity: Mutex::new(Activity::default()),
        });
        let _ = GLOBAL.set(s.clone());
        Ok(s)
    }

    fn save_config(&self) {
        write_json(&self.dir.join("config.json"), &*self.config.lock().unwrap());
    }

    fn save_index(&self) {
        write_json(&self.dir.join("index.json"), &*self.index.lock().unwrap());
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub fn rush(&self) {
        self.rush.store(true, std::sync::atomic::Ordering::Relaxed);
        self.wake();
    }

    pub fn has_nodes(&self) -> bool {
        !self.config.lock().unwrap().nodes.is_empty()
    }

    pub(crate) fn pick_for(&self, size: u64) -> Option<(String, Arc<tunnel::NodeLink>)> {
        self.pick_node(size, None)
    }

    pub(crate) fn cipher(&self, blob: &str) -> BlobCipher {
        BlobCipher::new(&self.master, blob)
    }

    pub(crate) fn adopt(
        &self,
        path: &Path,
        node: &str,
        blob: &str,
        size: u64,
        sha256: &str,
    ) -> bool {
        let Some(rel) = self.rel(path) else {
            return false;
        };
        let old = self.index.lock().unwrap().files.insert(
            rel,
            Entry {
                node: node.to_string(),
                blob: blob.to_string(),
                size,
                sha256: sha256.to_string(),
                stored_at: now_secs(),
                last_access: now_secs(),
            },
        );
        if let Some(old) = old {
            self.index
                .lock()
                .unwrap()
                .deletes
                .push((old.node, old.blob));
        }
        self.save_index();
        true
    }

    pub fn nodes(&self) -> Vec<NodeCfg> {
        self.config.lock().unwrap().nodes.clone()
    }

    pub fn add_node(&self, name: &str) -> (NodeCfg, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let mut secret = [0u8; 32];
        let _ = getrandom::getrandom(&mut secret);
        let key = format!("rsk_{}", hex::encode(secret));
        let cfg = NodeCfg {
            id,
            name: name.to_string(),
            key_sha256: hash_key(&key),
            limit: None,
            created_at: now_secs(),
            draining: false,
        };
        self.config.lock().unwrap().nodes.push(cfg.clone());
        self.save_config();
        (cfg, key)
    }

    pub fn update_node(
        &self,
        id: &str,
        name: Option<&str>,
        limit: Option<Option<u64>>,
        draining: Option<bool>,
    ) -> bool {
        let mut c = self.config.lock().unwrap();
        let Some(n) = c.nodes.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        if let Some(name) = name {
            n.name = name.to_string();
        }
        if let Some(limit) = limit {
            n.limit = limit;
        }
        if let Some(draining) = draining {
            n.draining = draining;
        }
        drop(c);
        self.save_config();
        self.wake();
        true
    }

    pub fn remove_node(&self, id: &str) -> Result<(), AppError> {
        if self.used_by_node().get(id).is_some_and(|u| u.1 > 0) {
            return Err(AppError::BadRequest(
                "prima svuota il server: ci sono ancora video",
            ));
        }
        let mut c = self.config.lock().unwrap();
        let before = c.nodes.len();
        c.nodes.retain(|n| n.id != id);
        if c.nodes.len() == before {
            return Err(AppError::NotFound);
        }
        drop(c);
        self.save_config();
        self.index.lock().unwrap().deletes.retain(|d| d.0 != id);
        self.save_index();
        Ok(())
    }

    pub fn rotate_key(&self, id: &str) -> Option<String> {
        let mut secret = [0u8; 32];
        let _ = getrandom::getrandom(&mut secret);
        let key = format!("rsk_{}", hex::encode(secret));
        let mut c = self.config.lock().unwrap();
        c.nodes.iter_mut().find(|n| n.id == id)?.key_sha256 = hash_key(&key);
        drop(c);
        self.save_config();
        Some(key)
    }

    pub fn node_for_key(&self, key: &str) -> Option<String> {
        let h = hash_key(key);
        self.config
            .lock()
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.key_sha256 == h)
            .map(|n| n.id.clone())
    }

    pub fn used_by_node(&self) -> BTreeMap<String, (u64, u64)> {
        let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for e in self.index.lock().unwrap().files.values() {
            let v = out.entry(e.node.clone()).or_default();
            v.0 += cipher_len(e.size);
            v.1 += 1;
        }
        out
    }

    fn pick_node(
        &self,
        size: u64,
        except: Option<&str>,
    ) -> Option<(String, Arc<tunnel::NodeLink>)> {
        let need = cipher_len(size);
        let used = self.used_by_node();
        let mut best: Option<(u64, String, Arc<tunnel::NodeLink>)> = None;
        for n in self.nodes() {
            if n.draining || except == Some(n.id.as_str()) {
                continue;
            }
            let Some(link) = self.links.get(&n.id) else {
                continue;
            };
            let stats = link.stats.lock().unwrap().clone();
            let by_disk = stats.free.saturating_sub(NODE_MARGIN);
            let by_limit = n
                .limit
                .map(|l| l.saturating_sub(used.get(&n.id).map(|u| u.0).unwrap_or(0)))
                .unwrap_or(u64::MAX);
            let room = by_disk.min(by_limit);
            if room > need && best.as_ref().is_none_or(|b| room > b.0) {
                best = Some((room, n.id.clone(), link));
            }
        }
        best.map(|(_, id, link)| (id, link))
    }

    fn rel(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.data_dir).ok()?;
        Some(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    pub fn entry(&self, path: &Path) -> Option<Entry> {
        let rel = self.rel(path)?;
        self.index.lock().unwrap().files.get(&rel).cloned()
    }

    pub fn present(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.len() > 0) || self.entry(path).is_some()
    }

    fn touch(&self, path: &Path) {
        if let Some(rel) = self.rel(path) {
            if let Some(e) = self.index.lock().unwrap().files.get_mut(&rel) {
                e.last_access = now_secs();
            }
        }
    }

    pub fn forget_dir(&self, dir: &Path) {
        let Some(prefix) = self.rel(dir) else { return };
        let prefix = format!("{prefix}/");
        let mut idx = self.index.lock().unwrap();
        let gone: Vec<String> = idx
            .files
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        if gone.is_empty() {
            return;
        }
        for k in gone {
            if let Some(e) = idx.files.remove(&k) {
                idx.deletes.push((e.node, e.blob));
            }
        }
        drop(idx);
        self.save_index();
        self.wake();
    }

    pub async fn serve(
        self: &Arc<Self>,
        path: &Path,
        entry: Entry,
        headers: &HeaderMap,
        name: Option<String>,
    ) -> Result<Response, AppError> {
        let total = entry.size;
        if total == 0 {
            return Err(AppError::NotFound);
        }
        let etag = format!("\"{}\"", entry.blob);
        if headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            == Some(etag.as_str())
        {
            return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
        }
        let link = self.links.get(&entry.node).ok_or(AppError::Unavailable(
            "video non disponibile: il server di archivio e' offline",
        ))?;
        let range = headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| crate::routes::parse_range(v, total));
        let (status, start, end) = match range {
            Some(Ok((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
            Some(Err(())) => {
                return Ok((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{total}"))],
                )
                    .into_response())
            }
            None => (StatusCode::OK, 0, total - 1),
        };
        self.touch(path);
        let body = self
            .clone()
            .read_range(link, entry.clone(), start, end)
            .await?;
        let len = end - start + 1;
        let mut resp = (status, axum::body::Body::from_stream(body)).into_response();
        let h = resp.headers_mut();
        h.insert(header::CONTENT_TYPE, "video/mp4".parse().unwrap());
        h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
        h.insert(header::CONTENT_LENGTH, len.into());
        h.insert(
            header::CACHE_CONTROL,
            "private, max-age=31536000, immutable".parse().unwrap(),
        );
        h.insert(header::ETAG, etag.parse().unwrap());
        if status == StatusCode::PARTIAL_CONTENT {
            h.insert(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}").parse().unwrap(),
            );
        }
        if let Some(name) = name {
            h.insert(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\"").parse().unwrap(),
            );
        }
        Ok(resp)
    }

    async fn read_range(
        self: Arc<Self>,
        link: Arc<tunnel::NodeLink>,
        e: Entry,
        start: u64,
        end: u64,
    ) -> Result<impl futures_util::Stream<Item = Result<Bytes, std::io::Error>>, AppError> {
        use futures_util::StreamExt;
        let first = start / PLAIN_CHUNK;
        let last = end / PLAIN_CHUNK;
        let cipher = Arc::new(BlobCipher::new(&self.master, &e.blob));
        let first_piece = fetch_chunk(&link, &e, first).await.map_err(|_| {
            AppError::Unavailable("video non disponibile: il server di archivio e' offline")
        })?;
        let began = Instant::now();
        let storage = self.clone();
        let node = e.node.clone();
        let rest = futures_util::stream::iter(first + 1..=last)
            .map(move |i| {
                let (link, e) = (link.clone(), e.clone());
                async move { (i, fetch_chunk(&link, &e, i).await) }
            })
            .buffered(READ_AHEAD);
        let pieces =
            futures_util::stream::once(async move { (first, Ok(first_piece)) }).chain(rest);
        Ok(pieces.map(move |(i, piece)| {
            let piece = piece.map_err(std::io::Error::other)?;
            let plain = cipher
                .open(i, &piece)
                .ok_or_else(|| std::io::Error::other("pezzo del video danneggiato"))?;
            let base = i * PLAIN_CHUNK;
            let from = start.saturating_sub(base) as usize;
            let to = ((end - base) as usize + 1).min(plain.len());
            if i == last {
                let secs = began.elapsed().as_secs_f64().max(0.001);
                storage.note_down((end - start + 1) as f64 / secs, &node);
            }
            Ok(Bytes::copy_from_slice(&plain[from.min(to)..to]))
        }))
    }

    fn note_down(&self, rate: f64, node: &str) {
        if let Some(l) = self.links.any(node) {
            l.down_rate
                .store(rate as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }

    async fn offload(&self, path: &Path, rel: &str) -> Result<(), String> {
        let meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
        let size = meta.len();
        let (node, link) = self
            .pick_node(size, None)
            .ok_or("nessun server di archivio con spazio collegato")?;
        let blob = uuid::Uuid::new_v4().to_string();
        let cipher = BlobCipher::new(&self.master, &blob);
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL_CHUNKS));
        let mut tasks = tokio::task::JoinSet::new();
        let began = Instant::now();
        let mut failed: Option<String> = None;
        for i in 0..chunks(size) {
            let want = size.saturating_sub(i * PLAIN_CHUNK).min(PLAIN_CHUNK) as usize;
            let mut buf = vec![0u8; want];
            if let Err(e) = file.read_exact(&mut buf).await {
                failed = Some(e.to_string());
                break;
            }
            let sealed = cipher.seal(i, &buf);
            hasher.update(&sealed);
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| e.to_string())?;
            let (link, blob) = (link.clone(), blob.clone());
            tasks.spawn(async move {
                let r = link
                    .request(
                        &Request::Put {
                            blob,
                            offset: cipher_offset(i),
                        },
                        &sealed,
                    )
                    .await;
                drop(permit);
                r
            });
            while let Some(r) = tasks.try_join_next() {
                if let Err(e) = r.map_err(|e| e.to_string()).and_then(|r| r.map(|_| ())) {
                    failed.get_or_insert(e);
                }
            }
            if failed.is_some() {
                break;
            }
        }
        while let Some(r) = tasks.join_next().await {
            if let Err(e) = r.map_err(|e| e.to_string()).and_then(|r| r.map(|_| ())) {
                failed.get_or_insert(e);
            }
        }
        let same = tokio::fs::metadata(path)
            .await
            .is_ok_and(|m| m.len() == size && m.modified().ok() == meta.modified().ok());
        if failed.is_none() && !same {
            failed = Some("il file e' cambiato durante il trasferimento".into());
        }
        if let Some(e) = failed {
            let _ = link.request(&Request::Abort { blob }, &[]).await;
            return Err(e);
        }
        let sha256 = hex::encode(hasher.finalize());
        match link
            .request(
                &Request::Commit {
                    blob: blob.clone(),
                    size: cipher_len(size),
                    sha256: sha256.clone(),
                },
                &[],
            )
            .await
        {
            Ok(NodeResponse::Ok) => {}
            Ok(other) => return Err(format!("risposta inattesa: {other:?}")),
            Err(e) => return Err(e),
        }
        let secs = began.elapsed().as_secs_f64().max(0.001);
        link.up_rate.store(
            (size as f64 / secs) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.index.lock().unwrap().files.insert(
            rel.to_string(),
            Entry {
                node,
                blob,
                size,
                sha256,
                stored_at: now_secs(),
                last_access: now_secs(),
            },
        );
        self.save_index();
        Ok(())
    }

    fn candidates(&self, st: &AppState, rush: bool) -> Vec<(PathBuf, u64, SystemTime)> {
        let age = |min: Duration, m: &std::fs::Metadata| {
            rush || m
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|e| e >= min)
        };
        let mut out = Vec::new();
        let mut add = |p: PathBuf, min: Duration| {
            if let Ok(m) = std::fs::metadata(&p) {
                if m.is_file() && m.len() > 0 && age(min, &m) {
                    out.push((p, m.len(), m.modified().unwrap_or(UNIX_EPOCH)));
                }
            }
        };
        for ext in std::fs::read_dir(&self.data_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = ext.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), "matches" | "storage" | "app") || !ext.path().is_dir() {
                continue;
            }
            for user in std::fs::read_dir(ext.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                for f in std::fs::read_dir(user.path().join("clips"))
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let n = f.file_name().to_string_lossy().into_owned();
                    if n.ends_with(".mp4") && !n.starts_with('.') {
                        add(f.path(), CLIP_AGE);
                    }
                }
            }
        }
        let ended: Vec<uuid::Uuid> = st
            .matches
            .try_read()
            .map(|m| {
                m.values()
                    .filter(|m| m.status == relay_common::MatchStatus::Ended)
                    .map(|m| m.id)
                    .collect()
            })
            .unwrap_or_default();
        for id in ended {
            if !st.processing_of(id).is_empty() {
                continue;
            }
            for p in std::fs::read_dir(st.match_dir(id))
                .into_iter()
                .flatten()
                .flatten()
            {
                let dir = p.path();
                if !dir.is_dir() {
                    continue;
                }
                let web = dir.join(crate::finalize::WEB_FILE);
                let vod = dir.join(crate::finalize::VOD_DIR);
                let vod_ready = vod.join(crate::finalize::VOD_INDEX).exists() || !web.exists();
                if !vod_ready {
                    continue;
                }
                add(dir.join(crate::finalize::VIDEO_FILE), MATCH_AGE);
                add(web, MATCH_AGE);
                add(vod.join(crate::finalize::VOD_MEDIA), MATCH_AGE);
            }
        }
        out
    }

    async fn drop_local_copies(&self) {
        let archived: Vec<String> = self.index.lock().unwrap().files.keys().cloned().collect();
        let mut freed = 0u64;
        for rel in archived {
            let path = self.data_dir.join(&rel);
            let Ok(meta) = tokio::fs::metadata(&path).await else {
                continue;
            };
            if tokio::fs::remove_file(&path).await.is_ok() {
                freed += meta.len();
            }
        }
        if freed > 0 {
            tracing::info!(
                "archivio: tolte le copie locali gia' archiviate ({} MB)",
                freed / 1_000_000
            );
        }
    }

    async fn move_entry(&self, rel: &str, e: &Entry) -> Result<String, String> {
        let from = self
            .links
            .get(&e.node)
            .ok_or("server di partenza offline")?;
        let (to, link) = self
            .pick_node(e.size, Some(&e.node))
            .ok_or("nessun altro server con spazio collegato")?;
        let mut rx = from.get(&e.blob, 0, cipher_len(e.size)).await?;
        let mut hasher = Sha256::new();
        let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL_CHUNKS));
        let mut tasks = tokio::task::JoinSet::new();
        let began = Instant::now();
        let mut buf: Vec<u8> = Vec::new();
        let mut failed: Option<String> = None;
        'outer: for i in 0..chunks(e.size) {
            let need = cipher_chunk_len(e.size, i) as usize;
            while buf.len() < need {
                match rx.recv().await {
                    Some(Ok(b)) => buf.extend_from_slice(&b),
                    Some(Err(m)) => {
                        failed = Some(m);
                        break 'outer;
                    }
                    None => {
                        failed = Some("trasferimento interrotto".into());
                        break 'outer;
                    }
                }
            }
            let piece: Vec<u8> = buf.drain(..need).collect();
            hasher.update(&piece);
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| e.to_string())?;
            let (link, blob) = (link.clone(), e.blob.clone());
            tasks.spawn(async move {
                let r = link
                    .request(
                        &Request::Put {
                            blob,
                            offset: cipher_offset(i),
                        },
                        &piece,
                    )
                    .await;
                drop(permit);
                r
            });
            while let Some(r) = tasks.try_join_next() {
                if let Err(err) = r.map_err(|e| e.to_string()).and_then(|r| r.map(|_| ())) {
                    failed.get_or_insert(err);
                }
            }
            if failed.is_some() {
                break;
            }
        }
        drop(rx);
        while let Some(r) = tasks.join_next().await {
            if let Err(err) = r.map_err(|e| e.to_string()).and_then(|r| r.map(|_| ())) {
                failed.get_or_insert(err);
            }
        }
        if failed.is_none() && !hex::encode(hasher.finalize()).eq_ignore_ascii_case(&e.sha256) {
            failed = Some("la copia sul server di partenza e' danneggiata".into());
        }
        if let Some(err) = failed {
            let _ = link
                .request(
                    &Request::Abort {
                        blob: e.blob.clone(),
                    },
                    &[],
                )
                .await;
            return Err(err);
        }
        match link
            .request(
                &Request::Commit {
                    blob: e.blob.clone(),
                    size: cipher_len(e.size),
                    sha256: e.sha256.clone(),
                },
                &[],
            )
            .await
        {
            Ok(NodeResponse::Ok) => {}
            Ok(other) => return Err(format!("risposta inattesa: {other:?}")),
            Err(err) => return Err(err),
        }
        let secs = began.elapsed().as_secs_f64().max(0.001);
        link.up_rate.store(
            (e.size as f64 / secs) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut idx = self.index.lock().unwrap();
        match idx.files.get_mut(rel) {
            Some(cur) if cur.blob == e.blob && cur.node == e.node => {
                cur.node = to.clone();
                idx.deletes.push((e.node.clone(), e.blob.clone()));
            }
            _ => {
                idx.deletes.push((to.clone(), e.blob.clone()));
            }
        }
        drop(idx);
        self.save_index();
        Ok(to)
    }

    async fn drain(&self) {
        let nodes = self.nodes();
        let draining: Vec<&NodeCfg> = nodes.iter().filter(|n| n.draining).collect();
        if draining.is_empty() {
            return;
        }
        let name = |id: &str| {
            nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.name.clone())
                .unwrap_or_default()
        };
        let todo: Vec<(String, Entry)> = self
            .index
            .lock()
            .unwrap()
            .files
            .iter()
            .filter(|(_, e)| draining.iter().any(|n| n.id == e.node))
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        for (rel, e) in todo {
            if self.links.get(&e.node).is_none() {
                continue;
            }
            self.activity.lock().unwrap().current = Some(rel.clone());
            match self.move_entry(&rel, &e).await {
                Ok(to) => {
                    tracing::info!(
                        "archivio: {rel} spostato da {} a {} ({} MB)",
                        name(&e.node),
                        name(&to),
                        e.size / 1_000_000
                    );
                    self.activity.lock().unwrap().last_error = None;
                }
                Err(err) => {
                    tracing::warn!("archivio: {rel} non spostato da {}: {err}", name(&e.node));
                    self.activity.lock().unwrap().last_error = Some(format!("{rel}: {err}"));
                    break;
                }
            }
        }
        self.activity.lock().unwrap().current = None;
        self.run_deletes().await;
    }

    async fn run_deletes(&self) {
        let pending: Vec<(String, String)> = self.index.lock().unwrap().deletes.clone();
        if pending.is_empty() {
            return;
        }
        let mut done = Vec::new();
        for (node, blob) in pending {
            if let Some(link) = self.links.get(&node) {
                if link
                    .request(&Request::Delete { blob: blob.clone() }, &[])
                    .await
                    .is_ok()
                {
                    done.push((node, blob));
                }
            }
        }
        if !done.is_empty() {
            self.index
                .lock()
                .unwrap()
                .deletes
                .retain(|d| !done.contains(d));
            self.save_index();
        }
    }

    pub async fn run(self: Arc<Self>, st: Arc<AppState>) {
        tokio::spawn(tunnel::refresh_stats(self.clone()));
        loop {
            let _ = tokio::time::timeout(Duration::from_secs(60), self.wake.notified()).await;
            self.run_deletes().await;
            self.drain().await;
            let rush = self.rush.swap(false, std::sync::atomic::Ordering::Relaxed);
            let this = self.clone();
            let st2 = st.clone();
            let mut todo = tokio::task::spawn_blocking(move || this.candidates(&st2, rush))
                .await
                .unwrap_or_default();
            todo.sort_by_key(|c| c.2);
            let pending: Vec<(PathBuf, u64)> = todo
                .into_iter()
                .filter(|(p, _, _)| self.entry(p).is_none())
                .map(|(p, s, _)| (p, s))
                .collect();
            {
                let mut a = self.activity.lock().unwrap();
                a.queued = pending.len() as u64;
                a.queued_bytes = pending.iter().map(|p| p.1).sum();
                a.local_only_bytes = a.queued_bytes;
            }
            for (path, size) in pending {
                let Some(rel) = self.rel(&path) else { continue };
                if self.pick_node(size, None).is_none() {
                    break;
                }
                self.activity.lock().unwrap().current = Some(rel.clone());
                match self.offload(&path, &rel).await {
                    Ok(()) => {
                        tracing::info!("archivio: {rel} spostato ({} MB)", size / 1_000_000);
                        let mut a = self.activity.lock().unwrap();
                        a.queued = a.queued.saturating_sub(1);
                        a.queued_bytes = a.queued_bytes.saturating_sub(size);
                        a.local_only_bytes = a.queued_bytes;
                        a.last_error = None;
                    }
                    Err(e) => {
                        tracing::warn!("archivio: {rel} non spostato: {e}");
                        self.activity.lock().unwrap().last_error = Some(format!("{rel}: {e}"));
                        break;
                    }
                }
                self.activity.lock().unwrap().current = None;
                self.drop_local_copies().await;
            }
            self.activity.lock().unwrap().current = None;
            self.drop_local_copies().await;
            self.save_index();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stored_as_hashes_and_found_again() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::open(dir.path()).unwrap();
        let (node, key) = s.add_node("casa");
        assert!(key.starts_with("rsk_"));
        assert_eq!(s.node_for_key(&key).as_deref(), Some(node.id.as_str()));
        assert!(s.node_for_key("rsk_sbagliata").is_none());
        let raw = std::fs::read_to_string(dir.path().join("storage/config.json")).unwrap();
        assert!(!raw.contains(&key), "la chiave non si salva in chiaro");
        let key2 = s.rotate_key(&node.id).unwrap();
        assert!(s.node_for_key(&key).is_none());
        assert!(s.node_for_key(&key2).is_some());
    }

    #[test]
    fn a_node_is_removed_only_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::open(dir.path()).unwrap();
        let (node, _) = s.add_node("casa");
        s.index.lock().unwrap().files.insert(
            "matches/a/p/video.mp4".into(),
            Entry {
                node: node.id.clone(),
                blob: "1".into(),
                size: 1,
                sha256: String::new(),
                stored_at: 0,
                last_access: 0,
            },
        );
        assert!(s.remove_node(&node.id).is_err(), "ha ancora video");
        assert!(s.update_node(&node.id, Some("nas"), None, Some(true)));
        assert!(s.nodes()[0].draining);
        assert!(
            s.pick_node(1, None).is_none(),
            "un server da svuotare non riceve video nuovi"
        );
        s.forget_dir(&dir.path().join("matches/a"));
        s.remove_node(&node.id).unwrap();
        assert!(s.nodes().is_empty());
        assert!(s.index.lock().unwrap().deletes.is_empty());
    }

    #[test]
    fn forgetting_a_match_queues_the_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::open(dir.path()).unwrap();
        let e = |b: &str| Entry {
            node: "n".into(),
            blob: b.into(),
            size: 1,
            sha256: String::new(),
            stored_at: 0,
            last_access: 0,
        };
        {
            let mut i = s.index.lock().unwrap();
            i.files.insert("matches/a/p/video.mp4".into(), e("1"));
            i.files.insert("matches/ab/p/video.mp4".into(), e("2"));
        }
        assert!(s.present(&dir.path().join("matches/a/p/video.mp4")));
        s.forget_dir(&dir.path().join("matches/a"));
        assert!(!s.present(&dir.path().join("matches/a/p/video.mp4")));
        assert!(
            s.present(&dir.path().join("matches/ab/p/video.mp4")),
            "solo la partita cancellata"
        );
        assert_eq!(
            s.index.lock().unwrap().deletes,
            vec![("n".to_string(), "1".to_string())]
        );
    }
}

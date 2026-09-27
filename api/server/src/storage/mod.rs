pub mod crypto;
pub mod routes;
pub mod tunnel;

use std::{
    collections::{BTreeMap, HashSet},
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
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
};

use crate::{error::AppError, state::AppState};
use crypto::{chunks, cipher_chunk_len, cipher_offset, BlobCipher};

pub const GB: u64 = 1_000_000_000;
pub const DEFAULT_CACHE: u64 = 10 * GB;
const NODE_MARGIN: u64 = 5 * GB;
const CLIP_AGE: Duration = Duration::from_secs(3 * 60);
const MATCH_AGE: Duration = Duration::from_secs(15 * 60);
const PARALLEL_CHUNKS: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCfg {
    pub id: String,
    pub name: String,
    key_sha256: String,
    #[serde(default)]
    pub limit: Option<u64>,
    pub created_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    nodes: Vec<NodeCfg>,
    #[serde(default)]
    cache_limit: Option<u64>,
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
    pub cache_bytes: u64,
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
    filling: Mutex<HashSet<String>>,
}

static GLOBAL: OnceLock<Arc<Storage>> = OnceLock::new();

pub fn global() -> Option<&'static Arc<Storage>> {
    GLOBAL.get()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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
            filling: Mutex::new(HashSet::new()),
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


    pub fn nodes(&self) -> Vec<NodeCfg> {
        self.config.lock().unwrap().nodes.clone()
    }

    pub fn cache_limit(&self) -> u64 {
        self.config.lock().unwrap().cache_limit.unwrap_or(DEFAULT_CACHE)
    }

    pub fn set_cache_limit(&self, bytes: u64) {
        self.config.lock().unwrap().cache_limit = Some(bytes);
        self.save_config();
        self.wake();
    }

    pub fn add_node(&self, name: &str) -> (NodeCfg, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let mut secret = [0u8; 32];
        let _ = getrandom::getrandom(&mut secret);
        let key = format!("rsk_{}", hex::encode(secret));
        let cfg = NodeCfg { id, name: name.to_string(), key_sha256: hash_key(&key), limit: None, created_at: now_secs() };
        self.config.lock().unwrap().nodes.push(cfg.clone());
        self.save_config();
        (cfg, key)
    }

    pub fn update_node(&self, id: &str, name: Option<&str>, limit: Option<Option<u64>>) -> bool {
        let mut c = self.config.lock().unwrap();
        let Some(n) = c.nodes.iter_mut().find(|n| n.id == id) else { return false };
        if let Some(name) = name {
            n.name = name.to_string();
        }
        if let Some(limit) = limit {
            n.limit = limit;
        }
        drop(c);
        self.save_config();
        self.wake();
        true
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
        self.config.lock().unwrap().nodes.iter().find(|n| n.key_sha256 == h).map(|n| n.id.clone())
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

    fn pick_node(&self, size: u64) -> Option<(String, Arc<tunnel::NodeLink>)> {
        let need = cipher_len(size);
        let used = self.used_by_node();
        let mut best: Option<(u64, String, Arc<tunnel::NodeLink>)> = None;
        for n in self.nodes() {
            let Some(link) = self.links.get(&n.id) else { continue };
            let stats = link.stats.lock().unwrap().clone();
            let by_disk = stats.free.saturating_sub(NODE_MARGIN);
            let by_limit = n.limit.map(|l| l.saturating_sub(used.get(&n.id).map(|u| u.0).unwrap_or(0))).unwrap_or(u64::MAX);
            let room = by_disk.min(by_limit);
            if room > need && best.as_ref().is_none_or(|b| room > b.0) {
                best = Some((room, n.id.clone(), link));
            }
        }
        best.map(|(_, id, link)| (id, link))
    }


    fn rel(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.data_dir).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
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
        let gone: Vec<String> = idx.files.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
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


    pub async fn serve(self: &Arc<Self>, path: &Path, entry: Entry, headers: &HeaderMap, name: Option<String>) -> Result<Response, AppError> {
        let total = entry.size;
        if total == 0 {
            return Err(AppError::NotFound);
        }
        let etag = format!("\"{}\"", entry.blob);
        if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
            return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
        }
        let link = self.links.get(&entry.node).ok_or(AppError::Unavailable("video non disponibile: il server di archivio e' offline"))?;
        let range = headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| crate::routes::parse_range(v, total));
        let (status, start, end) = match range {
            Some(Ok((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
            Some(Err(())) => {
                return Ok((StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{total}"))]).into_response())
            }
            None => (StatusCode::OK, 0, total - 1),
        };
        self.touch(path);
        let fill = (start == 0 && end == total - 1 && self.filling.lock().unwrap().insert(entry.blob.clone())).then(|| path.to_path_buf());
        let body = self.clone().read_range(link, entry.clone(), start, end, fill).await?;
        let len = end - start + 1;
        let mut resp = (status, axum::body::Body::from_stream(body)).into_response();
        let h = resp.headers_mut();
        h.insert(header::CONTENT_TYPE, "video/mp4".parse().unwrap());
        h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
        h.insert(header::CONTENT_LENGTH, len.into());
        h.insert(header::CACHE_CONTROL, "private, max-age=31536000, immutable".parse().unwrap());
        h.insert(header::ETAG, etag.parse().unwrap());
        if status == StatusCode::PARTIAL_CONTENT {
            h.insert(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}").parse().unwrap());
        }
        if let Some(name) = name {
            h.insert(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"").parse().unwrap());
        }
        Ok(resp)
    }

    async fn read_range(
        self: Arc<Self>,
        link: Arc<tunnel::NodeLink>,
        e: Entry,
        start: u64,
        end: u64,
        fill: Option<PathBuf>,
    ) -> Result<impl futures_util::Stream<Item = Result<Bytes, std::io::Error>>, AppError> {
        let first = start / PLAIN_CHUNK;
        let last = end / PLAIN_CHUNK;
        let off = cipher_offset(first);
        let len = cipher_offset(last) + cipher_chunk_len(e.size, last) - off;
        let rx = link.get(&e.blob, off, len).await.map_err(|_| AppError::Unavailable("video non disponibile: il server di archivio e' offline"))?;
        let cache = match &fill {
            Some(p) => {
                let tmp = p.with_file_name(format!(".{}.fill", e.blob));
                tokio::fs::File::create(&tmp).await.ok().map(|f| (f, tmp, p.clone()))
            }
            None => None,
        };
        struct St {
            storage: Arc<Storage>,
            rx: tokio::sync::mpsc::Receiver<Result<Bytes, String>>,
            cipher: BlobCipher,
            buf: Vec<u8>,
            index: u64,
            e: Entry,
            start: u64,
            end: u64,
            last: u64,
            cache: Option<(tokio::fs::File, PathBuf, PathBuf)>,
            began: Instant,
            done: bool,
        }
        let st = St {
            cipher: BlobCipher::new(&self.master, &e.blob),
            storage: self,
            rx,
            buf: Vec::new(),
            index: first,
            e,
            start,
            end,
            last,
            cache,
            began: Instant::now(),
            done: false,
        };
        Ok(futures_util::stream::unfold(st, |mut st| async move {
            if st.done {
                return None;
            }
            loop {
                let need = cipher_chunk_len(st.e.size, st.index) as usize;
                if st.buf.len() >= need {
                    let chunk: Vec<u8> = st.buf.drain(..need).collect();
                    let Some(plain) = st.cipher.open(st.index, &chunk) else {
                        st.done = true;
                        st.storage.end_fill(&st.e, st.cache.take(), false).await;
                        return Some((Err(std::io::Error::other("pezzo del video danneggiato")), st));
                    };
                    if let Some((f, _, _)) = st.cache.as_mut() {
                        if f.write_all(&plain).await.is_err() {
                            st.cache = None;
                        }
                    }
                    let base = st.index * PLAIN_CHUNK;
                    let from = st.start.saturating_sub(base) as usize;
                    let to = ((st.end - base) as usize + 1).min(plain.len());
                    let out = Bytes::copy_from_slice(&plain[from.min(to)..to]);
                    if st.index == st.last {
                        st.done = true;
                        let secs = st.began.elapsed().as_secs_f64().max(0.001);
                        st.storage.note_down((st.end - st.start + 1) as f64 / secs, &st.e.node);
                        st.storage.end_fill(&st.e, st.cache.take(), true).await;
                    }
                    st.index += 1;
                    return Some((Ok(out), st));
                }
                match st.rx.recv().await {
                    Some(Ok(b)) => st.buf.extend_from_slice(&b),
                    Some(Err(m)) => {
                        st.done = true;
                        st.storage.end_fill(&st.e, st.cache.take(), false).await;
                        return Some((Err(std::io::Error::other(m)), st));
                    }
                    None => {
                        st.done = true;
                        st.storage.end_fill(&st.e, st.cache.take(), false).await;
                        return Some((Err(std::io::Error::other("trasferimento interrotto")), st));
                    }
                }
            }
        }))
    }

    async fn end_fill(&self, e: &Entry, cache: Option<(tokio::fs::File, PathBuf, PathBuf)>, ok: bool) {
        if let Some((mut f, tmp, dst)) = cache {
            let ok = ok && f.flush().await.is_ok() && tokio::fs::metadata(&tmp).await.is_ok_and(|m| m.len() == e.size);
            drop(f);
            if ok && tokio::fs::rename(&tmp, &dst).await.is_ok() {
                tracing::info!("archivio: {} di nuovo in cache", dst.display());
                self.wake();
            } else {
                let _ = tokio::fs::remove_file(&tmp).await;
            }
        }
        self.filling.lock().unwrap().remove(&e.blob);
    }

    fn note_down(&self, rate: f64, node: &str) {
        if let Some(l) = self.links.any(node) {
            l.down_rate.store(rate as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }


    async fn offload(&self, path: &Path, rel: &str) -> Result<(), String> {
        let meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
        let size = meta.len();
        let (node, link) = self.pick_node(size).ok_or("nessun server di archivio con spazio collegato")?;
        let blob = uuid::Uuid::new_v4().to_string();
        let cipher = BlobCipher::new(&self.master, &blob);
        let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
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
            let permit = sem.clone().acquire_owned().await.map_err(|e| e.to_string())?;
            let (link, blob) = (link.clone(), blob.clone());
            tasks.spawn(async move {
                let r = link.request(&Request::Put { blob, offset: cipher_offset(i) }, &sealed).await;
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
        let same = tokio::fs::metadata(path).await.is_ok_and(|m| m.len() == size && m.modified().ok() == meta.modified().ok());
        if failed.is_none() && !same {
            failed = Some("il file e' cambiato durante il trasferimento".into());
        }
        if let Some(e) = failed {
            let _ = link.request(&Request::Abort { blob }, &[]).await;
            return Err(e);
        }
        let sha256 = hex::encode(hasher.finalize());
        match link.request(&Request::Commit { blob: blob.clone(), size: cipher_len(size), sha256: sha256.clone() }, &[]).await {
            Ok(NodeResponse::Ok) => {}
            Ok(other) => return Err(format!("risposta inattesa: {other:?}")),
            Err(e) => return Err(e),
        }
        let secs = began.elapsed().as_secs_f64().max(0.001);
        link.up_rate.store((size as f64 / secs) as u64, std::sync::atomic::Ordering::Relaxed);
        self.index.lock().unwrap().files.insert(
            rel.to_string(),
            Entry { node, blob, size, sha256, stored_at: now_secs(), last_access: now_secs() },
        );
        self.save_index();
        Ok(())
    }

    fn candidates(&self, st: &AppState, rush: bool) -> Vec<(PathBuf, u64, SystemTime)> {
        let age = |min: Duration, m: &std::fs::Metadata| rush || m.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e >= min);
        let mut out = Vec::new();
        let mut add = |p: PathBuf, min: Duration| {
            if let Ok(m) = std::fs::metadata(&p) {
                if m.is_file() && m.len() > 0 && age(min, &m) {
                    out.push((p, m.len(), m.modified().unwrap_or(UNIX_EPOCH)));
                }
            }
        };
        for ext in std::fs::read_dir(&self.data_dir).into_iter().flatten().flatten() {
            let name = ext.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), "matches" | "storage" | "app") || !ext.path().is_dir() {
                continue;
            }
            for user in std::fs::read_dir(ext.path()).into_iter().flatten().flatten() {
                for f in std::fs::read_dir(user.path().join("clips")).into_iter().flatten().flatten() {
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
            .map(|m| m.values().filter(|m| m.status == relay_common::MatchStatus::Ended).map(|m| m.id).collect())
            .unwrap_or_default();
        for id in ended {
            if !st.processing_of(id).is_empty() {
                continue;
            }
            for p in std::fs::read_dir(st.match_dir(id)).into_iter().flatten().flatten() {
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

    async fn evict(&self) {
        let limit = self.cache_limit();
        let mut cached: Vec<(u64, String, u64)> = self
            .index
            .lock()
            .unwrap()
            .files
            .iter()
            .filter_map(|(rel, e)| {
                let size = std::fs::metadata(self.data_dir.join(rel)).ok()?.len();
                Some((e.last_access.max(e.stored_at), rel.clone(), size))
            })
            .collect();
        let mut total: u64 = cached.iter().map(|c| c.2).sum();
        cached.sort();
        for (_, rel, size) in cached {
            if total <= limit {
                break;
            }
            if tokio::fs::remove_file(self.data_dir.join(&rel)).await.is_ok() {
                total -= size;
            }
        }
        self.activity.lock().unwrap().cache_bytes = total;
    }

    async fn run_deletes(&self) {
        let pending: Vec<(String, String)> = self.index.lock().unwrap().deletes.clone();
        if pending.is_empty() {
            return;
        }
        let mut done = Vec::new();
        for (node, blob) in pending {
            if let Some(link) = self.links.get(&node) {
                if link.request(&Request::Delete { blob: blob.clone() }, &[]).await.is_ok() {
                    done.push((node, blob));
                }
            }
        }
        if !done.is_empty() {
            self.index.lock().unwrap().deletes.retain(|d| !done.contains(d));
            self.save_index();
        }
    }

    pub async fn run(self: Arc<Self>, st: Arc<AppState>) {
        tokio::spawn(tunnel::refresh_stats(self.clone()));
        loop {
            let _ = tokio::time::timeout(Duration::from_secs(60), self.wake.notified()).await;
            self.run_deletes().await;
            let rush = self.rush.swap(false, std::sync::atomic::Ordering::Relaxed);
            let this = self.clone();
            let st2 = st.clone();
            let mut todo = tokio::task::spawn_blocking(move || this.candidates(&st2, rush)).await.unwrap_or_default();
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
                if self.pick_node(size).is_none() {
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
                self.evict().await;
            }
            self.activity.lock().unwrap().current = None;
            self.evict().await;
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
    fn forgetting_a_match_queues_the_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::open(dir.path()).unwrap();
        let e = |b: &str| Entry { node: "n".into(), blob: b.into(), size: 1, sha256: String::new(), stored_at: 0, last_access: 0 };
        {
            let mut i = s.index.lock().unwrap();
            i.files.insert("matches/a/p/video.mp4".into(), e("1"));
            i.files.insert("matches/ab/p/video.mp4".into(), e("2"));
        }
        assert!(s.present(&dir.path().join("matches/a/p/video.mp4")));
        s.forget_dir(&dir.path().join("matches/a"));
        assert!(!s.present(&dir.path().join("matches/a/p/video.mp4")));
        assert!(s.present(&dir.path().join("matches/ab/p/video.mp4")), "solo la partita cancellata");
        assert_eq!(s.index.lock().unwrap().deletes, vec![("n".to_string(), "1".to_string())]);
    }
}

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use futures_util::stream;
use reqwest::{Body, StatusCode};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::{hls, throttle::Throttle};

const CHUNK: usize = 16 * 1024;
const POLL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct UploadConfig {
    pub base_url: String,
    pub token: String,
    pub match_id: String,
    pub player: String,
    pub dir: PathBuf,

    pub limit_bytes_per_sec: Option<u64>,
}

pub struct Uploader {
    cfg: UploadConfig,
    http: reqwest::Client,
    throttle: Option<Arc<Throttle>>,

    pub bytes_sent: Arc<AtomicU64>,
}

async fn ready_segments(dir: &Path) -> Result<Vec<(u64, PathBuf)>> {
    let mut v = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await?;
    while let Some(e) = rd.next_entry().await? {
        if let Some(n) = hls::segment_index(&e.file_name().to_string_lossy()) {
            v.push((n, e.path()));
        }
    }
    v.sort();
    Ok(v)
}

pub async fn read_all_durations(dir: &Path) -> std::collections::HashMap<u64, u32> {
    let mut all = std::collections::HashMap::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return all;
    };
    let mut files = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        if hls::is_playlist(&name) {
            files.push((hls::playlist_generation(&name), e.path()));
        }
    }

    files.sort();
    for (_, path) in files {
        if let Ok(p) = tokio::fs::read_to_string(path).await {
            all.extend(hls::parse_durations(&p));
        }
    }
    all
}

pub async fn pending_count(dir: &Path) -> u32 {
    ready_segments(dir)
        .await
        .map(|v| v.len() as u32)
        .unwrap_or(0)
}

impl Uploader {
    pub fn new(cfg: UploadConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        let throttle = cfg.limit_bytes_per_sec.map(|r| Arc::new(Throttle::new(r)));
        Ok(Self {
            cfg,
            http,
            throttle,
            bytes_sent: Arc::new(AtomicU64::new(0)),
        })
    }

    fn url(&self, tail: &str) -> String {
        format!(
            "{}/api/matches/{}/players/{}/{}",
            self.cfg.base_url.trim_end_matches('/'),
            self.cfg.match_id,
            self.cfg.player,
            tail
        )
    }

    async fn upload_one(&self, n: u64, path: &Path, duration_ms: Option<u32>) -> Result<()> {
        let data = Bytes::from(tokio::fs::read(path).await?);
        let sha = hex::encode(Sha256::digest(&data));
        let len = data.len();

        let body = match &self.throttle {
            Some(t) => {
                let t = t.clone();
                let chunks = (0..len.div_ceil(CHUNK)).map(move |i| (data.clone(), i, t.clone()));
                Body::wrap_stream(stream::unfold(chunks, |mut it| async move {
                    let (data, i, t) = it.next()?;
                    let end = ((i + 1) * CHUNK).min(data.len());
                    let chunk = data.slice(i * CHUNK..end);
                    t.acquire(chunk.len()).await;
                    Some((Ok::<_, std::io::Error>(chunk), it))
                }))
            }
            None => Body::from(data),
        };

        let secs = self
            .throttle
            .as_ref()
            .map(|t| (len as u64 / t.bytes_per_sec()) * 2)
            .unwrap_or(0)
            .max(60);
        let mut req = self
            .http
            .put(self.url(&format!("segments/{n:08}.ts")))
            .bearer_auth(&self.cfg.token)
            .header("x-sha256", sha)
            .header(reqwest::header::CONTENT_LENGTH, len)
            .timeout(Duration::from_secs(secs))
            .body(body);
        if let Some(ms) = duration_ms {
            req = req.header("x-segment-duration-ms", ms);
        }
        let resp = req.send().await?;
        let st = resp.status();
        if st.is_success() {
            self.bytes_sent.fetch_add(len as u64, Ordering::Relaxed);
            return Ok(());
        }
        let msg = resp.text().await.unwrap_or_default();
        match st {
            StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::CONFLICT => {
                bail!(Fatal(format!("{st}: {msg}")))
            }
            _ => bail!("{st}: {msg}"),
        }
    }

    pub async fn run(&self, mut capture_done: watch::Receiver<bool>) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            let done = *capture_done.borrow();
            let ready = ready_segments(&self.cfg.dir).await?;
            if ready.is_empty() {
                if done {
                    return Ok(());
                }
                tokio::select! {
                    _ = tokio::time::sleep(POLL) => {}
                    _ = capture_done.changed() => {}
                }
                continue;
            }

            let durations = read_all_durations(&self.cfg.dir).await;

            for (n, path) in ready {
                let dur = durations.get(&n).copied();

                if dur.is_none() && !*capture_done.borrow() {
                    tokio::time::sleep(POLL).await;
                    break;
                }

                if tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len() == 0)
                    .unwrap_or(false)
                {
                    tracing::warn!("segmento {n} vuoto, lo salto");
                    tokio::fs::remove_file(&path).await.ok();
                    continue;
                }
                match self.upload_one(n, &path, dur).await {
                    Ok(()) => {
                        tokio::fs::remove_file(&path).await.ok();
                        tracing::info!("segmento {n} caricato");
                        backoff = Duration::from_secs(1);
                    }
                    Err(e) if e.downcast_ref::<Fatal>().is_some() => return Err(e),
                    Err(e) => {
                        tracing::warn!(
                            "upload segmento {n} fallito ({e:#}), riprovo tra {backoff:?}"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                        break;
                    }
                }
            }
        }
    }

    pub async fn finish(&self) -> Result<()> {
        let resp = self
            .http
            .post(self.url("finish"))
            .bearer_auth(&self.cfg.token)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("invio di finish")?;
        if !resp.status().is_success() {
            bail!("finish rifiutato: {}", resp.status());
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Fatal(String);
impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "errore non recuperabile: {}", self.0)
    }
}
impl std::error::Error for Fatal {}

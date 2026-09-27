mod update;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use relay_common::{
    media::{self, Report},
    ops::{self, BigState, Finish, InputFile, Job, JobKind, Poll, Progress, BIG_REQUEST},
};
use reqwest::{Client, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const PARALLEL_DOWNLOADS: usize = 4;
const TRIES: u32 = 5;

#[derive(Clone)]
struct Config {
    server: String,
    key: String,
    dir: PathBuf,
    ffmpeg: PathBuf,
    threads: u32,
    slots: u32,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn config() -> Result<Config, String> {
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2);
    Ok(Config {
        server: var("RELAY_SERVER").unwrap_or_else(|| "https://relay.gavatech.org".into()).trim_end_matches('/').to_string(),
        key: var("RELAY_OPS_KEY").ok_or("manca RELAY_OPS_KEY")?,
        dir: var("RELAY_OPS_DIR").unwrap_or_else(|| "/var/lib/relay-ops".into()).into(),
        ffmpeg: var("RELAY_FFMPEG").unwrap_or_else(|| "ffmpeg".into()).into(),
        threads: var("RELAY_OPS_THREADS").and_then(|v| v.parse().ok()).unwrap_or(cores).clamp(1, 256),
        slots: var("RELAY_OPS_MAX_JOBS").and_then(|v| v.parse().ok()).unwrap_or(8).clamp(1, 16),
    })
}

fn load() -> f32 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0.0)
}

async fn ffmpeg_version(ffmpeg: &Path) -> Option<String> {
    let out = tokio::process::Command::new(ffmpeg).arg("-version").stdin(std::process::Stdio::null()).output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string())
}

struct Api {
    cfg: Config,
    http: Client,
    ffmpeg: Option<String>,
    running: Mutex<std::collections::HashSet<uuid::Uuid>>,
}

impl Api {
    fn url(&self, path: &str) -> String {
        format!("{}/api/ops{path}", self.cfg.server)
    }

    fn auth(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        r.header("authorization", format!("Bearer {}", self.cfg.key))
    }

    async fn progress(&self, job: &Job, stage: &str, pct: u8) {
        let _ = self
            .auth(self.http.post(self.url(&format!("/jobs/{}/progress", job.id))))
            .json(&Progress { stage: stage.into(), pct })
            .timeout(Duration::from_secs(30))
            .send()
            .await;
    }

    async fn finish(&self, job: &Job, f: &Finish) -> Result<(), String> {
        let mut last = String::new();
        for attempt in 0..TRIES {
            match self.auth(self.http.post(self.url(&format!("/jobs/{}/finish", job.id)))).json(f).timeout(Duration::from_secs(600)).send().await {
                Ok(r) if r.status().is_success() => return Ok(()),
                Ok(r) if r.status() == StatusCode::NOT_FOUND => return Err("lavoro non piu' assegnato a questo server".into()),
                Ok(r) => last = format!("{}: {}", r.status(), r.text().await.unwrap_or_default()),
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(Duration::from_secs(5 << attempt)).await;
        }
        Err(last)
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "relay_ops=info,relay_common=info".into()))
        .init();
    let cfg = match config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("relay-ops: {e}");
            std::process::exit(2);
        }
    };
    let ff = ffmpeg_version(&cfg.ffmpeg).await;
    match &ff {
        Some(v) => tracing::info!("relay-ops {VERSION}: {v}, {} thread, cartella {}", cfg.threads, cfg.dir.display()),
        None => tracing::error!("relay-ops {VERSION}: ffmpeg ({}) non funziona: i lavori falliranno", cfg.ffmpeg.display()),
    }
    let _ = tokio::fs::remove_dir_all(cfg.dir.join("work")).await;
    if let Err(e) = tokio::fs::create_dir_all(cfg.dir.join("work")).await {
        eprintln!("relay-ops: non riesco a creare {}: {e}", cfg.dir.display());
        std::process::exit(2);
    }
    tokio::spawn(update::run(cfg.server.clone()));
    let http = Client::builder().connect_timeout(Duration::from_secs(20)).build().expect("client http");
    let slots = cfg.slots;
    let api = Arc::new(Api { cfg, http, ffmpeg: ff, running: Mutex::new(Default::default()) });
    let mut loops = Vec::new();
    for slot in 0..slots {
        let api = api.clone();
        loops.push(tokio::spawn(async move { poll_loop(api, slot).await }));
    }
    for l in loops {
        let _ = l.await;
    }
}

async fn poll_loop(api: Arc<Api>, slot: u32) {
    let mut wait = 1u64;
    let mut connected = false;
    if slot > 0 {
        tokio::time::sleep(Duration::from_millis(300 * slot as u64)).await;
    }
    loop {
        let running: Vec<uuid::Uuid> = api.running.lock().unwrap().iter().copied().collect();
        let poll = Poll {
            version: VERSION.into(),
            threads: api.cfg.threads,
            load: load(),
            ffmpeg: api.ffmpeg.clone(),
            running,
            slots: api.cfg.slots,
        };
        let r = api.auth(api.http.post(api.url("/poll"))).json(&poll).timeout(Duration::from_secs(60)).send().await;
        match r {
            Ok(r) if r.status() == StatusCode::NO_CONTENT => {
                if !connected && slot == 0 {
                    tracing::info!("collegato a {}: aspetto lavori", api.cfg.server);
                }
                connected = true;
                wait = 1;
            }
            Ok(r) if r.status() == StatusCode::OK => {
                connected = true;
                wait = 1;
                match r.json::<Job>().await {
                    Ok(job) => {
                        api.running.lock().unwrap().insert(job.id);
                        let id = job.id;
                        run(&api, job).await;
                        api.running.lock().unwrap().remove(&id);
                    }
                    Err(e) => tracing::warn!("lavoro illeggibile: {e}"),
                }
            }
            Ok(r) if r.status() == StatusCode::UNAUTHORIZED => {
                if slot == 0 {
                    tracing::error!("chiave rifiutata dal centrale: controlla RELAY_OPS_KEY");
                }
                connected = false;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Ok(r) => {
                if slot == 0 {
                    tracing::warn!("il centrale risponde {}", r.status());
                }
                connected = false;
                tokio::time::sleep(Duration::from_secs(wait)).await;
                wait = (wait * 2).min(60);
            }
            Err(e) => {
                if slot == 0 {
                    tracing::warn!("centrale non raggiungibile: {e}");
                }
                connected = false;
                tokio::time::sleep(Duration::from_secs(wait)).await;
                wait = (wait * 2).min(60);
            }
        }
    }
}

fn describe(job: &Job) -> String {
    match &job.kind {
        JobKind::Match { match_id, player } => format!("partita {match_id}, video di {player}"),
        JobKind::Clip { engine, clip, .. } => format!("clip {engine} {clip}"),
    }
}

async fn run(api: &Arc<Api>, job: Job) {
    let name = describe(&job);
    tracing::info!("inizio: {name} ({} thread)", if job.threads > 0 { job.threads } else { api.cfg.threads });
    let work = api.cfg.dir.join("work").join(job.id.to_string());
    let _ = tokio::fs::remove_dir_all(&work).await;
    let state = Arc::new(Mutex::new(("download".to_string(), 0u8, true)));
    let beat = {
        let (api, job, state) = (api.clone(), job.clone(), state.clone());
        tokio::spawn(async move {
            let mut since = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                since += 2;
                let (stage, pct, changed) = {
                    let mut s = state.lock().unwrap();
                    let c = s.2;
                    s.2 = false;
                    (s.0.clone(), s.1, c)
                };
                if changed || since >= 30 {
                    since = 0;
                    api.progress(&job, &stage, pct).await;
                }
            }
        })
    };
    let report: Report = {
        let state = state.clone();
        Arc::new(move |stage: &'static str, pct: u8| {
            let mut s = state.lock().unwrap();
            if s.0 != stage || s.1 != pct {
                *s = (stage.to_string(), pct, true);
            }
        })
    };
    let result = async {
        tokio::fs::create_dir_all(&work).await.map_err(|e| e.to_string())?;
        download(api, &job, &work, &report).await?;
        let threads = if job.threads > 0 { job.threads } else { api.cfg.threads };
        let outputs = process(&api.cfg, threads, &job, &work, &report).await?;
        upload(api, &job, &work, &outputs, &report).await?;
        Ok::<Vec<String>, String>(outputs.into_iter().map(|o| o.0.to_string()).collect())
    }
    .await;
    beat.abort();
    let finish = match &result {
        Ok(outputs) => Finish { ok: true, error: None, outputs: outputs.clone() },
        Err(e) => Finish { ok: false, error: Some(e.clone()), outputs: Vec::new() },
    };
    match (api.finish(&job, &finish).await, &result) {
        (Ok(()), Ok(_)) => tracing::info!("fatto: {name}"),
        (Ok(()), Err(e)) => tracing::warn!("non riuscito: {name}: {e}"),
        (Err(e), _) => tracing::warn!("{name}: il centrale non ha accettato la fine del lavoro: {e}"),
    }
    let _ = tokio::fs::remove_dir_all(&work).await;
}

async fn download(api: &Arc<Api>, job: &Job, work: &Path, report: &Report) -> Result<(), String> {
    let total: u64 = job.inputs.iter().map(|i| i.size).sum::<u64>().max(1);
    let done = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL_DOWNLOADS));
    let mut tasks = tokio::task::JoinSet::new();
    for input in job.inputs.clone() {
        if !ops::valid_input_name(&input.name) {
            return Err(format!("nome di file non valido: {}", input.name));
        }
        let permit = sem.clone().acquire_owned().await.map_err(|e| e.to_string())?;
        let (api, id, work, done, report) = (api.clone(), job.id, work.to_path_buf(), done.clone(), report.clone());
        tasks.spawn(async move {
            let r = fetch(&api, id, &input, &work, &done, total, &report).await;
            drop(permit);
            r
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.map_err(|e| e.to_string())??;
    }
    Ok(())
}

async fn fetch(
    api: &Api,
    id: uuid::Uuid,
    input: &InputFile,
    work: &Path,
    done: &std::sync::atomic::AtomicU64,
    total: u64,
    report: &Report,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let path = work.join(&input.name);
    let mut last = String::new();
    for attempt in 0..TRIES {
        let mut got = 0u64;
        let res = async {
            let mut r = api
                .auth(api.http.get(api.url(&format!("/jobs/{id}/in/{}", input.name))))
                .timeout(Duration::from_secs(3 * 3600))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !r.status().is_success() {
                return Err(format!("{} per {}", r.status(), input.name));
            }
            let mut f = tokio::fs::File::create(&path).await.map_err(|e| e.to_string())?;
            while let Some(chunk) = r.chunk().await.map_err(|e| e.to_string())? {
                f.write_all(&chunk).await.map_err(|e| e.to_string())?;
                got += chunk.len() as u64;
                let all = done.fetch_add(chunk.len() as u64, Ordering::Relaxed) + chunk.len() as u64;
                report("download", ((all * 100) / total).min(100) as u8);
            }
            f.flush().await.map_err(|e| e.to_string())?;
            if got != input.size {
                return Err(format!("{}: ricevuti {got} byte invece di {}", input.name, input.size));
            }
            Ok(())
        }
        .await;
        match res {
            Ok(()) => return Ok(()),
            Err(e) => {
                done.fetch_sub(got, Ordering::Relaxed);
                last = e;
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
        }
    }
    Err(format!("download non riuscito: {last}"))
}

async fn exists(p: &Path) -> bool {
    tokio::fs::metadata(p).await.is_ok_and(|m| m.len() > 0)
}

async fn process(cfg: &Config, threads: u32, job: &Job, work: &Path, report: &Report) -> Result<Vec<(&'static str, PathBuf)>, String> {
    let ff = &cfg.ffmpeg;
    let mut out: Vec<(&'static str, PathBuf)> = Vec::new();
    match &job.kind {
        JobKind::Match { .. } => {
            let has_segments = job.inputs.iter().any(|i| i.name.ends_with(".ts"));
            if has_segments {
                if !media::build_video(ff, work, report).await? {
                    return Err("nessun segmento da unire".into());
                }
                out.push((ops::OUT_VIDEO, work.join(media::VIDEO_FILE)));
                for (name, file) in [(ops::OUT_INFO, media::INFO_FILE), (ops::OUT_THUMB, media::THUMB_FILE)] {
                    if exists(&work.join(file)).await {
                        out.push((name, work.join(file)));
                    }
                }
                match media::make_web(ff, work, report, threads).await {
                    Ok(true) => out.push((ops::OUT_WEB, work.join(media::WEB_FILE))),
                    Ok(false) => {}
                    Err(e) => tracing::warn!("versione leggera non creata: {e}"),
                }
            } else if job.inputs.iter().any(|i| i.name == media::WEB_FILE) {
                report("web", 50);
                media::make_vod(ff, &work.join(media::WEB_FILE), work).await?;
            } else {
                return Err("lavoro senza file da elaborare".into());
            }
            let vod = work.join(media::VOD_DIR);
            if exists(&vod.join(media::VOD_INDEX)).await && exists(&vod.join(media::VOD_MEDIA)).await {
                out.push((ops::OUT_VOD_MEDIA, vod.join(media::VOD_MEDIA)));
                out.push((ops::OUT_VOD_INDEX, vod.join(media::VOD_INDEX)));
            }
        }
        JobKind::Clip { clip, offset_secs, duration_secs, .. } => {
            let src = work.join(format!("{clip}.src.ts"));
            let mp4 = work.join(format!("{clip}.mp4"));
            if exists(&src).await {
                report("video", 10);
                media::cut_clip(ff, &src, *offset_secs, *duration_secs, &mp4).await?;
                out.push((ops::OUT_CLIP, mp4.clone()));
            } else if !exists(&mp4).await {
                return Err("clip non ricevuta".into());
            }
            report("video", 80);
            let dur = media::probe(ff, &mp4).await.and_then(|p| p.duration_secs).unwrap_or(*duration_secs);
            let thumb = work.join(format!("{clip}.jpg"));
            if media::make_thumb(ff, &mp4, &thumb, (dur * 0.4).min(60.0), 640).await {
                out.push((ops::OUT_THUMB, thumb));
            }
        }
    }
    Ok(out)
}

async fn upload(api: &Arc<Api>, job: &Job, _work: &Path, outputs: &[(&'static str, PathBuf)], report: &Report) -> Result<(), String> {
    let mut sizes = Vec::new();
    for (_, p) in outputs {
        sizes.push(tokio::fs::metadata(p).await.map_err(|e| e.to_string())?.len());
    }
    let total: u64 = sizes.iter().sum::<u64>().max(1);
    let mut sent = 0u64;
    for ((name, path), size) in outputs.iter().zip(sizes) {
        if ops::is_big(name) {
            send_big(api, job, name, path, size, sent, total, report).await?;
        } else {
            let body = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
            let mut last = String::new();
            let mut ok = false;
            for attempt in 0..TRIES {
                match api
                    .auth(api.http.put(api.url(&format!("/jobs/{}/out/{name}?part=0", job.id))))
                    .body(body.clone())
                    .timeout(Duration::from_secs(120))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {
                        ok = true;
                        break;
                    }
                    Ok(r) => last = r.status().to_string(),
                    Err(e) => last = e.to_string(),
                }
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
            if !ok {
                return Err(format!("invio di {name} non riuscito: {last}"));
            }
        }
        sent += size;
        report("upload", ((sent * 100) / total).min(100) as u8);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_big(api: &Api, job: &Job, name: &str, path: &Path, size: u64, before: u64, total: u64, report: &Report) -> Result<(), String> {
    let parts = size.div_ceil(BIG_REQUEST).max(1);
    let mut f = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let mut part = 0u64;
    let mut failures = 0u32;
    while part < parts {
        let from = part * BIG_REQUEST;
        let len = (size - from).min(BIG_REQUEST) as usize;
        let mut buf = vec![0u8; len];
        f.seek(std::io::SeekFrom::Start(from)).await.map_err(|e| e.to_string())?;
        f.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
        let r = api
            .auth(api.http.put(api.url(&format!("/jobs/{}/out/{name}?part={part}&size={size}", job.id))))
            .body(buf)
            .timeout(Duration::from_secs(1800))
            .send()
            .await;
        let err = match r {
            Ok(r) if r.status().is_success() => {
                part += 1;
                failures = 0;
                report("upload", (((before + from + len as u64) * 100) / total).min(100) as u8);
                continue;
            }
            Ok(r) if r.status() == StatusCode::CONFLICT => {
                part = r.json::<BigState>().await.map(|s| s.next).unwrap_or(0);
                continue;
            }
            Ok(r) if r.status() == StatusCode::NOT_FOUND => return Err("lavoro non piu' assegnato a questo server".into()),
            Ok(r) => format!("{}: {}", r.status(), r.text().await.unwrap_or_default()),
            Err(e) => e.to_string(),
        };
        failures += 1;
        if failures > TRIES {
            return Err(format!("invio di {name} non riuscito: {err}"));
        }
        tracing::warn!("invio di {name}, pezzo {part}: {err}; riprovo");
        tokio::time::sleep(Duration::from_secs(5 << failures.min(5))).await;
        part = match api
            .auth(api.http.get(api.url(&format!("/jobs/{}/out/{name}", job.id))))
            .timeout(Duration::from_secs(60))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.json::<BigState>().await.map(|s| s.next).unwrap_or(0),
            _ => 0,
        };
    }
    Ok(())
}

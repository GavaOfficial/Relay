use std::{path::Path, process::Stdio, sync::Arc};

use relay_common::{
    media::{self, Report},
    ops::{InputFile, JobKind},
};
use tokio::process::Command;
use uuid::Uuid;

use crate::state::AppState;

pub use relay_common::media::{
    duration_ok, parse_loudnorm, parse_probe, Loudness, Probe, INFO_FILE, THUMB_FILE, VIDEO_FILE, VOD_DIR,
    VOD_INDEX, VOD_MEDIA, WEB_FILE,
};

const LOCAL_THREADS: u32 = 2;

fn archived(path: &Path) -> bool {
    crate::storage::global().is_some_and(|s| s.entry(path).is_some())
}

pub async fn has_video(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join(VIDEO_FILE)).await.map(|m| m.len() > 0).unwrap_or(false) || archived(&dir.join(VIDEO_FILE))
}

pub async fn has_vod(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join(VOD_DIR).join(VOD_INDEX)).await.map(|m| m.len() > 0).unwrap_or(false)
}

pub async fn has_web(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join(WEB_FILE)).await.map(|m| m.len() > 0).unwrap_or(false) || archived(&dir.join(WEB_FILE))
}

pub async fn read_duration(dir: &Path) -> Option<u64> {
    media::read_duration(dir).await
}

async fn file_input(dir: &Path, name: &str) -> Option<InputFile> {
    let path = dir.join(name);
    let size = match tokio::fs::metadata(&path).await {
        Ok(m) => m.len(),
        Err(_) => crate::storage::global()?.entry(&path)?.size,
    };
    Some(InputFile { name: name.to_string(), size })
}

async fn match_inputs(dir: &Path) -> Option<Vec<InputFile>> {
    if !has_video(dir).await {
        let segs = media::segments(dir).await.ok()?;
        if segs.is_empty() {
            return None;
        }
        let mut inputs = Vec::new();
        let mut rd = tokio::fs::read_dir(dir).await.ok()?;
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().into_owned();
            if media::is_segment_file(&name) {
                if let Ok(m) = e.metadata().await {
                    inputs.push(InputFile { name, size: m.len() });
                }
            }
        }
        inputs.sort_by(|a, b| a.name.cmp(&b.name));
        return Some(inputs);
    }
    if has_web(dir).await && !has_vod(dir).await {
        return Some(vec![file_input(dir, WEB_FILE).await?]);
    }
    None
}

async fn enqueue_match(st: &Arc<AppState>, id: Uuid) {
    let players = match st.matches.read().await.get(&id) {
        Some(m) => m.players.clone(),
        None => return,
    };
    for p in players {
        let dir = st.player_dir(id, &p);
        let kind = JobKind::Match { match_id: id, player: p.clone() };
        match match_inputs(&dir).await {
            Some(inputs) => {
                if st.ops.enqueue(kind, &dir, inputs) {
                    tracing::info!("partita {id}: video di {p} in coda per il server operazioni");
                }
                if media::segments(&dir).await.is_ok_and(|s| !s.is_empty()) && st.processing_of(id).get(&p).is_none() {
                    st.set_processing(id, &p, "queue", 0);
                }
            }
            None => {
                if !st.ops.is_queued(&kind) && has_video(&dir).await {
                    st.clear_processing(id, &p);
                }
            }
        }
    }
}

pub async fn run(st: Arc<AppState>, id: Uuid) {
    if st.ops.enabled() {
        enqueue_match(&st, id).await;
        return;
    }
    let Some(ffmpeg) = st.ffmpeg.clone() else {
        return;
    };
    let players = match st.matches.read().await.get(&id) {
        Some(m) => m.players.clone(),
        None => return,
    };

    let mut queued = Vec::new();
    for p in &players {
        let dir = st.player_dir(id, p);
        if !has_video(&dir).await && media::segments(&dir).await.is_ok_and(|s| !s.is_empty()) {
            st.set_processing(id, p, "queue", 0);
            queued.push(p.clone());
        }
    }
    let _one_at_a_time = st.finalize_lock.lock().await;
    let reporter = |p: &str| -> Report {
        let (st, p) = (st.clone(), p.to_string());
        Arc::new(move |stage, pct| st.set_processing(id, &p, stage, pct))
    };
    let mut fresh = Vec::new();
    for p in players {
        let dir = st.player_dir(id, &p);
        let res = if has_video(&dir).await {
            media::remove_segments(&dir).await;
            media::write_extras(&ffmpeg, &dir).await;
            Ok(false)
        } else {
            media::build_video(&ffmpeg, &dir, &reporter(&p)).await
        };
        match res {
            Ok(true) => {
                tracing::info!("partita {id}: video di {p} pronto, segmenti eliminati");
                fresh.push((p, dir));
            }
            Ok(false) => st.clear_processing(id, &p),
            Err(e) => {
                st.clear_processing(id, &p);
                tracing::warn!("partita {id}: video di {p} non creato ({e}); i segmenti restano");
            }
        }
    }

    for (p, dir) in fresh {
        if !has_web(&dir).await {
            match media::make_web(&ffmpeg, &dir, &reporter(&p), LOCAL_THREADS).await {
                Ok(true) => tracing::info!("partita {id}: versione leggera di {p} pronta"),
                Ok(false) => {}
                Err(e) => tracing::warn!("partita {id}: versione leggera di {p} non creata ({e})"),
            }
        }
        st.clear_processing(id, &p);
    }
    for p in queued {
        st.clear_processing(id, &p);
    }

    let players = match st.matches.read().await.get(&id) {
        Some(m) => m.players.clone(),
        None => return,
    };
    for p in players {
        let dir = st.player_dir(id, &p);
        if !has_vod(&dir).await && has_web(&dir).await {
            if let Err(e) = media::make_vod(&ffmpeg, &dir.join(WEB_FILE), &dir).await {
                tracing::warn!("versione per lo streaming non creata ({e})");
            }
        }
    }
}

pub async fn resume_all(st: Arc<AppState>) {
    if !st.ops.enabled() {
        let Some(ff) = st.ffmpeg.clone() else {
            tracing::warn!("nessun server operazioni e RELAY_FFMPEG non impostato: i segmenti restano sul disco");
            return;
        };
        match Command::new(&ff).arg("-version").stdin(Stdio::null()).output().await {
            Ok(o) if o.status.success() => {
                tracing::info!("ffmpeg: {}", String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("?"));
            }
            Ok(o) => tracing::error!("ffmpeg ({}) non funziona: {}", ff.display(), o.status),
            Err(e) => tracing::error!("ffmpeg ({}) non parte: {e}", ff.display()),
        }
    }
    let ended: Vec<Uuid> = st
        .matches
        .read()
        .await
        .values()
        .filter(|m| m.status == relay_common::MatchStatus::Ended)
        .map(|m| m.id)
        .collect();
    for id in ended {
        run(st.clone(), id).await;
    }
    crate::songlib::resume_clips(st.clone()).await;
}

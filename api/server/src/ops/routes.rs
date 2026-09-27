use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use relay_common::{
    media,
    ops::{self, BigState, Finish, JobKind, Poll, Progress},
    storage::{cipher_len, Request, Response as NodeResponse, PLAIN_CHUNK},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{label, BigUpload, Queued, Sink};
use crate::{
    auth::AuthUser,
    error::AppError,
    state::AppState,
    storage::{crypto::cipher_offset, routes::admin},
};

type St = State<Arc<AppState>>;

const LONG_POLL: Duration = Duration::from_secs(25);
const PARALLEL_PUTS: usize = 16;

fn node_of(st: &AppState, headers: &HeaderMap) -> Result<String, AppError> {
    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;
    st.ops.node_for_key(key.trim()).ok_or(AppError::Unauthorized)
}

fn stage_name(stage: &str) -> &'static str {
    match stage {
        "queue" => "queue",
        "download" => "download",
        "web" => "web",
        "upload" => "upload",
        _ => "video",
    }
}

fn out_path(st: &AppState, q: &Queued, output: &str) -> Option<PathBuf> {
    let dir = st.ops.abs(&q.dir);
    match &q.job.kind {
        JobKind::Match { .. } => Some(match output {
            ops::OUT_VIDEO => dir.join(media::VIDEO_FILE),
            ops::OUT_WEB => dir.join(media::WEB_FILE),
            ops::OUT_VOD_MEDIA => dir.join(media::VOD_DIR).join(media::VOD_MEDIA),
            ops::OUT_VOD_INDEX => dir.join(media::VOD_DIR).join(media::VOD_INDEX),
            ops::OUT_THUMB => dir.join(media::THUMB_FILE),
            ops::OUT_INFO => dir.join(media::INFO_FILE),
            _ => return None,
        }),
        JobKind::Clip { clip, .. } => Some(match output {
            ops::OUT_CLIP => dir.join(format!("{clip}.mp4")),
            ops::OUT_THUMB => dir.join(format!("{clip}.jpg")),
            _ => return None,
        }),
    }
}

fn tmp_path(st: &AppState, q: &Queued, output: &str) -> PathBuf {
    st.ops.abs(&q.dir).join(format!(".ops-{}-{output}", q.job.id))
}

pub async fn poll(State(st): St, headers: HeaderMap, Json(p): Json<Poll>) -> Result<Response, AppError> {
    let node = node_of(&st, &headers)?;
    let running = p.running.clone();
    st.ops.see(&node, Some(p));
    let deadline = tokio::time::Instant::now() + LONG_POLL;
    let mut first = true;
    loop {
        let notified = st.ops.wake.notified();
        let known = first.then_some(running.as_slice());
        first = false;
        if let Some(job) = st.ops.take(&node, known) {
            if let JobKind::Match { match_id, player } = &job.kind {
                st.set_processing(*match_id, player, "download", 0);
            }
            tracing::info!("operazioni: {} assegnato", label(&job.kind));
            return Ok(Json(job).into_response());
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        st.ops.see(&node, None);
    }
}

pub async fn input(State(st): St, headers: HeaderMap, Path((id, name)): Path<(Uuid, String)>) -> Result<Response, AppError> {
    let node = node_of(&st, &headers)?;
    let q = st.ops.leased(&node, id).ok_or(AppError::NotFound)?;
    if !ops::valid_input_name(&name) || !q.job.inputs.iter().any(|i| i.name == name) {
        return Err(AppError::NotFound);
    }
    crate::routes::serve_file(&st.ops.abs(&q.dir).join(&name), &headers, "", None).await
}

pub async fn progress(State(st): St, headers: HeaderMap, Path(id): Path<Uuid>, Json(p): Json<Progress>) -> Result<StatusCode, AppError> {
    let node = node_of(&st, &headers)?;
    let q = st.ops.leased(&node, id).ok_or(AppError::NotFound)?;
    st.ops.progress(&node, id, &p.stage, p.pct);
    if let JobKind::Match { match_id, player } = &q.job.kind {
        st.set_processing(*match_id, player, stage_name(&p.stage), p.pct);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PartQuery {
    part: u64,
    #[serde(default)]
    size: Option<u64>,
}

async fn abort(sink: Sink) {
    match sink {
        Sink::Local { tmp, file } => {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        Sink::Stored { link, blob, .. } => {
            let _ = link.request(&Request::Abort { blob }, &[]).await;
        }
    }
}

pub async fn big_state(State(st): St, headers: HeaderMap, Path((id, output)): Path<(Uuid, String)>) -> Result<Json<BigState>, AppError> {
    let node = node_of(&st, &headers)?;
    st.ops.leased(&node, id).ok_or(AppError::NotFound)?;
    let next = st.ops.bigs.lock().await.get(&(id, output)).map(|b| b.next).unwrap_or(0);
    Ok(Json(BigState { next }))
}

pub async fn put_output(
    State(st): St,
    headers: HeaderMap,
    Path((id, output)): Path<(Uuid, String)>,
    Query(pq): Query<PartQuery>,
    body: Bytes,
) -> Result<Response, AppError> {
    let node = node_of(&st, &headers)?;
    let q = st.ops.leased(&node, id).ok_or(AppError::NotFound)?;
    out_path(&st, &q, &output).ok_or(AppError::BadRequest("risultato sconosciuto"))?;
    if !ops::is_big(&output) {
        if pq.part != 0 || body.len() > 4 * 1024 * 1024 {
            return Err(AppError::BadRequest("file piccolo troppo grande"));
        }
        let tmp = tmp_path(&st, &q, &output);
        tokio::fs::write(&tmp, &body).await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if body.len() as u64 > ops::BIG_REQUEST {
        return Err(AppError::BadRequest("pezzo troppo grande"));
    }
    let key = (id, output.clone());
    let current = st.ops.bigs.lock().await.remove(&key);
    let mut up = match (pq.part, current) {
        (0, old) => {
            if let Some(old) = old {
                abort(old.sink).await;
            }
            let sink = if st.storage.has_nodes() {
                let (node_id, link) = st
                    .storage
                    .pick_for(pq.size.unwrap_or(0))
                    .ok_or(AppError::Unavailable("nessun server di archivio con spazio collegato"))?;
                Sink::Stored { node: node_id, link, blob: Uuid::new_v4().to_string(), hasher: Sha256::new() }
            } else {
                let tmp = tmp_path(&st, &q, &output);
                let file = tokio::fs::File::create(&tmp).await?;
                Sink::Local { tmp, file }
            };
            BigUpload { next: 0, chunk: 0, plain: 0, sink }
        }
        (part, Some(cur)) if cur.next == part => cur,
        (_, cur) => {
            let next = cur.as_ref().map(|c| c.next).unwrap_or(0);
            if let Some(cur) = cur {
                st.ops.bigs.lock().await.insert(key, cur);
            }
            return Ok((StatusCode::CONFLICT, Json(BigState { next })).into_response());
        }
    };
    let res = write_part(&st, &mut up, &body).await;
    match res {
        Ok(()) => {
            up.next += 1;
            up.plain += body.len() as u64;
            st.ops.bigs.lock().await.insert(key, up);
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Err(e) => {
            abort(up.sink).await;
            tracing::warn!("operazioni: invio di {output} interrotto: {e}");
            Err(AppError::Unavailable("invio al server di archivio non riuscito: riprova"))
        }
    }
}

async fn write_part(st: &AppState, up: &mut BigUpload, body: &[u8]) -> Result<(), String> {
    match &mut up.sink {
        Sink::Local { file, .. } => {
            use tokio::io::AsyncWriteExt;
            file.write_all(body).await.map_err(|e| e.to_string())
        }
        Sink::Stored { link, blob, hasher, .. } => {
            let cipher = st.storage.cipher(blob);
            let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL_PUTS));
            let mut tasks = tokio::task::JoinSet::new();
            for piece in body.chunks(PLAIN_CHUNK as usize) {
                let sealed = cipher.seal(up.chunk, piece);
                hasher.update(&sealed);
                let permit = sem.clone().acquire_owned().await.map_err(|e| e.to_string())?;
                let (link, blob, offset) = (link.clone(), blob.clone(), cipher_offset(up.chunk));
                tasks.spawn(async move {
                    let r = link.request(&Request::Put { blob, offset }, &sealed).await;
                    drop(permit);
                    r
                });
                up.chunk += 1;
            }
            while let Some(r) = tasks.join_next().await {
                r.map_err(|e| e.to_string())??;
            }
            Ok(())
        }
    }
}

async fn place(st: &AppState, q: &Queued, output: &str) -> Result<(), String> {
    let path = out_path(st, q, output).ok_or("risultato sconosciuto")?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    if !ops::is_big(output) {
        let tmp = tmp_path(st, q, output);
        return tokio::fs::rename(&tmp, &path).await.map_err(|_| format!("{output} non ricevuto"));
    }
    let up = st.ops.bigs.lock().await.remove(&(q.job.id, output.to_string())).ok_or(format!("{output} non ricevuto"))?;
    match up.sink {
        Sink::Local { tmp, mut file } => {
            use tokio::io::AsyncWriteExt;
            file.flush().await.map_err(|e| e.to_string())?;
            drop(file);
            tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())
        }
        Sink::Stored { node, link, blob, hasher } => {
            let sha = hex::encode(hasher.finalize());
            match link.request(&Request::Commit { blob: blob.clone(), size: cipher_len(up.plain), sha256: sha.clone() }, &[]).await {
                Ok(NodeResponse::Ok) => {}
                Ok(other) => return Err(format!("risposta inattesa dall'archivio: {other:?}")),
                Err(e) => return Err(e),
            }
            st.storage.adopt(&path, &node, &blob, up.plain, &sha);
            let _ = tokio::fs::remove_file(&path).await;
            Ok(())
        }
    }
}

async fn cleanup(st: &AppState, q: &Queued) {
    let keys: Vec<(Uuid, String)> = st.ops.bigs.lock().await.keys().filter(|k| k.0 == q.job.id).cloned().collect();
    for k in keys {
        if let Some(up) = st.ops.bigs.lock().await.remove(&k) {
            abort(up.sink).await;
        }
    }
    let dir = st.ops.abs(&q.dir);
    let prefix = format!(".ops-{}-", q.job.id);
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }
}

pub async fn finish(State(st): St, headers: HeaderMap, Path(id): Path<Uuid>, Json(f): Json<Finish>) -> Result<StatusCode, AppError> {
    let node = node_of(&st, &headers)?;
    let q = st.ops.leased(&node, id).ok_or(AppError::NotFound)?;
    if !f.ok {
        cleanup(&st, &q).await;
        st.ops.failed(&node, id, f.error.as_deref().unwrap_or("errore sconosciuto"));
        if let JobKind::Match { match_id, player } = &q.job.kind {
            st.set_processing(*match_id, player, "queue", 0);
        }
        return Ok(StatusCode::NO_CONTENT);
    }
    for output in &f.outputs {
        if let Err(e) = place(&st, &q, output).await {
            cleanup(&st, &q).await;
            st.ops.failed(&node, id, &e);
            return Err(AppError::Unavailable("risultati non salvati: il lavoro verra' ripetuto"));
        }
    }
    cleanup(&st, &q).await;
    match &q.job.kind {
        JobKind::Match { match_id, player } => {
            let dir = st.ops.abs(&q.dir);
            if crate::finalize::has_video(&dir).await {
                media::remove_segments(&dir).await;
            }
            st.clear_processing(*match_id, player);
            tracing::info!("partita {match_id}: video di {player} pronto (server operazioni)");
        }
        JobKind::Clip { clip, .. } => {
            let clips = st.ops.abs(&q.dir);
            let _ = tokio::fs::remove_file(clips.join(format!("{clip}.src.ts"))).await;
            if let Some(user_dir) = clips.parent() {
                crate::songlib::clip_processed(user_dir, *clip, f.outputs.iter().any(|o| o == ops::OUT_THUMB)).await;
            }
        }
    }
    st.ops.done(&node, id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn status(State(st): St, AuthUser(user): AuthUser) -> Result<Json<serde_json::Value>, AppError> {
    admin(&st, &user)?;
    let live = st.ops.live.lock().unwrap().clone();
    let jobs = st.ops.jobs();
    let nodes: Vec<serde_json::Value> = st
        .ops
        .nodes()
        .into_iter()
        .map(|n| {
            let l = live.get(&n.id).cloned().unwrap_or_default();
            let running: Vec<serde_json::Value> = l
                .jobs
                .iter()
                .filter_map(|(id, (stage, pct))| {
                    let q = jobs.iter().find(|q| q.job.id == *id && q.assigned.as_deref() == Some(n.id.as_str()))?;
                    Some(serde_json::json!({ "label": label(&q.job.kind), "stage": stage, "pct": pct }))
                })
                .collect();
            serde_json::json!({
                "id": n.id,
                "name": n.name,
                "created_at": n.created_at,
                "online": st.ops.online(&n.id),
                "version": (!l.poll.version.is_empty()).then(|| l.poll.version.clone()),
                "threads": l.poll.threads,
                "load": l.poll.load,
                "ffmpeg": l.poll.ffmpeg,
                "jobs": running,
                "parallel": n.parallel,
                "limit": st.ops.limit(&n.id),
                "slots": l.poll.slots,
            })
        })
        .collect();
    let now = super::now_secs();
    let queue: Vec<serde_json::Value> = jobs
        .iter()
        .map(|q| {
            serde_json::json!({
                "id": q.job.id,
                "label": label(&q.job.kind),
                "created_at": q.created_at,
                "attempts": q.attempts,
                "last_error": q.last_error,
                "running": q.assigned.is_some(),
                "retry_in": q.not_before.saturating_sub(now),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "nodes": nodes, "queue": queue })))
}

#[derive(Deserialize)]
pub struct NewNode {
    name: String,
}

#[derive(Deserialize)]
pub struct NodePatch {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parallel: Option<u32>,
    #[serde(default)]
    auto: bool,
}

pub async fn add_node(State(st): St, AuthUser(user): AuthUser, Json(input): Json<NewNode>) -> Result<Json<serde_json::Value>, AppError> {
    admin(&st, &user)?;
    let name = relay_common::clean_name(&input.name).ok_or(AppError::BadRequest("nome non valido"))?;
    let first = !st.ops.enabled();
    let (node, key) = st.ops.add_node(&name);
    if first {
        tokio::spawn(crate::finalize::resume_all(st.clone()));
    }
    Ok(Json(serde_json::json!({ "id": node.id, "name": node.name, "key": key })))
}

pub async fn update_node(State(st): St, AuthUser(user): AuthUser, Path(id): Path<String>, Json(p): Json<NodePatch>) -> Result<StatusCode, AppError> {
    admin(&st, &user)?;
    if let Some(name) = &p.name {
        let name = relay_common::clean_name(name).ok_or(AppError::BadRequest("nome non valido"))?;
        if !st.ops.rename_node(&id, &name) {
            return Err(AppError::NotFound);
        }
    }
    if p.auto || p.parallel.is_some() {
        let parallel = if p.auto { None } else { p.parallel };
        if !st.ops.set_parallel(&id, parallel) {
            return Err(AppError::NotFound);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_key(State(st): St, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
    admin(&st, &user)?;
    let key = st.ops.rotate_key(&id).ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::json!({ "key": key })))
}

pub async fn remove_node(State(st): St, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    admin(&st, &user)?;
    if !st.ops.remove_node(&id) {
        return Err(AppError::NotFound);
    }
    if !st.ops.enabled() {
        tokio::spawn(crate::finalize::resume_all(st.clone()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn retry(State(st): St, AuthUser(user): AuthUser) -> Result<StatusCode, AppError> {
    admin(&st, &user)?;
    st.ops.retry_now();
    Ok(StatusCode::NO_CONTENT)
}

pub async fn install_script() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], INSTALL_SH)
}

const INSTALL_SH: &str = include_str!("install.sh");

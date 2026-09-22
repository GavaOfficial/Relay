use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use relay_common::{
    clean_name, next_boundary, valid_id, CreateMatch, MatchInfo, MatchStatus, ServerMsg,
    SessionGrant, START_LEAD_MS,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    auth::{bearer_token, AppUser, AppWho, AuthUser, Identity, Who},
    error::AppError,
    playlist,
    state::AppState,
};

type St = State<Arc<AppState>>;

async fn get_match(st: &AppState, id: Uuid) -> Result<MatchInfo, AppError> {
    st.matches
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or(AppError::NotFound)
}

fn ensure_participant(m: &MatchInfo, user: &str) -> Result<(), AppError> {
    if m.players.iter().any(|p| p == user) || m.coordinator == user {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn broadcast_state(st: &AppState, m: &MatchInfo) {
    st.hub_broadcast(
        m.id,
        &ServerMsg::State {
            status: m.status,
            start_at_ms: m.started_at_ms,
            stop_at_ms: m.stopped_at_ms,
        },
    );
}

pub async fn healthz() -> &'static str {
    "ok"
}

fn redact(mut m: MatchInfo, user: &str) -> MatchInfo {
    if m.coordinator != user {
        m.invite_code = None;
    }
    m
}

fn remember_name(m: &mut MatchInfo, who: &Identity) {
    if let Some(n) = who.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        m.names.insert(who.id.clone(), n.chars().take(80).collect());
    }
}

pub async fn create_match(
    State(st): St,
    AppWho(who): AppWho,
    Json(req): Json<CreateMatch>,
) -> Result<(StatusCode, Json<MatchInfo>), AppError> {
    let mut players = req.players;
    if req.play {
        players.push(who.id.clone());
    }
    if players.len() > 16 {
        return Err(AppError::BadRequest("at most 16 players"));
    }
    if !valid_id(&who.id) || !players.iter().all(|p| valid_id(p)) {
        return Err(AppError::BadRequest("invalid player id"));
    }
    players.sort();
    players.dedup();
    let mut m = MatchInfo {
        id: Uuid::new_v4(),
        name: req.name.as_deref().and_then(clean_name),
        coordinator: who.id.clone(),
        players,
        finished: vec![],
        status: MatchStatus::Open,
        started_at_ms: None,
        stopped_at_ms: None,
        names: Default::default(),
        invite_code: Some(Uuid::new_v4().simple().to_string()),
        created_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    remember_name(&mut m, &who);
    st.persist(&m).await?;
    st.matches.write().await.insert(m.id, m.clone());
    Ok((StatusCode::CREATED, Json(m)))
}

#[derive(serde::Deserialize)]
pub struct JoinQuery {
    code: String,
}

pub async fn join_match(
    State(st): St,
    AppWho(who): AppWho,
    Path(id): Path<Uuid>,
    axum::extract::Query(q): axum::extract::Query<JoinQuery>,
) -> Result<Json<MatchInfo>, AppError> {
    if !valid_id(&who.id) {
        return Err(AppError::BadRequest("invalid user id"));
    }
    let mut all = st.matches.write().await;
    let m = all.get_mut(&id).ok_or(AppError::NotFound)?;

    if m.invite_code.as_deref() != Some(q.code.as_str()) {
        return Err(AppError::NotFound);
    }
    if !m.players.contains(&who.id) {
        if m.started_at_ms.is_some() || m.status == MatchStatus::Ended {
            return Err(AppError::Conflict("match already started"));
        }
        if m.players.len() >= 16 {
            return Err(AppError::Conflict("match is full"));
        }
        m.players.push(who.id.clone());
        m.players.sort();
    }
    remember_name(m, &who);
    st.persist(m).await?;
    Ok(Json(redact(m.clone(), &who.id)))
}

#[derive(serde::Serialize)]
pub struct MatchView {
    #[serde(flatten)]
    info: MatchInfo,

    connected: Vec<String>,

    health: Vec<relay_common::PlayerHealth>,

    videos: Vec<String>,

    web_videos: Vec<String>,

    vod_videos: Vec<String>,

    processing: std::collections::HashMap<String, crate::state::Processing>,
}

pub async fn show_match(
    State(st): St,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MatchView>, AppError> {
    let info = get_match(&st, id).await?;
    ensure_participant(&info, &user)?;
    let connected = st.hub_connected(id);
    let health =
        relay_common::redact_health(st.health(id, &info.players), info.coordinator == user);
    let mut videos = Vec::new();
    let mut web_videos = Vec::new();
    let mut vod_videos = Vec::new();
    for p in &info.players {
        let dir = st.player_dir(id, p);
        if crate::finalize::has_video(&dir).await {
            videos.push(p.clone());
            if crate::finalize::has_web(&dir).await {
                web_videos.push(p.clone());
                if crate::finalize::has_vod(&dir).await {
                    vod_videos.push(p.clone());
                }
            }
        }
    }
    let processing = st.processing_of(id);
    Ok(Json(MatchView {
        info: redact(info, &user),
        connected,
        health,
        videos,
        web_videos,
        vod_videos,
        processing,
    }))
}

pub async fn me(Who(who): Who) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "user": who.id, "name": who.name }))
}

pub async fn create_session(
    State(st): St,
    parts: axum::http::request::Parts,
) -> Result<Json<SessionGrant>, AppError> {
    let token = bearer_token(&parts).ok_or(AppError::Unauthorized)?;
    let who = st
        .auth
        .authenticate_primary(token)
        .await
        .ok_or(AppError::Unauthorized)?;
    if !valid_id(&who.id) {
        return Err(AppError::BadRequest("invalid user id"));
    }
    let (session, expires_in) = st
        .auth
        .issue_session(&who)
        .ok_or(AppError::Internal("sessions are not configured".into()))?;
    Ok(Json(SessionGrant {
        session,
        expires_in,
        user: who.id,
        name: who.name,
    }))
}

#[derive(serde::Serialize)]
pub struct MatchListItem {
    #[serde(flatten)]
    info: MatchInfo,

    has_recording: bool,

    duration_secs: Option<u64>,

    has_thumb: bool,
}

async fn recording_summary(st: &AppState, m: &MatchInfo) -> (bool, Option<u64>, bool) {
    let (mut any, mut thumb, mut dur): (bool, bool, Option<u64>) = (false, false, None);
    for p in &m.players {
        let dir = st.player_dir(m.id, p);
        if crate::finalize::has_video(&dir).await {
            any = true;
            if tokio::fs::metadata(dir.join(crate::finalize::THUMB_FILE))
                .await
                .is_ok()
            {
                thumb = true;
            }
            if let Some(d) = crate::finalize::read_duration(&dir).await {
                dur = Some(dur.map_or(d, |x| x.max(d)));
            }
        } else if let Ok(segs) = list_segments(st, m.id, p).await {
            if !segs.is_empty() {
                any = true;
                let secs: u64 = segs.values().map(|&ms| ms as u64).sum::<u64>() / 1000;
                dur = Some(dur.map_or(secs, |x| x.max(secs)));
            }
        }
    }
    (
        any,
        if m.status == MatchStatus::Ended {
            dur
        } else {
            None
        },
        thumb,
    )
}

pub async fn list_matches(State(st): St, AuthUser(user): AuthUser) -> Json<Vec<MatchListItem>> {
    let mut mine: Vec<MatchInfo> = st
        .matches
        .read()
        .await
        .values()
        .filter(|m| ensure_participant(m, &user).is_ok())
        .cloned()
        .collect();
    mine.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    let mut out = Vec::with_capacity(mine.len());
    for m in mine {
        let (has_recording, duration_secs, has_thumb) = recording_summary(&st, &m).await;
        out.push(MatchListItem {
            info: redact(m, &user),
            has_recording,
            duration_secs,
            has_thumb,
        });
    }
    Json(out)
}

#[derive(serde::Deserialize)]
pub struct RenameReq {
    #[serde(default)]
    name: String,
}

pub async fn rename_match(
    State(st): St,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameReq>,
) -> Result<Json<MatchInfo>, AppError> {
    let mut all = st.matches.write().await;
    let m = all.get_mut(&id).ok_or(AppError::NotFound)?;
    if m.coordinator != user {
        return Err(AppError::Forbidden);
    }
    m.name = clean_name(&req.name);
    st.persist(m).await?;
    Ok(Json(redact(m.clone(), &user)))
}

pub async fn get_thumb(
    State(st): St,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let m = get_match(&st, id).await?;
    ensure_participant(&m, &user)?;
    for p in &m.players {
        if let Ok(bytes) =
            tokio::fs::read(st.player_dir(id, p).join(crate::finalize::THUMB_FILE)).await
        {
            return Ok((
                [
                    (header::CONTENT_TYPE, "image/jpeg"),
                    (header::CACHE_CONTROL, "private, max-age=3600"),
                ],
                bytes,
            ));
        }
    }
    Err(AppError::NotFound)
}

pub async fn put_segment(
    State(st): St,
    AuthUser(user): AuthUser,
    Path((id, pid, file)): Path<(Uuid, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let n = playlist::parse_segment(&file).ok_or(AppError::BadRequest("bad segment index"))?;
    if pid != user {
        return Err(AppError::Forbidden);
    }
    let m = get_match(&st, id).await?;
    if !m.players.contains(&user) {
        return Err(AppError::Forbidden);
    }
    if m.status == MatchStatus::Ended {
        return Err(AppError::Conflict("match ended"));
    }
    if body.is_empty() {
        return Err(AppError::BadRequest("empty body"));
    }
    if let Some(expected) = headers.get("x-sha256").and_then(|v| v.to_str().ok()) {
        if !hex::encode(Sha256::digest(&body)).eq_ignore_ascii_case(expected) {
            return Err(AppError::BadRequest("sha256 mismatch"));
        }
    }

    let dir = st.player_dir(id, &pid);
    tokio::fs::create_dir_all(&dir).await?;

    if let Some(ms) = headers
        .get("x-segment-duration-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|ms| (1..=60_000).contains(ms))
    {
        tokio::fs::write(dir.join(format!("{n:08}.dur")), ms.to_string()).await?;
    }
    let tmp = dir.join(format!(".{n:08}.{}.tmp", Uuid::new_v4()));
    tokio::fs::write(&tmp, &body).await?;
    if let Err(e) = tokio::fs::rename(&tmp, dir.join(playlist::segment_file(n))).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_segment(
    State(st): St,
    AuthUser(user): AuthUser,
    Path((id, pid, file)): Path<(Uuid, String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let n = playlist::parse_segment(&file).ok_or(AppError::NotFound)?;
    if !valid_id(&pid) {
        return Err(AppError::NotFound);
    }
    let m = get_match(&st, id).await?;
    ensure_participant(&m, &user)?;
    let bytes = tokio::fs::read(st.player_dir(id, &pid).join(playlist::segment_file(n))).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "video/mp2t"),
            (header::CACHE_CONTROL, "private, max-age=3600, immutable"),
        ],
        bytes,
    ))
}

async fn list_segments(st: &AppState, id: Uuid, pid: &str) -> Result<BTreeMap<u64, u32>, AppError> {
    let dir = st.player_dir(id, pid);
    let mut map = BTreeMap::new();
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
        Err(e) => return Err(e.into()),
    };
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();

        if name.ends_with(".ts") && !name.starts_with('.') {
            if let Some(n) = playlist::parse_segment(&name) {
                map.insert(n, relay_common::SEGMENT_SECONDS * 1000);
            }
        }
    }
    for (n, ms) in map.iter_mut() {
        if let Ok(s) = tokio::fs::read_to_string(dir.join(format!("{n:08}.dur"))).await {
            if let Ok(v) = s.trim().parse() {
                *ms = v;
            }
        }
    }
    Ok(map)
}

#[derive(serde::Deserialize)]
pub struct VideoQuery {
    #[serde(default)]
    download: Option<String>,

    #[serde(default)]
    q: Option<String>,
}

pub async fn get_video(
    State(st): St,
    AuthUser(user): AuthUser,
    Path((id, pid)): Path<(Uuid, String)>,
    axum::extract::Query(q): axum::extract::Query<VideoQuery>,
    headers: HeaderMap,
) -> Result<axum::response::Response, AppError> {
    if !valid_id(&pid) {
        return Err(AppError::NotFound);
    }
    let m = get_match(&st, id).await?;
    ensure_participant(&m, &user)?;
    if !m.players.contains(&pid) {
        return Err(AppError::NotFound);
    }
    let dir = st.player_dir(id, &pid);

    let download = matches!(q.download.as_deref(), Some("1") | Some("true"));
    let want_web =
        !download && q.q.as_deref() == Some("web") && crate::finalize::has_web(&dir).await;
    let path = dir.join(if want_web {
        crate::finalize::WEB_FILE
    } else {
        crate::finalize::VIDEO_FILE
    });
    let name = download.then(|| {
        format!(
            "relay-{}-{}.mp4",
            id.simple().to_string().get(..8).unwrap_or("partita"),
            pid
        )
    });
    serve_file(&path, &headers, if want_web { "-w" } else { "" }, name).await
}

async fn serve_file(
    path: &std::path::Path,
    headers: &HeaderMap,
    etag_tag: &str,
    name: Option<String>,
) -> Result<axum::response::Response, AppError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let meta = file.metadata().await?;
    let total = meta.len();
    if total == 0 {
        return Err(AppError::NotFound);
    }

    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let etag = format!("\"{total:x}-{mtime:x}{etag_tag}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag.as_str())
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable".to_string(),
                ),
            ],
        )
            .into_response());
    }

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, total));
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
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let len = end - start + 1;
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(
        (file.take(len), vec![0u8; 256 * 1024]),
        |(mut f, mut buf)| async move {
            match f.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => Some((
                    Ok::<_, std::io::Error>(Bytes::copy_from_slice(&buf[..n])),
                    (f, buf),
                )),
                Err(e) => Some((Err(e), (f, buf))),
            }
        },
    ));
    let mut resp = (status, body).into_response();
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

pub async fn get_vod(
    State(st): St,
    AuthUser(user): AuthUser,
    Path((id, pid, file)): Path<(Uuid, String, String)>,
    headers: HeaderMap,
) -> Result<axum::response::Response, AppError> {
    if !valid_id(&pid) || (file != crate::finalize::VOD_INDEX && file != crate::finalize::VOD_MEDIA)
    {
        return Err(AppError::NotFound);
    }
    let m = get_match(&st, id).await?;
    ensure_participant(&m, &user)?;
    if !m.players.contains(&pid) {
        return Err(AppError::NotFound);
    }
    let path = st
        .player_dir(id, &pid)
        .join(crate::finalize::VOD_DIR)
        .join(&file);
    if file == crate::finalize::VOD_MEDIA {
        return serve_file(&path, &headers, "-v", None).await;
    }
    let body = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        body,
    )
        .into_response())
}

fn parse_range(v: &str, total: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = v.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let r = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(Err(()));
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e: u64 = if b.is_empty() {
            total - 1
        } else {
            b.parse::<u64>().ok()?.min(total - 1)
        };
        (s, e)
    };
    Some(if r.0 > r.1 || r.0 >= total {
        Err(())
    } else {
        Ok(r)
    })
}

#[derive(serde::Deserialize)]
struct AppRelease {
    version: String,
    file: String,
    sha256: String,
    size: u64,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    published_at: String,
}

async fn read_manifest(st: &AppState, manifest: &str) -> Option<AppRelease> {
    let bytes = tokio::fs::read(st.data_dir.join("app").join(manifest))
        .await
        .ok()?;
    let r: AppRelease = serde_json::from_slice(&bytes).ok()?;

    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            && !s.starts_with('.')
    };
    (plain(&r.file) && plain(&r.version)).then_some(r)
}

fn release_json(r: &AppRelease, download_url: &str) -> serde_json::Value {
    serde_json::json!({
        "version": r.version,
        "size": r.size,
        "sha256": r.sha256,
        "signature": r.signature,
        "notes": r.notes,
        "published_at": r.published_at,
        "download_url": download_url,
    })
}

async fn download_response(
    st: &AppState,
    manifest: &str,
    content_type: &str,
    filename: String,
) -> Result<axum::response::Response, AppError> {
    let r = read_manifest(st, manifest)
        .await
        .ok_or(AppError::NotFound)?;
    let file = tokio::fs::File::open(st.data_dir.join("app").join(&r.file))
        .await
        .map_err(|_| AppError::NotFound)?;
    let len = file.metadata().await?.len();
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(
        (file, vec![0u8; 256 * 1024]),
        |(mut f, mut buf)| async move {
            use tokio::io::AsyncReadExt;
            match f.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => Some((
                    Ok::<_, std::io::Error>(Bytes::copy_from_slice(&buf[..n])),
                    (f, buf),
                )),
                Err(e) => Some((Err(e), (f, buf))),
            }
        },
    ));
    let mut resp = body.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    h.insert(header::CONTENT_LENGTH, len.into());
    h.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{filename}\"")
            .parse()
            .unwrap(),
    );
    h.insert(
        header::CACHE_CONTROL,
        "public, max-age=300".parse().unwrap(),
    );
    Ok(resp)
}

pub async fn app_latest(State(st): St) -> Result<impl IntoResponse, AppError> {
    let r = read_manifest(&st, "latest.json")
        .await
        .ok_or(AppError::NotFound)?;
    Ok((
        [(header::CACHE_CONTROL, "no-cache")],
        Json(release_json(&r, "/api/app/download")),
    ))
}

pub async fn app_download(State(st): St) -> Result<axum::response::Response, AppError> {
    let r = read_manifest(&st, "latest.json")
        .await
        .ok_or(AppError::NotFound)?;
    download_response(
        &st,
        "latest.json",
        "application/vnd.microsoft.portable-executable",
        format!("Relay-{}.exe", r.version),
    )
    .await
}

pub async fn ffmpeg_latest(State(st): St) -> Result<impl IntoResponse, AppError> {
    let r = read_manifest(&st, "ffmpeg.json")
        .await
        .ok_or(AppError::NotFound)?;
    Ok((
        [(header::CACHE_CONTROL, "no-cache")],
        Json(release_json(&r, "/api/app/ffmpeg/download")),
    ))
}

pub async fn ffmpeg_download(State(st): St) -> Result<axum::response::Response, AppError> {
    let r = read_manifest(&st, "ffmpeg.json")
        .await
        .ok_or(AppError::NotFound)?;
    download_response(
        &st,
        "ffmpeg.json",
        "application/zip",
        format!("relay-ffmpeg-{}.zip", r.version),
    )
    .await
}

pub async fn capture_latest(State(st): St) -> Result<impl IntoResponse, AppError> {
    let r = read_manifest(&st, "capture.json")
        .await
        .ok_or(AppError::NotFound)?;
    Ok((
        [(header::CACHE_CONTROL, "no-cache")],
        Json(release_json(&r, "/api/app/capture/download")),
    ))
}

pub async fn capture_download(State(st): St) -> Result<axum::response::Response, AppError> {
    let r = read_manifest(&st, "capture.json")
        .await
        .ok_or(AppError::NotFound)?;
    download_response(
        &st,
        "capture.json",
        "application/vnd.microsoft.portable-executable",
        format!("relay-capture-{}.exe", r.version),
    )
    .await
}

pub async fn get_playlist(
    State(st): St,
    AuthUser(user): AuthUser,
    Path((id, pid)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, AppError> {
    if !valid_id(&pid) {
        return Err(AppError::NotFound);
    }
    let m = get_match(&st, id).await?;
    ensure_participant(&m, &user)?;
    if !m.players.contains(&pid) {
        return Err(AppError::NotFound);
    }
    let segs = list_segments(&st, id, &pid).await?;
    let body = playlist::build(&segs, m.status == MatchStatus::Ended);
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    ))
}

pub async fn finish_player(
    State(st): St,
    AuthUser(user): AuthUser,
    Path((id, pid)): Path<(Uuid, String)>,
) -> Result<Json<MatchInfo>, AppError> {
    if pid != user {
        return Err(AppError::Forbidden);
    }
    let mut all = st.matches.write().await;
    let m = all.get_mut(&id).ok_or(AppError::NotFound)?;
    if !m.players.contains(&user) {
        return Err(AppError::Forbidden);
    }
    if !m.finished.contains(&user) {
        m.finished.push(user);
    }
    if m.players.iter().all(|p| m.finished.contains(p)) {
        m.status = MatchStatus::Ended;
    }
    st.persist(m).await?;
    if m.status == MatchStatus::Ended {
        broadcast_state(&st, m);
        tokio::spawn(crate::finalize::run(st.clone(), id));
    }
    Ok(Json(m.clone()))
}

pub async fn end_match(
    State(st): St,
    AppUser(user): AppUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MatchInfo>, AppError> {
    let mut all = st.matches.write().await;
    let m = all.get_mut(&id).ok_or(AppError::NotFound)?;
    if m.coordinator != user {
        return Err(AppError::Forbidden);
    }
    m.status = MatchStatus::Ended;
    st.persist(m).await?;
    broadcast_state(&st, m);
    tokio::spawn(crate::finalize::run(st.clone(), id));
    Ok(Json(m.clone()))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(serde::Deserialize, Default)]
pub struct StartQuery {
    #[serde(default)]
    pub force: bool,
}

pub async fn start_match(
    State(st): St,
    AppUser(user): AppUser,
    Path(id): Path<Uuid>,
    axum::extract::Query(q): axum::extract::Query<StartQuery>,
) -> Result<Json<MatchInfo>, AppError> {
    let mut all = st.matches.write().await;
    let m = all.get_mut(&id).ok_or(AppError::NotFound)?;
    if m.coordinator != user {
        return Err(AppError::Forbidden);
    }
    if m.started_at_ms.is_some() {
        return Err(AppError::Conflict("already started"));
    }
    if m.status == MatchStatus::Ended {
        return Err(AppError::Conflict("match ended"));
    }
    if m.players.is_empty() {
        return Err(AppError::Conflict("no players yet"));
    }
    if !q.force {
        let connected = st.hub_connected(id);
        if !m.players.iter().all(|p| connected.contains(p)) {
            return Err(AppError::Conflict("not all players are connected"));
        }
        if st.health(id, &m.players).iter().any(|h| !h.ready) {
            return Err(AppError::Conflict("not all players are ready"));
        }
    }
    let at = next_boundary(0, now_ms() + START_LEAD_MS);
    m.started_at_ms = Some(at);
    st.persist(m).await?;
    st.hub_broadcast(id, &ServerMsg::Start { at_ms: at });
    Ok(Json(m.clone()))
}

pub async fn stop_match(
    State(st): St,
    AppUser(user): AppUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MatchInfo>, AppError> {
    let mut all = st.matches.write().await;
    let m = all.get_mut(&id).ok_or(AppError::NotFound)?;
    if m.coordinator != user {
        return Err(AppError::Forbidden);
    }
    let start = m.started_at_ms.ok_or(AppError::Conflict("not started"))?;
    if m.stopped_at_ms.is_some() {
        return Err(AppError::Conflict("already stopped"));
    }
    let at = next_boundary(start, now_ms() + 1_000);
    m.stopped_at_ms = Some(at);
    st.persist(m).await?;
    st.hub_broadcast(id, &ServerMsg::Stop { at_ms: at });
    Ok(Json(m.clone()))
}

pub async fn speedtest(
    AuthUser(_): AuthUser,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    use futures_util::StreamExt;
    const MAX: u64 = 64 * 1024 * 1024;
    let mut n: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        n += chunk.map_err(|_| AppError::BadRequest("read error"))?.len() as u64;
        if n > MAX {
            return Err(AppError::BadRequest("too large"));
        }
    }
    Ok(Json(serde_json::json!({ "bytes": n })))
}

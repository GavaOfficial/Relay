use std::{
    collections::BTreeMap,
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
    time::SystemTime,
};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
    Extension, Json, Router,
};
use relay_common::{clean_name, valid_id};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::{
    auth::{AppUser, AuthUser},
    error::AppError,
    state::AppState,
};

type St = State<Arc<AppState>>;

pub const MAX_PARTS: u32 = 256;
const MAX_ASSET_BYTES: usize = 5 * 1024 * 1024;
const MAX_TRACKS: usize = 1000;
const MAX_EXTRA_BYTES: usize = 4096;
const MAX_ALBUMS: usize = 200;

#[derive(Debug, Clone, Copy)]
pub struct Engine(pub &'static str);

pub fn router(engine: &'static str) -> Router<Arc<AppState>> {
    Router::new()
        .route("/best", get(best))
        .route("/catalog", put(put_catalog))
        .route("/profile", get(get_profile).put(put_profile))
        .route("/mods", get(list_mods))
        .route("/mods/{key}", get(show_mod))
        .route("/mods/{key}/gamebanana", post(set_gamebanana))
        .route("/mods/{key}/assets/{name}", get(get_asset).put(put_asset))
        .route("/uploads/{upload}/{part}", put(put_part))
        .route("/uploads/{upload}/finish", post(finish_upload))
        .route("/clips/{id}/video.mp4", get(clip_video))
        .route("/clips/{id}/thumb.jpg", get(clip_thumb))
        .layer(Extension(Engine(engine)))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Contributor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f32>,
    #[serde(default)]
    pub difficulties: Vec<String>,
    #[serde(default)]
    pub ratings: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub contributors: Vec<Contributor>,
    #[serde(default)]
    pub has_icon: bool,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongMod {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamebanana_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gb_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gb_author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gb_cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<Catalog>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongClip {
    pub id: Uuid,
    pub mod_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song_id: Option<String>,
    pub song: String,
    pub difficulty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation: Option<String>,
    pub score: i64,
    #[serde(default)]
    pub accuracy: Option<f32>,
    #[serde(default)]
    pub misses: u32,
    pub duration_ms: u64,
    pub recorded_at: u64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub has_thumb: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Library {
    mods: Vec<SongMod>,
    clips: Vec<SongClip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<serde_json::Value>,
}

fn small_object(v: Option<serde_json::Value>) -> Option<serde_json::Value> {
    v.filter(|v| v.is_object() && v.to_string().len() <= MAX_EXTRA_BYTES)
}

fn lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(Default::default)
}

pub fn mod_key(name: &str) -> String {
    let mut key = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else if !key.ends_with('-') {
            key.push('-');
        }
    }
    let key: String = key.trim_matches('-').chars().take(60).collect();
    let key = key.trim_matches('-').to_string();
    if key.is_empty() {
        "mod".into()
    } else {
        key
    }
}

fn variation_of(v: Option<&str>) -> &str {
    match v.map(str::trim) {
        Some(v) if !v.is_empty() && !v.eq_ignore_ascii_case("default") => v,
        _ => "",
    }
}

fn same_chart(c: &SongClip, key: &str, song: &str, difficulty: &str, variation: Option<&str>) -> bool {
    c.mod_key == key
        && c.song.eq_ignore_ascii_case(song)
        && c.difficulty.eq_ignore_ascii_case(difficulty)
        && variation_of(c.variation.as_deref()).eq_ignore_ascii_case(variation_of(variation))
}

fn user_dir(st: &AppState, engine: Engine, user: &str) -> Result<PathBuf, AppError> {
    if !valid_id(user) {
        return Err(AppError::Forbidden);
    }
    Ok(st.data_dir.join(engine.0).join(user))
}

async fn load(dir: &FsPath) -> Library {
    match tokio::fs::read(dir.join("library.json")).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Library::default(),
    }
}

async fn save(dir: &FsPath, lib: &Library) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let tmp = dir.join(format!(".library.{}.tmp", Uuid::new_v4()));
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(lib)?).await?;
    tokio::fs::rename(&tmp, dir.join("library.json")).await
}

fn clip_path(dir: &FsPath, id: Uuid) -> PathBuf {
    dir.join("clips").join(format!("{id}.mp4"))
}

fn thumb_path(dir: &FsPath, id: Uuid) -> PathBuf {
    dir.join("clips").join(format!("{id}.jpg"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Deserialize)]
pub struct BestQuery {
    pub mod_name: String,
    pub song: String,
    pub difficulty: String,
    #[serde(default)]
    pub variation: Option<String>,
    #[serde(default)]
    pub aliases: Option<String>,
}

fn split_aliases(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split('|')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .take(8)
        .map(String::from)
        .collect()
}

fn merge_aliases(lib: &mut Library, key: &str, name: &str, aliases: &[String]) -> bool {
    let old: Vec<String> = aliases
        .iter()
        .map(|a| mod_key(a))
        .filter(|k| k != key && lib.mods.iter().any(|m| m.key == *k))
        .collect();
    if old.is_empty() {
        return false;
    }
    if !lib.mods.iter().any(|m| m.key == key) {
        lib.mods.push(new_mod(key.to_string(), name.to_string()));
    }
    for k in &old {
        let Some(pos) = lib.mods.iter().position(|m| m.key == *k) else {
            continue;
        };
        let gone = lib.mods.remove(pos);
        let target = lib.mods.iter_mut().find(|m| m.key == key).expect("appena aggiunta");
        if target.gamebanana_url.is_none() && gone.gamebanana_url.is_some() {
            target.gamebanana_url = gone.gamebanana_url;
            target.gb_name = gone.gb_name;
            target.gb_author = gone.gb_author;
            target.gb_cover_url = gone.gb_cover_url;
        }
        if target.catalog.is_none() {
            target.catalog = gone.catalog;
        }
        for c in lib.clips.iter_mut().filter(|c| c.mod_key == *k) {
            c.mod_key = key.to_string();
        }
    }
    let mut best: BTreeMap<(String, String, String), (i64, Uuid)> = BTreeMap::new();
    for c in lib.clips.iter().filter(|c| c.mod_key == key && !c.archived) {
        let chart = (
            c.song.to_lowercase(),
            c.difficulty.to_lowercase(),
            variation_of(c.variation.as_deref()).to_lowercase(),
        );
        let e = best.entry(chart).or_insert((c.score, c.id));
        if c.score > e.0 {
            *e = (c.score, c.id);
        }
    }
    let keep: std::collections::HashSet<Uuid> = best.values().map(|(_, id)| *id).collect();
    for c in lib.clips.iter_mut().filter(|c| c.mod_key == key && !c.archived) {
        if !keep.contains(&c.id) {
            c.archived = true;
        }
    }
    true
}

pub async fn best(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
    Query(q): Query<BestQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let mut lib = load(&dir).await;
    let key = mod_key(&q.mod_name);
    merge_aliases(&mut lib, &key, &q.mod_name, &split_aliases(q.aliases.as_deref()));
    let score = lib
        .clips
        .iter()
        .filter(|c| {
            !c.archived && same_chart(c, &key, &q.song, &q.difficulty, q.variation.as_deref())
        })
        .map(|c| c.score)
        .max();
    Ok(Json(serde_json::json!({ "score": score })))
}

pub async fn put_part(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AppUser(user): AppUser,
    Path((upload, part)): Path<(Uuid, u32)>,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    if part >= MAX_PARTS {
        return Err(AppError::BadRequest("troppi pezzi"));
    }
    if body.is_empty() {
        return Err(AppError::BadRequest("pezzo vuoto"));
    }
    let dir = user_dir(&st, engine, &user)?
        .join("uploads")
        .join(upload.to_string());
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = dir.join(format!(".{part:05}.tmp"));
    tokio::fs::write(&tmp, &body).await?;
    tokio::fs::rename(&tmp, dir.join(format!("{part:05}"))).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct FinishInput {
    pub mod_name: String,
    #[serde(default)]
    pub song_id: Option<String>,
    pub song: String,
    pub difficulty: String,
    #[serde(default)]
    pub variation: Option<String>,
    pub score: i64,
    #[serde(default)]
    pub accuracy: Option<f32>,
    #[serde(default)]
    pub misses: u32,
    pub duration_ms: u64,
    pub parts: u32,
    pub size: u64,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
}

pub async fn finish_upload(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AppUser(user): AppUser,
    Path(upload): Path<Uuid>,
    Json(input): Json<FinishInput>,
) -> Result<(StatusCode, Json<SongClip>), AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let up_dir = dir.join("uploads").join(upload.to_string());
    let result = finish_inner(&st, &dir, &up_dir, input).await;
    let _ = tokio::fs::remove_dir_all(&up_dir).await;
    result
}

fn new_mod(key: String, name: String) -> SongMod {
    SongMod {
        key,
        name,
        gamebanana_url: None,
        gb_name: None,
        gb_author: None,
        gb_cover_url: None,
        catalog: None,
    }
}

fn clean_id(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 80)
        .map(|s| s.chars().filter(|c| !c.is_control()).collect())
}

async fn finish_inner(
    st: &Arc<AppState>,
    dir: &FsPath,
    up_dir: &FsPath,
    input: FinishInput,
) -> Result<(StatusCode, Json<SongClip>), AppError> {
    let mod_name =
        clean_name(&input.mod_name).ok_or(AppError::BadRequest("nome mod non valido"))?;
    let song = clean_name(&input.song).ok_or(AppError::BadRequest("nome canzone non valido"))?;
    let difficulty = clean_name(&input.difficulty).unwrap_or_else(|| "normal".into());
    let variation = Some(variation_of(input.variation.as_deref()))
        .filter(|v| !v.is_empty())
        .and_then(clean_name);
    if input.parts == 0 || input.parts > MAX_PARTS {
        return Err(AppError::BadRequest("numero di pezzi non valido"));
    }
    let key = mod_key(&mod_name);

    let _guard = lock().lock().await;
    let mut lib = load(dir).await;
    if merge_aliases(&mut lib, &key, &mod_name, &input.aliases) {
        save(dir, &lib).await?;
    }
    let is_chart = |c: &SongClip| same_chart(c, &key, &song, &difficulty, variation.as_deref());
    let current_best = lib
        .clips
        .iter()
        .filter(|c| !c.archived && is_chart(c))
        .map(|c| c.score)
        .max();
    if current_best.is_some_and(|b| input.score <= b) {
        return Err(AppError::Conflict("not a record"));
    }

    let id = Uuid::new_v4();
    let clips_dir = dir.join("clips");
    tokio::fs::create_dir_all(&clips_dir).await?;
    let tmp = clips_dir.join(format!(".{id}.tmp"));
    {
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::File::create(&tmp).await?;
        for part in 0..input.parts {
            let bytes = tokio::fs::read(up_dir.join(format!("{part:05}")))
                .await
                .map_err(|_| AppError::BadRequest("manca un pezzo della clip"))?;
            out.write_all(&bytes).await?;
        }
        out.flush().await?;
    }
    if tokio::fs::metadata(&tmp).await?.len() != input.size {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(AppError::BadRequest("dimensione della clip diversa"));
    }
    tokio::fs::rename(&tmp, clip_path(dir, id)).await?;

    for c in lib.clips.iter_mut() {
        if !c.archived && is_chart(c) {
            c.archived = true;
        }
    }
    if !lib.mods.iter().any(|m| m.key == key) {
        lib.mods.push(new_mod(key.clone(), mod_name));
    }
    let clip = SongClip {
        id,
        mod_key: key,
        song_id: clean_id(input.song_id.as_deref()),
        song,
        difficulty,
        variation,
        score: input.score,
        accuracy: input.accuracy.filter(|a| a.is_finite()),
        misses: input.misses,
        duration_ms: input.duration_ms,
        recorded_at: now_secs(),
        archived: false,
        has_thumb: false,
        extra: small_object(input.extra),
    };
    lib.clips.push(clip.clone());
    save(dir, &lib).await?;
    drop(_guard);

    if let Some(ffmpeg) = st.ffmpeg.clone() {
        let dir = dir.to_path_buf();
        let at = (input.duration_ms as f64 / 1000.0 * 0.4).min(60.0);
        tokio::spawn(async move { make_thumb(&ffmpeg, &dir, id, at).await });
    }
    Ok((StatusCode::CREATED, Json(clip)))
}

async fn make_thumb(ffmpeg: &FsPath, dir: &FsPath, id: Uuid, at: f64) {
    let tmp = dir.join("clips").join(format!(".{id}.thumb.jpg"));
    let ok = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-ss"])
        .arg(format!("{at:.1}"))
        .arg("-i")
        .arg(clip_path(dir, id))
        .args(["-frames:v", "1", "-vf", "scale=640:-2", "-q:v", "4"])
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = tokio::fs::remove_file(&tmp).await;
        return;
    }
    if tokio::fs::rename(&tmp, thumb_path(dir, id)).await.is_err() {
        return;
    }
    let _guard = lock().lock().await;
    let mut lib = load(dir).await;
    if let Some(c) = lib.clips.iter_mut().find(|c| c.id == id) {
        c.has_thumb = true;
        let _ = save(dir, &lib).await;
    }
}

fn short(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    }
}

fn clean_url(u: Option<&str>) -> Option<String> {
    u.map(str::trim)
        .filter(|u| (u.starts_with("https://") || u.starts_with("http://")) && u.len() <= 300)
        .map(String::from)
}

fn sanitize_catalog(mut c: Catalog) -> Catalog {
    c.title = c.title.as_deref().map(|t| short(t, 120)).filter(|t| !t.is_empty());
    c.extra = small_object(c.extra.take());
    c.description = c
        .description
        .as_deref()
        .map(|d| short(d, 1500))
        .filter(|d| !d.is_empty());
    c.version = c.version.as_deref().map(|v| short(v, 40));
    c.contributors.truncate(50);
    for p in c.contributors.iter_mut() {
        p.name = short(&p.name, 80);
        p.role = p.role.as_deref().map(|r| short(r, 160));
        p.url = clean_url(p.url.as_deref());
    }
    c.contributors.retain(|p| !p.name.is_empty());
    c.albums.truncate(MAX_ALBUMS);
    for a in c.albums.iter_mut() {
        a.id = short(&a.id, 80);
        a.name = short(&a.name, 120);
        a.artists = a.artists.iter().take(10).map(|x| short(x, 80)).collect();
        a.art = a.art.take().filter(|n| asset_name_ok(n));
    }
    c.tracks.truncate(MAX_TRACKS);
    for t in c.tracks.iter_mut() {
        t.id = short(&t.id, 80);
        t.name = short(&t.name, 120);
        t.artist = t.artist.as_deref().map(|x| short(x, 120));
        t.album = t.album.as_deref().map(|x| short(x, 80));
        t.variation = Some(variation_of(t.variation.as_deref()).to_string()).filter(|v| !v.is_empty());
        t.bpm = t.bpm.filter(|b| b.is_finite() && *b > 0.0 && *b < 2000.0);
        t.difficulties = t.difficulties.iter().take(12).map(|d| short(d, 40)).collect();
        t.ratings = std::mem::take(&mut t.ratings)
            .into_iter()
            .take(12)
            .map(|(k, v)| (short(&k, 40), v.min(99)))
            .collect();
        t.icon = t.icon.take().filter(|n| asset_name_ok(n));
        t.extra = small_object(t.extra.take());
        t.color = t.color.take().filter(|c| {
            c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())
        });
    }
    c
}

#[derive(Deserialize)]
pub struct CatalogInput {
    pub mod_name: String,
    pub catalog: Catalog,
    #[serde(default)]
    pub aliases: Vec<String>,
}

pub async fn put_catalog(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AppUser(user): AppUser,
    Json(input): Json<CatalogInput>,
) -> Result<Json<serde_json::Value>, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let name = clean_name(&input.mod_name).ok_or(AppError::BadRequest("nome mod non valido"))?;
    let key = mod_key(&name);
    let catalog = sanitize_catalog(input.catalog);
    let _guard = lock().lock().await;
    let mut lib = load(&dir).await;
    let aliases: Vec<String> = input.aliases.into_iter().take(8).collect();
    merge_aliases(&mut lib, &key, &name, &aliases);
    match lib.mods.iter_mut().find(|m| m.key == key) {
        Some(m) => {
            m.name = name;
            m.catalog = Some(catalog);
        }
        None => {
            let mut m = new_mod(key.clone(), name);
            m.catalog = Some(catalog);
            lib.mods.push(m);
        }
    }
    save(&dir, &lib).await?;
    Ok(Json(serde_json::json!({ "key": key })))
}

pub async fn put_profile(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AppUser(user): AppUser,
    Json(profile): Json<serde_json::Value>,
) -> Result<StatusCode, AppError> {
    let profile = small_object(Some(profile)).ok_or(AppError::BadRequest("profilo non valido"))?;
    let dir = user_dir(&st, engine, &user)?;
    let _guard = lock().lock().await;
    let mut lib = load(&dir).await;
    lib.profile = Some(profile);
    save(&dir, &lib).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_profile(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    Ok(Json(load(&dir).await.profile.unwrap_or(serde_json::Value::Null)))
}

fn asset_name_ok(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 100
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn asset_path(dir: &FsPath, key: &str, name: &str) -> Result<PathBuf, AppError> {
    if !asset_name_ok(name) || key != mod_key(key) {
        return Err(AppError::BadRequest("nome immagine non valido"));
    }
    Ok(dir.join("assets").join(key).join(name))
}

pub async fn put_asset(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AppUser(user): AppUser,
    Path((key, name)): Path<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    if body.is_empty() || body.len() > MAX_ASSET_BYTES {
        return Err(AppError::BadRequest("immagine vuota o troppo grande"));
    }
    if !body.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(AppError::BadRequest("serve un'immagine PNG"));
    }
    let dir = user_dir(&st, engine, &user)?;
    let path = asset_path(&dir, &key, &name)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, &body).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_asset(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
    Path((key, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let bytes = tokio::fs::read(asset_path(&dir, &key, &name)?)
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        bytes,
    ))
}

#[derive(Serialize)]
pub struct ModSummary {
    #[serde(flatten)]
    pub info: SongMod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub has_icon: bool,
    pub tracks: usize,
    pub songs: usize,
    pub clips: usize,
    pub last_at: u64,
    pub preview_clip: Option<Uuid>,
}

pub async fn list_mods(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
) -> Result<Json<Vec<ModSummary>>, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let lib = load(&dir).await;
    let mut out: Vec<ModSummary> = lib
        .mods
        .iter()
        .filter_map(|m| {
            let clips: Vec<&SongClip> = lib.clips.iter().filter(|c| c.mod_key == m.key).collect();
            let best: Vec<&&SongClip> = clips.iter().filter(|c| !c.archived).collect();
            if best.is_empty() && m.catalog.is_none() {
                return None;
            }
            let latest = best.iter().max_by_key(|c| c.recorded_at).copied();
            let mut info = m.clone();
            let catalog = info.catalog.take();
            Some(ModSummary {
                info,
                title: catalog.as_ref().and_then(|c| c.title.clone()),
                has_icon: catalog.as_ref().is_some_and(|c| c.has_icon),
                tracks: catalog.as_ref().map_or(0, |c| c.tracks.len()),
                songs: best.len(),
                clips: clips.len(),
                last_at: latest.map(|c| c.recorded_at).unwrap_or(0),
                preview_clip: best
                    .iter()
                    .filter(|c| c.has_thumb)
                    .max_by_key(|c| c.recorded_at)
                    .map(|c| c.id),
            })
        })
        .collect();
    out.sort_by_key(|m| (m.songs == 0, std::cmp::Reverse(m.last_at), m.info.name.to_lowercase()));
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct SongEntry {
    pub song: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_id: Option<String>,
    pub difficulty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variation: Option<String>,
    pub best: SongClip,
    pub archive: Vec<SongClip>,
}

#[derive(Serialize)]
pub struct ModDetail {
    #[serde(rename = "mod")]
    pub info: SongMod,
    pub songs: Vec<SongEntry>,
}

pub async fn show_mod(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
    Path(key): Path<String>,
) -> Result<Json<ModDetail>, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let lib = load(&dir).await;
    let info = lib
        .mods
        .iter()
        .find(|m| m.key == key)
        .cloned()
        .ok_or(AppError::NotFound)?;
    let mut charts: BTreeMap<(String, String, String), Vec<SongClip>> = BTreeMap::new();
    for c in lib.clips.iter().filter(|c| c.mod_key == key) {
        charts
            .entry((
                c.song.to_lowercase(),
                c.difficulty.to_lowercase(),
                variation_of(c.variation.as_deref()).to_lowercase(),
            ))
            .or_default()
            .push(c.clone());
    }
    let mut songs: Vec<SongEntry> = charts
        .into_values()
        .filter_map(|mut clips| {
            clips.sort_by(|a, b| b.score.cmp(&a.score).then(b.recorded_at.cmp(&a.recorded_at)));
            let pos = clips.iter().position(|c| !c.archived)?;
            let best = clips.remove(pos);
            Some(SongEntry {
                song: best.song.clone(),
                song_id: best.song_id.clone(),
                difficulty: best.difficulty.clone(),
                variation: best.variation.clone(),
                best,
                archive: clips,
            })
        })
        .collect();
    songs.sort_by_key(|s| std::cmp::Reverse(s.best.recorded_at));
    Ok(Json(ModDetail { info, songs }))
}

pub fn parse_gamebanana(url: &str) -> Option<(&'static str, &'static str, u64)> {
    let rest = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let rest = rest.strip_prefix("gamebanana.com/")?;
    let mut parts = rest.split(['/', '?', '#']);
    let section = parts.next()?;
    let id: u64 = parts.next()?.parse().ok()?;
    let (section, model) = match section {
        "mods" => ("mods", "Mod"),
        "wips" => ("wips", "Wip"),
        "tools" => ("tools", "Tool"),
        _ => return None,
    };
    Some((section, model, id))
}

pub fn parse_gamejolt(url: &str) -> Option<(String, u64)> {
    let rest = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let rest = rest.strip_prefix("gamejolt.com/games/")?;
    let mut parts = rest.split(['/', '?', '#']);
    let slug = parts.next()?.to_string();
    let id: u64 = parts.next()?.parse().ok()?;
    (!slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then_some((slug, id))
}

#[derive(Deserialize)]
struct GjResponse {
    payload: GjPayload,
}

#[derive(Deserialize)]
struct GjPayload {
    game: GjGame,
}

#[derive(Deserialize)]
struct GjGame {
    title: Option<String>,
    developer: Option<GjDeveloper>,
    thumbnail_media_item: Option<GjMedia>,
    header_media_item: Option<GjMedia>,
}

#[derive(Deserialize)]
struct GjDeveloper {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct GjMedia {
    img_url: Option<String>,
}

type Fetched = (String, Option<String>, Option<String>, Option<String>);

async fn fetch_gamejolt(slug: &str, id: u64) -> Result<Fetched, AppError> {
    let api = format!("https://gamejolt.com/site-api/web/discover/games/{id}");
    let game = reqwest::Client::new()
        .get(&api)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
        .ok_or(AppError::BadRequest("Game Jolt non risponde o il gioco non esiste"))?
        .json::<GjResponse>()
        .await
        .map_err(|_| AppError::BadRequest("risposta di Game Jolt non valida"))?
        .payload
        .game;
    let cover = game
        .thumbnail_media_item
        .or(game.header_media_item)
        .and_then(|m| m.img_url)
        .filter(|c| c.starts_with("https://"));
    Ok((
        format!("https://gamejolt.com/games/{slug}/{id}"),
        game.title.as_deref().and_then(clean_name),
        game.developer.and_then(|d| d.display_name).as_deref().and_then(clean_name),
        cover,
    ))
}

#[derive(Deserialize)]
pub struct GameBananaInput {
    pub url: String,
}

#[derive(Deserialize)]
struct GbProfile {
    #[serde(rename = "_sName")]
    name: Option<String>,
    #[serde(rename = "_aSubmitter")]
    submitter: Option<GbSubmitter>,
    #[serde(rename = "_aPreviewMedia")]
    preview: Option<GbPreview>,
}

#[derive(Deserialize)]
struct GbSubmitter {
    #[serde(rename = "_sName")]
    name: Option<String>,
}

#[derive(Deserialize)]
struct GbPreview {
    #[serde(rename = "_aImages", default)]
    images: Vec<GbImage>,
}

#[derive(Deserialize)]
struct GbImage {
    #[serde(rename = "_sBaseUrl")]
    base: String,
    #[serde(rename = "_sFile")]
    file: String,
    #[serde(rename = "_sFile530")]
    file530: Option<String>,
}

pub async fn set_gamebanana(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
    Path(key): Path<String>,
    Json(input): Json<GameBananaInput>,
) -> Result<Json<SongMod>, AppError> {
    let dir = user_dir(&st, engine, &user)?;
    let fetched = if input.url.trim().is_empty() {
        None
    } else if let Some((slug, id)) = parse_gamejolt(&input.url) {
        Some(fetch_gamejolt(&slug, id).await?)
    } else {
        let (section, model, id) = parse_gamebanana(&input.url)
            .ok_or(AppError::BadRequest("link non valido: serve una pagina di GameBanana o di Game Jolt"))?;
        let api = format!("https://gamebanana.com/apiv11/{model}/{id}/ProfilePage");
        let profile: GbProfile = reqwest::Client::new()
            .get(&api)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .ok()
            .filter(|r| r.status().is_success())
            .ok_or(AppError::BadRequest(
                "GameBanana non risponde o la mod non esiste",
            ))?
            .json()
            .await
            .map_err(|_| AppError::BadRequest("risposta di GameBanana non valida"))?;
        let cover = profile
            .preview
            .and_then(|p| p.images.into_iter().next())
            .map(|i| format!("{}/{}", i.base, i.file530.unwrap_or(i.file)));
        Some((
            format!("https://gamebanana.com/{section}/{id}"),
            profile.name.as_deref().and_then(clean_name),
            profile
                .submitter
                .and_then(|s| s.name)
                .as_deref()
                .and_then(clean_name),
            cover.filter(|c| c.starts_with("https://")),
        ))
    };

    let _guard = lock().lock().await;
    let mut lib = load(&dir).await;
    let m = lib
        .mods
        .iter_mut()
        .find(|m| m.key == key)
        .ok_or(AppError::NotFound)?;
    match fetched {
        Some((url, name, author, cover)) => {
            m.gamebanana_url = Some(url);
            m.gb_name = name;
            m.gb_author = author;
            m.gb_cover_url = cover;
        }
        None => {
            m.gamebanana_url = None;
            m.gb_name = None;
            m.gb_author = None;
            m.gb_cover_url = None;
        }
    }
    let out = m.clone();
    save(&dir, &lib).await?;
    Ok(Json(out))
}

async fn find_clip(st: &AppState, engine: Engine, user: &str, id: Uuid) -> Result<PathBuf, AppError> {
    let dir = user_dir(st, engine, user)?;
    let lib = load(&dir).await;
    if !lib.clips.iter().any(|c| c.id == id) {
        return Err(AppError::NotFound);
    }
    Ok(dir)
}

pub async fn clip_video(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<axum::response::Response, AppError> {
    let dir = find_clip(&st, engine, &user, id).await?;
    crate::routes::serve_file(&clip_path(&dir, id), &headers, "", None).await
}

pub async fn clip_thumb(
    State(st): St,
    Extension(engine): Extension<Engine>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let dir = find_clip(&st, engine, &user, id).await?;
    let bytes = tokio::fs::read(thumb_path(&dir, id))
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_keys_are_stable_slugs() {
        assert_eq!(mod_key("VS Impostor V4"), "vs-impostor-v4");
        assert_eq!(mod_key("  Vs. Hex!! (Codename) "), "vs-hex-codename");
        assert_eq!(mod_key("???"), "mod");
    }

    #[test]
    fn gamebanana_links_are_parsed() {
        assert_eq!(
            parse_gamebanana("https://gamebanana.com/mods/44291"),
            Some(("mods", "Mod", 44291))
        );
        assert_eq!(
            parse_gamebanana("gamebanana.com/wips/123?foo=1"),
            Some(("wips", "Wip", 123))
        );
        assert_eq!(parse_gamebanana("https://example.com/mods/1"), None);
        assert_eq!(parse_gamebanana("https://gamebanana.com/members/1"), None);
        assert_eq!(
            parse_gamejolt("https://gamejolt.com/games/indiecross/643540"),
            Some(("indiecross".to_string(), 643540))
        );
        assert_eq!(parse_gamejolt("https://gamejolt.com/@brightfyre_"), None);
    }

    #[test]
    fn the_default_variation_is_the_same_as_none() {
        assert_eq!(variation_of(None), "");
        assert_eq!(variation_of(Some("default")), "");
        assert_eq!(variation_of(Some(" ")), "");
        assert_eq!(variation_of(Some("erect")), "erect");
    }

    #[test]
    fn only_simple_png_names_are_accepted_as_assets() {
        assert!(asset_name_ok("icon.png"));
        assert!(asset_name_ok("album-volume1.png"));
        assert!(!asset_name_ok("../library.json"));
        assert!(!asset_name_ok("x.jpg"));
        assert!(!asset_name_ok("a/b.png"));
        assert!(!asset_name_ok(".png"));
    }
}

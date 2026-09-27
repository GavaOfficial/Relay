use std::{
    collections::{BTreeMap, HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use reqwest::Method;
use serde_json::{json, Value};

use crate::core::Core;

pub const BASE_GAME: &str = "Gioco base";
const MOD_DIR: &str = "relay-integration";
const MODULE_HXC: &str = include_str!("funkin_module.hxc.txt");
const EVENTS_FILE: &str = "events.jsonl";
const CATALOG_FORMAT: u32 = 3;
pub(crate) const MAX_ASSET_BYTES: u64 = 5 * 1024 * 1024;
const CHECK_EVERY: Duration = Duration::from_secs(60);

fn folders_path(base: &Path) -> PathBuf {
    base.join("funkin-folders.json")
}

pub fn list_folders(base: &Path) -> Vec<String> {
    std::fs::read_to_string(folders_path(base))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_folders(base: &Path, folders: &[String]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(folders).map_err(|e| e.to_string())?;
    std::fs::write(folders_path(base), json).map_err(|e| e.to_string())
}

pub fn game_version(game_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(game_dir.join("CHANGELOG.md")).ok()?;
    text.lines().find_map(|l| {
        let v = l.trim().strip_prefix("## [")?.split(']').next()?.trim();
        let ok = v.split('.').count() == 3
            && v.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        ok.then(|| v.to_string())
    })
}

pub(crate) fn write_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

fn is_game_dir(dir: &Path) -> bool {
    dir.join("assets").is_dir()
        && (dir.join("Funkin.exe").is_file() || dir.join("CHANGELOG.md").is_file())
}

fn install(game_dir: &Path) -> Result<(), String> {
    if !is_game_dir(game_dir) {
        return Err("Non sembra la cartella di Friday Night Funkin': scegli quella con Funkin.exe.".into());
    }
    let version = game_version(game_dir).unwrap_or_else(|| "0.8.0".into());
    let meta = json!({
        "title": "Relay",
        "description": "Registra in automatico i tuoi record per il sito Relay. La aggiorna l'app Relay.",
        "contributors": [{ "name": "Relay" }],
        "api_version": version,
        "mod_version": "1.0.0",
        "license": "Apache-2.0",
    });
    let dir = game_dir.join("mods").join(MOD_DIR);
    let meta = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
    write_if_changed(&dir.join("_polymod_meta.json"), &meta)
        .and_then(|_| write_if_changed(&dir.join("scripts").join("RelayIntegration.hxc"), MODULE_HXC))
        .and_then(|_| std::fs::create_dir_all(game_dir.join("relay")))
        .map_err(|e| format!("non riesco a installare la mod di Relay: {e}"))
}

fn uninstall(game_dir: &Path) {
    let _ = std::fs::remove_dir_all(game_dir.join("mods").join(MOD_DIR));
    let _ = std::fs::remove_dir_all(game_dir.join("relay"));
}

pub fn add_folder(core: &Arc<Core>, folder: String) -> Result<Vec<String>, String> {
    let folder = folder.trim().to_string();
    install(Path::new(&folder))?;
    let base = core.data_dir();
    let mut folders = list_folders(&base);
    if !folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
        folders.push(folder.clone());
    }
    save_folders(&base, &folders)?;
    start_tail(core, &folder);
    let core = core.clone();
    tauri::async_runtime::spawn(async move { sync_catalogs(&core, Path::new(&folder)).await });
    Ok(folders)
}

pub fn remove_folder(base: &Path, folder: &str) -> Result<Vec<String>, String> {
    uninstall(Path::new(folder));
    let mut folders = list_folders(base);
    folders.retain(|f| f != folder);
    save_folders(base, &folders)?;
    Ok(folders)
}

struct Source {
    name: String,
    title: String,
    dir: PathBuf,
    meta: Option<Value>,
}

fn is_official(game_dir: &Path) -> bool {
    game_dir.join("Funkin.exe").is_file()
}

fn sources(game_dir: &Path) -> Vec<Source> {
    let mut mods: Vec<Source> = std::fs::read_dir(game_dir.join("mods"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && e.file_name() != MOD_DIR)
        .filter_map(|e| {
            let meta = std::fs::read_to_string(e.path().join("_polymod_meta.json")).ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            Some(Source {
                title: name.clone(),
                name,
                dir: e.path(),
                meta: serde_json::from_str(&meta).ok(),
            })
        })
        .collect();
    mods.sort_by_key(|m| m.name.to_lowercase());
    let (name, title) = if is_official(game_dir) {
        (BASE_GAME.to_string(), "Friday Night Funkin'".to_string())
    } else {
        let folder = crate::modname::folder_name(&game_dir.to_string_lossy());
        (crate::modname::game_name(game_dir), crate::modname::strip_version(&folder))
    };
    let mut out = vec![Source {
        name,
        title,
        dir: game_dir.join("assets"),
        meta: None,
    }];
    out.extend(mods);
    out
}

fn plain_variation(v: Option<&str>) -> Option<String> {
    v.map(|v| v.trim().to_lowercase())
        .filter(|v| !v.is_empty() && v != "default")
}

fn metadata_path(dir: &Path, id: &str, variation: Option<&str>) -> PathBuf {
    let file = match variation {
        Some(v) => format!("{id}-metadata-{v}.json"),
        None => format!("{id}-metadata.json"),
    };
    dir.join("data").join("songs").join(id).join(file)
}

pub fn mod_for(game_dir: &Path, song_id: &str, variation: Option<&str>) -> (String, Vec<String>) {
    let variation = plain_variation(variation);
    sources(game_dir)
        .into_iter()
        .skip(1)
        .filter(|s| metadata_path(&s.dir, song_id, variation.as_deref()).is_file())
        .last()
        .map(|s| identity(&s))
        .unwrap_or_else(|| identity(&sources(game_dir)[0]))
}

fn identity(src: &Source) -> (String, Vec<String>) {
    let Some(meta) = &src.meta else {
        return (src.name.clone(), Vec::new());
    };
    let title = meta["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&src.name);
    let name = crate::modname::strip_version(title);
    let aliases = crate::modname::aliases(&name, &[&src.name]);
    (name, aliases)
}

pub(crate) fn natural_key(s: &str) -> String {
    let mut out = String::new();
    let mut digits = String::new();
    for c in s.to_lowercase().chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            out.push_str(&format!("{digits:0>10}"));
            digits.clear();
        }
        if c != '\0' {
            out.push(c);
        }
    }
    out
}

pub(crate) fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn song_order(all: &[Source]) -> HashMap<String, (usize, usize)> {
    let mut order = HashMap::new();
    let mut rank = 0;
    for src in all {
        let mut levels: Vec<PathBuf> = std::fs::read_dir(src.dir.join("data").join("levels"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        levels.sort_by_key(|p| {
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
            (stem != "tutorial", natural_key(&stem))
        });
        for level in levels {
            let songs = read_json(&level)
                .and_then(|v| v["songs"].as_array().cloned())
                .unwrap_or_default();
            for (i, id) in songs.iter().filter_map(|s| s.as_str()).enumerate() {
                order.entry(id.to_string()).or_insert((rank, i));
            }
            rank += 1;
        }
    }
    order
}

fn variation_rank(v: Option<&str>) -> u8 {
    match v {
        None => 0,
        Some("erect") => 1,
        Some("pico") => 2,
        Some(_) => 3,
    }
}

fn metadata_files(dir: &Path) -> Vec<(String, Option<String>, PathBuf)> {
    let mut out = Vec::new();
    for song in std::fs::read_dir(dir.join("data").join("songs")).into_iter().flatten().flatten() {
        let id = song.file_name().to_string_lossy().into_owned();
        for file in std::fs::read_dir(song.path()).into_iter().flatten().flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(&format!("{id}-metadata")) else {
                continue;
            };
            let Some(rest) = rest.strip_suffix(".json") else {
                continue;
            };
            let variation = match rest.strip_prefix('-') {
                Some(v) if !v.is_empty() => Some(v.to_lowercase()),
                Some(_) => continue,
                None if rest.is_empty() => None,
                None => continue,
            };
            out.push((id.clone(), variation, file.path()));
        }
    }
    out
}

pub fn asset_slug(id: &str) -> String {
    let mut s = String::new();
    for c in id.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_matches('-').chars().take(80).collect();
    if s.is_empty() {
        "album".into()
    } else {
        s
    }
}

fn album_rank(id: &str) -> (u8, String) {
    let l = id.to_lowercase();
    let group = if l.starts_with("volume") {
        0
    } else if l.starts_with("expansion") {
        1
    } else {
        2
    };
    (group, natural_key(&l))
}

pub(crate) struct Built {
    pub catalog: Value,
    pub assets: Vec<(String, PathBuf)>,
    pub aliases: Vec<String>,
}

fn build_catalog(src: &Source, base_dir: &Path, order: &HashMap<String, (usize, usize)>, version: Option<&str>) -> Built {
    let mut assets = Vec::new();
    let mut tracks: Vec<((usize, usize), u8, String, Value)> = Vec::new();
    for (id, variation, path) in metadata_files(&src.dir) {
        let Some(pos) = order.get(&id).copied() else {
            continue;
        };
        let Some(meta) = read_json(&path) else {
            continue;
        };
        let play = &meta["playData"];
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default()
        };
        let ratings: BTreeMap<String, u64> = play["ratings"]
            .as_object()
            .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).collect())
            .unwrap_or_default();
        let track = json!({
            "id": id,
            "variation": variation,
            "name": meta["songName"].as_str().unwrap_or(&id),
            "artist": meta["artist"].as_str(),
            "album": play["album"].as_str(),
            "bpm": meta["timeChanges"][0]["bpm"].as_f64(),
            "difficulties": strings(&play["difficulties"]),
            "ratings": ratings,
        });
        tracks.push((pos, variation_rank(variation.as_deref()), id, track));
    }
    tracks.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));

    let mut album_ids: Vec<String> = Vec::new();
    for (_, _, _, t) in &tracks {
        if let Some(a) = t["album"].as_str() {
            if !album_ids.iter().any(|x| x == a) {
                album_ids.push(a.to_string());
            }
        }
    }
    if src.meta.is_none() {
        album_ids.sort_by_key(|a| album_rank(a));
    }
    let find = |rel: &Path| -> Option<PathBuf> {
        [src.dir.join(rel), base_dir.join(rel)].into_iter().find(|p| p.is_file())
    };
    let mut albums = Vec::new();
    for id in album_ids {
        let rel = Path::new("data").join("ui").join("freeplay").join("albums").join(format!("{id}.json"));
        let data = find(&rel).and_then(|p| read_json(&p)).unwrap_or(Value::Null);
        let art = data["albumArtAsset"]
            .as_str()
            .and_then(|a| find(&Path::new("images").join(format!("{a}.png"))))
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_ASSET_BYTES))
            .map(|p| {
                let name = format!("album-{}.png", asset_slug(&id));
                assets.push((name.clone(), p));
                name
            });
        albums.push(json!({
            "id": id,
            "name": data["name"].as_str().unwrap_or(&id),
            "artists": data["artists"].as_array().cloned().unwrap_or_default(),
            "art": art,
        }));
    }

    let meta = src.meta.clone().unwrap_or(Value::Null);
    let icon = src.dir.join("_polymod_icon.png");
    let has_icon = src.meta.is_some()
        && std::fs::metadata(&icon).is_ok_and(|m| m.len() <= MAX_ASSET_BYTES);
    if has_icon {
        assets.push(("icon.png".into(), icon));
    }
    let contributors: Vec<Value> = meta["contributors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(json!({ "name": c["name"].as_str()?, "role": c["role"].as_str(), "url": c["url"].as_str() }))
                })
                .collect()
        })
        .unwrap_or_default();
    let catalog = json!({
        "title": if src.meta.is_some() { meta["title"].as_str().unwrap_or(&src.title).to_string() } else { src.title.clone() },
        "description": meta["description"].as_str(),
        "version": if src.meta.is_some() { meta["mod_version"].as_str() } else { version },
        "contributors": contributors,
        "has_icon": has_icon,
        "albums": albums,
        "tracks": tracks.into_iter().map(|t| t.3).collect::<Vec<_>>(),
    });
    Built { catalog, assets, aliases: identity(src).1 }
}

fn fingerprint(built: &Built) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    CATALOG_FORMAT.hash(&mut h);
    built.catalog.to_string().hash(&mut h);
    built.aliases.hash(&mut h);
    for (name, path) in &built.assets {
        name.hash(&mut h);
        if let Ok(m) = std::fs::metadata(path) {
            m.len().hash(&mut h);
            m.modified().ok().hash(&mut h);
        }
    }
    format!("{:016x}", h.finish())
}

fn sync_path(base: &Path) -> PathBuf {
    base.join("funkin-sync.json")
}

fn sync_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(Default::default)
}

async fn sync_catalogs(core: &Arc<Core>, game_dir: &Path) {
    let dir = game_dir.to_path_buf();
    let primary = list_folders(&core.data_dir()).into_iter().find(|f| is_official(Path::new(f)));
    let skip_base = is_official(game_dir)
        && primary.is_some_and(|p| !Path::new(&p).eq(game_dir));
    let Ok(builts) = tokio::task::spawn_blocking(move || {
        let all: Vec<Source> = sources(&dir)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !(skip_base && *i == 0))
            .map(|(_, s)| s)
            .collect();
        let order = song_order(&all);
        let version = game_version(&dir);
        let base_dir = dir.join("assets");
        all.iter()
            .map(|s| (identity(s).0, build_catalog(s, &base_dir, &order, version.as_deref())))
            .collect::<Vec<_>>()
    })
    .await
    else {
        return;
    };
    upload_changed(core, "funkin", game_dir, builts).await;
}

pub(crate) async fn upload_changed(core: &Arc<Core>, engine: &str, game_dir: &Path, builts: Vec<(String, Built)>) {
    let _guard = sync_lock().lock().await;
    let base = core.data_dir();
    let mut done: HashMap<String, String> = std::fs::read_to_string(sync_path(&base))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut changed = false;
    for (name, built) in builts {
        let key = if engine == "funkin" {
            format!("{}|{name}", game_dir.display())
        } else {
            format!("{engine}|{}|{name}", game_dir.display())
        };
        let fp = fingerprint(&built);
        if done.get(&key) == Some(&fp) {
            continue;
        }
        match upload_catalog(core, engine, &name, &built).await {
            Ok(()) => {
                tracing::info!("catalogo di {name} aggiornato sul sito");
                done.insert(key, fp);
                changed = true;
            }
            Err(e) => {
                tracing::debug!("catalogo di {name} non caricato: {e}");
                break;
            }
        }
    }
    if changed {
        if let Ok(text) = serde_json::to_string_pretty(&done) {
            let _ = std::fs::write(sync_path(&base), text);
        }
    }
}

async fn upload_catalog(core: &Arc<Core>, engine: &str, name: &str, built: &Built) -> Result<(), String> {
    let resp = core
        .call(
            Method::PUT,
            &format!("/api/{engine}/catalog"),
            Some(json!({ "mod_name": name, "catalog": built.catalog, "aliases": built.aliases })),
        )
        .await?;
    if !resp.status().is_success() {
        return Err(format!("catalogo rifiutato ({})", resp.status()));
    }
    let key: Value = resp.json().await.map_err(|e| e.to_string())?;
    let key = key["key"].as_str().ok_or("risposta senza chiave")?.to_string();
    for (asset, path) in &built.assets {
        let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
        let resp = core
            .call_bytes(Method::PUT, &format!("/api/{engine}/mods/{key}/assets/{asset}"), bytes)
            .await?;
        if !resp.status().is_success() {
            tracing::debug!("immagine {asset} di {name} rifiutata ({})", resp.status());
        }
    }
    link_known_gamebanana(core, engine, &key).await;
    Ok(())
}

const KNOWN_GAMEBANANA: &[(&str, &str, &str)] = &[
    ("nmv", "impostorlegacy", "https://gamebanana.com/mods/55652"),
    ("psych", "mario", "https://gamebanana.com/mods/359554"),
    ("psych", "pibby-apocalypse", "https://gamebanana.com/wips/73842"),
    ("psych", "wii-funkin-vs-matt", "https://gamebanana.com/mods/44511"),
    ("kade", "indie-cross", "https://gamejolt.com/games/indiecross/643540"),
];

async fn link_known_gamebanana(core: &Arc<Core>, engine: &str, key: &str) {
    let Some((_, _, url)) = KNOWN_GAMEBANANA.iter().find(|(e, k, _)| *e == engine && *k == key) else {
        return;
    };
    let Ok(detail) = core.json::<Value>(Method::GET, &format!("/api/{engine}/mods/{key}"), None).await else {
        return;
    };
    if detail["mod"]["gamebanana_url"].is_string() {
        return;
    }
    match core
        .call(Method::POST, &format!("/api/{engine}/mods/{key}/gamebanana"), Some(json!({ "url": url })))
        .await
    {
        Ok(r) if r.status().is_success() => tracing::info!("{key}: collegata a GameBanana ({url})"),
        Ok(r) => tracing::debug!("{key}: GameBanana non collegato ({})", r.status()),
        Err(e) => tracing::debug!("{key}: GameBanana non collegato: {e}"),
    }
}

pub(crate) fn tailed() -> &'static Mutex<HashSet<String>> {
    static T: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

pub(crate) fn start_tail(core: &Arc<Core>, folder: &str) {
    if !tailed().lock().unwrap().insert(folder.to_lowercase()) {
        return;
    }
    let path = Path::new(folder).join("relay").join(EVENTS_FILE);
    tauri::async_runtime::spawn(crate::fnf::tail_events(
        core.clone(),
        path,
        Some(folder.to_string()),
    ));
}

fn game_is_open() -> bool {
    relay_agent::windows::list_windows()
        .iter()
        .any(|w| w.exe.eq_ignore_ascii_case("Funkin.exe"))
}

pub fn spawn(core: Arc<Core>) {
    for folder in list_folders(&core.data_dir()) {
        start_tail(&core, &folder);
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            let open = tokio::task::spawn_blocking(game_is_open).await.unwrap_or(true);
            for folder in list_folders(&core.data_dir()) {
                let dir = PathBuf::from(&folder);
                if let Err(e) = install(&dir) {
                    tracing::warn!("mod di Relay non aggiornata in {folder}: {e}");
                    continue;
                }
                let events = dir.join("relay").join(EVENTS_FILE);
                if !open && std::fs::metadata(&events).is_ok_and(|m| m.len() > 1024 * 1024) {
                    let _ = std::fs::remove_file(&events);
                }
                sync_catalogs(&core, &dir).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_version_comes_from_the_changelog() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n## [0.8.6] - 2026-08-12\n### Added\n## [0.8.5]\n",
        )
        .unwrap();
        assert_eq!(game_version(dir.path()).as_deref(), Some("0.8.6"));
    }

    #[test]
    fn a_standalone_game_built_on_fnf_is_not_the_base_game() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path().join("Vs Sky Redux");
        write(g.join("Redux.exe"), "");
        write(g.join("assets/data/levels/week1.json"), r#"{"songs":[]}"#);
        let base = &sources(&g)[0];
        assert_eq!(identity(base).0, "Redux");
        assert_eq!(base.title, "Vs Sky Redux");
        write(g.join("Funkin.exe"), "");
        let _ = std::fs::remove_file(g.join("Redux.exe"));
        assert_eq!(identity(&sources(&g)[0]).0, BASE_GAME);
    }

    #[test]
    fn weeks_sort_by_number() {
        let mut v = vec!["week10", "week2", "weekend1", "week1"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, ["week1", "week2", "week10", "weekend1"]);
        let mut a = vec!["spaghetti", "expansion1", "volume2", "volume1"];
        a.sort_by_key(|s| album_rank(s));
        assert_eq!(a, ["volume1", "volume2", "expansion1", "spaghetti"]);
    }

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn songs_belong_to_the_mod_that_has_their_metadata() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path();
        let meta = r#"{"songName":"Bopeebo","artist":"Kawai Sprite","timeChanges":[{"bpm":100}],
            "playData":{"album":"volume1","difficulties":["easy","hard"],"ratings":{"hard":2}}}"#;
        write(g.join("assets/data/songs/bopeebo/bopeebo-metadata.json"), meta);
        write(g.join("assets/data/songs/bopeebo/bopeebo-metadata-erect.json"), meta);
        write(g.join("assets/data/songs/test/test-metadata.json"), meta);
        write(g.join("assets/data/levels/week1.json"), r#"{"songs":["bopeebo"]}"#);
        write(g.join("assets/data/ui/freeplay/albums/volume1.json"), r#"{"name":"Volume 1","albumArtAsset":"freeplay/albumRoll/volume1"}"#);
        write(g.join("assets/images/freeplay/albumRoll/volume1.png"), "png");
        write(g.join("mods/Haniel/_polymod_meta.json"), r#"{"title":"FNF: Haniel","contributors":[{"name":"H"}]}"#);
        write(g.join("mods/Haniel/data/songs/bopeebo/bopeebo-metadata-haniel.json"), meta);
        write(g.join("mods/relay-integration/_polymod_meta.json"), "{}");
        write(g.join("Funkin.exe"), "");

        assert_eq!(mod_for(g, "bopeebo", None).0, BASE_GAME);
        assert_eq!(mod_for(g, "bopeebo", Some("default")).0, BASE_GAME);
        assert_eq!(
            mod_for(g, "bopeebo", Some("haniel")),
            ("FNF: Haniel".to_string(), vec!["Haniel".to_string()]),
            "nome dal titolo della mod, cartella come vecchio nome"
        );

        let all = sources(g);
        assert_eq!(all.len(), 2, "la mod di Relay non e' una mod da mostrare");
        let order = song_order(&all);
        let base = build_catalog(&all[0], &g.join("assets"), &order, Some("0.8.6"));
        let tracks = base.catalog["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 2, "la canzone di prova fuori dalle settimane resta fuori");
        assert_eq!(tracks[0]["variation"], Value::Null);
        assert_eq!(tracks[1]["variation"], "erect");
        assert_eq!(base.catalog["albums"][0]["art"], "album-volume1.png");
        assert_eq!(base.catalog["version"], "0.8.6");

        let haniel = build_catalog(&all[1], &g.join("assets"), &order, None);
        assert_eq!(haniel.catalog["title"], "FNF: Haniel");
        assert_eq!(haniel.catalog["tracks"][0]["variation"], "haniel");
        assert_eq!(haniel.assets.len(), 1, "l'album del gioco base usato dalla mod ha la sua copertina");
    }
}

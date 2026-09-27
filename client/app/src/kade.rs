use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde_json::{json, Value};

use crate::{
    core::Core,
    funkin::{read_json, upload_changed, write_if_changed, Built},
    modname::{aliases, folder_name, game_name},
};

const SCRIPT: &str = include_str!("kade_script.lua.txt");
const SCRIPT_FILE: &str = "modchart.lua";
const MARKER: &str = "-- Relay: registra in automatico i tuoi record.";
const CHECK_EVERY: Duration = Duration::from_secs(60);
pub const ENGINE: &str = "kade";

fn folders_path(base: &Path) -> PathBuf {
    base.join("kade-folders.json")
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

fn exes(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            n.ends_with(".exe") && !n.contains("crash") && !n.starts_with("unins")
        })
        .collect()
}

pub fn is_kade_exe(data: &[u8]) -> bool {
    memchr::memmem::find(data, b"ModchartState").is_some()
}

fn detect(dir: &Path) -> bool {
    exes(dir).iter().any(|exe| std::fs::read(exe).is_ok_and(|d| is_kade_exe(&d)))
}

fn song_dirs(game_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(game_dir.join("assets").join("data"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_lowercase();
            let has_chart = !charts(&e.path(), &id).is_empty();
            has_chart.then(|| (id, e.path()))
        })
        .collect();
    out.sort();
    out
}

fn charts(dir: &Path, id: &str) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let stem = name.strip_suffix(".json")?;
            let diff = if stem == id {
                "Normal".to_string()
            } else {
                let d = stem.strip_prefix(id)?.strip_prefix('-')?;
                let mut c = d.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect())?
            };
            Some((diff, e.path()))
        })
        .collect();
    let rank = |d: &str| match d {
        "Easy" => 0,
        "Normal" => 1,
        "Hard" => 2,
        _ => 3,
    };
    out.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then(a.0.cmp(&b.0)));
    out
}

fn save_files(game_dir: &Path) -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else {
        return Vec::new();
    };
    let titles: Vec<String> = exes(game_dir)
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .collect();
    let mut out = Vec::new();
    for company in std::fs::read_dir(&appdata).into_iter().flatten().flatten() {
        for game in std::fs::read_dir(company.path()).into_iter().flatten().flatten() {
            if !titles.contains(&game.file_name().to_string_lossy().to_lowercase()) {
                continue;
            }
            let mut stack = vec![(game.path(), 0)];
            while let Some((dir, depth)) = stack.pop() {
                for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() && depth < 2 {
                        stack.push((p, depth + 1));
                    } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sol")) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn ogg_duration_ms(path: &Path) -> Option<u64> {
    let data = std::fs::read(path).ok()?;
    let rate_at = memchr::memmem::find(&data, b"\x01vorbis")? + 12;
    let rate = u32::from_le_bytes(data.get(rate_at..rate_at + 4)?.try_into().ok()?) as u64;
    let last = memchr::memmem::rfind(&data, b"OggS")?;
    let granule = u64::from_le_bytes(data.get(last + 6..last + 14)?.try_into().ok()?);
    (rate > 0 && granule > 0 && granule != u64::MAX).then(|| granule * 1000 / rate)
}

fn script_for(game_dir: &Path, saves: &[PathBuf], id: &str) -> String {
    let saves: Vec<String> = saves
        .iter()
        .map(|p| format!("[[{}]]", p.to_string_lossy().replace('\\', "/")))
        .collect();
    let length = ogg_duration_ms(&game_dir.join("assets").join("songs").join(id).join("Inst.ogg")).unwrap_or(0);
    SCRIPT
        .replace("--[[RELAY_SAVES]]", &saves.join(", "))
        .replace("--[[RELAY_LENGTH]]0", &length.to_string())
}

fn install(game_dir: &Path) -> Result<(), String> {
    let songs = song_dirs(game_dir);
    if songs.is_empty() {
        return Err("Non trovo le canzoni della mod (assets/data): Relay non puo' collegarla.".into());
    }
    let saves = save_files(game_dir);
    for (id, dir) in songs {
        let script = script_for(game_dir, &saves, &id);
        let path = dir.join(SCRIPT_FILE);
        match std::fs::read_to_string(&path) {
            Ok(old) if !old.starts_with(MARKER) => {
                tracing::debug!("{id} ha gia' un suo modchart: Relay non registra questa canzone");
                continue;
            }
            _ => {}
        }
        write_if_changed(&path, &script).map_err(|e| format!("non riesco a installare lo script di Relay: {e}"))?;
    }
    std::fs::create_dir_all(game_dir.join("relay")).map_err(|e| e.to_string())
}

fn uninstall(game_dir: &Path) {
    for (_, dir) in song_dirs(game_dir) {
        let path = dir.join(SCRIPT_FILE);
        if std::fs::read_to_string(&path).is_ok_and(|s| s.starts_with(MARKER)) {
            let _ = std::fs::remove_file(path);
        }
    }
    let _ = std::fs::remove_dir_all(game_dir.join("relay"));
}

pub fn add_folder(core: &Arc<Core>, folder: String) -> Result<Vec<String>, String> {
    let folder = folder.trim().to_string();
    let dir = PathBuf::from(&folder);
    if !dir.is_dir() {
        return Err("La cartella scelta non esiste.".into());
    }
    if !detect(&dir) {
        return Err("Non sembra una mod fatta con Kade Engine (con i modchart Lua): scegli la cartella con il suo .exe.".into());
    }
    install(&dir)?;
    let base = core.data_dir();
    let mut folders = list_folders(&base);
    if !folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
        folders.push(folder.clone());
    }
    save_folders(&base, &folders)?;
    crate::funkin::start_tail(core, &folder);
    let core = core.clone();
    tauri::async_runtime::spawn(async move { sync_catalog(&core, Path::new(&folder)).await });
    Ok(folders)
}

pub fn remove_folder(base: &Path, folder: &str) -> Result<Vec<String>, String> {
    uninstall(Path::new(folder));
    let mut folders = list_folders(base);
    folders.retain(|f| f != folder);
    save_folders(base, &folders)?;
    Ok(folders)
}

pub fn mod_identity(game_dir: &Path) -> (String, Vec<String>) {
    let name = game_name(game_dir);
    let old = folder_name(&game_dir.to_string_lossy());
    let aliases = aliases(&name, &[&old]);
    (name, aliases)
}

fn title(id: &str) -> String {
    id.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn build_catalog(game_dir: &Path) -> Option<(String, Built)> {
    let (name, aliases) = mod_identity(game_dir);
    let mut tracks = Vec::new();
    for (id, dir) in song_dirs(game_dir) {
        let charts = charts(&dir, &id);
        let song = charts
            .iter()
            .filter_map(|(_, p)| read_json(p))
            .map(|v| v["song"].clone())
            .find(Value::is_object)
            .unwrap_or(Value::Null);
        let display = song["song"].as_str().map(|s| s.replace('-', " ")).filter(|s| !s.trim().is_empty());
        tracks.push(json!({
            "id": id,
            "name": display.unwrap_or_else(|| title(&id)),
            "bpm": song["bpm"].as_f64(),
            "difficulties": charts.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>(),
        }));
    }
    if tracks.is_empty() {
        return None;
    }
    let catalog = json!({
        "title": name,
        "contributors": [],
        "has_icon": false,
        "albums": [],
        "tracks": tracks,
    });
    Some((name, Built { catalog, assets: Vec::new(), aliases }))
}

async fn sync_catalog(core: &Arc<Core>, game_dir: &Path) {
    let dir = game_dir.to_path_buf();
    let Ok(Some(built)) = tokio::task::spawn_blocking(move || build_catalog(&dir)).await else {
        return;
    };
    upload_changed(core, ENGINE, game_dir, vec![built]).await;
}

pub fn spawn(core: Arc<Core>) {
    for folder in list_folders(&core.data_dir()) {
        crate::funkin::start_tail(&core, &folder);
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(9)).await;
        loop {
            for folder in list_folders(&core.data_dir()) {
                let dir = PathBuf::from(&folder);
                if let Err(e) = install(&dir) {
                    tracing::warn!("script di Relay non aggiornato in {folder}: {e}");
                    continue;
                }
                let events = dir.join("relay").join("events.jsonl");
                if std::fs::metadata(&events).is_ok_and(|m| m.len() > 1024 * 1024) {
                    let _ = std::fs::write(&events, "");
                }
                sync_catalog(&core, &dir).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn songs_and_difficulties_come_from_the_charts() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path();
        write(g.join("Indie Cross.exe"), "..ModchartState..");
        write(g.join("assets/data/snake-eyes/snake-eyes.json"), r#"{"song":{"song":"Snake-Eyes","bpm":154}}"#);
        write(g.join("assets/data/snake-eyes/snake-eyes-hard.json"), "{}");
        write(g.join("assets/data/snake-eyes/snake-eyes-easy.json"), "{}");
        write(g.join("assets/data/bad-time/bad-time-hard.json"), r#"{"song":{"song":"Bad-Time","bpm":150}}"#);
        write(g.join("assets/data/empty/readme.txt"), "");
        write(g.join("assets/data/modchart.lua"), "print()");

        assert!(detect(g));
        let (name, built) = build_catalog(g).unwrap();
        assert_eq!(name, "Indie Cross");
        let t = &built.catalog["tracks"];
        assert_eq!(t.as_array().unwrap().len(), 2, "solo le cartelle con un chart");
        assert_eq!(t[0]["id"], "bad-time");
        assert_eq!(t[0]["difficulties"], json!(["Hard"]));
        assert_eq!(t[1]["name"], "Snake Eyes");
        assert_eq!(t[1]["bpm"], 154.0);
        assert_eq!(t[1]["difficulties"], json!(["Easy", "Normal", "Hard"]));
    }

    #[test]
    fn the_script_goes_only_where_the_mod_has_no_modchart() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path();
        write(g.join("assets/data/a/a.json"), "{}");
        write(g.join("assets/data/b/b-hard.json"), "{}");
        write(g.join("assets/data/b/modchart.lua"), "function start(song) end");
        install(g).unwrap();
        assert!(std::fs::read_to_string(g.join("assets/data/a/modchart.lua")).unwrap().starts_with(MARKER));
        assert_eq!(std::fs::read_to_string(g.join("assets/data/b/modchart.lua")).unwrap(), "function start(song) end");
        uninstall(g);
        assert!(!g.join("assets/data/a/modchart.lua").exists());
        assert!(g.join("assets/data/b/modchart.lua").exists(), "il modchart della mod resta");
    }
}

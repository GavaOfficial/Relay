use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use reqwest::Method;
use serde_json::{json, Value};

use crate::{core::Core, fnf::SongEvent};

const MOD_FILE: &str = "gavatech.relay.geode";
const MOD_BYTES: &[u8] = include_bytes!("../resources/gd/gavatech.relay.geode");
const CHECK_EVERY: Duration = Duration::from_secs(60);

fn folders_path(base: &Path) -> PathBuf {
    base.join("gd-folders.json")
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

pub fn is_game_dir(dir: &Path) -> bool {
    dir.join("GeometryDash.exe").is_file()
}

pub fn find_game() -> Option<String> {
    let steam = PathBuf::from(r"C:\Program Files (x86)\Steam");
    let mut libs = vec![steam.clone()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        for line in vdf.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("\"path\"") {
                let path = rest.trim().trim_matches('"').replace("\\\\", "\\");
                libs.push(PathBuf::from(path));
            }
        }
    }
    libs.into_iter()
        .map(|l| l.join("steamapps").join("common").join("Geometry Dash"))
        .find(|d| is_game_dir(d))
        .map(|d| d.to_string_lossy().into_owned())
}

fn install(game_dir: &Path) -> Result<(), String> {
    if !is_game_dir(game_dir) {
        return Err(
            "Non e' la cartella di Geometry Dash: scegli quella con GeometryDash.exe.".into(),
        );
    }
    if !game_dir.join("Geode.dll").is_file() {
        return Err("Geode non e' installato: installalo da geode-sdk.org e poi riprova.".into());
    }
    let target = game_dir.join("geode").join("mods").join(MOD_FILE);
    if std::fs::read(&target).is_ok_and(|b| b == MOD_BYTES) {
        return Ok(());
    }
    std::fs::create_dir_all(target.parent().expect("ha una cartella"))
        .and_then(|_| std::fs::write(&target, MOD_BYTES))
        .map_err(|e| format!("non riesco a installare la mod di Relay: {e}"))
}

pub fn add_folder(core: &Arc<Core>, folder: String) -> Result<Vec<String>, String> {
    let folder = folder.trim().trim_end_matches(['\\', '/']).to_string();
    install(Path::new(&folder))?;
    let base = core.data_dir();
    let mut folders = list_folders(&base);
    if !folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
        folders.push(folder);
    }
    save_folders(&base, &folders)?;
    start_tail(core);
    Ok(folders)
}

pub fn remove_folder(base: &Path, folder: &str) -> Result<Vec<String>, String> {
    let _ = std::fs::remove_file(Path::new(folder).join("geode").join("mods").join(MOD_FILE));
    let mut folders = list_folders(base);
    folders.retain(|f| f != folder);
    save_folders(base, &folders)?;
    Ok(folders)
}

fn events_path(core: &Core) -> PathBuf {
    core.data_dir().join("gd-events.jsonl")
}

fn start_tail(core: &Arc<Core>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    let Some(folder) = list_folders(&core.data_dir()).into_iter().next() else {
        return;
    };
    if STARTED.set(()).is_err() {
        return;
    }
    tauri::async_runtime::spawn(crate::fnf::tail_events(
        core.clone(),
        events_path(core),
        Some(folder),
    ));
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Known {
    sections: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(default)]
    uploaded: BTreeMap<String, String>,
    #[serde(default)]
    profile: Option<Value>,
}

fn known_path(core: &Core) -> PathBuf {
    core.data_dir().join("gd-levels.json")
}

fn lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(Default::default)
}

fn track_for(level: &Value, mode: &str) -> Option<Value> {
    let id = level["id"].as_i64()?.to_string();
    let mut extra = level.clone();
    if let Some(o) = extra.as_object_mut() {
        for k in ["id", "name", "creator"] {
            o.remove(k);
        }
    }
    Some(json!({
        "id": id,
        "name": level["name"].as_str().unwrap_or("?"),
        "artist": level["creator"].as_str().filter(|c| !c.is_empty()),
        "difficulties": [mode],
        "extra": extra,
    }))
}

pub fn observe(core: &Arc<Core>, ev: &SongEvent) {
    if ev.kind != "song_load" {
        return;
    }
    let Some(data) = ev.data.clone() else {
        return;
    };
    let section = ev
        .mod_folder
        .clone()
        .unwrap_or_else(|| "Livelli online".into());
    let mode = ev.difficulty.clone().unwrap_or_else(|| "classic".into());
    let core = core.clone();
    tauri::async_runtime::spawn(async move {
        let _guard = lock().lock().await;
        let path = known_path(&core);
        let mut known: Known = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if let Some(track) = track_for(&data["level"], &mode) {
            let id = track["id"].as_str().unwrap_or_default().to_string();
            known
                .sections
                .entry(section.clone())
                .or_default()
                .insert(id, track);
            let tracks: Vec<Value> = known.sections[&section].values().cloned().collect();
            let body =
                json!({ "mod_name": section, "catalog": { "title": section, "tracks": tracks } });
            let fingerprint = body.to_string();
            if known.uploaded.get(&section) != Some(&fingerprint) {
                match core.call(Method::PUT, "/api/gd/catalog", Some(body)).await {
                    Ok(r) if r.status().is_success() => {
                        known.uploaded.insert(section.clone(), fingerprint);
                    }
                    Ok(r) => tracing::debug!("livelli di GD non caricati ({})", r.status()),
                    Err(e) => tracing::debug!("livelli di GD non caricati: {e}"),
                }
            }
        }
        let profile = data["profile"].clone();
        if profile.is_object() && known.profile.as_ref() != Some(&profile) {
            if let Ok(r) = core
                .call(Method::PUT, "/api/gd/profile", Some(profile.clone()))
                .await
            {
                if r.status().is_success() {
                    known.profile = Some(profile);
                }
            }
        }
        if let Ok(text) = serde_json::to_string_pretty(&known) {
            let _ = std::fs::write(&path, text);
        }
    });
}

pub fn result_text(data: Option<&Value>, platformer: bool) -> String {
    let Some(d) = data else {
        return String::new();
    };
    if d["completed"].as_bool() == Some(true) {
        if platformer {
            let ms = d["time_ms"].as_i64().unwrap_or(0).max(0);
            return format!(
                "{}:{:02}.{:02}",
                ms / 60_000,
                (ms / 1000) % 60,
                (ms % 1000) / 10
            );
        }
        return match d["coins"].as_i64().unwrap_or(0) {
            0 => "completato".into(),
            1 => "completato con 1 moneta".into(),
            n => format!("completato con {n} monete"),
        };
    }
    format!("{}%", d["percent"].as_i64().unwrap_or(0))
}

pub fn spawn(core: Arc<Core>) {
    start_tail(&core);
    tauri::async_runtime::spawn(async move {
        loop {
            for folder in list_folders(&core.data_dir()) {
                if let Err(e) = install(Path::new(&folder)) {
                    tracing::warn!("mod di Relay per Geometry Dash non aggiornata: {e}");
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_read_like_the_game() {
        assert_eq!(
            result_text(Some(&json!({"percent": 82, "completed": false})), false),
            "82%"
        );
        assert_eq!(
            result_text(
                Some(&json!({"percent": 100, "completed": true, "coins": 3})),
                false
            ),
            "completato con 3 monete"
        );
        assert_eq!(
            result_text(Some(&json!({"completed": true, "time_ms": 83456})), true),
            "1:23.45"
        );
    }

    #[test]
    fn a_level_becomes_a_catalog_track() {
        let t = track_for(
            &json!({"id": 1, "name": "Stereo Madness", "creator": "", "stars": 1}),
            "classic",
        )
        .unwrap();
        assert_eq!(t["id"], "1");
        assert_eq!(t["artist"], Value::Null);
        assert_eq!(t["extra"]["stars"], 1);
        assert_eq!(t["extra"].get("name"), None);
    }
}

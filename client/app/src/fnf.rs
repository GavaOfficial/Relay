use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::core::Core;

// Porta fissa: e' lo script Hscript dentro al gioco a doverla conoscere in anticipo.
pub const PORT: u16 = 47811;

const ADDON_NAME: &str = "relay-integration";
const GLOBAL_HX: &str = include_str!("fnf_global.hx.txt");
const INJECT_BEGIN: &str = "// RELAY-INTEGRATION-BEGIN";
const INJECT_END: &str = "// RELAY-INTEGRATION-END";
static FIRST_EVENT_LOGGED: AtomicBool = AtomicBool::new(false);

fn folders_path(base: &Path) -> PathBuf {
    base.join("codename-engine-folders.json")
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

fn rendered_script(callback: &str) -> String {
    GLOBAL_HX
        .replace("__PORT__", &PORT.to_string())
        .replace("__CALLBACK__", callback)
}

fn strip_injection(text: &str) -> String {
    let Some(start) = text.find(INJECT_BEGIN) else {
        return text.to_string();
    };
    let Some(relative_end) = text[start..].find(INJECT_END) else {
        return text.to_string();
    };
    let end = start + relative_end + INJECT_END.len();
    let mut clean = String::with_capacity(text.len());
    clean.push_str(text[..start].trim_end());
    clean.push('\n');
    clean.push_str(text[end..].trim_start_matches(['\r', '\n']));
    clean
}

fn has_function(text: &str, name: &str) -> bool {
    text.contains(&format!("function {name}("))
}

fn inject_global(path: &Path) -> Result<(), String> {
    let original = std::fs::read_to_string(path)
        .map_err(|e| format!("non riesco a leggere {}: {e}", path.display()))?;
    let clean = strip_injection(&original);
    let callback = ["postUpdate", "preUpdate", "update"]
        .into_iter()
        .find(|name| !has_function(&clean, name))
        .ok_or_else(|| format!("{} usa gia' update, preUpdate e postUpdate", path.display()))?;

    let backup = path.with_file_name("global.hx.relay-backup");
    if !backup.exists() {
        std::fs::write(&backup, original.as_bytes())
            .map_err(|e| format!("non riesco a creare il backup {}: {e}", backup.display()))?;
    }
    let patched = format!(
        "{}\n\n{}\n{}\n{}\n",
        clean.trim_end(),
        INJECT_BEGIN,
        rendered_script(callback).trim(),
        INJECT_END
    );
    std::fs::write(path, patched)
        .map_err(|e| format!("non riesco ad aggiornare {}: {e}", path.display()))
}

fn inject_loaded_mods(game_dir: &Path) -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(game_dir.join("mods")) else {
        return Ok(0);
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let global = entry.path().join("data").join("global.hx");
        if global.is_file() {
            inject_global(&global)?;
            count += 1;
        }
    }
    Ok(count)
}

fn install_integration(game_dir: &Path) -> Result<(), String> {
    let data_dir = game_dir.join("addons").join(ADDON_NAME).join("data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("non riesco a creare la cartella dell'addon: {e}"))?;
    std::fs::write(data_dir.join("global.hx"), rendered_script("update"))
        .map_err(|e| format!("non riesco a scrivere lo script: {e}"))?;
    inject_loaded_mods(game_dir)?;
    Ok(())
}

fn remove_integration(game_dir: &Path) {
    let addon_dir = game_dir.join("addons").join(ADDON_NAME);
    let _ = std::fs::remove_dir_all(addon_dir);
    if let Ok(entries) = std::fs::read_dir(game_dir.join("mods")) {
        for entry in entries.flatten() {
            let global = entry.path().join("data").join("global.hx");
            if let Ok(text) = std::fs::read_to_string(&global) {
                let clean = strip_injection(&text);
                if clean != text {
                    let _ = std::fs::write(global, clean);
                }
            }
        }
    }
}

pub fn add_folder(base: &Path, folder: String) -> Result<Vec<String>, String> {
    let folder = folder.trim().to_string();
    let dir = PathBuf::from(&folder);
    if !dir.is_dir() {
        return Err("La cartella scelta non esiste.".into());
    }
    install_integration(&dir)?;
    let mut folders = list_folders(base);
    if !folders.iter().any(|f| f == &folder) {
        folders.push(folder);
    }
    save_folders(base, &folders)?;
    Ok(folders)
}

pub fn remove_folder(base: &Path, folder: &str) -> Result<Vec<String>, String> {
    remove_integration(Path::new(folder));
    let mut folders = list_folders(base);
    folders.retain(|f| f != folder);
    save_folders(base, &folders)?;
    Ok(folders)
}

#[derive(Clone, Deserialize)]
struct EventIn {
    song_name: Option<String>,
    difficulty: Option<String>,
    score: Option<i64>,
    accuracy: Option<f32>,
    #[serde(default)]
    miss: bool,
}

async fn accept_event(core: &Arc<Core>, ev: EventIn) {
    if !FIRST_EVENT_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::info!("primo evento Codename Engine ricevuto correttamente");
    }
    core.report_fnf_event(ev.song_name, ev.difficulty, ev.score, ev.accuracy, ev.miss)
        .await;
}

async fn on_event(
    State(core): State<Arc<Core>>,
    Json(ev): Json<EventIn>,
) -> axum::http::StatusCode {
    accept_event(&core, ev).await;
    axum::http::StatusCode::NO_CONTENT
}

// Server HTTP locale: lo script dentro al gioco Codename Engine manda qui i suoi eventi
// (nota mancata, punteggio, accuracy). Ascolta solo su 127.0.0.1: non e' mai raggiungibile
// da fuori questo PC.
pub fn spawn(core: Arc<Core>) {
    for folder in list_folders(&core.data_dir()) {
        if let Err(e) = install_integration(Path::new(&folder)) {
            tracing::warn!("integrazione Codename Engine non aggiornata in {folder}: {e}");
        }
    }
    let event_path = core.data_dir().join("fnf-event.json");
    let _ = std::fs::remove_file(&event_path);
    let file_core = core.clone();
    tauri::async_runtime::spawn(async move {
        let mut last = String::new();
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&event_path).await {
                if text != last {
                    if let Ok(event) = serde_json::from_str::<EventIn>(&text) {
                        last = text;
                        accept_event(&file_core, event).await;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });
    tauri::async_runtime::spawn(async move {
        let app = Router::new()
            .route("/event", post(on_event))
            .with_state(core);
        let listener = match TcpListener::bind(("127.0.0.1", PORT)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("server locale Codename Engine non avviato: {e}");
                return;
            }
        };
        if let Err(e) = axum::serve(listener, app).await {
            tracing::warn!("server locale Codename Engine terminato: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_block_is_replaced_and_removable() {
        let original = "import flixel.FlxG;\n\nfunction update(elapsed:Float) {}\n";
        let callback = ["postUpdate", "preUpdate", "update"]
            .into_iter()
            .find(|name| !has_function(original, name))
            .unwrap();
        assert_eq!(callback, "postUpdate");
        let once = format!(
            "{}\n{}\n{}\n{}\n",
            original.trim_end(),
            INJECT_BEGIN,
            rendered_script(callback),
            INJECT_END
        );
        let clean = strip_injection(&once);
        assert_eq!(clean.trim(), original.trim());
        assert!(!clean.contains("relaySend"));
    }
}

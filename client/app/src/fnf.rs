use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::core::Core;

// Porta fissa: e' lo script Hscript dentro al gioco a doverla conoscere in anticipo.
pub const PORT: u16 = 47811;

const ADDON_NAME: &str = "relay-integration";
const GLOBAL_HX: &str = include_str!("fnf_global.hx.txt");

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

fn install_addon(game_dir: &Path) -> Result<(), String> {
    let data_dir = game_dir.join("addons").join(ADDON_NAME).join("data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("non riesco a creare la cartella dell'addon: {e}"))?;
    std::fs::write(
        data_dir.join("global.hx"),
        GLOBAL_HX.replace("__PORT__", &PORT.to_string()),
    )
    .map_err(|e| format!("non riesco a scrivere lo script: {e}"))
}

fn remove_addon(game_dir: &Path) {
    let addon_dir = game_dir.join("addons").join(ADDON_NAME);
    let _ = std::fs::remove_dir_all(addon_dir);
}

pub fn add_folder(base: &Path, folder: String) -> Result<Vec<String>, String> {
    let folder = folder.trim().to_string();
    let dir = PathBuf::from(&folder);
    if !dir.is_dir() {
        return Err("La cartella scelta non esiste.".into());
    }
    install_addon(&dir)?;
    let mut folders = list_folders(base);
    if !folders.iter().any(|f| f == &folder) {
        folders.push(folder);
    }
    save_folders(base, &folders)?;
    Ok(folders)
}

pub fn remove_folder(base: &Path, folder: &str) -> Result<Vec<String>, String> {
    remove_addon(Path::new(folder));
    let mut folders = list_folders(base);
    folders.retain(|f| f != folder);
    save_folders(base, &folders)?;
    Ok(folders)
}

#[derive(Deserialize)]
struct EventIn {
    song_name: Option<String>,
    difficulty: Option<String>,
    score: Option<i64>,
    accuracy: Option<f32>,
    #[serde(default)]
    miss: bool,
}

async fn on_event(
    State(core): State<Arc<Core>>,
    Json(ev): Json<EventIn>,
) -> axum::http::StatusCode {
    core.report_fnf_event(ev.song_name, ev.difficulty, ev.score, ev.accuracy, ev.miss)
        .await;
    axum::http::StatusCode::NO_CONTENT
}

// Server HTTP locale: lo script dentro al gioco Codename Engine manda qui i suoi eventi
// (nota mancata, punteggio, accuracy). Ascolta solo su 127.0.0.1: non e' mai raggiungibile
// da fuori questo PC.
pub fn spawn(core: Arc<Core>) {
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

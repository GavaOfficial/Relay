use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;
use tokio::{net::TcpListener, sync::mpsc};

use crate::core::Core;

pub const PORT: u16 = 47811;

const ADDON_NAME: &str = "relay-integration";
const SONG_HX: &str = include_str!("fnf_song.hx.txt");
const SONG_SCRIPT: &str = "relay-integration.hx";
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

fn remove_old_integration(game_dir: &Path) {
    let old_global = game_dir.join("addons").join(ADDON_NAME).join("data");
    let _ = std::fs::remove_file(old_global.join("global.hx"));
    let _ = std::fs::remove_dir(&old_global);
    let Ok(entries) = std::fs::read_dir(game_dir.join("mods")) else {
        return;
    };
    for entry in entries.flatten() {
        let global = entry.path().join("data").join("global.hx");
        if let Ok(text) = std::fs::read_to_string(&global) {
            let clean = strip_injection(&text);
            if clean != text && std::fs::write(&global, clean).is_ok() {
                let _ = std::fs::remove_file(global.with_file_name("global.hx.relay-backup"));
            }
        }
    }
}

fn install_integration(game_dir: &Path) -> Result<(), String> {
    remove_old_integration(game_dir);
    let songs_dir = game_dir.join("addons").join(ADDON_NAME).join("songs");
    std::fs::create_dir_all(&songs_dir)
        .map_err(|e| format!("non riesco a creare la cartella dell'addon: {e}"))?;
    std::fs::write(songs_dir.join(SONG_SCRIPT), SONG_HX)
        .map_err(|e| format!("non riesco a scrivere lo script: {e}"))
}

fn remove_integration(game_dir: &Path) {
    remove_old_integration(game_dir);
    let _ = std::fs::remove_dir_all(game_dir.join("addons").join(ADDON_NAME));
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

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
pub struct SongEvent {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub sent_at: f64,
    pub song_name: Option<String>,
    pub difficulty: Option<String>,
    pub score: Option<i64>,
    pub accuracy: Option<f32>,
    #[serde(default)]
    pub misses: u32,
    #[serde(default)]
    pub miss: bool,
    #[serde(default = "yes")]
    pub valid: bool,
    #[serde(default)]
    pub reason: Option<String>,
    pub mod_folder: Option<String>,
    pub game_dir: Option<String>,
    pub exe: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub song_id: Option<String>,
    #[serde(default)]
    pub variation: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl SongEvent {
    pub fn is_funkin(&self) -> bool {
        self.engine.as_deref() == Some("funkin")
    }

    pub fn is_psych(&self) -> bool {
        self.engine.as_deref() == Some("psych")
    }

    pub fn is_nmv(&self) -> bool {
        self.engine.as_deref() == Some("nmv")
    }

    pub fn is_kade(&self) -> bool {
        self.engine.as_deref() == Some("kade")
    }

    pub fn is_gd(&self) -> bool {
        self.engine.as_deref() == Some("gd")
    }
}

type EventKey = (String, Option<String>, Option<String>, u32, Option<i64>);

fn already_seen(ev: &SongEvent) -> bool {
    if ev.is_gd() {
        return false;
    }
    static SEEN: OnceLock<Mutex<VecDeque<(EventKey, f64)>>> = OnceLock::new();
    let key: EventKey = (
        ev.kind.clone(),
        ev.song_name.clone(),
        ev.difficulty.clone(),
        ev.misses,
        ev.score,
    );
    let mut seen = SEEN.get_or_init(Default::default).lock().unwrap();
    if seen
        .iter()
        .any(|(k, at)| *k == key && (ev.sent_at - at).abs() < 1500.0)
    {
        return true;
    }
    seen.push_back((key, ev.sent_at));
    if seen.len() > 256 {
        seen.pop_front();
    }
    false
}

fn song_queue() -> &'static OnceLock<mpsc::UnboundedSender<SongEvent>> {
    static Q: OnceLock<mpsc::UnboundedSender<SongEvent>> = OnceLock::new();
    &Q
}

async fn accept_event(core: &Arc<Core>, ev: SongEvent) {
    if already_seen(&ev) {
        return;
    }
    if !FIRST_EVENT_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::info!(
            "primo evento {} ricevuto correttamente",
            if ev.is_funkin() {
                "Friday Night Funkin'"
            } else if ev.is_psych() {
                "Psych Engine"
            } else if ev.is_nmv() {
                "Nightmare Vision"
            } else if ev.is_kade() {
                "Kade Engine"
            } else if ev.is_gd() {
                "Geometry Dash"
            } else {
                "Codename Engine"
            }
        );
    }
    if ev.is_gd() {
        crate::gd::observe(core, &ev);
    }
    if ev.kind.starts_with("song_") || ev.kind.starts_with("results_") {
        if let Some(q) = song_queue().get() {
            let _ = q.send(ev);
        }
        return;
    }
    core.report_fnf_event(ev.song_name, ev.difficulty, ev.score, ev.accuracy, ev.miss)
        .await;
}

async fn on_event(
    State(core): State<Arc<Core>>,
    Json(ev): Json<SongEvent>,
) -> axum::http::StatusCode {
    accept_event(&core, ev).await;
    axum::http::StatusCode::NO_CONTENT
}

pub(crate) async fn tail_events(core: Arc<Core>, path: PathBuf, game_dir: Option<String>) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut offset = match &game_dir {
        Some(_) => tokio::fs::metadata(&path)
            .await
            .map(|m| m.len())
            .unwrap_or(0),
        None => 0,
    };
    let mut pending = String::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let Ok(mut file) = tokio::fs::File::open(&path).await else {
            continue;
        };
        let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        if len < offset {
            offset = 0;
            pending.clear();
        }
        if len == offset {
            continue;
        }
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
            continue;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).await.is_err() {
            continue;
        }
        offset += buf.len() as u64;
        pending.push_str(&String::from_utf8_lossy(&buf));
        while let Some(pos) = pending.find('\n') {
            let line: String = pending.drain(..=pos).collect();
            if let Ok(mut ev) = serde_json::from_str::<SongEvent>(line.trim()) {
                if game_dir.is_some() {
                    ev.game_dir = game_dir.clone();
                }
                if ev.sent_at <= 0.0 {
                    ev.sent_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                }
                accept_event(&core, ev).await;
            }
        }
    }
}

pub fn spawn(core: Arc<Core>) {
    for folder in list_folders(&core.data_dir()) {
        if let Err(e) = install_integration(Path::new(&folder)) {
            tracing::warn!("integrazione Codename Engine non aggiornata in {folder}: {e}");
        }
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<SongEvent>();
    let _ = song_queue().set(tx);
    let song_core = core.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            crate::songclip::handle(&song_core, ev).await;
        }
    });

    let _ = std::fs::remove_file(core.data_dir().join("fnf-event.json"));
    let events_path = core.data_dir().join("fnf-events.jsonl");
    let _ = std::fs::remove_file(&events_path);
    tauri::async_runtime::spawn(tail_events(core.clone(), events_path, None));

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

pub fn folder_for(folders: &[String], ev: &SongEvent) -> Option<String> {
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let places: Vec<String> = [ev.game_dir.as_deref(), ev.exe.as_deref()]
        .into_iter()
        .flatten()
        .map(norm)
        .collect();
    folders
        .iter()
        .find(|f| {
            let f = norm(f);
            places
                .iter()
                .any(|p| *p == f || p.starts_with(&format!("{f}\\")))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_matched_to_the_connected_mod_folder() {
        let folders = vec![
            r"D:\FNF\VS Impostor".to_string(),
            r"C:\Giochi\Hex".to_string(),
        ];
        let ev = |dir: &str, exe: &str| SongEvent {
            kind: "song_load".into(),
            sent_at: 0.0,
            song_name: None,
            difficulty: None,
            score: None,
            accuracy: None,
            misses: 0,
            miss: false,
            valid: true,
            reason: None,
            mod_folder: None,
            game_dir: Some(dir.into()),
            exe: Some(exe.into()),
            engine: None,
            song_id: None,
            variation: None,
            data: None,
        };
        assert_eq!(
            folder_for(
                &folders,
                &ev("d:/fnf/vs impostor/", r"D:\FNF\VS Impostor\Impostor.exe")
            ),
            Some(folders[0].clone())
        );
        assert_eq!(
            folder_for(&folders, &ev(r"C:\Altro", r"C:\Giochi\Hex\bin\Hex.exe")),
            Some(folders[1].clone())
        );
        assert_eq!(
            folder_for(&folders, &ev(r"D:\FNF\VS Impostor 2", r"D:\x.exe")),
            None
        );
    }

    #[test]
    fn the_same_event_from_two_script_copies_counts_once() {
        let ev = |kind: &str, sent_at: f64, misses: u32| SongEvent {
            kind: kind.into(),
            sent_at,
            song_name: Some("dedup-test".into()),
            difficulty: Some("hard".into()),
            score: Some(0),
            accuracy: None,
            misses,
            miss: false,
            valid: true,
            reason: None,
            mod_folder: None,
            game_dir: None,
            exe: None,
            engine: None,
            song_id: None,
            variation: None,
            data: None,
        };
        assert!(!already_seen(&ev("song_load", 1000.0, 0)));
        assert!(
            already_seen(&ev("song_load", 1002.0, 0)),
            "copia dell'addon a 2 ms"
        );
        assert!(!already_seen(&ev("tick", 1500.0, 1)));
        assert!(
            !already_seen(&ev("tick", 1600.0, 2)),
            "una nuova nota mancata non e' un doppione"
        );
        assert!(
            !already_seen(&ev("song_load", 9000.0, 0)),
            "riprovare la canzone dopo e' un evento nuovo"
        );
    }

    #[test]
    fn the_old_injected_block_is_removed_cleanly() {
        let original = "import flixel.FlxG;\n\nfunction update(elapsed:Float) {}\n";
        let injected = format!("{original}\n{INJECT_BEGIN}\nvar relaySeq:Int = 0;\n{INJECT_END}\n");
        assert_eq!(strip_injection(&injected).trim(), original.trim());
        assert_eq!(strip_injection(original), original);
    }
}

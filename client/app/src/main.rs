#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod core;
mod ffmpeg;
mod overlay;
mod updater;

use std::{sync::Arc, time::Duration};

use core::{Core, Created, Joined, RecentMatch};
use relay_agent::{
    presets::SpeedResult,
    settings::{Settings, WindowChoice},
    windows::WindowInfo,
};
use serde_json::Value;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use tauri_plugin_deep_link::DeepLinkExt;

type Res<T> = Result<T, String>;
type St<'a> = State<'a, Arc<Core>>;

#[tauri::command]
fn get_state(core: St) -> Value {
    core.snapshot()
}

#[tauri::command]
async fn login(core: St<'_>) -> Res<()> {
    core.login().await
}

#[tauri::command]
async fn logout(core: St<'_>) -> Res<()> {
    core.logout().await
}

#[tauri::command]
fn get_settings(core: St) -> Settings {
    core.settings()
}

#[tauri::command]
fn save_settings(core: St, settings: Settings) -> Res<Settings> {
    core.save_settings(settings)
}

#[tauri::command]
async fn list_windows(core: St<'_>) -> Res<Vec<WindowInfo>> {
    Ok(core.list_windows().await)
}

#[tauri::command]
fn detect_steam_game() -> Option<Value> {
    relay_agent::steam::detect_current_game()
        .map(|(app_id, name)| serde_json::json!({ "app_id": app_id, "name": name, "cover_url": format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{app_id}/library_600x900_2x.jpg") }))
}

#[tauri::command]
async fn search_steam_games(query: String) -> Res<Vec<relay_agent::steam::SteamGame>> {
    relay_agent::steam::search_catalog(&query)
        .await
        .map_err(|e| format!("Catalogo Steam non disponibile: {e}"))
}

#[tauri::command]
async fn run_speedtest(app: AppHandle, core: St<'_>) -> Res<SpeedResult> {
    core.run_speedtest(move |progress, mbps| {
        let _ = app.emit(
            "speedtest",
            serde_json::json!({ "progress": progress, "mbps": mbps }),
        );
    })
    .await
}

#[tauri::command]
async fn create_match(core: St<'_>, name: Option<String>) -> Res<Created> {
    core.create_match(name).await
}

#[tauri::command]
async fn rename_match(core: St<'_>, name: String) -> Res<()> {
    core.rename_match(&name).await
}

#[tauri::command]
async fn join_match(core: St<'_>, link: String) -> Res<Joined> {
    core.join_match(&link).await
}

#[tauri::command]
async fn accept_invite(core: St<'_>) -> Res<Joined> {
    core.accept_invite().await
}

#[tauri::command]
fn decline_invite(core: St) {
    core.decline_invite();
}

#[tauri::command]
async fn open_match(core: St<'_>, id: String) -> Res<()> {
    core.open_match(&id).await
}

#[tauri::command]
fn set_window(core: St, window: Option<WindowChoice>) -> Res<()> {
    core.set_window(window)
}

#[tauri::command]
async fn apply_update(app: AppHandle, core: St<'_>) -> Res<()> {
    core.apply_update()?;

    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn retry_ffmpeg(core: St<'_>) -> Res<()> {
    core.ensure_ffmpeg().await;
    Ok(())
}

#[tauri::command]
async fn retry_capture(core: St<'_>) -> Res<()> {
    core.ensure_capture().await;
    Ok(())
}

#[tauri::command]
fn skip_capture(core: St) -> Res<()> {
    core.skip_capture_check();
    Ok(())
}

#[tauri::command]
async fn check_update(core: St<'_>) -> Res<Value> {
    Ok(core.check_update_now().await)
}

#[tauri::command]
fn open_logs() -> Res<()> {
    let dir = Settings::default_path()
        .parent()
        .map(|p| p.join("logs"))
        .ok_or("Cartella dei log non trovata.")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let opener = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Non riesco ad aprire la cartella: {e}"))
}

#[tauri::command]
fn set_audio(core: St, game: bool, mic: bool, mic_gain: f32) -> Res<()> {
    core.set_audio(game, mic, mic_gain)
}

#[tauri::command]
async fn leave_match(core: St<'_>) -> Res<()> {
    core.leave_match().await;
    Ok(())
}

#[tauri::command]
async fn list_matches(core: St<'_>) -> Res<Vec<RecentMatch>> {
    core.list_matches().await
}

#[tauri::command]
async fn host_start(
    core: St<'_>,
    force: bool,
    game: Option<relay_agent::steam::SteamGame>,
) -> Res<()> {
    core.host_start(force, game).await
}

#[tauri::command]
async fn host_stop(core: St<'_>) -> Res<()> {
    core.host_stop().await
}

#[tauri::command]
fn open_url(url: String) -> Res<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Indirizzo non valido.".into());
    }
    relay_agent::login::open_browser(&url).map_err(|e| e.to_string())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "relay_agent=info,relay_app=info".into());
    let dir = Settings::default_path()
        .parent()
        .map(|p| p.join("logs"))
        .unwrap_or_else(|| std::path::PathBuf::from("logs"));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("relay.log");

    if std::fs::metadata(&path)
        .map(|m| m.len() > 2 * 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("relay.old.log"));
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
    tracing::info!("Relay {} avviato", updater::current_version());
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    init_logging();

    updater::cleanup_old();
    let started = std::time::Instant::now();
    let core = Core::new(Settings::default_path());

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .manage(core.clone())
        .invoke_handler(tauri::generate_handler![
            get_state,
            login,
            logout,
            get_settings,
            save_settings,
            list_windows,
            detect_steam_game,
            search_steam_games,
            run_speedtest,
            create_match,
            rename_match,
            join_match,
            accept_invite,
            decline_invite,
            open_match,
            set_window,
            set_audio,
            apply_update,
            check_update,
            open_logs,
            retry_ffmpeg,
            retry_capture,
            skip_capture,
            leave_match,
            list_matches,
            host_start,
            host_stop,
            open_url
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            overlay::create(app.handle())?;

            let _ = app.deep_link().register_all();
            {
                let (h, c) = (app.handle().clone(), core.clone());
                app.deep_link().on_open_url(move |ev| {
                    for u in ev.urls() {
                        c.offer_invite(u.as_str());
                    }
                    show_main(&h);
                });
            }
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for u in urls {
                    core.offer_invite(u.as_str());
                }
            }

            let show = MenuItem::with_id(app, "show", "Mostra Relay", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Esci", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let core_q = core.clone();
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("icona dell'app"))
                .tooltip("Relay")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, ev| match ev.id().as_ref() {
                    "show" => show_main(app),
                    "quit" => {
                        let (app, core) = (app.clone(), core_q.clone());
                        tauri::async_runtime::spawn(async move {
                            core.leave_match().await;
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            app.exit(0);
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            {
                let core = core.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        core.ensure_ffmpeg().await;
                        let wait = if core.ffmpeg_ready() { 6 * 3600 } else { 30 };
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                    }
                });
            }
            {
                let core = core.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        core.ensure_capture().await;
                        let wait = if core.capture_ready() { 6 * 3600 } else { 30 };
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                    }
                });
            }

            {
                let (handle, core) = (app.handle().clone(), core.clone());
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(8)).await;
                    loop {
                        core.check_update().await;
                        for _ in 0..(6 * 3600 / 20) {
                            if core.update_ready() && core.is_idle() {
                                let hidden = handle
                                    .get_webview_window("main")
                                    .and_then(|w| w.is_visible().ok())
                                    == Some(false);
                                if (hidden || started.elapsed() < Duration::from_secs(120))
                                    && core.apply_update().is_ok()
                                {
                                    handle.exit(0);
                                    return;
                                }
                            }
                            tokio::time::sleep(Duration::from_secs(20)).await;
                        }
                    }
                });
            }

            let handle = app.handle().clone();
            let core = core.clone();
            tauri::async_runtime::spawn(async move {
                core.init().await;
                let (mut last, mut overlay_on) = (String::new(), false);
                loop {
                    core.refresh_info().await;
                    let snap = core.snapshot();
                    let text = snap.to_string();
                    if text != last {
                        let _ = handle.emit("state", &snap);
                        last = text;
                    }
                    let want = core.overlay_wanted(&snap);
                    if want != overlay_on {
                        overlay::set_visible(&handle, want);
                        overlay_on = want;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("errore nell'avvio dell'app");
}

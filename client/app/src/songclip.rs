use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use relay_agent::{
    audio::AudioChoice,
    capture::{Encoder, WindowSel},
    obs_capture::{ObsCapture, ObsCaptureConfig},
};
use relay_capture::ipc::EncoderChoice;
use reqwest::Method;
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

use crate::{core::Core, fnf::SongEvent};

const PART_BYTES: usize = 16 * 1024 * 1024;
const RESULTS_MAX_MS: f64 = 30_000.0;

struct Session {
    capture: ObsCapture,
    dir: PathBuf,
    exe: String,
}

#[derive(Clone)]
struct Run {
    engine: &'static str,
    mod_name: String,
    aliases: Vec<String>,
    song_id: Option<String>,
    song: String,
    difficulty: String,
    variation: Option<String>,
    loaded_at: f64,
    start_ms: Option<f64>,
    generation: u32,
    first_frame: Option<f64>,
}

struct Ending {
    core: Arc<Core>,
    run: Run,
    start_ms: f64,
    ev: SongEvent,
    since: Instant,
}

struct Pending {
    dir: PathBuf,
    from_ms: f64,
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    run: Option<Run>,
    ending: Option<Ending>,
    pending: Vec<(u64, Pending)>,
    retired: Vec<PathBuf>,
    next_id: u64,
    monitor: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

pub async fn handle(core: &Arc<Core>, ev: SongEvent) {
    let mut st = state().lock().await;
    if matches!(ev.kind.as_str(), "results_end" | "song_load" | "song_start") {
        if let Some(e) = st.ending.take() {
            let end_ms = ev.sent_at.min(e.ev.sent_at + RESULTS_MAX_MS);
            spawn_finish(&mut st, e, end_ms);
        }
    }
    match ev.kind.as_str() {
        "song_load" => {
            st.run = None;
            let s = core.settings();
            if !s.fnf_autorecord || !ev.valid {
                return;
            }
            let Some(target) = target_for(core, &ev) else {
                return;
            };
            if st
                .session
                .as_ref()
                .is_some_and(|x| !x.exe.eq_ignore_ascii_case(&target.exe))
            {
                retire(&mut st).await;
            }
            if st.session.is_none() {
                let Some(session) = start_session(core, &target.exe) else {
                    return;
                };
                st.session = Some(session);
                if !st.monitor {
                    st.monitor = true;
                    tauri::async_runtime::spawn(monitor());
                }
            }
            let (generation, first_frame) = match st.session.as_mut() {
                Some(session) => session.capture.fit_window().await.map_or((0, None), |(g, f)| (g, Some(f))),
                None => (0, None),
            };
            tracing::info!("registro la canzone {} ({})", target.song, target.mod_name);
            st.run = Some(Run {
                engine: if ev.is_funkin() {
                    "funkin"
                } else if ev.is_psych() {
                    "psych"
                } else if ev.is_nmv() {
                    "nmv"
                } else if ev.is_kade() {
                    "kade"
                } else if ev.is_gd() {
                    "gd"
                } else {
                    "codename"
                },
                mod_name: target.mod_name,
                aliases: target.aliases,
                song_id: ev
                    .song_id
                    .clone()
                    .or_else(|| (ev.is_psych() || ev.is_nmv()).then(|| crate::psych::song_path(&target.song))),
                song: target.song,
                difficulty: ev.difficulty.clone().unwrap_or_else(|| "normal".into()),
                variation: ev.variation.clone().filter(|v| !v.is_empty() && v != "default"),
                loaded_at: ev.sent_at,
                start_ms: None,
                generation,
                first_frame,
            });
        }
        "song_start" => {
            if st.run.as_ref().is_some_and(|r| r.engine == "gd") {
                if let Some(run) = st.run.as_mut() {
                    run.start_ms = ev.valid.then_some(ev.sent_at);
                }
                return;
            }
            if !ev.valid {
                if st.run.take().is_some() {
                    tracing::info!("canzone in botplay o practice: non la registro");
                }
            } else if let Some(run) = st.run.as_mut() {
                run.start_ms = Some(ev.sent_at);
            }
        }
        "song_end" => {
            let keep = st.run.as_ref().filter(|r| r.engine == "gd").map(|r| Run {
                start_ms: None,
                ..r.clone()
            });
            let Some(run) = st.run.take() else {
                return;
            };
            st.run = keep;
            let (Some(start_ms), true) = (run.start_ms, ev.valid) else {
                tracing::info!(
                    "{} finita ma non valida (botplay o practice): nessuna clip",
                    run.song
                );
                return;
            };
            let tail = match (run.engine, ev.data.as_ref().and_then(|d| d["completed"].as_bool())) {
                ("gd", Some(true)) => 2500.0,
                ("gd", _) => 800.0,
                _ => 0.0,
            };
            let end_ms = ev.sent_at + tail;
            let ending = Ending {
                core: core.clone(),
                run,
                start_ms,
                ev,
                since: Instant::now(),
            };
            if ending.run.engine == "funkin" {
                st.ending = Some(ending);
            } else {
                spawn_finish(&mut st, ending, end_ms);
            }
        }
        "song_abort" => {
            if st.run.as_ref().is_some_and(|r| r.engine == "gd")
                && ev.reason.as_deref() != Some("uscita dal livello")
            {
                if let Some(run) = st.run.as_mut() {
                    run.start_ms = None;
                }
                return;
            }
            if let Some(run) = st.run.take() {
                tracing::info!(
                    "{} interrotta ({}): nessuna clip",
                    run.song,
                    ev.reason.as_deref().unwrap_or("motivo sconosciuto")
                );
            }
        }
        _ => {}
    }
}

fn spawn_finish(st: &mut State, e: Ending, end_ms: f64) {
    let Some(session) = st.session.as_ref() else {
        return;
    };
    let dir = session.dir.clone();
    let first_frame = e.run.first_frame.or_else(|| session.capture.first_frame_unix_secs());
    let id = st.next_id;
    st.next_id += 1;
    st.pending.push((
        id,
        Pending {
            dir: dir.clone(),
            from_ms: e.start_ms,
        },
    ));
    tokio::spawn(async move {
        finish(&e.core, &dir, first_frame, &e.run, e.start_ms, end_ms, &e.ev).await;
        let mut st = state().lock().await;
        st.pending.retain(|(p, _)| *p != id);
    });
}

struct Target {
    exe: String,
    mod_name: String,
    aliases: Vec<String>,
    song: String,
}

fn target_for(core: &Arc<Core>, ev: &SongEvent) -> Option<Target> {
    if ev.is_funkin() {
        let folder = PathBuf::from(ev.game_dir.as_deref()?);
        let exe = if folder.join("Funkin.exe").is_file() {
            "Funkin.exe".to_string()
        } else {
            game_exe_in(&folder)?
        };
        let song = ev.song_name.clone().filter(|s| !s.trim().is_empty())?;
        let (mod_name, aliases) =
            crate::funkin::mod_for(&folder, ev.song_id.as_deref()?, ev.variation.as_deref());
        return Some(Target {
            exe,
            mod_name,
            aliases,
            song,
        });
    }
    if ev.is_gd() {
        let song = ev.song_name.clone().filter(|s| !s.trim().is_empty())?;
        return Some(Target {
            exe: "GeometryDash.exe".into(),
            mod_name: ev.mod_folder.clone().unwrap_or_else(|| "Livelli online".into()),
            aliases: Vec::new(),
            song,
        });
    }
    if ev.is_kade() {
        let folder = ev.game_dir.clone()?;
        let exe = running_exe_in(Path::new(&folder))?;
        let song = ev.song_name.clone().filter(|s| !s.trim().is_empty())?;
        let (mod_name, aliases) = crate::kade::mod_identity(Path::new(&folder));
        return Some(Target {
            exe,
            mod_name,
            aliases,
            song,
        });
    }
    if ev.is_psych() || ev.is_nmv() {
        let folder = ev.game_dir.clone()?;
        let exe = running_exe_in(Path::new(&folder))?;
        let song = ev.song_name.clone().filter(|s| !s.trim().is_empty())?;
        let kind = if ev.is_nmv() { crate::psych::Kind::Nmv } else { crate::psych::Kind::Psych };
        let (mod_name, aliases) = crate::psych::mod_identity(
            Path::new(&folder),
            kind,
            ev.mod_folder.as_deref().unwrap_or("").trim(),
        );
        return Some(Target {
            exe,
            mod_name,
            aliases,
            song,
        });
    }
    let folders = crate::fnf::list_folders(&core.data_dir());
    let unknown_place = ev.game_dir.is_none() && ev.exe.is_none();
    let folder = crate::fnf::folder_for(&folders, ev)
        .or_else(|| (unknown_place && folders.len() == 1).then(|| folders[0].clone()))?;
    let exe = match ev.exe.as_deref().and_then(|p| Path::new(p).file_name()) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => game_exe_in(Path::new(&folder))?,
    };
    let song = ev.song_name.clone().filter(|s| !s.trim().is_empty())?;
    let inner = ev.mod_folder.clone().filter(|m| !m.trim().is_empty());
    let mod_name = match &inner {
        Some(m) => crate::modname::strip_version(m),
        None => crate::modname::game_name(Path::new(&folder)),
    };
    let old = [inner.clone().unwrap_or_default(), folder_name(&folder)];
    let aliases = crate::modname::aliases(&mod_name, &[&old[0], &old[1]]);
    Some(Target {
        exe,
        mod_name,
        aliases,
        song,
    })
}

fn start_session(core: &Arc<Core>, exe: &str) -> Option<Session> {
    if !core.capture_ready() {
        tracing::warn!("clip della canzone saltata: il motore di registrazione non e' pronto");
        return None;
    }
    let s = core.settings();
    let runtime = relay_capture::install::runtime_dir(&core.data_dir());
    let dir = core
        .data_dir()
        .join("song-clips")
        .join((now_ms() as u64).to_string());
    let encoder = match s.encoder_choice() {
        Some(Encoder::Nvenc) => EncoderChoice::Nvenc,
        Some(Encoder::Amf) => EncoderChoice::Amf,
        Some(Encoder::Qsv) => EncoderChoice::Qsv,
        Some(Encoder::X264) => EncoderChoice::X264,
        None => EncoderChoice::Auto,
    };
    let capture = ObsCapture::spawn(&ObsCaptureConfig {
        exe: relay_capture::install::host_exe(&runtime),
        cwd: relay_capture::install::spawn_dir(&runtime),
        window: WindowSel::Exe(exe.to_string()),
        fallback_monitor: None,
        fallback_monitor_name: None,
        fps: s.fps,
        bitrate_kbps: s.bitrate_kbps,
        dir: dir.clone(),
        encoder,
        origin_unix_secs: None,
        start_segment: 0,
        generation: 0,
        audio: AudioChoice {
            game: true,
            mic: s.fnf_mic,
            mic_gain: s.audio_mic_gain,
        },
        segment_secs: 1,
    })
    .map_err(|e| tracing::warn!("registrazione delle canzoni non avviata: {e:#}"))
    .ok()?;
    tracing::info!("registrazione continua di {exe} avviata");
    Some(Session {
        capture,
        dir,
        exe: exe.to_string(),
    })
}

async fn retire(st: &mut State) {
    if let Some(mut s) = st.session.take() {
        let _ = s.capture.stop().await;
        tracing::info!("registrazione continua di {} fermata", s.exe);
        st.retired.push(s.dir);
    }
    st.run = None;
}

fn game_is_open(exe: &str) -> bool {
    relay_agent::windows::list_windows()
        .iter()
        .any(|w| w.exe.eq_ignore_ascii_case(exe))
}

fn connected_exes(core: &Arc<Core>) -> Vec<String> {
    let base = core.data_dir();
    let mut folders: Vec<String> = crate::funkin::list_folders(&base);
    folders.extend(crate::fnf::list_folders(&base));
    folders.extend(crate::psych::list_folders(&base, crate::psych::Kind::Psych));
    folders.extend(crate::psych::list_folders(&base, crate::psych::Kind::Nmv));
    folders.extend(crate::kade::list_folders(&base));
    let mut exes: Vec<String> = folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let l = n.to_lowercase();
            (l.ends_with(".exe") && !l.starts_with("unins") && !l.contains("crash")).then_some(n)
        })
        .collect();
    if !crate::gd::list_folders(&base).is_empty() {
        exes.push("GeometryDash.exe".into());
    }
    exes.sort_by_key(|e| e.to_lowercase());
    exes.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    exes
}

fn watch_focus(core: Arc<Core>) {
    std::thread::spawn(move || {
        let mut games: Vec<String> = Vec::new();
        let mut refreshed = Instant::now() - Duration::from_secs(60);
        let mut last: Option<(String, String, String)> = None;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if refreshed.elapsed() > Duration::from_secs(10) {
                games = connected_exes(&core);
                refreshed = Instant::now();
            }
            let now = relay_agent::windows::foreground();
            let was_game = last.as_ref().is_some_and(|l| games.iter().any(|g| g.eq_ignore_ascii_case(&l.0)));
            let still_game = now.as_ref().is_some_and(|n| last.as_ref().is_some_and(|l| n.0.eq_ignore_ascii_case(&l.0)));
            if was_game && !still_game {
                let (exe, class, title) = now.clone().unwrap_or_default();
                tracing::info!(
                    "{} ha perso il primo piano: ora c'e' {exe} (classe {class}, titolo '{title}')",
                    last.as_ref().map(|l| l.0.as_str()).unwrap_or("?")
                );
            }
            last = now;
        }
    });
}

pub fn watch(core: Arc<Core>) {
    watch_focus(core.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if !core.settings().fnf_autorecord || state().lock().await.session.is_some() {
                continue;
            }
            let exes = connected_exes(&core);
            if exes.is_empty() {
                continue;
            }
            let open = tokio::task::spawn_blocking(move || {
                let windows = relay_agent::windows::list_windows();
                exes.into_iter().find(|e| windows.iter().any(|w| w.exe.eq_ignore_ascii_case(e)))
            })
            .await
            .ok()
            .flatten();
            let Some(exe) = open else {
                continue;
            };
            let mut st = state().lock().await;
            if st.session.is_some() {
                continue;
            }
            if let Some(session) = start_session(&core, &exe) {
                tracing::info!("{exe} aperto: registrazione pronta prima della partita");
                st.session = Some(session);
                if !st.monitor {
                    st.monitor = true;
                    tauri::async_runtime::spawn(monitor());
                }
            }
        }
    });
}

async fn monitor() {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let exe = state().lock().await.session.as_ref().map(|s| s.exe.clone());
        let open = match &exe {
            Some(exe) => {
                let exe = exe.clone();
                tokio::task::spawn_blocking(move || game_is_open(&exe))
                    .await
                    .unwrap_or(true)
            }
            None => true,
        };
        let mut st = state().lock().await;
        let overdue = st
            .ending
            .as_ref()
            .is_some_and(|e| !open || e.since.elapsed().as_secs_f64() * 1000.0 > RESULTS_MAX_MS);
        if overdue {
            if let Some(e) = st.ending.take() {
                let end_ms = now_ms().min(e.ev.sent_at + RESULTS_MAX_MS);
                spawn_finish(&mut st, e, end_ms);
            }
        }
        if st.session.is_some() && !open {
            retire(&mut st).await;
        }
        cleanup(&mut st).await;
    }
}

async fn cleanup(st: &mut State) {
    let busy: Vec<PathBuf> = st.pending.iter().map(|(_, p)| p.dir.clone()).collect();
    let retired = std::mem::take(&mut st.retired);
    for dir in retired {
        if busy.contains(&dir) {
            st.retired.push(dir);
        } else {
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
    }
    let Some(session) = st.session.as_ref() else {
        return;
    };
    let generations = session.capture.generations();
    if generations.is_empty() {
        return;
    }
    let keep_from_ms = st
        .pending
        .iter()
        .filter(|(_, p)| p.dir == session.dir)
        .map(|(_, p)| p.from_ms)
        .chain(st.run.as_ref().map(|r| r.loaded_at))
        .chain(st.ending.as_ref().map(|e| e.start_ms))
        .fold(now_ms(), f64::min)
        - 10_000.0;
    for (generation, first_frame) in generations {
        let keep_from = keep_from_ms / 1000.0 - first_frame;
        let playlist = session.dir.join(relay_agent::hls::playlist_name(generation));
        let Ok(text) = tokio::fs::read_to_string(&playlist).await else {
            continue;
        };
        for seg in parse_segments(&text) {
            if seg.start + seg.duration < keep_from {
                let _ = tokio::fs::remove_file(session.dir.join(&seg.name)).await;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Segment {
    name: String,
    start: f64,
    duration: f64,
}

fn parse_segments(playlist: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut at = 0.0;
    let mut pending: Option<f64> = None;
    for line in playlist.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            pending = rest.split(',').next().and_then(|d| d.parse().ok());
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(duration) = pending.take() {
                out.push(Segment {
                    name: line.to_string(),
                    start: at,
                    duration,
                });
                at += duration;
            }
        }
    }
    out
}

fn pick(segments: &[Segment], from: f64, to: f64) -> Option<(Vec<Segment>, f64)> {
    let chosen: Vec<Segment> = segments
        .iter()
        .filter(|s| s.start + s.duration > from && s.start < to)
        .cloned()
        .collect();
    let offset = (from - chosen.first()?.start).max(0.0);
    Some((chosen, offset))
}

struct RawClip {
    bytes: Vec<u8>,
    offset: f64,
    duration: f64,
}

async fn raw_clip(dir: &Path, generation: u32, first_frame: f64, start_ms: f64, end_ms: f64) -> Result<RawClip, String> {
    let from = start_ms / 1000.0 - first_frame;
    let to = end_ms / 1000.0 - first_frame;
    let playlist = dir.join(relay_agent::hls::playlist_name(generation));
    let deadline = Instant::now() + Duration::from_secs(15);
    let segments = loop {
        let text = tokio::fs::read_to_string(&playlist)
            .await
            .map_err(|e| format!("registrazione non trovata: {e}"))?;
        let segs = parse_segments(&text);
        let covered = segs.last().is_some_and(|s| s.start + s.duration >= to);
        if covered || Instant::now() > deadline {
            break segs;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let (chosen, offset) = pick(&segments, from, to).ok_or("nessun segmento per la canzone")?;
    let mut bytes = Vec::new();
    for seg in &chosen {
        let data = tokio::fs::read(dir.join(&seg.name))
            .await
            .map_err(|e| format!("pezzo {} non leggibile: {e}", seg.name))?;
        bytes.extend_from_slice(&data);
    }
    Ok(RawClip { bytes, offset, duration: (to - from).max(1.0) })
}

fn running_exe_in(folder: &Path) -> Option<String> {
    let names: Vec<String> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".exe") && !l.starts_with("unins") && !l.contains("crash")
        })
        .collect();
    let open = relay_agent::windows::list_windows();
    names
        .iter()
        .find(|n| open.iter().any(|w| w.exe.eq_ignore_ascii_case(n)))
        .or(names.first())
        .cloned()
}

fn game_exe_in(folder: &Path) -> Option<String> {
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| {
            let l = n.to_lowercase();
            l.ends_with(".exe") && !l.starts_with("unins")
        })
}

fn folder_name(folder: &str) -> String {
    Path::new(folder.trim_end_matches(['\\', '/']))
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.to_string())
}

async fn finish(
    core: &Arc<Core>,
    dir: &Path,
    first_frame: Option<f64>,
    run: &Run,
    start_ms: f64,
    end_ms: f64,
    ev: &SongEvent,
) {
    let score = ev.score.unwrap_or(0);
    match save_record(core, dir, first_frame, run, start_ms, end_ms, ev).await {
        Ok(true) => {
            tracing::info!("nuovo record su {}: {score}", run.song);
            if let Some(app) = APP.get() {
                let _ = app
                    .notification()
                    .builder()
                    .title("Nuovo record!")
                    .body(if run.engine == "gd" {
                        format!(
                            "{}: {}. La clip e' sul sito, in Giochi.",
                            run.song,
                            crate::gd::result_text(ev.data.as_ref(), run.difficulty == "platformer")
                        )
                    } else {
                        format!(
                            "{} ({}): {} punti. La clip e' sul sito, in Giochi.",
                            run.song,
                            run.difficulty,
                            score_text(score)
                        )
                    })
                    .show();
            }
        }
        Ok(false) => tracing::info!("{}: {score} non batte il record, clip scartata", run.song),
        Err(e) => tracing::warn!("clip della canzone {} non salvata: {e}", run.song),
    }
}

fn score_text(score: i64) -> String {
    let digits = score.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    if score < 0 {
        format!("-{out}")
    } else {
        out
    }
}

async fn save_record(
    core: &Arc<Core>,
    dir: &Path,
    first_frame: Option<f64>,
    run: &Run,
    start_ms: f64,
    end_ms: f64,
    ev: &SongEvent,
) -> Result<bool, String> {
    let score = ev.score.unwrap_or(0);
    let api = format!("/api/{}", run.engine);
    let mut url = reqwest::Url::parse(&format!("http://relay{api}/best")).expect("url fisso");
    url.query_pairs_mut()
        .append_pair("mod_name", &run.mod_name)
        .append_pair("song", &run.song)
        .append_pair("difficulty", &run.difficulty);
    if let Some(v) = &run.variation {
        url.query_pairs_mut().append_pair("variation", v);
    }
    if !run.aliases.is_empty() {
        url.query_pairs_mut().append_pair("aliases", &run.aliases.join("|"));
    }
    let best: Value = core
        .json(
            Method::GET,
            &format!("{}?{}", url.path(), url.query().unwrap_or("")),
            None,
        )
        .await?;
    if best["score"].as_i64().is_some_and(|b| score <= b) {
        return Ok(false);
    }

    let first_frame = first_frame.ok_or("la registrazione non era ancora partita")?;
    let raw = raw_clip(dir, run.generation, first_frame, start_ms, end_ms).await?;
    let duration_ms = (end_ms - start_ms).max(1000.0) as u64;
    let bytes = raw.bytes;

    let upload = uuid::Uuid::new_v4();
    let parts: Vec<&[u8]> = bytes.chunks(PART_BYTES).collect();
    for (i, part) in parts.iter().enumerate() {
        let resp = core
            .call_bytes(
                Method::PUT,
                &format!("{api}/uploads/{upload}/{i}"),
                part.to_vec(),
            )
            .await?;
        if !resp.status().is_success() {
            return Err(format!(
                "upload del pezzo {i} rifiutato ({})",
                resp.status()
            ));
        }
    }
    let resp = core
        .call(
            Method::POST,
            &format!("{api}/uploads/{upload}/finish"),
            Some(json!({
                "mod_name": run.mod_name,
                "aliases": run.aliases,
                "extra": ev.data,
                "song_id": run.song_id,
                "song": run.song,
                "difficulty": run.difficulty,
                "variation": run.variation,
                "score": score,
                "accuracy": ev.accuracy,
                "misses": ev.misses,
                "duration_ms": duration_ms,
                "parts": parts.len(),
                "size": bytes.len(),
                "raw": { "offset_secs": raw.offset, "duration_secs": raw.duration },
            })),
        )
        .await?;
    match resp.status().as_u16() {
        200..=299 => Ok(true),
        409 => Ok(false),
        s => Err(format!("il server ha rifiutato la clip ({s})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn scores_use_italian_thousands_separator() {
        assert_eq!(score_text(184320), "184.320");
        assert_eq!(score_text(999), "999");
        assert_eq!(score_text(-1500), "-1.500");
    }

    #[test]
    fn a_song_is_cut_from_the_segments_it_spans() {
        let playlist = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n\
            #EXTINF:1.000000,\nseg_00000000.ts\n#EXTINF:1.000000,\nseg_00000001.ts\n\
            #EXTINF:1.016000,\nseg_00000002.ts\n#EXTINF:0.984000,\nseg_00000003.ts\n\
            #EXTINF:1.000000,\nseg_00000004.ts\n";
        let segs = parse_segments(playlist);
        assert_eq!(segs.len(), 5);
        assert!((segs[3].start - 3.016).abs() < 1e-9);

        let (chosen, offset) = pick(&segs, 1.5, 3.5).unwrap();
        let names: Vec<&str> = chosen.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["seg_00000001.ts", "seg_00000002.ts", "seg_00000003.ts"]
        );
        assert!((offset - 0.5).abs() < 1e-9);

        assert!(
            pick(&segs, 10.0, 12.0).is_none(),
            "fuori dalla registrazione"
        );
    }
}

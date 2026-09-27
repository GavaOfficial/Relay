#[cfg(feature = "obs")]
mod obs;

use std::{
    io::BufRead,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use anyhow::Result;
use relay_capture::{
    fallback,
    ipc::{
        self, legacy_encoder_id, playlist_generation, EncoderChoice, Event, MonitorInfo,
        RecordConfig, Source, WindowInfo, ENGINE_OBS, ENGINE_RELAY,
    },
};
use relay_recorder::{Config, EncoderPref, EventSink, FrameCheck, Target};

const FIRST_FRAMES: Duration = Duration::from_secs(4);

pub fn emit(ev: &Event) {
    print!("{}", ipc::encode(ev));
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let engine = args
        .iter()
        .position(|a| a == "--engine")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let result = if args.iter().any(|a| a == "--probe") {
        probe(engine.as_deref())
    } else if let Some(i) = args.iter().position(|a| a == "--config") {
        let path = args.get(i + 1).expect("--config richiede un percorso");
        let text = std::fs::read_to_string(path).expect("configurazione non leggibile");
        let cfg: RecordConfig = serde_json::from_str(&text).expect("configurazione non valida");
        record(cfg, engine.as_deref())
    } else {
        eprintln!("uso: relay-capture --probe | --config <file.json> [--engine relay|obs]");
        std::process::exit(2);
    };

    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            emit(&Event::Error(format!("{e:#}")));
            std::process::exit(1);
        }
    }
}

fn runtime_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn use_relay(dir: &Path, engine: Option<&str>) -> bool {
    match engine {
        Some(ENGINE_OBS) => false,
        Some(ENGINE_RELAY) => true,
        _ => match fallback::obs_reason(dir, version()) {
            Some(reason) => {
                emit(&Event::Warning(format!(
                    "su questo PC registro con OBS: il motore di Relay non aveva funzionato ({reason})"
                )));
                false
            }
            None => true,
        },
    }
}

fn give_up_relay(dir: &Path, reason: String) {
    fallback::remember_obs(dir, version(), &reason);
    emit(&Event::Fallback { reason });
}

fn probe(engine: Option<&str>) -> Result<()> {
    let dir = runtime_dir();
    if use_relay(&dir, engine) {
        match relay_recorder::probe() {
            Ok(p) => {
                emit(&Event::Ready {
                    obs_version: String::new(),
                    engine: ENGINE_RELAY.into(),
                });
                let mut encoders = p.encoders.clone();
                encoders.extend(
                    p.encoders
                        .iter()
                        .filter_map(|e| legacy_encoder_id(e))
                        .map(String::from),
                );
                emit(&Event::Probed {
                    encoders,
                    monitors: p
                        .monitors
                        .iter()
                        .map(|m| MonitorInfo {
                            index: m.index,
                            width: m.width,
                            height: m.height,
                        })
                        .collect(),
                    windows: p
                        .windows
                        .iter()
                        .map(|w| WindowInfo {
                            exe: w.exe.clone(),
                            title: w.title.clone(),
                        })
                        .collect(),
                    best: Some(p.best),
                });
                return Ok(());
            }
            Err(e) => give_up_relay(&dir, format!("{e:#}")),
        }
    }
    obs_probe(&dir)
}

#[cfg(feature = "obs")]
fn obs_probe(dir: &Path) -> Result<()> {
    if !relay_capture::install::is_ready(dir) {
        anyhow::bail!("serve OBS come riserva ma non e' ancora installato");
    }
    enter_obs(dir)?;
    obs::probe()
}

#[cfg(not(feature = "obs"))]
fn obs_probe(_dir: &Path) -> Result<()> {
    anyhow::bail!(
        "il motore di Relay non funziona su questo PC e questo relay-capture non include OBS"
    )
}

enum Failure {
    Fallback(String),
    Broken(anyhow::Error),
}

fn record(cfg: RecordConfig, engine: Option<&str>) -> Result<()> {
    let dir = runtime_dir();
    if use_relay(&dir, engine) {
        match record_relay(&cfg) {
            Ok(()) => return Ok(()),
            Err(Failure::Broken(e)) => {
                fallback::remember_obs(
                    &dir,
                    version(),
                    &format!("errore durante la registrazione: {e:#}"),
                );
                return Err(e);
            }
            Err(Failure::Fallback(reason)) => give_up_relay(&dir, reason),
        }
    }
    record_obs(cfg, &dir)
}

#[cfg(feature = "obs")]
fn record_obs(cfg: RecordConfig, dir: &Path) -> Result<()> {
    if !relay_capture::install::is_ready(dir) {
        emit(&Event::Warning(
            "scarico OBS per registrare come riserva".into(),
        ));
        let mut last = -10.0f32;
        relay_capture::install::install(
            dir,
            &relay_capture::install::Canceller::new(),
            &mut |p| {
                if p.percent - last >= 10.0 {
                    last = p.percent;
                    emit(&Event::Warning(format!("scarico OBS: {:.0}%", p.percent)));
                }
            },
        )?;
    }
    enter_obs(dir)?;
    obs::record(cfg)
}

#[cfg(not(feature = "obs"))]
fn record_obs(_cfg: RecordConfig, _dir: &Path) -> Result<()> {
    anyhow::bail!(
        "il motore di Relay non funziona su questo PC e questo relay-capture non include OBS"
    )
}

#[cfg(feature = "obs")]
fn enter_obs(dir: &Path) -> Result<()> {
    let bin = relay_capture::install::spawn_dir(dir);
    std::fs::create_dir_all(&bin)?;
    std::env::set_current_dir(&bin)?;
    Ok(())
}

fn recorder_config(cfg: &RecordConfig) -> Config {
    Config {
        dir: cfg.dir.clone(),
        generation: playlist_generation(&cfg.playlist),
        origin_unix_secs: cfg.origin_unix_secs,
        target: match &cfg.source {
            Source::Window { exe } => Target::Window { exe: exe.clone() },
            Source::Monitor { index } => Target::Monitor { index: *index },
        },
        fallback_monitor: cfg.fallback_monitor,
        fallback_monitor_name: cfg.fallback_monitor_name.clone(),
        fps: cfg.fps,
        bitrate_kbps: cfg.bitrate_kbps,
        encoder: match cfg.encoder {
            EncoderChoice::Auto => EncoderPref::Auto,
            EncoderChoice::Nvenc => EncoderPref::Nvenc,
            EncoderChoice::Amf => EncoderPref::Amf,
            EncoderChoice::Qsv => EncoderPref::Qsv,
            EncoderChoice::X264 => EncoderPref::Software,
        },
        game_audio: cfg.game_audio_exe.clone(),
        mic_gain: cfg.mic.then_some(cfg.mic_gain),
        start_segment: cfg.start_segment,
        segment_secs: cfg.segment_secs,
        output_height: cfg.output_height,
    }
}

fn forward(e: relay_recorder::Event) {
    let ev = match e {
        relay_recorder::Event::Started {
            generation,
            first_frame_unix_secs,
            encoder,
            source,
        } => Event::Started {
            encoder,
            source,
            first_frame_unix_secs,
            generation,
            engine: ENGINE_RELAY.into(),
        },
        relay_recorder::Event::Resized {
            generation,
            width,
            height,
        } => Event::Resized {
            generation,
            width,
            height,
        },
        relay_recorder::Event::SourceChanged(source) => Event::SourceChanged { source },
        relay_recorder::Event::Warning(m) => Event::Warning(m),
    };
    emit(&ev);
}

enum Line {
    Quit,
    Fit,
    Other,
}

fn commands() -> mpsc::Receiver<Line> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            let mut line = String::new();
            let msg = match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) if line.trim() == "q" => Line::Quit,
                Ok(_) if line.trim() == "r" => Line::Fit,
                Ok(_) => Line::Other,
            };
            if tx.send(msg).is_err() {
                return;
            }
        }
    });
    rx
}

fn record_relay(cfg: &RecordConfig) -> std::result::Result<(), Failure> {
    let sink: EventSink = Arc::new(forward);
    let prepared = relay_recorder::prepare(recorder_config(cfg), sink)
        .map_err(|e| Failure::Fallback(format!("{e:#}")))?;
    match prepared.wait_frames(FIRST_FRAMES) {
        FrameCheck::Frames => {}
        FrameCheck::Waiting(why) => emit(&Event::Warning(why)),
        FrameCheck::Broken(why) => {
            prepared.abort();
            return Err(Failure::Fallback(why));
        }
    }
    let recorder = prepared.start();
    let lines = commands();
    loop {
        match lines.recv_timeout(Duration::from_millis(200)) {
            Ok(Line::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Line::Fit) => recorder.fit_window(),
            Ok(Line::Other) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(why) = recorder.failure() {
            let _ = recorder.stop();
            return Err(Failure::Broken(anyhow::anyhow!(why)));
        }
    }
    recorder.stop().map_err(Failure::Broken)?;
    emit(&Event::Stopped);
    Ok(())
}

use std::{
    io::BufRead,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use anyhow::{bail, Result};
use relay_capture::ipc::{
    self, playlist_generation, EncoderChoice, Event, MonitorInfo, RecordConfig, Source, WindowInfo,
    ENGINE_RELAY,
};
use relay_recorder::{Config, EncoderPref, EventSink, FrameCheck, Target};

const FIRST_FRAMES: Duration = Duration::from_secs(4);

pub fn emit(ev: &Event) {
    print!("{}", ipc::encode(ev));
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--encoders") {
        for line in relay_recorder::check_encoders() {
            println!("{line}");
        }
        std::process::exit(0);
    }
    let result = if args.iter().any(|a| a == "--probe") {
        probe()
    } else if let Some(i) = args.iter().position(|a| a == "--config") {
        let path = args.get(i + 1).expect("--config richiede un percorso");
        let text = std::fs::read_to_string(path).expect("configurazione non leggibile");
        let cfg: RecordConfig = serde_json::from_str(&text).expect("configurazione non valida");
        record(cfg)
    } else {
        eprintln!("uso: relay-capture --probe | --encoders | --config <file.json>");
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

fn probe() -> Result<()> {
    let p = relay_recorder::probe()?;
    emit(&Event::Ready {
        obs_version: String::new(),
        engine: ENGINE_RELAY.into(),
    });
    for r in &p.rejected {
        emit(&Event::Warning(format!("encoder scartato: {r}")));
    }
    emit(&Event::Probed {
        encoders: p.encoders.clone(),
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
        yellow_border: p.yellow_border,
    });
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

fn record(cfg: RecordConfig) -> Result<()> {
    let sink: EventSink = Arc::new(forward);
    let prepared = relay_recorder::prepare(recorder_config(&cfg), sink)?;
    match prepared.wait_frames(FIRST_FRAMES) {
        FrameCheck::Frames => {}
        FrameCheck::Waiting(why) => emit(&Event::Warning(why)),
        FrameCheck::Broken(why) => {
            prepared.abort();
            bail!(why);
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
            bail!(why);
        }
    }
    recorder.stop()?;
    emit(&Event::Stopped);
    Ok(())
}

use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{bail, Context, Result};
use relay_capture::ipc::{self, EncoderChoice, Event, RecordConfig, Source};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    task::JoinHandle,
};

use crate::{audio::AudioChoice, capture::WindowSel};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct ObsCaptureConfig {
    pub exe: PathBuf,
    pub cwd: PathBuf,
    pub window: WindowSel,
    pub fallback_monitor: Option<u32>,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub dir: PathBuf,
    pub encoder: EncoderChoice,
    pub origin_unix_secs: Option<f64>,
    pub start_segment: u64,
    pub generation: u32,
    pub audio: AudioChoice,
}

pub struct ObsCapture {
    child: Child,
    reader: JoinHandle<()>,
}

impl ObsCapture {
    pub fn spawn(cfg: &ObsCaptureConfig) -> Result<Self> {
        let source = match &cfg.window {
            WindowSel::Exe(exe) => Source::Window { exe: exe.clone() },
            WindowSel::Monitor(index) => Source::Monitor { index: *index },
            WindowSel::TitleRegex(_) => {
                bail!("relay-capture richiede il file eseguibile del gioco")
            }
        };
        let game_audio_exe = cfg.audio.game.then(|| match &cfg.window {
            WindowSel::Exe(exe) => exe.clone(),
            _ => String::new(),
        });
        let record = RecordConfig {
            dir: cfg.dir.clone(),
            origin_unix_secs: cfg.origin_unix_secs,
            playlist: crate::hls::playlist_name(cfg.generation),
            source,
            fallback_monitor: cfg.fallback_monitor,
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            encoder: cfg.encoder,
            game_audio_exe,
            mic: cfg.audio.mic,
            mic_gain: cfg.audio.mic_gain,
            start_segment: cfg.start_segment,
            segment_secs: relay_common::SEGMENT_SECONDS,
            output_height: 1080,
        };
        std::fs::create_dir_all(&cfg.dir)?;
        let config_path = cfg.dir.join(format!("capture_{}.json", cfg.generation));
        std::fs::write(&config_path, serde_json::to_vec(&record)?)?;

        let mut command = Command::new(&cfg.exe);
        command
            .arg("--config")
            .arg(&config_path)
            .current_dir(&cfg.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .with_context(|| format!("avvio di {}", cfg.exe.display()))?;
        let stdout = child.stdout.take().context("relay-capture senza uscita")?;
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match ipc::decode::<Event>(&line) {
                    Ok(Event::Started {
                        encoder, source, ..
                    }) => {
                        tracing::info!(
                            "relay-capture avviato: encoder {encoder}, sorgente {source}"
                        )
                    }
                    Ok(Event::SourceChanged { source }) => {
                        tracing::info!("relay-capture: sorgente {source}")
                    }
                    Ok(Event::Warning(message)) => tracing::warn!("relay-capture: {message}"),
                    Ok(Event::Error(message)) => tracing::error!("relay-capture: {message}"),
                    Ok(Event::Stopped) => tracing::info!("relay-capture fermato"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("uscita non valida da relay-capture: {e}"),
                }
            }
        });
        Ok(Self { child, reader })
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait().await?)
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(stdin) = &mut self.child.stdin {
            let _ = stdin.write_all(b"q\n").await;
            let _ = stdin.flush().await;
        }
        match tokio::time::timeout(Duration::from_secs(20), self.child.wait()).await {
            Ok(status) => {
                status?;
            }
            Err(_) => {
                self.child.kill().await?;
            }
        }
        self.reader.abort();
        Ok(())
    }

    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

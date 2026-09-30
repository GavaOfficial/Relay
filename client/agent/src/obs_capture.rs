use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use relay_capture::ipc::{self, EncoderChoice, Event, RecordConfig, Source};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
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
    pub fallback_monitor_name: Option<String>,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub dir: PathBuf,
    pub encoder: EncoderChoice,
    pub origin_unix_secs: Option<f64>,
    pub start_segment: u64,
    pub generation: u32,
    pub audio: AudioChoice,
    pub segment_secs: u32,
}

#[derive(Default)]
struct Shared {
    generations: Vec<(u32, f64)>,
    resized: u64,
    size: Option<(u32, u32)>,
}

pub struct ObsCapture {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: JoinHandle<()>,
    shared: Arc<Mutex<Shared>>,
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
            fallback_monitor_name: cfg.fallback_monitor_name.clone(),
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            encoder: cfg.encoder,
            game_audio_exe,
            mic: cfg.audio.mic,
            mic_gain: cfg.audio.mic_gain,
            start_segment: cfg.start_segment,
            segment_secs: cfg.segment_secs,
            output_height: 1080,
        };
        std::fs::create_dir_all(&cfg.dir)?;
        let _ = std::fs::create_dir_all(&cfg.cwd);
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
        let stdin = child.stdin.take();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let shared_w = shared.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim_start().starts_with('{') {
                    continue;
                }
                match ipc::decode::<Event>(&line) {
                    Ok(Event::Started {
                        encoder,
                        source,
                        first_frame_unix_secs,
                        generation,
                        engine,
                    }) => {
                        let mut sh = shared_w.lock().unwrap();
                        sh.generations.retain(|g| g.0 != generation);
                        sh.generations.push((generation, first_frame_unix_secs));
                        drop(sh);
                        if generation == 0 {
                            tracing::info!(
                                "relay-capture avviato: motore {engine}, encoder {encoder}, sorgente {source}"
                            )
                        }
                    }
                    Ok(Event::Resized {
                        generation,
                        width,
                        height,
                    }) => {
                        let mut sh = shared_w.lock().unwrap();
                        if sh.size.is_some_and(|s| s != (width, height)) {
                            tracing::info!("relay-capture: nuova dimensione {width}x{height} (playlist {generation})");
                        }
                        sh.size = Some((width, height));
                        sh.resized += 1;
                    }
                    Ok(Event::SourceChanged { source }) => {
                        tracing::info!("relay-capture: sorgente {source}")
                    }
                    Ok(Event::Warning(message)) => tracing::warn!("relay-capture: {message}"),
                    Ok(Event::Fallback { reason }) => {
                        tracing::warn!("relay-capture passa a OBS: {reason}")
                    }
                    Ok(Event::Error(message)) => tracing::error!("relay-capture: {message}"),
                    Ok(Event::Stopped) => tracing::info!("relay-capture fermato"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("uscita non valida da relay-capture: {e}"),
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            reader,
            shared,
        })
    }

    pub fn first_frame_unix_secs(&self) -> Option<f64> {
        self.current().map(|c| c.1)
    }

    pub fn current(&self) -> Option<(u32, f64)> {
        self.shared.lock().unwrap().generations.last().copied()
    }

    pub fn generations(&self) -> Vec<(u32, f64)> {
        self.shared.lock().unwrap().generations.clone()
    }

    pub async fn fit_window(&mut self) -> Option<(u32, f64)> {
        let before = self.shared.lock().unwrap().resized;
        let stdin = self.stdin.as_mut()?;
        stdin.write_all(b"r\n").await.ok()?;
        stdin.flush().await.ok()?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while tokio::time::Instant::now() < deadline {
            if self.shared.lock().unwrap().resized > before {
                return self.current();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.current()
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait().await?)
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(stdin) = &mut self.stdin {
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_for_the_process_does_not_close_its_input() {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("finto.cmd");
        std::fs::write(
            &script,
            "@echo off\r\nset /p line=\r\nif not defined line exit /b 7\r\nexit /b 0\r\n",
        )
        .unwrap();
        let mut cap = ObsCapture::spawn(&ObsCaptureConfig {
            exe: script,
            cwd: d.path().to_path_buf(),
            window: WindowSel::Exe("gioco.exe".into()),
            fallback_monitor: None,
            fallback_monitor_name: None,
            fps: 60,
            bitrate_kbps: 6000,
            dir: d.path().join("dati"),
            encoder: EncoderChoice::Auto,
            origin_unix_secs: None,
            start_segment: 0,
            generation: 0,
            audio: AudioChoice {
                game: false,
                mic: false,
                mic_gain: 1.0,
            },
            segment_secs: 4,
        })
        .unwrap();
        let waited = tokio::time::timeout(Duration::from_millis(1500), cap.wait()).await;
        assert!(
            waited.is_err(),
            "il programma si e' chiuso da solo: {waited:?}"
        );
        cap.stop().await.unwrap();
    }
}

use std::path::{Path, PathBuf};

use relay_capture::install::{self, Canceller, InstallProgress};
use serde::Serialize;

use crate::updater::{self, Release};

pub const KIND_CAPTURE: &str = "relay-capture";
pub const MAX_CAPTURE_BYTES: u64 = 80 * 1024 * 1024;
const API_LATEST: &str = "/api/app/capture/latest";
const API_DOWNLOAD: &str = "/api/app/capture/download";

fn runtime_dir(base: &Path) -> PathBuf {
    install::runtime_dir(base)
}

fn exe_path(base: &Path) -> PathBuf {
    install::host_exe(&runtime_dir(base))
}

fn exe_version_file(base: &Path) -> PathBuf {
    runtime_dir(base).join("relay-capture-version.txt")
}

pub fn obs_ready(base: &Path) -> bool {
    install::is_ready(&runtime_dir(base))
}

pub fn exe_ready(base: &Path) -> bool {
    exe_path(base).exists() && exe_version_file(base).exists()
}

pub fn ready(base: &Path) -> bool {
    exe_ready(base)
}

pub fn exe_installed_version(base: &Path) -> Option<String> {
    std::fs::read_to_string(exe_version_file(base))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn fetch_latest(http: &reqwest::Client, server: &str) -> Result<Option<Release>, String> {
    updater::fetch_latest(http, server, API_LATEST).await
}

pub async fn install_exe(
    http: &reqwest::Client,
    server: &str,
    rel: &Release,
    base: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let dir = runtime_dir(base);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = exe_path(base);
    let _ = std::fs::remove_file(exe_version_file(base));
    updater::download(
        http,
        server,
        API_DOWNLOAD,
        KIND_CAPTURE,
        rel,
        MAX_CAPTURE_BYTES,
        &target,
        progress,
    )
    .await?;
    std::fs::write(exe_version_file(base), &rel.version).map_err(|e| e.to_string())
}

pub fn install_obs(
    base: &Path,
    cancel: &Canceller,
    progress: &mut dyn FnMut(&InstallProgress),
) -> Result<(), String> {
    install::install(&runtime_dir(base), cancel, progress).map_err(|e| format!("{e:#}"))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Compat {
    pub ok: bool,
    pub obs_version: Option<String>,
    pub engine: Option<String>,
    pub fallback: Option<String>,
    pub yellow_border: bool,

    pub best_encoder: Option<String>,
    pub hardware_encoder: bool,
    pub monitors: usize,
    pub error: Option<String>,
}

pub async fn check_compat(base: &Path) -> Compat {
    let exe = exe_path(base);
    let cwd = install::spawn_dir(&runtime_dir(base));
    let _ = std::fs::create_dir_all(&cwd);
    let run = tokio::task::spawn_blocking(move || {
        use std::process::{Command, Stdio};
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = Command::new(&exe);
        c.arg("--probe")
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = c.spawn().map_err(|e| {
            (
                None,
                format!("non riesco ad avviare relay-capture.exe: {e}"),
            )
        })?;
        let pid = child.id();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let _watchdog = std::thread::spawn(move || {
            if done_rx.recv_timeout(std::time::Duration::from_secs(40))
                != Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            {
                return;
            }
            let mut k = Command::new("taskkill");
            k.args(["/F", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                k.creation_flags(CREATE_NO_WINDOW);
            }
            let _ = k.status();
        });
        let stdout = child
            .stdout
            .take()
            .ok_or((None, "relay-capture.exe senza uscita".to_string()))?;
        let mut stderr_pipe = child.stderr.take();
        let out = std::io::Read::bytes(stdout)
            .filter_map(|b| b.ok())
            .collect::<Vec<u8>>();
        let mut err_out = Vec::new();
        if let Some(e) = &mut stderr_pipe {
            let _ = std::io::Read::read_to_end(e, &mut err_out);
        }
        let status = child.wait().map_err(|e| (None, e.to_string()))?;
        drop(done_tx);
        if !status.success() {
            let text = String::from_utf8_lossy(&out);
            let fallback = text
                .lines()
                .filter_map(|l| relay_capture::ipc::decode::<relay_capture::ipc::Event>(l).ok())
                .find_map(|e| match e {
                    relay_capture::ipc::Event::Fallback { reason } => Some(reason),
                    _ => None,
                });
            let msg = text
                .lines()
                .filter_map(|l| relay_capture::ipc::decode::<relay_capture::ipc::Event>(l).ok())
                .find_map(|e| match e {
                    relay_capture::ipc::Event::Error(m) => Some(m),
                    _ => None,
                });
            let tail_out: String = text
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            let tail_err: String = String::from_utf8_lossy(&err_out)
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err((
                fallback,
                msg.unwrap_or_else(|| {
                    format!(
                        "relay-capture.exe si e' fermato ({status}); uscita: {}; errori: {}",
                        if tail_out.is_empty() {
                            "(vuota)"
                        } else {
                            &tail_out
                        },
                        if tail_err.is_empty() {
                            "(vuota)"
                        } else {
                            &tail_err
                        }
                    )
                }),
            ));
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    })
    .await;

    let failed = |fallback: Option<String>, error: String| Compat {
        ok: false,
        obs_version: None,
        engine: None,
        fallback,
        yellow_border: false,
        best_encoder: None,
        hardware_encoder: false,
        monitors: 0,
        error: Some(error),
    };
    let text = match run {
        Ok(Ok(t)) => t,
        Ok(Err((fallback, e))) => return failed(fallback, e),
        Err(e) => return failed(None, e.to_string()),
    };
    read_probe(&text)
}

fn read_probe(text: &str) -> Compat {
    use relay_capture::ipc::{encoder_name, Event};
    let mut obs_version = None;
    let mut engine = None;
    let mut fallback = None;
    let mut encoders: Vec<String> = Vec::new();
    let mut best = None;
    let mut yellow_border = false;
    let mut monitors = 0usize;
    for line in text.lines() {
        match relay_capture::ipc::decode::<Event>(line) {
            Ok(Event::Ready {
                obs_version: v,
                engine: e,
            }) => {
                obs_version = Some(v).filter(|v| !v.is_empty());
                engine = Some(e);
            }
            Ok(Event::Probed {
                encoders: e,
                monitors: m,
                best: b,
                yellow_border: y,
                ..
            }) => {
                encoders = e;
                monitors = m.len();
                best = b;
                yellow_border = y;
            }
            Ok(Event::Fallback { reason }) => fallback = Some(reason),
            _ => {}
        }
    }
    let pick = best
        .as_deref()
        .and_then(encoder_name)
        .or_else(|| {
            ["nvenc", "amf", "qsv", "software", "x264"]
                .into_iter()
                .find(|n| encoders.iter().any(|e| encoder_name(e) == Some(*n)))
        })
        .map(String::from);
    let hardware = pick
        .as_deref()
        .is_some_and(|n| n != "x264" && n != "software");
    Compat {
        ok: pick.is_some(),
        obs_version,
        engine,
        fallback,
        yellow_border,
        error: pick
            .is_none()
            .then(|| "nessun encoder video funzionante su questo PC".into()),
        best_encoder: pick,
        hardware_encoder: hardware,
        monitors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_engine_reports_its_best_encoder() {
        let text = concat!(
            r#"{"Ready":{"obs_version":"","engine":"relay"}}"#,
            "\n",
            r#"{"Probed":{"encoders":["nvenc","software","obs_nvenc_h264_tex","obs_x264"],"monitors":[{"index":0,"width":1920,"height":1080}],"windows":[],"best":"nvenc","yellow_border":true}}"#,
            "\n"
        );
        let c = read_probe(text);
        assert!(c.ok);
        assert_eq!(c.engine.as_deref(), Some("relay"));
        assert_eq!(c.best_encoder.as_deref(), Some("nvenc"));
        assert!(c.hardware_encoder);
        assert!(c.yellow_border);
        assert_eq!(c.monitors, 1);
        assert_eq!(c.obs_version, None);
    }

    #[test]
    fn the_obs_engine_is_still_understood() {
        let text = concat!(
            r#"{"Warning":"su questo PC registro con OBS"}"#,
            "\n",
            r#"{"Ready":{"obs_version":"32.2.2","engine":"obs"}}"#,
            "\n",
            r#"{"Probed":{"encoders":["obs_x264"],"monitors":[],"windows":[]}}"#,
            "\n"
        );
        let c = read_probe(text);
        assert!(c.ok);
        assert_eq!(c.engine.as_deref(), Some("obs"));
        assert_eq!(c.best_encoder.as_deref(), Some("x264"));
        assert!(!c.hardware_encoder);
        assert_eq!(c.obs_version.as_deref(), Some("32.2.2"));
    }

    #[test]
    fn no_encoder_means_not_compatible() {
        let c = read_probe(r#"{"Probed":{"encoders":[],"monitors":[],"windows":[]}}"#);
        assert!(!c.ok);
        assert!(c.error.is_some());
    }
}

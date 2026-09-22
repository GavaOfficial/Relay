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
    obs_ready(base) && exe_ready(base)
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

    pub best_encoder: Option<String>,
    pub hardware_encoder: bool,
    pub monitors: usize,
    pub error: Option<String>,
}

pub async fn check_compat(base: &Path) -> Compat {
    let exe = exe_path(base);
    let cwd = install::spawn_dir(&runtime_dir(base));
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
        let mut child = c
            .spawn()
            .map_err(|e| format!("non riesco ad avviare relay-capture.exe: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or("relay-capture.exe senza uscita")?;
        let mut stderr_pipe = child.stderr.take();
        let out = std::io::Read::bytes(stdout)
            .filter_map(|b| b.ok())
            .collect::<Vec<u8>>();
        let mut err_out = Vec::new();
        if let Some(e) = &mut stderr_pipe {
            let _ = std::io::Read::read_to_end(e, &mut err_out);
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            let text = String::from_utf8_lossy(&out);
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
            return Err(msg.unwrap_or_else(|| {
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
            }));
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    })
    .await;

    let text = match run {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            return Compat {
                ok: false,
                obs_version: None,
                best_encoder: None,
                hardware_encoder: false,
                monitors: 0,
                error: Some(e),
            }
        }
        Err(e) => {
            return Compat {
                ok: false,
                obs_version: None,
                best_encoder: None,
                hardware_encoder: false,
                monitors: 0,
                error: Some(e.to_string()),
            }
        }
    };

    let mut obs_version = None;
    let mut encoders: Vec<String> = Vec::new();
    let mut monitors = 0usize;
    for line in text.lines() {
        match relay_capture::ipc::decode::<relay_capture::ipc::Event>(line) {
            Ok(relay_capture::ipc::Event::Ready { obs_version: v }) => obs_version = Some(v),
            Ok(relay_capture::ipc::Event::Probed {
                encoders: e,
                monitors: m,
                ..
            }) => {
                encoders = e;
                monitors = m.len();
            }
            _ => {}
        }
    }
    let pick = [
        ("obs_nvenc_h264_tex", "nvenc"),
        ("h264_texture_amf", "amf"),
        ("obs_qsv11", "qsv"),
        ("obs_x264", "x264"),
    ]
    .into_iter()
    .find(|(id, _)| encoders.iter().any(|e| e == id))
    .map(|(_, name)| name.to_string());
    let hardware = pick.as_deref().is_some_and(|n| n != "x264");
    if pick.is_none() {
        return Compat {
            ok: false,
            obs_version,
            best_encoder: None,
            hardware_encoder: false,
            monitors,
            error: Some("nessun encoder video funzionante su questo PC".into()),
        };
    }
    Compat {
        ok: true,
        obs_version,
        best_encoder: pick,
        hardware_encoder: hardware,
        monitors,
        error: None,
    }
}

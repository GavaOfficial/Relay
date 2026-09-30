use std::path::{Path, PathBuf};

use relay_capture::install::{self};
use serde::Serialize;

use crate::updater::{self, Release};

pub const KIND_RECORDER: &str = "relay-recorder";
pub const MAX_PACKAGE_BYTES: u64 = 120 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
const API_LATEST: &str = "/api/app/recorder/latest";
const API_DOWNLOAD: &str = "/api/app/recorder/download";
const VERSION_FILE: &str = "relay-recorder-version.txt";
const ALLOWED_EXTENSIONS: [&str; 4] = ["exe", "dll", "json", "txt"];
pub const REQUIRED_FILES: [&str; 5] = [
    "relay-capture.exe",
    "hooks/x64/relay_hook.dll",
    "hooks/x64/relay-inject.exe",
    "hooks/x64/relay_vk_layer.dll",
    "hooks/x64/relay-vk-layer.json",
];

fn runtime_dir(base: &Path) -> PathBuf {
    install::runtime_dir(base)
}

fn exe_path(base: &Path) -> PathBuf {
    install::host_exe(&runtime_dir(base))
}

fn exe_version_file(base: &Path) -> PathBuf {
    runtime_dir(base).join(VERSION_FILE)
}

pub fn exe_ready(base: &Path) -> bool {
    let dir = runtime_dir(base);
    exe_version_file(base).is_file() && REQUIRED_FILES.iter().all(|f| dir.join(f).is_file())
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

pub async fn install_package(
    http: &reqwest::Client,
    server: &str,
    rel: &Release,
    base: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let archive = base.join("relay-recorder.zip");
    updater::download(
        http,
        server,
        API_DOWNLOAD,
        KIND_RECORDER,
        rel,
        MAX_PACKAGE_BYTES,
        &archive,
        progress,
    )
    .await?;
    let (base, version) = (base.to_path_buf(), rel.version.clone());
    let zip = archive.clone();
    let result = tokio::task::spawn_blocking(move || swap_in(&zip, &base, &version))
        .await
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&archive);
    result
}

fn swap_in(archive: &Path, base: &Path, version: &str) -> Result<(), String> {
    let staging = base.join("relay-capture.new");
    extract_package(archive, &staging).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
    })?;
    let dir = runtime_dir(base);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            format!("non riesco a sostituire il programma di registrazione (e' in uso?): {e}")
        })?;
    }
    std::fs::rename(&staging, &dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(VERSION_FILE), version).map_err(|e| e.to_string())?;
    remove_obs_leftovers(base);
    Ok(())
}

pub fn remove_all(base: &Path) -> Result<(), String> {
    crate::vulkan::unregister()?;
    let dir = runtime_dir(base);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    remove_obs_leftovers(base);
    Ok(())
}

pub fn remove_obs_leftovers(base: &Path) {
    let obs = base.join("obs");
    if obs.is_dir() {
        match std::fs::remove_dir_all(&obs) {
            Ok(()) => tracing::info!("cancellata la vecchia cartella di OBS"),
            Err(e) => tracing::warn!("non riesco a cancellare {}: {e}", obs.display()),
        }
    }
}

pub fn extract_package(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("pacchetto non valido: {e}"))?;
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| format!("percorso non valido nel pacchetto: {}", entry.name()))?;
        let allowed = rel
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ALLOWED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
        if !allowed {
            return Err(format!(
                "file non previsto nel pacchetto: {}",
                rel.display()
            ));
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut target = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        let copied = std::io::copy(
            &mut std::io::Read::take(&mut entry, MAX_FILE_BYTES + 1),
            &mut target,
        )
        .map_err(|e| e.to_string())?;
        if copied > MAX_FILE_BYTES {
            return Err(format!(
                "file troppo grande nel pacchetto: {}",
                rel.display()
            ));
        }
    }
    for required in REQUIRED_FILES {
        if !dest.join(required).is_file() {
            return Err(format!("nel pacchetto manca {required}"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Compat {
    pub ok: bool,
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
        let mut child = c
            .spawn()
            .map_err(|e| format!("non riesco ad avviare relay-capture.exe: {e}"))?;
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
            .ok_or_else(|| "relay-capture.exe senza uscita".to_string())?;
        let mut stderr_pipe = child.stderr.take();
        let out = std::io::Read::bytes(stdout)
            .filter_map(|b| b.ok())
            .collect::<Vec<u8>>();
        let mut err_out = Vec::new();
        if let Some(e) = &mut stderr_pipe {
            let _ = std::io::Read::read_to_end(e, &mut err_out);
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        drop(done_tx);
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

    let failed = |error: String| Compat {
        ok: false,
        yellow_border: false,
        best_encoder: None,
        hardware_encoder: false,
        monitors: 0,
        error: Some(error),
    };
    let text = match run {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return failed(e),
        Err(e) => return failed(e.to_string()),
    };
    read_probe(&text)
}

fn read_probe(text: &str) -> Compat {
    use relay_capture::ipc::{encoder_name, Event};
    let mut encoders: Vec<String> = Vec::new();
    let mut best = None;
    let mut yellow_border = false;
    let mut monitors = 0usize;
    for line in text.lines() {
        if let Ok(Event::Probed {
            encoders: e,
            monitors: m,
            best: b,
            yellow_border: y,
            ..
        }) = relay_capture::ipc::decode::<Event>(line)
        {
            encoders = e;
            monitors = m.len();
            best = b;
            yellow_border = y;
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
    fn the_best_encoder_is_read_from_the_probe() {
        let text = concat!(
            r#"{"Ready":{"obs_version":"","engine":"relay"}}"#,
            "\n",
            r#"{"Probed":{"encoders":["nvenc","software"],"monitors":[{"index":0,"width":1920,"height":1080}],"windows":[],"best":"nvenc","yellow_border":true}}"#,
            "\n"
        );
        let c = read_probe(text);
        assert!(c.ok);
        assert_eq!(c.best_encoder.as_deref(), Some("nvenc"));
        assert!(c.hardware_encoder);
        assert!(c.yellow_border);
        assert_eq!(c.monitors, 1);
    }

    #[test]
    fn software_only_is_compatible_but_not_hardware() {
        let text =
            r#"{"Probed":{"encoders":["software"],"monitors":[],"windows":[],"best":"software"}}"#;
        let c = read_probe(text);
        assert!(c.ok);
        assert_eq!(c.best_encoder.as_deref(), Some("software"));
        assert!(!c.hardware_encoder);
    }

    #[test]
    fn no_encoder_means_not_compatible() {
        let c = read_probe(r#"{"Probed":{"encoders":[],"monitors":[],"windows":[]}}"#);
        assert!(!c.ok);
        assert!(c.error.is_some());
    }

    fn package(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        use std::io::Write;
        let path = dir.join(name);
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (n, data) in files {
            z.start_file(*n, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
        path
    }

    fn full() -> Vec<(&'static str, &'static [u8])> {
        REQUIRED_FILES
            .iter()
            .map(|f| (*f, b"x".as_slice()))
            .collect()
    }

    #[test]
    fn the_package_is_unpacked_with_its_hooks() {
        let d = tempfile::tempdir().unwrap();
        let mut files = full();
        files.push(("hooks/x86/relay_hook.dll", b"y"));
        let zip = package(d.path(), "ok.zip", &files);
        let dest = d.path().join("out");
        extract_package(&zip, &dest).unwrap();
        assert!(dest.join("relay-capture.exe").is_file());
        assert!(dest.join("hooks/x86/relay_hook.dll").is_file());
    }

    #[test]
    fn a_package_missing_a_hook_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let files: Vec<_> = full()
            .into_iter()
            .filter(|f| !f.0.ends_with("relay-inject.exe"))
            .collect();
        let zip = package(d.path(), "manca.zip", &files);
        let e = extract_package(&zip, &d.path().join("out")).unwrap_err();
        assert!(e.contains("relay-inject.exe"), "{e}");
    }

    #[test]
    fn paths_outside_the_folder_and_odd_files_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let mut files = full();
        files.push(("../fuori.dll", b"z"));
        let zip = package(d.path(), "slip.zip", &files);
        assert!(extract_package(&zip, &d.path().join("a")).is_err());
        assert!(!d.path().join("fuori.dll").exists());
        let mut files = full();
        files.push(("script.bat", b"z"));
        let zip = package(d.path(), "bat.zip", &files);
        assert!(extract_package(&zip, &d.path().join("b"))
            .unwrap_err()
            .contains("script.bat"));
        assert!(extract_package(
            d.path().join("non-esiste.zip").as_path(),
            &d.path().join("c")
        )
        .is_err());
    }

    #[test]
    fn the_new_folder_replaces_the_old_one_and_old_obs_goes_away() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path();
        let old = runtime_dir(base);
        std::fs::create_dir_all(old.join("bin/64bit")).unwrap();
        std::fs::write(old.join("bin/64bit/obs.dll"), b"vecchio").unwrap();
        std::fs::create_dir_all(base.join("obs/bin")).unwrap();
        std::fs::write(base.join("obs/bin/obs.dll"), b"vecchio").unwrap();
        let zip = package(base, "relay-recorder.zip", &full());
        swap_in(&zip, base, "0.4.0").unwrap();
        assert!(exe_ready(base));
        assert_eq!(exe_installed_version(base).as_deref(), Some("0.4.0"));
        assert!(!old.join("bin").exists());
        assert!(!base.join("obs").exists());
        assert!(!base.join("relay-capture.new").exists());
    }

    #[test]
    fn a_broken_package_leaves_the_working_install_alone() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path();
        let zip = package(base, "uno.zip", &full());
        swap_in(&zip, base, "0.4.0").unwrap();
        let bad = package(base, "due.zip", &[("relay-capture.exe", b"x")]);
        assert!(swap_in(&bad, base, "0.4.1").is_err());
        assert_eq!(exe_installed_version(base).as_deref(), Some("0.4.0"));
        assert!(exe_ready(base));
        assert!(!base.join("relay-capture.new").exists());
    }
}

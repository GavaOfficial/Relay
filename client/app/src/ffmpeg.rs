use std::path::{Path, PathBuf};

use crate::updater::{self, Release, KIND_FFMPEG, MAX_FFMPEG_BYTES};

const API_LATEST: &str = "/api/app/ffmpeg/latest";
const API_DOWNLOAD: &str = "/api/app/ffmpeg/download";

pub fn dir(base: &Path) -> PathBuf {
    base.join("ffmpeg")
}

pub fn exe(base: &Path) -> PathBuf {
    dir(base).join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    })
}

fn version_file(base: &Path) -> PathBuf {
    dir(base).join("version.txt")
}

pub fn installed_version(base: &Path) -> Option<String> {
    if !exe(base).exists() {
        return None;
    }
    std::fs::read_to_string(version_file(base))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn fetch_latest(http: &reqwest::Client, server: &str) -> Result<Option<Release>, String> {
    updater::fetch_latest(http, server, API_LATEST).await
}

pub async fn install(
    http: &reqwest::Client,
    server: &str,
    rel: &Release,
    base: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let d = dir(base);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let zip_path = d.join("download.zip");
    let _ = std::fs::remove_file(version_file(base));
    updater::download(
        http,
        server,
        API_DOWNLOAD,
        KIND_FFMPEG,
        rel,
        MAX_FFMPEG_BYTES,
        &zip_path,
        progress,
    )
    .await?;

    let target = exe(base);
    let part = target.with_extension("part");
    let z = zip_path.clone();
    let (p2, t2) = (part.clone(), target.clone());

    tokio::task::spawn_blocking(move || extract_exe(&z, &p2, &t2))
        .await
        .map_err(|e| e.to_string())??;
    let _ = std::fs::remove_file(&zip_path);
    std::fs::write(version_file(base), &rel.version).map_err(|e| e.to_string())
}

fn extract_exe(zip_path: &Path, part: &Path, target: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("pacchetto non valido: {e}"))?;
    let want = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("ffmpeg.exe")
        .to_ascii_lowercase();
    let idx = (0..z.len())
        .find(|&i| {
            z.by_index(i)
                .map(|e| {
                    !e.is_dir()
                        && e.name()
                            .rsplit(['/', '\\'])
                            .next()
                            .map(|n| n.to_ascii_lowercase())
                            == Some(want.clone())
                })
                .unwrap_or(false)
        })
        .ok_or("nel pacchetto non c'e' ffmpeg")?;
    {
        let mut entry = z.by_index(idx).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(part).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(target);
    std::fs::rename(part, target).map_err(|e| format!("non riesco a installare ffmpeg: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn finds_ffmpeg_exe_in_any_folder_of_the_zip() {
        let d = std::env::temp_dir().join(format!("relay-ff-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let zp = d.join("a.zip");
        make_zip(
            &zp,
            &[
                ("ffmpeg-9.0-full/README.txt", b"x"),
                ("ffmpeg-9.0-full/bin/ffmpeg.exe", b"MZ-ffmpeg"),
                ("ffmpeg-9.0-full/bin/ffprobe.exe", b"no"),
            ],
        );
        let target = d.join("ffmpeg.exe");
        extract_exe(&zp, &d.join("ffmpeg.part"), &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MZ-ffmpeg");

        let zp2 = d.join("b.zip");
        make_zip(&zp2, &[("qualcosa.txt", b"x")]);
        assert!(extract_exe(&zp2, &d.join("p2"), &d.join("t2.exe")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn version_is_known_only_when_both_files_exist() {
        let d = std::env::temp_dir().join(format!("relay-ffv-{}", std::process::id()));
        std::fs::create_dir_all(dir(&d)).unwrap();
        assert_eq!(installed_version(&d), None);
        std::fs::write(exe(&d), b"x").unwrap();
        assert_eq!(installed_version(&d), None);
        std::fs::write(version_file(&d), "9.0.1\n").unwrap();
        assert_eq!(installed_version(&d).as_deref(), Some("9.0.1"));
        let _ = std::fs::remove_dir_all(&d);
    }
}

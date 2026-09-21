use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use tokio::process::Command;
use uuid::Uuid;

use crate::state::AppState;

pub const VIDEO_FILE: &str = "video.mp4";

pub const WEB_FILE: &str = "web.mp4";

const WEB_HEIGHT: u32 = 720;

pub const THUMB_FILE: &str = "thumb.jpg";

pub const INFO_FILE: &str = "info.json";
const TIMEOUT: Duration = Duration::from_secs(3 * 3600);

pub async fn has_video(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join(VIDEO_FILE))
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

pub const VOD_DIR: &str = "hls";
pub const VOD_INDEX: &str = "index.m3u8";
pub const VOD_MEDIA: &str = "media.mp4";

pub async fn has_vod(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join(VOD_DIR).join(VOD_INDEX))
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

async fn make_vod(ffmpeg: &Path, src: &Path, dir: &Path) -> Result<(), String> {
    let tmp = dir.join(".hls.tmp");
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp)
        .await
        .map_err(|e| e.to_string())?;
    let out = tokio::time::timeout(
        TIMEOUT,
        Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
            .arg(src)
            .args([
                "-c",
                "copy",
                "-f",
                "hls",
                "-hls_time",
                "4",
                "-hls_playlist_type",
                "vod",
            ])
            .args([
                "-hls_segment_type",
                "fmp4",
                "-hls_flags",
                "single_file+independent_segments",
            ])
            .arg("-hls_segment_filename")
            .arg(tmp.join(VOD_MEDIA))
            .arg(tmp.join(VOD_INDEX))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "ffmpeg ha impiegato troppo".to_string())?
    .map_err(|e| format!("ffmpeg non parte: {e}"))?;
    if !out.status.success() {
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        return Err(format!(
            "ffmpeg {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("errore")
        ));
    }
    let dst = dir.join(VOD_DIR);
    let _ = tokio::fs::remove_dir_all(&dst).await;
    tokio::fs::rename(&tmp, &dst)
        .await
        .map_err(|e| e.to_string())
}

async fn ensure_vod(ffmpeg: &Path, dir: &Path) {
    if has_vod(dir).await || !has_web(dir).await {
        return;
    }
    if let Err(e) = make_vod(ffmpeg, &dir.join(WEB_FILE), dir).await {
        tracing::warn!("versione per lo streaming non creata ({e})");
    }
}

pub async fn has_web(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join(WEB_FILE))
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

async fn make_web(ffmpeg: &Path, dir: &Path, report: &Report) -> Result<bool, String> {
    if has_web(dir).await {
        return Ok(false);
    }
    let video = dir.join(VIDEO_FILE);
    let Some(p) = probe(ffmpeg, &video).await else {
        return Ok(false);
    };
    if p.size.1 <= WEB_HEIGHT {
        return Ok(false);
    }
    report("web", 0);
    let tmp = dir.join(".web.tmp.mp4");
    let _ = tokio::fs::remove_file(&tmp).await;
    let mut c = Command::new(ffmpeg);
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-progress",
        "pipe:1",
        "-nostats",
        "-y",
        "-threads",
        "2",
        "-i",
    ])
    .arg(&video)
    .args(["-vf", &format!("scale=-2:{WEB_HEIGHT}")])
    .args([
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "26", "-maxrate", "3000k", "-bufsize",
        "6000k", "-pix_fmt", "yuv420p",
    ])
    .args(["-force_key_frames", "expr:gte(t,n_forced*4)"])
    .args(["-c:a", "copy", "-movflags", "+faststart", "-f", "mp4"])
    .arg(&tmp);
    let rep = report.clone();
    let (status, err) = run_progress(c, p.duration_secs.unwrap_or(0.0), move |f| {
        rep("web", (f * 99.0) as u8)
    })
    .await?;
    if !status.success() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!(
            "ffmpeg {}: {}",
            status,
            err.lines().last().unwrap_or("errore")
        ));
    }

    if let Err(e) = make_vod(ffmpeg, &tmp, dir).await {
        tracing::warn!("versione per lo streaming non creata ({e}); si usa il file MP4");
    }
    tokio::fs::rename(&tmp, dir.join(WEB_FILE))
        .await
        .map_err(|e| e.to_string())?;
    Ok(true)
}

pub async fn read_duration(dir: &Path) -> Option<u64> {
    let v: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(dir.join(INFO_FILE)).await.ok()?).ok()?;
    v["duration_secs"].as_u64()
}

async fn write_extras(ffmpeg: &Path, dir: &Path) {
    let video = dir.join(VIDEO_FILE);
    let Some(p) = probe(ffmpeg, &video).await else {
        return;
    };
    let dur = p.duration_secs.unwrap_or(0.0);
    if dur > 0.0 && !dir.join(INFO_FILE).exists() {
        let _ = tokio::fs::write(
            dir.join(INFO_FILE),
            format!("{{\"duration_secs\": {}}}", dur.round() as u64),
        )
        .await;
    }
    if !dir.join(THUMB_FILE).exists() {
        let at = (dur * 0.25).min(90.0);
        let tmp = dir.join(".thumb.tmp.jpg");
        let ok = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-y",
                "-ss",
            ])
            .arg(format!("{at:.1}"))
            .arg("-i")
            .arg(&video)
            .args(["-frames:v", "1", "-vf", "scale=640:-2", "-q:v", "4"])
            .arg(&tmp)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let _ = tokio::fs::rename(&tmp, dir.join(THUMB_FILE)).await;
        } else {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }
}

type Report = Arc<dyn Fn(&'static str, u8) + Send + Sync>;

async fn run_progress(
    mut c: Command,
    total: f64,
    on_frac: impl Fn(f64) + Send + 'static,
) -> Result<(std::process::ExitStatus, String), String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = c.spawn().map_err(|e| format!("ffmpeg non parte: {e}"))?;
    let stdout = child.stdout.take().ok_or("ffmpeg senza uscita")?;
    let mut stderr = child.stderr.take().ok_or("ffmpeg senza errori")?;
    let reader = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if let Some(v) = l.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<f64>() {
                    if total > 0.0 {
                        on_frac((us / 1e6 / total).clamp(0.0, 1.0));
                    }
                }
            }
        }
    });
    let err = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });
    let status = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .map_err(|_| "ffmpeg ha impiegato troppo".to_string())?
        .map_err(|e| format!("ffmpeg: {e}"))?;
    let _ = reader.await;
    Ok((status, err.await.unwrap_or_default()))
}

pub async fn run(st: Arc<AppState>, id: Uuid) {
    let Some(ffmpeg) = st.ffmpeg.clone() else {
        return;
    };
    let players = match st.matches.read().await.get(&id) {
        Some(m) => m.players.clone(),
        None => return,
    };

    let mut queued = Vec::new();
    for p in &players {
        let dir = st.player_dir(id, p);
        if !has_video(&dir).await && segments(&dir).await.is_ok_and(|s| !s.is_empty()) {
            st.set_processing(id, p, "queue", 0);
            queued.push(p.clone());
        }
    }
    let _one_at_a_time = st.finalize_lock.lock().await;
    let reporter = |p: &str| -> Report {
        let (st, p) = (st.clone(), p.to_string());
        Arc::new(move |stage, pct| st.set_processing(id, &p, stage, pct))
    };
    let mut fresh = Vec::new();
    for p in players {
        let dir = st.player_dir(id, &p);
        match player(&ffmpeg, &dir, &reporter(&p)).await {
            Ok(true) => {
                tracing::info!("partita {id}: video di {p} pronto, segmenti eliminati");
                fresh.push((p, dir));
            }
            Ok(false) => st.clear_processing(id, &p),
            Err(e) => {
                st.clear_processing(id, &p);
                tracing::warn!("partita {id}: video di {p} non creato ({e}); i segmenti restano");
            }
        }
    }

    for (p, dir) in fresh {
        match make_web(&ffmpeg, &dir, &reporter(&p)).await {
            Ok(true) => tracing::info!("partita {id}: versione leggera di {p} pronta"),
            Ok(false) => {}
            Err(e) => tracing::warn!("partita {id}: versione leggera di {p} non creata ({e})"),
        }
        st.clear_processing(id, &p);
    }
    for p in queued {
        st.clear_processing(id, &p);
    }

    let players = match st.matches.read().await.get(&id) {
        Some(m) => m.players.clone(),
        None => return,
    };
    for p in players {
        ensure_vod(&ffmpeg, &st.player_dir(id, &p)).await;
    }
}

pub async fn resume_all(st: Arc<AppState>) {
    let Some(ff) = st.ffmpeg.clone() else { return };
    match Command::new(&ff)
        .arg("-version")
        .stdin(Stdio::null())
        .output()
        .await
    {
        Ok(o) if o.status.success() => {
            tracing::info!(
                "ffmpeg: {}",
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("?")
            );
        }
        Ok(o) => tracing::error!("ffmpeg ({}) non funziona: {}", ff.display(), o.status),
        Err(e) => tracing::error!("ffmpeg ({}) non parte: {e}", ff.display()),
    }
    let ended: Vec<Uuid> = st
        .matches
        .read()
        .await
        .values()
        .filter(|m| m.status == relay_common::MatchStatus::Ended)
        .map(|m| m.id)
        .collect();
    for id in ended {
        run(st.clone(), id).await;
    }
}

async fn segments(dir: &Path) -> std::io::Result<Vec<(u64, PathBuf)>> {
    let mut v = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(v),
        Err(e) => return Err(e),
    };
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') {
            if let Some(n) = crate::playlist::parse_segment(&name).filter(|_| name.ends_with(".ts"))
            {
                v.push((n, e.path()));
            }
        }
    }
    v.sort();
    Ok(v)
}

async fn player(ffmpeg: &Path, dir: &Path, report: &Report) -> Result<bool, String> {
    if has_video(dir).await {
        remove_segments(dir).await;
        write_extras(ffmpeg, dir).await;
        return Ok(false);
    }
    let segs = segments(dir).await.map_err(|e| e.to_string())?;
    if segs.is_empty() {
        return Ok(false);
    }

    report("video", 1);
    let total = expected_secs(dir, &segs)
        .await
        .unwrap_or(segs.len() as f64 * relay_common::SEGMENT_SECONDS as f64);

    let mut sizes = Vec::with_capacity(segs.len());
    for (_, path) in &segs {
        sizes.push(probe(ffmpeg, path).await.map(|p| p.size));
    }
    let known: Vec<(u32, u32)> = sizes.iter().flatten().copied().collect();
    let same = known.windows(2).all(|w| w[0] == w[1]);
    let target = known
        .iter()
        .fold((0, 0), |a, s| (a.0.max(s.0), a.1.max(s.1)));

    let all = dir.join(".all.ts");
    join_segments(&segs, &all)
        .await
        .map_err(|e| format!("unione dei segmenti: {e}"))?;
    report("video", 5);

    let has_audio = match segs.first() {
        Some((_, first)) => probe(ffmpeg, first).await.is_some_and(|p| p.has_audio),
        None => false,
    };

    let loud = if has_audio {
        let rep = report.clone();
        measure_loudness(ffmpeg, dir, &all, total, move |f| {
            rep("video", 5 + (f * 25.0) as u8)
        })
        .await
    } else {
        None
    };
    let base = if has_audio { 30.0 } else { 5.0 };
    let rep = report.clone();
    let on_frac = move |f: f64| rep("video", (base + f * (97.0 - base)) as u8);

    let tmp = dir.join(".video.tmp.mp4");
    let _ = tokio::fs::remove_file(&tmp).await;
    let res = remux(
        ffmpeg, dir, &all, &tmp, same, target, has_audio, loud, total, on_frac,
    )
    .await;
    let _ = tokio::fs::remove_file(&all).await;
    if let Err(e) = res {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!(
            "{e} ({} segmenti, {})",
            segs.len(),
            if same { "copia" } else { "ricodifica" }
        ));
    }

    let got = probe(ffmpeg, &tmp)
        .await
        .and_then(|p| p.duration_secs)
        .unwrap_or(0.0);
    match expected_secs(dir, &segs).await {
        Some(expected) if !duration_ok(expected, got) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(format!(
                "durata del video {got:.1} s invece di {expected:.1} s"
            ));
        }

        None if got <= 0.0 => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err("il video risulta vuoto".into());
        }
        _ => {}
    }

    report("video", 98);
    tokio::fs::rename(&tmp, dir.join(VIDEO_FILE))
        .await
        .map_err(|e| e.to_string())?;
    remove_segments(dir).await;
    write_extras(ffmpeg, dir).await;
    Ok(true)
}

async fn join_segments(segs: &[(u64, PathBuf)], out: &Path) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut w = tokio::io::BufWriter::new(tokio::fs::File::create(out).await?);
    for (_, p) in segs {
        let mut r = tokio::fs::File::open(p).await?;
        tokio::io::copy(&mut r, &mut w).await?;
    }
    w.flush().await
}

#[allow(clippy::too_many_arguments)]
async fn remux(
    ffmpeg: &Path,
    dir: &Path,
    all: &Path,
    tmp: &Path,
    same: bool,
    target: (u32, u32),
    has_audio: bool,
    loud: Option<Loudness>,
    total: f64,
    on_frac: impl Fn(f64) + Send + 'static,
) -> Result<(), String> {
    let mut c = Command::new(ffmpeg);
    c.current_dir(dir)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-progress",
            "pipe:1",
            "-nostats",
            "-y",
        ])
        .arg("-i")
        .arg(all);
    if same {
        c.args(["-c", "copy"]);
    } else {
        let (w, h) = (target.0 & !1, target.1 & !1);
        c.args([
            "-vf",
            &format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black,setsar=1"),
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
        ]);
    }
    if has_audio {
        match loud {
            Some(l) => {
                c.args([
                    "-af",
                    &format!(
                        "loudnorm=I={TARGET_LUFS}:TP=-1.5:LRA=11:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true",
                        l.i, l.tp, l.lra, l.thresh, l.offset
                    ),
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                    "-ar",
                    "48000",
                ]);
            }

            None => {
                c.args(["-c:a", "copy"]);
            }
        }
    }
    c.args(["-movflags", "+faststart", "-f", "mp4"]).arg(tmp);
    let (status, err) = run_progress(c, total, on_frac).await?;
    if status.success() {
        return Ok(());
    }

    let tail: Vec<&str> = err.lines().rev().take(4).collect();
    Err(format!(
        "ffmpeg {}: {}",
        status,
        if tail.is_empty() {
            "nessun messaggio".to_string()
        } else {
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        }
    ))
}

const TARGET_LUFS: i32 = -16;

#[derive(Debug, Clone, PartialEq)]
pub struct Loudness {
    pub i: String,
    pub tp: String,
    pub lra: String,
    pub thresh: String,
    pub offset: String,
}

async fn measure_loudness(
    ffmpeg: &Path,
    dir: &Path,
    all: &Path,
    total: f64,
    on_frac: impl Fn(f64) + Send + 'static,
) -> Option<Loudness> {
    let mut c = Command::new(ffmpeg);
    c.current_dir(dir)
        .args([
            "-hide_banner",
            "-nostdin",
            "-progress",
            "pipe:1",
            "-nostats",
            "-i",
        ])
        .arg(all)
        .args(["-vn", "-af"])
        .arg(format!(
            "loudnorm=I={TARGET_LUFS}:TP=-1.5:LRA=11:print_format=json"
        ))
        .args(["-f", "null", "-"]);
    let (_, err) = run_progress(c, total, on_frac).await.ok()?;
    parse_loudnorm(&err)
}

pub fn parse_loudnorm(stderr: &str) -> Option<Loudness> {
    let json = &stderr[stderr.rfind('{')?..];
    let get = |key: &str| -> Option<String> {
        let at = json.find(&format!("\"{key}\""))?;
        let rest = &json[at + key.len() + 2..];
        let v = rest.split_once(':')?.1.trim_start().strip_prefix('"')?;
        Some(v.split('"').next()?.to_string())
    };
    let l = Loudness {
        i: get("input_i")?,
        tp: get("input_tp")?,
        lra: get("input_lra")?,
        thresh: get("input_thresh")?,
        offset: get("target_offset")?,
    };

    let i: f64 = l.i.parse().ok()?;
    (i.is_finite() && i > -50.0).then_some(l)
}

async fn remove_segments(dir: &Path) {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        let stem = name
            .strip_suffix(".ts")
            .or_else(|| name.strip_suffix(".dur"));
        let seg = stem.is_some_and(|s| s.parse::<u64>().is_ok());
        if seg || name == ".concat.txt" || name == ".all.ts" {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
}

async fn expected_secs(dir: &Path, segs: &[(u64, PathBuf)]) -> Option<f64> {
    let mut ms = 0u64;
    for (n, _) in segs {
        let d = tokio::fs::read_to_string(dir.join(format!("{n:08}.dur")))
            .await
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?;
        ms += d;
    }
    Some(ms as f64 / 1000.0)
}

pub fn duration_ok(expected: f64, got: f64) -> bool {
    got > 0.0 && (expected - got).abs() <= (expected * 0.05).max(5.0)
}

pub struct Probe {
    pub size: (u32, u32),
    pub duration_secs: Option<f64>,
    pub has_audio: bool,
}

async fn probe(ffmpeg: &Path, file: &Path) -> Option<Probe> {
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new(ffmpeg)
            .args(["-hide_banner", "-i"])
            .arg(file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_probe(&String::from_utf8_lossy(&out.stderr))
}

pub fn parse_probe(stderr: &str) -> Option<Probe> {
    let video = stderr.lines().find(|l| l.contains("Video:"))?;
    let size = video.split([',', ' ']).find_map(|t| {
        let (w, h) = t.split_once('x')?;
        let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
        (w >= 16 && h >= 16).then_some((w, h))
    })?;
    let duration_secs = stderr.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("Duration:")?.trim();
        let t = rest.split(',').next()?.trim();
        let mut p = t.split(':');
        let (h, m, s): (f64, f64, f64) = (
            p.next()?.parse().ok()?,
            p.next()?.parse().ok()?,
            p.next()?.parse().ok()?,
        );
        Some(h * 3600.0 + m * 60.0 + s)
    });
    let has_audio = stderr.lines().any(|l| l.contains("Audio:"));
    Some(Probe {
        size,
        duration_secs,
        has_audio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_reads_size_and_duration() {
        let s = "Input #0, mpegts, from 'x.ts':\n  Duration: 00:01:02.50, start: 1.4, bitrate: 4000 kb/s\n  Stream #0:0: Video: h264 (High), yuv420p(tv), 2560x1440 [SAR 1:1 DAR 16:9], 60 fps\n";
        let p = parse_probe(s).unwrap();
        assert_eq!(p.size, (2560, 1440));
        assert_eq!(p.duration_secs, Some(62.5));
        assert!(parse_probe("niente video").is_none());
    }

    #[test]
    fn loudnorm_json_is_read_and_silence_is_skipped() {
        let out = "[Parsed_loudnorm_0 @ 0x1]
{
	\"input_i\" : \"-24.13\",
	\"input_tp\" : \"-20.40\",
	\"input_lra\" : \"0.00\",
	\"input_thresh\" : \"-34.13\",
	\"output_i\" : \"-16.05\",
	\"target_offset\" : \"0.05\"
}
";
        let l = parse_loudnorm(out).unwrap();
        assert_eq!(
            (
                l.i.as_str(),
                l.tp.as_str(),
                l.thresh.as_str(),
                l.offset.as_str()
            ),
            ("-24.13", "-20.40", "-34.13", "0.05")
        );
        assert!(parse_loudnorm(&out.replace("-24.13", "-inf")).is_none());
        assert!(parse_loudnorm(&out.replace("-24.13", "-70.00")).is_none());
        assert!(parse_loudnorm("nessun json").is_none());
    }

    #[test]
    fn duration_check_allows_small_drift_only() {
        assert!(duration_ok(100.0, 99.0));
        assert!(duration_ok(4.0, 1.0 + 4.0));
        assert!(!duration_ok(100.0, 60.0));
        assert!(!duration_ok(100.0, 0.0));
    }
}

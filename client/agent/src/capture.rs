use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use relay_common::SEGMENT_SECONDS;
use tokio::process::Command;

#[cfg(windows)]
const NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    Nvenc,
    Amf,
    Qsv,
    X264,
}

impl Encoder {
    pub const ALL: [Encoder; 4] = [Encoder::Nvenc, Encoder::Amf, Encoder::Qsv, Encoder::X264];

    pub fn name(self) -> &'static str {
        match self {
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Amf => "h264_amf",
            Encoder::Qsv => "h264_qsv",
            Encoder::X264 => "libx264",
        }
    }

    pub fn parse(s: &str) -> Option<Encoder> {
        match s {
            "nvenc" => Some(Encoder::Nvenc),
            "amf" => Some(Encoder::Amf),
            "qsv" => Some(Encoder::Qsv),
            "x264" => Some(Encoder::X264),
            _ => None,
        }
    }

    fn filter_suffix(self) -> &'static str {
        match self {
            Encoder::Nvenc => "",

            Encoder::Amf | Encoder::Qsv | Encoder::X264 => {
                ",hwdownload,format=bgra,crop=trunc(iw/2)*2:trunc(ih/2)*2,format=nv12"
            }
        }
    }

    fn codec_args(self, kbps: u32) -> Vec<String> {
        let base: &[&str] = match self {
            Encoder::Nvenc => &[
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-tune",
                "ll",
                "-profile:v",
                "high",
            ],
            Encoder::Amf => &[
                "-c:v",
                "h264_amf",
                "-usage",
                "lowlatency",
                "-quality",
                "speed",
                "-rc",
                "cbr",
                "-profile:v",
                "high",
                "-forced_idr",
                "1",
            ],
            Encoder::Qsv => &[
                "-c:v",
                "h264_qsv",
                "-preset",
                "veryfast",
                "-profile:v",
                "high",
            ],
            Encoder::X264 => &[
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-profile:v",
                "high",
            ],
        };
        let mut a: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        a.extend([
            "-b:v".into(),
            format!("{kbps}k"),
            "-maxrate".into(),
            format!("{kbps}k"),
        ]);
        a.extend(["-bufsize".into(), format!("{}k", kbps * 2)]);
        a
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowSel {
    Exe(String),

    TitleRegex(String),

    Monitor(u32),
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub ffmpeg: PathBuf,
    pub window: WindowSel,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub dir: PathBuf,
    pub encoder: Encoder,

    pub duration_secs: Option<u32>,

    pub origin_unix_secs: Option<f64>,

    pub start_segment: u64,

    pub generation: u32,

    pub audio_port: Option<u16>,
}

fn regex_exact(s: &str) -> String {
    let mut out = String::from("(?i)^");
    for c in s.chars() {
        if ".^$|?*+()[]{}\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('$');
    out
}

fn filter_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "'\\''"))
}

fn source_filter(sel: &str, fps: u32, enc: Encoder, origin: Option<f64>) -> String {
    let clock = match origin {
        Some(o) => format!(",setpts='(time(0)-{o:.6})/TB',trim=start=0,fps=fps={fps}:start_time=0"),
        None => String::new(),
    };
    format!(
        "gfxcapture={sel}:max_framerate={fps}{clock}{}[v]",
        enc.filter_suffix()
    )
}

fn capture_filter(sel: &WindowSel, fps: u32, enc: Encoder, origin: Option<f64>) -> String {
    match sel {
        WindowSel::Exe(e) => source_filter(
            &format!("window_exe={}", filter_quote(&regex_exact(e))),
            fps,
            enc,
            origin,
        ),
        WindowSel::TitleRegex(t) => source_filter(
            &format!("window_title={}", filter_quote(t)),
            fps,
            enc,
            origin,
        ),

        WindowSel::Monitor(idx) => source_filter(
            &format!("monitor_idx={idx}:capture_cursor=0"),
            fps,
            enc,
            origin,
        ),
    }
}

pub fn build_args(cfg: &CaptureConfig) -> Vec<String> {
    let gop = cfg.fps * SEGMENT_SECONDS;
    let mut a: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        std::env::var("RELAY_FFMPEG_LOGLEVEL").unwrap_or_else(|_| "warning".into()),
        "-nostats".into(),
        "-y".into(),
    ];
    if let Some(port) = cfg.audio_port {
        a.extend([
            "-thread_queue_size".into(),
            "1024".into(),
            "-f".into(),
            "s16le".into(),
            "-ar".into(),
            "48000".into(),
            "-ac".into(),
            "2".into(),
            "-i".into(),
            format!("tcp://127.0.0.1:{port}"),
        ]);
    }
    a.extend([
        "-filter_complex".into(),
        capture_filter(&cfg.window, cfg.fps, cfg.encoder, cfg.origin_unix_secs),
        "-map".into(),
        "[v]".into(),
        "-fps_mode".into(),
        "cfr".into(),
        "-r".into(),
        cfg.fps.to_string(),
    ]);
    if cfg.audio_port.is_some() {
        a.extend(
            [
                "-map",
                "0:a",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-shortest",
            ]
            .map(String::from),
        );
    }
    if let Some(d) = cfg.duration_secs {
        a.extend(["-t".into(), d.to_string()]);
    }
    if cfg.start_segment > 0 {
        a.extend([
            "-output_ts_offset".into(),
            (cfg.start_segment * SEGMENT_SECONDS as u64).to_string(),
        ]);
    }
    a.extend(cfg.encoder.codec_args(cfg.bitrate_kbps));
    a.extend([
        "-g".into(),
        gop.to_string(),
        "-keyint_min".into(),
        gop.to_string(),
        "-force_key_frames".into(),

        format!("expr:isnan(prev_forced_t)+gt(floor(t/{SEGMENT_SECONDS}),floor(prev_forced_t/{SEGMENT_SECONDS}))"),
        "-f".into(),
        "hls".into(),
        "-hls_time".into(),
        SEGMENT_SECONDS.to_string(),
        "-hls_list_size".into(),
        "0".into(),

        "-hls_flags".into(),
        "independent_segments+temp_file".into(),
        "-start_number".into(),
        cfg.start_segment.to_string(),
        "-hls_segment_filename".into(),
        cfg.dir.join("seg_%08d.ts").to_string_lossy().into_owned(),
        cfg.dir.join(crate::hls::playlist_name(cfg.generation)).to_string_lossy().into_owned(),
    ]);
    a
}

fn command(ffmpeg: &Path) -> Command {
    let mut c = Command::new(ffmpeg);
    #[cfg(windows)]
    c.creation_flags(NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    c.kill_on_drop(true);
    c
}

pub async fn detect_encoder(ffmpeg: &Path) -> Result<Encoder> {
    for enc in Encoder::ALL {
        let mut c = command(ffmpeg);
        c.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=1280x720:rate=30,format=nv12")
            .args(["-frames:v", "15"])
            .args(enc.codec_args(2000))
            .args(["-f", "null", "-"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let ok = tokio::time::timeout(Duration::from_secs(20), c.status()).await;
        if matches!(ok, Ok(Ok(s)) if s.success()) {
            return Ok(enc);
        }
    }
    bail!("nessun encoder H.264 funzionante (provati nvenc, amf, qsv, x264)")
}

pub async fn fill_gap(
    ffmpeg: &Path,
    dir: &Path,
    generation: u32,
    from: u64,
    to: u64,
    audio: bool,
) -> Result<()> {
    if to <= from {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let secs = (to - from) * SEGMENT_SECONDS as u64;
    let mut c = command(ffmpeg);
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
    ])
    .arg("color=c=0x101010:s=1280x720:r=30");
    if audio {
        c.args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-ar",
            "48000",
            "-ac",
            "2",
        ]);
    }
    c.args(["-t", &secs.to_string()])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args([
            "-g",
            &(30 * SEGMENT_SECONDS).to_string(),
            "-keyint_min",
            &(30 * SEGMENT_SECONDS).to_string(),
        ])
        .args(["-sc_threshold", "0", "-b:v", "300k"])
        .args([
            "-output_ts_offset",
            &(from * SEGMENT_SECONDS as u64).to_string(),
        ])
        .args([
            "-f",
            "hls",
            "-hls_time",
            &SEGMENT_SECONDS.to_string(),
            "-hls_list_size",
            "0",
        ])
        .args([
            "-hls_flags",
            "independent_segments+temp_file",
            "-start_number",
            &from.to_string(),
        ])
        .arg("-hls_segment_filename")
        .arg(dir.join("seg_%08d.ts"))
        .arg(dir.join(crate::hls::playlist_name(generation)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let st = tokio::time::timeout(Duration::from_secs(30 * 60), c.status())
        .await
        .context("i segmenti di riempimento non sono finiti in tempo")??;
    if !st.success() {
        bail!("ffmpeg non e' riuscito a creare i segmenti di riempimento");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MonitorInfo {
    pub index: u32,
    pub width: u32,
    pub height: u32,
}

pub fn parse_video_size(ffmpeg_stderr: &str) -> Option<(u32, u32)> {
    let line = ffmpeg_stderr.lines().find(|l| l.contains("Video:"))?;
    let re = regex::Regex::new(r"\b(\d{3,5})x(\d{3,5})\b").ok()?;
    let c = re.captures(line)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?))
}

pub async fn list_monitors(ffmpeg: &Path) -> Vec<MonitorInfo> {
    let mut out = Vec::new();
    for index in 0..8u32 {
        let mut c = command(ffmpeg);
        c.args(["-hide_banner", "-loglevel", "info", "-f", "lavfi", "-i"])
            .arg(format!("ddagrab=output_idx={index}:framerate=5"))
            .args(["-frames:v", "1", "-f", "null", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let run = tokio::time::timeout(Duration::from_secs(8), c.output()).await;
        let Ok(Ok(o)) = run else { break };
        if !o.status.success() {
            break;
        }
        match parse_video_size(&String::from_utf8_lossy(&o.stderr)) {
            Some((width, height)) => out.push(MonitorInfo {
                index,
                width,
                height,
            }),
            None => break,
        }
    }
    out
}

#[cfg(windows)]
fn kill_with_us(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    static JOB: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        h as isize
    });
    if job != 0 {
        unsafe {
            AssignProcessToJobObject(job as _, child.as_raw_handle() as _);
        }
    }
}

#[cfg(not(windows))]
fn kill_with_us(_child: &std::process::Child) {}

pub struct Capture {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
}

impl Capture {
    pub fn spawn(cfg: &CaptureConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.dir).context("creazione cartella di lavoro")?;
        let mut c = std::process::Command::new(&cfg.ffmpeg);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
        }
        c.args(build_args(cfg)).stdin(Stdio::piped());
        let mut child = c
            .spawn()
            .with_context(|| format!("avvio di {}", cfg.ffmpeg.display()))?;
        let stdin = child.stdin.take();
        kill_with_us(&child);
        Ok(Self { child, stdin })
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        loop {
            if let Some(st) = self.child.try_wait()? {
                return Ok(st);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(
                b"q
",
            );
            let _ = stdin.flush();

            self.stdin = Some(stdin);
        }
        match tokio::time::timeout(Duration::from_secs(15), self.wait()).await {
            Ok(r) => {
                r?;
            }
            Err(_) => {
                tracing::warn!("ffmpeg non si e' fermato in 15 s, lo termino");
                self.child.kill()?;
                self.child.wait()?;
            }
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_regex_is_escaped_and_anchored() {
        assert_eq!(regex_exact("game.exe"), r"(?i)^game\.exe$");
    }

    #[test]
    fn filter_quoting_doubles_backslash() {
        assert_eq!(filter_quote(r"a\.b"), r"'a\\.b'");
        assert_eq!(filter_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn args_align_gop_to_segments() {
        let cfg = CaptureConfig {
            ffmpeg: "ffmpeg".into(),
            window: WindowSel::Exe("game.exe".into()),
            fps: 60,
            bitrate_kbps: 5000,
            dir: "w".into(),
            encoder: Encoder::Nvenc,
            duration_secs: None,
            origin_unix_secs: None,
            start_segment: 0,
            generation: 0,
            audio_port: None,
        };
        let a = build_args(&cfg);
        let g = a.iter().position(|x| x == "-g").unwrap();
        assert_eq!(a[g + 1], "240");
        assert!(a.iter().any(|x| x.starts_with("expr:isnan(prev_forced_t)")));
    }

    #[test]
    fn amd_encoder_is_told_to_make_real_keyframes() {
        let a = Encoder::Amf.codec_args(4000);
        let i = a
            .iter()
            .position(|x| x == "-forced_idr")
            .expect("manca -forced_idr");
        assert_eq!(a[i + 1], "1");
    }

    #[test]
    fn clock_filter_uses_origin() {
        let f = source_filter("window_exe='x'", 60, Encoder::Nvenc, Some(1789809292.5));
        assert!(f.contains("setpts='(time(0)-1789809292.500000)/TB'"), "{f}");
        assert!(f.contains("trim=start=0,fps=fps=60:start_time=0"));
        assert!(f.ends_with("[v]"));
    }

    #[test]
    fn video_size_is_read_from_ffmpeg_output() {
        let s = "Input #0, lavfi, from 'ddagrab=output_idx=0':\n  Stream #0:0: Video: wrapped_avframe, d3d11(bgra), 2560x1440 [SAR 1:1 DAR 16:9], 5 fps, 5 tbr, 5 tbn\n";
        assert_eq!(parse_video_size(s), Some((2560, 1440)));
        assert_eq!(parse_video_size("nessun video qui"), None);
        assert_eq!(
            parse_video_size("  Stream #0:0: Video: h264, yuv420p(tv), 1920x1080, 60 fps"),
            Some((1920, 1080))
        );
    }

    #[test]
    fn monitor_capture_uses_graphics_capture() {
        let cfg = CaptureConfig {
            ffmpeg: "ffmpeg".into(),
            window: WindowSel::Monitor(0),
            fps: 60,
            bitrate_kbps: 5000,
            dir: "w".into(),
            encoder: Encoder::Nvenc,
            duration_secs: None,
            origin_unix_secs: None,
            start_segment: 0,
            generation: 0,
            audio_port: None,
        };
        let args = build_args(&cfg);
        let i = args.iter().position(|x| x == "-filter_complex").unwrap();
        assert_eq!(
            args[i + 1],
            "gfxcapture=monitor_idx=0:capture_cursor=0:max_framerate=60[v]"
        );
    }
}

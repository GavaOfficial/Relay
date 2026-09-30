pub mod aac;
pub mod clock;
pub mod h264;
pub mod hls;
pub mod layout;
pub mod mix;
pub mod ts;

#[cfg(windows)]
mod win;

use std::{path::PathBuf, sync::Arc, time::Duration};

pub const ENGINE: &str = "relay";

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Window { exe: String },
    Monitor { index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderPref {
    #[default]
    Auto,
    Nvenc,
    Amf,
    Qsv,
    Software,
}

impl EncoderPref {
    pub fn name(self) -> &'static str {
        match self {
            EncoderPref::Auto => "auto",
            EncoderPref::Nvenc => "nvenc",
            EncoderPref::Amf => "amf",
            EncoderPref::Qsv => "qsv",
            EncoderPref::Software => "software",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub dir: PathBuf,
    pub generation: u32,
    pub origin_unix_secs: Option<f64>,
    pub target: Target,
    pub fallback_monitor: Option<u32>,
    pub fallback_monitor_name: Option<String>,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub encoder: EncoderPref,
    pub game_audio: Option<String>,
    pub mic_gain: Option<f32>,
    pub start_segment: u64,
    pub segment_secs: u32,
    pub output_height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started {
        generation: u32,
        first_frame_unix_secs: f64,
        encoder: String,
        source: String,
    },
    Resized {
        generation: u32,
        width: u32,
        height: u32,
    },
    SourceChanged(String),
    Warning(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameCheck {
    Frames,
    Waiting(String),
    Broken(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub exe: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub encoders: Vec<String>,
    pub best: String,
    pub rejected: Vec<String>,
    pub yellow_border: bool,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
}

pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

#[cfg(windows)]
pub struct Prepared(win::engine::Prepared);
#[cfg(windows)]
pub struct Recorder(win::engine::Recorder);

#[cfg(not(windows))]
pub struct Prepared(std::convert::Infallible);
#[cfg(not(windows))]
pub struct Recorder(std::convert::Infallible);

#[cfg(not(windows))]
const UNSUPPORTED: &str = "la registrazione funziona solo su Windows 10 2004 o successivo";

#[cfg(windows)]
pub fn prepare(cfg: Config, events: EventSink) -> anyhow::Result<Prepared> {
    win::engine::prepare(cfg, events).map(Prepared)
}

#[cfg(not(windows))]
pub fn prepare(_cfg: Config, _events: EventSink) -> anyhow::Result<Prepared> {
    anyhow::bail!(UNSUPPORTED)
}

#[cfg(windows)]
pub fn probe() -> anyhow::Result<Probe> {
    win::probe::run()
}

#[cfg(windows)]
pub fn check_encoders() -> Vec<String> {
    win::probe::encoders()
}

#[cfg(not(windows))]
pub fn check_encoders() -> Vec<String> {
    vec![UNSUPPORTED.into()]
}

#[cfg(not(windows))]
pub fn probe() -> anyhow::Result<Probe> {
    anyhow::bail!(UNSUPPORTED)
}

impl Prepared {
    pub fn wait_frames(&self, timeout: Duration) -> FrameCheck {
        #[cfg(windows)]
        return self.0.wait_frames(timeout);
        #[cfg(not(windows))]
        {
            let _ = timeout;
            match self.0 {}
        }
    }

    pub fn start(self) -> Recorder {
        #[cfg(windows)]
        return Recorder(self.0.start());
        #[cfg(not(windows))]
        match self.0 {}
    }

    pub fn abort(self) {
        #[cfg(windows)]
        self.0.abort();
        #[cfg(not(windows))]
        match self.0 {}
    }
}

impl Recorder {
    pub fn fit_window(&self) {
        #[cfg(windows)]
        self.0.fit_window();
        #[cfg(not(windows))]
        match self.0 {}
    }

    pub fn captured_frames(&self) -> u64 {
        #[cfg(windows)]
        return self.0.captured_frames();
        #[cfg(not(windows))]
        match self.0 {}
    }

    pub fn failure(&self) -> Option<String> {
        #[cfg(windows)]
        return self.0.failure();
        #[cfg(not(windows))]
        match self.0 {}
    }

    pub fn stop(self) -> anyhow::Result<()> {
        #[cfg(windows)]
        return self.0.stop();
        #[cfg(not(windows))]
        match self.0 {}
    }
}

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const HOST_ARG: &str = "--capture-host";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Window { exe: String },
    Monitor { index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EncoderChoice {
    #[default]
    Auto,
    Nvenc,
    Amf,
    Qsv,
    X264,
}

impl EncoderChoice {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "nvenc" => Some(Self::Nvenc),
            "amf" => Some(Self::Amf),
            "qsv" => Some(Self::Qsv),
            "x264" => Some(Self::X264),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordConfig {
    pub dir: PathBuf,

    pub origin_unix_secs: Option<f64>,
    pub playlist: String,
    pub source: Source,
    pub fallback_monitor: Option<u32>,
    #[serde(default)]
    pub fallback_monitor_name: Option<String>,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub encoder: EncoderChoice,
    pub game_audio_exe: Option<String>,
    pub mic: bool,
    pub mic_gain: f32,
    pub start_segment: u64,
    pub segment_secs: u32,
    pub output_height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub index: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub exe: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Probe,
    Start(RecordConfig),
    Stop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Ready {
        obs_version: String,
    },
    Probed {
        encoders: Vec<String>,
        monitors: Vec<MonitorInfo>,
        windows: Vec<WindowInfo>,
    },
    Started {
        encoder: String,
        source: String,
        first_frame_unix_secs: f64,
    },
    SourceChanged {
        source: String,
    },
    Warning(String),
    Error(String),
    Stopped,
}

pub fn encode<T: Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string(v).expect("i messaggi si serializzano sempre");
    s.push('\n');
    s
}

pub fn decode<T: for<'a> Deserialize<'a>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_survive_a_round_trip_on_one_line() {
        let cfg = RecordConfig {
            dir: PathBuf::from("C:/Relay/work"),
            playlist: "out_2.m3u8".into(),
            source: Source::Window {
                exe: "game.exe".into(),
            },
            origin_unix_secs: Some(1_700_000_000.5),
            fallback_monitor: Some(1),
            fallback_monitor_name: Some(r"\\.\DISPLAY2".into()),
            fps: 60,
            bitrate_kbps: 6000,
            encoder: EncoderChoice::Nvenc,
            game_audio_exe: Some("game.exe".into()),
            mic: true,
            mic_gain: 3.0,
            start_segment: 12,
            segment_secs: 4,
            output_height: 1080,
        };
        let line = encode(&Command::Start(cfg.clone()));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(decode::<Command>(&line).unwrap(), Command::Start(cfg));

        let ev = Event::Started {
            encoder: "nvenc".into(),
            source: "game".into(),
            first_frame_unix_secs: 1.5,
        };
        assert_eq!(decode::<Event>(&encode(&ev)).unwrap(), ev);
        assert!(decode::<Command>("non e' json").is_err());
    }

    #[test]
    fn encoder_names_are_parsed() {
        assert_eq!(EncoderChoice::parse("nvenc"), Some(EncoderChoice::Nvenc));
        assert_eq!(EncoderChoice::parse("auto"), Some(EncoderChoice::Auto));
        assert_eq!(EncoderChoice::parse("boh"), None);
    }
}

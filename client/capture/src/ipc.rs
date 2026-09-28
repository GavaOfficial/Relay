use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const HOST_ARG: &str = "--capture-host";

pub const ENGINE_RELAY: &str = "relay";
pub const ENGINE_OBS: &str = "obs";

fn engine_obs() -> String {
    ENGINE_OBS.into()
}

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
        #[serde(default = "engine_obs")]
        engine: String,
    },
    Probed {
        encoders: Vec<String>,
        monitors: Vec<MonitorInfo>,
        windows: Vec<WindowInfo>,
        #[serde(default)]
        best: Option<String>,
        #[serde(default)]
        yellow_border: bool,
    },
    Started {
        encoder: String,
        source: String,
        first_frame_unix_secs: f64,
        #[serde(default)]
        generation: u32,
        #[serde(default = "engine_obs")]
        engine: String,
    },
    Resized {
        generation: u32,
        width: u32,
        height: u32,
    },
    SourceChanged {
        source: String,
    },
    Warning(String),
    Error(String),
    Fallback {
        reason: String,
    },
    Stopped,
}

pub fn playlist_generation(name: &str) -> u32 {
    name.strip_prefix("out_")
        .and_then(|s| s.strip_suffix(".m3u8"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub fn legacy_encoder_id(name: &str) -> Option<&'static str> {
    match name {
        "nvenc" => Some("obs_nvenc_h264_tex"),
        "amf" => Some("h264_texture_amf"),
        "qsv" => Some("obs_qsv11"),
        "software" | "x264" => Some("obs_x264"),
        _ => None,
    }
}

pub fn encoder_name(id: &str) -> Option<&'static str> {
    match id {
        "nvenc" | "obs_nvenc_h264_tex" => Some("nvenc"),
        "amf" | "h264_texture_amf" => Some("amf"),
        "qsv" | "obs_qsv11" => Some("qsv"),
        "software" => Some("software"),
        "x264" | "obs_x264" => Some("x264"),
        _ => None,
    }
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
            generation: 2,
            engine: ENGINE_RELAY.into(),
        };
        assert_eq!(decode::<Event>(&encode(&ev)).unwrap(), ev);
        let fallback = Event::Fallback {
            reason: "nessuna immagine".into(),
        };
        assert_eq!(decode::<Event>(&encode(&fallback)).unwrap(), fallback);
        assert!(decode::<Command>("non e' json").is_err());
    }

    #[test]
    fn messages_from_the_obs_version_are_still_understood() {
        let old = r#"{"Started":{"encoder":"nvenc","source":"game","first_frame_unix_secs":2.5}}"#;
        match decode::<Event>(old).unwrap() {
            Event::Started {
                generation, engine, ..
            } => {
                assert_eq!(generation, 0);
                assert_eq!(engine, ENGINE_OBS);
            }
            other => panic!("{other:?}"),
        }
        let ready = r#"{"Ready":{"obs_version":"32.2.2"}}"#;
        assert_eq!(
            decode::<Event>(ready).unwrap(),
            Event::Ready {
                obs_version: "32.2.2".into(),
                engine: ENGINE_OBS.into()
            }
        );
        let probed = r#"{"Probed":{"encoders":["obs_x264"],"monitors":[],"windows":[]}}"#;
        assert!(matches!(
            decode::<Event>(probed).unwrap(),
            Event::Probed { best: None, .. }
        ));
    }

    #[test]
    fn playlist_names_give_their_generation() {
        assert_eq!(playlist_generation("out.m3u8"), 0);
        assert_eq!(playlist_generation("out_7.m3u8"), 7);
        assert_eq!(playlist_generation("altro"), 0);
    }

    #[test]
    fn encoder_names_map_both_ways() {
        for name in ["nvenc", "amf", "qsv", "software"] {
            let legacy = legacy_encoder_id(name).unwrap();
            let back = encoder_name(legacy).unwrap();
            assert_eq!(back, if name == "software" { "x264" } else { name });
            assert_eq!(encoder_name(name), Some(name));
        }
        assert_eq!(encoder_name("boh"), None);
    }

    #[test]
    fn encoder_names_are_parsed() {
        assert_eq!(EncoderChoice::parse("nvenc"), Some(EncoderChoice::Nvenc));
        assert_eq!(EncoderChoice::parse("auto"), Some(EncoderChoice::Auto));
        assert_eq!(EncoderChoice::parse("boh"), None);
    }
}

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const BIG_REQUEST: u64 = 32 * crate::storage::PLAIN_CHUNK;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobKind {
    Match { match_id: Uuid, player: String },
    Clip {
        engine: String,
        clip: Uuid,
        #[serde(default)]
        offset_secs: f64,
        #[serde(default)]
        duration_secs: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputFile {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    #[serde(flatten)]
    pub kind: JobKind,
    pub inputs: Vec<InputFile>,
    #[serde(default)]
    pub threads: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Poll {
    pub version: String,
    pub threads: u32,
    pub load: f32,
    #[serde(default)]
    pub ffmpeg: Option<String>,
    #[serde(default)]
    pub running: Vec<Uuid>,
    #[serde(default)]
    pub slots: u32,
}

pub fn auto_parallel(threads: u32) -> u32 {
    (threads / 6).clamp(1, 4)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub stage: String,
    pub pct: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finish {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BigState {
    pub next: u64,
}

pub const OUT_VIDEO: &str = "video";
pub const OUT_WEB: &str = "web";
pub const OUT_VOD_MEDIA: &str = "vod_media";
pub const OUT_VOD_INDEX: &str = "vod_index";
pub const OUT_THUMB: &str = "thumb";
pub const OUT_INFO: &str = "info";
pub const OUT_CLIP: &str = "clip";

pub fn is_big(output: &str) -> bool {
    matches!(output, OUT_VIDEO | OUT_WEB | OUT_VOD_MEDIA | OUT_CLIP)
}

pub fn valid_input_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_round_trip_as_flat_json() {
        let job = Job {
            id: Uuid::nil(),
            kind: JobKind::Clip { engine: "psych".into(), clip: Uuid::nil(), offset_secs: 1.5, duration_secs: 90.0 },
            inputs: vec![InputFile { name: "source.ts".into(), size: 10 }],
            threads: 4,
        };
        let text = serde_json::to_string(&job).unwrap();
        assert!(text.contains("\"kind\":\"clip\""));
        assert_eq!(serde_json::from_str::<Job>(&text).unwrap(), job);
    }

    #[test]
    fn parallel_jobs_grow_with_the_processor() {
        assert_eq!(auto_parallel(2), 1);
        assert_eq!(auto_parallel(12), 2);
        assert_eq!(auto_parallel(32), 4);
    }

    #[test]
    fn input_names_cannot_escape_the_folder() {
        assert!(valid_input_name("00000012.ts"));
        assert!(valid_input_name("video.mp4"));
        assert!(!valid_input_name("../x"));
        assert!(!valid_input_name("a/b"));
        assert!(!valid_input_name(".all.ts"));
    }
}

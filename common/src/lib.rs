use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SEGMENT_SECONDS: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchStatus {
    Open,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchInfo {
    pub id: Uuid,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub coordinator: String,
    pub players: Vec<String>,
    pub finished: Vec<String>,
    pub status: MatchStatus,
    pub created_at: u64,

    #[serde(default)]
    pub started_at_ms: Option<u64>,

    #[serde(default)]
    pub stopped_at_ms: Option<u64>,

    #[serde(default)]
    pub names: BTreeMap<String, String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_token: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_app_id: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_cover_url: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fnf_song_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fnf_difficulty: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fnf_score: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fnf_accuracy: Option<f32>,

    // Ogni nota mancata, con l'istante esatto (ms dall'inizio della registrazione) in cui e'
    // successa: serve per poter in futuro allineare la lista alla posizione del video.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fnf_misses: Vec<FnfMiss>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FnfMiss {
    pub at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateMatch {
    #[serde(default)]
    pub players: Vec<String>,

    #[serde(default)]
    pub play: bool,

    #[serde(default)]
    pub name: Option<String>,
}

pub const MAX_NAME_CHARS: usize = 60;

pub fn clean_name(raw: &str) -> Option<String> {
    let s: String = raw
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(MAX_NAME_CHARS).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGrant {
    pub session: String,
    pub expires_in: u64,
    pub user: String,
    pub name: Option<String>,
}

pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub const START_LEAD_MS: u64 = 5_000;

pub fn next_boundary(origin_ms: u64, min_ms: u64) -> u64 {
    let seg = SEGMENT_SECONDS as u64 * 1000;
    if min_ms <= origin_ms {
        return origin_ms;
    }
    origin_ms + (min_ms - origin_ms).div_ceil(seg) * seg
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMsg {
    Ping { t0: u64 },

    Status(PlayerStatus),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMsg {
    Pong {
        t0: u64,
        server_ms: u64,
    },

    State {
        status: MatchStatus,
        start_at_ms: Option<u64>,
        stop_at_ms: Option<u64>,
    },

    Presence {
        connected: Vec<String>,
    },

    Health {
        players: Vec<PlayerHealth>,
    },
    Start {
        at_ms: u64,
    },
    Stop {
        at_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    #[default]
    Idle,

    Waiting,
    Recording,

    Uploading,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlayerStatus {
    pub state: AgentState,

    pub window_found: Option<bool>,
    pub encoder: Option<String>,

    pub backlog: u32,

    pub upload_kbps: u32,

    pub rtt_ms: Option<u32>,

    pub offset_ms: Option<i64>,
    pub issue: Option<String>,

    #[serde(default)]
    pub window: Option<String>,

    #[serde(default)]
    pub audio: Option<String>,

    #[serde(default)]
    pub audio_issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerHealth {
    pub id: String,
    pub connected: bool,
    pub ready: bool,
    pub status: Option<PlayerStatus>,

    pub age_ms: Option<u64>,

    pub issue: Option<String>,
}

pub fn redact_health(mut players: Vec<PlayerHealth>, is_host: bool) -> Vec<PlayerHealth> {
    if !is_host {
        for p in &mut players {
            if let Some(s) = p.status.as_mut() {
                s.window = None;
            }
        }
    }
    players
}

pub const STATUS_MAX_AGE_MS: u64 = 8_000;

pub const MAX_BACKLOG: u32 = 3;

pub fn not_ready_reason(
    connected: bool,
    status: Option<&PlayerStatus>,
    age_ms: Option<u64>,
) -> Option<String> {
    if !connected {
        return Some("non connesso".into());
    }
    let Some(s) = status else {
        return Some("nessuno stato ricevuto".into());
    };
    if age_ms.is_none_or(|a| a > STATUS_MAX_AGE_MS) {
        return Some("l'app non risponde".into());
    }
    if s.state == AgentState::Error {
        return Some(s.issue.clone().unwrap_or_else(|| "errore".into()));
    }
    match s.window_found {
        Some(true) => {}
        Some(false) => {
            return Some(
                s.issue
                    .clone()
                    .unwrap_or_else(|| "finestra non trovata".into()),
            )
        }
        None => return Some("finestra non verificata".into()),
    }
    if s.rtt_ms.is_none() {
        return Some("orologio non sincronizzato".into());
    }
    if s.backlog > MAX_BACKLOG {
        return Some(format!("upload in ritardo ({} pezzi in coda)", s.backlog));
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn match_names_are_cleaned() {
        assert_eq!(
            clean_name("  Finale   di\tcampionato \n"),
            Some("Finale di campionato".into())
        );
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name("a\u{7}b"), Some("ab".into()));
        assert_eq!(
            clean_name(&"x".repeat(200)).map(|n| n.chars().count()),
            Some(MAX_NAME_CHARS)
        );
        assert_eq!(
            clean_name("Partita 🎮 di prova"),
            Some("Partita 🎮 di prova".into())
        );
    }

    use super::*;

    #[test]
    fn boundary_is_on_grid() {
        assert_eq!(next_boundary(1000, 1000), 1000);
        assert_eq!(next_boundary(1000, 1001), 5000);
        assert_eq!(next_boundary(1000, 5000), 5000);
        assert_eq!(next_boundary(1000, 900), 1000);
    }

    #[test]
    fn msg_json_shape() {
        let s = serde_json::to_string(&ServerMsg::Start { at_ms: 7 }).unwrap();
        assert_eq!(s, r#"{"type":"start","at_ms":7}"#);
        let c: ClientMsg = serde_json::from_str(r#"{"type":"ping","t0":5}"#).unwrap();
        matches!(c, ClientMsg::Ping { t0: 5 });
    }

    fn ok_status() -> PlayerStatus {
        PlayerStatus {
            state: AgentState::Waiting,
            window_found: Some(true),
            rtt_ms: Some(20),
            ..Default::default()
        }
    }

    #[test]
    fn readiness_rules() {
        let s = ok_status();
        assert_eq!(not_ready_reason(true, Some(&s), Some(500)), None);
        assert_eq!(
            not_ready_reason(false, Some(&s), Some(500)).as_deref(),
            Some("non connesso")
        );
        assert!(not_ready_reason(true, None, None).is_some());
        assert_eq!(
            not_ready_reason(true, Some(&s), Some(9_000)).as_deref(),
            Some("l'app non risponde")
        );
        let mut w = ok_status();
        w.window_found = Some(false);
        assert_eq!(
            not_ready_reason(true, Some(&w), Some(1)).as_deref(),
            Some("finestra non trovata")
        );
        w.window_found = None;
        assert_eq!(
            not_ready_reason(true, Some(&w), Some(1)).as_deref(),
            Some("finestra non verificata")
        );

        w.issue = Some("finestra non scelta".into());
        w.window_found = Some(false);
        assert_eq!(
            not_ready_reason(true, Some(&w), Some(1)).as_deref(),
            Some("finestra non scelta")
        );
        let mut c = ok_status();
        c.rtt_ms = None;
        assert_eq!(
            not_ready_reason(true, Some(&c), Some(1)).as_deref(),
            Some("orologio non sincronizzato")
        );
        let mut b = ok_status();
        b.backlog = 4;
        assert!(not_ready_reason(true, Some(&b), Some(1))
            .unwrap()
            .contains("4 pezzi"));
        b.backlog = 3;
        assert_eq!(not_ready_reason(true, Some(&b), Some(1)), None);
        let mut e = ok_status();
        e.state = AgentState::Error;
        e.issue = Some("ffmpeg non parte".into());
        assert_eq!(
            not_ready_reason(true, Some(&e), Some(1)).as_deref(),
            Some("ffmpeg non parte")
        );
    }

    #[test]
    fn status_message_json_shape() {
        let m = ClientMsg::Status(ok_status());
        let j = serde_json::to_string(&m).unwrap();
        assert!(
            j.starts_with(r#"{"type":"status","state":"waiting""#),
            "{j}"
        );
        let back: ClientMsg = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, ClientMsg::Status(s) if s.window_found == Some(true)));
    }

    #[test]
    fn window_name_is_only_for_the_host() {
        let mk = |w: &str| PlayerHealth {
            id: "a".into(),
            connected: true,
            ready: true,
            status: Some(PlayerStatus {
                window: Some(w.into()),
                ..ok_status()
            }),
            age_ms: Some(1),
            issue: None,
        };
        let host = redact_health(vec![mk("game.exe - Gioco")], true);
        assert_eq!(
            host[0].status.as_ref().unwrap().window.as_deref(),
            Some("game.exe - Gioco")
        );
        let other = redact_health(vec![mk("game.exe - Gioco")], false);
        assert_eq!(other[0].status.as_ref().unwrap().window, None);

        assert_eq!(other[0].status.as_ref().unwrap().window_found, Some(true));
    }

    #[test]
    fn old_match_files_still_load() {
        let json = r#"{"id":"86870f34-c44b-465c-a073-d360a5670cc8","coordinator":"a","players":["a"],
            "finished":[],"status":"open","created_at":1}"#;
        let m: MatchInfo = serde_json::from_str(json).unwrap();
        assert!(m.names.is_empty() && m.invite_code.is_none());
    }
}

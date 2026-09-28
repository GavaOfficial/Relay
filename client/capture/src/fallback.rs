use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "engine.json";
pub const STRIKES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
struct Marker {
    version: String,
    #[serde(default)]
    engine: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    strikes: u32,
}

fn load(dir: &Path, version: &str) -> Marker {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Marker>(&t).ok())
        .filter(|m| m.version == version)
        .unwrap_or_else(|| Marker {
            version: version.into(),
            ..Default::default()
        })
}

fn save(dir: &Path, m: &Marker) {
    if let Ok(text) = serde_json::to_vec_pretty(m) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(FILE), text);
    }
}

pub fn obs_reason(dir: &Path, version: &str) -> Option<String> {
    let m = load(dir, version);
    (m.engine == crate::ipc::ENGINE_OBS).then_some(m.reason)
}

pub fn strikes(dir: &Path, version: &str) -> u32 {
    load(dir, version).strikes
}

pub fn strike(dir: &Path, version: &str, reason: &str) -> u32 {
    let mut m = load(dir, version);
    m.strikes += 1;
    m.reason = reason.into();
    if m.strikes >= STRIKES {
        m.engine = crate::ipc::ENGINE_OBS.into();
    }
    save(dir, &m);
    m.strikes
}

pub fn remember_obs(dir: &Path, version: &str, reason: &str) {
    let m = Marker {
        version: version.into(),
        engine: crate::ipc::ENGINE_OBS.into(),
        reason: reason.into(),
        strikes: STRIKES,
    };
    save(dir, &m);
}

pub fn succeeded(dir: &Path, version: &str) {
    let mut m = load(dir, version);
    if m.strikes > 0 && m.engine != crate::ipc::ENGINE_OBS {
        m.strikes = 0;
        m.reason.clear();
        save(dir, &m);
    }
}

pub fn forget(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("relay-fallback-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_choice_holds_only_for_the_same_version() {
        let d = temp("version");
        assert_eq!(obs_reason(&d, "0.4.0"), None);
        remember_obs(&d, "0.4.0", "nessuna immagine");
        assert_eq!(obs_reason(&d, "0.4.0").as_deref(), Some("nessuna immagine"));
        assert_eq!(obs_reason(&d, "0.4.1"), None);
        assert_eq!(strikes(&d, "0.4.1"), 0);
        forget(&d);
        assert_eq!(obs_reason(&d, "0.4.0"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn obs_is_kept_only_after_three_failures_in_a_row() {
        let d = temp("strikes");
        assert_eq!(strike(&d, "1", "gioco in caricamento"), 1);
        assert_eq!(obs_reason(&d, "1"), None);
        assert_eq!(strike(&d, "1", "finestra coperta"), 2);
        assert_eq!(obs_reason(&d, "1"), None);
        succeeded(&d, "1");
        assert_eq!(strikes(&d, "1"), 0);
        for n in 1..=STRIKES {
            assert_eq!(strike(&d, "1", "nessuna immagine"), n);
        }
        assert_eq!(obs_reason(&d, "1").as_deref(), Some("nessuna immagine"));
        succeeded(&d, "1");
        assert!(obs_reason(&d, "1").is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_marker_of_the_first_version_is_still_read() {
        let d = temp("old");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join(FILE),
            r#"{"version":"0.3.53","engine":"obs","reason":"encoder rotto"}"#,
        )
        .unwrap();
        assert_eq!(obs_reason(&d, "0.3.53").as_deref(), Some("encoder rotto"));
        let _ = std::fs::remove_dir_all(&d);
    }
}

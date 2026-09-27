use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "engine.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Marker {
    version: String,
    engine: String,
    reason: String,
}

pub fn obs_reason(dir: &Path, version: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
    let m: Marker = serde_json::from_str(&text).ok()?;
    (m.version == version && m.engine == crate::ipc::ENGINE_OBS).then_some(m.reason)
}

pub fn remember_obs(dir: &Path, version: &str, reason: &str) {
    let m = Marker {
        version: version.into(),
        engine: crate::ipc::ENGINE_OBS.into(),
        reason: reason.into(),
    };
    if let Ok(text) = serde_json::to_vec_pretty(&m) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(FILE), text);
    }
}

pub fn forget(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_holds_only_for_the_same_version() {
        let d = std::env::temp_dir().join(format!("relay-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(obs_reason(&d, "0.4.0"), None);
        remember_obs(&d, "0.4.0", "nessuna immagine");
        assert_eq!(obs_reason(&d, "0.4.0").as_deref(), Some("nessuna immagine"));
        assert_eq!(obs_reason(&d, "0.4.1"), None);
        forget(&d);
        assert_eq!(obs_reason(&d, "0.4.0"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}

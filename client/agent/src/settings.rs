use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    capture::{Encoder, WindowSel},
    presets,
};

pub const SERVER_URL: &str = "https://relay.gavatech.org";

pub const AUTH_URL: &str = "https://auth.gavatech.org";

fn locked(var: &str, fixed: &str) -> String {
    if cfg!(debug_assertions) {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim().trim_end_matches('/');
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    fixed.to_string()
}

pub fn server_url() -> String {
    locked("RELAY_DEV_SERVER", SERVER_URL)
}

pub fn auth_url() -> String {
    locked("RELAY_DEV_AUTH", AUTH_URL)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowChoice {
    pub exe: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastSpeedtest {
    pub upload_mbps: f64,

    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server_url: String,
    pub auth_url: String,

    pub site_url: String,
    pub window: Option<WindowChoice>,

    pub preset: String,
    pub fps: u32,
    pub bitrate_kbps: u32,

    pub limit_kbps: Option<u32>,

    pub encoder: String,
    pub ffmpeg_path: String,
    pub last_speedtest: Option<LastSpeedtest>,

    pub overlay: bool,

    pub audio_game: bool,

    pub audio_mic: bool,

    pub audio_mic_gain: f32,
}

impl Default for Settings {
    fn default() -> Self {
        let p = presets::preset("medium").expect("preset medium");
        Self {
            server_url: server_url(),
            auth_url: auth_url(),
            site_url: String::new(),
            window: None,
            preset: p.id.into(),
            fps: p.fps,
            bitrate_kbps: p.bitrate_kbps,
            limit_kbps: None,
            encoder: "auto".into(),
            ffmpeg_path: "ffmpeg".into(),
            last_speedtest: None,
            overlay: true,
            audio_game: true,
            audio_mic: false,
            audio_mic_gain: crate::audio::DEFAULT_MIC_GAIN,
        }
    }
}

impl Settings {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Relay").join("settings.json")
    }

    pub fn enforce(&mut self) {
        self.server_url = server_url();
        self.auth_url = auth_url();
        self.site_url = String::new();
    }

    pub fn load(path: &Path) -> Settings {
        std::fs::read(path)
            .ok()
            .and_then(|b| {
                serde_json::from_slice::<Settings>(b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&b))
                    .ok()
            })
            .map(|mut s| {
                s.enforce();
                s
            })
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path
            .parent()
            .context("percorso impostazioni senza cartella")?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(".settings.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn window_sel(&self) -> Option<WindowSel> {
        self.window
            .as_ref()
            .filter(|w| !w.exe.trim().is_empty())
            .map(|w| {
                let exe = w.exe.trim();
                exe.strip_prefix("@monitor:")
                    .and_then(|n| n.parse::<u32>().ok())
                    .map(WindowSel::Monitor)
                    .unwrap_or_else(|| WindowSel::Exe(exe.to_string()))
            })
    }

    pub fn encoder_choice(&self) -> Option<Encoder> {
        Encoder::parse(&self.encoder)
    }

    pub fn effective_limit_kbps(&self) -> Option<u32> {
        Some(
            self.limit_kbps
                .unwrap_or_else(|| match &self.last_speedtest {
                    Some(s) => presets::limit_for(s.upload_mbps),

                    None => (self.bitrate_kbps as f64 * 1.3).ceil() as u32,
                }),
        )
    }

    pub fn apply_preset(&mut self, id: &str) -> bool {
        match presets::preset(id) {
            Some(p) => {
                self.preset = p.id.into();
                self.fps = p.fps;
                self.bitrate_kbps = p.bitrate_kbps;
                true
            }
            None => false,
        }
    }

    pub fn sanitize(&mut self) {
        self.fps = self.fps.clamp(10, 120);
        self.audio_mic_gain = if self.audio_mic_gain.is_finite() {
            self.audio_mic_gain.clamp(0.5, 10.0)
        } else {
            crate::audio::DEFAULT_MIC_GAIN
        };
        self.bitrate_kbps = self.bitrate_kbps.clamp(500, 50_000);
        if let Some(l) = self.limit_kbps {
            self.limit_kbps = Some(l.clamp(200, 1_000_000));
        }
        self.enforce();
        if !matches!(
            self.encoder.as_str(),
            "auto" | "nvenc" | "amf" | "qsv" | "x264"
        ) {
            self.encoder = "auto".into();
        }
        if self.ffmpeg_path.trim().is_empty() {
            self.ffmpeg_path = "ffmpeg".into();
        }
        if !matches!(
            self.preset.as_str(),
            "low" | "medium" | "high" | "max" | "custom"
        ) {
            self.preset = "custom".into();
        }
    }

    pub fn site_origin(&self) -> &str {
        if self.site_url.is_empty() {
            &self.server_url
        } else {
            &self.site_url
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial_files() {
        let dir = tempdir();
        let p = dir.join("s.json");
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings {
            fps: 30,
            window: Some(WindowChoice {
                exe: "game.exe".into(),
                title: "Gioco".into(),
            }),
            ..Settings::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);

        std::fs::write(&p, br#"{"fps":24}"#).unwrap();
        let old = Settings::load(&p);
        assert_eq!(old.fps, 24);
        assert_eq!(old.bitrate_kbps, 4000);
        assert!(old.overlay);

        std::fs::write(&p, b"\xEF\xBB\xBF{\"fps\":25}").unwrap();
        assert_eq!(Settings::load(&p).fps, 25);

        std::fs::write(&p, b"{ non json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn effective_limit_and_presets() {
        let mut s = Settings::default();

        assert_eq!(
            s.effective_limit_kbps(),
            Some((s.bitrate_kbps as f64 * 1.3).ceil() as u32)
        );
        s.last_speedtest = Some(LastSpeedtest {
            upload_mbps: 22.4,
            at: "x".into(),
        });
        assert_eq!(s.effective_limit_kbps(), Some(11_200));
        s.limit_kbps = Some(5_000);
        assert_eq!(s.effective_limit_kbps(), Some(5_000));
        assert!(s.apply_preset("max"));
        assert_eq!((s.fps, s.bitrate_kbps), (60, 9_000));
        assert!(!s.apply_preset("boh"));
    }

    #[test]
    fn sanitize_clamps() {
        let mut s = Settings {
            fps: 999,
            bitrate_kbps: 1,
            encoder: "boh".into(),
            ..Default::default()
        };
        s.limit_kbps = Some(1);
        s.sanitize();
        assert_eq!((s.fps, s.bitrate_kbps, s.limit_kbps), (120, 500, Some(200)));
        assert_eq!(s.encoder, "auto");
    }

    #[test]
    fn server_cannot_be_changed() {
        let p = tempdir().join("locked.json");
        std::fs::write(
            &p,
            br#"{"server_url":"https://evil.example","auth_url":"https://evil.example","site_url":"https://evil.example"}"#,
        )
        .unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.server_url, server_url());
        assert_eq!(s.auth_url, auth_url());
        assert_eq!(s.site_url, "");
        let mut t = Settings {
            server_url: "https://evil.example".into(),
            ..Default::default()
        };
        t.sanitize();
        assert_eq!(t.server_url, server_url());

        if std::env::var("RELAY_DEV_SERVER").is_err() {
            assert_eq!(s.server_url, "https://relay.gavatech.org");
        }
    }

    #[test]
    fn window_sel_needs_an_exe() {
        let mut s = Settings::default();
        assert!(s.window_sel().is_none());
        s.window = Some(WindowChoice {
            exe: "  ".into(),
            title: "t".into(),
        });
        assert!(s.window_sel().is_none());
        s.window = Some(WindowChoice {
            exe: "game.exe".into(),
            title: String::new(),
        });
        assert!(matches!(s.window_sel(), Some(WindowSel::Exe(e)) if e == "game.exe"));
        s.window = Some(WindowChoice {
            exe: "@monitor:0".into(),
            title: "Monitor principale".into(),
        });
        assert!(matches!(s.window_sel(), Some(WindowSel::Monitor(0))));
    }

    fn tempdir() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d =
            std::env::temp_dir().join(format!("relay-settings-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

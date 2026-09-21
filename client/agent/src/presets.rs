use serde::{Deserialize, Serialize};

pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

pub const PRESETS: [Preset; 4] = [
    Preset {
        id: "low",
        label: "Leggera",
        fps: 30,
        bitrate_kbps: 2_500,
    },
    Preset {
        id: "medium",
        label: "Standard",
        fps: 60,
        bitrate_kbps: 4_000,
    },
    Preset {
        id: "high",
        label: "Alta",
        fps: 60,
        bitrate_kbps: 6_000,
    },
    Preset {
        id: "max",
        label: "Massima",
        fps: 60,
        bitrate_kbps: 9_000,
    },
];

pub const UPLOAD_SHARE: f64 = 0.5;

pub const HEADROOM: f64 = 1.15;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetFit {
    pub id: String,
    pub label: String,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub fits: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeedResult {
    pub upload_mbps: f64,
    pub recommended: String,

    pub limit_kbps: u32,
    pub presets: Vec<PresetFit>,
}

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

pub fn limit_for(upload_mbps: f64) -> u32 {
    (upload_mbps * 1000.0 * UPLOAD_SHARE).round().max(0.0) as u32
}

pub fn recommend(upload_mbps: f64) -> SpeedResult {
    let limit = limit_for(upload_mbps);
    let presets: Vec<PresetFit> = PRESETS
        .iter()
        .map(|p| PresetFit {
            id: p.id.into(),
            label: p.label.into(),
            fps: p.fps,
            bitrate_kbps: p.bitrate_kbps,
            fits: (p.bitrate_kbps as f64 * HEADROOM) <= limit as f64,
        })
        .collect();

    let recommended = presets
        .iter()
        .rev()
        .find(|p| p.fits)
        .map(|p| p.id.clone())
        .unwrap_or_else(|| "low".into());
    SpeedResult {
        upload_mbps,
        recommended,
        limit_kbps: limit,
        presets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_heaviest_that_fits_in_half_the_upload() {
        assert_eq!(recommend(30.0).recommended, "max");
        assert_eq!(recommend(22.0).recommended, "max");
        assert_eq!(recommend(20.0).recommended, "high");
        assert_eq!(recommend(15.0).recommended, "high");
        assert_eq!(recommend(10.0).recommended, "medium");
        assert_eq!(recommend(6.0).recommended, "low");
    }

    #[test]
    fn slow_connection_recommends_lightest_but_flags_it() {
        let r = recommend(3.0);
        assert_eq!(r.recommended, "low");
        assert!(r.presets.iter().all(|p| !p.fits));
        assert_eq!(r.limit_kbps, 1500);
    }

    #[test]
    fn limit_is_half_of_upload() {
        assert_eq!(limit_for(22.4), 11_200);
        assert_eq!(limit_for(0.0), 0);
    }

    #[test]
    fn lookup() {
        assert_eq!(preset("high").unwrap().bitrate_kbps, 6_000);
        assert!(preset("boh").is_none());
    }
}

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ts::CLOCK;

pub const AUDIO_RATE: u32 = 48_000;
pub const PTS_BASE: u64 = 126_000;

pub fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn instant_at(unix_secs: f64) -> Instant {
    let now = Instant::now();
    let delta = unix_secs - unix_now();
    if delta >= 0.0 {
        now + Duration::from_secs_f64(delta)
    } else {
        now.checked_sub(Duration::from_secs_f64(-delta))
            .unwrap_or(now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeline {
    pub fps: u32,
    pub segment_secs: u32,
    pub origin_unix: f64,
    pub start_segment: u64,
}

impl Timeline {
    pub fn frames_per_segment(&self) -> u64 {
        self.fps as u64 * self.segment_secs as u64
    }

    pub fn frame_unix(&self, k: u64) -> f64 {
        self.origin_unix + k as f64 / self.fps as f64
    }

    pub fn frame_offset(&self, k: u64) -> Duration {
        Duration::from_nanos(k * 1_000_000_000 / self.fps as u64)
    }

    pub fn first_frame(&self, now_unix: f64) -> u64 {
        if now_unix <= self.origin_unix {
            return 0;
        }
        let late = ((now_unix - self.origin_unix) * self.fps as f64).floor() as u64;
        late / self.frames_per_segment() * self.frames_per_segment()
    }

    fn base(&self) -> u64 {
        PTS_BASE + self.start_segment * self.segment_secs as u64 * CLOCK
    }

    pub fn video_pts(&self, k: u64) -> u64 {
        self.base() + k * CLOCK / self.fps as u64
    }

    pub fn frame_ticks(&self) -> u64 {
        CLOCK / self.fps as u64
    }

    pub fn sample_for_frame(&self, k: u64) -> u64 {
        (k * AUDIO_RATE as u64).div_ceil(self.fps as u64)
    }

    pub fn audio_pts(&self, sample: u64) -> u64 {
        self.base() + sample * CLOCK / AUDIO_RATE as u64
    }

    pub fn sample_time_100ns(&self, k: u64) -> i64 {
        (k as u128 * 10_000_000 / self.fps as u128) as i64
    }

    pub fn frame_from_100ns(&self, t: i64) -> u64 {
        ((t.max(0) as f64) * self.fps as f64 / 10_000_000.0).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(fps: u32) -> Timeline {
        Timeline {
            fps,
            segment_secs: 4,
            origin_unix: 1_000.0,
            start_segment: 0,
        }
    }

    #[test]
    fn segments_line_up_with_the_common_clock() {
        let t = line(60);
        assert_eq!(t.frames_per_segment(), 240);
        assert_eq!(t.video_pts(0), PTS_BASE);
        assert_eq!(t.video_pts(240), PTS_BASE + 4 * CLOCK);
        let resumed = Timeline {
            start_segment: 12,
            ..t
        };
        assert_eq!(resumed.video_pts(0), PTS_BASE + 48 * CLOCK);
        assert_eq!(resumed.audio_pts(0), resumed.video_pts(0));
        assert_eq!(t.audio_pts(48_000), t.video_pts(60));
    }

    #[test]
    fn a_late_start_goes_back_to_the_start_of_its_segment() {
        let t = line(60);
        assert_eq!(t.first_frame(999.0), 0);
        assert_eq!(t.first_frame(1_000.0), 0);
        assert_eq!(t.first_frame(1_003.9), 0);
        assert_eq!(t.first_frame(1_004.2), 240);
        assert_eq!(t.first_frame(1_009.0), 480);
    }

    #[test]
    fn media_foundation_times_map_back_to_frames() {
        for fps in [24, 30, 60, 144] {
            let t = line(fps);
            for k in [0u64, 1, 2, 59, 60, 61, 10_007, 1_000_000] {
                assert_eq!(
                    t.frame_from_100ns(t.sample_time_100ns(k)),
                    k,
                    "fps {fps} k {k}"
                );
            }
        }
    }

    #[test]
    fn audio_starts_with_the_first_frame() {
        let t = line(60);
        assert_eq!(t.sample_for_frame(240), 192_000);
        assert_eq!(t.audio_pts(t.sample_for_frame(240)), t.video_pts(240));
        let odd = line(144);
        assert!(odd.audio_pts(odd.sample_for_frame(7)) >= odd.video_pts(7));
    }
}

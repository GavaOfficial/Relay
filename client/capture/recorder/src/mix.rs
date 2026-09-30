use std::collections::VecDeque;

pub const CHANNELS: usize = 2;

pub fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}

pub fn mix(frames: usize, inputs: &mut [(&mut VecDeque<f32>, f32)], out: &mut Vec<i16>) {
    let mut acc = vec![0f32; frames * CHANNELS];
    for (ring, gain) in inputs.iter_mut() {
        let take = acc.len().min(ring.len());
        for (a, s) in acc.iter_mut().zip(ring.drain(..take)) {
            *a += s * *gain;
        }
    }
    out.extend(acc.iter().map(|v| (soft_limit(*v) * 32767.0) as i16));
}

pub struct Jitter {
    delay_frames: usize,
    max_frames: usize,
    starved: bool,
}

impl Jitter {
    pub fn new(delay_frames: usize, max_frames: usize) -> Self {
        Self {
            delay_frames,
            max_frames,
            starved: false,
        }
    }

    pub fn prepare(&mut self, ring: &mut VecDeque<f32>, frames: usize) {
        if self.starved && !ring.is_empty() {
            for _ in 0..self.delay_frames * CHANNELS {
                ring.push_front(0.0);
            }
            self.starved = false;
        }
        let level = ring.len() / CHANNELS;
        if level > frames + self.max_frames {
            let drop = (level - frames - self.delay_frames) * CHANNELS;
            ring.drain(..drop);
        }
        if ring.len() / CHANNELS < frames {
            self.starved = true;
        }
    }
}

pub fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_limit_keeps_quiet_sound_and_never_exceeds_one() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.7), -0.7);
        for x in [0.9, 1.0, 1.5, 4.0, 100.0] {
            let y = soft_limit(x);
            assert!(y > 0.8 && y <= 1.0, "{x} -> {y}");
            assert_eq!(soft_limit(-x), -y);
        }
    }

    #[test]
    fn sources_are_mixed_with_their_gain_and_silence_fills_the_gaps() {
        let mut game: VecDeque<f32> = std::iter::repeat_n(0.2f32, 8).collect();
        let mut mic: VecDeque<f32> = std::iter::repeat_n(0.1f32, 4).collect();
        let mut out = Vec::new();
        mix(6, &mut [(&mut game, 1.0), (&mut mic, 3.0)], &mut out);
        assert_eq!(out.len(), 12);
        let v: Vec<f32> = out.iter().map(|s| *s as f32 / 32767.0).collect();
        assert!((v[0] - 0.5).abs() < 0.001);
        assert!((v[5] - 0.2).abs() < 0.001);
        assert_eq!(v[10], 0.0);
        assert!(game.is_empty() && mic.is_empty());
    }

    #[test]
    fn sound_after_a_silence_keeps_its_place_on_the_clock() {
        let mut j = Jitter::new(100, 400);
        let mut ring: VecDeque<f32> = VecDeque::new();
        j.prepare(&mut ring, 50);
        let mut out = Vec::new();
        mix(50, &mut [(&mut ring, 1.0)], &mut out);
        assert!(out.iter().all(|s| *s == 0));
        ring.extend(std::iter::repeat_n(0.5f32, 60 * CHANNELS));
        j.prepare(&mut ring, 50);
        assert_eq!(ring.len(), 160 * CHANNELS);
        out.clear();
        mix(50, &mut [(&mut ring, 1.0)], &mut out);
        assert!(
            out.iter().all(|s| *s == 0),
            "il suono nuovo va dopo il ritardo"
        );
    }

    #[test]
    fn steady_sound_is_neither_padded_nor_cut() {
        let mut j = Jitter::new(100, 400);
        let mut ring: VecDeque<f32> = std::iter::repeat_n(0.25f32, 150 * CHANNELS).collect();
        for _ in 0..20 {
            j.prepare(&mut ring, 50);
            let mut out = Vec::new();
            mix(50, &mut [(&mut ring, 1.0)], &mut out);
            assert!(out.iter().all(|s| *s > 0));
            ring.extend(std::iter::repeat_n(0.25f32, 50 * CHANNELS));
        }
        assert_eq!(ring.len(), 150 * CHANNELS);
    }

    #[test]
    fn a_runaway_backlog_goes_back_to_the_delay() {
        let mut j = Jitter::new(100, 400);
        let mut ring: VecDeque<f32> = std::iter::repeat_n(0.25f32, 1000 * CHANNELS).collect();
        j.prepare(&mut ring, 50);
        assert_eq!(ring.len(), 150 * CHANNELS);
    }
}

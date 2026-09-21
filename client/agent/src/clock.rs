use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn local_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn local_now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

const KEEP: usize = 8;

#[derive(Default)]
pub struct Clock {
    samples: Mutex<VecDeque<(u64, f64)>>,
}

impl Clock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, t0: u64, t1: u64, server_ms: u64) {
        let rtt = t1.saturating_sub(t0);
        let offset = server_ms as f64 - (t0 as f64 + t1 as f64) / 2.0;
        let mut s = self.samples.lock().unwrap();
        if s.len() == KEEP {
            s.pop_front();
        }
        s.push_back((rtt, offset));
    }

    pub fn sample_count(&self) -> usize {
        self.samples.lock().unwrap().len()
    }

    pub fn best(&self) -> Option<(f64, u64)> {
        self.samples
            .lock()
            .unwrap()
            .iter()
            .min_by_key(|(rtt, _)| *rtt)
            .map(|&(rtt, off)| (off, rtt))
    }

    pub fn server_ms_to_local_secs(&self, server_ms: u64) -> Option<f64> {
        let (off, _) = self.best()?;
        Some((server_ms as f64 - off) / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_lowest_rtt_sample() {
        let c = Clock::new();

        c.record(10_000, 10_200, 11_180);

        c.record(20_000, 20_020, 21_010);
        let (off, rtt) = c.best().unwrap();
        assert_eq!(rtt, 20);
        assert!((off - 1000.0).abs() < 0.5, "{off}");
    }

    #[test]
    fn converts_server_time_to_local() {
        let c = Clock::new();
        c.record(20_000, 20_020, 21_010);
        let local = c.server_ms_to_local_secs(31_000).unwrap();
        assert!((local - 30.0).abs() < 0.001, "{local}");
    }

    #[test]
    fn empty_has_no_estimate() {
        assert!(Clock::new().server_ms_to_local_secs(1).is_none());
    }
}

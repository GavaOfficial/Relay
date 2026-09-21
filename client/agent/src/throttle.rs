use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct Throttle {
    bytes_per_sec: u64,
    next: Mutex<Instant>,
}

impl Throttle {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec: bytes_per_sec.max(1),
            next: Mutex::new(Instant::now()),
        }
    }

    pub fn bytes_per_sec(&self) -> u64 {
        self.bytes_per_sec
    }

    pub async fn acquire(&self, n: usize) {
        let start = {
            let mut next = self.next.lock().unwrap();
            let start = (*next).max(Instant::now());
            *next = start + Duration::from_secs_f64(n as f64 / self.bytes_per_sec as f64);
            start
        };
        tokio::time::sleep_until(start.into()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paces_to_rate() {
        let t = Throttle::new(100_000);
        let t0 = Instant::now();
        for _ in 0..3 {
            t.acquire(10_000).await;
        }
        let e = t0.elapsed();
        assert!(e >= Duration::from_millis(180), "{e:?}");
        assert!(e < Duration::from_millis(400), "{e:?}");
    }
}

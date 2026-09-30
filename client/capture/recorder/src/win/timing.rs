use std::{cell::RefCell, fs::File, io::Write, path::Path, sync::mpsc, thread, time::Instant};

#[derive(Clone, Copy)]
pub enum Stage { Acquire, Copy, Flush, Release, Convert, Input, Output, Allocate, Poll }
const NAMES: [&str; 9] = ["AcquireSync", "CopyResource", "Flush", "ReleaseSync", "conversion", "encoder.ProcessInput", "encoder.ProcessOutput", "encoder.allocate", "encoder.poll"];
#[derive(Clone, Copy, Default)]
struct Stats { calls: u64, total_us: u64, max_us: u64, over_16ms: u64 }
struct Logger { origin: Instant, last: Instant, stats: [Stats; 9], tx: mpsc::SyncSender<(f64, [Stats; 9])> }
thread_local! { static LOGGER: RefCell<Option<Logger>> = const { RefCell::new(None) }; }
pub struct Session(Option<thread::JoinHandle<()>>);
impl Session {
    pub fn start(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut file = File::create(dir.join("capture-timings.jsonl"))?;
        let (tx, rx) = mpsc::sync_channel::<(f64, [Stats; 9])>(8);
        let worker = thread::spawn(move || {
            for (elapsed, stats) in rx {
                for (name, s) in NAMES.iter().zip(stats) {
                    if s.calls == 0 { continue; }
                    if writeln!(file, "{{\"seconds\":{elapsed:.3},\"stage\":\"{name}\",\"calls\":{},\"mean_us\":{:.2},\"max_us\":{},\"over_16ms\":{}}}", s.calls, s.total_us as f64 / s.calls as f64, s.max_us, s.over_16ms).is_err() { return; }
                }
            }
        });
        LOGGER.with(|l| *l.borrow_mut() = Some(Logger { origin: Instant::now(), last: Instant::now(), stats: [Stats::default(); 9], tx }));
        Ok(Self(Some(worker)))
    }
}
pub fn tick() {
    LOGGER.with(|l| {
        let mut l = l.borrow_mut();
        let Some(l) = l.as_mut() else { return; };
        if l.last.elapsed().as_secs() == 0 { return; }
        if l.tx.try_send((l.origin.elapsed().as_secs_f64(), l.stats)).is_ok() {
            l.stats = [Stats::default(); 9]; l.last = Instant::now();
        }
    });
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(l) = LOGGER.with(|l| l.borrow_mut().take()) {
            let _ = l.tx.send((l.origin.elapsed().as_secs_f64(), l.stats));
        }
        if let Some(worker) = self.0.take() { let _ = worker.join(); }
    }
}
pub struct Span(Stage, Instant);
pub fn span(stage: Stage) -> Span { Span(stage, Instant::now()) }
impl Drop for Span {
    fn drop(&mut self) {
        let us = self.1.elapsed().as_micros().min(u64::MAX as u128) as u64;
        LOGGER.with(|l| {
            if let Some(l) = l.borrow_mut().as_mut() {
                let s = &mut l.stats[self.0 as usize];
                s.calls += 1; s.total_us += us; s.max_us = s.max_us.max(us); s.over_16ms += u64::from(us > 16_667);
            }
        });
    }
}

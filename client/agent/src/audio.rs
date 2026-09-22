use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::clock::local_now_secs;

pub const RATE: u32 = 48_000;

const MAX_BUFFER_FRAMES: usize = RATE as usize * 3;

pub const DEFAULT_MIC_GAIN: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioChoice {
    pub game: bool,
    pub mic: bool,

    pub mic_gain: f32,
}

impl Default for AudioChoice {
    fn default() -> Self {
        AudioChoice {
            game: false,
            mic: false,
            mic_gain: DEFAULT_MIC_GAIN,
        }
    }
}

impl AudioChoice {
    pub fn any(self) -> bool {
        self.game || self.mic
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioReport {
    pub label: Option<String>,
    pub issue: Option<String>,
}

type Ring = Arc<Mutex<std::collections::VecDeque<f32>>>;

struct Source {
    name: &'static str,

    gain: f32,
    ring: Ring,

    error: Arc<Mutex<Option<String>>>,
}

impl Source {
    fn failed(name: &'static str, why: String) -> Self {
        Source {
            name,
            gain: 1.0,
            ring: Ring::default(),
            error: Arc::new(Mutex::new(Some(why))),
        }
    }
    fn ok(&self) -> bool {
        self.error.lock().unwrap().is_none()
    }
}

pub struct AudioEngine {
    game: Option<Source>,
    mic: Option<Source>,
    stop: Arc<AtomicBool>,
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl AudioEngine {
    pub fn start(choice: AudioChoice, game_pid: Option<u32>) -> AudioEngine {
        let stop = Arc::new(AtomicBool::new(false));
        let game = choice.game.then(|| match game_pid {
            Some(pid) => spawn_capture("gioco", Kind::Game(pid), &stop),
            None => Source::failed(
                "gioco",
                "audio del gioco: scegli la finestra del gioco (non uno schermo intero)".into(),
            ),
        });
        let mic = choice.mic.then(|| {
            let mut s = spawn_capture("microfono", Kind::Mic, &stop);
            s.gain = choice.mic_gain.clamp(0.1, 10.0);
            s
        });
        AudioEngine { game, mic, stop }
    }

    pub fn has_audio(&self) -> bool {
        [&self.game, &self.mic]
            .iter()
            .any(|s| s.as_ref().is_some_and(Source::ok))
    }

    pub fn report(&self) -> AudioReport {
        let mut ok = Vec::new();
        let mut issues = Vec::new();
        for s in [&self.game, &self.mic].into_iter().flatten() {
            match s.error.lock().unwrap().as_ref() {
                None => ok.push(s.name),
                Some(e) => issues.push(e.clone()),
            }
        }
        AudioReport {
            label: (!ok.is_empty()).then(|| ok.join(" + ")),
            issue: (!issues.is_empty()).then(|| issues.join("; ")),
        }
    }

    pub fn attach(&self, origin: f64) -> std::io::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let rings: Vec<(Ring, f32)> = [&self.game, &self.mic]
            .into_iter()
            .flatten()
            .filter(|s| s.ok())
            .map(|s| (s.ring.clone(), s.gain))
            .collect();
        let stop = self.stop.clone();
        thread::Builder::new()
            .name("audio-sink".into())
            .spawn(move || {
                let t0 = Instant::now();
                let stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(_)
                            if t0.elapsed() < Duration::from_secs(30)
                                && !stop.load(Ordering::Relaxed) =>
                        {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(_) => return,
                    }
                };
                let _ = stream.set_nodelay(true);
                let _ = stream.set_nonblocking(false);
                run_sink(stream, origin, &rings, &stop);
            })?;
        Ok(port)
    }
}

fn run_sink(mut s: TcpStream, origin: f64, rings: &[(Ring, f32)], stop: &AtomicBool) {
    for (r, _) in rings {
        r.lock().unwrap().clear();
    }
    let mut written: u64 = 0;
    let mut bytes: Vec<u8> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let due = ((local_now_secs() - origin) * RATE as f64).floor();
        if due < 0.0 || due as u64 <= written {
            thread::sleep(Duration::from_millis(5));
            continue;
        }

        let n = ((due as u64 - written) as usize).min(RATE as usize);
        let mut acc = vec![0f32; n * 2];
        for (r, gain) in rings {
            let mut q = r.lock().unwrap();
            let take = (n * 2).min(q.len());
            for a in acc.iter_mut().take(take) {
                *a += q.pop_front().unwrap_or(0.0) * gain;
            }
        }
        bytes.clear();
        for v in &acc {
            bytes.extend_from_slice(&((soft_limit(*v) * 32767.0) as i16).to_le_bytes());
        }
        if s.write_all(&bytes).is_err() {
            return;
        }
        written += n as u64;
        if n < RATE as usize / 2 {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}

enum Kind {
    Game(u32),
    Mic,
}

fn spawn_capture(name: &'static str, kind: Kind, stop: &Arc<AtomicBool>) -> Source {
    let ring: Ring = Ring::default();
    let error = Arc::new(Mutex::new(None));
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let (r2, e2, s2) = (ring.clone(), error.clone(), stop.clone());
    let spawned = thread::Builder::new()
        .name(format!("audio-{name}"))
        .spawn(move || {
            let res = capture_loop(kind, &r2, &s2, &tx);
            if let Err(e) = res {
                *e2.lock().unwrap() = Some(format!(
                    "audio {}: {e}",
                    if matches!(name, "gioco") {
                        "del gioco"
                    } else {
                        "del microfono"
                    }
                ));
                let _ = tx.send(Err(e));
            }
        });
    if spawned.is_err() {
        return Source::failed(
            name,
            format!("audio {name}: impossibile avviare la cattura"),
        );
    }

    match rx.recv_timeout(Duration::from_secs(4)) {
        Ok(Ok(())) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
    }
    Source {
        name,
        gain: 1.0,
        ring,
        error,
    }
}

#[cfg(windows)]
fn capture_loop(
    kind: Kind,
    ring: &Ring,
    stop: &AtomicBool,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    use wasapi::{
        initialize_mta, AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode,
        WaveFormat,
    };

    let _ = initialize_mta().ok();
    let format = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
    let mut client = match kind {
        Kind::Game(pid) => {
            AudioClient::new_application_loopback_client(pid, true).map_err(|e| {
                format!("non disponibile ({e}); serve Windows 10 versione 2004 o successiva")
            })?
        }
        Kind::Mic => DeviceEnumerator::new()
            .and_then(|e| e.get_default_device(&Direction::Capture))
            .and_then(|d| d.get_iaudioclient())
            .map_err(|e| format!("nessun microfono disponibile ({e})"))?,
    };
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| format!("cattura non avviabile ({e})"))?;
    let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
    let capture = client.get_audiocaptureclient().map_err(|e| e.to_string())?;
    client.start_stream().map_err(|e| e.to_string())?;
    let _ = ready.send(Ok(()));

    let mut raw: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        let _ = event.wait_for_event(200);
        loop {
            let frames = capture
                .get_next_packet_size()
                .map_err(|e| e.to_string())?
                .unwrap_or(0);
            if frames == 0 {
                break;
            }
            capture
                .read_from_device_to_deque(&mut raw)
                .map_err(|e| e.to_string())?;
        }
        if raw.is_empty() {
            continue;
        }
        let mut q = ring.lock().unwrap();
        while raw.len() >= 4 {
            let b = [
                raw.pop_front().unwrap(),
                raw.pop_front().unwrap(),
                raw.pop_front().unwrap(),
                raw.pop_front().unwrap(),
            ];
            q.push_back(f32::from_le_bytes(b));
        }
        trim_ring(&mut q, MAX_BUFFER_FRAMES);
    }
    let _ = client.stop_stream();
    Ok(())
}

fn trim_ring(q: &mut std::collections::VecDeque<f32>, max_frames: usize) {
    let max = max_frames * 2;
    if q.len() > max {
        let drop = q.len() - max;
        q.drain(..drop);
    }
}

#[cfg(not(windows))]
fn capture_loop(
    _kind: Kind,
    _ring: &Ring,
    _stop: &AtomicBool,
    _ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    Err("non supportato su questo sistema".into())
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

        assert!(soft_limit(0.95) < soft_limit(1.2));
    }

    #[test]
    fn the_volume_of_a_source_is_applied_when_mixing() {
        let ring: Ring = Ring::default();
        ring.lock()
            .unwrap()
            .extend(std::iter::repeat_n(0.1f32, RATE as usize * 2));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let (r2, s2) = (ring.clone(), stop.clone());

        let origin = local_now_secs() - 0.5;
        let t = thread::spawn(move || {
            let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            run_sink(s, origin, &[(r2, 3.0)], &s2);
        });
        let (mut c, _) = l.accept().unwrap();
        thread::sleep(Duration::from_millis(50));
        ring.lock()
            .unwrap()
            .extend(std::iter::repeat_n(0.1f32, RATE as usize * 2));
        c.set_read_timeout(Some(Duration::from_millis(1500)))
            .unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 16384];
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(800) {
            if let Ok(n) = std::io::Read::read(&mut c, &mut buf) {
                got.extend_from_slice(&buf[..n]);
            }
        }
        stop.store(true, Ordering::Relaxed);
        drop(c);
        let _ = t.join();
        let peak = got
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]).abs())
            .max()
            .unwrap_or(0) as f32
            / 32767.0;
        assert!(
            (0.27..=0.33).contains(&peak),
            "picco {peak} invece di circa 0,3"
        );
    }

    #[test]
    fn a_burst_of_buffered_audio_survives_the_ring_cap() {
        let mut q: std::collections::VecDeque<f32> =
            std::iter::repeat_n(0.2f32, (RATE as usize * 3 / 2) * 2).collect();
        trim_ring(&mut q, MAX_BUFFER_FRAMES);
        let secs = q.len() as f64 / 2.0 / RATE as f64;
        assert!(
            secs > 1.4,
            "raffica di 1,5 s ridotta a {secs:.2} s: il margine e' troppo piccolo per assorbire un rallentamento della cattura"
        );
    }

    #[test]
    fn the_ring_cap_still_bounds_memory() {
        let mut q: std::collections::VecDeque<f32> =
            std::iter::repeat_n(0.2f32, (RATE as usize * 10) * 2).collect();
        trim_ring(&mut q, MAX_BUFFER_FRAMES);
        assert_eq!(q.len(), MAX_BUFFER_FRAMES * 2);
    }

    #[test]
    fn engine_without_sources_has_no_audio() {
        let e = AudioEngine::start(AudioChoice::default(), None);
        assert!(!e.has_audio());
        assert_eq!(e.report(), AudioReport::default());
    }

    #[test]
    fn game_without_pid_is_reported_not_fatal() {
        let e = AudioEngine::start(
            AudioChoice {
                game: true,
                ..Default::default()
            },
            None,
        );
        assert!(!e.has_audio());
        let r = e.report();
        assert!(r.label.is_none());
        assert!(r.issue.unwrap().contains("scegli la finestra"));
    }

    #[test]
    fn sink_follows_the_clock_and_pads_silence() {
        let ring: Ring = Ring::default();
        ring.lock().unwrap().extend(std::iter::repeat_n(0.5f32, 0));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let (r2, s2) = (ring.clone(), stop.clone());
        let origin = local_now_secs();
        let t = thread::spawn(move || {
            let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            run_sink(s, origin, &[(r2, 1.0)], &s2);
        });
        let (mut c, _) = l.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_millis(1500)))
            .unwrap();
        let mut total = 0usize;
        let mut buf = [0u8; 8192];
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1000) {
            if let Ok(n) = std::io::Read::read(&mut c, &mut buf) {
                total += n;
            }
        }
        stop.store(true, Ordering::Relaxed);
        drop(c);
        let _ = t.join();
        let secs = total as f64 / (RATE as f64 * 4.0);
        assert!(
            (0.8..1.3).contains(&secs),
            "{secs} s di audio in 1 s di orologio"
        );
    }
}

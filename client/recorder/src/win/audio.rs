use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use wasapi::{
    initialize_mta, AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat,
};

use super::aac::AacEncoder;
use crate::{
    aac::to_adts,
    clock::{Timeline, AUDIO_RATE},
    hls::AudioPacket,
    mix::{mix, to_bytes, Jitter, CHANNELS},
};

const DELAY_FRAMES: usize = AUDIO_RATE as usize * 6 / 100;
const MAX_BACKLOG_FRAMES: usize = AUDIO_RATE as usize / 5;
const MAX_RING_FRAMES: usize = AUDIO_RATE as usize * 3;

type Ring = Arc<Mutex<VecDeque<f32>>>;

#[derive(Clone, Copy)]
enum Kind {
    Process(u32),
    System,
    Mic,
}

struct Input {
    ring: Ring,
    gain: f32,
}

pub struct Inputs {
    inputs: Vec<Input>,
    stop: Arc<AtomicBool>,
}

impl Drop for Inputs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Inputs {
    pub fn start(
        game_pid: Option<u32>,
        whole_system: bool,
        mic_gain: Option<f32>,
        warn: &dyn Fn(String),
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut inputs = Vec::new();
        if let Some(pid) = game_pid {
            match spawn(Kind::Process(pid), &stop) {
                Ok(ring) => inputs.push(Input { ring, gain: 1.0 }),
                Err(e) => {
                    warn(format!(
                        "audio del solo gioco non disponibile ({e}): registro l'audio di tutto il sistema"
                    ));
                    match spawn(Kind::System, &stop) {
                        Ok(ring) => inputs.push(Input { ring, gain: 1.0 }),
                        Err(e) => warn(format!("audio del sistema non disponibile ({e})")),
                    }
                }
            }
        } else if whole_system {
            match spawn(Kind::System, &stop) {
                Ok(ring) => inputs.push(Input { ring, gain: 1.0 }),
                Err(e) => warn(format!("audio del sistema non disponibile ({e})")),
            }
        }
        if let Some(gain) = mic_gain {
            match spawn(Kind::Mic, &stop) {
                Ok(ring) => inputs.push(Input {
                    ring,
                    gain: gain.clamp(0.1, 10.0),
                }),
                Err(e) => warn(format!("microfono non disponibile ({e})")),
            }
        }
        Self { inputs, stop }
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    pub fn run(
        self,
        timeline: Timeline,
        origin: Instant,
        first_sample: u64,
        packets: mpsc::Sender<AudioPacket>,
        stop: Arc<AtomicBool>,
        warn: impl Fn(String) + Send + 'static,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("relay-audio-mix".into())
            .spawn(move || {
                super::com_init();
                let mut encoder = match AacEncoder::open() {
                    Ok(e) => e,
                    Err(e) => {
                        warn(format!("audio non registrato: {e:#}"));
                        return;
                    }
                };
                for input in &self.inputs {
                    input.ring.lock().unwrap().clear();
                }
                let mut written = first_sample;
                let mut pcm: Vec<i16> = Vec::new();
                let mut frames = Vec::new();
                let mut jitter: Vec<Jitter> = self
                    .inputs
                    .iter()
                    .map(|_| Jitter::new(DELAY_FRAMES, MAX_BACKLOG_FRAMES))
                    .collect();
                let delay = DELAY_FRAMES as f64 / AUDIO_RATE as f64;
                while !stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(10));
                    let elapsed = Instant::now()
                        .saturating_duration_since(origin)
                        .as_secs_f64()
                        - delay;
                    let due = (elapsed.max(0.0) * AUDIO_RATE as f64).floor() as u64;
                    if due <= written {
                        continue;
                    }
                    let n = ((due - written) as usize).min(AUDIO_RATE as usize);
                    let mut guards: Vec<_> = self
                        .inputs
                        .iter()
                        .map(|i| (i.ring.lock().unwrap(), i.gain))
                        .collect();
                    for ((ring, _), j) in guards.iter_mut().zip(jitter.iter_mut()) {
                        j.prepare(ring, n);
                    }
                    let mut refs: Vec<(&mut VecDeque<f32>, f32)> = guards
                        .iter_mut()
                        .map(|(g, gain)| (&mut **g, *gain))
                        .collect();
                    pcm.clear();
                    mix(n, &mut refs, &mut pcm);
                    drop(refs);
                    drop(guards);
                    frames.clear();
                    if let Err(e) = encoder.encode(&to_bytes(&pcm), written, &mut frames) {
                        warn(format!("audio interrotto: {e:#}"));
                        return;
                    }
                    written += n as u64;
                    for f in frames.drain(..) {
                        let packet = AudioPacket {
                            pts: timeline.audio_pts(f.sample),
                            data: to_adts(&f.data, AUDIO_RATE, CHANNELS as u8),
                        };
                        if packets.send(packet).is_err() {
                            return;
                        }
                    }
                }
                drop(self);
            })
            .expect("thread dell'audio")
    }
}

fn spawn(kind: Kind, stop: &Arc<AtomicBool>) -> Result<Ring, String> {
    let ring: Ring = Ring::default();
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let (r, s) = (ring.clone(), stop.clone());
    thread::Builder::new()
        .name("relay-audio".into())
        .spawn(move || {
            if let Err(e) = capture(kind, &r, &s, &tx) {
                let _ = tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(4)) {
        Ok(Ok(())) => Ok(ring),
        Ok(Err(e)) => Err(e),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(ring),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("cattura interrotta".into()),
    }
}

fn capture(
    kind: Kind,
    ring: &Ring,
    stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let _ = initialize_mta().ok();
    let format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        AUDIO_RATE as usize,
        CHANNELS,
        None,
    );
    let mut client = match kind {
        Kind::Process(pid) => AudioClient::new_application_loopback_client(pid, true)
            .map_err(|e| format!("{e}; serve Windows 10 versione 2004 o successiva"))?,
        Kind::System => DeviceEnumerator::new()
            .and_then(|e| e.get_default_device(&Direction::Render))
            .and_then(|d| d.get_iaudioclient())
            .map_err(|e| format!("nessuna uscita audio ({e})"))?,
        Kind::Mic => DeviceEnumerator::new()
            .and_then(|e| e.get_default_device(&Direction::Capture))
            .and_then(|d| d.get_iaudioclient())
            .map_err(|e| format!("nessun microfono ({e})"))?,
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

    let mut raw: VecDeque<u8> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        let _ = event.wait_for_event(20);
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
        if raw.len() < 4 {
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
        let max = MAX_RING_FRAMES * CHANNELS;
        if q.len() > max {
            let extra = q.len() - max;
            q.drain(..extra - extra % CHANNELS);
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

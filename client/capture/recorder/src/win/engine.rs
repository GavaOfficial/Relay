use super::timing::{self, Stage};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Graphics::Direct3D11::ID3D11Texture2D,
    Media::{timeBeginPeriod, timeEndPeriod},
};

use super::{
    audio::{GameAudio, Inputs},
    capture,
    convert::Converter,
    device::{self, Gpu},
    encoder::{self, Candidate, Encoded, Kind, H264},
    source::Source,
    window::{self, Found},
    Unsync,
};
use crate::{
    clock::{self, Timeline},
    h264::AccessUnits,
    hls::{self, AudioPacket, SegmentWriter, VideoPacket},
    layout, Config, EncoderPref, Event, EventSink, FrameCheck, Target,
};

const WINDOW_STALE: Duration = Duration::from_millis(2500);
const MAINTAIN_EVERY: Duration = Duration::from_millis(250);
const MAX_BEHIND: Duration = Duration::from_secs(2);

enum Cmd {
    Start,
    Abort,
    Fit,
    Stop,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum WindowState {
    #[default]
    Unknown,
    Visible,
    Minimized,
    Missing,
}

#[derive(Default)]
struct Health {
    frames: u64,
    window: WindowState,
    error: Option<String>,
    finished: bool,
}

pub struct Prepared {
    tx: mpsc::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    health: Arc<Mutex<Health>>,
}

pub struct Recorder {
    tx: mpsc::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    health: Arc<Mutex<Health>>,
}

pub fn prepare(cfg: Config, events: EventSink) -> Result<Prepared> {
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    let health = Arc::new(Mutex::new(Health::default()));
    let shared = health.clone();
    let thread = thread::Builder::new()
        .name("relay-recorder".into())
        .spawn(move || {
            super::com_init();
            encoder::startup();
            let mut engine = match Engine::new(cfg, events, shared.clone()) {
                Ok(e) => {
                    let _ = ready_tx.send(Ok(()));
                    e
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let result = engine.run(rx);
            let mut h = shared.lock().unwrap();
            if let Err(e) = result {
                h.error = Some(format!("{e:#}"));
            }
            h.finished = true;
        })
        .context("avvio del thread di registrazione")?;
    match ready_rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(())) => Ok(Prepared {
            tx,
            thread: Some(thread),
            health,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => bail!("il motore di registrazione non si e' preparato in 30 s"),
    }
}

impl Prepared {
    pub fn wait_frames(&self, timeout: Duration) -> FrameCheck {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let h = self.health.lock().unwrap();
                if h.frames > 0 {
                    return FrameCheck::Frames;
                }
                if let Some(e) = &h.error {
                    return FrameCheck::Broken(e.clone());
                }
                if h.finished {
                    return FrameCheck::Broken("il motore di registrazione si e' fermato".into());
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        match self.health.lock().unwrap().window {
            WindowState::Minimized => {
                FrameCheck::Waiting("la finestra del gioco e' ridotta a icona".into())
            }
            WindowState::Missing => {
                FrameCheck::Waiting("la finestra del gioco non c'e' ancora".into())
            }
            _ => FrameCheck::Broken(format!(
                "la cattura di Windows non ha dato immagini in {} s",
                timeout.as_secs()
            )),
        }
    }

    pub fn start(mut self) -> Recorder {
        let _ = self.tx.send(Cmd::Start);
        Recorder {
            tx: self.tx.clone(),
            thread: self.thread.take(),
            health: self.health.clone(),
        }
    }

    pub fn abort(mut self) {
        let _ = self.tx.send(Cmd::Abort);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = self.tx.send(Cmd::Abort);
            let _ = t.join();
        }
    }
}

impl Recorder {
    pub fn fit_window(&self) {
        let _ = self.tx.send(Cmd::Fit);
    }

    pub fn captured_frames(&self) -> u64 {
        self.health.lock().unwrap().frames
    }

    pub fn failure(&self) -> Option<String> {
        let h = self.health.lock().unwrap();
        match (&h.error, h.finished) {
            (Some(e), _) => Some(e.clone()),
            (None, true) => Some("il motore di registrazione si e' fermato da solo".into()),
            _ => None,
        }
    }

    pub fn stop(mut self) -> Result<()> {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        match self.health.lock().unwrap().error.take() {
            Some(e) => bail!(e),
            None => Ok(()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = self.tx.send(Cmd::Stop);
            let _ = t.join();
        }
    }
}

pub struct Choice {
    pub gpu: Gpu,
    pub candidate: Candidate,
    pub names: Vec<String>,
    pub rejected: Vec<String>,
}

type Slot = (Option<usize>, Candidate);

fn options() -> (Vec<device::Adapter>, Vec<Slot>) {
    let adapters = device::adapters();
    let mut hardware: Vec<Slot> = Vec::new();
    for (i, a) in adapters.iter().enumerate() {
        for c in encoder::hardware(a.luid, a.vendor) {
            hardware.push((Some(i), c));
        }
    }
    hardware.sort_by_key(|(_, c)| c.kind);
    let default_adapter = (!adapters.is_empty()).then_some(0);
    let mut options = hardware;
    options.extend(
        encoder::software()
            .into_iter()
            .map(|c| (default_adapter, c)),
    );
    (adapters, options)
}

fn describe(adapters: &[device::Adapter], adapter: Option<usize>, candidate: &Candidate) -> String {
    format!(
        "{} ({} su {})",
        candidate.kind.name(),
        candidate.label,
        adapter.map_or("scheda predefinita", |i| adapters[i].name.as_str())
    )
}

pub fn choose_encoder(
    pref: EncoderPref,
    size: (u32, u32),
    fps: u32,
    kbps: u32,
    warn: &dyn Fn(String),
) -> Result<Choice> {
    let (adapters, options) = options();
    let mut names: Vec<String> = Vec::new();
    for (_, c) in &options {
        let n = c.kind.name().to_string();
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let (mut order, rest): (Vec<_>, Vec<_>) =
        options.into_iter().partition(|(_, c)| c.kind.matches(pref));
    if pref != EncoderPref::Auto {
        if order.is_empty() {
            warn(format!(
                "l'encoder {} non c'e' su questo PC: uso il migliore disponibile",
                pref.name()
            ));
        }
        order.extend(rest);
    }
    let mut rejected = Vec::new();
    for (adapter, candidate) in order {
        let who = describe(&adapters, adapter, &candidate);
        if candidate.kind == Kind::Software && pref != EncoderPref::Software {
            rejected.push(format!(
                "{who}: codifica CPU esclusa dal profilo automatico a basso consumo"
            ));
            continue;
        }
        let gpu = match device::create(adapter.map(|i| &adapters[i])) {
            Ok(g) => g,
            Err(e) => {
                rejected.push(format!("{who}: {e:#}"));
                continue;
            }
        };
        match self_test(&candidate, &gpu, size, fps, kbps) {
            Ok(Verdict::Good) => {
                return Ok(Choice {
                    gpu,
                    candidate,
                    names,
                    rejected,
                });
            }
            Ok(Verdict::BadKeyframes(why)) => {
                rejected.push(format!("{who}: {why}"));
            }
            Err(e) => rejected.push(format!("{who}: {e:#}")),
        }
    }
    if rejected.is_empty() {
        bail!("nessun encoder H.264 di Windows trovato");
    }
    bail!("nessun encoder H.264 funzionante: {}", rejected.join("; "))
}

pub fn check_all(size: (u32, u32), fps: u32, kbps: u32) -> Vec<String> {
    let (adapters, options) = options();
    if options.is_empty() {
        return vec!["nessun encoder H.264 di Windows trovato".into()];
    }
    options
        .into_iter()
        .map(|(adapter, candidate)| {
            let who = describe(&adapters, adapter, &candidate);
            let verdict = device::create(adapter.map(|i| &adapters[i]))
                .and_then(|gpu| self_test(&candidate, &gpu, size, fps, kbps));
            match verdict {
                Ok(Verdict::Good) => format!("{who}: ok"),
                Ok(Verdict::BadKeyframes(why)) => format!("{who}: {why}"),
                Err(e) => format!("{who}: {e:#}"),
            }
        })
        .collect()
}

fn live_frame(scheduled: u64, current: u64) -> u64 {
    scheduled.max(current)
}

fn next_frame(enc: &mut H264, out: &mut Vec<Encoded>) -> Result<Option<encoder::Frame>> {
    next_frame_wait(enc, out, 25)
}
fn next_frame_wait(
    enc: &mut H264,
    out: &mut Vec<Encoded>,
    attempts: usize,
) -> Result<Option<encoder::Frame>> {
    for _ in 0..attempts {
        if let Some(f) = enc.frame()? {
            return Ok(Some(f));
        }
        enc.poll(out)?;
        thread::sleep(Duration::from_millis(1));
    }
    Ok(None)
}

pub enum Verdict {
    Good,
    BadKeyframes(String),
}

const TEST_GOP: u64 = 5;
const TEST_FRAMES: u64 = 12;

pub fn self_test(
    candidate: &Candidate,
    gpu: &Gpu,
    size: (u32, u32),
    fps: u32,
    kbps: u32,
) -> Result<Verdict> {
    let mut enc = H264::open(candidate, gpu, size, fps, kbps, TEST_GOP as u32)?;
    let mut conv = Converter::new(gpu, size, fps)?;
    let line = Timeline {
        fps,
        segment_secs: 1,
        origin_unix: 0.0,
        start_segment: 0,
    };
    let mut out = Vec::new();
    for k in 0..TEST_FRAMES {
        let frame = next_frame(&mut enc, &mut out)?.context("l'encoder non accetta immagini")?;
        conv.convert(None, &frame.texture, frame.slice)?;
        let duration = line.sample_time_100ns(k + 1) - line.sample_time_100ns(k);
        enc.encode(
            frame,
            line.sample_time_100ns(k),
            duration,
            k.is_multiple_of(TEST_GOP),
            &mut out,
        )?;
    }
    enc.drain(&mut out)?;
    if out.is_empty() {
        bail!("l'encoder non ha prodotto video");
    }
    let mut units = AccessUnits::default();
    if let Some(h) = &enc.sequence_header {
        units.remember(h);
    }
    let times: Vec<Option<u64>> = out
        .iter()
        .map(|e| e.time.map(|t| line.frame_from_100ns(t)))
        .collect();
    let by_time = times.iter().all(|t| t.is_some_and(|f| f < TEST_FRAMES))
        && times.windows(2).all(|w| w[0] < w[1]);
    let mut keyframes = std::collections::HashMap::new();
    for (i, e) in out.iter().enumerate() {
        let frame = if by_time { times[i].unwrap() } else { i as u64 };
        let (_, key) = units.prepare(&e.data);
        keyframes.insert(frame, key);
    }
    let missing: Vec<String> = (0..TEST_FRAMES)
        .step_by(TEST_GOP as usize)
        .filter(|k| keyframes.get(k) != Some(&true))
        .map(|k| k.to_string())
        .collect();
    if missing.is_empty() {
        Ok(Verdict::Good)
    } else {
        Ok(Verdict::BadKeyframes(format!(
            "fotogrammi chiave IDR mancanti dove richiesti (fotogrammi {} su {TEST_FRAMES})",
            missing.join(", ")
        )))
    }
}

struct Engine {
    cfg: Config,
    events: EventSink,
    health: Arc<Mutex<Health>>,
    gpu: Gpu,
    candidate: Candidate,
    exe: Option<String>,
    window: Option<Found>,
    window_source: Option<Source>,
    hook_source: Option<Source>,
    hook_since: Option<Instant>,
    using_hook: bool,
    window_since: Instant,
    monitor_source: Option<Source>,
    monitor_handle: Option<isize>,
    fixed_monitor: bool,
    using_monitor: bool,
    last_search: Instant,
    last_frame: Option<(Unsync<ID3D11Texture2D>, (u32, u32))>,
    base: (u32, u32),
    canvas: (u32, u32),
    audio: Option<Inputs>,
    game_audio: Option<GameAudio>,
    warned: HashSet<&'static str>,
}

impl Engine {
    fn new(cfg: Config, events: EventSink, health: Arc<Mutex<Health>>) -> Result<Self> {
        window::dpi_aware();
        if !capture::supported() {
            bail!("la cattura di Windows non e' disponibile: serve Windows 10 versione 1903 o successiva");
        }
        let monitors = window::monitors();
        let exe = match &cfg.target {
            Target::Window { exe } => Some(exe.clone()),
            Target::Monitor { .. } => None,
        };
        let found = exe.as_deref().and_then(window::find);
        let fixed_monitor = matches!(cfg.target, Target::Monitor { .. });
        let wanted_index = match cfg.target {
            Target::Monitor { index } => Some(index),
            Target::Window { .. } => cfg.fallback_monitor,
        };
        let monitor = window::pick_monitor(
            &monitors,
            found.as_ref().map(|f| f.handle),
            cfg.fallback_monitor_name.as_deref(),
            wanted_index,
        );
        let base = found
            .as_ref()
            .and_then(|f| window::client_size(f.handle))
            .or_else(|| monitor.as_ref().map(|m| (m.width, m.height)))
            .unwrap_or((1920, 1080));
        let canvas = layout::canvas(base, cfg.output_height);
        let warn_events = events.clone();
        let Choice {
            gpu,
            candidate,
            rejected,
            ..
        } = choose_encoder(cfg.encoder, canvas, cfg.fps, cfg.bitrate_kbps, &|m| {
            warn_events(Event::Warning(m))
        })?;
        for r in rejected {
            events(Event::Warning(format!("encoder scartato: {r}")));
        }

        let mut engine = Self {
            cfg,
            events,
            health,
            gpu,
            candidate,
            exe,
            window: None,
            window_source: None,
            hook_source: None,
            hook_since: None,
            using_hook: false,
            window_since: Instant::now(),
            monitor_source: None,
            monitor_handle: None,
            fixed_monitor,
            using_monitor: fixed_monitor,
            last_search: Instant::now(),
            last_frame: None,
            base,
            canvas,
            audio: None,
            game_audio: None,
            warned: HashSet::new(),
        };

        if fixed_monitor {
            let m = monitor.context("nessuno schermo trovato")?;
            let source = Source::monitor(&engine.gpu, m.handle)?;
            engine.note_border(&source);
            engine.monitor_source = Some(source);
            engine.monitor_handle = Some(m.handle);
        } else if let Some(f) = found {
            engine.attach_window(f);
        } else {
            engine.warn_once(
                "missing",
                "finestra del gioco non trovata: registro nero finche' non compare".into(),
            );
        }

        let game_pid = match &engine.cfg.game_audio {
            None => None,
            Some(exe) if !exe.is_empty() => window::find(exe).map(|w| w.pid),
            Some(_) => engine.window.as_ref().map(|w| w.pid),
        };
        let wants_game = engine.cfg.game_audio.is_some();
        if wants_game && game_pid.is_none() {
            let why = if fixed_monitor {
                "registro l'audio di tutto il sistema: con lo schermo intero non posso isolare il gioco"
            } else {
                "processo del gioco non trovato: registro l'audio di tutto il sistema"
            };
            engine.warn_once("system-audio", why.into());
        }
        let ev = engine.events.clone();
        let mut inputs = Inputs::start(
            game_pid,
            wants_game && game_pid.is_none(),
            engine.cfg.mic_gain,
            &|m| ev(Event::Warning(m)),
        );
        engine.game_audio = inputs.take_game();
        engine.audio = Some(inputs);
        engine.maintain();
        Ok(engine)
    }

    fn follow_game_audio(&mut self) {
        let Some(game) = self.game_audio.as_mut() else {
            return;
        };
        let pid = match self.cfg.game_audio.as_deref() {
            Some(exe) if !exe.is_empty() => window::find(exe).map(|w| w.pid),
            _ => self.window.as_ref().map(|w| w.pid),
        };
        let Some(pid) = pid.filter(|p| *p != game.pid()) else {
            return;
        };
        match game.retarget(pid) {
            Ok(()) => self.emit(Event::Warning(
                "il gioco e' ripartito: riaggancio il suo audio".into(),
            )),
            Err(e) => self.emit(Event::Warning(format!(
                "audio del gioco ripartito non riagganciato: {e}"
            ))),
        }
    }

    fn emit(&self, e: Event) {
        (self.events)(e)
    }

    fn warn_once(&mut self, key: &'static str, message: String) {
        if self.warned.insert(key) {
            self.emit(Event::Warning(message));
        }
    }

    fn note_border(&mut self, source: &Source) {
        if source.border() {
            self.warn_once(
                "border",
                "su questa versione di Windows compare un bordo giallo attorno al gioco mentre registra: lo disegna Windows e non finisce nel video (da Windows 11 non c'e' piu')".into(),
            );
        }
    }

    fn attach_window(&mut self, found: Found) {
        self.hook_source = None;
        self.window_source = None;
        self.hook_since = None;
        self.using_hook = false;
        let attempt = Source::hook(&self.gpu, found.pid, found.handle, self.cfg.fps);
        self.window = Some(found);
        match attempt {
            Ok(source) => {
                self.hook_source = Some(source);
                self.hook_since = Some(Instant::now());
            }
            Err(e) => {
                self.warn_once(
                    "hook-unavailable",
                    format!("aggancio GPU non disponibile: {e:#}; uso WGC finestra"),
                );
                self.attach_wgc();
            }
        }
    }

    fn attach_wgc(&mut self) {
        let Some(w) = self.window.as_ref().map(|w| w.handle) else {
            return;
        };
        self.window_since = Instant::now();
        match Source::window(&self.gpu, w) {
            Ok(source) => {
                self.note_border(&source);
                self.window_source = Some(source);
            }
            Err(e) => self.warn_once(
                "window-capture",
                format!("cattura della finestra non riuscita: {e:#}"),
            ),
        }
    }

    fn maintain(&mut self) {
        let now = Instant::now();
        if !self.fixed_monitor {
            let gone = self.window.as_ref().is_some_and(|w| {
                !window::exists(w.handle) || self.window_source.as_ref().is_some_and(|s| s.closed())
            });
            if gone {
                self.window = None;
                self.window_source = None;
                self.monitor_source = None;
                self.monitor_handle = None;
                self.hook_source = None;
                self.using_hook = false;
                if self.using_monitor {
                    self.using_monitor = false;
                }
                self.emit(Event::SourceChanged("nessuna".into()));
                self.warn_once(
                    "closed",
                    "il gioco si e' chiuso: tengo l'ultima immagine finche' non torna".into(),
                );
            }
            if self.window.is_none()
                && now.duration_since(self.last_search) > Duration::from_secs(1)
            {
                self.last_search = now;
                if let Some(found) = self.exe.as_deref().and_then(window::find) {
                    self.attach_window(found);
                    self.emit(Event::SourceChanged("game".into()));
                    self.follow_game_audio();
                }
            }
            if let Some(w) = self.window.as_ref().map(|w| w.handle) {
                let fullscreen = window::covers_monitor(w);
                let minimized = window::is_minimized(w);
                let hook_ready = self.hook_source.as_ref().is_some_and(|s| {
                    !s.closed()
                        && s.frames() > 0
                        && s.last_frame()
                            .is_some_and(|t| now.saturating_duration_since(t) < WINDOW_STALE)
                });
                let use_hook = hook_ready;
                if self.using_hook != use_hook {
                    self.using_hook = use_hook;
                    self.emit(Event::SourceChanged(
                        if use_hook { "hook" } else { "game" }.into(),
                    ));
                }
                let hook_starting = self
                    .hook_since
                    .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(2))
                    && self.hook_source.as_ref().is_some_and(|s| !s.closed());
                if !use_hook && !hook_starting && !minimized && self.hook_source.is_some() {
                    self.hook_source = None;
                    self.warn_once(
                        "hook-no-frames",
                        "aggancio GPU senza immagini recenti: passo a WGC finestra".into(),
                    );
                    self.attach_wgc();
                }
                let stale = match self.window_source.as_ref().and_then(|s| s.last_frame()) {
                    Some(t) => now.saturating_duration_since(t) > WINDOW_STALE,
                    None => now.saturating_duration_since(self.window_since) > WINDOW_STALE,
                };
                let in_front = fullscreen && !minimized && window::is_foreground(w);
                let want_monitor = !use_hook && !hook_starting && stale && in_front;
                if want_monitor && self.monitor_handle != Some(window::monitor_of(w)) {
                    let handle = window::monitor_of(w);
                    self.monitor_source = None;
                    self.monitor_handle = None;
                    match Source::monitor(&self.gpu, handle) {
                        Ok(s) => {
                            self.note_border(&s);
                            self.monitor_source = Some(s);
                            self.monitor_handle = Some(handle);
                            self.warn_once(
                                "fullscreen",
                                "a schermo intero la finestra del gioco non manda immagini (schermo intero esclusivo?): registro lo schermo su cui gira".into(),
                            );
                        }
                        Err(e) => self.warn_once(
                            "monitor-capture",
                            format!("cattura dello schermo non riuscita: {e:#}"),
                        ),
                    }
                }
                let use_monitor = want_monitor && self.monitor_source.is_some();
                if !want_monitor {
                    self.monitor_source = None;
                    self.monitor_handle = None;
                }
                if use_monitor != self.using_monitor {
                    self.using_monitor = use_monitor;
                    self.emit(Event::SourceChanged(self.source_label().into()));
                }
            }
        }
        let frames = self.window_source.as_ref().map_or(0, |s| s.frames())
            + self.hook_source.as_ref().map_or(0, |s| s.frames())
            + self.monitor_source.as_ref().map_or(0, |s| s.frames());
        let state = match &self.window {
            _ if self.fixed_monitor => WindowState::Unknown,
            None => WindowState::Missing,
            Some(w) if window::is_minimized(w.handle) => WindowState::Minimized,
            Some(_) => WindowState::Visible,
        };
        let mut h = self.health.lock().unwrap();
        h.frames = frames;
        h.window = state;
    }

    fn current_frame(&mut self) -> Option<(ID3D11Texture2D, (u32, u32))> {
        if self
            .window
            .as_ref()
            .is_some_and(|w| window::is_minimized(w.handle))
            && !self.fixed_monitor
        {
            return self.last_frame.as_ref().map(|(t, s)| (t.0.clone(), *s));
        }
        let source = if self.using_hook {
            self.hook_source.as_ref()
        } else if self.using_monitor {
            self.monitor_source.as_ref()
        } else {
            self.window_source.as_ref()
        };
        if let Some(f) = source.and_then(|s| s.frame()) {
            self.last_frame = Some((Unsync(f.0.clone()), f.1));
            return Some(f);
        }
        self.warn_once(
            "no-source-frame",
            "nessuna sorgente manda immagini: tengo l'ultima immagine disponibile".into(),
        );
        self.last_frame.as_ref().map(|(t, s)| (t.0.clone(), *s))
    }

    fn source_label(&self) -> &'static str {
        if self.using_hook {
            "hook"
        } else if self.using_monitor {
            "monitor"
        } else {
            "game"
        }
    }

    fn run(&mut self, rx: mpsc::Receiver<Cmd>) -> Result<()> {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Cmd::Start) => break,
                Ok(Cmd::Abort) | Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    return Ok(())
                }
                Ok(Cmd::Fit) | Err(RecvTimeoutError::Timeout) => self.maintain(),
            }
        }
        unsafe {
            timeBeginPeriod(1);
        }
        let result = self.record(&rx);
        unsafe {
            timeEndPeriod(1);
        }
        result
    }

    fn record(&mut self, rx: &mpsc::Receiver<Cmd>) -> Result<()> {
        let _timings = timing::Session::start(&self.cfg.dir).ok();
        let mut enc = H264::open(
            &self.candidate,
            &self.gpu,
            self.canvas,
            self.cfg.fps,
            self.cfg.bitrate_kbps,
            self.cfg.fps * self.cfg.segment_secs,
        )?;
        let mut conv = Converter::new(&self.gpu, self.canvas, self.cfg.fps)?;
        let primed = next_frame(&mut enc, &mut Vec::new())?
            .context("preparazione del primo fotogramma dell'encoder")?;
        let source = self.current_frame();
        conv.convert(
            source.as_ref().map(|(t, s)| (t, *s)),
            &primed.texture,
            primed.slice,
        )?;
        let origin_unix = self.cfg.origin_unix_secs.unwrap_or_else(clock::unix_now);
        let timeline = Timeline {
            fps: self.cfg.fps,
            segment_secs: self.cfg.segment_secs,
            origin_unix,
            start_segment: self.cfg.start_segment,
        };
        let origin = clock::instant_at(origin_unix);
        let mut last_maintain = Instant::now();
        loop {
            let now = Instant::now();
            if now >= origin {
                break;
            }
            match rx.recv_timeout((origin - now).min(Duration::from_millis(100))) {
                Ok(Cmd::Stop) | Ok(Cmd::Abort) | Err(RecvTimeoutError::Disconnected) => {
                    return Ok(())
                }
                _ => {}
            }
            if last_maintain.elapsed() > MAINTAIN_EVERY {
                self.maintain();
                last_maintain = Instant::now();
            }
        }

        let per_segment = timeline.frames_per_segment();
        let k0 = timeline.first_frame(clock::unix_now());
        let audio_inputs = self.audio.take().filter(|a| !a.is_empty());
        let (audio_tx, audio_rx) = mpsc::channel::<AudioPacket>();
        let audio_stop = Arc::new(AtomicBool::new(false));
        let has_audio = audio_inputs.is_some();
        let events = self.events.clone();
        let audio_thread = audio_inputs.map(|a| {
            a.run(
                timeline,
                origin,
                timeline.sample_for_frame(k0),
                audio_tx,
                audio_stop.clone(),
                move |m| events(Event::Warning(m)),
            )
        });

        let mut run = Run {
            timeline,
            generation: self.cfg.generation,
            gen_start: k0,
            writer: Some(SegmentWriter::new(
                &self.cfg.dir,
                hls::playlist_name(self.cfg.generation),
                self.cfg.start_segment + k0 / per_segment,
                self.cfg.fps,
                self.cfg.segment_secs,
                has_audio,
            )),
            enc: Some(enc),
            primed: Some(primed),
            conv,
            units: AccessUnits::default(),
            encoded: Vec::new(),
            submitted: VecDeque::new(),
            audio_rx,
            has_audio,
            started: false,
            resized: None,
            dropped: 0,
            key_pending: false,
            late_warned: false,
        };
        run.remember_headers();

        let mut k = k0;
        let result = (|| -> Result<()> {
            loop {
                timing::tick();
                let due = origin + timeline.frame_offset(k);
                let mut stop = false;
                let mut fit = false;
                loop {
                    let now = Instant::now();
                    if now >= due {
                        break;
                    }
                    match rx.recv_timeout((due - now).min(Duration::from_millis(50))) {
                        Ok(Cmd::Stop) | Ok(Cmd::Abort) | Err(RecvTimeoutError::Disconnected) => {
                            stop = true;
                            break;
                        }
                        Ok(Cmd::Fit) => fit = true,
                        _ => {}
                    }
                }
                while let Ok(c) = rx.try_recv() {
                    match c {
                        Cmd::Stop | Cmd::Abort => stop = true,
                        Cmd::Fit => fit = true,
                        Cmd::Start => {}
                    }
                }
                if stop {
                    return Ok(());
                }
                if last_maintain.elapsed() > MAINTAIN_EVERY {
                    self.maintain();
                    last_maintain = Instant::now();
                }
                if fit {
                    self.fit(&mut run, k)?;
                }
                let late = Instant::now().saturating_duration_since(due);
                let now_k = (Instant::now()
                    .saturating_duration_since(origin)
                    .as_secs_f64()
                    * self.cfg.fps as f64) as u64;
                let live_k = live_frame(k, now_k);
                if live_k > k {
                    run.dropped += live_k - k;
                    let segment = timeline.frames_per_segment();
                    run.key_pending |= (live_k - run.gen_start) / segment
                        != (k - run.gen_start) / segment
                        || (k - run.gen_start).is_multiple_of(segment);
                    k = live_k;
                }
                let encoder_stale = run.submitted.front().is_some_and(|old| {
                    k.saturating_sub(*old) > self.cfg.fps as u64 * MAX_BEHIND.as_secs()
                });
                if late > MAX_BEHIND || encoder_stale {
                    run.flush()?;
                    run.enc.as_mut().unwrap().recover()?;
                    run.dropped += run.submitted.len() as u64;
                    run.submitted.clear();
                    run.encoded.clear();
                    run.key_pending = true;
                    self.warn_once("behind", "il PC e' rimasto indietro: riprendo dal fotogramma corrente e svuoto la coda dell'encoder".into());
                }
                let source = self.current_frame();
                let label = self.source_label();
                run.frame(k, source, label, &self.candidate, &self.events)?;
                k += 1;
            }
        })();

        let finish = run.finish(audio_thread, &audio_stop);
        if run.dropped > 0 {
            self.emit(Event::Warning(format!(
                "fotogrammi saltati: {}",
                run.dropped
            )));
        }
        result.and(finish)
    }

    fn fit(&mut self, run: &mut Run, k: u64) -> Result<()> {
        let size = if self.using_monitor || self.fixed_monitor {
            None
        } else {
            self.window
                .as_ref()
                .and_then(|w| window::client_size(w.handle))
        };
        let Some(size) = size.filter(|s| *s != self.base) else {
            self.emit(Event::Resized {
                generation: run.generation,
                width: self.canvas.0,
                height: self.canvas.1,
            });
            return Ok(());
        };
        self.base = size;
        self.canvas = layout::canvas(size, self.cfg.output_height);
        run.restart(k, &self.cfg, &self.candidate, &self.gpu, self.canvas)?;
        Ok(())
    }
}

struct Run {
    timeline: Timeline,
    generation: u32,
    gen_start: u64,
    writer: Option<SegmentWriter>,
    enc: Option<H264>,
    primed: Option<encoder::Frame>,
    conv: Converter,
    units: AccessUnits,
    encoded: Vec<Encoded>,
    submitted: VecDeque<u64>,
    audio_rx: mpsc::Receiver<AudioPacket>,
    has_audio: bool,
    started: bool,
    resized: Option<(u32, u32)>,
    dropped: u64,
    key_pending: bool,
    late_warned: bool,
}

impl Run {
    fn remember_headers(&mut self) {
        self.units = AccessUnits::default();
        if let Some(h) = self.enc.as_ref().and_then(|e| e.sequence_header.clone()) {
            self.units.remember(&h);
        }
    }

    fn frame(
        &mut self,
        k: u64,
        source: Option<(ID3D11Texture2D, (u32, u32))>,
        label: &str,
        candidate: &Candidate,
        events: &EventSink,
    ) -> Result<()> {
        let per_segment = self.timeline.frames_per_segment();
        let boundary = (k - self.gen_start).is_multiple_of(per_segment);
        let enc = self.enc.as_mut().unwrap();
        let frame = match self.primed.take() {
            Some(frame) => Some(frame),
            None => next_frame_wait(enc, &mut self.encoded, 2)?,
        };
        let Some(frame) = frame else {
            self.dropped += 1;
            self.key_pending |= boundary;
            return self.flush();
        };
        let measure = timing::span(Stage::Convert);
        self.conv.convert(
            source.as_ref().map(|(t, s)| (t, *s)),
            &frame.texture,
            frame.slice,
        )?;
        drop(measure);
        let keyframe = boundary || self.key_pending;
        let time = self.timeline.sample_time_100ns(k);
        let duration = self.timeline.sample_time_100ns(k + 1) - time;
        let enc = self.enc.as_mut().unwrap();
        enc.encode(frame, time, duration, keyframe, &mut self.encoded)?;
        self.key_pending = false;
        self.submitted.push_back(k);
        if !self.started {
            self.started = true;
            events(Event::Started {
                generation: self.generation,
                first_frame_unix_secs: self.timeline.frame_unix(self.gen_start),
                encoder: candidate.kind.name().into(),
                source: label.into(),
            });
            if let Some((width, height)) = self.resized.take() {
                events(Event::Resized {
                    generation: self.generation,
                    width,
                    height,
                });
            }
        }
        self.flush()?;
        let late = self.writer.as_ref().map_or(0, |w| w.late_keyframes());
        if late > 0 && !self.late_warned {
            self.late_warned = true;
            events(Event::Warning(
                "l'encoder non ha messo il fotogramma chiave all'inizio di un pezzo: quel pezzo sara' piu' lungo".into(),
            ));
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        let writer = self.writer.as_mut().unwrap();
        for e in self.encoded.drain(..) {
            let by_time = e.time.map(|t| self.timeline.frame_from_100ns(t));
            let front = self.submitted.front().copied();
            let back = self.submitted.back().copied();
            let frame = match (by_time, front, back) {
                (Some(t), Some(f), Some(b)) if t >= f && t <= b => t,
                (_, Some(f), _) => f,
                (Some(t), None, _) => t,
                (None, None, _) => continue,
            };
            while self.submitted.front().is_some_and(|&f| f <= frame) {
                self.submitted.pop_front();
            }
            let (data, keyframe) = self.units.prepare(&e.data);
            writer.push_video(VideoPacket {
                frame,
                pts: self.timeline.video_pts(frame),
                keyframe,
                data,
            })?;
        }
        while let Ok(p) = self.audio_rx.try_recv() {
            writer.push_audio(p)?;
        }
        Ok(())
    }

    fn restart(
        &mut self,
        k: u64,
        cfg: &Config,
        candidate: &Candidate,
        gpu: &Gpu,
        canvas: (u32, u32),
    ) -> Result<()> {
        self.primed = None;
        if let Some(mut enc) = self.enc.take() {
            enc.drain(&mut self.encoded)?;
        }
        self.flush()?;
        let (summary, carry) = self
            .writer
            .take()
            .unwrap()
            .finish(Some(self.timeline.video_pts(k)))?;
        self.generation += 1;
        self.gen_start = k;
        self.submitted.clear();
        self.started = false;
        let mut writer = SegmentWriter::new(
            &cfg.dir,
            hls::playlist_name(self.generation),
            summary.next_index,
            cfg.fps,
            cfg.segment_secs,
            self.has_audio,
        );
        for p in carry {
            writer.push_audio(p)?;
        }
        self.writer = Some(writer);
        self.enc = Some(H264::open(
            candidate,
            gpu,
            canvas,
            cfg.fps,
            cfg.bitrate_kbps,
            self.timeline.frames_per_segment() as u32,
        )?);
        self.remember_headers();
        self.conv.resize(canvas);
        self.resized = Some(canvas);
        Ok(())
    }

    fn finish(&mut self, audio: Option<JoinHandle<()>>, audio_stop: &AtomicBool) -> Result<()> {
        if let Some(mut enc) = self.enc.take() {
            let _ = enc.drain(&mut self.encoded);
        }
        audio_stop.store(true, Ordering::Relaxed);
        if let Some(t) = audio {
            let _ = t.join();
        }
        self.flush()?;
        if let Some(w) = self.writer.take() {
            w.finish(None)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::live_frame;
    #[test]
    fn resumes_live_after_stall_without_catchup_burst() {
        assert_eq!(live_frame(420, 1260), 1260);
        assert_eq!(live_frame(1261, 1261), 1261);
        assert_eq!(live_frame(1262, 1261), 1262);
    }
}

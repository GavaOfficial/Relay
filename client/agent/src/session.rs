use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use futures_util::{SinkExt, StreamExt};
use relay_common::{AgentState, ClientMsg, PlayerHealth, PlayerStatus, ServerMsg, SEGMENT_SECONDS};
use serde::Serialize;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

use crate::{
    audio::{AudioChoice, AudioEngine},
    capture::{detect_encoder, list_monitors, Capture, CaptureConfig, Encoder, WindowSel},
    clock::{local_now_ms, local_now_secs, Clock},
    priority,
    uploader::{pending_count, read_all_durations, UploadConfig, Uploader},
    windows,
};

#[derive(Clone)]
pub struct SessionParams {
    pub ffmpeg: PathBuf,
    pub server: String,
    pub token: String,
    pub match_id: String,
    pub player: String,

    pub plays: bool,

    pub window: Option<WindowSel>,
    pub fps: u32,
    pub bitrate_kbps: u32,

    pub encoder: Option<Encoder>,
    pub limit_bytes_per_sec: Option<u64>,
    pub work_dir: PathBuf,

    pub audio: AudioChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    #[default]
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Lobby,
    Recording,

    Stopping,

    Uploading,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct SessionState {
    pub connection: Connection,
    pub phase: Phase,
    pub error: Option<String>,

    pub started_at_ms: Option<u64>,
    pub stopped_at_ms: Option<u64>,

    pub presence: Vec<String>,

    pub health: Vec<PlayerHealth>,

    pub me: Option<PlayerStatus>,

    pub clock_offset_ms: Option<i64>,
}

struct Shared {
    state: watch::Sender<SessionState>,
    status: Mutex<PlayerStatus>,

    selected: Mutex<Option<WindowSel>>,

    audio: Mutex<AudioChoice>,

    report: bool,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut SessionState)) {
        self.state.send_modify(f);
    }

    fn set_status(&self, f: impl FnOnce(&mut PlayerStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap();
            f(&mut s);
            s.clone()
        };
        if self.report {
            self.update(|st| st.me = Some(snapshot));
        }
    }

    fn set_phase(&self, phase: Phase) {
        self.update(|s| s.phase = phase);
        if self.report {
            let st = match phase {
                Phase::Lobby => AgentState::Waiting,
                Phase::Recording | Phase::Stopping => AgentState::Recording,
                Phase::Uploading => AgentState::Uploading,
                Phase::Done => AgentState::Waiting,
                Phase::Error => AgentState::Error,
            };
            self.set_status(|s| s.state = st);
        }
    }

    fn fail(&self, msg: &str) {
        tracing::error!("sessione fermata per un errore: {msg}");
        self.update(|s| s.error = Some(msg.to_string()));
        self.set_status(|s| s.issue = Some(msg.to_string()));
        self.set_phase(Phase::Error);
    }
}

#[derive(Debug)]
enum Event {
    Start(u64),
    Stop(u64),
    Ended,
}

fn ws_url(server: &str, match_id: &str) -> String {
    let base = server.trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{base}/api/matches/{match_id}/ws")
}

async fn ws_task(
    url: String,
    token: String,
    clock: Arc<Clock>,
    tx: mpsc::UnboundedSender<Event>,
    shared: Arc<Shared>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let req = match url.as_str().into_client_request() {
            Ok(mut r) => {
                r.headers_mut()
                    .insert("authorization", format!("Bearer {token}").parse().unwrap());
                r
            }
            Err(e) => {
                shared.fail(&format!("indirizzo del server non valido: {e}"));
                return;
            }
        };
        match connect_async(req).await {
            Ok((ws, _)) => {
                tracing::info!("connesso al server");
                backoff = Duration::from_secs(1);
                shared.update(|s| s.connection = Connection::Connected);
                let (mut sink, mut stream) = ws.split();
                let mut sent = 0u32;

                let mut next_ping = Instant::now();
                let mut status_iv = tokio::time::interval(Duration::from_secs(2));
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(next_ping.into()) => {
                            let t0 = local_now_ms();
                            let msg = serde_json::to_string(&ClientMsg::Ping { t0 }).unwrap();
                            if sink.send(Message::Text(msg.into())).await.is_err() { break; }
                            sent += 1;

                            next_ping = Instant::now() + if sent < 8 { Duration::from_millis(250) } else { Duration::from_secs(10) };
                        }
                        _ = status_iv.tick() => {
                            if shared.report {
                                let s = shared.status.lock().unwrap().clone();
                                let msg = serde_json::to_string(&ClientMsg::Status(s)).unwrap();
                                if sink.send(Message::Text(msg.into())).await.is_err() { break; }
                            }
                        }
                        inc = stream.next() => {
                            let Some(Ok(m)) = inc else { break };
                            let Message::Text(t) = m else { continue };
                            let Ok(msg) = serde_json::from_str::<ServerMsg>(&t) else { continue };
                            match msg {
                                ServerMsg::Pong { t0, server_ms } => clock.record(t0, local_now_ms(), server_ms),
                                ServerMsg::Start { at_ms } => { let _ = tx.send(Event::Start(at_ms)); }
                                ServerMsg::Stop { at_ms } => { let _ = tx.send(Event::Stop(at_ms)); }
                                ServerMsg::Presence { connected } => shared.update(|s| s.presence = connected),
                                ServerMsg::Health { players } => shared.update(|s| s.health = players),
                                ServerMsg::State { status, start_at_ms, stop_at_ms } => {
                                    if let Some(a) = start_at_ms { let _ = tx.send(Event::Start(a)); }
                                    if let Some(a) = stop_at_ms { let _ = tx.send(Event::Stop(a)); }
                                    if status == relay_common::MatchStatus::Ended { let _ = tx.send(Event::Ended); }
                                }
                            }
                        }
                    }
                }
                shared.update(|s| s.connection = Connection::Reconnecting);
                tracing::warn!("connessione al server persa, riprovo");
            }
            Err(e) => {
                shared.update(|s| {
                    if s.connection == Connection::Connected {
                        s.connection = Connection::Reconnecting;
                    }
                });
                tracing::warn!("connessione al server fallita ({e}), riprovo tra {backoff:?}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(10));
    }
}

fn audio_label(c: AudioChoice) -> Option<String> {
    match (c.game, c.mic) {
        (true, true) => Some("gioco + microfono".into()),
        (true, false) => Some("gioco".into()),
        (false, true) => Some("microfono".into()),
        (false, false) => None,
    }
}

fn open_segment_age(dir: &std::path::Path) -> Option<Duration> {
    let mut oldest: Option<Duration> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.file_name().to_string_lossy().ends_with(".ts.tmp") {
            if let Ok(t) = e
                .metadata()
                .and_then(|m| m.created().or_else(|_| m.modified()))
            {
                let age = t.elapsed().unwrap_or_default();
                oldest = Some(oldest.map_or(age, |o| o.max(age)));
            }
        }
    }
    oldest
}

async fn reporter(
    shared: Arc<Shared>,
    dir: PathBuf,
    sent: Arc<std::sync::atomic::AtomicU64>,
    clock: Arc<Clock>,
    encoder_name: Arc<Mutex<Option<String>>>,
) {
    let mut window: VecDeque<(Instant, u64)> = VecDeque::new();
    let mut iv = tokio::time::interval(Duration::from_secs(1));
    loop {
        iv.tick().await;
        let sel = shared.selected.lock().unwrap().clone();
        let chosen = sel.is_some();
        let win = match sel {
            Some(sel) => tokio::task::spawn_blocking(move || windows::find_window(&sel))
                .await
                .unwrap_or(None),
            None => None,
        };
        let found = win.is_some();
        let backlog = pending_count(&dir).await;

        let now = Instant::now();
        window.push_back((now, sent.load(Ordering::Relaxed)));
        while window.len() > 2 && now.duration_since(window[0].0) > Duration::from_secs(6) {
            window.pop_front();
        }
        let kbps = match (window.front(), window.back()) {
            (Some(&(t0, b0)), Some(&(t1, b1))) if t1 > t0 => {
                ((b1 - b0) as f64 * 8.0 / 1000.0 / t1.duration_since(t0).as_secs_f64()).round()
                    as u32
            }
            _ => 0,
        };

        let recording = shared.state.borrow().phase == Phase::Recording;
        let stuck = recording
            && open_segment_age(&dir)
                .is_some_and(|age| age > Duration::from_secs(SEGMENT_SECONDS as u64 * 3));

        let best = clock.best();
        let enc = encoder_name.lock().unwrap().clone();
        shared.set_status(|s| {
            s.window_found = Some(found);

            if let Some(w) = &win {
                s.window = Some(w.label());
            }
            if !chosen {
                s.window = None;
            }

            if s.state != AgentState::Error {
                s.issue = if !chosen {
                    Some("finestra non scelta".into())
                } else if stuck {
                    Some("l'encoder non chiude i segmenti (immagini chiave mancanti): il replay non sara' utilizzabile".into())
                } else {
                    None
                };
            }
            s.backlog = backlog;
            s.upload_kbps = kbps;
            s.rtt_ms = best.map(|(_, rtt)| rtt as u32);
            s.offset_ms = best.map(|(off, _)| off.round() as i64);
            s.encoder = enc;
        });
        shared.update(|st| st.clock_offset_ms = best.map(|(off, _)| off.round() as i64));
    }
}

pub struct SessionHandle {
    pub state: watch::Receiver<SessionState>,
    cancel: Arc<watch::Sender<bool>>,
    task: JoinHandle<Result<()>>,
    shared: Arc<Shared>,
}

impl SessionHandle {
    pub fn set_audio(&self, choice: AudioChoice) {
        *self.shared.audio.lock().unwrap() = choice;
        self.shared.set_status(|s| {
            s.audio = audio_label(choice);
            s.audio_issue = None;
        });
    }

    pub fn set_window(&self, sel: Option<WindowSel>) {
        *self.shared.selected.lock().unwrap() = sel;
    }

    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub async fn join(self) -> Result<()> {
        self.task
            .await
            .map_err(|e| anyhow!("sessione interrotta: {e}"))?
    }
}

pub fn spawn(p: SessionParams) -> SessionHandle {
    let (state_tx, state_rx) = watch::channel(SessionState::default());
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let shared = Arc::new(Shared {
        state: state_tx,
        status: Mutex::new(PlayerStatus {
            state: AgentState::Idle,
            ..Default::default()
        }),
        selected: Mutex::new(p.window.clone()),
        audio: Mutex::new(p.audio),
        report: p.plays,
    });
    if p.plays {
        shared.set_status(|s| {
            s.state = AgentState::Waiting;
            s.audio = audio_label(p.audio);
        });
    }
    let shared_for_handle = shared.clone();
    let task = tokio::spawn({
        let shared = shared.clone();
        async move {
            let r = run_inner(p, shared.clone(), cancel_rx).await;
            if let Err(e) = &r {
                shared.fail(&format!("{e:#}"));
            }
            r
        }
    });
    SessionHandle {
        state: state_rx,
        cancel: Arc::new(cancel_tx),
        task,
        shared: shared_for_handle,
    }
}

pub async fn run(p: SessionParams) -> Result<()> {
    let h = spawn(p);
    let cancel = h.cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("stop richiesto");
        let _ = cancel.send(true);
    });
    h.join().await
}

async fn cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn sleep_until_local(secs: f64) {
    let d = secs - local_now_secs();
    if d > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(d)).await;
    }
}

async fn run_inner(
    p: SessionParams,
    shared: Arc<Shared>,
    mut cancel: watch::Receiver<bool>,
) -> Result<()> {
    let clock = Clock::new();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let ws = tokio::spawn(ws_task(
        ws_url(&p.server, &p.match_id),
        p.token.clone(),
        clock.clone(),
        ev_tx,
        shared.clone(),
    ));

    if !p.plays {
        let r = observe(&shared, &mut ev_rx, &mut cancel).await;
        ws.abort();
        return r;
    }

    let dir = p.work_dir.join(&p.match_id);
    priority::set_low_priority();

    let up = Arc::new(Uploader::new(UploadConfig {
        base_url: p.server.clone(),
        token: p.token.clone(),
        match_id: p.match_id.clone(),
        player: p.player.clone(),
        dir: dir.clone(),
        limit_bytes_per_sec: p.limit_bytes_per_sec,
    })?);

    let encoder_name = Arc::new(Mutex::new(p.encoder.map(|e| e.name().to_string())));
    let enc_task: JoinHandle<Result<Encoder>> = tokio::spawn({
        let (ff, fixed, name) = (p.ffmpeg.clone(), p.encoder, encoder_name.clone());
        async move {
            let e = match fixed {
                Some(e) => e,
                None => detect_encoder(&ff).await?,
            };
            *name.lock().unwrap() = Some(e.name().to_string());
            Ok(e)
        }
    });
    let rep = tokio::spawn(reporter(
        shared.clone(),
        dir.clone(),
        up.bytes_sent.clone(),
        clock.clone(),
        encoder_name,
    ));

    let result = record_flow(
        &p,
        &shared,
        &mut ev_rx,
        &mut cancel,
        &clock,
        &up,
        dir,
        enc_task,
    )
    .await;
    rep.abort();
    ws.abort();
    result
}

async fn observe(
    shared: &Shared,
    ev_rx: &mut mpsc::UnboundedReceiver<Event>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<()> {
    loop {
        tokio::select! {
            ev = ev_rx.recv() => match ev {
                Some(Event::Start(at)) => shared.update(|s| { s.started_at_ms = Some(at); s.phase = Phase::Recording; }),
                Some(Event::Stop(at)) => shared.update(|s| { s.stopped_at_ms = Some(at); s.phase = Phase::Uploading; }),
                Some(Event::Ended) => { shared.set_phase(Phase::Done); }
                None => return Ok(()),
            },
            _ = cancelled(cancel) => return Ok(()),
        }
    }
}

fn desired_source(sel: &WindowSel, outputs: &[(u32, u32, u32)]) -> WindowSel {
    if matches!(sel, WindowSel::Monitor(_)) {
        return sel.clone();
    }
    windows::fullscreen_screen(sel)
        .and_then(|sc| windows::output_index(sc, &windows::display_rects(), outputs))
        .map(WindowSel::Monitor)
        .unwrap_or_else(|| sel.clone())
}

async fn local_state(dir: &std::path::Path) -> (std::collections::BTreeSet<u64>, u32) {
    let durations = read_all_durations(dir).await;
    let full = |n: &u64| {
        durations
            .get(n)
            .is_some_and(|&ms| ms >= crate::hls::MIN_FULL_SEGMENT_MS)
    };
    let mut known: std::collections::BTreeSet<u64> =
        durations.keys().filter(|n| full(n)).copied().collect();
    let mut generations = 0u32;
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tmp") {
                let _ = tokio::fs::remove_file(e.path()).await;
            } else if crate::hls::is_playlist(&name) {
                generations += 1;
            } else if let Some(n) = crate::hls::segment_index(&name) {
                if full(&n) {
                    known.insert(n);
                } else {
                    let _ = tokio::fs::remove_file(e.path()).await;
                }
            }
        }
    }
    (known, generations)
}

async fn server_segments(p: &SessionParams) -> Result<std::collections::BTreeSet<u64>> {
    let url = format!(
        "{}/api/matches/{}/players/{}/playlist.m3u8",
        p.server.trim_end_matches('/'),
        p.match_id,
        p.player
    );
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(&p.token)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!(
            "il server ha risposto {} alla lettura dei segmenti gia' caricati",
            resp.status()
        );
    }
    let text = resp.text().await?;
    Ok(crate::hls::parse_durations(&text)
        .into_iter()
        .filter(|&(_, ms)| ms >= crate::hls::MIN_FULL_SEGMENT_MS)
        .map(|(n, _)| n)
        .collect())
}

async fn next_segment(dir: &std::path::Path) -> u64 {
    let mut last = read_all_durations(dir).await.keys().max().copied();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if let Some(n) = crate::hls::segment_index(&e.file_name().to_string_lossy()) {
                last = Some(last.map_or(n, |l| l.max(n)));
            }
        }
    }
    last.map_or(0, |l| l + 1)
}

#[allow(clippy::too_many_arguments)]
async fn record_flow(
    p: &SessionParams,
    shared: &Arc<Shared>,
    ev_rx: &mut mpsc::UnboundedReceiver<Event>,
    cancel: &mut watch::Receiver<bool>,
    clock: &Arc<Clock>,
    up: &Arc<Uploader>,
    dir: PathBuf,
    enc_task: JoinHandle<Result<Encoder>>,
) -> Result<()> {
    let outputs_task = {
        let ff = p.ffmpeg.clone();
        tokio::spawn(async move { list_monitors(&ff).await })
    };

    tracing::info!("in attesa dello start dal coordinatore");
    let start_ms = loop {
        tokio::select! {
            ev = ev_rx.recv() => match ev {
                Some(Event::Start(at)) => break at,
                Some(_) => {}
                None => bail!("canale eventi chiuso"),
            },
            _ = cancelled(cancel) => { enc_task.abort(); return Ok(()); }
        }
    };
    shared.update(|s| s.started_at_ms = Some(start_ms));

    for _ in 0..40 {
        if clock.sample_count() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let Some(origin) = clock.server_ms_to_local_secs(start_ms) else {
        bail!("impossibile sincronizzare l'orologio con il server");
    };
    let (off, rtt) = clock.best().unwrap();
    let lead = origin - local_now_secs();
    tracing::info!("start tra {lead:.2} s (offset orologio {off:+.1} ms, rtt {rtt} ms)");

    let resuming = lead < -1.0;
    if resuming {
        tracing::info!(
            "la partita e' in corso da {:.0} s: riprendo la registrazione",
            -lead
        );

        tokio::time::sleep(Duration::from_millis(700)).await;
        let mut stopped = false;
        while let Ok(ev) = ev_rx.try_recv() {
            if let Event::Stop(at) = ev {
                stopped |= clock
                    .server_ms_to_local_secs(at)
                    .is_some_and(|s| s <= local_now_secs());
            }
        }
        if stopped {
            enc_task.abort();
            tracing::info!("la partita e' gia' finita: carico solo quanto e' rimasto");
            shared.set_phase(Phase::Uploading);
            let (_keep, done_rx) = watch::channel(true);
            up.run(done_rx).await?;
            up.finish().await?;
            shared.set_phase(Phase::Done);
            return Ok(());
        }
    } else if lead < 1.0 {
        tracing::warn!("start troppo vicino: l'inizio potrebbe essere un fermo immagine");
    }

    let window = loop {
        let chosen = shared.selected.lock().unwrap().clone();
        if let Some(w) = chosen {
            if !resuming || matches!(w, WindowSel::Monitor(_)) {
                break w;
            }
            let probe = w.clone();
            if tokio::task::spawn_blocking(move || windows::window_exists(&probe))
                .await
                .unwrap_or(false)
            {
                break w;
            }
        }
        if !resuming {
            enc_task.abort();
            bail!("finestra da registrare non scelta");
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = cancelled(cancel) => { enc_task.abort(); return Ok(()); }
        }
    };
    let encoder = enc_task
        .await
        .map_err(|e| anyhow!("rilevamento encoder interrotto: {e}"))??;
    tracing::info!("encoder: {}", encoder.name());

    let outputs: Vec<(u32, u32, u32)> = outputs_task
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.index, m.width, m.height))
        .collect();

    let audio_choice = *shared.audio.lock().unwrap();
    let engine: Option<AudioEngine> = if audio_choice.any() {
        let w = window.clone();
        let pid = tokio::task::spawn_blocking(move || windows::find_pid(&w))
            .await
            .unwrap_or(None);
        tokio::task::spawn_blocking(move || AudioEngine::start(audio_choice, pid))
            .await
            .ok()
    } else {
        None
    };
    let has_audio = engine.as_ref().is_some_and(|e| e.has_audio());
    let mut last_report = engine.as_ref().map(|e| e.report()).unwrap_or_default();
    if engine.is_some() {
        let r = last_report.clone();
        shared.set_status(|s| {
            s.audio = r.label;
            s.audio_issue = r.issue;
        });
    }

    let make_cfg = |source: WindowSel, generation: u32, start_segment: u64| {
        let start = origin + (start_segment * SEGMENT_SECONDS as u64) as f64;
        CaptureConfig {
            ffmpeg: p.ffmpeg.clone(),

            window: match source {
                WindowSel::Monitor(o) => WindowSel::Monitor(
                    windows::display_index(o, &windows::display_rects(), &outputs).unwrap_or(o),
                ),
                other => other,
            },
            fps: p.fps,
            bitrate_kbps: p.bitrate_kbps,
            dir: dir.clone(),
            encoder,
            duration_secs: None,
            origin_unix_secs: Some(start),
            start_segment,
            generation,
            audio_port: engine
                .as_ref()
                .filter(|_| has_audio)
                .and_then(|e| e.attach(start).ok()),
        }
    };
    let sel = window.clone();
    let mut current = {
        let (s, o) = (sel.clone(), outputs.clone());
        tokio::task::spawn_blocking(move || desired_source(&s, &o))
            .await
            .unwrap_or_else(|_| window.clone())
    };
    if current != sel {
        tracing::info!("il gioco e' a schermo intero: registro {current:?}");
    }
    let (mut generation, first_segment, gap_runs, gap_base) = if resuming {
        let (mut known, generations) = local_state(&dir).await;
        let mut last_err = None;
        for _ in 0..3 {
            match server_segments(p).await {
                Ok(s) => {
                    known.extend(s);
                    last_err = None;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
        if let Some(e) = last_err {
            return Err(e.context("non riesco a sapere cosa e' gia' stato caricato"));
        }

        let now_seg =
            ((local_now_secs() + 2.0 - origin) / SEGMENT_SECONDS as f64).floor() as u64 + 1;
        let k = now_seg.max(known.iter().next_back().map_or(0, |l| l + 1));

        let runs = crate::hls::missing_runs(&known, k);
        (generations + runs.len() as u32, k, runs, generations)
    } else {
        (0u32, 0u64, Vec::new(), 0u32)
    };
    let mut cap = Capture::spawn(&make_cfg(current.clone(), generation, first_segment))?;
    shared.set_phase(Phase::Recording);
    let (done_tx, done_rx) = watch::channel(false);
    let upload_task = tokio::spawn({
        let up = up.clone();
        async move { up.run(done_rx).await }
    });

    let fill_task = (!gap_runs.is_empty()).then(|| {
        let (ffmpeg, dir, runs) = (p.ffmpeg.clone(), dir.clone(), gap_runs);
        tokio::spawn(async move {
            for (i, (from, to)) in runs.into_iter().enumerate() {
                tracing::info!("riempio i segmenti {from}..{to} (tempo senza registrazione)");
                let mut done = false;
                for attempt in 1..=3 {
                    match crate::capture::fill_gap(
                        &ffmpeg,
                        &dir,
                        gap_base + i as u32,
                        from,
                        to,
                        has_audio,
                    )
                    .await
                    {
                        Ok(()) => {
                            done = true;
                            break;
                        }
                        Err(e) => {
                            tracing::warn!("riempimento {from}..{to}, tentativo {attempt}: {e:#}")
                        }
                    }
                }
                if !done {
                    tracing::error!(
                        "non sono riuscito a riempire {from}..{to}: il replay salta quel tratto"
                    );
                }
            }
        })
    });

    let mut stop_local: Option<f64> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut candidate: Option<(WindowSel, u32)> = None;
    let mut last_switch = Instant::now();
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    loop {
        let stop_timer = async {
            match stop_local {
                Some(s) => sleep_until_local(s).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            ev = ev_rx.recv() => {
                if let Some(Event::Stop(at)) = ev {
                    if let Some(s) = clock.server_ms_to_local_secs(at) {
                        tracing::info!("stop tra {:.2} s", s - local_now_secs());
                        stop_local = Some(s);
                        shared.update(|st| st.stopped_at_ms = Some(at));
                    }
                }
            }
            _ = stop_timer => { shared.set_phase(Phase::Stopping); cap.stop().await?; break; }
            _ = tick.tick() => {
                if let Some(e) = &engine {
                    let r = e.report();
                    if r != last_report {
                        last_report = r.clone();
                        shared.set_status(|s| {
                            s.audio = r.label;
                            s.audio_issue = r.issue;
                        });
                    }
                }

                if stop_local.is_none() && last_switch.elapsed() > Duration::from_secs(30)
                    && open_segment_age(&dir).is_some_and(|a| a > Duration::from_secs(SEGMENT_SECONDS as u64 * 5))
                {
                    tracing::warn!("nessun segmento chiuso da troppo tempo: riavvio la cattura");
                    cap.kill();
                    last_switch = Instant::now();
                }

                if stop_local.is_some() || matches!(sel, WindowSel::Monitor(_)) { continue; }
                let (s, o) = (sel.clone(), outputs.clone());
                let want = tokio::task::spawn_blocking(move || desired_source(&s, &o)).await.unwrap_or_else(|_| current.clone());
                if want == current { candidate = None; continue; }
                let n = match &candidate { Some((c, n)) if *c == want => n + 1, _ => 1 };
                candidate = Some((want.clone(), n));
                if n >= 2 && last_switch.elapsed() >= Duration::from_secs(6) {
                    tracing::info!("cambio sorgente: {current:?} -> {want:?}");
                    cap.kill();
                    generation += 1;
                    let k = next_segment(&dir).await;
                    cap = Capture::spawn(&make_cfg(want.clone(), generation, k))?;
                    current = want;
                    candidate = None;
                    last_switch = Instant::now();
                }
            }
            st = cap.wait() => {
                let ok = st?.success();

                crashes.retain(|t| t.elapsed() < Duration::from_secs(60));

                {
                    let recent = crashes.len() as u32;
                    crashes.push_back(Instant::now());
                    tracing::warn!("ffmpeg terminato (ok={ok}, {} negli ultimi 60 s): lo riavvio", recent + 1);
                    if recent >= 3 && !ok {
                        shared.set_status(|s| s.issue = Some("la cattura si interrompe di continuo: riprovo".into()));
                    }
                    tokio::time::sleep(Duration::from_secs((1u64 << recent.min(4)).min(15))).await;
                    let (s, o) = (sel.clone(), outputs.clone());
                    current = tokio::task::spawn_blocking(move || desired_source(&s, &o)).await.unwrap_or_else(|_| sel.clone());
                    generation += 1;
                    let k = next_segment(&dir).await;
                    cap = Capture::spawn(&make_cfg(current.clone(), generation, k))?;
                    last_switch = Instant::now();
                    continue;
                }
            }
            _ = cancelled(cancel) => { tracing::info!("uscita richiesta"); shared.set_phase(Phase::Stopping); cap.stop().await?; break; }
        }
    }

    shared.set_phase(Phase::Uploading);
    if let Some(f) = fill_task {
        let _ = f.await;
    }
    done_tx.send(true).ok();
    upload_task.await??;
    up.finish().await?;
    shared.set_phase(Phase::Done);
    tracing::info!("registrazione completa e caricata");
    Ok(())
}

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use relay_agent::{
    credentials, login, presets,
    session::{self, SessionHandle, SessionParams},
    settings::{LastSpeedtest, Settings, WindowChoice},
    speedtest,
    tokenstore::{KeyringStore, TokenStore},
    windows::{self, WindowInfo},
};
use relay_common::{AgentState, MatchInfo, MatchStatus, PlayerHealth};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const CLIENT_ID: &str = "relay";

const INFO_EVERY: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Player,
    HostPlayer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Player => "player",
            Role::HostPlayer => "host_player",
        }
    }
    fn is_host(self) -> bool {
        matches!(self, Role::Host | Role::HostPlayer)
    }
    pub fn plays(self) -> bool {
        matches!(self, Role::Player | Role::HostPlayer)
    }
}

#[derive(Clone)]
struct Auth {
    token: String,
    user: String,
    name: Option<String>,
}

#[derive(Deserialize, Clone)]
struct MatchView {
    #[serde(flatten)]
    info: MatchInfo,
    #[serde(default)]
    health: Vec<PlayerHealth>,
}

struct Current {
    id: String,
    role: Role,
    invite_url: Option<String>,
    handle: Option<SessionHandle>,
    info: Option<MatchView>,
    info_at: Option<Instant>,
    fetching: bool,
}

pub struct Core {
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    store: KeyringStore,
    auth: Mutex<Option<Auth>>,
    current: Mutex<Option<Current>>,
    last_error: Mutex<Option<String>>,

    pending_invite: Mutex<Option<String>>,

    update: Mutex<UpdateState>,

    ffmpeg: Mutex<FfmpegState>,
    http: reqwest::Client,
}

#[derive(Default, Clone)]
struct FfmpegState {
    installing: bool,

    progress: f32,
    error: Option<String>,
}

#[derive(Default, Clone)]
struct UpdateState {
    release: Option<crate::updater::Release>,

    ready: Option<PathBuf>,
    checking: bool,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct Created {
    pub id: String,
    pub invite_url: String,
}

#[derive(Serialize)]
pub struct Joined {
    pub id: String,
}

#[derive(Serialize)]
pub struct RecentMatch {
    id: String,

    name: Option<String>,
    players: Vec<String>,
    status: &'static str,
    created_at: u64,
    host: bool,
}

type Res<T> = Result<T, String>;

fn friendly(e: impl std::fmt::Display) -> String {
    let s = format!("{e:#}");

    s.replace("esegui `relay-agent login`", "accedi di nuovo")
}

fn explain(status: u16, body: &str) -> String {
    let b = body.trim();
    match (status, b) {
        (409, "not all players are connected") | (409, "not all players are ready") => {
            "Non tutti i giocatori sono pronti.".into()
        }
        (409, "no players yet") => "Nessun giocatore: condividi il link di invito.".into(),
        (409, "already started") | (409, "match already started") => {
            "La partita e' gia' iniziata.".into()
        }
        (409, "match is full") => "La partita e' al completo.".into(),
        (409, "match ended") => "La partita e' finita.".into(),
        (404, _) => "Partita non trovata o link non valido.".into(),
        (403, _) => "Non hai i permessi per questa azione.".into(),
        (401, _) => "Sessione scaduta: accedi di nuovo.".into(),
        (503, _) | (502, _) | (504, _) => "Il server non e' raggiungibile.".into(),
        (_, "") => format!("Errore del server ({status})."),
        _ => format!("Errore del server ({status}): {b}"),
    }
}

fn iso_now() -> String {
    iso_from_secs(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64,
    )
}

fn iso_from_secs(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn agent_state_str(s: AgentState) -> &'static str {
    match s {
        AgentState::Idle => "idle",
        AgentState::Waiting => "waiting",
        AgentState::Recording => "recording",
        AgentState::Uploading => "uploading",
        AgentState::Error => "error",
    }
}

impl Core {
    pub fn new(settings_path: PathBuf) -> Arc<Self> {
        let settings = Settings::load(&settings_path);
        Arc::new(Self {
            settings_path,
            settings: Mutex::new(settings),
            store: KeyringStore::new(),
            auth: Mutex::new(None),
            current: Mutex::new(None),
            last_error: Mutex::new(None),
            pending_invite: Mutex::new(None),
            update: Mutex::new(UpdateState::default()),
            ffmpeg: Mutex::new(FfmpegState::default()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("client http"),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn work_dir(&self) -> PathBuf {
        self.settings_path
            .parent()
            .map(|p| p.join("work"))
            .unwrap_or_else(|| PathBuf::from("./relay-work"))
    }

    pub async fn check_update(&self) {
        if !self.is_idle() {
            return;
        }
        {
            let mut u = self.update.lock().unwrap();
            if u.checking {
                return;
            }
            u.checking = true;
        }
        let server = self.settings().server_url;
        let found: Result<Option<(crate::updater::Release, PathBuf)>, String> = async {
            match crate::updater::fetch_latest(&self.http, &server, "/api/app/latest").await? {
                Some(r)
                    if crate::updater::is_newer(crate::updater::current_version(), &r.version) =>
                {
                    let dir = self
                        .work_dir()
                        .parent()
                        .map(|p| p.join("update"))
                        .unwrap_or_else(|| PathBuf::from("update"));
                    let dest = dir.join(format!("relay-app-{}.exe", r.version));
                    if !dest.exists() {
                        crate::updater::download(
                            &self.http,
                            &server,
                            "/api/app/download",
                            crate::updater::KIND_APP,
                            &r,
                            crate::updater::MAX_APP_BYTES,
                            &dest,
                            &|_, _| {},
                        )
                        .await?;
                    }
                    Ok(Some((r, dest)))
                }
                _ => Ok(None),
            }
        }
        .await;
        let mut u = self.update.lock().unwrap();
        u.checking = false;
        match found {
            Ok(Some((r, dest))) => {
                tracing::info!("aggiornamento {} pronto", r.version);
                u.release = Some(r);
                u.ready = Some(dest);
                u.error = None;
            }
            Ok(None) => {
                u.release = None;
                u.ready = None;
                u.error = None;
            }
            Err(e) => {
                tracing::warn!("controllo aggiornamenti: {e}");
                u.error = Some(e);
            }
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.work_dir()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn ffmpeg_path(&self) -> PathBuf {
        crate::ffmpeg::exe(&self.data_dir())
    }

    pub fn ffmpeg_ready(&self) -> bool {
        crate::ffmpeg::installed_version(&self.data_dir()).is_some()
    }

    pub async fn ensure_ffmpeg(&self) {
        {
            let mut f = self.ffmpeg.lock().unwrap();
            if f.installing {
                return;
            }
            f.installing = true;
            f.progress = 0.0;
        }
        let base = self.data_dir();
        let server = self.settings().server_url;
        let installed = crate::ffmpeg::installed_version(&base);
        let result: Result<(), String> = async {
            match crate::ffmpeg::fetch_latest(&self.http, &server).await {
                Ok(Some(rel)) => {
                    let newer = installed
                        .as_deref()
                        .is_none_or(|v| crate::updater::is_newer(v, &rel.version));
                    if newer && (installed.is_none() || self.is_idle()) {
                        tracing::info!("scarico ffmpeg {}", rel.version);
                        let total = rel.size.max(1);
                        let progress = |done: u64, _| {
                            self.ffmpeg.lock().unwrap().progress =
                                (done as f64 / total as f64) as f32;
                        };
                        crate::ffmpeg::install(&self.http, &server, &rel, &base, &progress).await?;
                        tracing::info!("ffmpeg {} installato", rel.version);
                    }
                    Ok(())
                }
                Ok(None) if installed.is_none() => {
                    Err("il server non ha ancora un ffmpeg da scaricare".into())
                }
                Ok(None) => Ok(()),
                Err(e) if installed.is_none() => Err(e),
                Err(_) => Ok(()),
            }
        }
        .await;
        let mut f = self.ffmpeg.lock().unwrap();
        f.installing = false;
        f.error = result.err();
        if let Some(e) = &f.error {
            tracing::warn!("ffmpeg: {e}");
        }
    }

    pub async fn check_update_now(&self) -> Value {
        if !self.is_idle() {
            return json!({ "state": "busy" });
        }
        self.check_update().await;
        let u = self.update.lock().unwrap();
        match (&u.release, &u.ready, &u.error) {
            (Some(r), Some(_), _) => json!({ "state": "ready", "version": r.version }),
            (_, _, Some(e)) => json!({ "state": "error", "error": e }),
            _ => json!({ "state": "uptodate", "version": crate::updater::current_version() }),
        }
    }

    pub fn update_ready(&self) -> bool {
        self.update.lock().unwrap().ready.is_some()
    }

    pub fn is_idle(&self) -> bool {
        self.current.lock().unwrap().is_none()
    }

    pub fn apply_update(&self) -> Res<()> {
        if !self.is_idle() {
            return Err("Esci prima dalla partita: l'app si riavvia per aggiornarsi.".into());
        }
        let path = self
            .update
            .lock()
            .unwrap()
            .ready
            .clone()
            .ok_or("Nessun aggiornamento pronto.")?;
        crate::updater::install(&path)
    }

    pub async fn init(self: &Arc<Self>) {
        let s = self.settings();
        if !s.server_url.is_empty() && self.store.get().ok().flatten().is_some() {
            let _ = self.reauth().await;
        }
    }

    async fn reauth(&self) -> Res<()> {
        let s = self.settings();
        if s.server_url.is_empty() {
            return Err("Imposta prima il server nelle impostazioni.".into());
        }
        let ses = credentials::resolve_session(&s.server_url, &s.auth_url, CLIENT_ID, &self.store)
            .await
            .map_err(friendly)?;
        *self.auth.lock().unwrap() = Some(Auth {
            token: ses.token,
            user: ses.user,
            name: ses.name,
        });
        Ok(())
    }

    fn auth(&self) -> Res<Auth> {
        self.auth
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "Accedi per continuare.".to_string())
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Res<reqwest::Response> {
        let base = self.settings().server_url;
        if base.is_empty() {
            return Err("Imposta prima il server nelle impostazioni.".into());
        }
        for attempt in 0..2 {
            let a = self.auth()?;
            let mut req = self
                .http
                .request(method.clone(), format!("{base}{path}"))
                .bearer_auth(&a.token);
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req
                .send()
                .await
                .map_err(|_| "Il server non e' raggiungibile.".to_string())?;
            if resp.status().as_u16() == 401 && attempt == 0 {
                self.reauth().await?;
                continue;
            }
            return Ok(resp);
        }
        Err("Sessione scaduta: accedi di nuovo.".into())
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Res<T> {
        let resp = self.call(method, path, body).await?;
        let status = resp.status();
        if !status.is_success() {
            let t = resp.text().await.unwrap_or_default();
            return Err(explain(status.as_u16(), &t));
        }
        resp.json::<T>()
            .await
            .map_err(|e| format!("Risposta del server non valida: {e}"))
    }

    pub async fn login(&self) -> Res<()> {
        let s = self.settings();
        if s.server_url.is_empty() {
            return Err("Imposta prima il server nelle impostazioni.".into());
        }
        let t = login::login(&s.auth_url, CLIENT_ID, login::open_browser)
            .await
            .map_err(friendly)?;
        self.store.set(&t.refresh_token).map_err(friendly)?;
        self.reauth().await
    }

    pub async fn logout(&self) -> Res<()> {
        self.leave_match().await;
        let s = self.settings();
        let r = credentials::logout(&s.auth_url, &self.store).await;
        *self.auth.lock().unwrap() = None;
        r.map_err(friendly)
    }

    pub fn save_settings(&self, mut s: Settings) -> Res<Settings> {
        s.sanitize();
        s.save(&self.settings_path)
            .map_err(|e| format!("Impossibile salvare le impostazioni: {e}"))?;
        *self.settings.lock().unwrap() = s.clone();
        Ok(s)
    }

    pub async fn list_windows(&self) -> Vec<WindowInfo> {
        let ffmpeg = self.ffmpeg_path();
        let (wins, mons) = tokio::join!(
            tokio::task::spawn_blocking(windows::list_windows),
            relay_agent::capture::list_monitors(&ffmpeg)
        );
        let mut out: Vec<WindowInfo> = mons
            .iter()
            .map(|m| WindowInfo {
                title: format!(
                    "Schermo intero: monitor {} ({}\u{d7}{})",
                    m.index + 1,
                    m.width,
                    m.height
                ),
                exe: format!("@monitor:{}", m.index),
            })
            .collect();
        out.extend(wins.unwrap_or_default());
        out
    }

    pub async fn run_speedtest(&self, on_progress: impl Fn(f64, f64)) -> Res<presets::SpeedResult> {
        let s = self.settings();
        let a = self.auth()?;
        let r = speedtest::run(&s.server_url, &a.token, Duration::from_secs(8), on_progress)
            .await
            .map_err(friendly)?;
        let mut cur = self.settings();
        cur.last_speedtest = Some(LastSpeedtest {
            upload_mbps: r.upload_mbps,
            at: iso_now(),
        });
        let _ = self.save_settings(cur);
        Ok(r)
    }

    fn params(&self, match_id: &str, plays: bool) -> Res<SessionParams> {
        if plays && !self.ffmpeg_ready() {
            return Err(
                "Sto preparando i componenti per registrare (ffmpeg): riprova tra poco.".into(),
            );
        }
        let s = self.settings();
        let a = self.auth()?;
        Ok(SessionParams {
            ffmpeg: self.ffmpeg_path(),
            server: s.server_url.clone(),
            token: a.token,
            match_id: match_id.to_string(),
            player: a.user,
            plays,

            window: if plays { s.window_sel() } else { None },
            fps: s.fps,
            bitrate_kbps: s.bitrate_kbps,
            encoder: s.encoder_choice(),
            limit_bytes_per_sec: s.effective_limit_kbps().map(|k| k as u64 * 1000 / 8),
            work_dir: self.work_dir(),
            audio: relay_agent::audio::AudioChoice {
                game: s.audio_game,
                mic: s.audio_mic,
                mic_gain: s.audio_mic_gain,
            },
        })
    }

    fn busy(&self) -> bool {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|c| c.handle.as_ref().is_some_and(|h| !h.is_finished()))
    }

    fn invite_url(&self, id: &str, code: &str) -> String {
        format!("{}/join/{id}?code={code}", self.settings().site_origin())
    }

    fn begin(&self, id: &str, role: Role, invite: Option<String>, with_session: bool) -> Res<()> {
        let handle = if with_session {
            Some(session::spawn(self.params(id, role.plays())?))
        } else {
            None
        };
        *self.last_error.lock().unwrap() = None;
        *self.current.lock().unwrap() = Some(Current {
            id: id.to_string(),
            role,
            invite_url: invite,
            handle,
            info: None,
            info_at: None,
            fetching: false,
        });
        Ok(())
    }

    pub fn set_window(&self, w: Option<WindowChoice>) -> Res<()> {
        let w = w.filter(|w| !w.exe.trim().is_empty());
        let mut s = self.settings();
        if s.window != w {
            s.window = w;
            self.save_settings(s)?;
        }
        if let Some(h) = self
            .current
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.handle.as_ref())
        {
            h.set_window(self.settings().window_sel());
        }
        Ok(())
    }

    pub fn set_audio(&self, game: bool, mic: bool, mic_gain: f32) -> Res<()> {
        let mut s = self.settings();
        if s.audio_game != game
            || s.audio_mic != mic
            || (s.audio_mic_gain - mic_gain).abs() > f32::EPSILON
        {
            s.audio_game = game;
            s.audio_mic = mic;
            s.audio_mic_gain = mic_gain;
            self.save_settings(s)?;
        }
        let mic_gain = self.settings().audio_mic_gain;
        if let Some(h) = self
            .current
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.handle.as_ref())
        {
            h.set_audio(relay_agent::audio::AudioChoice {
                game,
                mic,
                mic_gain,
            });
        }
        Ok(())
    }

    pub async fn create_match(&self, name: Option<String>) -> Res<Created> {
        if self.busy() {
            return Err("Sei gia' in una partita: esci prima.".into());
        }
        let m: MatchInfo = self
            .json(
                Method::POST,
                "/api/matches",
                Some(json!({ "play": true, "name": name })),
            )
            .await?;
        let id = m.id.to_string();
        let invite = self.invite_url(&id, m.invite_code.as_deref().unwrap_or(""));
        self.begin(&id, Role::HostPlayer, Some(invite.clone()), true)?;
        Ok(Created {
            id,
            invite_url: invite,
        })
    }

    pub async fn rename_match(&self, name: &str) -> Res<()> {
        let id = self.current_id()?;
        let m: MatchInfo = self
            .json(
                Method::POST,
                &format!("/api/matches/{id}/rename"),
                Some(json!({ "name": name })),
            )
            .await?;
        if let Some(c) = self.current.lock().unwrap().as_mut().filter(|c| c.id == id) {
            if let Some(v) = c.info.as_mut() {
                v.info.name = m.name;
            }
        }
        Ok(())
    }

    pub async fn join_match(&self, link: &str) -> Res<Joined> {
        if self.busy() {
            return Err("Sei gia' in una partita: esci prima.".into());
        }
        let (id, code) = parse_invite(link)?;
        let _m: MatchInfo = self
            .json(
                Method::POST,
                &format!("/api/matches/{id}/join?code={code}"),
                None,
            )
            .await?;
        self.begin(&id, Role::Player, None, true)?;
        Ok(Joined { id })
    }

    pub fn offer_invite(&self, link: &str) {
        if parse_invite(link).is_ok() {
            *self.pending_invite.lock().unwrap() = Some(link.trim().to_string());
        } else {
            *self.last_error.lock().unwrap() = Some("Link di invito non valido.".into());
        }
    }

    pub async fn accept_invite(&self) -> Res<Joined> {
        let link = self
            .pending_invite
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "Nessun invito in attesa.".to_string())?;
        self.join_match(&link).await
    }

    pub fn decline_invite(&self) {
        *self.pending_invite.lock().unwrap() = None;
    }

    pub async fn open_match(&self, id: &str) -> Res<()> {
        if self.busy() {
            return Err("Sei gia' in una partita: esci prima.".into());
        }
        let me = self.auth()?.user;
        let v: MatchView = self
            .json(Method::GET, &format!("/api/matches/{id}"), None)
            .await?;
        let host = v.info.coordinator == me;
        let plays = v.info.players.contains(&me);
        let role = match (host, plays) {
            (true, true) => Role::HostPlayer,
            (true, false) => Role::Host,
            (false, true) => Role::Player,
            (false, false) => return Err("Non partecipi a questa partita.".into()),
        };
        let invite = v
            .info
            .invite_code
            .as_deref()
            .map(|c| self.invite_url(id, c));

        let live = v.info.status != MatchStatus::Ended;
        self.begin(id, role, invite, live)?;
        if let Some(c) = self.current.lock().unwrap().as_mut() {
            c.info = Some(v);
            c.info_at = Some(Instant::now());
        }
        Ok(())
    }

    pub async fn leave_match(&self) {
        let cur = self.current.lock().unwrap().take();
        if let Some(Current {
            handle: Some(h), ..
        }) = cur
        {
            h.cancel();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(120), h.join()).await;
            });
        }
    }

    pub async fn host_start(&self, force: bool) -> Res<()> {
        let id = self.current_id()?;
        let q = if force { "?force=true" } else { "" };
        let _: MatchInfo = self
            .json(Method::POST, &format!("/api/matches/{id}/start{q}"), None)
            .await?;
        Ok(())
    }

    pub async fn host_stop(&self) -> Res<()> {
        let id = self.current_id()?;
        let _: MatchInfo = self
            .json(Method::POST, &format!("/api/matches/{id}/stop"), None)
            .await?;
        Ok(())
    }

    fn current_id(&self) -> Res<String> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.id.clone())
            .ok_or_else(|| "Nessuna partita aperta.".to_string())
    }

    pub async fn list_matches(&self) -> Res<Vec<RecentMatch>> {
        let me = self.auth()?.user;
        let list: Vec<MatchInfo> = self.json(Method::GET, "/api/matches", None).await?;
        Ok(list
            .into_iter()
            .take(10)
            .map(|m| RecentMatch {
                id: m.id.to_string(),
                name: m.name.clone(),
                players: m
                    .players
                    .iter()
                    .map(|p| m.names.get(p).cloned().unwrap_or_else(|| p.clone()))
                    .collect(),
                status: if m.status == MatchStatus::Ended {
                    "done"
                } else if m.started_at_ms.is_some() {
                    "recording"
                } else {
                    "waiting"
                },
                created_at: m.created_at,
                host: m.coordinator == me,
            })
            .collect())
    }

    pub async fn refresh_info(self: &Arc<Self>) {
        let id = {
            let mut g = self.current.lock().unwrap();
            let Some(c) = g.as_mut() else { return };
            if c.fetching || c.info_at.is_some_and(|t| t.elapsed() < INFO_EVERY) {
                return;
            }
            c.fetching = true;
            c.id.clone()
        };
        let r: Res<MatchView> = self
            .json(Method::GET, &format!("/api/matches/{id}"), None)
            .await;
        let mut g = self.current.lock().unwrap();
        if let Some(c) = g.as_mut().filter(|c| c.id == id) {
            c.fetching = false;
            c.info_at = Some(Instant::now());
            if let Ok(v) = r {
                c.info = Some(v);
            }
        }
    }

    pub fn snapshot(&self) -> Value {
        let auth = self.auth.lock().unwrap().clone();
        let settings = self.settings();
        let cur = self.current.lock().unwrap();
        let mut connection = "connected";
        let mut error = self.last_error.lock().unwrap().clone();

        let match_v = cur.as_ref().map(|c| {
            let st = c.handle.as_ref().map(|h| h.state.borrow().clone());
            if let Some(s) = &st {
                connection = match s.connection {
                    session::Connection::Connected => "connected",
                    _ => "reconnecting",
                };
                if error.is_none() {
                    error = s.error.clone();
                }
            }
            let info = c.info.as_ref();
            let names = info.map(|i| &i.info.names);
            let name_of = |id: &str| names.and_then(|n| n.get(id)).cloned().unwrap_or_else(|| id.to_string());
            let player_ids: Vec<String> = info.map(|i| i.info.players.clone()).unwrap_or_default();

            let health: Vec<PlayerHealth> = match &st {
                Some(s) if !s.health.is_empty() => s.health.clone(),
                _ => info.map(|i| i.health.clone()).unwrap_or_default(),
            };
            let players: Vec<Value> = player_ids
                .iter()
                .map(|id| {
                    let h = health.iter().find(|h| &h.id == id);
                    let ps = h.and_then(|h| h.status.as_ref());
                    json!({
                        "id": id,
                        "name": name_of(id),
                        "is_host": info.is_some_and(|i| &i.info.coordinator == id),
                        "connected": h.is_some_and(|h| h.connected),
                        "ready": h.is_some_and(|h| h.ready),
                        "state": ps.map(|s| agent_state_str(s.state)).unwrap_or("idle"),
                        "window_found": ps.and_then(|s| s.window_found),

                        "window": ps.and_then(|s| s.window.clone()),

                        "audio": ps.and_then(|s| s.audio.clone()),
                        "audio_issue": ps.and_then(|s| s.audio_issue.clone()),
                        "backlog": ps.map(|s| s.backlog).unwrap_or(0),
                        "upload_kbps": ps.map(|s| s.upload_kbps).unwrap_or(0),
                        "rtt_ms": ps.and_then(|s| s.rtt_ms),
                        "offset_ms": ps.and_then(|s| s.offset_ms),
                        "issue": h.and_then(|h| h.issue.clone()).or_else(|| (h.is_none()).then(|| "non connesso".to_string())),
                    })
                })
                .collect();

            let started = st.as_ref().and_then(|s| s.started_at_ms).or(info.and_then(|i| i.info.started_at_ms));
            let stopped = st.as_ref().and_then(|s| s.stopped_at_ms).or(info.and_then(|i| i.info.stopped_at_ms));
            let ended = info.is_some_and(|i| i.info.status == MatchStatus::Ended);

            let phase = match &st {
                Some(s) => serde_json::to_value(s.phase).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
                None if ended => "done".into(),
                None if stopped.is_some() => "uploading".into(),
                None if started.is_some() => "recording".into(),
                None => "lobby".into(),
            };

            let blockers: Vec<String> = if player_ids.is_empty() {
                vec!["Nessun giocatore: condividi il link di invito".into()]
            } else {
                players
                    .iter()
                    .filter(|p| p["ready"] != json!(true))
                    .map(|p| format!("{}: {}", p["name"].as_str().unwrap_or("?"), p["issue"].as_str().unwrap_or("non pronto")))
                    .collect()
            };
            let can_start = c.role.is_host() && phase == "lobby" && blockers.is_empty();
            let me = st.as_ref().and_then(|s| s.me.as_ref()).map(|m| {
                json!({
                    "window_found": m.window_found.unwrap_or(false),
                    "encoder": m.encoder,
                    "backlog": m.backlog,
                    "upload_kbps": m.upload_kbps,
                    "rtt_ms": m.rtt_ms,
                    "offset_ms": m.offset_ms,
                    "audio": m.audio,
                    "audio_issue": m.audio_issue,
                })
            });

            json!({
                "id": c.id,
                "name": info.and_then(|i| i.info.name.clone()),
                "role": c.role.as_str(),
                "phase": phase,
                "invite_url": if c.role.is_host() { c.invite_url.clone() } else { None },
                "replay_url": format!("{}/matches/{}", settings.site_origin(), c.id),
                "started_at_ms": started,
                "stopped_at_ms": stopped,
                "clock_offset_ms": st.as_ref().and_then(|s| s.clock_offset_ms),
                "players": players,
                "can_start": can_start,
                "start_blockers": if c.role.is_host() && phase == "lobby" { blockers } else { vec![] },
                "me": me,
            })
        });

        let ffmpeg_v = {
            let f = self.ffmpeg.lock().unwrap();
            let state = if self.ffmpeg_ready() {
                "ready"
            } else if f.installing {
                "installing"
            } else if f.error.is_some() {
                "error"
            } else {
                "missing"
            };
            json!({ "state": state, "progress": f.progress, "error": f.error })
        };
        let update_v = {
            let u = self.update.lock().unwrap();
            u.release.as_ref().map(
                |r| json!({ "version": r.version, "notes": r.notes, "ready": u.ready.is_some() }),
            )
        };
        json!({
            "auth": {
                "logged_in": auth.is_some(),
                "name": auth.as_ref().and_then(|a| a.name.clone()),
                "user": auth.as_ref().map(|a| a.user.clone()),
            },
            "connection": connection,
            "error": error,
            "version": crate::updater::current_version(),
            "update": update_v,
            "ffmpeg": ffmpeg_v,
            "match": match_v,
            "invite_request": self.pending_invite.lock().unwrap().as_deref().and_then(|l| parse_invite(l).ok()).map(|(id, _)| json!({ "id": id })),
        })
    }

    pub fn overlay_wanted(&self, snap: &Value) -> bool {
        let m = &snap["match"];
        self.settings().overlay
            && m["phase"] == json!("recording")
            && matches!(m["role"].as_str(), Some("player") | Some("host_player"))
    }
}

pub fn parse_invite(link: &str) -> Res<(String, String)> {
    let bad = || "Link di invito non valido.".to_string();
    let u = reqwest::Url::parse(link.trim()).map_err(|_| bad())?;
    let mut seg = u.path_segments().ok_or_else(bad)?;
    let id = if u.scheme() == "relay" {
        if u.host_str() != Some("join") {
            return Err(bad());
        }
        seg.next().ok_or_else(bad)?
    } else {
        let (Some("join"), Some(id)) = (seg.next(), seg.next()) else {
            return Err(bad());
        };
        id
    };
    let code = u
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(bad)?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if !ok(id) || !ok(&code) {
        return Err(bad());
    }
    Ok((id.to_string(), code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_links() {
        let (id, code) = parse_invite(
            "https://relay.example.com/join/5f65d77b-7e1a-4e1c-89e5-b13ba6234f07?code=abc123",
        )
        .unwrap();
        assert_eq!(
            (id.as_str(), code.as_str()),
            ("5f65d77b-7e1a-4e1c-89e5-b13ba6234f07", "abc123")
        );
        assert!(parse_invite("  https://x/join/a1?code=b2  ").is_ok());
        assert!(parse_invite("https://x/matches/a1?code=b2").is_err());
        assert!(parse_invite("https://x/join/a1").is_err());
        assert!(parse_invite("https://x/join/../etc?code=b2").is_err());
        assert!(parse_invite("non un link").is_err());

        let (id, code) = parse_invite("relay://join/5f65d77b-7e1a?code=abc123").unwrap();
        assert_eq!((id.as_str(), code.as_str()), ("5f65d77b-7e1a", "abc123"));
        assert!(parse_invite("relay://altro/5f65?code=abc").is_err());
        assert!(parse_invite("relay://join/a1").is_err());
    }

    #[test]
    fn server_errors_are_explained() {
        assert_eq!(
            explain(409, "not all players are ready"),
            "Non tutti i giocatori sono pronti."
        );
        assert_eq!(explain(404, ""), "Partita non trovata o link non valido.");
        assert!(explain(500, "boom").contains("boom"));
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_from_secs(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_secs(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(iso_from_secs(1_789_819_200), "2026-09-19T12:00:00Z");
        assert_eq!(iso_from_secs(951_782_400), "2000-02-29T00:00:00Z");
        let now = iso_now();
        assert!(
            now.len() == 20 && now.starts_with("20") && now.ends_with('Z'),
            "{now}"
        );
    }
}

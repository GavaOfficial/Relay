use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use relay_agent::{
    capture::{detect_encoder, Capture, CaptureConfig, Encoder, WindowSel},
    credentials, login, priority, session,
    tokenstore::{KeyringStore, TokenStore},
    uploader::{UploadConfig, Uploader},
};
use tokio::sync::watch;

#[derive(Parser)]
#[command(
    name = "relay-agent",
    about = "Registra una finestra di gioco e la invia al server Relay"
)]
struct Cli {
    #[arg(long, global = true, default_value = "ffmpeg", env = "RELAY_FFMPEG")]
    ffmpeg: PathBuf,

    #[arg(
        long,
        global = true,
        default_value = "https://auth.gavatech.org",
        env = "RELAY_AUTH_URL"
    )]
    auth_url: String,

    #[arg(long, global = true, default_value = "relay")]
    client_id: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Encoders,

    Windows,

    Speedtest {
        #[arg(long, env = "RELAY_SERVER")]
        server: String,

        #[arg(long, env = "RELAY_TOKEN")]
        token: Option<String>,
    },

    Login,

    Logout,

    Whoami {
        #[arg(long, env = "RELAY_SERVER")]
        server: String,
    },

    Join(Common),

    Record(RecordArgs),
}

#[derive(Args, Clone)]
struct Common {
    #[arg(long, env = "RELAY_SERVER")]
    server: String,

    #[arg(long, env = "RELAY_TOKEN")]
    token: Option<String>,
    #[arg(long)]
    match_id: String,

    #[arg(long)]
    player: Option<String>,

    #[arg(long, conflicts_with = "window_title")]
    window_exe: Option<String>,

    #[arg(long)]
    window_title: Option<String>,

    #[arg(long, conflicts_with_all = ["window_exe", "window_title"])]
    monitor: Option<u32>,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    #[arg(long, default_value_t = 5000)]
    bitrate_kbps: u32,

    #[arg(long)]
    limit_kbps: Option<u64>,

    #[arg(long, default_value = "auto")]
    encoder: String,
    #[arg(long, default_value = "./relay-work")]
    work_dir: PathBuf,

    #[arg(long)]
    audio_game: bool,

    #[arg(long)]
    audio_mic: bool,

    #[arg(long, default_value_t = relay_agent::audio::DEFAULT_MIC_GAIN)]
    mic_gain: f32,
}

#[derive(Args)]
struct RecordArgs {
    #[command(flatten)]
    common: Common,

    #[arg(long)]
    duration: Option<u32>,

    #[arg(long)]
    stop_after: Option<u64>,
}

impl Common {
    fn window(&self) -> Result<WindowSel> {
        match (&self.window_exe, &self.window_title, self.monitor) {
            (Some(e), None, None) => Ok(WindowSel::Exe(e.clone())),
            (None, Some(t), None) => Ok(WindowSel::TitleRegex(t.clone())),
            (None, None, Some(n)) => Ok(WindowSel::Monitor(n)),
            _ => bail!("indica --window-exe, --window-title oppure --monitor"),
        }
    }

    fn encoder(&self) -> Result<Option<Encoder>> {
        if self.encoder == "auto" {
            return Ok(None);
        }
        Encoder::parse(&self.encoder)
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("encoder sconosciuto: {}", self.encoder))
    }

    fn limit_bytes_per_sec(&self) -> Option<u64> {
        self.limit_kbps.map(|k| k * 1000 / 8)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "relay_agent=info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Encoders => {
            println!("{}", detect_encoder(&cli.ffmpeg).await?.name());
            Ok(())
        }
        Cmd::Windows => {
            let monitors = relay_agent::capture::list_monitors(&cli.ffmpeg).await;
            for m in &monitors {
                println!(
                    "--monitor {:<18} schermo intero {}x{}",
                    m.index, m.width, m.height
                );
            }
            let outputs: Vec<(u32, u32, u32)> = monitors
                .iter()
                .map(|m| (m.index, m.width, m.height))
                .collect();
            let displays = relay_agent::windows::display_rects();
            for w in relay_agent::windows::list_windows() {
                let full = relay_agent::windows::fullscreen_screen(&WindowSel::Exe(w.exe.clone()))
                    .and_then(|s| relay_agent::windows::output_index(s, &displays, &outputs))
                    .map(|i| format!("  [a schermo intero: monitor {i}]"))
                    .unwrap_or_default();
                println!("{:<28} {}{}", w.exe, w.title, full);
            }
            Ok(())
        }
        Cmd::Speedtest { server, token } => {
            let s = match token {
                Some(t) => credentials::session_from_token(&server, &t).await?,
                None => {
                    credentials::resolve_session(
                        &server,
                        &cli.auth_url,
                        &cli.client_id,
                        &KeyringStore::new(),
                    )
                    .await?
                }
            };
            println!("Misuro l'upload verso {server} (circa 8 secondi)...");
            let r = relay_agent::speedtest::run(
                &server,
                &s.token,
                std::time::Duration::from_secs(8),
                |_, _| {},
            )
            .await?;
            println!("Upload: {:.1} Mbit/s", r.upload_mbps);
            println!(
                "Limite di banda consigliato: {} kbit/s (meta' dell'upload)",
                r.limit_kbps
            );
            for p in &r.presets {
                println!(
                    "  {} {:<9} {} fps {:>5} kbit/s {}",
                    if p.id == r.recommended { ">" } else { " " },
                    p.label,
                    p.fps,
                    p.bitrate_kbps,
                    if p.fits { "" } else { "(troppo pesante)" }
                );
            }
            Ok(())
        }
        Cmd::Login => {
            let t = login::login(&cli.auth_url, &cli.client_id, login::open_browser).await?;
            KeyringStore::new().set(&t.refresh_token)?;
            println!("Accesso eseguito. Ora puoi usare `join` e `record` senza --token.");
            Ok(())
        }
        Cmd::Logout => {
            credentials::logout(&cli.auth_url, &KeyringStore::new()).await?;
            println!("Uscito.");
            Ok(())
        }
        Cmd::Whoami { server } => {
            let s = credentials::resolve_session(
                &server,
                &cli.auth_url,
                &cli.client_id,
                &KeyringStore::new(),
            )
            .await?;
            println!("{} ({})", s.name.as_deref().unwrap_or("-"), s.user);
            Ok(())
        }
        Cmd::Join(c) => {
            let (token, player) = resolve_identity(&c, &cli.auth_url, &cli.client_id).await?;
            if c.limit_kbps.is_none() {
                tracing::warn!("nessun --limit-kbps: l'upload potrebbe saturare la connessione e alzare il ping");
            }
            let audio = relay_agent::audio::AudioChoice {
                game: c.audio_game,
                mic: c.audio_mic,
                mic_gain: c.mic_gain,
            };
            session::run(session::SessionParams {
                audio,
                ffmpeg: cli.ffmpeg,
                plays: true,
                window: Some(c.window()?),
                encoder: c.encoder()?,
                limit_bytes_per_sec: c.limit_bytes_per_sec(),
                server: c.server,
                token,
                match_id: c.match_id,
                player,
                fps: c.fps,
                bitrate_kbps: c.bitrate_kbps,
                work_dir: c.work_dir,
            })
            .await
        }
        Cmd::Record(a) => {
            let (token, player) =
                resolve_identity(&a.common, &cli.auth_url, &cli.client_id).await?;
            record(cli.ffmpeg, a, token, player).await
        }
    }
}

async fn resolve_identity(c: &Common, auth_url: &str, client_id: &str) -> Result<(String, String)> {
    let s = match &c.token {
        Some(t) => credentials::session_from_token(&c.server, t).await?,
        None => {
            credentials::resolve_session(&c.server, auth_url, client_id, &KeyringStore::new())
                .await?
        }
    };
    if let Some(p) = &c.player {
        if *p != s.user {
            bail!("--player {p} non coincide con l'utente connesso ({}): il server lo rifiuterebbe (403)", s.user);
        }
    }
    tracing::info!("utente: {} ({})", s.name.as_deref().unwrap_or("-"), s.user);
    Ok((s.token, s.user))
}

async fn record(ffmpeg: PathBuf, a: RecordArgs, token: String, player: String) -> Result<()> {
    let c = a.common;
    let window = c.window()?;
    let encoder = match c.encoder()? {
        Some(e) => e,
        None => detect_encoder(&ffmpeg).await?,
    };
    tracing::info!("encoder: {}", encoder.name());
    if c.limit_kbps.is_none() {
        tracing::warn!(
            "nessun --limit-kbps: l'upload potrebbe saturare la connessione e alzare il ping"
        );
    }

    let dir = c.work_dir.join(&c.match_id);
    if dir.join("out.m3u8").exists() {
        bail!(
            "{} contiene gia' una registrazione: spostala o cancellala",
            dir.display()
        );
    }

    priority::set_low_priority();

    let up = std::sync::Arc::new(Uploader::new(UploadConfig {
        base_url: c.server.clone(),
        token,
        match_id: c.match_id.clone(),
        player,
        dir: dir.clone(),
        limit_bytes_per_sec: c.limit_bytes_per_sec(),
    })?);

    let engine = (c.audio_game || c.audio_mic).then(|| {
        let pid = relay_agent::windows::find_pid(&window);
        relay_agent::audio::AudioEngine::start(
            relay_agent::audio::AudioChoice {
                game: c.audio_game,
                mic: c.audio_mic,
                mic_gain: c.mic_gain,
            },
            pid,
        )
    });
    if let Some(e) = &engine {
        tracing::info!("audio: {:?}", e.report());
    }
    let audio_port = engine
        .as_ref()
        .filter(|e| e.has_audio())
        .and_then(|e| e.attach(relay_agent::clock::local_now_secs()).ok());

    let mut cap = Capture::spawn(&CaptureConfig {
        ffmpeg,
        window,
        fps: c.fps,
        bitrate_kbps: c.bitrate_kbps,
        dir,
        encoder,
        duration_secs: a.duration,
        origin_unix_secs: None,
        start_segment: 0,
        generation: 0,
        audio_port,
    })?;

    let (done_tx, done_rx) = watch::channel(false);
    let upload_task = tokio::spawn({
        let up = up.clone();
        async move { up.run(done_rx).await }
    });

    let stop_after = async {
        match a.stop_after {
            Some(s) => tokio::time::sleep(std::time::Duration::from_secs(s)).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        st = cap.wait() => {
            let st = st?;
            if !st.success() {
                tracing::error!("ffmpeg terminato con {st}: controlla che la finestra esista");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("stop richiesto, chiudo la registrazione");
            cap.stop().await?;
        }
        _ = stop_after => {
            tracing::info!("stop-after: chiudo la registrazione");
            cap.stop().await?;
        }
    }
    done_tx.send(true).ok();

    upload_task.await??;
    up.finish().await?;
    tracing::info!("registrazione completa e caricata");
    Ok(())
}

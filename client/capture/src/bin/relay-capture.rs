//! Motore di cattura: usa `libobs` (Game Capture, con ripiego sul monitor) per registrare a
//! segmenti da 4 secondi. Sostituisce ffmpeg per la cattura video sul PC di chi gioca; il
//! server continua a usare ffmpeg per unire e convertire i video a fine partita.
//!
//! Uso: `relay-capture --probe` (elenca encoder/monitor/finestre disponibili, poi esce) oppure
//! `relay-capture --config <file.json>` (registra secondo `ipc::RecordConfig`, finche' non
//! riceve `q\n` su stdin o viene terminato).

use std::{
    io::BufRead,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use libobs_simple::sources::windows::{
    GameCaptureSourceBuilder, MonitorCaptureSourceBuilder, ObsGameCaptureMode, ObsHookRate,
};
use libobs_window_helper::{get_all_windows, WindowSearchMode};
use libobs_wrapper::{
    context::ObsContext,
    data::{output::ObsOutputTrait, ObsDataSetters},
    encoders::{ObsAudioEncoderType, ObsContextEncoders, ObsVideoEncoderType},
    scenes::SceneItemTrait,
    sources::ObsSourceBuilder,
    utils::{AudioEncoderInfo, OutputInfo, StartupInfo, VideoEncoderInfo},
};
use relay_capture::ipc::{EncoderChoice, Event, MonitorInfo, RecordConfig, Source, WindowInfo};

fn emit(ev: &Event) {
    print!("{}", relay_capture::ipc::encode(ev));
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn main() {
    // niente finestra della console: l'app la lancia in background
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }

    let args: Vec<String> = std::env::args().collect();
    let result = if args.iter().any(|a| a == "--probe") {
        run_probe()
    } else if let Some(i) = args.iter().position(|a| a == "--config") {
        let path = args.get(i + 1).expect("--config richiede un percorso");
        let text = std::fs::read_to_string(path).expect("configurazione non leggibile");
        let cfg: RecordConfig = serde_json::from_str(&text).expect("configurazione non valida");
        run_record(cfg)
    } else {
        eprintln!("uso: relay-capture --probe | --config <file.json>");
        std::process::exit(2);
    };

    if let Err(e) = result {
        emit(&Event::Error(format!("{e:#}")));
        std::process::exit(1);
    }
}

fn video_encoders(context: &ObsContext) -> Vec<libobs_wrapper::encoders::ObsVideoEncoderType> {
    context
        .available_video_encoders()
        .map(|v| v.into_iter().map(|e| e.get_encoder_id().clone()).collect())
        .unwrap_or_default()
}

fn run_probe() -> Result<()> {
    let context = ObsContext::new(StartupInfo::default()).context("avvio di OBS")?;
    let encoders: Vec<String> = video_encoders(&context)
        .into_iter()
        .map(|e| libobs_wrapper::utils::ObsString::from(e).to_string())
        .collect();
    let monitors = MonitorCaptureSourceBuilder::get_monitors()
        .map(|v| {
            v.into_iter()
                .enumerate()
                .map(|(i, m)| MonitorInfo {
                    index: i as u32,
                    width: m.0.width,
                    height: m.0.height,
                })
                .collect()
        })
        .unwrap_or_default();
    let windows = get_all_windows(WindowSearchMode::ExcludeMinimized)
        .map(|v| {
            v.into_iter()
                .filter(|w| w.is_game)
                .map(|w| WindowInfo {
                    exe: exe_name(&w.full_exe),
                    title: w.title.unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    emit(&Event::Ready {
        obs_version: context.get_version().unwrap_or_default(),
    });
    emit(&Event::Probed {
        encoders,
        monitors,
        windows,
    });
    Ok(())
}

fn exe_name(full_path: &str) -> String {
    full_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(full_path)
        .to_string()
}

/// Ordine di preferenza degli encoder video: GPU dedicata prima, processore per ultimo.
fn pick_video_encoder(
    context: &ObsContext,
    choice: EncoderChoice,
) -> Result<(ObsVideoEncoderType, &'static str)> {
    let available = video_encoders(context);
    let candidates: &[(ObsVideoEncoderType, &str, &str)] = &[
        (
            ObsVideoEncoderType::OBS_NVENC_H264_TEX,
            "obs_nvenc_h264_tex",
            "nvenc",
        ),
        (
            ObsVideoEncoderType::H264_TEXTURE_AMF,
            "h264_texture_amf",
            "amf",
        ),
        (ObsVideoEncoderType::OBS_QSV11, "obs_qsv11", "qsv"),
        (ObsVideoEncoderType::OBS_X264, "obs_x264", "x264"),
    ];
    let wanted = match choice {
        EncoderChoice::Auto => None,
        EncoderChoice::Nvenc => Some("nvenc"),
        EncoderChoice::Amf => Some("amf"),
        EncoderChoice::Qsv => Some("qsv"),
        EncoderChoice::X264 => Some("x264"),
    };
    for (ty, _id, name) in candidates {
        if wanted.is_some_and(|w| w != *name) {
            continue;
        }
        if available.iter().any(|a| a == ty) {
            return Ok((ty.clone(), name));
        }
    }
    if wanted.is_some() {
        bail!("l'encoder richiesto non e' disponibile su questo PC");
    }
    bail!("nessun encoder H.264 funzionante (provati nvenc, amf, qsv, x264)");
}

fn sleep_until(unix_secs: f64) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let wait = unix_secs - now;
    if wait > 0.0 {
        thread::sleep(Duration::from_secs_f64(wait));
    }
}

fn run_record(cfg: RecordConfig) -> Result<()> {
    std::fs::create_dir_all(&cfg.dir).context("creazione della cartella di lavoro")?;
    let mut context = ObsContext::new(StartupInfo::default()).context("avvio di OBS")?;
    let mut scene = context
        .scene("relay", Some(0))
        .context("creazione della scena")?;

    let mut window: Option<WindowInfo> = None;
    if let Source::Window { exe } = &cfg.source {
        window = get_all_windows(WindowSearchMode::ExcludeMinimized)
            .unwrap_or_default()
            .into_iter()
            .find(|w| exe_name(&w.full_exe).eq_ignore_ascii_case(exe))
            .map(|w| WindowInfo {
                exe: exe_name(&w.full_exe),
                title: w.title.clone().unwrap_or_default(),
            });
    }

    let source_label = if window.is_some() { "game" } else { "monitor" };
    if window.is_none() && matches!(cfg.source, Source::Window { .. }) {
        emit(&Event::Warning(
            "finestra non trovata: registro lo schermo".into(),
        ));
    }

    if let Some(w) = &window {
        let raw = get_all_windows(WindowSearchMode::ExcludeMinimized)
            .unwrap_or_default()
            .into_iter()
            .find(|x| exe_name(&x.full_exe).eq_ignore_ascii_case(&w.exe))
            .ok_or_else(|| anyhow!("finestra sparita durante l'avvio"))?;
        context
            .source_builder::<GameCaptureSourceBuilder, _>("Gioco")?
            .set_capture_mode(ObsGameCaptureMode::CaptureSpecificWindow)
            .set_window(&raw)
            .set_hook_rate(ObsHookRate::Fast)
            .set_anti_cheat_hook(true)
            .add_to_scene(&mut scene)?;
    } else {
        let idx = match &cfg.source {
            Source::Monitor { index } => *index,
            Source::Window { .. } => cfg.fallback_monitor.unwrap_or(0),
        };
        let monitors = MonitorCaptureSourceBuilder::get_monitors().context("elenco dei monitor")?;
        let monitor = monitors
            .get(idx as usize)
            .or_else(|| monitors.first())
            .ok_or_else(|| anyhow!("nessun monitor trovato"))?;
        let item = context
            .source_builder::<MonitorCaptureSourceBuilder, _>("Schermo")?
            .set_monitor(monitor)
            .add_to_scene(&mut scene)?;
        item.fit_source_to_screen()?;
    }
    scene.set_to_channel(0)?;

    let (video_ty, encoder_name) = pick_video_encoder(&context, cfg.encoder)?;

    if let Some(t) = cfg.origin_unix_secs {
        sleep_until(t);
    }

    let dir = cfg.dir.clone();
    let mut settings = context.data()?;
    settings.set_string("path", dir.join(&cfg.playlist).to_string_lossy().as_ref())?;
    let seg_pattern = dir
        .join("seg_%08d.ts")
        .to_string_lossy()
        .replace(char::from(92), "/");
    settings.set_string(
        "muxer_settings",
        format!(
            "hls_time={} hls_list_size=0 hls_flags=independent_segments+temp_file hls_segment_filename={seg_pattern} start_number={}",
            cfg.segment_secs, cfg.start_segment
        )
        .as_str(),
    )?;
    let output_id = "ffmpeg_muxer";
    let mut output = context
        .output(OutputInfo::new(output_id, "relay", Some(settings), None))
        .context("creazione dell'uscita")?;

    let mut v = context.data()?;
    v.set_string("rate_control", "CBR")?;
    v.set_int("bitrate", cfg.bitrate_kbps as i64)?;
    v.set_int("keyint_sec", cfg.segment_secs as i64)?;
    if encoder_name != "x264" {
        v.set_string("preset2", "p5")?;
    } else {
        v.set_string("preset", "veryfast")?;
    }
    output
        .create_and_set_video_encoder(VideoEncoderInfo::new(video_ty, "relay_v", Some(v), None))
        .context("encoder video")?;

    let mut a = context.data()?;
    a.set_int("bitrate", 160)?;
    output
        .create_and_set_audio_encoder(
            AudioEncoderInfo::new(ObsAudioEncoderType::FFMPEG_AAC, "relay_a", Some(a), None),
            0,
        )
        .context("encoder audio")?;

    output.start().context("avvio della registrazione")?;
    emit(&Event::Started {
        encoder: encoder_name.into(),
        source: source_label.into(),
        first_frame_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64(),
    });

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) if l.trim() == "q" => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    output.stop().context("chiusura della registrazione")?;
    emit(&Event::Stopped);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_name_strips_the_path() {
        assert_eq!(exe_name(r"C:\Games\Foo\game.exe"), "game.exe");
        assert_eq!(exe_name("game.exe"), "game.exe");
    }
}

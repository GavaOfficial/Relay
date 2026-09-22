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
    data::object::ObsObjectTrait,
    data::video::ObsVideoInfoBuilder,
    data::{output::ObsOutputTrait, ObsData, ObsDataSetters},
    encoders::{ObsAudioEncoderType, ObsContextEncoders, ObsVideoEncoderType},
    run_with_obs,
    scenes::{SceneItemExtSceneTrait, SceneItemTrait},
    sources::{ObsSourceBuilder, ObsSourceRef},
    utils::{AudioEncoderInfo, OutputInfo, StartupInfo, VideoEncoderInfo},
};
use relay_capture::ipc::{EncoderChoice, Event, MonitorInfo, RecordConfig, Source, WindowInfo};

fn emit(ev: &Event) {
    print!("{}", relay_capture::ipc::encode(ev));
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn main() {
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

    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            emit(&Event::Error(format!("{e:#}")));
            std::process::exit(1);
        }
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
    std::process::exit(0);
}

fn exe_name(full_path: &str) -> String {
    full_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(full_path)
        .to_string()
}

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

fn raw_source(
    context: &ObsContext,
    id: &str,
    name: &str,
    settings: Option<ObsData>,
) -> Result<ObsSourceRef> {
    ObsSourceRef::new(
        id.to_string(),
        name.to_string(),
        settings.map(Into::into),
        None,
        context.runtime().clone(),
    )
    .with_context(|| format!("creazione della sorgente {id}"))
}

fn set_volume(context: &ObsContext, source: &ObsSourceRef, gain: f32) -> Result<()> {
    let ptr = source.as_ptr();
    run_with_obs!(context.runtime(), (ptr), move || unsafe {
        libobs::obs_source_set_volume(ptr.get_ptr(), gain);
    })
    .context("impostazione del volume")
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
    let monitors = MonitorCaptureSourceBuilder::get_monitors().unwrap_or_default();
    let window = if let Source::Window { exe } = &cfg.source {
        get_all_windows(WindowSearchMode::IncludeMinimized)
            .unwrap_or_default()
            .into_iter()
            .find(|w| exe_name(&w.full_exe).eq_ignore_ascii_case(exe))
    } else {
        None
    };
    let monitor_index = window
        .as_ref()
        .and_then(|w| w.monitor.as_ref())
        .and_then(|id| {
            monitors
                .iter()
                .position(|m| m.0.name.eq_ignore_ascii_case(id))
        })
        .or_else(|| {
            cfg.fallback_monitor_name.as_ref().and_then(|name| {
                monitors
                    .iter()
                    .position(|m| m.0.name.eq_ignore_ascii_case(name))
            })
        })
        .unwrap_or_else(|| match &cfg.source {
            Source::Monitor { index } => *index as usize,
            Source::Window { .. } => cfg.fallback_monitor.unwrap_or(0) as usize,
        });
    let monitor = monitors.get(monitor_index).or_else(|| monitors.first());
    let (base_width, base_height) = monitor
        .map(|m| (m.0.width, m.0.height))
        .unwrap_or((1920, 1080));
    let output_height = cfg.output_height.min(base_height).max(2) & !1;
    let output_width =
        (((base_width as u64 * output_height as u64 / base_height as u64) as u32).max(2)) & !1;
    let mut context = ObsContext::new(StartupInfo::default()).context("avvio di OBS")?;

    context
        .reset_video(
            ObsVideoInfoBuilder::new()
                .fps_num(cfg.fps)
                .fps_den(1)
                .base_width(base_width)
                .base_height(base_height)
                .output_width(output_width)
                .output_height(output_height)
                .build(),
        )
        .context("impostazione dei fotogrammi al secondo")?;
    let mut scene = context
        .scene("relay", Some(0))
        .context("creazione della scena")?;

    let source_label = if matches!(cfg.source, Source::Window { .. }) {
        "game"
    } else {
        "monitor"
    };

    let want_game_audio = cfg.game_audio_exe.is_some();
    let mut game_audio_done = false;
    if matches!(cfg.source, Source::Window { .. }) {
        if window.is_none() {
            emit(&Event::Warning(
                "finestra non elencata da OBS: uso la cattura fullscreen".into(),
            ));
        }
        let build_base = || -> Result<GameCaptureSourceBuilder> {
            let builder = context
                .source_builder::<GameCaptureSourceBuilder, _>("Gioco")?
                .set_capture_mode(if window.is_some() {
                    ObsGameCaptureMode::CaptureSpecificWindow
                } else {
                    ObsGameCaptureMode::Any
                });
            let builder = if let Some(raw) = &window {
                builder.set_window(raw)
            } else {
                builder
            };
            Ok(builder
                .set_hook_rate(ObsHookRate::Fast)
                .set_anti_cheat_hook(true))
        };

        let builder = if want_game_audio {
            match build_base()?.set_capture_audio(true) {
                Ok(b) => {
                    game_audio_done = true;
                    b
                }
                Err(e) => {
                    emit(&Event::Warning(format!(
                        "audio del gioco non disponibile su questo PC ({e})"
                    )));
                    build_base()?
                }
            }
        } else {
            build_base()?
        };
        let item = builder
            .add_to_scene(&mut scene)
            .context("aggiunta della sorgente di gioco alla scena")?;
        item.fit_source_to_screen()?;
    } else {
        let idx = monitor_index as u32;
        let monitor = monitors
            .get(idx as usize)
            .or_else(|| monitors.first())
            .ok_or_else(|| anyhow!("nessun monitor trovato"))?;
        let item = context
            .source_builder::<MonitorCaptureSourceBuilder, _>("Schermo")?
            .set_monitor(monitor)
            .add_to_scene(&mut scene)?;
        item.fit_source_to_screen()?;

        if want_game_audio {
            emit(&Event::Warning(
                "registro l'audio di tutto lo schermo: non riesco a isolare solo il gioco".into(),
            ));
            let desktop = raw_source(&context, "wasapi_output_capture", "Desktop", None)?;
            scene
                .add_source(desktop)
                .context("aggiunta dell'audio del desktop")?;
            game_audio_done = true;
        }
    }
    let _ = game_audio_done;

    if cfg.mic {
        let mut mic_settings = context.data()?;
        mic_settings.set_string("device_id", "default")?;
        let mic = raw_source(
            &context,
            "wasapi_input_capture",
            "Microfono",
            Some(mic_settings),
        )?;
        set_volume(&context, &mic, cfg.mic_gain)?;
        scene.add_source(mic).context("aggiunta del microfono")?;
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

    let mut stdin = std::io::stdin().lock();
    loop {
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => thread::sleep(Duration::from_millis(200)),
            Ok(_) if line.trim() == "q" => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    output.stop().context("chiusura della registrazione")?;
    emit(&Event::Stopped);
    std::process::exit(0);
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

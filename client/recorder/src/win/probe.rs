use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use super::{
    capture, encoder,
    engine::{check_all, choose_encoder, Choice},
    window,
};
use crate::{EncoderPref, MonitorInfo, Probe, WindowInfo};

pub fn run() -> Result<Probe> {
    thread::Builder::new()
        .name("relay-probe".into())
        .spawn(probe)
        .context("avvio del controllo")?
        .join()
        .unwrap_or_else(|_| bail!("il controllo del PC si e' interrotto"))
}

pub fn encoders() -> Vec<String> {
    thread::Builder::new()
        .name("relay-encoders".into())
        .spawn(|| {
            super::com_init();
            encoder::startup();
            check_all((1920, 1080), 60, 6000)
        })
        .ok()
        .and_then(|t| t.join().ok())
        .unwrap_or_else(|| vec!["controllo degli encoder interrotto".into()])
}

fn probe() -> Result<Probe> {
    super::com_init();
    encoder::startup();
    window::dpi_aware();
    if !capture::supported() {
        bail!(
            "la cattura di Windows non e' disponibile: serve Windows 10 versione 1903 o successiva"
        );
    }
    let monitors = window::monitors();
    let first = monitors
        .first()
        .context("nessuno schermo collegato")?
        .clone();
    let Choice {
        gpu,
        candidate,
        names,
        rejected,
    } = choose_encoder(EncoderPref::Auto, (1280, 720), 30, 2500, &|_| {})?;
    let source = capture::Source::monitor(&gpu, first.handle)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while source.frames() == 0 {
        if Instant::now() > deadline {
            bail!("la cattura dello schermo non restituisce immagini");
        }
        thread::sleep(Duration::from_millis(50));
    }
    let yellow_border = source.border;
    drop(source);
    Ok(Probe {
        encoders: names,
        best: candidate.kind.name().into(),
        rejected,
        yellow_border,
        monitors: monitors
            .into_iter()
            .map(|m| MonitorInfo {
                index: m.index,
                name: m.name,
                width: m.width,
                height: m.height,
            })
            .collect(),
        windows: window::list(false)
            .into_iter()
            .map(|w| WindowInfo {
                exe: w.exe,
                title: w.title,
            })
            .collect(),
    })
}

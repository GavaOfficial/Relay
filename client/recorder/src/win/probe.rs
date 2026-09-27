use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use super::{capture, encoder, engine::choose_encoder, window};
use crate::{EncoderPref, MonitorInfo, Probe, WindowInfo};

pub fn run() -> Result<Probe> {
    thread::Builder::new()
        .name("relay-probe".into())
        .spawn(probe)
        .context("avvio del controllo")?
        .join()
        .unwrap_or_else(|_| bail!("il controllo del PC si e' interrotto"))
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
    let (gpu, candidate, encoders) =
        choose_encoder(EncoderPref::Auto, (1280, 720), 30, 2500, &|_| {})?;
    let source = capture::Source::monitor(&gpu, first.handle)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while source.frames() == 0 {
        if Instant::now() > deadline {
            bail!("la cattura dello schermo non restituisce immagini");
        }
        thread::sleep(Duration::from_millis(50));
    }
    drop(source);
    Ok(Probe {
        encoders,
        best: candidate.kind.name().into(),
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

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::presets::{recommend, SpeedResult};

const START_CHUNK: usize = 512 * 1024;
const MAX_CHUNK: usize = 4 * 1024 * 1024;

pub async fn measure_upload(
    server: &str,
    token: &str,
    duration: Duration,
    on_progress: impl Fn(f64, f64),
) -> Result<f64> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let url = format!("{}/api/speedtest", server.trim_end_matches('/'));

    let mut payload = vec![0u8; MAX_CHUNK];
    getrandom::fill(&mut payload).map_err(|e| anyhow::anyhow!("nessun generatore casuale: {e}"))?;

    let begin = Instant::now();
    let mut chunk = START_CHUNK;
    let (mut bytes, mut secs) = (0f64, 0f64);
    let mut n = 0u32;
    while begin.elapsed() < duration || n < 2 {
        let t = Instant::now();
        let r = http
            .post(&url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(60))
            .body(payload[..chunk].to_vec())
            .send()
            .await
            .context("il server non risponde")?;
        if !r.status().is_success() {
            bail!("il server ha rifiutato il test ({})", r.status());
        }
        let took = t.elapsed().as_secs_f64();
        n += 1;

        if n > 1 {
            bytes += chunk as f64;
            secs += took;
        }
        let mbps = if secs > 0.0 {
            bytes * 8.0 / secs / 1_000_000.0
        } else {
            0.0
        };
        on_progress(
            (begin.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0),
            mbps,
        );

        if took < 1.0 && chunk < MAX_CHUNK {
            chunk = (chunk * 2).min(MAX_CHUNK);
        }
        if n > 200 {
            break;
        }
    }
    if secs <= 0.0 {
        bail!("misura non riuscita");
    }
    Ok(bytes * 8.0 / secs / 1_000_000.0)
}

pub async fn run(
    server: &str,
    token: &str,
    duration: Duration,
    on_progress: impl Fn(f64, f64),
) -> Result<SpeedResult> {
    let mbps = measure_upload(server, token, duration, on_progress).await?;
    Ok(recommend((mbps * 10.0).round() / 10.0))
}

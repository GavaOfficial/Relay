use std::{
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use relay_capture::install::{self, Canceller, InstallProgress, OBS_VERSION};

struct Srv {
    zip: Vec<u8>,
    ranges: bool,
    hits: AtomicUsize,
}

async fn serve(State(s): State<Arc<Srv>>, h: HeaderMap) -> impl IntoResponse {
    s.hits.fetch_add(1, Ordering::Relaxed);
    let total = s.zip.len();
    let range = h
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
        });
    match range {
        Some((a, b)) if s.ranges => {
            let b = b.min(total - 1);
            (
                StatusCode::PARTIAL_CONTENT,
                [(header::CONTENT_RANGE, format!("bytes {a}-{b}/{total}"))],
                s.zip[a..=b].to_vec(),
            )
                .into_response()
        }
        _ => (StatusCode::OK, s.zip.clone()).into_response(),
    }
}

fn pseudo(n: usize, seed: u8) -> Vec<u8> {
    let mut x = seed as u32 | 1;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

fn make_zip() -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("bin/64bit/obs.dll", pseudo(900_000, 1)),
        ("bin/64bit/avcodec-62.dll", pseudo(1_500_000, 2)),
        ("bin/64bit/obs64.exe", pseudo(300_000, 3)),
        ("bin/64bit/obs.pdb", pseudo(300_000, 4)),
        ("obs-plugins/64bit/win-capture.dll", pseudo(200_000, 5)),
        ("obs-plugins/64bit/obs-browser.dll", pseudo(200_000, 6)),
        ("data/libobs/default.effect", b"effect".repeat(1000)),
        (
            "data/obs-plugins/win-capture/graphics-hook64.dll",
            pseudo(100_000, 7),
        ),
        (
            "data/obs-plugins/win-capture/locale/it-IT.ini",
            b"x".to_vec(),
        ),
        ("LEGGIMI.txt", b"no".to_vec()),
    ];
    for (name, data) in files {
        z.start_file(name, opt).unwrap();
        z.write_all(&data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

async fn start(ranges: bool) -> (String, Arc<Srv>) {
    start_with(ranges, make_zip()).await
}

async fn start_with(ranges: bool, zip: Vec<u8>) -> (String, Arc<Srv>) {
    let srv = Arc::new(Srv {
        zip,
        ranges,
        hits: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/obs.zip", get(serve))
        .with_state(srv.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/obs.zip"), srv)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("relay-cap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

async fn run(
    url: String,
    dir: PathBuf,
    cancel: Canceller,
) -> (anyhow::Result<()>, Vec<InstallProgress>) {
    tokio::task::spawn_blocking(move || {
        let mut seen = Vec::new();
        let r = install::install_from(
            &url,
            OBS_VERSION,
            &dir,
            &install::obs_select,
            &cancel,
            &mut |p| seen.push(p.clone()),
        );
        (r, seen)
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_only_the_selected_files_and_reports_progress() {
    let (url, _srv) = start(true).await;
    let dir = tmp("ok");
    let (r, seen) = run(url, dir.clone(), Canceller::new()).await;
    r.unwrap();

    assert!(dir.join("obs.dll").exists());
    assert!(dir.join("avcodec-62.dll").exists());
    assert!(dir.join("obs-plugins/64bit/win-capture.dll").exists());
    assert!(dir.join("data/libobs/default.effect").exists());
    assert!(dir
        .join("data/obs-plugins/win-capture/graphics-hook64.dll")
        .exists());
    assert!(!dir.join("obs64.exe").exists());
    assert!(!dir.join("obs.pdb").exists());
    assert!(!dir.join("obs-plugins/64bit/obs-browser.dll").exists());
    assert!(!dir
        .join("data/obs-plugins/win-capture/locale/it-IT.ini")
        .exists());
    assert!(!dir.join("LEGGIMI.txt").exists());
    assert_eq!(
        std::fs::read(dir.join("obs.dll")).unwrap(),
        pseudo(900_000, 1)
    );
    assert!(install::is_ready(&dir));

    assert!(seen.len() > 3);
    let pct: Vec<f32> = seen.iter().map(|p| p.percent).collect();
    assert!(
        pct.windows(2).all(|w| w[0] <= w[1] + 0.001),
        "la percentuale non deve scendere: {pct:?}"
    );
    assert_eq!(seen.last().unwrap().percent, 100.0);
    assert_eq!(seen.first().unwrap().percent, 0.0);
    let last = seen.last().unwrap();
    assert!(
        last.groups.iter().all(|g| g.done && g.percent == 100.0),
        "{:?}",
        last.groups
    );
    let ids: Vec<&str> = last.groups.iter().map(|g| g.id).collect();
    assert_eq!(ids, ["engine", "encoders", "games"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn resumes_without_downloading_files_that_are_already_fine() {
    let (url, _srv) = start(true).await;
    let dir = tmp("resume");
    let (r, _) = run(url.clone(), dir.clone(), Canceller::new()).await;
    r.unwrap();
    let kept = dir.join("avcodec-62.dll");
    let kept_time = std::fs::metadata(&kept).unwrap().modified().unwrap();
    std::fs::remove_file(dir.join("obs.dll")).unwrap();
    std::fs::write(dir.join("data/libobs/default.effect"), b"rotto").unwrap();
    std::fs::write(dir.join("obs-plugins/64bit/win-capture.part"), b"a meta").unwrap();
    assert!(!install::is_ready(&dir));

    let (r, seen) = run(url, dir.clone(), Canceller::new()).await;
    r.unwrap();
    assert!(install::is_ready(&dir));
    assert_eq!(
        std::fs::read(dir.join("obs.dll")).unwrap(),
        pseudo(900_000, 1)
    );
    assert_eq!(
        std::fs::read(dir.join("data/libobs/default.effect")).unwrap(),
        b"effect".repeat(1000)
    );
    assert_eq!(
        std::fs::metadata(&kept).unwrap().modified().unwrap(),
        kept_time,
        "un file gia' a posto non va riscritto"
    );
    assert_eq!(seen.last().unwrap().percent, 100.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_partial_downloads_gives_a_clear_error() {
    let (url, _srv) = start(false).await;
    let dir = tmp("noranges");
    let (r, _) = run(url, dir.clone(), Canceller::new()).await;
    let e = r.unwrap_err().to_string();
    assert!(e.contains("parziali") || e.contains("archivio"), "{e}");
    assert!(!install::is_ready(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn can_be_cancelled() {
    let (url, _srv) = start(true).await;
    let dir = tmp("cancel");
    let c = Canceller::new();
    c.cancel();
    let (r, _) = run(url, dir.clone(), c).await;
    assert!(r.is_err());
    assert!(!install::is_ready(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn groups_are_listed_in_a_fixed_order() {
    let ids: Vec<&str> = install::GROUPS.iter().map(|g| g.id).collect();
    assert_eq!(ids, ["engine", "encoders", "games", "audio"]);
}

fn make_big_zip() -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file("bin/64bit/obs.dll", stored).unwrap();
    z.write_all(&pseudo(300_000, 1)).unwrap();
    for i in 0..60 {
        z.start_file(format!("altro/{i}.bin"), stored).unwrap();
        z.write_all(&pseudo(1_000_000, i as u8 + 10)).unwrap();
    }
    z.start_file("obs-plugins/64bit/win-capture.dll", stored)
        .unwrap();
    z.write_all(&pseudo(200_000, 2)).unwrap();
    z.finish().unwrap().into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_read_the_headers_of_files_it_does_not_need() {
    let (url, srv) = start_with(true, make_big_zip()).await;
    let dir = tmp("big");
    let (r, _) = run(url, dir.clone(), Canceller::new()).await;
    r.unwrap();
    assert_eq!(
        std::fs::read(dir.join("obs.dll")).unwrap(),
        pseudo(300_000, 1)
    );
    assert!(dir.join("obs-plugins/64bit/win-capture.dll").exists());
    let hits = srv.hits.load(Ordering::Relaxed);
    assert!(
        hits < 25,
        "troppe richieste ({hits}) per scaricare 500 KB da un archivio di 60 MB"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

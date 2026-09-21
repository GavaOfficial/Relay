use std::{collections::HashMap, path::PathBuf, process::Command as StdCommand, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use relay_server::{app, auth::Authenticator, state::AppState};
use tower::ServiceExt;

fn ffmpeg() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var("RELAY_TEST_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()));
    StdCommand::new(&p)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| p)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (status, h) = (resp.status(), resp.headers().clone());
    (
        status,
        h,
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

fn make_segments(
    ff: &PathBuf,
    dir: &std::path::Path,
    size: &str,
    secs: u32,
    first: u64,
) -> Vec<(u64, Vec<u8>)> {
    std::fs::create_dir_all(dir).unwrap();
    let st = StdCommand::new(ff)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("testsrc2=size={size}:rate=30"))
        .args([
            "-t",
            &secs.to_string(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(["-g", "120", "-keyint_min", "120", "-sc_threshold", "0"])
        .args([
            "-output_ts_offset",
            &(first * 4).to_string(),
            "-f",
            "hls",
            "-hls_time",
            "4",
            "-hls_list_size",
            "0",
        ])
        .args(["-start_number", &first.to_string(), "-hls_segment_filename"])
        .arg(dir.join("s_%d.ts"))
        .arg(dir.join("p.m3u8"))
        .status()
        .unwrap();
    assert!(st.success());
    let mut v = Vec::new();
    for n in first.. {
        let p = dir.join(format!("s_{n}.ts"));
        if !p.exists() {
            break;
        }
        v.push((n, std::fs::read(p).unwrap()));
    }
    v
}

async fn upload_and_end(app: &Router, id: &str, segs: &[(u64, Vec<u8>)]) {
    for (n, data) in segs {
        let (s, _, _) = send(
            app,
            "PUT",
            &format!("/api/matches/{id}/players/alice/segments/{n:08}.ts"),
            "ta",
            &[("x-segment-duration-ms", "4000")],
            data.clone(),
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }
    let (s, _, _) = send(
        app,
        "POST",
        &format!("/api/matches/{id}/players/alice/finish"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

async fn create(app: &Router) -> String {
    let (s, _, b) = send(
        app,
        "POST",
        "/api/matches",
        "ta",
        &[("content-type", "application/json")],
        br#"{"players":["alice"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    serde_json::from_slice::<serde_json::Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn wait_for_video(dir: &std::path::Path) -> bool {
    for _ in 0..120 {
        let no_ts = !std::fs::read_dir(dir)
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".ts"));
        if dir.join("video.mp4").exists() && no_ts {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

async fn setup(ff: PathBuf) -> (Router, tempfile::TempDir, std::sync::Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let tokens: HashMap<String, String> = [("ta".to_string(), "alice".to_string())].into();
    let st = AppState::with_ffmpeg(
        dir.path().to_path_buf(),
        Authenticator::dev(tokens),
        Some(ff),
    )
    .await
    .unwrap();
    (app(st.clone()), dir, st)
}

#[tokio::test]
async fn segments_become_one_mp4_and_are_deleted() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let work = tempfile::tempdir().unwrap();
    let segs = make_segments(&ff, work.path(), "320x180", 12, 0);
    assert_eq!(segs.len(), 3);

    let (app, dir, _st) = setup(ff.clone()).await;
    let id = create(&app).await;
    upload_and_end(&app, &id, &segs).await;

    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(
        wait_for_video(&pdir).await,
        "video.mp4 non creato o segmenti rimasti"
    );

    let out = StdCommand::new(&ff)
        .args(["-hide_banner", "-i"])
        .arg(pdir.join("video.mp4"))
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        info.contains("Duration: 00:00:12") || info.contains("Duration: 00:00:11"),
        "{info}"
    );

    for _ in 0..80 {
        if pdir.join("thumb.jpg").exists() && pdir.join("info.json").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (_, _, b) = send(&app, "GET", "/api/matches", "ta", &[], vec![]).await;
    let list: Vec<serde_json::Value> = serde_json::from_slice(&b).unwrap();
    assert_eq!(list[0]["has_recording"], true);
    assert_eq!(list[0]["has_thumb"], true);
    let d = list[0]["duration_secs"].as_u64().unwrap();
    assert!((11..=13).contains(&d), "durata {d}");
    let (s, h, jpg) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}/thumb.jpg"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(
        (s, h["content-type"].to_str().unwrap()),
        (StatusCode::OK, "image/jpeg")
    );
    assert_eq!(&jpg[..3], &[0xFF, 0xD8, 0xFF], "non e' un JPEG");

    let (_, _, b) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["videos"],
        serde_json::json!(["alice"])
    );
    let url = format!("/api/matches/{id}/players/alice/video.mp4");
    let (s, h, full) = send(&app, "GET", &url, "ta", &[], vec![]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["content-type"], "video/mp4");
    let (s, h, part) = send(
        &app,
        "GET",
        &url,
        "ta",
        &[("range", "bytes=100-199")],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part, full[100..200]);
    assert_eq!(h["content-range"], format!("bytes 100-199/{}", full.len()));
    let (s, _, tail) = send(&app, "GET", &url, "ta", &[("range", "bytes=-50")], vec![]).await;
    assert_eq!(
        (s, tail),
        (
            StatusCode::PARTIAL_CONTENT,
            full[full.len() - 50..].to_vec()
        )
    );
    let (s, _, _) = send(
        &app,
        "GET",
        &url,
        "ta",
        &[("range", "bytes=99999999-")],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE);

    let (s, _, _) = send(
        &app,
        "GET",
        &format!("{url}?download=1"),
        "nobody",
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn different_resolutions_are_reencoded_into_one_mp4() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let work = tempfile::tempdir().unwrap();
    let mut segs = make_segments(&ff, &work.path().join("a"), "320x180", 8, 0);
    segs.extend(make_segments(&ff, &work.path().join("b"), "160x90", 8, 2));
    assert_eq!(segs.len(), 4);

    let (app, dir, _st) = setup(ff).await;
    let id = create(&app).await;
    upload_and_end(&app, &id, &segs).await;
    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(
        wait_for_video(&pdir).await,
        "video.mp4 non creato o segmenti rimasti"
    );
}

#[tokio::test]
async fn broken_segments_are_kept() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let (app, dir, st) = setup(ff).await;
    let id = create(&app).await;
    upload_and_end(&app, &id, &[(0, b"questo non e un video".to_vec())]).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), st.finalize_lock.lock())
        .await
        .expect("finalize bloccato");
    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(!pdir.join("video.mp4").exists());
    assert!(
        pdir.join("00000000.ts").exists(),
        "con un video non valido i segmenti non vanno cancellati"
    );
}

fn make_segments_with_audio(
    ff: &PathBuf,
    dir: &std::path::Path,
    vol: f32,
    secs: u32,
) -> Vec<(u64, Vec<u8>)> {
    std::fs::create_dir_all(dir).unwrap();
    let st = StdCommand::new(ff)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("sine=frequency=440:sample_rate=48000,volume={vol}"))
        .args([
            "-t",
            &secs.to_string(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args([
            "-g",
            "120",
            "-keyint_min",
            "120",
            "-sc_threshold",
            "0",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
        ])
        .args([
            "-f",
            "hls",
            "-hls_time",
            "4",
            "-hls_list_size",
            "0",
            "-hls_segment_filename",
        ])
        .arg(dir.join("s_%d.ts"))
        .arg(dir.join("p.m3u8"))
        .status()
        .unwrap();
    assert!(st.success());
    (0..)
        .map_while(|n| {
            std::fs::read(dir.join(format!("s_{n}.ts")))
                .ok()
                .map(|d| (n as u64, d))
        })
        .collect()
}

fn lufs(ff: &PathBuf, file: &std::path::Path) -> f64 {
    let o = StdCommand::new(ff)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(file)
        .args(["-vn", "-af", "ebur128", "-f", "null", "-"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    let summary = &err[err.rfind("Integrated loudness").expect("riepilogo ebur128")..];
    let i = summary.find("I:").unwrap();
    summary[i + 2..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn every_view_ends_at_the_same_loudness() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let work = tempfile::tempdir().unwrap();
    let quiet = make_segments_with_audio(&ff, &work.path().join("q"), 0.15, 12);
    let loud = make_segments_with_audio(&ff, &work.path().join("l"), 4.0, 12);

    let dir = tempfile::tempdir().unwrap();
    let tokens: HashMap<String, String> = [("ta", "alice"), ("tb", "bob")]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let st = AppState::with_ffmpeg(
        dir.path().to_path_buf(),
        Authenticator::dev(tokens),
        Some(ff.clone()),
    )
    .await
    .unwrap();
    let app = app(st);
    let (s, _, b) = send(
        &app,
        "POST",
        "/api/matches",
        "ta",
        &[("content-type", "application/json")],
        br#"{"players":["alice","bob"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let id = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    for (tok, who, segs) in [("ta", "alice", &quiet), ("tb", "bob", &loud)] {
        for (n, data) in segs.iter() {
            let (s, _, _) = send(
                &app,
                "PUT",
                &format!("/api/matches/{id}/players/{who}/segments/{n:08}.ts"),
                tok,
                &[("x-segment-duration-ms", "4000")],
                data.clone(),
            )
            .await;
            assert_eq!(s, StatusCode::NO_CONTENT);
        }
        let (s, _, _) = send(
            &app,
            "POST",
            &format!("/api/matches/{id}/players/{who}/finish"),
            tok,
            &[],
            vec![],
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let base = dir.path().join("matches").join(&id).join("players");
    for who in ["alice", "bob"] {
        assert!(
            wait_for_video(&base.join(who)).await,
            "video di {who} non creato"
        );
    }
    let (a, b) = (
        lufs(&ff, &base.join("alice").join("video.mp4")),
        lufs(&ff, &base.join("bob").join("video.mp4")),
    );
    eprintln!("alice {a} LUFS, bob {b} LUFS");

    assert!(
        (a + 16.0).abs() < 1.5 && (b + 16.0).abs() < 1.5,
        "volumi dopo il livellamento: {a} e {b}"
    );
    assert!((a - b).abs() < 1.0);
}

#[tokio::test]
async fn large_videos_get_a_light_version_and_are_cacheable() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let work = tempfile::tempdir().unwrap();
    let segs = make_segments(&ff, work.path(), "1280x960", 8, 0);
    let (app, dir, _st) = setup(ff.clone()).await;
    let id = create(&app).await;
    upload_and_end(&app, &id, &segs).await;
    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(wait_for_video(&pdir).await);
    for _ in 0..120 {
        if pdir.join("web.mp4").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(pdir.join("web.mp4").exists(), "versione leggera non creata");

    let (_, _, b) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["web_videos"],
        serde_json::json!(["alice"])
    );
    let url = format!("/api/matches/{id}/players/alice/video.mp4");
    let (_, _, orig) = send(&app, "GET", &url, "ta", &[], vec![]).await;
    let (s, h, web) = send(&app, "GET", &format!("{url}?q=web"), "ta", &[], vec![]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        web.len() < orig.len(),
        "la leggera ({}) deve pesare meno dell'originale ({})",
        web.len(),
        orig.len()
    );
    let (s, h2, dl) = send(
        &app,
        "GET",
        &format!("{url}?q=web&download=1"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "il link del sito usa ?download=1");
    assert_eq!(
        dl.len(),
        orig.len(),
        "scaricando si ottiene sempre l'originale"
    );
    assert!(h2["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    let out = StdCommand::new(&ff)
        .args(["-hide_banner", "-i"])
        .arg(pdir.join("web.mp4"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("x720"),
        "non e' 720p"
    );

    assert!(h["cache-control"].to_str().unwrap().contains("immutable"));
    let etag = h["etag"].to_str().unwrap().to_string();
    let (s, _, body) = send(
        &app,
        "GET",
        &format!("{url}?q=web"),
        "ta",
        &[("if-none-match", &etag)],
        vec![],
    )
    .await;
    assert_eq!((s, body.len()), (StatusCode::NOT_MODIFIED, 0));
}

#[tokio::test]
async fn recordings_without_declared_durations_are_still_converted() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };

    let work = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(work.path()).unwrap();
    let st = StdCommand::new(&ff)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30",
        ])
        .args([
            "-t",
            "12",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "hls",
            "-hls_time",
            "100",
            "-hls_list_size",
            "0",
        ])
        .arg("-hls_segment_filename")
        .arg(work.path().join("s_%d.ts"))
        .arg(work.path().join("p.m3u8"))
        .status()
        .unwrap();
    assert!(st.success());
    let seg = std::fs::read(work.path().join("s_0.ts")).unwrap();
    assert!(
        !work.path().join("s_1.ts").exists(),
        "doveva essere un solo segmento"
    );

    let (app, dir, _st) = setup(ff).await;
    let id = create(&app).await;

    let (s, _, _) = send(
        &app,
        "PUT",
        &format!("/api/matches/{id}/players/alice/segments/00000000.ts"),
        "ta",
        &[],
        seg,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/players/alice/finish"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(wait_for_video(&pdir).await, "il video non e' stato creato");
}

#[tokio::test]
async fn conversion_progress_is_reported_and_cleared() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };

    let work = tempfile::tempdir().unwrap();
    let mut segs = make_segments(&ff, &work.path().join("a"), "1920x1080", 60, 0);
    let n = segs.len() as u64;
    segs.extend(make_segments(
        &ff,
        &work.path().join("b"),
        "1280x720",
        60,
        n,
    ));
    let (app, dir, _st) = setup(ff).await;
    let id = create(&app).await;
    upload_and_end(&app, &id, &segs).await;

    let mut seen: Vec<(String, u64)> = Vec::new();
    for _ in 0..4000 {
        let (_, _, b) = send(
            &app,
            "GET",
            &format!("/api/matches/{id}"),
            "ta",
            &[],
            vec![],
        )
        .await;
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        if let Some(p) = v["processing"].get("alice") {
            let sample = (
                p["stage"].as_str().unwrap().to_string(),
                p["pct"].as_u64().unwrap(),
            );
            if seen.last() != Some(&sample) {
                seen.push(sample);
            }
        } else if v["videos"].as_array().is_some_and(|a| !a.is_empty()) && !seen.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(wait_for_video(&pdir).await, "il video non e' stato creato");
    let video: Vec<u64> = seen
        .iter()
        .filter(|(s, _)| s == "video")
        .map(|(_, p)| *p)
        .collect();
    assert!(
        video.iter().any(|p| (6..=97).contains(p)),
        "nessun avanzamento intermedio: {seen:?}"
    );
    assert!(
        video.windows(2).all(|w| w[0] <= w[1]),
        "la percentuale non deve tornare indietro: {seen:?}"
    );

    let mut last = serde_json::Value::Null;
    for _ in 0..240 {
        let (_, _, b) = send(
            &app,
            "GET",
            &format!("/api/matches/{id}"),
            "ta",
            &[],
            vec![],
        )
        .await;
        last = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["processing"].clone();
        if last == serde_json::json!({}) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        last,
        serde_json::json!({}),
        "avanzamento non ripulito: {seen:?}"
    );
}

#[tokio::test]
async fn light_version_is_also_served_as_one_file_read_in_pieces() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let work = tempfile::tempdir().unwrap();
    let segs = make_segments(&ff, work.path(), "1280x960", 16, 0);
    let (app, dir, _st) = setup(ff).await;
    let id = create(&app).await;
    upload_and_end(&app, &id, &segs).await;
    let pdir = dir
        .path()
        .join("matches")
        .join(&id)
        .join("players")
        .join("alice");
    assert!(wait_for_video(&pdir).await);
    for _ in 0..120 {
        if pdir.join("web.mp4").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(pdir.join("web.mp4").exists());

    let (_, _, b) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["vod_videos"],
        serde_json::json!(["alice"])
    );

    let base = format!("/api/matches/{id}/players/alice/vod");
    let (s, h, pl) = send(
        &app,
        "GET",
        &format!("{base}/index.m3u8"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["content-type"], "application/vnd.apple.mpegurl");
    let pl = String::from_utf8(pl).unwrap();
    assert!(
        pl.contains("#EXT-X-BYTERANGE:") && pl.contains("#EXT-X-ENDLIST"),
        "playlist inattesa: {pl}"
    );
    assert_eq!(
        pl.matches("media.mp4").count(),
        pl.matches("#EXTINF").count() + 1,
        "un solo file, piu' l'intestazione"
    );

    let (s, h, part) = send(
        &app,
        "GET",
        &format!("{base}/media.mp4"),
        "ta",
        &[("range", "bytes=0-1023")],
        vec![],
    )
    .await;
    assert_eq!((s, part.len()), (StatusCode::PARTIAL_CONTENT, 1024));
    assert!(h["content-range"]
        .to_str()
        .unwrap()
        .starts_with("bytes 0-1023/"));
    let (s, _, _) = send(
        &app,
        "GET",
        &format!("{base}/../video.mp4"),
        "ta",
        &[],
        vec![],
    )
    .await;
    assert_ne!(s, StatusCode::INTERNAL_SERVER_ERROR);
    let (s, _, _) = send(&app, "GET", &format!("{base}/altro.txt"), "ta", &[], vec![]).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "solo i due file previsti");
}

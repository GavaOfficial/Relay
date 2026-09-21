use std::collections::HashMap;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use relay_server::{app, state::AppState};
use tower::ServiceExt;

async fn setup() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let tokens: HashMap<String, String> = [("ta", "alice"), ("tb", "bob"), ("tc", "carol")]
        .into_iter()
        .map(|(t, u)| (t.to_string(), u.to_string()))
        .collect();
    let st = AppState::new(dir.path().to_path_buf(), tokens)
        .await
        .unwrap();
    (app(st), dir)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    extra: Option<(&str, String)>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    if let Some((k, v)) = extra {
        req = req.header(k, v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (
        status,
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

fn text(b: Vec<u8>) -> String {
    String::from_utf8(b).unwrap()
}

fn json_header() -> Option<(&'static str, String)> {
    Some(("content-type", "application/json".to_string()))
}

#[tokio::test]
async fn full_flow() {
    let (app, dir) = setup().await;

    let (s, _) = send(&app, "POST", "/api/matches", None, None, vec![]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (s, b) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["alice","bob"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let id = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let seg = |p: &str, n: &str| format!("/api/matches/{id}/players/{p}/segments/{n}");
    let pl = |p: &str| format!("/api/matches/{id}/players/{p}/playlist.m3u8");

    let (s, _) = send(&app, "PUT", &seg("alice", "0"), Some("tb"), None, vec![1]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(&app, "PUT", &seg("carol", "0"), Some("tc"), None, vec![1]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = send(
        &app,
        "PUT",
        &seg("alice", "0"),
        Some("ta"),
        Some(("x-sha256", "00".to_string())),
        vec![1, 2],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    for n in ["0", "1", "3"] {
        let (s, _) = send(
            &app,
            "PUT",
            &seg("alice", n),
            Some("ta"),
            None,
            vec![7; 100],
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }

    let (s, _) = send(
        &app,
        "PUT",
        &seg("alice", "1"),
        Some("ta"),
        None,
        vec![7; 100],
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (s, b) = send(&app, "GET", &pl("alice"), Some("tb"), None, vec![]).await;
    assert_eq!(s, StatusCode::OK);
    let p = text(b);
    assert!(p.contains("00000000.ts") && p.contains("00000001.ts"));
    assert!(!p.contains("00000003.ts") && !p.contains("ENDLIST"));

    let (s, _) = send(&app, "GET", &pl("alice"), Some("tc"), None, vec![]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, b) = send(
        &app,
        "GET",
        &seg("alice", "00000000.ts"),
        Some("tb"),
        None,
        vec![],
    )
    .await;
    assert_eq!((s, b.len()), (StatusCode::OK, 100));

    let entries: Vec<String> =
        std::fs::read_dir(dir.path().join(format!("matches/{id}/players/alice")))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    assert!(entries.iter().all(|n| !n.ends_with(".tmp")), "{entries:?}");

    let (_, b) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/players/alice/finish"),
        Some("ta"),
        None,
        vec![],
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["status"],
        "open"
    );
    let (_, b) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/players/bob/finish"),
        Some("tb"),
        None,
        vec![],
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["status"],
        "ended"
    );

    let (_, b) = send(&app, "GET", &pl("alice"), Some("tb"), None, vec![]).await;
    let p = text(b);
    assert!(p.contains("00000003.ts") && p.contains("DISCONTINUITY") && p.contains("ENDLIST"));

    let (s, _) = send(&app, "PUT", &seg("alice", "9"), Some("ta"), None, vec![1]).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn cookie_session_me_and_listing() {
    let (app, _dir) = setup().await;

    let (s, b) = send(
        &app,
        "GET",
        "/api/me",
        None,
        Some(("cookie", "x=1; relay_token=ta; y=2".to_string())),
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["user"],
        "alice"
    );
    let (s, _) = send(&app, "GET", "/api/me", Some("tb"), None, vec![]).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(
        &app,
        "GET",
        "/api/me",
        None,
        Some(("cookie", "relay_token=wrong".to_string())),
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = send(&app, "GET", "/api/me", None, None, vec![]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (s, _) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["alice","bob"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    for (tok, expected) in [("ta", 1), ("tb", 1), ("tc", 0)] {
        let (s, b) = send(&app, "GET", "/api/matches", Some(tok), None, vec![]).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Vec<serde_json::Value>>(&b)
                .unwrap()
                .len(),
            expected,
            "{tok}"
        );
    }

    let (_, b) = send(&app, "GET", "/api/matches", Some("ta"), None, vec![]).await;
    let id = serde_json::from_slice::<Vec<serde_json::Value>>(&b).unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, b) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}"),
        Some("tb"),
        None,
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let v = serde_json::from_slice::<serde_json::Value>(&b).unwrap();
    assert_eq!(v["connected"], serde_json::json!([]));
    assert_eq!(v["players"], serde_json::json!(["alice", "bob"]));
}

#[tokio::test]
async fn rejects_path_traversal_ids() {
    let (app, _dir) = setup().await;
    let (s, _) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["../x"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn site_cookie_cannot_create_start_stop_or_end() {
    let (app, _dir) = setup().await;
    let cookie = || Some(("cookie", "relay_token=ta".to_string()));

    let (s, _) = send(
        &app,
        "POST",
        "/api/matches",
        None,
        cookie(),
        br#"{"players":["alice"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, b) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["alice"]}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let m: serde_json::Value = serde_json::from_slice(&b).unwrap();
    let id = m["id"].as_str().unwrap().to_string();

    for action in ["start?force=true", "stop", "end"] {
        let (s, _) = send(
            &app,
            "POST",
            &format!("/api/matches/{id}/{action}"),
            None,
            cookie(),
            vec![],
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{action} con il cookie del sito");
    }

    let code = m["invite_code"].as_str().unwrap();
    let (s, _) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/join?code={code}"),
        None,
        Some(("cookie", "relay_token=tb".to_string())),
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/stop"),
        None,
        None,
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (s, _) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/start?force=true"),
        Some("ta"),
        None,
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}"),
        None,
        cookie(),
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn app_release_is_public_and_downloadable() {
    let (app, dir) = setup().await;

    let (s, _) = send(&app, "GET", "/api/app/latest", None, None, vec![]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let d = dir.path().join("app");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("relay-app-0.2.0.exe"), b"MZ-finto").unwrap();
    std::fs::write(
        d.join("latest.json"),
        br#"{"version":"0.2.0","file":"relay-app-0.2.0.exe","sha256":"ab","size":8,"signature":"cd","notes":"prova","published_at":"2026-09-19"}"#,
    )
    .unwrap();

    let (s, b) = send(&app, "GET", "/api/app/latest", None, None, vec![]).await;
    assert_eq!(s, StatusCode::OK);
    let j: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (
            j["version"].as_str(),
            j["size"].as_u64(),
            j["download_url"].as_str()
        ),
        (Some("0.2.0"), Some(8), Some("/api/app/download"))
    );
    let (s, b) = send(&app, "GET", "/api/app/download", None, None, vec![]).await;
    assert_eq!((s, b), (StatusCode::OK, b"MZ-finto".to_vec()));

    std::fs::write(
        d.join("latest.json"),
        br#"{"version":"0.2.0","file":"../meta.json","sha256":"ab","size":8}"#,
    )
    .unwrap();
    let (s, _) = send(&app, "GET", "/api/app/download", None, None, vec![]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ffmpeg_package_is_served_like_the_app() {
    let (app, dir) = setup().await;
    let (s, _) = send(&app, "GET", "/api/app/ffmpeg/latest", None, None, vec![]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let d = dir.path().join("app");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("ffmpeg-9.0.1.zip"), vec![7u8; 700_000]).unwrap();
    std::fs::write(d.join("ffmpeg.json"), br#"{"version":"9.0.1","file":"ffmpeg-9.0.1.zip","sha256":"ab","size":700000,"signature":"cd"}"#).unwrap();
    let (s, b) = send(&app, "GET", "/api/app/ffmpeg/latest", None, None, vec![]).await;
    assert_eq!(s, StatusCode::OK);
    let j: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (j["version"].as_str(), j["download_url"].as_str()),
        (Some("9.0.1"), Some("/api/app/ffmpeg/download"))
    );
    let (s, b) = send(&app, "GET", "/api/app/ffmpeg/download", None, None, vec![]).await;
    assert_eq!(
        (s, b.len(), b.iter().all(|&x| x == 7)),
        (StatusCode::OK, 700_000, true)
    );

    let (s, _) = send(&app, "GET", "/api/app/latest", None, None, vec![]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn matches_have_names_that_the_host_can_change_from_app_or_site() {
    let (app, _dir) = setup().await;

    let (s, b) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["bob"],"name":"  Finale   di\tcampionato "}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let m: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(m["name"], "Finale di campionato");
    let id = m["id"].as_str().unwrap().to_string();
    let (_, b) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["bob"]}"#.to_vec(),
    )
    .await;
    assert!(serde_json::from_slice::<serde_json::Value>(&b)
        .unwrap()
        .get("name")
        .is_none());

    let (s, b) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/rename"),
        Some("ta"),
        json_header(),
        br#"{"name":"Serata 2"}"#.to_vec(),
    )
    .await;
    assert_eq!(
        (
            s,
            serde_json::from_slice::<serde_json::Value>(&b).unwrap()["name"].clone()
        ),
        (StatusCode::OK, "Serata 2".into())
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/matches/{id}/rename"))
        .header("cookie", "relay_token=ta")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"name":"Dal sito"}"#))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let (_, b) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}"),
        Some("tb"),
        None,
        vec![],
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&b).unwrap()["name"],
        "Dal sito"
    );

    let (s, _) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/rename"),
        Some("tb"),
        json_header(),
        br#"{"name":"Hackerato"}"#.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, b) = send(
        &app,
        "POST",
        &format!("/api/matches/{id}/rename"),
        Some("ta"),
        json_header(),
        br#"{"name":"   "}"#.to_vec(),
    )
    .await;
    assert!(serde_json::from_slice::<serde_json::Value>(&b)
        .unwrap()
        .get("name")
        .is_none());
}

#[tokio::test]
async fn the_list_says_which_matches_have_something_to_watch() {
    let (app, _dir) = setup().await;
    let (_, b) = send(
        &app,
        "POST",
        "/api/matches",
        Some("ta"),
        json_header(),
        br#"{"players":["alice"]}"#.to_vec(),
    )
    .await;
    let id = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let list = |tok: &'static str| {
        let app = app.clone();
        async move {
            let (_, b) = send(&app, "GET", "/api/matches", Some(tok), None, vec![]).await;
            serde_json::from_slice::<Vec<serde_json::Value>>(&b).unwrap()
        }
    };

    let l = list("ta").await;
    assert_eq!(
        (
            l.len(),
            l[0]["has_recording"].as_bool(),
            l[0]["has_thumb"].as_bool()
        ),
        (1, Some(false), Some(false))
    );

    send(
        &app,
        "POST",
        &format!("/api/matches/{id}/start?force=true"),
        Some("ta"),
        None,
        vec![],
    )
    .await;
    let (s, _) = send(
        &app,
        "PUT",
        &format!("/api/matches/{id}/players/alice/segments/00000000.ts"),
        Some("ta"),
        Some(("x-segment-duration-ms", "4000".to_string())),
        b"dati".to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let l = list("ta").await;
    assert_eq!(l[0]["has_recording"], true);

    let (s, _) = send(
        &app,
        "GET",
        &format!("/api/matches/{id}/thumb.jpg"),
        Some("ta"),
        None,
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

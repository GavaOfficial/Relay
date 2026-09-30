use std::collections::HashMap;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use relay_server::{app, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn setup() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let tokens: HashMap<String, String> = [("ta", "alice"), ("tb", "bob")]
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
    token: &str,
    json_body: bool,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    if json_body {
        req = req.header("content-type", "application/json");
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

async fn upload(
    app: &Router,
    token: &str,
    song: &str,
    difficulty: &str,
    score: i64,
    video: &[u8],
) -> (StatusCode, Value) {
    let id = uuid::Uuid::new_v4();
    let parts: Vec<&[u8]> = video.chunks(4).collect();
    for (i, p) in parts.iter().enumerate() {
        let (st, _) = send(
            app,
            "PUT",
            &format!("/api/codename/uploads/{id}/{i}"),
            token,
            false,
            p.to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
    }
    let body = json!({
        "mod_name": "VS Impostor V4", "song": song, "difficulty": difficulty, "score": score,
        "accuracy": 0.9, "misses": 3, "duration_ms": 120000, "parts": parts.len(), "size": video.len(),
    });
    let (st, b) = send(
        app,
        "POST",
        &format!("/api/codename/uploads/{id}/finish"),
        token,
        true,
        body.to_string().into_bytes(),
    )
    .await;
    (st, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn get_json(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let (st, b) = send(app, "GET", uri, token, false, vec![]).await;
    (st, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_better_score_replaces_the_record_and_archives_the_old_clip() {
    let (app, _dir) = setup().await;
    let best =
        "/api/codename/best?mod_name=VS%20Impostor%20V4&song=Sussus%20Moogus&difficulty=Hard";

    let (_, v) = get_json(&app, best, "ta").await;
    assert_eq!(v["score"], Value::Null);

    let (st, first) = upload(&app, "ta", "Sussus Moogus", "Hard", 1000, b"primo-video!").await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, _) = upload(&app, "ta", "Sussus Moogus", "Hard", 900, b"peggiore").await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "uno score piu' basso non e' un record"
    );
    let (st, _) = upload(&app, "ta", "Sussus Moogus", "Hard", 1000, b"uguale").await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "a parita' resta il record vecchio"
    );

    let (st, second) = upload(&app, "ta", "Sussus Moogus", "Hard", 1500, b"secondo-video").await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = upload(
        &app,
        "ta",
        "Sussus Moogus",
        "Normal",
        10,
        b"altra-difficolta",
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "ogni difficolta' ha il suo record");

    let (_, v) = get_json(&app, best, "ta").await;
    assert_eq!(v["score"], 1500);

    let (_, mods) = get_json(&app, "/api/codename/mods", "ta").await;
    assert_eq!(mods.as_array().unwrap().len(), 1);
    assert_eq!(mods[0]["key"], "vs-impostor-v4");
    assert_eq!(mods[0]["name"], "VS Impostor V4");
    assert_eq!(mods[0]["songs"], 2);

    let (_, detail) = get_json(&app, "/api/codename/mods/vs-impostor-v4", "ta").await;
    let songs = detail["songs"].as_array().unwrap();
    let hard = songs.iter().find(|s| s["difficulty"] == "Hard").unwrap();
    assert_eq!(hard["best"]["id"], second["id"]);
    assert_eq!(hard["archive"].as_array().unwrap().len(), 1);
    assert_eq!(hard["archive"][0]["id"], first["id"]);
    assert_eq!(hard["archive"][0]["archived"], true);

    let (st, video) = send(
        &app,
        "GET",
        &format!(
            "/api/codename/clips/{}/video.mp4",
            second["id"].as_str().unwrap()
        ),
        "ta",
        false,
        vec![],
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        video, b"secondo-video",
        "i pezzi vengono riuniti nell'ordine giusto"
    );
}

#[tokio::test]
async fn clips_are_private_to_their_owner() {
    let (app, _dir) = setup().await;
    let (_, clip) = upload(&app, "ta", "Bopeebo", "Hard", 10, b"video-di-alice").await;
    let id = clip["id"].as_str().unwrap();

    let (st, _) = send(
        &app,
        "GET",
        &format!("/api/codename/clips/{id}/video.mp4"),
        "tb",
        false,
        vec![],
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, mods) = get_json(&app, "/api/codename/mods", "tb").await;
    assert_eq!(mods.as_array().unwrap().len(), 0);
    let (st, _) = get_json(&app, "/api/codename/mods/vs-impostor-v4", "tb").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_missing_part_or_wrong_size_is_rejected() {
    let (app, _dir) = setup().await;
    let id = uuid::Uuid::new_v4();
    send(
        &app,
        "PUT",
        &format!("/api/codename/uploads/{id}/0"),
        "ta",
        false,
        b"abcd".to_vec(),
    )
    .await;
    let body = json!({ "mod_name": "M", "song": "S", "difficulty": "Hard", "score": 1, "duration_ms": 1, "parts": 2, "size": 8 });
    let (st, _) = send(
        &app,
        "POST",
        &format!("/api/codename/uploads/{id}/finish"),
        "ta",
        true,
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (_, v) = get_json(
        &app,
        "/api/codename/best?mod_name=M&song=S&difficulty=Hard",
        "ta",
    )
    .await;
    assert_eq!(v["score"], Value::Null, "un upload fallito non crea record");
}

async fn funkin_upload(
    app: &Router,
    song: &str,
    variation: Option<&str>,
    score: i64,
) -> StatusCode {
    let id = uuid::Uuid::new_v4();
    send(
        app,
        "PUT",
        &format!("/api/funkin/uploads/{id}/0"),
        "ta",
        false,
        b"clip".to_vec(),
    )
    .await;
    let body = json!({
        "mod_name": "Gioco base", "song_id": "bopeebo", "song": song, "difficulty": "hard",
        "variation": variation, "score": score, "duration_ms": 1000, "parts": 1, "size": 4,
    });
    send(
        app,
        "POST",
        &format!("/api/funkin/uploads/{id}/finish"),
        "ta",
        true,
        body.to_string().into_bytes(),
    )
    .await
    .0
}

#[tokio::test]
async fn funkin_keeps_a_record_per_variation_apart_from_codename() {
    let (app, _dir) = setup().await;
    assert_eq!(
        funkin_upload(&app, "Bopeebo", None, 100).await,
        StatusCode::CREATED
    );
    assert_eq!(
        funkin_upload(&app, "Bopeebo", Some("default"), 50).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        funkin_upload(&app, "Bopeebo", Some("erect"), 50).await,
        StatusCode::CREATED
    );

    let (_, v) = get_json(
        &app,
        "/api/funkin/best?mod_name=Gioco%20base&song=Bopeebo&difficulty=hard&variation=erect",
        "ta",
    )
    .await;
    assert_eq!(v["score"], 50);
    let (_, codename) = get_json(&app, "/api/codename/mods", "ta").await;
    assert_eq!(
        codename.as_array().unwrap().len(),
        0,
        "le estensioni hanno dati separati"
    );

    let (_, detail) = get_json(&app, "/api/funkin/mods/gioco-base", "ta").await;
    let songs = detail["songs"].as_array().unwrap();
    assert_eq!(songs.len(), 2);
    assert!(songs.iter().all(|s| s["song_id"] == "bopeebo"));
}

#[tokio::test]
async fn funkin_catalog_and_images_are_stored_for_the_mod() {
    let (app, _dir) = setup().await;
    let catalog = json!({ "mod_name": "Gioco base", "catalog": {
        "title": "Friday Night Funkin'", "description": "Il gioco", "has_icon": true,
        "contributors": [{ "name": "ninjamuffin99", "role": "Programmatore", "url": "javascript:alert(1)" }],
        "albums": [{ "id": "volume1", "name": "Volume 1", "art": "album-volume1.png" }],
        "tracks": [{ "id": "bopeebo", "name": "Bopeebo", "album": "volume1", "bpm": 100.0,
                     "difficulties": ["easy", "normal", "hard"], "ratings": { "hard": 3 } }]
    }});
    let (st, _) = send(
        &app,
        "PUT",
        "/api/funkin/catalog",
        "ta",
        true,
        catalog.to_string().into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let png = b"\x89PNG\r\n\x1a\nresto".to_vec();
    let (st, _) = send(
        &app,
        "PUT",
        "/api/funkin/mods/gioco-base/assets/icon.png",
        "ta",
        false,
        png.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(
        &app,
        "PUT",
        "/api/funkin/mods/gioco-base/assets/x.png",
        "ta",
        false,
        b"non png".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, got) = send(
        &app,
        "GET",
        "/api/funkin/mods/gioco-base/assets/icon.png",
        "ta",
        false,
        vec![],
    )
    .await;
    assert_eq!((st, got), (StatusCode::OK, png));
    let (st, _) = send(
        &app,
        "GET",
        "/api/funkin/mods/gioco-base/assets/icon.png",
        "tb",
        false,
        vec![],
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (_, mods) = get_json(&app, "/api/funkin/mods", "ta").await;
    assert_eq!(
        mods[0]["key"], "gioco-base",
        "la mod col catalogo si vede anche senza record"
    );
    assert_eq!(mods[0]["tracks"], 1);
    assert_eq!(mods[0]["songs"], 0);
    assert_eq!(mods[0].get("catalog"), None);

    let (_, detail) = get_json(&app, "/api/funkin/mods/gioco-base", "ta").await;
    let c = &detail["mod"]["catalog"];
    assert_eq!(c["tracks"][0]["ratings"]["hard"], 3);
    assert_eq!(c["albums"][0]["art"], "album-volume1.png");
    assert_eq!(c["title"], "Friday Night Funkin'");
    assert_eq!(
        c["contributors"][0].get("url"),
        None,
        "i link non http vengono scartati"
    );
}

#[tokio::test]
async fn psych_tracks_keep_icon_and_color() {
    let (app, _dir) = setup().await;
    let catalog = json!({ "mod_name": "YeahMan", "catalog": {
        "albums": [{ "id": "weekman", "name": "Week 1", "art": "week-weekman.png" }],
        "tracks": [
            { "id": "yuh", "name": "Yuh", "album": "weekman", "icon": "hi-fuegoyeah.png", "color": "#ff6900" },
            { "id": "bad", "name": "Bad", "icon": "../x.png", "color": "red" }
        ]
    }});
    let (st, _) = send(
        &app,
        "PUT",
        "/api/psych/catalog",
        "ta",
        true,
        catalog.to_string().into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, detail) = get_json(&app, "/api/psych/mods/yeahman", "ta").await;
    let t = &detail["mod"]["catalog"]["tracks"];
    assert_eq!(t[0]["icon"], "hi-fuegoyeah.png");
    assert_eq!(t[0]["color"], "#ff6900");
    assert_eq!(t[1].get("icon"), None);
    assert_eq!(t[1].get("color"), None);
    let (_, funkin) = get_json(&app, "/api/funkin/mods", "ta").await;
    assert_eq!(funkin.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn a_mod_with_a_new_name_takes_over_its_old_records() {
    let (app, _dir) = setup().await;
    let (st, old) = upload(&app, "ta", "Sussus Moogus", "Hard", 1000, b"vecchio").await;
    assert_eq!(st, StatusCode::CREATED);
    let (_, v) = get_json(
        &app,
        "/api/codename/best?mod_name=VS%20Impostor&song=Sussus%20Moogus&difficulty=Hard&aliases=VS%20Impostor%20V4",
        "ta",
    )
    .await;
    assert_eq!(
        v["score"], 1000,
        "il record col vecchio nome conta anche col nome nuovo"
    );

    let id = uuid::Uuid::new_v4();
    send(
        &app,
        "PUT",
        &format!("/api/codename/uploads/{id}/0"),
        "ta",
        false,
        b"nuovo".to_vec(),
    )
    .await;
    let body = json!({ "mod_name": "VS Impostor", "aliases": ["VS Impostor V4"], "song": "Sussus Moogus",
        "difficulty": "Hard", "score": 900, "duration_ms": 1, "parts": 1, "size": 5 });
    let (st, _) = send(
        &app,
        "POST",
        &format!("/api/codename/uploads/{id}/finish"),
        "ta",
        true,
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "900 non batte il 1000 fatto col vecchio nome"
    );

    let (_, mods) = get_json(&app, "/api/codename/mods", "ta").await;
    let keys: Vec<&str> = mods
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        ["vs-impostor"],
        "la card vecchia confluisce in quella nuova"
    );
    let (_, detail) = get_json(&app, "/api/codename/mods/vs-impostor", "ta").await;
    assert_eq!(detail["songs"][0]["best"]["id"], old["id"]);
}

#[tokio::test]
async fn geometry_dash_keeps_attempt_stats_level_info_and_profile() {
    let (app, _dir) = setup().await;
    let id = uuid::Uuid::new_v4();
    send(
        &app,
        "PUT",
        &format!("/api/gd/uploads/{id}/0"),
        "ta",
        false,
        b"clip".to_vec(),
    )
    .await;
    let body = json!({ "mod_name": "Livelli principali", "song_id": "1", "song": "Stereo Madness", "difficulty": "classic",
        "score": 82000, "duration_ms": 30000, "parts": 1, "size": 4,
        "extra": { "percent": 82, "coins": 0, "attempt": 12, "time_ms": 30000 } });
    let (st, clip) = send(
        &app,
        "POST",
        &format!("/api/gd/uploads/{id}/finish"),
        "ta",
        true,
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let clip: Value = serde_json::from_slice(&clip).unwrap();
    assert_eq!(clip["extra"]["percent"], 82);

    let catalog = json!({ "mod_name": "Livelli principali", "catalog": { "tracks": [
        { "id": "1", "name": "Stereo Madness", "difficulties": ["classic"], "extra": { "stars": 1, "difficulty": 1, "coins": 3 } }
    ]}});
    let (st, _) = send(
        &app,
        "PUT",
        "/api/gd/catalog",
        "ta",
        true,
        catalog.to_string().into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, detail) = get_json(&app, "/api/gd/mods/livelli-principali", "ta").await;
    assert_eq!(detail["mod"]["catalog"]["tracks"][0]["extra"]["coins"], 3);

    let profile = json!({ "username": "Gavatech", "stars": 191 });
    let (st, _) = send(
        &app,
        "PUT",
        "/api/gd/profile",
        "ta",
        true,
        profile.to_string().into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, p) = get_json(&app, "/api/gd/profile", "ta").await;
    assert_eq!(p["username"], "Gavatech");
    let (_, other) = get_json(&app, "/api/gd/profile", "tb").await;
    assert_eq!(other, Value::Null, "il profilo e' privato");
}

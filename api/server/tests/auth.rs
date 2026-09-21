use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{routing::get, Json, Router};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use relay_server::{
    app,
    auth::{Authenticator, GavaConfig},
    state::AppState,
};
use serde_json::{json, Value};

#[path = "common/keys.rs"]
mod keys;

const ISS: &str = "https://auth.test";
const AUD: &str = "gavaauth-clients";
const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef-test-secret";

struct Env {
    base: String,
    jwks_hits: Arc<AtomicUsize>,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

async fn spawn() -> Env {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let jwks = Router::new().route(
        "/jwks.json",
        get(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                Json(json!({"keys":[{"kty":"RSA","kid":"k1","use":"sig","alg":"RS256","n":keys::PRIV_N,"e":keys::PRIV_E}]}))
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let jwks_url = format!("http://{}/jwks.json", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, jwks).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let auth = Authenticator::new(
        HashMap::new(),
        Some(GavaConfig {
            issuer: ISS.into(),
            audience: AUD.into(),
            jwks_url,
        }),
        Some((SECRET.to_vec(), Duration::from_secs(3600))),
    );
    let st = AppState::with_auth(dir.path().to_path_buf(), auth)
        .await
        .unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app(st)).await.unwrap() });
    Env {
        base,
        jwks_hits: hits,
        http: reqwest::Client::new(),
        _dir: dir,
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn mint_with(
    sub: &str,
    name: &str,
    exp_delta: i64,
    iss: &str,
    aud: &str,
    pem: &str,
    kid: &str,
) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some(kid.to_string());
    let claims = json!({"sub": sub, "name": name, "sid": "s1", "iss": iss, "aud": aud, "exp": now() + exp_delta});
    encode(
        &h,
        &claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn mint(sub: &str, name: &str) -> String {
    mint_with(sub, name, 900, ISS, AUD, keys::PRIV_PEM, "k1")
}

async fn me_status(env: &Env, token: &str) -> u16 {
    env.http
        .get(format!("{}/api/me", env.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn gavaauth_token_is_verified() {
    let env = spawn().await;

    let r: Value = env
        .http
        .get(format!("{}/api/me", env.base))
        .bearer_auth(mint("cuid1", "Ada Lovelace"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["user"], "cuid1");
    assert_eq!(r["name"], "Ada Lovelace");

    assert_eq!(
        me_status(
            &env,
            &mint_with("u", "x", -600, ISS, AUD, keys::PRIV_PEM, "k1")
        )
        .await,
        401
    );
    assert_eq!(
        me_status(
            &env,
            &mint_with("u", "x", 900, ISS, "altro", keys::PRIV_PEM, "k1")
        )
        .await,
        401
    );
    assert_eq!(
        me_status(
            &env,
            &mint_with("u", "x", 900, "https://evil", AUD, keys::PRIV_PEM, "k1")
        )
        .await,
        401
    );
    assert_eq!(
        me_status(
            &env,
            &mint_with("u", "x", 900, ISS, AUD, keys::OTHER_PEM, "k1")
        )
        .await,
        401
    );
    assert_eq!(
        me_status(
            &env,
            &mint_with("u", "x", 900, ISS, AUD, keys::PRIV_PEM, "sconosciuto")
        )
        .await,
        401
    );
    assert_eq!(me_status(&env, "spazzatura").await, 401);

    assert_eq!(env.jwks_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn hs256_confusion_attack_is_rejected() {
    let env = spawn().await;

    let mut h = Header::new(Algorithm::HS256);
    h.kid = Some("k1".into());
    let claims = json!({"sub":"attacker","iss":ISS,"aud":AUD,"exp":now()+900});
    let forged = encode(
        &h,
        &claims,
        &EncodingKey::from_secret(keys::PRIV_N.as_bytes()),
    )
    .unwrap();
    assert_eq!(me_status(&env, &forged).await, 401);

    let claims = json!({"sub":"attacker","iss":"relay","iat":now(),"exp":now()+900});
    let forged = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"un-altro-segreto-lungo-abbastanza-1234"),
    )
    .unwrap();
    assert_eq!(me_status(&env, &forged).await, 401);
}

#[tokio::test]
async fn session_exchange() {
    let env = spawn().await;

    let r = env
        .http
        .post(format!("{}/api/session", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = env
        .http
        .post(format!("{}/api/session", env.base))
        .bearer_auth("x.y.z")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    let g: Value = env
        .http
        .post(format!("{}/api/session", env.base))
        .bearer_auth(mint("cuid1", "Ada"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(g["user"], "cuid1");
    assert_eq!(g["expires_in"], 3600);
    let session = g["session"].as_str().unwrap().to_string();

    assert_eq!(me_status(&env, &session).await, 200);
    let r: Value = env
        .http
        .get(format!("{}/api/me", env.base))
        .header("cookie", format!("relay_token={session}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["user"].as_str(), r["name"].as_str()),
        (Some("cuid1"), Some("Ada"))
    );

    let r = env
        .http
        .post(format!("{}/api/session", env.base))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    let claims = json!({"sub":"cuid1","iss":"relay","iat":now()-7200,"exp":now()-3600});
    let old = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET),
    )
    .unwrap();
    assert_eq!(me_status(&env, &old).await, 401);
}

#[tokio::test]
async fn invite_flow() {
    let env = spawn().await;
    let session = |sub: &'static str, name: &'static str| {
        let (http, base) = (env.http.clone(), env.base.clone());
        async move {
            let g: Value = http
                .post(format!("{base}/api/session"))
                .bearer_auth(mint(sub, name))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            g["session"].as_str().unwrap().to_string()
        }
    };
    let (coord, bob, carol) = (
        session("coord1", "Cora").await,
        session("bob1", "Bob").await,
        session("carol1", "Carla").await,
    );

    let m: Value = env
        .http
        .post(format!("{}/api/matches", env.base))
        .bearer_auth(&coord)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = m["id"].as_str().unwrap().to_string();
    let code = m["invite_code"].as_str().unwrap().to_string();
    assert_eq!(m["players"], json!([]));
    assert_eq!(m["names"]["coord1"], "Cora");

    let join = |token: String, code: String| {
        let (http, base, id) = (env.http.clone(), env.base.clone(), id.clone());
        async move {
            http.post(format!("{base}/api/matches/{id}/join?code={code}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
        }
    };

    let r = env
        .http
        .get(format!("{}/api/matches/{id}", env.base))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(join(bob.clone(), "sbagliato".into()).await.status(), 404);

    let r = join(bob.clone(), code.clone()).await;
    assert_eq!(r.status(), 200);
    let j: Value = r.json().await.unwrap();
    assert_eq!(j["players"], json!(["bob1"]));
    assert_eq!(j["names"]["bob1"], "Bob");
    assert!(
        j.get("invite_code").is_none(),
        "il codice e' solo del coordinatore"
    );
    assert_eq!(join(bob.clone(), code.clone()).await.status(), 200);
    assert_eq!(join(carol.clone(), code.clone()).await.status(), 200);

    let seen = |t: String| {
        let (http, base, id) = (env.http.clone(), env.base.clone(), id.clone());
        async move {
            http.get(format!("{base}/api/matches/{id}"))
                .bearer_auth(t)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let c = seen(coord.clone()).await;
    assert_eq!(c["invite_code"], code.as_str());
    assert_eq!(c["players"], json!(["bob1", "carol1"]));
    assert!(seen(bob.clone()).await.get("invite_code").is_none());

    let r = env
        .http
        .post(format!("{}/api/matches/{id}/start?force=true", env.base))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = env
        .http
        .post(format!("{}/api/matches/{id}/start?force=true", env.base))
        .bearer_auth(&coord)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let late = session("late1", "Late").await;
    assert_eq!(join(late, code.clone()).await.status(), 409);

    let l: Vec<Value> = env
        .http
        .get(format!("{}/api/matches", env.base))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(l.len(), 1);
    assert!(l[0].get("invite_code").is_none());
}

#[tokio::test]
async fn cannot_start_without_players() {
    let env = spawn().await;
    let g: Value = env
        .http
        .post(format!("{}/api/session", env.base))
        .bearer_auth(mint("coord1", "Cora"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let s = g["session"].as_str().unwrap();
    let m: Value = env
        .http
        .post(format!("{}/api/matches", env.base))
        .bearer_auth(s)
        .json(&json!({"play": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = m["id"].as_str().unwrap();
    let r = env
        .http
        .post(format!("{}/api/matches/{id}/start?force=true", env.base))
        .bearer_auth(s)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
}

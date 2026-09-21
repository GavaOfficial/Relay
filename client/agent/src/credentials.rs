use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use relay_common::SessionGrant;
use serde::Deserialize;

use crate::{login::Tokens, tokenstore::TokenStore};

#[derive(Debug, Clone)]
pub struct Session {
    pub token: String,
    pub user: String,
    pub name: Option<String>,
}

#[derive(Deserialize)]
struct Me {
    user: String,
    name: Option<String>,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?)
}

const RELOGIN: &str = "esegui `relay-agent login`";

async fn refresh(auth: &str, store: &dyn TokenStore, refresh_token: &str) -> Result<Tokens> {
    let resp = client()?
        .post(format!("{auth}/token"))
        .json(&serde_json::json!({ "grant_type": "refresh_token", "refresh_token": refresh_token }))
        .send()
        .await
        .map_err(|e| anyhow!("rete non raggiungibile: impossibile contattare GavaAuth ({e}); il login salvato e' intatto, riprova"))?;
    let status = resp.status();
    if status.as_u16() == 400 || status.as_u16() == 401 {
        store.clear()?;
        bail!("login scaduto o revocato: {RELOGIN}");
    }
    if !status.is_success() {
        bail!("GavaAuth ha risposto {status} al rinnovo del login (login salvato intatto)");
    }
    resp.json().await.context("risposta di GavaAuth non valida")
}

pub async fn resolve_session(
    server: &str,
    auth_url: &str,
    _client_id: &str,
    store: &dyn TokenStore,
) -> Result<Session> {
    let auth = auth_url.trim_end_matches('/');
    let server = server.trim_end_matches('/');
    let Some(old) = store.get()? else {
        bail!("non hai eseguito l'accesso: {RELOGIN}");
    };
    let tokens = refresh(auth, store, &old).await?;

    store
        .set(&tokens.refresh_token)
        .context("impossibile salvare il nuovo refresh token")?;

    let resp = client()?
        .post(format!("{server}/api/session"))
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .map_err(|e| anyhow!("impossibile contattare il server Relay ({e})"))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("il server Relay ha rifiutato la sessione ({status})");
    }
    let g: SessionGrant = resp
        .json()
        .await
        .context("risposta /api/session non valida")?;
    Ok(Session {
        token: g.session,
        user: g.user,
        name: g.name,
    })
}

pub async fn session_from_token(server: &str, token: &str) -> Result<Session> {
    let resp = client()?
        .get(format!("{}/api/me", server.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| anyhow!("impossibile contattare il server Relay ({e})"))?;
    let status = resp.status();
    if status.as_u16() == 401 {
        bail!("il server Relay non accetta il token (401)");
    }
    if !status.is_success() {
        bail!("il server Relay ha risposto {status} a /api/me");
    }
    let me: Me = resp.json().await.context("risposta /api/me non valida")?;
    Ok(Session {
        token: token.to_string(),
        user: me.user,
        name: me.name,
    })
}

pub async fn logout(auth_url: &str, store: &dyn TokenStore) -> Result<()> {
    if let Some(rt) = store.get()? {
        let r = client()?
            .post(format!("{}/logout", auth_url.trim_end_matches('/')))
            .json(&serde_json::json!({ "refresh_token": rt }))
            .send()
            .await;
        match r {
            Ok(x) if x.status().is_success() => {}
            Ok(x) => tracing::warn!(
                "revoca sul server non riuscita ({}): il login locale viene comunque rimosso",
                x.status()
            ),
            Err(e) => tracing::warn!(
                "revoca sul server non riuscita ({e}): il login locale viene comunque rimosso"
            ),
        }
    }
    store.clear()
}

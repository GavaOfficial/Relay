use std::{collections::HashMap, env, path::PathBuf, time::Duration};

use relay_server::{
    app,
    auth::{Authenticator, GavaConfig},
    state::AppState,
};

fn var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "relay_server=info,tower_http=info".into()),
        )
        .init();

    let bind = var("RELAY_BIND").unwrap_or_else(|| "127.0.0.1:8080".into());
    let data = PathBuf::from(var("RELAY_DATA").unwrap_or_else(|| "./data".into()));

    let dev_tokens: HashMap<String, String> = var("RELAY_DEV_TOKENS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(t, u)| (t.trim().to_string(), u.trim().to_string()))
        .filter(|(t, u)| !t.is_empty() && !u.is_empty())
        .collect();

    let gava = var("GAVAAUTH_ISSUER").map(|issuer| {
        let issuer = issuer.trim_end_matches('/').to_string();
        GavaConfig {
            audience: var("GAVAAUTH_AUDIENCE").unwrap_or_else(|| "gavaauth-clients".into()),
            jwks_url: var("GAVAAUTH_JWKS_URL").unwrap_or_else(|| format!("{issuer}/jwks.json")),
            issuer,
        }
    });

    let session = match var("RELAY_SESSION_SECRET") {
        Some(s) if s.len() >= 32 => {
            let hours: u64 = var("RELAY_SESSION_TTL_HOURS")
                .and_then(|h| h.parse().ok())
                .unwrap_or(24);
            Some((s.into_bytes(), Duration::from_secs(hours * 3600)))
        }
        Some(_) => return Err("RELAY_SESSION_SECRET deve avere almeno 32 caratteri".into()),
        None => None,
    };
    if gava.is_some() && session.is_none() {
        return Err(
            "con GAVAAUTH_ISSUER serve anche RELAY_SESSION_SECRET (almeno 32 caratteri)".into(),
        );
    }

    let auth = Authenticator::new(dev_tokens, gava, session);
    if !auth.is_configured() {
        return Err("nessuna autenticazione: imposta GAVAAUTH_ISSUER (produzione) o RELAY_DEV_TOKENS (sviluppo)".into());
    }
    if var("RELAY_DEV_TOKENS").is_some() {
        tracing::warn!(
            "RELAY_DEV_TOKENS attivo: token statici accettati. Non usarlo in produzione."
        );
    }

    let state = AppState::with_auth(data, auth).await?;
    if state.ffmpeg.is_some() {
        tokio::spawn(relay_server::finalize::resume_all(state.clone()));
    } else {
        tracing::warn!("RELAY_FFMPEG non impostato: i segmenti non vengono uniti in un MP4 e restano sul disco");
    }
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("in ascolto su {bind}");
    axum::serve(listener, app(state)).await?;
    Ok(())
}

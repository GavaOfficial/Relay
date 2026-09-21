use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::FromRequestParts,
    http::{
        header::{AUTHORIZATION, COOKIE},
        request::Parts,
    },
};
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{error::AppError, state::AppState};

pub const SESSION_COOKIE: &str = "relay_token";

const SESSION_ISSUER: &str = "relay";

const JWKS_MAX_AGE: Duration = Duration::from_secs(3600);

const JWKS_MIN_REFETCH: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: String,
    pub name: Option<String>,
}

pub struct GavaConfig {
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

struct Gava {
    cfg: GavaConfig,
    http: reqwest::Client,
    cache: RwLock<KeyCache>,
}

#[derive(Serialize, Deserialize)]
struct GavaClaims {
    sub: String,
    name: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SessionClaims {
    sub: String,
    name: Option<String>,
    iss: String,
    iat: u64,
    exp: u64,
}

struct Session {
    secret: Vec<u8>,
    ttl: Duration,
}

pub struct Authenticator {
    dev_tokens: HashMap<String, String>,
    gava: Option<Gava>,
    session: Option<Session>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Authenticator {
    pub fn dev(dev_tokens: HashMap<String, String>) -> Self {
        Self {
            dev_tokens,
            gava: None,
            session: None,
        }
    }

    pub fn new(
        dev_tokens: HashMap<String, String>,
        gava: Option<GavaConfig>,
        session: Option<(Vec<u8>, Duration)>,
    ) -> Self {
        Self {
            dev_tokens,
            gava: gava.map(|cfg| Gava {
                cfg,
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .expect("client http"),
                cache: RwLock::new(KeyCache::default()),
            }),
            session: session.map(|(secret, ttl)| Session { secret, ttl }),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.dev_tokens.is_empty() || self.gava.is_some()
    }

    pub async fn authenticate(&self, token: &str) -> Option<Identity> {
        if let Some(id) = self.dev_tokens.get(token) {
            return Some(Identity {
                id: id.clone(),
                name: None,
            });
        }
        match decode_header(token).ok()?.alg {
            Algorithm::HS256 => self.verify_session(token),
            Algorithm::RS256 => self.verify_gava(token).await,
            _ => None,
        }
    }

    pub async fn authenticate_primary(&self, token: &str) -> Option<Identity> {
        if let Some(id) = self.dev_tokens.get(token) {
            return Some(Identity {
                id: id.clone(),
                name: None,
            });
        }
        if decode_header(token).ok()?.alg == Algorithm::RS256 {
            return self.verify_gava(token).await;
        }
        None
    }

    fn verify_session(&self, token: &str) -> Option<Identity> {
        let s = self.session.as_ref()?;
        let mut v = Validation::new(Algorithm::HS256);
        v.set_issuer(&[SESSION_ISSUER]);
        v.leeway = 5;
        let c = decode::<SessionClaims>(token, &DecodingKey::from_secret(&s.secret), &v)
            .ok()?
            .claims;
        Some(Identity {
            id: c.sub,
            name: c.name,
        })
    }

    pub fn issue_session(&self, who: &Identity) -> Option<(String, u64)> {
        let s = self.session.as_ref()?;
        let now = now_secs();
        let claims = SessionClaims {
            sub: who.id.clone(),
            name: who.name.clone(),
            iss: SESSION_ISSUER.to_string(),
            iat: now,
            exp: now + s.ttl.as_secs(),
        };
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&s.secret),
        )
        .ok()?;
        Some((token, s.ttl.as_secs()))
    }

    async fn verify_gava(&self, token: &str) -> Option<Identity> {
        let g = self.gava.as_ref()?;
        let header = decode_header(token).ok()?;
        let key = g.key_for(header.kid.as_deref().unwrap_or("")).await?;

        let mut v = Validation::new(Algorithm::RS256);
        v.set_issuer(&[g.cfg.issuer.as_str()]);
        v.set_audience(&[g.cfg.audience.as_str()]);
        v.leeway = 30;
        let c = decode::<GavaClaims>(token, &key, &v).ok()?.claims;
        Some(Identity {
            id: c.sub,
            name: c.name,
        })
    }
}

impl Gava {
    async fn key_for(&self, kid: &str) -> Option<DecodingKey> {
        {
            let c = self.cache.read().await;
            let fresh = c.fetched_at.is_some_and(|t| t.elapsed() < JWKS_MAX_AGE);
            if fresh {
                if let Some(k) = c.keys.get(kid) {
                    return Some(k.clone());
                }
            }
        }

        let mut c = self.cache.write().await;
        let may_refetch = c.fetched_at.is_none_or(|t| t.elapsed() >= JWKS_MIN_REFETCH);
        if may_refetch {
            match self.fetch().await {
                Ok(keys) => {
                    c.keys = keys;
                    c.fetched_at = Some(Instant::now());
                }
                Err(e) => tracing::warn!("download JWKS fallito: {e}"),
            }
        }
        c.keys.get(kid).cloned()
    }

    async fn fetch(&self) -> Result<HashMap<String, DecodingKey>, String> {
        let jwks: Jwks = self
            .http
            .get(&self.cfg.jwks_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let mut keys = HashMap::new();
        for k in jwks.keys {
            if k.kty != "RSA" {
                continue;
            }
            if let (Some(n), Some(e)) = (k.n, k.e) {
                if let Ok(dk) = DecodingKey::from_rsa_components(&n, &e) {
                    keys.insert(k.kid.unwrap_or_default(), dk);
                }
            }
        }
        Ok(keys)
    }
}

fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn cookie(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v)
}

pub fn bearer_token(parts: &Parts) -> Option<&str> {
    bearer(parts)
}

pub struct AuthUser(pub String);

pub struct AppUser(pub String);

pub struct AppWho(pub Identity);

pub struct Who(pub Identity);

async fn identify(parts: &Parts, state: &Arc<AppState>) -> Result<Identity, AppError> {
    let token = bearer(parts)
        .or_else(|| cookie(parts))
        .ok_or(AppError::Unauthorized)?;
    state
        .auth
        .authenticate(token)
        .await
        .ok_or(AppError::Unauthorized)
}

async fn identify_app(parts: &Parts, state: &Arc<AppState>) -> Result<Identity, AppError> {
    match bearer(parts) {
        Some(token) => state
            .auth
            .authenticate(token)
            .await
            .ok_or(AppError::Unauthorized),

        None if cookie(parts).is_some() => Err(AppError::Forbidden),
        None => Err(AppError::Unauthorized),
    }
}

impl FromRequestParts<Arc<AppState>> for AppUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(AppUser(identify_app(parts, state).await?.id))
    }
}

impl FromRequestParts<Arc<AppState>> for AppWho {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(AppWho(identify_app(parts, state).await?))
    }
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(AuthUser(identify(parts, state).await?.id))
    }
}

impl FromRequestParts<Arc<AppState>> for Who {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(Who(identify(parts, state).await?))
    }
}

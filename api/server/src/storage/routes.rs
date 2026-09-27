use std::sync::{atomic::Ordering, Arc};

use axum::{
    extract::{Path, State},
    http::header,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use crate::{auth::AuthUser, error::AppError, state::AppState};

type St = State<Arc<AppState>>;

fn is_admin(st: &AppState, user: &str) -> bool {
    let list = std::env::var("RELAY_ADMINS")
        .ok()
        .or_else(|| std::fs::read_to_string(st.data_dir.join("admins.txt")).ok())
        .unwrap_or_default();
    list.split([',', '\n']).map(str::trim).any(|a| !a.is_empty() && a == user)
}

fn admin(st: &AppState, user: &str) -> Result<(), AppError> {
    if is_admin(st, user) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

pub async fn am_admin(State(st): St, AuthUser(user): AuthUser) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "admin": is_admin(&st, &user) }))
}

pub async fn status(State(st): St, AuthUser(user): AuthUser) -> Result<Json<serde_json::Value>, AppError> {
    admin(&st, &user)?;
    let s = &st.storage;
    let used = s.used_by_node();
    let nodes: Vec<serde_json::Value> = s
        .nodes()
        .into_iter()
        .map(|n| {
            let link = s.links.any(&n.id);
            let (bytes, files) = used.get(&n.id).copied().unwrap_or((0, 0));
            let stats = link.as_ref().map(|l| l.stats.lock().unwrap().clone()).unwrap_or_default();
            serde_json::json!({
                "id": n.id,
                "name": n.name,
                "limit": n.limit,
                "created_at": n.created_at,
                "online": link.as_ref().is_some_and(|l| l.online()),
                "connections": link.as_ref().map(|l| l.connections()).unwrap_or(0),
                "version": link.as_ref().map(|l| l.version.lock().unwrap().clone()),
                "since": link.as_ref().map(|l| l.since.load(Ordering::Relaxed)),
                "disk_total": stats.total,
                "disk_free": stats.free,
                "used": bytes,
                "files": files,
                "up_rate": link.as_ref().map(|l| l.up_rate.load(Ordering::Relaxed)).unwrap_or(0),
                "down_rate": link.as_ref().map(|l| l.down_rate.load(Ordering::Relaxed)).unwrap_or(0),
            })
        })
        .collect();
    let activity = s.activity.lock().unwrap().clone();
    Ok(Json(serde_json::json!({
        "nodes": nodes,
        "cache_limit": s.cache_limit(),
        "activity": activity,
    })))
}

#[derive(Deserialize)]
pub struct NewNode {
    name: String,
}

pub async fn add_node(State(st): St, AuthUser(user): AuthUser, Json(input): Json<NewNode>) -> Result<Json<serde_json::Value>, AppError> {
    admin(&st, &user)?;
    let name = relay_common::clean_name(&input.name).ok_or(AppError::BadRequest("nome non valido"))?;
    let (node, key) = st.storage.add_node(&name);
    Ok(Json(serde_json::json!({ "id": node.id, "name": node.name, "key": key })))
}

#[derive(Deserialize)]
pub struct NodePatch {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, with = "double_option")]
    limit: Option<Option<u64>>,
}

mod double_option {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<u64>>, D::Error> {
        Ok(Some(Option::deserialize(d)?))
    }
}

pub async fn update_node(State(st): St, AuthUser(user): AuthUser, Path(id): Path<String>, Json(p): Json<NodePatch>) -> Result<impl IntoResponse, AppError> {
    admin(&st, &user)?;
    let name = match p.name {
        Some(n) => Some(relay_common::clean_name(&n).ok_or(AppError::BadRequest("nome non valido"))?),
        None => None,
    };
    if !st.storage.update_node(&id, name.as_deref(), p.limit) {
        return Err(AppError::NotFound);
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn rotate_key(State(st): St, AuthUser(user): AuthUser, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
    admin(&st, &user)?;
    let key = st.storage.rotate_key(&id).ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::json!({ "key": key })))
}

#[derive(Deserialize)]
pub struct CacheInput {
    limit: u64,
}

pub async fn set_cache(State(st): St, AuthUser(user): AuthUser, Json(c): Json<CacheInput>) -> Result<impl IntoResponse, AppError> {
    admin(&st, &user)?;
    st.storage.set_cache_limit(c.limit);
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn migrate(State(st): St, AuthUser(user): AuthUser) -> Result<impl IntoResponse, AppError> {
    admin(&st, &user)?;
    st.storage.rush();
    Ok(axum::http::StatusCode::ACCEPTED)
}

pub async fn install_script() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], INSTALL_SH)
}

const INSTALL_SH: &str = include_str!("install.sh");

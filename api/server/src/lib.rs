pub mod auth;
pub mod error;
pub mod finalize;
pub mod playlist;
pub mod routes;
pub mod state;
pub mod ws;

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use state::AppState;
use tower_http::trace::TraceLayer;

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/healthz", get(routes::healthz))
        .route("/api/me", get(routes::me))
        .route("/api/games/search", get(routes::search_games))
        .route("/api/session", post(routes::create_session))
        .route("/api/speedtest", post(routes::speedtest))
        .route("/api/app/latest", get(routes::app_latest))
        .route("/api/app/download", get(routes::app_download))
        .route("/api/app/ffmpeg/latest", get(routes::ffmpeg_latest))
        .route("/api/app/ffmpeg/download", get(routes::ffmpeg_download))
        .route("/api/app/capture/latest", get(routes::capture_latest))
        .route("/api/app/capture/download", get(routes::capture_download))
        .route(
            "/api/matches",
            get(routes::list_matches).post(routes::create_match),
        )
        .route(
            "/api/matches/{id}",
            get(routes::show_match).delete(routes::delete_match),
        )
        .route("/api/matches/{id}/end", post(routes::end_match))
        .route("/api/matches/{id}/rename", post(routes::rename_match))
        .route("/api/matches/{id}/game", post(routes::set_game))
        .route("/api/matches/{id}/fnf", post(routes::set_fnf))
        .route(
            "/api/matches/{id}/share",
            post(routes::enable_share).delete(routes::disable_share),
        )
        .route("/api/matches/{id}/thumb.jpg", get(routes::get_thumb))
        .route("/api/matches/{id}/join", post(routes::join_match))
        .route("/api/matches/{id}/start", post(routes::start_match))
        .route("/api/matches/{id}/stop", post(routes::stop_match))
        .route("/api/matches/{id}/ws", get(ws::match_ws))
        .route(
            "/api/matches/{id}/players/{pid}/playlist.m3u8",
            get(routes::get_playlist),
        )
        .route(
            "/api/matches/{id}/players/{pid}/video.mp4",
            get(routes::get_video),
        )
        .route(
            "/api/matches/{id}/players/{pid}/vod/{file}",
            get(routes::get_vod),
        )
        .route(
            "/api/matches/{id}/players/{pid}/finish",
            post(routes::finish_player),
        )
        .route(
            "/api/matches/{id}/players/{pid}/segments/{file}",
            get(routes::get_segment).put(routes::put_segment),
        )
        .route("/api/share/{token}", get(routes::share_match))
        .route("/api/share/{token}/thumb.jpg", get(routes::share_thumb))
        .route(
            "/api/share/{token}/players/{pid}/video.mp4",
            get(routes::share_video),
        )
        .route(
            "/api/share/{token}/players/{pid}/vod/{file}",
            get(routes::share_vod),
        )
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

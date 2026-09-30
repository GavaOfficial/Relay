pub mod auth;
pub mod error;
pub mod finalize;
pub mod ops;
pub mod playlist;
pub mod routes;
pub mod songlib;
pub mod state;
pub mod storage;
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
        .route("/api/app/storage/latest", get(routes::storage_latest))
        .route("/api/app/storage/download", get(routes::storage_download))
        .route(
            relay_common::storage::TUNNEL_PATH,
            get(storage::tunnel::tunnel),
        )
        .route(
            "/api/storage/install.sh",
            get(storage::routes::install_script),
        )
        .route("/api/storage/admin", get(storage::routes::am_admin))
        .route("/api/storage/status", get(storage::routes::status))
        .route("/api/storage/nodes", post(storage::routes::add_node))
        .route(
            "/api/storage/nodes/{id}",
            axum::routing::patch(storage::routes::update_node).delete(storage::routes::remove_node),
        )
        .route(
            "/api/storage/nodes/{id}/key",
            post(storage::routes::rotate_key),
        )
        .route("/api/storage/migrate", post(storage::routes::migrate))
        .route("/api/ops/install.sh", get(ops::routes::install_script))
        .route("/api/ops/status", get(ops::routes::status))
        .route("/api/ops/retry", post(ops::routes::retry))
        .route("/api/ops/nodes", post(ops::routes::add_node))
        .route(
            "/api/ops/nodes/{id}",
            axum::routing::patch(ops::routes::update_node).delete(ops::routes::remove_node),
        )
        .route("/api/ops/nodes/{id}/key", post(ops::routes::rotate_key))
        .route("/api/ops/poll", post(ops::routes::poll))
        .route("/api/ops/jobs/{id}/in/{name}", get(ops::routes::input))
        .route("/api/ops/jobs/{id}/progress", post(ops::routes::progress))
        .route(
            "/api/ops/jobs/{id}/out/{output}",
            get(ops::routes::big_state).put(ops::routes::put_output),
        )
        .route("/api/ops/jobs/{id}/finish", post(ops::routes::finish))
        .route("/api/app/ops/latest", get(routes::ops_latest))
        .route("/api/app/ops/download", get(routes::ops_download))
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
        .nest("/api/codename", songlib::router("codename"))
        .nest("/api/funkin", songlib::router("funkin"))
        .nest("/api/psych", songlib::router("psych"))
        .nest("/api/nmv", songlib::router("nmv"))
        .nest("/api/kade", songlib::router("kade"))
        .nest("/api/gd", songlib::router("gd"))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

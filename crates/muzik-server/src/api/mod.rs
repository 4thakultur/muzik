mod auth;
mod extract;
mod library;
mod playlists;
mod scan;
pub mod serve_file;
mod stream;
mod users;

use axum::{
    extract::State,
    http::{header, HeaderValue},
    middleware,
    routing::{get, patch, post, put},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::{error::ApiResult, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/auth/setup", get(auth::setup_status).post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/login/2fa", post(auth::login_2fa))
        .route("/auth/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/me/library", put(auth::set_own_library))
        .route("/me/password", post(auth::change_password))
        .route("/me/2fa", get(auth::totp_status))
        .route("/me/2fa/setup", post(auth::totp_setup))
        .route("/me/2fa/enable", post(auth::totp_enable))
        .route("/me/2fa/disable", post(auth::totp_disable))
        .route("/me/2fa/recovery-codes", post(auth::totp_recovery_codes))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", patch(users::update))
        .route(
            "/admin/security",
            get(users::security_policy).put(users::set_security_policy),
        )
        .route(
            "/admin/inkult",
            get(users::inkult_settings).put(users::set_inkult_settings),
        )
        .route("/admin/inkult/test", post(users::test_inkult))
        .route("/admin/inkult/libraries", put(users::set_inkult_libraries))
        .route("/artists", get(library::artists))
        .route("/artists/{id}", get(library::artist))
        .route("/albums", get(library::albums))
        .route("/albums/{id}", get(library::album))
        .route("/albums/{id}/cover", get(library::album_cover))
        .route("/tracks/{id}", get(library::track))
        .route("/tracks/{id}/suggestions", get(library::suggestions))
        .route("/search", get(library::search))
        .route(
            "/playlists",
            get(playlists::library).post(playlists::create),
        )
        .route("/playlists/shared", get(playlists::shared))
        .route(
            "/playlists/{id}",
            get(playlists::get)
                .patch(playlists::update)
                .delete(playlists::delete),
        )
        .route("/playlists/{id}/tracks", post(playlists::add_tracks))
        .route(
            "/playlists/{id}/entries/{entry_id}",
            patch(playlists::move_entry).delete(playlists::remove_entry),
        )
        .route(
            "/playlists/{id}/follow",
            put(playlists::follow).delete(playlists::unfollow),
        )
        .route("/playlists/{id}/duplicate", post(playlists::duplicate))
        .route("/stream/{id}", get(stream::stream))
        .route("/scan", get(scan::status).post(scan::start))
        .route("/scan/events", get(scan::events))
        .fallback(|| async { crate::error::AppError::NotFound })
        .layer(middleware::from_fn(crate::security::same_origin_guard))
        // Personal data must never land in a shared cache; streams and covers
        // set their own (private) caching.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
}

async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    sqlx::query!("SELECT 1 AS one")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(json!({ "status": "ok" })))
}

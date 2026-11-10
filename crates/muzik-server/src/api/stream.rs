use axum::{extract::State, http::HeaderMap, response::Response};
use muzik_common::Quality;
use serde::Deserialize;

use super::extract::{Path, Query};
use super::serve_file::serve_file;
use crate::{
    auth::CurrentUser,
    error::{ApiResult, AppError},
    state::AppState,
    transcode,
};

#[derive(Debug, Deserialize)]
pub struct StreamParams {
    #[serde(default)]
    pub quality: Quality,
}

/// `GET /api/stream/{track_id}?quality=original|high|normal|low`
///
/// The client only sends an ID; the path comes from the database and is
/// re-checked against the library roots before opening. With separate
/// libraries, a song the user doesn't own in Inkult answers 404 like a
/// missing one.
pub async fn stream(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Query(p): Query<StreamParams>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let t = sqlx::query!(
        r#"SELECT path, mtime, mime, codec, lossless AS "lossless: bool", bitrate_kbps
           FROM tracks t WHERE t.id = ?1 AND (?2 IS NULL OR NOT EXISTS (SELECT 1 FROM inkult_files kf WHERE kf.path = t.path)
                    OR EXISTS (SELECT 1 FROM inkult_owners ko WHERE ko.path = t.path AND ko.owner_id = ?2))"#,
        id,
        user.library_owner
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let path = state
        .roots()
        .resolve(std::path::Path::new(&t.path))
        .map_err(|e| {
            tracing::warn!(track_id = id, error = %e, "refusing to stream track");
            AppError::NotFound
        })?;

    match transcode::plan(p.quality, t.lossless, &t.codec, t.bitrate_kbps) {
        None => serve_file(&path, &t.mime, &headers, "private, max-age=3600").await,
        Some(target) => {
            let out = state.transcoder.get(id, &path, t.mtime, target).await?;
            serve_file(&out, target.mime(), &headers, "private, max-age=3600").await
        }
    }
}

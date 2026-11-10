use std::{convert::Infallible, time::Duration};

use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures_util::{Stream, StreamExt};
use muzik_common::{ScanProgress, ScanRequest};
use tokio_stream::wrappers::WatchStream;

use super::extract::Json as JsonBody;
use crate::{
    auth::AdminUser,
    error::{ApiResult, AppError},
    state::AppState,
};

pub async fn status(State(state): State<AppState>, _admin: AdminUser) -> Json<ScanProgress> {
    Json(state.scanner.current())
}

pub async fn start(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    JsonBody(req): JsonBody<ScanRequest>,
) -> ApiResult<StatusCode> {
    state
        .scanner
        .start(req.mode)
        .map_err(|e| AppError::Conflict(e.to_string()))?;
    tracing::info!(by = admin.id, mode = ?req.mode, "scan requested");
    Ok(StatusCode::ACCEPTED)
}

/// Server-sent events with scan progress, throttled to ~4 updates/second.
pub async fn events(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let updates = WatchStream::new(state.scanner.subscribe());
    let stream = tokio_stream::StreamExt::throttle(updates, Duration::from_millis(250)).map(|p| {
        Ok(Event::default()
            .event("progress")
            .json_data(p)
            .unwrap_or_default())
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

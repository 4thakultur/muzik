//! Muzik server library: everything except process startup lives here so it
//! can be exercised from tests.

pub mod api;
pub mod auth;
pub mod config;
pub mod covers;
pub mod db;
pub mod error;
pub mod inkult;
pub mod inkult_index;
pub mod library;
pub mod security;
pub mod state;
pub mod transcode;

use axum::{
    http::{header, HeaderName, HeaderValue},
    Router,
};
use tower_http::{
    compression::{
        predicate::{NotForContentType, Predicate},
        CompressionLayer, DefaultPredicate,
    },
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

use state::AppState;

const CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; \
connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; \
frame-ancestors 'none'; form-action 'self'";

pub fn app(state: AppState) -> Router {
    let web_dir = state.config.web_dir.clone();
    // `no-cache` = always revalidate (cheap 304s via Last-Modified), so a
    // rebuilt UI shows up on the next load instead of a stale index.html that
    // still points at the previous WASM bundle.
    let spa = tower::ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .service(ServeDir::new(&web_dir).fallback(ServeFile::new(web_dir.join("index.html"))));

    // Audio is already compressed and must keep exact byte ranges.
    let compression = CompressionLayer::new()
        .compress_when(DefaultPredicate::new().and(NotForContentType::const_new("audio/")));

    let hsts = state.config.secure_cookies;
    let security_headers = [
        (header::CONTENT_SECURITY_POLICY, CSP),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
        (
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=()",
        ),
        (
            HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ),
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            "same-origin",
        ),
    ];

    let mut router = Router::new()
        .nest("/api", api::router())
        .fallback_service(spa)
        .with_state(state)
        .layer(compression)
        .layer(RequestBodyLimitLayer::new(8 * 1024 * 1024))
        .layer(TraceLayer::new_for_http());
    for (name, value) in security_headers {
        router = router.layer(SetResponseHeaderLayer::if_not_present(
            name,
            HeaderValue::from_static(value),
        ));
    }
    // With secure cookies Muzik is served over HTTPS (browsers ignore HSTS on
    // plain HTTP), so pin it there for a year.
    if hsts {
        router = router.layer(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        ));
    }
    router
}

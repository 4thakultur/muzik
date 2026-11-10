//! Request-level protections shared by every API route.

use axum::{
    extract::Request,
    http::{header, HeaderMap, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error::AppError;

/// Cross-site request forgery guard for state-changing requests.
///
/// `SameSite=Lax` session cookies already stop most CSRF; this adds a check
/// that also covers same-site attackers (another app on a sibling subdomain)
/// and old browsers. Modern browsers send `Sec-Fetch-Site`; otherwise the
/// `Origin` must match the host. Non-browser clients send neither and are let
/// through (they can't ride on a victim's cookies anyway).
pub async fn same_origin_guard(req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
        || is_same_origin(req.headers())
    {
        next.run(req).await
    } else {
        tracing::warn!(
            method = %req.method(),
            path = %req.uri().path(),
            origin = ?req.headers().get(header::ORIGIN),
            "blocked cross-origin request"
        );
        AppError::Forbidden.into_response()
    }
}

pub fn is_same_origin(h: &HeaderMap) -> bool {
    let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
    if let Some(site) = get("sec-fetch-site") {
        return matches!(site, "same-origin" | "none");
    }
    let Some(origin) = get("origin") else {
        return true;
    };
    // "scheme://host[:port]" → "host[:port]"
    let Some(origin_host) = origin.split_once("://").map(|(_, rest)| rest) else {
        return false; // includes the opaque "null" origin
    };
    [get("host"), get("x-forwarded-host")]
        .into_iter()
        .flatten()
        .any(|host| host.eq_ignore_ascii_case(origin_host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn same_origin_rules() {
        assert!(is_same_origin(&headers(&[(
            "sec-fetch-site",
            "same-origin"
        )])));
        assert!(!is_same_origin(&headers(&[(
            "sec-fetch-site",
            "cross-site"
        )])));
        assert!(!is_same_origin(&headers(&[(
            "sec-fetch-site",
            "same-site"
        )])));
        assert!(is_same_origin(&headers(&[])));
        assert!(is_same_origin(&headers(&[
            ("origin", "https://music.example.com"),
            ("host", "music.example.com"),
        ])));
        assert!(!is_same_origin(&headers(&[
            ("origin", "https://evil.example.com"),
            ("host", "music.example.com"),
        ])));
        assert!(!is_same_origin(&headers(&[
            ("origin", "null"),
            ("host", "music.example.com"),
        ])));
    }
}

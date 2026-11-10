//! Serving files with HTTP Range, ETag and conditional request support.

use std::path::Path;

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::error::{ApiResult, AppError};

const STREAM_BUF: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// No (usable) range: send the whole file.
    Full,
    /// Inclusive byte range.
    Partial {
        start: u64,
        end: u64,
    },
    Unsatisfiable,
}

/// Parses a `Range` header for a resource of `size` bytes. Only single ranges
/// are honoured; multi-range or malformed headers fall back to `Full`
/// (permitted by RFC 9110).
pub fn parse_range(value: &str, size: u64) -> RangeSpec {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeSpec::Full;
    };
    if spec.contains(',') {
        return RangeSpec::Full;
    }
    let Some((start, end)) = spec.trim().split_once('-') else {
        return RangeSpec::Full;
    };
    let (start, end) = (start.trim(), end.trim());
    match (start.is_empty(), end.is_empty()) {
        // bytes=-N : last N bytes
        (true, false) => {
            let Ok(n) = end.parse::<u64>() else {
                return RangeSpec::Full;
            };
            if n == 0 || size == 0 {
                return RangeSpec::Unsatisfiable;
            }
            RangeSpec::Partial {
                start: size.saturating_sub(n),
                end: size - 1,
            }
        }
        // bytes=S- or bytes=S-E
        (false, _) => {
            let Ok(s) = start.parse::<u64>() else {
                return RangeSpec::Full;
            };
            let e = if end.is_empty() {
                size.saturating_sub(1)
            } else {
                match end.parse::<u64>() {
                    Ok(e) if e >= s => e.min(size.saturating_sub(1)),
                    _ => return RangeSpec::Full,
                }
            };
            if s >= size {
                return RangeSpec::Unsatisfiable;
            }
            RangeSpec::Partial { start: s, end: e }
        }
        (true, true) => RangeSpec::Full,
    }
}

pub fn etag_for(size: u64, mtime_ms: i64) -> String {
    format!("\"{size:x}-{mtime_ms:x}\"")
}

fn etag_matches(header: &str, etag: &str) -> bool {
    header
        .split(',')
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == etag)
}

/// Streams `path` honouring `Range`, `If-Range` and `If-None-Match`.
/// `path` must already be validated to be inside the library or cache.
pub async fn serve_file(
    path: &Path,
    content_type: &str,
    req_headers: &HeaderMap,
    cache_control: &'static str,
) -> ApiResult<Response> {
    let mut file = tokio::fs::File::open(path).await?;
    let md = file.metadata().await?;
    let size = md.len();
    let mtime_ms = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let etag = etag_for(size, mtime_ms);

    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }

    if let Some(inm) = req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if etag_matches(inm, &etag) {
            return Ok((StatusCode::NOT_MODIFIED, headers).into_response());
        }
    }

    let if_range_ok = req_headers
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v.trim() == etag);
    let range = match req_headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        Some(r) if if_range_ok => parse_range(r, size),
        _ => RangeSpec::Full,
    };

    match range {
        RangeSpec::Unsatisfiable => Err(AppError::RangeNotSatisfiable { size }),
        RangeSpec::Full => {
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
            let body = Body::from_stream(ReaderStream::with_capacity(file, STREAM_BUF));
            Ok((StatusCode::OK, headers, body).into_response())
        }
        RangeSpec::Partial { start, end } => {
            let len = end - start + 1;
            file.seek(std::io::SeekFrom::Start(start)).await?;
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")) {
                headers.insert(header::CONTENT_RANGE, v);
            }
            let body = Body::from_stream(ReaderStream::with_capacity(file.take(len), STREAM_BUF));
            Ok((StatusCode::PARTIAL_CONTENT, headers, body).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn parses_ranges() {
        use RangeSpec::*;
        assert_eq!(
            parse_range("bytes=0-99", 1000),
            Partial { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range("bytes=500-", 1000),
            Partial {
                start: 500,
                end: 999
            }
        );
        assert_eq!(
            parse_range("bytes=-100", 1000),
            Partial {
                start: 900,
                end: 999
            }
        );
        assert_eq!(
            parse_range("bytes=-5000", 1000),
            Partial { start: 0, end: 999 }
        );
        assert_eq!(
            parse_range("bytes=900-5000", 1000),
            Partial {
                start: 900,
                end: 999
            }
        );
        assert_eq!(parse_range("bytes=1000-", 1000), Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,5-9", 1000), Full);
        assert_eq!(parse_range("bytes=9-1", 1000), Full);
        assert_eq!(parse_range("items=0-1", 1000), Full);
        assert_eq!(parse_range("bytes=abc-", 1000), Full);
    }

    async fn body_bytes(res: Response) -> Vec<u8> {
        res.into_body().collect().await.unwrap().to_bytes().to_vec()
    }

    #[tokio::test]
    async fn serves_partial_and_full() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.flac");
        let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        std::fs::write(&path, &data).unwrap();

        let res = serve_file(&path, "audio/flac", &HeaderMap::new(), "no-cache")
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "10000");
        assert_eq!(res.headers()[header::ACCEPT_RANGES], "bytes");
        let etag = res.headers()[header::ETAG].clone();
        assert_eq!(body_bytes(res).await, data);

        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=100-199".parse().unwrap());
        let res = serve_file(&path, "audio/flac", &h, "no-cache")
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 100-199/10000");
        assert_eq!(body_bytes(res).await, &data[100..200]);

        // Stale If-Range → full body.
        h.insert(header::IF_RANGE, "\"other\"".parse().unwrap());
        let res = serve_file(&path, "audio/flac", &h, "no-cache")
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, etag);
        let res = serve_file(&path, "audio/flac", &h, "no-cache")
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=20000-".parse().unwrap());
        let err = serve_file(&path, "audio/flac", &h, "no-cache")
            .await
            .unwrap_err();
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes */10000");
    }
}

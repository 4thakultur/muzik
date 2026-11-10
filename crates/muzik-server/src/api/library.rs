use axum::{extract::State, http::HeaderMap, response::Response, Json};
use muzik_common::{Album, AlbumDetail, Artist, ArtistDetail, Page, SearchResults, Track};
use serde::Deserialize;

use super::extract::{Path, Query};
use super::serve_file::serve_file;
use crate::{
    auth::CurrentUser,
    covers::{self, CoverSource},
    error::{ApiResult, AppError},
    state::AppState,
};

const MAX_LIMIT: i64 = 500;

#[derive(Debug, Deserialize)]
pub struct PageParams {
    #[serde(default)]
    pub offset: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    100
}

impl PageParams {
    fn clamp(&self) -> (i64, i64) {
        (self.offset.max(0), self.limit.clamp(1, MAX_LIMIT))
    }
}

pub async fn artists(
    State(state): State<AppState>,
    _user: CurrentUser,
    Query(p): Query<PageParams>,
) -> ApiResult<Json<Page<Artist>>> {
    let (offset, limit) = p.clamp();
    let total = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "n!: i64" FROM artists"#)
        .fetch_one(&state.pool)
        .await?;
    let items = sqlx::query_as!(
        Artist,
        r#"SELECT ar.id AS "id!", ar.name,
                  (SELECT COUNT(*) FROM albums al WHERE al.artist_id = ar.id) AS "album_count!: i64"
           FROM artists ar ORDER BY ar.sort_name LIMIT ? OFFSET ?"#,
        limit,
        offset
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(Page {
        items,
        total,
        offset,
        limit,
    }))
}

pub async fn artist(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ArtistDetail>> {
    let artist = sqlx::query_as!(
        Artist,
        r#"SELECT ar.id AS "id!", ar.name,
                  (SELECT COUNT(*) FROM albums al WHERE al.artist_id = ar.id) AS "album_count!: i64"
           FROM artists ar WHERE ar.id = ?"#,
        id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let albums = sqlx::query_as!(
        Album,
        r#"SELECT al.id AS "id!", al.title AS "title!", al.artist_id AS "artist_id!", ar.name AS "artist_name!", al.year,
                  COUNT(t.id) AS "track_count!: i64",
                  COALESCE(SUM(t.duration_ms), 0) AS "duration_ms!: i64",
                  (al.cover_track_id IS NOT NULL OR al.cover_path IS NOT NULL) AS "has_cover!: bool"
           FROM albums al
           JOIN artists ar ON ar.id = al.artist_id
           JOIN tracks t ON t.album_id = al.id
           WHERE al.artist_id = ?
           GROUP BY al.id
           ORDER BY al.year DESC, al.title"#,
        id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(ArtistDetail { artist, albums }))
}

#[derive(Debug, Deserialize)]
pub struct AlbumListParams {
    #[serde(default)]
    pub offset: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// `title` (default), `artist`, `year`, `recent`.
    pub sort: Option<String>,
}

pub async fn albums(
    State(state): State<AppState>,
    _user: CurrentUser,
    Query(p): Query<AlbumListParams>,
) -> ApiResult<Json<Page<Album>>> {
    let (offset, limit) = (p.offset.max(0), p.limit.clamp(1, MAX_LIMIT));
    let sort = match p.sort.as_deref() {
        None | Some("title") => 0,
        Some("artist") => 1,
        Some("year") => 2,
        Some("recent") => 3,
        Some(_) => return Err(AppError::BadRequest("invalid sort".into())),
    };
    let total = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "n!: i64" FROM albums"#)
        .fetch_one(&state.pool)
        .await?;
    let items = sqlx::query_as!(
        Album,
        r#"SELECT al.id AS "id!", al.title AS "title!", al.artist_id AS "artist_id!", ar.name AS "artist_name!", al.year,
                  COUNT(t.id) AS "track_count!: i64",
                  COALESCE(SUM(t.duration_ms), 0) AS "duration_ms!: i64",
                  (al.cover_track_id IS NOT NULL OR al.cover_path IS NOT NULL) AS "has_cover!: bool"
           FROM albums al
           JOIN artists ar ON ar.id = al.artist_id
           JOIN tracks t ON t.album_id = al.id
           GROUP BY al.id
           ORDER BY
             CASE WHEN ?1 = 0 THEN al.title END,
             CASE WHEN ?1 = 1 THEN ar.sort_name END, CASE WHEN ?1 = 1 THEN al.year END,
             CASE WHEN ?1 = 2 THEN al.year END DESC,
             CASE WHEN ?1 = 3 THEN MAX(t.id) END DESC,
             al.title
           LIMIT ?2 OFFSET ?3"#,
        sort,
        limit,
        offset
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(Page {
        items,
        total,
        offset,
        limit,
    }))
}

pub async fn album(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<AlbumDetail>> {
    let album = sqlx::query_as!(
        Album,
        r#"SELECT al.id AS "id!", al.title AS "title!", al.artist_id AS "artist_id!", ar.name AS "artist_name!", al.year,
                  COUNT(t.id) AS "track_count!: i64",
                  COALESCE(SUM(t.duration_ms), 0) AS "duration_ms!: i64",
                  (al.cover_track_id IS NOT NULL OR al.cover_path IS NOT NULL) AS "has_cover!: bool"
           FROM albums al
           JOIN artists ar ON ar.id = al.artist_id
           JOIN tracks t ON t.album_id = al.id
           WHERE al.id = ?
           GROUP BY al.id"#,
        id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let tracks = sqlx::query_as!(
        Track,
        r#"SELECT t.id AS "id!", t.title, t.artist_name, t.album_id, al.title AS album_title,
                  al.artist_id AS album_artist_id, t.disc_no, t.track_no, t.duration_ms, t.codec,
                  t.sample_rate, t.bit_depth, t.channels, t.bitrate_kbps,
                  t.lossless AS "lossless: bool"
           FROM tracks t JOIN albums al ON al.id = t.album_id
           WHERE t.album_id = ?
           ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.title"#,
        id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(AlbumDetail { album, tracks }))
}

pub async fn track(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Track>> {
    let track = sqlx::query_as!(
        Track,
        r#"SELECT t.id AS "id!", t.title, t.artist_name, t.album_id, al.title AS album_title,
                  al.artist_id AS album_artist_id, t.disc_no, t.track_no, t.duration_ms, t.codec,
                  t.sample_rate, t.bit_depth, t.channels, t.bitrate_kbps,
                  t.lossless AS "lossless: bool"
           FROM tracks t JOIN albums al ON al.id = t.album_id
           WHERE t.id = ?"#,
        id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(track))
}

#[derive(Debug, Deserialize)]
pub struct SuggestionParams {
    #[serde(default = "default_suggestions")]
    pub limit: i64,
}

fn default_suggestions() -> i64 {
    20
}

/// Tracks to suggest after `id`, from the local library only: same genre,
/// same album artist and a close release year score higher; ties (and the
/// rest of the library, as a fallback) come in random order so suggestions
/// vary. The seed's own album is excluded (it is usually already queued).
pub async fn suggestions(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<i64>,
    Query(p): Query<SuggestionParams>,
) -> ApiResult<Json<Vec<Track>>> {
    let limit = p.limit.clamp(1, 50);
    let exists = sqlx::query_scalar!(r#"SELECT 1 AS "x!: i64" FROM tracks WHERE id = ?"#, id)
        .fetch_optional(&state.pool)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let tracks = sqlx::query_as!(
        Track,
        r#"WITH seed AS (
               SELECT t.genre, t.year, t.album_id, al.artist_id
               FROM tracks t JOIN albums al ON al.id = t.album_id
               WHERE t.id = ?
           )
           SELECT t.id AS "id!", t.title, t.artist_name, t.album_id, al.title AS album_title,
                  al.artist_id AS album_artist_id, t.disc_no, t.track_no, t.duration_ms, t.codec,
                  t.sample_rate, t.bit_depth, t.channels, t.bitrate_kbps,
                  t.lossless AS "lossless: bool"
           FROM tracks t JOIN albums al ON al.id = t.album_id, seed s
           WHERE t.album_id != s.album_id
           ORDER BY
               (CASE WHEN s.genre IS NOT NULL AND t.genre = s.genre COLLATE NOCASE THEN 3 ELSE 0 END)
             + (CASE WHEN al.artist_id = s.artist_id THEN 2 ELSE 0 END)
             + (CASE WHEN s.year IS NOT NULL AND abs(t.year - s.year) <= 5 THEN 1 ELSE 0 END) DESC,
               random()
           LIMIT ?"#,
        id,
        limit
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(tracks))
}

#[derive(Debug, Deserialize)]
pub struct CoverParams {
    #[serde(default = "default_cover_size")]
    pub size: u32,
}

fn default_cover_size() -> u32 {
    300
}

pub async fn album_cover(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<i64>,
    Query(p): Query<CoverParams>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let row = sqlx::query!(
        r#"SELECT al.cover_path, t.path AS "track_path?", t.mtime AS "track_mtime?"
           FROM albums al LEFT JOIN tracks t ON t.id = al.cover_track_id
           WHERE al.id = ?"#,
        id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let (source, signature) = match (row.track_path, row.track_mtime, row.cover_path) {
        (Some(path), Some(mtime), _) => {
            let sig = format!("t{mtime:x}");
            (CoverSource::Embedded(path.into()), sig)
        }
        (_, _, Some(path)) => {
            let mtime = tokio::fs::metadata(&path)
                .await?
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (CoverSource::File(path.into()), format!("f{mtime:x}"))
        }
        _ => return Err(AppError::NotFound),
    };

    let thumb = covers::thumbnail(
        &state.config.cache_dir,
        state.roots(),
        id,
        p.size,
        signature,
        source,
    )
    .await?;
    serve_file(&thumb, "image/webp", &headers, "private, max-age=604800").await
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: i64,
}

fn default_search_limit() -> i64 {
    10
}

/// Turns user input into a safe FTS5 query: every word becomes a quoted prefix
/// term, so FTS operators in the input are never interpreted.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .take(8)
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub async fn search(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(p): Query<SearchParams>,
) -> ApiResult<Json<SearchResults>> {
    let Some(q) = fts_query(&p.q) else {
        return Ok(Json(SearchResults::default()));
    };
    let limit = p.limit.clamp(1, 50);

    let artists = sqlx::query_as!(
        Artist,
        r#"SELECT ar.id AS "id!", ar.name,
                  (SELECT COUNT(*) FROM albums al WHERE al.artist_id = ar.id) AS "album_count!: i64"
           FROM search_fts f JOIN artists ar ON ar.id = f.ref_id
           WHERE search_fts MATCH ? AND f.kind = 'artist'
           ORDER BY f.rank LIMIT ?"#,
        q,
        limit
    )
    .fetch_all(&state.pool)
    .await?;

    let albums = sqlx::query_as!(
        Album,
        r#"SELECT al.id AS "id!", al.title AS "title!", al.artist_id AS "artist_id!", ar.name AS "artist_name!", al.year,
                  (SELECT COUNT(*) FROM tracks t WHERE t.album_id = al.id) AS "track_count!: i64",
                  (SELECT COALESCE(SUM(duration_ms), 0) FROM tracks t WHERE t.album_id = al.id) AS "duration_ms!: i64",
                  (al.cover_track_id IS NOT NULL OR al.cover_path IS NOT NULL) AS "has_cover!: bool"
           FROM search_fts f
           JOIN albums al ON al.id = f.ref_id
           JOIN artists ar ON ar.id = al.artist_id
           WHERE search_fts MATCH ? AND f.kind = 'album'
           ORDER BY f.rank LIMIT ?"#,
        q,
        limit
    )
    .fetch_all(&state.pool)
    .await?;

    let tracks = sqlx::query_as!(
        Track,
        r#"SELECT t.id AS "id!", t.title, t.artist_name, t.album_id, al.title AS album_title,
                  al.artist_id AS album_artist_id, t.disc_no, t.track_no, t.duration_ms, t.codec,
                  t.sample_rate, t.bit_depth, t.channels, t.bitrate_kbps,
                  t.lossless AS "lossless: bool"
           FROM search_fts f
           JOIN tracks t ON t.id = f.ref_id
           JOIN albums al ON al.id = t.album_id
           WHERE search_fts MATCH ? AND f.kind = 'track'
           ORDER BY f.rank LIMIT ?"#,
        q,
        limit
    )
    .fetch_all(&state.pool)
    .await?;

    let playlists = super::playlists::summaries(
        &state.pool,
        user.id,
        super::playlists::Scope::Search(&p.q),
        limit,
    )
    .await?;

    Ok(Json(SearchResults {
        artists,
        albums,
        tracks,
        playlists,
    }))
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn fts_query_is_sanitized() {
        assert_eq!(fts_query("  "), None);
        assert_eq!(
            fts_query("pink floyd").as_deref(),
            Some("\"pink\"* \"floyd\"*")
        );
        assert_eq!(
            fts_query("a\" OR b*").as_deref(),
            Some("\"a\"* \"OR\"* \"b\"*")
        );
        assert_eq!(fts_query("NEAR(").as_deref(), Some("\"NEAR\"*"));
    }
}

//! Playlists: CRUD, songs (add / remove / reorder), sharing and saving.
//!
//! Visibility: a playlist is visible to its owner, and to everyone when
//! `public`. Invisible playlists answer 404 (their existence is not leaked).
//! Songs can be edited by the owner, or by anyone when public + collaborative.
//! Details, sharing and deletion are owner-only.

use std::collections::HashSet;

use axum::{extract::State, http::StatusCode, Json};
use muzik_common::{
    AddTracksRequest, AddTracksResult, CreatePlaylistRequest, MoveEntryRequest, PlaylistDetail,
    PlaylistEntry, PlaylistSummary, Track, UpdatePlaylistRequest, PLAYLIST_DESCRIPTION_MAX,
    PLAYLIST_MAX_TRACKS, PLAYLIST_NAME_MAX,
};
use sqlx::{SqliteConnection, SqlitePool};

use super::extract::{Json as JsonBody, Path};
use crate::{
    auth::CurrentUser,
    db::now,
    error::{ApiResult, AppError},
    state::AppState,
};

/// Which playlists [`summaries`] returns.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// Owned by the user, or saved by them (and still shared).
    Library,
    /// A single playlist, if visible.
    One(i64),
    /// Shared by other users.
    Shared,
    /// Visible playlists whose name contains the text.
    Search(&'a str),
}

pub async fn summaries(
    pool: &SqlitePool,
    me: i64,
    scope: Scope<'_>,
    limit: i64,
) -> Result<Vec<PlaylistSummary>, sqlx::Error> {
    let (mode, id, pattern) = match scope {
        Scope::Library => ("library", 0, String::new()),
        Scope::One(id) => ("one", id, String::new()),
        Scope::Shared => ("shared", 0, String::new()),
        Scope::Search(q) => ("search", 0, like_pattern(q)),
    };
    let rows = sqlx::query!(
        r#"SELECT p.id AS "id!", p.name, p.description, p.owner_id, u.display_name AS owner_name,
                  p.public AS "public: bool", p.collaborative AS "collaborative: bool",
                  p.created_at, p.updated_at,
                  fl.user_id IS NOT NULL AS "following!: bool",
                  (SELECT COUNT(*) FROM playlist_tracks pt
                    WHERE pt.playlist_id = p.id) AS "track_count!: i64",
                  (SELECT COALESCE(SUM(pt.duration_ms), 0) FROM playlist_tracks pt
                    WHERE pt.playlist_id = p.id AND pt.track_id IS NOT NULL) AS "duration_ms!: i64",
                  (SELECT group_concat(album_id) FROM (
                      SELECT t.album_id FROM playlist_tracks pt
                      JOIN tracks t ON t.id = pt.track_id
                      JOIN albums al ON al.id = t.album_id
                      WHERE pt.playlist_id = p.id AND pt.position < 200
                        AND (al.cover_track_id IS NOT NULL OR al.cover_path IS NOT NULL)
                      GROUP BY t.album_id ORDER BY MIN(pt.position) LIMIT 4)) AS "covers?: String"
           FROM playlists p
           JOIN users u ON u.id = p.owner_id
           LEFT JOIN playlist_follows fl ON fl.playlist_id = p.id AND fl.user_id = ?1
           WHERE CASE ?2
               WHEN 'library' THEN p.owner_id = ?1 OR (fl.user_id IS NOT NULL AND p.public = 1)
               WHEN 'one' THEN p.id = ?3 AND (p.owner_id = ?1 OR p.public = 1)
               WHEN 'shared' THEN p.public = 1 AND p.owner_id <> ?1
               ELSE (p.owner_id = ?1 OR p.public = 1) AND p.name LIKE ?4 ESCAPE '\'
           END
           ORDER BY MAX(p.updated_at, COALESCE(fl.created_at, 0)) DESC, p.id DESC
           LIMIT ?5"#,
        me,
        mode,
        id,
        pattern,
        limit
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let is_owner = r.owner_id == me;
            PlaylistSummary {
                id: r.id,
                name: r.name,
                description: r.description,
                owner_id: r.owner_id,
                owner_name: r.owner_name,
                public: r.public,
                collaborative: r.collaborative,
                track_count: r.track_count,
                duration_ms: r.duration_ms,
                cover_album_ids: r
                    .covers
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.parse().ok())
                    .collect(),
                created_at: r.created_at,
                updated_at: r.updated_at,
                is_owner,
                can_edit: is_owner || (r.public && r.collaborative),
                following: r.following,
            }
        })
        .collect())
}

async fn summary(pool: &SqlitePool, me: i64, id: i64) -> ApiResult<PlaylistSummary> {
    summaries(pool, me, Scope::One(id), 1)
        .await?
        .pop()
        .ok_or(AppError::NotFound)
}

/// `LIKE` pattern matching `q` anywhere, with wildcards in `q` escaped.
fn like_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.trim().chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name: String = name
        .chars()
        .filter(|c| !super::auth::is_unsafe_char(*c))
        .collect();
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("playlist name is required".into()));
    }
    if name.chars().count() > PLAYLIST_NAME_MAX {
        return Err(AppError::BadRequest(format!(
            "playlist name is limited to {PLAYLIST_NAME_MAX} characters"
        )));
    }
    Ok(name.to_owned())
}

fn clean_description(text: &str) -> ApiResult<String> {
    let text: String = text
        .chars()
        .filter(|c| *c == '\n' || !super::auth::is_unsafe_char(*c))
        .collect();
    let text = text.trim();
    if text.chars().count() > PLAYLIST_DESCRIPTION_MAX {
        return Err(AppError::BadRequest(format!(
            "description is limited to {PLAYLIST_DESCRIPTION_MAX} characters"
        )));
    }
    Ok(text.to_owned())
}

struct Access {
    owner_id: i64,
    public: bool,
    collaborative: bool,
}

impl Access {
    async fn load(conn: &mut SqliteConnection, id: i64, me: i64) -> ApiResult<Self> {
        let row = sqlx::query!(
            r#"SELECT owner_id, public AS "public: bool", collaborative AS "collaborative: bool"
               FROM playlists WHERE id = ?"#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(AppError::NotFound)?;
        if row.owner_id != me && !row.public {
            return Err(AppError::NotFound);
        }
        Ok(Self {
            owner_id: row.owner_id,
            public: row.public,
            collaborative: row.collaborative,
        })
    }

    fn require_owner(&self, me: i64) -> ApiResult<()> {
        if self.owner_id == me {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    fn require_edit(&self, me: i64) -> ApiResult<()> {
        if self.owner_id == me || (self.public && self.collaborative) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

/// Bumps `updated_at`. Run first in every write transaction: taking the write
/// lock before any read avoids SQLite's busy-snapshot upgrade failure.
async fn touch(conn: &mut SqliteConnection, id: i64) -> ApiResult<()> {
    let now = now();
    sqlx::query!("UPDATE playlists SET updated_at = ? WHERE id = ?", now, id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Inserts `track_ids` (in order) at `position` (end when `None`).
/// Returns `(added, skipped duplicates)`.
async fn insert_tracks(
    conn: &mut SqliteConnection,
    playlist_id: i64,
    me: i64,
    track_ids: &[i64],
    position: Option<i64>,
    allow_duplicates: bool,
) -> ApiResult<(i64, i64)> {
    if track_ids.len() as i64 > PLAYLIST_MAX_TRACKS {
        return Err(AppError::BadRequest(format!(
            "a playlist holds at most {PLAYLIST_MAX_TRACKS} songs"
        )));
    }
    let ids_json = serde_json::to_string(track_ids).map_err(crate::error::internal)?;
    let rows = sqlx::query!(
        r#"SELECT t.id AS "id!", t.title, t.artist_name, al.title AS album_title, t.duration_ms
           FROM json_each(?) j
           JOIN tracks t ON t.id = j.value
           JOIN albums al ON al.id = t.album_id
           ORDER BY j.key"#,
        ids_json
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() != track_ids.len() {
        return Err(AppError::BadRequest("unknown track id".into()));
    }

    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM playlist_tracks WHERE playlist_id = ?"#,
        playlist_id
    )
    .fetch_one(&mut *conn)
    .await?;

    let mut duplicates = 0;
    let rows = if allow_duplicates {
        rows
    } else {
        let mut present: HashSet<i64> = sqlx::query_scalar!(
            r#"SELECT track_id AS "id!" FROM playlist_tracks
               WHERE playlist_id = ? AND track_id IS NOT NULL"#,
            playlist_id
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect();
        let before = rows.len();
        let kept: Vec<_> = rows.into_iter().filter(|r| present.insert(r.id)).collect();
        duplicates = (before - kept.len()) as i64;
        kept
    };
    let n = rows.len() as i64;
    if n == 0 {
        return Ok((0, duplicates));
    }
    if count + n > PLAYLIST_MAX_TRACKS {
        return Err(AppError::BadRequest(format!(
            "a playlist holds at most {PLAYLIST_MAX_TRACKS} songs"
        )));
    }

    let at = position.unwrap_or(count).clamp(0, count);
    sqlx::query!(
        "UPDATE playlist_tracks SET position = position + ? WHERE playlist_id = ? AND position >= ?",
        n,
        playlist_id,
        at
    )
    .execute(&mut *conn)
    .await?;
    let now = now();
    for (i, r) in rows.iter().enumerate() {
        let pos = at + i as i64;
        sqlx::query!(
            "INSERT INTO playlist_tracks
               (playlist_id, track_id, position, added_by, added_at, title, artist_name, album_title, duration_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            playlist_id,
            r.id,
            pos,
            me,
            now,
            r.title,
            r.artist_name,
            r.album_title,
            r.duration_ms
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok((n, duplicates))
}

// ---------- Handlers ----------

/// The user's playlists: owned and saved, most recently active first.
pub async fn library(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<PlaylistSummary>>> {
    Ok(Json(
        summaries(&state.pool, user.id, Scope::Library, 1000).await?,
    ))
}

/// Playlists other users share on this server.
pub async fn shared(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<PlaylistSummary>>> {
    Ok(Json(
        summaries(&state.pool, user.id, Scope::Shared, 50).await?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(req): JsonBody<CreatePlaylistRequest>,
) -> ApiResult<(StatusCode, Json<PlaylistSummary>)> {
    let name = clean_name(&req.name)?;
    let description = clean_description(req.description.as_deref().unwrap_or(""))?;
    let collaborative = req.collaborative && req.public;
    let now = now();
    let mut tx = state.pool.begin().await?;
    let id = sqlx::query_scalar!(
        r#"INSERT INTO playlists (owner_id, name, description, public, collaborative, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id AS "id!""#,
        user.id,
        name,
        description,
        req.public,
        collaborative,
        now,
        now
    )
    .fetch_one(&mut *tx)
    .await?;
    if !req.track_ids.is_empty() {
        insert_tracks(&mut tx, id, user.id, &req.track_ids, None, true).await?;
    }
    tx.commit().await?;
    tracing::info!(user_id = user.id, playlist_id = id, "playlist created");
    Ok((
        StatusCode::CREATED,
        Json(summary(&state.pool, user.id, id).await?),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<PlaylistDetail>> {
    let playlist = summary(&state.pool, user.id, id).await?;
    let rows = sqlx::query!(
        r#"SELECT pt.id AS "entry_id!", pt.added_at, ub.display_name AS "added_by?",
                  pt.title AS snap_title, pt.artist_name AS snap_artist,
                  pt.album_title AS snap_album, pt.duration_ms AS snap_duration,
                  t.id AS "track_id?", t.title AS "title?", t.artist_name AS "artist_name?",
                  t.album_id AS "album_id?", al.title AS "album_title?",
                  al.artist_id AS "album_artist_id?", t.disc_no, t.track_no,
                  t.duration_ms AS "duration_ms?", t.codec AS "codec?", t.sample_rate,
                  t.bit_depth, t.channels, t.bitrate_kbps, t.lossless AS "lossless?: bool"
           FROM playlist_tracks pt
           LEFT JOIN tracks t ON t.id = pt.track_id
           LEFT JOIN albums al ON al.id = t.album_id
           LEFT JOIN users ub ON ub.id = pt.added_by
           WHERE pt.playlist_id = ?
           ORDER BY pt.position"#,
        id
    )
    .fetch_all(&state.pool)
    .await?;

    let entries = rows
        .into_iter()
        .map(|r| {
            let track = match (
                r.track_id,
                r.title,
                r.artist_name,
                r.album_id,
                r.album_title,
                r.album_artist_id,
                r.duration_ms,
                r.codec,
            ) {
                (
                    Some(id),
                    Some(title),
                    Some(artist_name),
                    Some(album_id),
                    Some(album_title),
                    Some(album_artist_id),
                    Some(duration_ms),
                    Some(codec),
                ) => Some(Track {
                    id,
                    title,
                    artist_name,
                    album_id,
                    album_title,
                    album_artist_id,
                    disc_no: r.disc_no,
                    track_no: r.track_no,
                    duration_ms,
                    codec,
                    sample_rate: r.sample_rate,
                    bit_depth: r.bit_depth,
                    channels: r.channels,
                    bitrate_kbps: r.bitrate_kbps,
                    lossless: r.lossless.unwrap_or(false),
                }),
                _ => None,
            };
            PlaylistEntry {
                entry_id: r.entry_id,
                added_at: r.added_at,
                added_by: r.added_by,
                title: r.snap_title,
                artist_name: r.snap_artist,
                album_title: r.snap_album,
                duration_ms: r.snap_duration,
                track,
            }
        })
        .collect();
    Ok(Json(PlaylistDetail { playlist, entries }))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    JsonBody(req): JsonBody<UpdatePlaylistRequest>,
) -> ApiResult<Json<PlaylistSummary>> {
    let name = req.name.as_deref().map(clean_name).transpose()?;
    let description = req
        .description
        .as_deref()
        .map(clean_description)
        .transpose()?;
    let mut tx = state.pool.begin().await?;
    touch(&mut tx, id).await?;
    let access = Access::load(&mut tx, id, user.id).await?;
    access.require_owner(user.id)?;
    let public = req.public.unwrap_or(access.public);
    let collaborative = req.collaborative.unwrap_or(access.collaborative) && public;
    sqlx::query!(
        "UPDATE playlists SET name = COALESCE(?, name), description = COALESCE(?, description),
                public = ?, collaborative = ? WHERE id = ?",
        name,
        description,
        public,
        collaborative,
        id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(summary(&state.pool, user.id, id).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let mut tx = state.pool.begin().await?;
    touch(&mut tx, id).await?;
    Access::load(&mut tx, id, user.id)
        .await?
        .require_owner(user.id)?;
    sqlx::query!("DELETE FROM playlists WHERE id = ?", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(user_id = user.id, playlist_id = id, "playlist deleted");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_tracks(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    JsonBody(req): JsonBody<AddTracksRequest>,
) -> ApiResult<Json<AddTracksResult>> {
    if req.track_ids.is_empty() {
        return Err(AppError::BadRequest("no tracks to add".into()));
    }
    let mut tx = state.pool.begin().await?;
    touch(&mut tx, id).await?;
    Access::load(&mut tx, id, user.id)
        .await?
        .require_edit(user.id)?;
    let (added, duplicates) = insert_tracks(
        &mut tx,
        id,
        user.id,
        &req.track_ids,
        req.position,
        req.allow_duplicates,
    )
    .await?;
    if added == 0 {
        // Nothing changed: don't bump `updated_at`.
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(AddTracksResult {
        added,
        duplicates,
        playlist: summary(&state.pool, user.id, id).await?,
    }))
}

pub async fn remove_entry(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, entry_id)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    let mut tx = state.pool.begin().await?;
    touch(&mut tx, id).await?;
    Access::load(&mut tx, id, user.id)
        .await?
        .require_edit(user.id)?;
    let pos = sqlx::query_scalar!(
        "SELECT position FROM playlist_tracks WHERE id = ? AND playlist_id = ?",
        entry_id,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    sqlx::query!("DELETE FROM playlist_tracks WHERE id = ?", entry_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "UPDATE playlist_tracks SET position = position - 1 WHERE playlist_id = ? AND position > ?",
        id,
        pos
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn move_entry(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, entry_id)): Path<(i64, i64)>,
    JsonBody(req): JsonBody<MoveEntryRequest>,
) -> ApiResult<StatusCode> {
    let mut tx = state.pool.begin().await?;
    touch(&mut tx, id).await?;
    Access::load(&mut tx, id, user.id)
        .await?
        .require_edit(user.id)?;
    let from = sqlx::query_scalar!(
        "SELECT position FROM playlist_tracks WHERE id = ? AND playlist_id = ?",
        entry_id,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM playlist_tracks WHERE playlist_id = ?"#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    let to = req.position.clamp(0, count - 1);
    if to > from {
        sqlx::query!(
            "UPDATE playlist_tracks SET position = position - 1
             WHERE playlist_id = ? AND position > ? AND position <= ?",
            id,
            from,
            to
        )
        .execute(&mut *tx)
        .await?;
    } else if to < from {
        sqlx::query!(
            "UPDATE playlist_tracks SET position = position + 1
             WHERE playlist_id = ? AND position >= ? AND position < ?",
            id,
            to,
            from
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE playlist_tracks SET position = ? WHERE id = ?",
        to,
        entry_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Saves another user's shared playlist to the library.
pub async fn follow(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<PlaylistSummary>> {
    let p = summary(&state.pool, user.id, id).await?;
    if p.is_owner {
        return Err(AppError::BadRequest(
            "this playlist is already yours".into(),
        ));
    }
    let now = now();
    sqlx::query!(
        "INSERT INTO playlist_follows (user_id, playlist_id, created_at) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
        user.id,
        id,
        now
    )
    .execute(&state.pool)
    .await?;
    Ok(Json(summary(&state.pool, user.id, id).await?))
}

pub async fn unfollow(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    sqlx::query!(
        "DELETE FROM playlist_follows WHERE user_id = ? AND playlist_id = ?",
        user.id,
        id
    )
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Copies a visible playlist (songs and order) into a new private playlist
/// owned by the user.
pub async fn duplicate(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<(StatusCode, Json<PlaylistSummary>)> {
    let src = summary(&state.pool, user.id, id).await?;
    let name: String = if src.is_owner {
        let base: String = src.name.chars().take(PLAYLIST_NAME_MAX - 7).collect();
        format!("{base} (Copy)")
    } else {
        src.name.clone()
    };
    let now = now();
    let mut tx = state.pool.begin().await?;
    let new_id = sqlx::query_scalar!(
        r#"INSERT INTO playlists (owner_id, name, description, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?) RETURNING id AS "id!""#,
        user.id,
        name,
        src.description,
        now,
        now
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO playlist_tracks
           (playlist_id, track_id, position, added_by, added_at, title, artist_name, album_title, duration_ms)
         SELECT ?, track_id, position, ?, ?, title, artist_name, album_title, duration_ms
         FROM playlist_tracks WHERE playlist_id = ? ORDER BY position",
        new_id,
        user.id,
        now,
        id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(summary(&state.pool, user.id, new_id).await?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern(" road "), "%road%");
        assert_eq!(like_pattern("100%_a\\b"), "%100\\%\\_a\\\\b%");
    }

    #[test]
    fn names_are_cleaned_and_bounded() {
        assert_eq!(clean_name("  Road\u{7}trip ").unwrap(), "Roadtrip");
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(PLAYLIST_NAME_MAX + 1)).is_err());
        assert_eq!(clean_description(" a\nb\t ").unwrap(), "a\nb");
    }
}

//! Incremental library scanner.
//!
//! 1. Walk every library root and `stat` supported files.
//! 2. Compare (path, size, mtime) with the database; only new/changed files
//!    are opened and their tags read (in parallel, on the blocking pool).
//! 3. Remove tracks whose files disappeared — but only under roots that were
//!    actually readable, so an unmounted drive never wipes the library.
//! 4. Drop orphan albums/artists, resolve covers, rebuild the search index.
//!
//! The scanner only ever reads from the library.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures_util::{stream, StreamExt};
use muzik_common::{ScanMode, ScanPhase, ScanProgress};
use sqlx::SqlitePool;
use tokio::sync::{watch, Mutex};

use super::{
    metadata::{self, TrackMeta},
    paths::LibraryRoots,
};
use crate::{db::now, inkult_index::InkultIndex};

const READ_CHUNK: usize = 32;
const WRITE_BATCH: usize = 256;

/// Shared scan coordinator: one scan at a time, progress broadcast via `watch`.
#[derive(Clone)]
pub struct Scanner {
    pool: SqlitePool,
    library_paths: Arc<Vec<PathBuf>>,
    roots: Arc<std::sync::RwLock<LibraryRoots>>,
    lock: Arc<Mutex<()>>,
    progress: Arc<watch::Sender<ScanProgress>>,
    inkult_index: Arc<InkultIndex>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("a scan is already running")]
    AlreadyRunning,
}

#[derive(Debug, Clone)]
struct FileEntry {
    path: PathBuf,
    size: i64,
    mtime: i64,
}

impl Scanner {
    pub fn new(
        pool: SqlitePool,
        library_paths: Vec<PathBuf>,
        roots: Arc<std::sync::RwLock<LibraryRoots>>,
        inkult_index: Arc<InkultIndex>,
    ) -> Self {
        let (tx, _) = watch::channel(ScanProgress::default());
        Self {
            pool,
            library_paths: Arc::new(library_paths),
            roots,
            lock: Arc::new(Mutex::new(())),
            progress: Arc::new(tx),
            inkult_index,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<ScanProgress> {
        self.progress.subscribe()
    }

    pub fn current(&self) -> ScanProgress {
        self.progress.borrow().clone()
    }

    /// Starts a scan in the background. Fails if one is already running.
    pub fn start(&self, mode: ScanMode) -> Result<(), ScanError> {
        let guard = self
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| ScanError::AlreadyRunning)?;
        let this = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            this.run(mode).await;
        });
        Ok(())
    }

    /// Runs a scan to completion (waits if another one is running).
    pub async fn run_blocking(&self, mode: ScanMode) {
        let _guard = self.lock.lock().await;
        self.run(mode).await;
    }

    async fn run(&self, mode: ScanMode) {
        let started = std::time::Instant::now();
        self.progress.send_replace(ScanProgress {
            phase: ScanPhase::Walking,
            mode: Some(mode),
            started_at: Some(now()),
            ..Default::default()
        });
        tracing::info!(?mode, "library scan started");
        match self.scan(mode).await {
            Ok(()) => {
                self.progress.send_modify(|p| {
                    p.phase = ScanPhase::Done;
                    p.finished_at = Some(now());
                });
                let p = self.current();
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    seen = p.files_seen,
                    added = p.added,
                    updated = p.updated,
                    removed = p.removed,
                    errors = p.errors,
                    "library scan finished"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "library scan failed");
                self.progress.send_modify(|p| {
                    p.phase = ScanPhase::Failed;
                    p.finished_at = Some(now());
                    p.message = Some(e.to_string());
                });
            }
        }
    }

    async fn scan(&self, mode: ScanMode) -> anyhow::Result<()> {
        // Refresh roots so drives mounted after startup are picked up.
        let roots = LibraryRoots::new(&self.library_paths);
        if let Ok(mut r) = self.roots.write() {
            *r = roots.clone();
        }

        let (files, walked_roots) = {
            let roots = roots.clone();
            tokio::task::spawn_blocking(move || walk(&roots)).await?
        };
        self.progress
            .send_modify(|p| p.files_seen = files.len() as u64);

        let existing: HashMap<String, (i64, i64)> =
            sqlx::query!(r#"SELECT path, size, mtime FROM tracks"#)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|r| (r.path, (r.size, r.mtime)))
                .collect();

        // Removals: DB paths not seen on disk, restricted to readable roots.
        let seen: std::collections::HashSet<&str> =
            files.iter().filter_map(|f| f.path.to_str()).collect();
        let to_remove: Vec<String> = existing
            .keys()
            .filter(|p| !seen.contains(p.as_str()))
            .filter(|p| walked_roots.iter().any(|r| Path::new(p).starts_with(r)))
            .cloned()
            .collect();

        let to_read: Vec<FileEntry> = files
            .into_iter()
            .filter(|f| {
                mode == ScanMode::Full
                    || f.path
                        .to_str()
                        .and_then(|p| existing.get(p))
                        .is_none_or(|&(size, mtime)| size != f.size || mtime != f.mtime)
            })
            .collect();

        self.progress.send_modify(|p| {
            p.phase = ScanPhase::Reading;
            p.files_to_read = to_read.len() as u64;
        });

        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 16);
        let mut results = stream::iter(to_read.chunks(READ_CHUNK).map(<[FileEntry]>::to_vec))
            .map(|chunk| {
                tokio::task::spawn_blocking(move || {
                    chunk
                        .into_iter()
                        .map(|f| {
                            let meta = metadata::read(&f.path);
                            (f, meta)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .buffer_unordered(workers);

        let mut batch = Vec::with_capacity(WRITE_BATCH);
        while let Some(chunk) = results.next().await {
            for (file, meta) in chunk? {
                match meta {
                    Ok(meta) => batch.push((file, meta)),
                    Err(e) => {
                        tracing::warn!(path = %file.path.display(), error = %e, "unreadable audio file");
                        self.progress.send_modify(|p| p.errors += 1);
                    }
                }
                self.progress.send_modify(|p| p.files_read += 1);
            }
            if batch.len() >= WRITE_BATCH {
                self.write_batch(std::mem::take(&mut batch), &existing)
                    .await?;
            }
        }
        self.write_batch(batch, &existing).await?;

        // New files may have arrived with an Inkult download: pick up their
        // owners now rather than at the next poll. Failures are logged and
        // keep the previous copy.
        if self.inkult_index.configured() {
            let _ = self.inkult_index.sync(false).await;
        }

        self.progress
            .send_modify(|p| p.phase = ScanPhase::Finalizing);
        let mut tx = self.pool.begin().await?;
        for path in &to_remove {
            sqlx::query!("DELETE FROM tracks WHERE path = ?", path)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.progress
            .send_modify(|p| p.removed = to_remove.len() as u64);

        finalize(&self.pool).await?;
        Ok(())
    }

    async fn write_batch(
        &self,
        batch: Vec<(FileEntry, TrackMeta)>,
        existing: &HashMap<String, (i64, i64)>,
    ) -> anyhow::Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let (mut added, mut updated) = (0, 0);
        let mut tx = self.pool.begin().await?;
        for (file, meta) in &batch {
            let Some(path) = file.path.to_str() else {
                continue;
            };
            upsert_track(&mut tx, file, path, meta).await?;
            if existing.contains_key(path) {
                updated += 1;
            } else {
                added += 1;
            }
        }
        tx.commit().await?;
        self.progress.send_modify(|p| {
            p.added += added;
            p.updated += updated;
        });
        Ok(())
    }
}

/// Walks all roots; returns supported files and the roots that were readable.
fn walk(roots: &LibraryRoots) -> (Vec<FileEntry>, Vec<PathBuf>) {
    let mut files = Vec::new();
    let mut walked = Vec::new();
    for root in roots.roots() {
        // An empty root is most likely an unmounted drive: don't treat its
        // tracks as deleted.
        match std::fs::read_dir(root) {
            Ok(mut it) => {
                if it.next().is_none() {
                    tracing::warn!(root = %root.display(), "library root is empty; skipping it");
                    continue;
                }
                walked.push(root.clone());
            }
            Err(e) => {
                tracing::warn!(root = %root.display(), error = %e, "library root unreadable");
                continue;
            }
        }
        for entry in walkdir::WalkDir::new(root).follow_links(true) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!(error = %e, "walk error");
                    continue;
                }
            };
            if !entry.file_type().is_file() || !metadata::is_supported(entry.path()) {
                continue;
            }
            // Resolve symlinks and make sure the target stays inside the library.
            let Ok(path) = roots.resolve(entry.path()) else {
                continue;
            };
            let Ok(md) = std::fs::metadata(&path) else {
                continue;
            };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            files.push(FileEntry {
                path,
                size: md.len() as i64,
                mtime,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    (files, walked)
}

fn folder_name(path: &Path, up: usize) -> Option<String> {
    let mut p = path.parent()?;
    for _ in 0..up {
        p = p.parent()?;
    }
    p.file_name().and_then(|n| n.to_str()).map(str::to_owned)
}

/// Strips a leading "The " for sorting.
fn sort_name(name: &str) -> String {
    let lower = name.to_lowercase();
    lower
        .strip_prefix("the ")
        .map(str::to_owned)
        .unwrap_or(lower)
}

async fn upsert_track(
    tx: &mut sqlx::SqliteConnection,
    file: &FileEntry,
    path: &str,
    meta: &TrackMeta,
) -> anyhow::Result<()> {
    // Grouping: album artist → artist → grandparent folder.
    let album_artist = meta
        .album_artist
        .clone()
        .or_else(|| meta.artist.clone())
        .or_else(|| folder_name(&file.path, 1))
        .unwrap_or_else(|| "Unknown Artist".into());
    let album_title = meta
        .album
        .clone()
        .or_else(|| folder_name(&file.path, 0))
        .unwrap_or_else(|| "Unknown Album".into());
    let track_artist = meta.artist.clone().unwrap_or_else(|| album_artist.clone());
    let title = meta.title.clone().unwrap_or_else(|| {
        file.path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_owned()
    });
    let artist_sort = sort_name(&album_artist);

    let artist_id = sqlx::query_scalar!(
        r#"INSERT INTO artists (name, sort_name) VALUES (?, ?)
           ON CONFLICT(name) DO UPDATE SET sort_name = excluded.sort_name
           RETURNING id AS "id!""#,
        album_artist,
        artist_sort
    )
    .fetch_one(&mut *tx)
    .await?;

    let year = meta.year.map(i64::from);
    let album_id = sqlx::query_scalar!(
        r#"INSERT INTO albums (artist_id, title, year) VALUES (?, ?, ?)
           ON CONFLICT(artist_id, title) DO UPDATE SET year = COALESCE(excluded.year, albums.year)
           RETURNING id AS "id!""#,
        artist_id,
        album_title,
        year
    )
    .fetch_one(&mut *tx)
    .await?;

    let disc_no = meta.disc_no.map(i64::from);
    let track_no = meta.track_no.map(i64::from);
    let duration_ms = meta.duration_ms as i64;
    let sample_rate = meta.sample_rate.map(i64::from);
    let bit_depth = meta.bit_depth.map(i64::from);
    let channels = meta.channels.map(i64::from);
    let bitrate = meta.bitrate_kbps.map(i64::from);
    sqlx::query!(
        r#"INSERT INTO tracks (path, size, mtime, album_id, artist_name, title, disc_no, track_no,
                               year, genre, duration_ms, codec, mime, sample_rate, bit_depth,
                               channels, bitrate_kbps, lossless, has_embedded_cover)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(path) DO UPDATE SET
             size = excluded.size, mtime = excluded.mtime, album_id = excluded.album_id,
             artist_name = excluded.artist_name, title = excluded.title,
             disc_no = excluded.disc_no, track_no = excluded.track_no, year = excluded.year,
             genre = excluded.genre, duration_ms = excluded.duration_ms, codec = excluded.codec,
             mime = excluded.mime, sample_rate = excluded.sample_rate,
             bit_depth = excluded.bit_depth, channels = excluded.channels,
             bitrate_kbps = excluded.bitrate_kbps, lossless = excluded.lossless,
             has_embedded_cover = excluded.has_embedded_cover"#,
        path,
        file.size,
        file.mtime,
        album_id,
        track_artist,
        title,
        disc_no,
        track_no,
        year,
        meta.genre,
        duration_ms,
        meta.codec,
        meta.mime,
        sample_rate,
        bit_depth,
        channels,
        bitrate,
        meta.lossless,
        meta.has_embedded_cover,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Orphan cleanup, cover resolution and search index rebuild.
async fn finalize(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!("DELETE FROM albums WHERE id NOT IN (SELECT DISTINCT album_id FROM tracks)")
        .execute(&mut *tx)
        .await?;
    sqlx::query!("DELETE FROM artists WHERE id NOT IN (SELECT DISTINCT artist_id FROM albums)")
        .execute(&mut *tx)
        .await?;
    // Embedded art wins; folder images are the fallback.
    sqlx::query!(
        "UPDATE albums SET cover_track_id = (
            SELECT MIN(id) FROM tracks WHERE album_id = albums.id AND has_embedded_cover = 1)"
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let needs_folder_cover = sqlx::query!(
        r#"SELECT a.id AS "id!", MIN(t.path) AS "path!: String"
           FROM albums a JOIN tracks t ON t.album_id = a.id
           WHERE a.cover_track_id IS NULL
           GROUP BY a.id"#
    )
    .fetch_all(pool)
    .await?;
    let covers: Vec<(i64, Option<String>)> = tokio::task::spawn_blocking(move || {
        needs_folder_cover
            .into_iter()
            .map(|r| {
                let cover = Path::new(&r.path)
                    .parent()
                    .and_then(metadata::find_folder_cover)
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    .and_then(|p| p.to_str().map(str::to_owned));
                (r.id, cover)
            })
            .collect()
    })
    .await?;

    let mut tx = pool.begin().await?;
    sqlx::query!("UPDATE albums SET cover_path = NULL WHERE cover_track_id IS NOT NULL")
        .execute(&mut *tx)
        .await?;
    for (id, cover) in covers {
        sqlx::query!("UPDATE albums SET cover_path = ? WHERE id = ?", cover, id)
            .execute(&mut *tx)
            .await?;
    }

    relink_user_tracks(&mut tx).await?;

    sqlx::query!("DELETE FROM search_fts")
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "INSERT INTO search_fts (kind, ref_id, text) SELECT 'artist', id, name FROM artists"
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO search_fts (kind, ref_id, text)
         SELECT 'album', al.id, al.title || ' ' || ar.name
         FROM albums al JOIN artists ar ON ar.id = al.artist_id"
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO search_fts (kind, ref_id, text)
         SELECT 'track', t.id, t.title || ' ' || t.artist_name || ' ' || al.title
         FROM tracks t JOIN albums al ON al.id = t.album_id"
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Keeps playlist entries attached to the library: refreshes their
/// title/artist/album snapshot from retagged tracks, then reattaches entries
/// whose file disappeared (`track_id` NULL) to a track with the same title,
/// artist and album, e.g. after files were moved or re-encoded.
async fn relink_user_tracks(conn: &mut sqlx::SqliteConnection) -> anyhow::Result<()> {
    sqlx::query!(
        "UPDATE playlist_tracks
         SET title = t.title, artist_name = t.artist_name, album_title = al.title,
             duration_ms = t.duration_ms
         FROM tracks t JOIN albums al ON al.id = t.album_id
         WHERE playlist_tracks.track_id = t.id
           AND (playlist_tracks.title IS NOT t.title
                OR playlist_tracks.artist_name IS NOT t.artist_name
                OR playlist_tracks.album_title IS NOT al.title
                OR playlist_tracks.duration_ms IS NOT t.duration_ms)"
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE playlist_tracks SET track_id = (
             SELECT MIN(t.id) FROM tracks t JOIN albums al ON al.id = t.album_id
             WHERE t.title = playlist_tracks.title COLLATE NOCASE
               AND t.artist_name = playlist_tracks.artist_name COLLATE NOCASE
               AND al.title = playlist_tracks.album_title COLLATE NOCASE)
         WHERE track_id IS NULL"
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

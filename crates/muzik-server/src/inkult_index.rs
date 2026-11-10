//! Separate libraries per Inkult account: Muzik's copy of Inkult's index.
//!
//! Inkult stores each download once in a shared music folder; its
//! `library.json` lists every file (path relative to that folder) with the
//! Inkult account ids that *own* it. Muzik reads that file, read-only, from
//! `MUZIK_INKULT_LIBRARY` and mirrors it into `inkult_files` /
//! `inkult_owners`, keyed by absolute canonical path like `tracks.path`.
//!
//! The file is polled (a `stat` every [`POLL_INTERVAL`], re-read only when
//! its size or mtime changed) and also checked after each library scan, so
//! ownership follows Inkult without a rescan: when someone downloads a song
//! another user already had, Inkult only adds an owner, the file stays put.
//!
//! A read that fails (missing file, corrupt JSON, unknown format version)
//! keeps the previous copy: access never widens because Inkult was briefly
//! unreachable or mid-write.
//!
//! The visibility rule itself lives in SQL (see `CurrentUser::library_owner`
//! and the `inkult_files` / `inkult_owners` predicates in `api/library.rs`).

use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

use muzik_common::InkultIndexStatus;
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::{config::Config, db::now};

/// How often the index file is checked for changes.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Inkult's index is ~1 KB per file; this allows hundreds of thousands.
const MAX_INDEX_BYTES: u64 = 512 * 1024 * 1024;
/// The `library.json` format version Muzik understands.
const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("MUZIK_INKULT_LIBRARY is not set")]
    NotConfigured,
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Format(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
struct Source {
    /// Inkult's `library.json`.
    file: PathBuf,
    /// Inkult's music folder as Muzik sees it.
    music_dir: PathBuf,
}

/// Size and mtime of the index file at its last successful read.
type Fingerprint = (u64, Option<SystemTime>);

#[derive(Debug, Default)]
struct SyncState {
    fingerprint: Option<Fingerprint>,
    synced_at: Option<i64>,
    error: Option<String>,
}

/// Owners per file, by absolute path.
type Ownership = HashMap<String, HashSet<i64>>;

pub struct InkultIndex {
    pool: SqlitePool,
    source: Option<Source>,
    /// Also serializes syncs.
    state: Mutex<SyncState>,
}

impl InkultIndex {
    pub fn new(pool: SqlitePool, config: &Config) -> Self {
        let source = config.inkult_library.clone().map(|file| Source {
            file,
            music_dir: config
                .inkult_music_dir
                .clone()
                .or_else(|| config.library_paths.first().cloned())
                .unwrap_or_default(),
        });
        Self {
            pool,
            source,
            state: Mutex::new(SyncState::default()),
        }
    }

    pub fn configured(&self) -> bool {
        self.source.is_some()
    }

    /// Re-reads the index if it changed since the last successful read (or
    /// unconditionally with `force`) and mirrors it into the database.
    pub async fn sync(&self, force: bool) -> Result<(), IndexError> {
        let source = self.source.clone().ok_or(IndexError::NotConfigured)?;
        let mut st = self.state.lock().await;
        let known = if force { None } else { st.fingerprint };
        let read = tokio::task::spawn_blocking(move || read_index(&source, known))
            .await
            .map_err(|e| IndexError::Format(format!("index reader crashed: {e}")));
        let outcome = match read {
            Ok(Ok(None)) => return Ok(()),
            Ok(Ok(Some((fingerprint, owners)))) => apply(&self.pool, owners)
                .await
                .map(|stats| (fingerprint, stats))
                .map_err(IndexError::from),
            Ok(Err(e)) | Err(e) => Err(e),
        };
        match outcome {
            Ok((fingerprint, (files, changes))) => {
                st.fingerprint = Some(fingerprint);
                st.synced_at = Some(now());
                if st.error.take().is_some() || changes > 0 {
                    tracing::info!(files, changes, "Inkult library index synced");
                }
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                // Polled every 30 s: only log when the problem changes.
                if st.error.as_deref() != Some(msg.as_str()) {
                    tracing::warn!(error = %msg, "cannot read Inkult's library index; keeping the previous copy");
                }
                st.error = Some(msg);
                Err(e)
            }
        }
    }

    /// `None` when no index is configured.
    pub async fn status(&self) -> Result<Option<InkultIndexStatus>, sqlx::Error> {
        let Some(source) = &self.source else {
            return Ok(None);
        };
        let files = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "n!: i64" FROM inkult_files"#)
            .fetch_one(&self.pool)
            .await?;
        let matched = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM inkult_files f JOIN tracks t ON t.path = f.path"#
        )
        .fetch_one(&self.pool)
        .await?;
        let st = self.state.lock().await;
        Ok(Some(InkultIndexStatus {
            path: source.file.display().to_string(),
            files,
            matched,
            synced_at: st.synced_at,
            error: st.error.clone(),
        }))
    }

    /// Checks the index for changes forever (no-op when not configured).
    pub async fn poll(&self) {
        if !self.configured() {
            return;
        }
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            // Failures are logged and kept in the status by `sync`.
            let _ = self.sync(false).await;
        }
    }
}

#[derive(Deserialize)]
struct IndexFile {
    version: u32,
    #[serde(default)]
    entries: Vec<IndexEntry>,
}

/// The fields Muzik needs from an Inkult `LibraryEntry` (others are ignored).
#[derive(Deserialize)]
struct IndexEntry {
    path: PathBuf,
    #[serde(default)]
    owners: Vec<u64>,
}

/// Reads and parses the index. `Ok(None)` when its fingerprint is `known`.
fn read_index(
    source: &Source,
    known: Option<Fingerprint>,
) -> Result<Option<(Fingerprint, Ownership)>, IndexError> {
    let path_str = source.file.display().to_string();
    let io_err = |e: std::io::Error| IndexError::Io {
        path: path_str.clone(),
        source: e,
    };
    let md = std::fs::metadata(&source.file).map_err(io_err)?;
    let fingerprint = (md.len(), md.modified().ok());
    if known == Some(fingerprint) {
        return Ok(None);
    }
    if md.len() > MAX_INDEX_BYTES {
        return Err(IndexError::Format(format!(
            "{path_str} is larger than {} MB",
            MAX_INDEX_BYTES >> 20
        )));
    }
    let bytes = std::fs::read(&source.file).map_err(io_err)?;
    let music_dir = std::fs::canonicalize(&source.music_dir).map_err(|e| IndexError::Io {
        path: source.music_dir.display().to_string(),
        source: e,
    })?;
    let owners =
        parse(&bytes, &music_dir).map_err(|e| IndexError::Format(format!("{path_str}: {e}")))?;
    Ok(Some((fingerprint, owners)))
}

/// Parses `library.json` into owners per absolute file path under `music_dir`.
fn parse(bytes: &[u8], music_dir: &Path) -> Result<Ownership, String> {
    let index: IndexFile =
        serde_json::from_slice(bytes).map_err(|e| format!("not an Inkult library index ({e})"))?;
    if index.version != SUPPORTED_VERSION {
        return Err(format!(
            "unsupported Inkult library index version {} (Muzik reads version {SUPPORTED_VERSION})",
            index.version
        ));
    }
    let mut out = Ownership::with_capacity(index.entries.len());
    for entry in index.entries {
        // Inkult only writes relative paths inside its folder; ignore
        // anything else rather than guess.
        let relative = !entry.path.as_os_str().is_empty()
            && entry
                .path
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if !relative {
            continue;
        }
        let joined = music_dir.join(&entry.path);
        // Same form as `tracks.path` (the scanner stores canonical paths).
        let path = std::fs::canonicalize(&joined).unwrap_or(joined);
        let Some(path) = path.to_str() else {
            continue;
        };
        out.entry(path.to_owned()).or_default().extend(
            entry
                .owners
                .into_iter()
                .filter_map(|id| i64::try_from(id).ok()),
        );
    }
    Ok(out)
}

/// Mirrors `owners` into the database, writing only the differences.
/// Returns (files in the index, rows changed).
async fn apply(pool: &SqlitePool, owners: Ownership) -> Result<(usize, usize), sqlx::Error> {
    // Syncs are serialized and nothing else writes these tables, so reading
    // outside the write transaction is safe.
    let current_files: HashSet<String> = sqlx::query_scalar!("SELECT path FROM inkult_files")
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    let current_owners: HashSet<(String, i64)> =
        sqlx::query!("SELECT path, owner_id FROM inkult_owners")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| (r.path, r.owner_id))
            .collect();
    let wanted_owners: HashSet<(String, i64)> = owners
        .iter()
        .flat_map(|(path, ids)| ids.iter().map(move |&id| (path.clone(), id)))
        .collect();

    let mut changes = 0;
    let mut tx = pool.begin().await?;
    // Grant before revoking files: a file never passes through "unindexed"
    // (shared with everyone) while moving between owners.
    for path in owners.keys().filter(|p| !current_files.contains(*p)) {
        sqlx::query!("INSERT INTO inkult_files (path) VALUES (?)", path)
            .execute(&mut *tx)
            .await?;
        changes += 1;
    }
    for (path, id) in wanted_owners.difference(&current_owners) {
        sqlx::query!(
            "INSERT INTO inkult_owners (path, owner_id) VALUES (?, ?)",
            path,
            id
        )
        .execute(&mut *tx)
        .await?;
        changes += 1;
    }
    for (path, id) in current_owners.difference(&wanted_owners) {
        sqlx::query!(
            "DELETE FROM inkult_owners WHERE path = ? AND owner_id = ?",
            path,
            id
        )
        .execute(&mut *tx)
        .await?;
        changes += 1;
    }
    for path in current_files.iter().filter(|p| !owners.contains_key(*p)) {
        sqlx::query!("DELETE FROM inkult_files WHERE path = ?", path)
            .execute(&mut *tx)
            .await?;
        changes += 1;
    }
    tx.commit().await?;
    Ok((owners.len(), changes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owners_and_ignores_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("A/B")).unwrap();
        std::fs::write(root.join("A/B/1.flac"), b"x").unwrap();
        let json = br#"{"version": 1, "entries": [
            {"path": "A/B/1.flac", "title": "One", "owners": [42, 7]},
            {"path": "A/B/2.flac", "owners": []},
            {"path": "../outside.flac", "owners": [42]},
            {"path": "/etc/passwd", "owners": [42]},
            {"path": "A/B/1.flac", "owners": [18446744073709551615, 9]}
        ]}"#;
        let owners = parse(json, &root).unwrap();
        assert_eq!(owners.len(), 2);
        let one = root.join("A/B/1.flac").to_str().unwrap().to_owned();
        assert_eq!(owners[&one], HashSet::from([42, 7, 9]));
        let two = root.join("A/B/2.flac").to_str().unwrap().to_owned();
        assert!(owners[&two].is_empty());
    }

    #[test]
    fn rejects_unknown_versions_and_garbage() {
        let root = Path::new("/music");
        assert!(parse(br#"{"version": 2, "entries": []}"#, root)
            .unwrap_err()
            .contains("version 2"));
        assert!(parse(b"[]", root).is_err());
        assert!(parse(br#"{"version": 1}"#, root).unwrap().is_empty());
    }
}

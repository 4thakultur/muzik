//! On-demand transcoding with ffmpeg, cached on disk.
//!
//! Output is written to a temp file and atomically renamed, so concurrent or
//! interrupted requests never observe partial files. Concurrent requests for
//! the same output wait on a per-key lock instead of spawning ffmpeg twice.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use muzik_common::Quality;
use tokio::{
    process::Command,
    sync::{Mutex, Semaphore},
};

use crate::error::{internal, ApiResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Flac,
    Opus { kbps: u32 },
}

impl Target {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Opus { .. } => "opus",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Flac => "audio/flac",
            Self::Opus { .. } => "audio/ogg; codecs=opus",
        }
    }
}

/// Decides what to serve for a requested quality. `None` = the original file.
pub fn plan(
    quality: Quality,
    lossless: bool,
    codec: &str,
    bitrate_kbps: Option<i64>,
) -> Option<Target> {
    let opus = |kbps: u32| {
        // Never "upgrade" an already-small lossy file.
        let already_small = !lossless && bitrate_kbps.is_some_and(|b| b <= i64::from(kbps) + 32);
        (!already_small).then_some(Target::Opus { kbps })
    };
    match quality {
        Quality::Original => None,
        // FLAC sources are already optimal; other lossless (WAV/AIFF/ALAC) → FLAC.
        Quality::High => (lossless && codec != "flac").then_some(Target::Flac),
        Quality::Normal => opus(160),
        Quality::Low => opus(96),
    }
}

#[derive(Clone)]
pub struct Transcoder {
    ffmpeg: PathBuf,
    dir: PathBuf,
    max_size: u64,
    inflight: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    /// Caps concurrent ffmpeg processes so clients can't exhaust CPU/RAM.
    slots: Arc<Semaphore>,
}

impl Transcoder {
    pub fn new(
        ffmpeg: PathBuf,
        cache_dir: &Path,
        max_size: u64,
        max_concurrent: usize,
    ) -> std::io::Result<Self> {
        let dir = cache_dir.join("transcodes");
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            ffmpeg,
            dir,
            max_size,
            inflight: Default::default(),
            slots: Arc::new(Semaphore::new(max_concurrent.max(1))),
        })
    }

    /// Returns the path of the cached transcode, producing it if needed.
    pub async fn get(
        &self,
        track_id: i64,
        source: &Path,
        source_mtime: i64,
        target: Target,
    ) -> ApiResult<PathBuf> {
        let tag = match target {
            Target::Flac => "flac".to_owned(),
            Target::Opus { kbps } => format!("opus{kbps}"),
        };
        let out = self.dir.join(format!(
            "{track_id}-{source_mtime:x}-{tag}.{}",
            target.ext()
        ));
        if tokio::fs::try_exists(&out).await.unwrap_or(false) {
            return Ok(out);
        }

        let key_lock = {
            let mut map = self.inflight.lock().await;
            map.entry(out.clone()).or_default().clone()
        };
        let _guard = key_lock.lock().await;
        let result = if tokio::fs::try_exists(&out).await.unwrap_or(false) {
            Ok(())
        } else {
            self.produce(source, &out, target).await
        };
        // Always drop the per-key entry, also on failure, so the map can't grow.
        self.inflight.lock().await.remove(&out);
        result.map(|()| out)
    }

    async fn produce(&self, source: &Path, out: &Path, target: Target) -> ApiResult<()> {
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|e| internal(format!("transcoder closed: {e}")))?;
        self.run_ffmpeg(source, out, target).await?;
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.prune().await {
                tracing::warn!(error = %e, "transcode cache prune failed");
            }
        });
        Ok(())
    }

    async fn run_ffmpeg(&self, source: &Path, out: &Path, target: Target) -> ApiResult<()> {
        let tmp = out.with_extension(format!("{}.part", target.ext()));
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(source)
            .args(["-map", "0:a:0", "-map_metadata", "-1", "-vn"]);
        match target {
            Target::Flac => {
                cmd.args(["-c:a", "flac", "-compression_level", "5", "-f", "flac"]);
            }
            Target::Opus { kbps } => {
                cmd.args([
                    "-c:a",
                    "libopus",
                    "-b:a",
                    &format!("{kbps}k"),
                    "-vbr",
                    "on",
                    "-f",
                    "ogg",
                ]);
            }
        }
        cmd.arg(&tmp).kill_on_drop(true);

        let started = std::time::Instant::now();
        let output = cmd
            .output()
            .await
            .map_err(|e| internal(format!("ffmpeg: {e}")))?;
        if !output.status.success() {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(internal(format!(
                "ffmpeg failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        tokio::fs::rename(&tmp, out).await?;
        tracing::debug!(out = %out.display(), elapsed_ms = started.elapsed().as_millis() as u64, "transcoded");
        Ok(())
    }

    /// Deletes least-recently-modified transcodes until under `max_size`.
    async fn prune(&self) -> std::io::Result<()> {
        let dir = self.dir.clone();
        let max = self.max_size;
        tokio::task::spawn_blocking(move || {
            let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(&dir)?
                .flatten()
                .filter_map(|e| {
                    let md = e.metadata().ok()?;
                    if !md.is_file() {
                        return None;
                    }
                    let used = md.accessed().or(md.modified()).ok()?;
                    Some((used, md.len(), e.path()))
                })
                .collect();
            let mut total: u64 = files.iter().map(|f| f.1).sum();
            if total <= max {
                return Ok(());
            }
            files.sort_by_key(|f| f.0);
            for (_, len, path) in files {
                if total <= max {
                    break;
                }
                if path.extension().is_some_and(|e| e == "part") {
                    continue;
                }
                std::fs::remove_file(&path)?;
                total = total.saturating_sub(len);
            }
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_plan() {
        assert_eq!(plan(Quality::Original, true, "pcm", None), None);
        assert_eq!(plan(Quality::High, true, "pcm", None), Some(Target::Flac));
        assert_eq!(plan(Quality::High, true, "flac", None), None);
        assert_eq!(plan(Quality::High, false, "mp3", Some(320)), None);
        assert_eq!(
            plan(Quality::Normal, true, "flac", None),
            Some(Target::Opus { kbps: 160 })
        );
        assert_eq!(
            plan(Quality::Low, false, "mp3", Some(320)),
            Some(Target::Opus { kbps: 96 })
        );
        assert_eq!(plan(Quality::Low, false, "mp3", Some(128)), None);
    }
}

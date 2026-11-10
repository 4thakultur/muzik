//! Album cover thumbnails: decoded, resized and cached as WebP.

use std::path::{Path, PathBuf};

use image::{imageops::FilterType, DynamicImage, ImageFormat, ImageReader, Limits};

use crate::{
    error::{internal, ApiResult, AppError},
    library::{metadata, paths::LibraryRoots},
};

pub const SIZES: &[u32] = &[96, 300, 600];

pub enum CoverSource {
    File(PathBuf),
    Embedded(PathBuf),
}

/// Returns a cached WebP thumbnail of `size` px, generating it if needed.
/// `signature` must change when the source changes (it is part of the file name).
pub async fn thumbnail(
    cache_dir: &Path,
    roots: LibraryRoots,
    album_id: i64,
    size: u32,
    signature: String,
    source: CoverSource,
) -> ApiResult<PathBuf> {
    if !SIZES.contains(&size) {
        return Err(AppError::BadRequest(format!(
            "size must be one of {SIZES:?}"
        )));
    }
    let dir = cache_dir.join("covers");
    let out = dir.join(format!("{album_id}-{size}-{signature}.webp"));
    if tokio::fs::try_exists(&out).await.unwrap_or(false) {
        return Ok(out);
    }
    tokio::task::spawn_blocking(move || -> ApiResult<PathBuf> {
        let bytes = match source {
            CoverSource::File(p) => {
                let p = roots.resolve(&p).map_err(|_| AppError::NotFound)?;
                std::fs::read(p)?
            }
            CoverSource::Embedded(p) => {
                let p = roots.resolve(&p).map_err(|_| AppError::NotFound)?;
                metadata::embedded_cover(&p).ok_or(AppError::NotFound)?
            }
        };
        let img = decode_limited(&bytes)?;
        let img = if img.width() > size || img.height() > size {
            img.resize(size, size, FilterType::Lanczos3)
        } else {
            img
        };
        std::fs::create_dir_all(&dir)?;
        let tmp = out.with_extension("webp.part");
        img.to_rgb8()
            .save_with_format(&tmp, ImageFormat::WebP)
            .map_err(internal)?;
        std::fs::rename(&tmp, &out)?;
        Ok(out)
    })
    .await
    .map_err(internal)?
}

/// Decodes an image with hard limits, so a malicious or broken cover (e.g. a
/// tiny file claiming 60000×60000 px) cannot exhaust memory.
pub fn decode_limited(bytes: &[u8]) -> ApiResult<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(10_000);
    limits.max_image_height = Some(10_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(internal)?;
    reader.limits(limits);
    reader.decode().map_err(internal)
}

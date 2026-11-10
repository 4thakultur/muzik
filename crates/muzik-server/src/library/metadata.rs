//! Reading tags and stream properties from audio files (blocking).

use std::path::Path;

use lofty::{
    config::ParseOptions,
    file::{AudioFile, TaggedFileExt},
    picture::PictureType,
    probe::Probe,
    tag::{Accessor, ItemKey},
};

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "flac", "wav", "m4a", "mp4", "alac", "aif", "aiff", "aifc", "mp3", "aac", "ogg", "oga", "opus",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Default)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub duration_ms: u64,
    pub codec: String,
    pub mime: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub lossless: bool,
    pub has_embedded_cover: bool,
}

fn non_empty(s: Option<impl Into<String>>) -> Option<String> {
    s.map(Into::into)
        .map(|s: String| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Codec name, MIME type and losslessness, from the container type and extension.
fn codec_info(path: &Path, file_type: Option<lofty::file::FileType>) -> (String, String, bool) {
    use lofty::file::FileType as F;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match file_type {
        Some(F::Flac) => ("flac".into(), "audio/flac".into(), true),
        Some(F::Wav) => ("pcm".into(), "audio/wav".into(), true),
        Some(F::Aiff) => ("pcm".into(), "audio/aiff".into(), true),
        Some(F::Mpeg) => ("mp3".into(), "audio/mpeg".into(), false),
        Some(F::Aac) => ("aac".into(), "audio/aac".into(), false),
        Some(F::Opus) => ("opus".into(), "audio/ogg; codecs=opus".into(), false),
        Some(F::Vorbis) => ("vorbis".into(), "audio/ogg".into(), false),
        // M4A may hold AAC or ALAC; refined below from the codec properties.
        Some(F::Mp4) => ("aac".into(), "audio/mp4".into(), false),
        _ => {
            let mime = mime_guess::from_path(path)
                .first_raw()
                .unwrap_or("application/octet-stream");
            (ext, mime.to_owned(), false)
        }
    }
}

pub fn read(path: &Path) -> Result<TrackMeta, lofty::error::LoftyError> {
    let tagged = Probe::open(path)?
        .options(ParseOptions::new().read_cover_art(true))
        .read()?;
    let (mut codec, mime, mut lossless) = codec_info(path, Some(tagged.file_type()));

    let props = tagged.properties();
    if let lofty::file::FileType::Mp4 = tagged.file_type() {
        if props.bit_depth().is_some() && props.audio_bitrate().unwrap_or(0) > 600 {
            // lofty only reports bit depth for ALAC in MP4 containers.
            codec = "alac".into();
            lossless = true;
        }
    }

    let mut meta = TrackMeta {
        duration_ms: props.duration().as_millis() as u64,
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        bitrate_kbps: props.audio_bitrate().or(props.overall_bitrate()),
        codec,
        mime,
        lossless,
        ..Default::default()
    };

    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        meta.title = non_empty(tag.title());
        meta.artist = non_empty(tag.artist());
        meta.album = non_empty(tag.album());
        meta.album_artist = non_empty(tag.get_string(&ItemKey::AlbumArtist));
        meta.track_no = tag.track();
        meta.disc_no = tag.disk();
        meta.year = tag.year();
        meta.genre = non_empty(tag.genre());
    }
    meta.has_embedded_cover = tagged.tags().iter().any(|t| !t.pictures().is_empty());

    if meta.duration_ms == 0 {
        meta.duration_ms = probe_duration_ms(path).unwrap_or(0);
    }
    Ok(meta)
}

/// Returns the embedded front cover (or first picture) bytes.
pub fn embedded_cover(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let pictures: Vec<_> = tagged.tags().iter().flat_map(|t| t.pictures()).collect();
    pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
        .map(|p| p.data().to_vec())
}

/// Fallback duration probe with symphonia when tags/headers lack it.
fn probe_duration_ms(path: &Path) -> Option<u64> {
    use symphonia::core::{
        formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
    };
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed.format.default_track()?;
    let params = &track.codec_params;
    let frames = params.n_frames?;
    let rate = params.sample_rate? as u64;
    Some(frames * 1000 / rate)
}

/// Looks for a cover image file in the album folder.
pub fn find_folder_cover(dir: &Path) -> Option<std::path::PathBuf> {
    const NAMES: &[&str] = &["cover", "folder", "front", "album"];
    const EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(usize, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        if !EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
            continue;
        }
        let stem = stem.to_ascii_lowercase();
        if let Some(rank) = NAMES.iter().position(|n| *n == stem) {
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

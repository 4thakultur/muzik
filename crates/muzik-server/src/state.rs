use std::sync::{Arc, RwLock};

use sqlx::SqlitePool;

use crate::{
    auth::rate_limit::AuthLimiter,
    config::Config,
    inkult::InkultClient,
    inkult_index::InkultIndex,
    library::{paths::LibraryRoots, scanner::Scanner},
    transcode::Transcoder,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    pub roots: Arc<RwLock<LibraryRoots>>,
    pub scanner: Scanner,
    pub transcoder: Transcoder,
    pub auth_limiter: Arc<AuthLimiter>,
    pub inkult: Arc<InkultClient>,
    /// Inkult's library index (who owns which file).
    pub inkult_index: Arc<InkultIndex>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config) -> std::io::Result<Self> {
        let roots = Arc::new(RwLock::new(LibraryRoots::new(&config.library_paths)));
        let inkult_index = Arc::new(InkultIndex::new(pool.clone(), &config));
        let scanner = Scanner::new(
            pool.clone(),
            config.library_paths.clone(),
            roots.clone(),
            inkult_index.clone(),
        );
        let transcoder = Transcoder::new(
            config.ffmpeg.clone(),
            &config.cache_dir,
            config.cache_max_size,
            config.max_transcodes,
        )?;
        let inkult = InkultClient::new().map_err(std::io::Error::other)?;
        Ok(Self {
            pool,
            roots,
            scanner,
            transcoder,
            auth_limiter: Arc::new(AuthLimiter::default()),
            inkult: Arc::new(inkult),
            inkult_index,
            config: Arc::new(config),
        })
    }

    /// Snapshot of the current library roots.
    pub fn roots(&self) -> LibraryRoots {
        self.roots
            .read()
            .map(|r| r.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }
}

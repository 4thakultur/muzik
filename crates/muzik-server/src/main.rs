use std::net::SocketAddr;

use anyhow::Context;
use muzik_common::ScanMode;
use muzik_server::{config::Config, db, state::AppState};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `muzik-server healthcheck`: used by the Docker HEALTHCHECK, so the
    // runtime image doesn't need curl.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck().await;
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn"));
    let json = std::env::var("MUZIK_LOG_FORMAT").is_ok_and(|v| v == "json");
    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer()).init();
    }

    let config = Config::from_env()?;
    tracing::info!(?config, "starting muzik");
    std::fs::create_dir_all(&config.data_dir).context("creating data dir")?;
    std::fs::create_dir_all(&config.cache_dir).context("creating cache dir")?;

    let pool = db::connect(&config.data_dir.join("muzik.db"))
        .await
        .context("opening database")?;
    let port = config.port;
    let scan_interval = config.scan_interval;
    let state = AppState::new(pool, config).context("initializing state")?;

    // Incremental scan at startup, then periodically if configured.
    let scanner = state.scanner.clone();
    tokio::spawn(async move {
        scanner.run_blocking(ScanMode::Incremental).await;
        if let Some(every) = scan_interval {
            let mut ticker = tokio::time::interval(every);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if scanner.start(ScanMode::Incremental).is_err() {
                    tracing::debug!("periodic scan skipped: scan already running");
                }
            }
        }
    });

    // Expired sessions and abandoned 2FA challenges are purged hourly.
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            ticker.tick().await;
            if let Err(e) = muzik_server::auth::session::purge_expired(&pool).await {
                tracing::warn!(error = %e, "session cleanup failed");
            }
        }
    });

    let app = muzik_server::app(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down");
}

/// Exits 0 when `GET /api/health` on the local port answers 200.
async fn healthcheck() -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let port: u16 = std::env::var("MUZIK_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9873);
    let check = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        s.write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = [0u8; 16];
        let n = s.read(&mut buf).await?;
        anyhow::ensure!(buf[..n].starts_with(b"HTTP/1.1 200"), "unhealthy");
        anyhow::Ok(())
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), check).await??;
    Ok(())
}

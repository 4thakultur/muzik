# syntax=docker/dockerfile:1

# ---- Build ---------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src
# Queries are checked against the committed .sqlx metadata, no DB needed.
ENV SQLX_OFFLINE=true
COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx
COPY migrations ./migrations
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p muzik-server \
 && cp target/release/muzik-server /muzik-server

# ---- Web frontend (Leptos → WASM via trunk) -------------------------------
FROM rust:1-bookworm AS web
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install --locked trunk --version 0.21.14
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=muzik-web-target \
    cd crates/muzik-web && trunk build --release --dist /web

# ---- Runtime -------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 1000 muzik \
 && useradd --uid 1000 --gid muzik --no-create-home --shell /usr/sbin/nologin muzik \
 && mkdir -p /data /cache /app/web \
 && chown muzik:muzik /data /cache

COPY --from=builder /muzik-server /usr/local/bin/muzik-server
COPY --from=web /web /app/web

ENV MUZIK_LIBRARY_PATHS=/music \
    MUZIK_DATA_DIR=/data \
    MUZIK_CACHE_DIR=/cache \
    MUZIK_WEB_DIR=/app/web \
    MUZIK_PORT=9873 \
    MUZIK_LOG_FORMAT=json

USER muzik
EXPOSE 9873
VOLUME ["/data", "/cache"]
# Built-in probe: no curl (or any other network tool) in the runtime image.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD ["muzik-server", "healthcheck"]
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["muzik-server"]

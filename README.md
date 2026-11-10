# Muzik

**A self-hosted music streaming server with a clean, native-feeling web player.**

Muzik indexes the music library you already have on disk and streams it — bit-perfect lossless included — to every member of your household, from a fast web app that works on desktop and phones and installs as a PWA.

Written in Rust end to end: an `axum` backend and a Leptos (WebAssembly) frontend.

---

## Features

**Library**
- Reads FLAC, WAV, ALAC/M4A, AIFF, MP3, AAC, OGG Vorbis and Opus from one or more folders.
- **Read-only by design**: Muzik never modifies, moves or deletes your files. Add music by dropping files into the folder and running a rescan.
- Fast incremental scans (only changed files are re-read), with live progress in the UI and optional periodic rescans.
- Artist → album → track grouping from tags (album artist, disc and track numbers), with folder-structure fallback for untagged files.
- Cover art from embedded images or `cover.jpg` / `folder.jpg` / `front.*`.
- Instant full-text search across artists, albums, tracks and playlists.

**Streaming**
- `original` quality streams the untouched file with full HTTP Range support (instant seeking).
- Optional transcodes for weaker connections: FLAC (lossless), Opus 160 kbps or 96 kbps, cached on disk.
- Per-device quality settings for Wi-Fi and mobile data.
- Next-track preloading for near-gapless playback.

**Player & UI**
- Desktop layout with sidebar and player bar; on mobile, tab bar, mini-player and a full-screen Now Playing sheet.
- Light, true-black dark, and automatic themes, with a user-selectable accent color.
- Touch gestures: swipe tracks, swipe the player, edge-swipe back.
- Lock-screen / headset / Bluetooth controls (Media Session API) and keyboard shortcuts.
- Queue with shuffle and repeat, Up Next and local suggestions.

**Users**
- Multiple users with an admin; no open sign-ups.
- Private, shared and collaborative playlists; others can save shared ones to their library.
- Profile pictures, play history, resumable queue.
- Two-factor authentication (TOTP) with recovery codes, optionally mandatory.
- Optional sign-in through an [Inkult](#inkult-sign-in-optional) server.

**Privacy**: no telemetry, no external calls at runtime unless you explicitly enable an integration.

---

## Quick start (Docker)

```bash
git clone <repo-url> muzik
cd muzik
```

Edit `docker-compose.yml` and point the library volume at your music:

```yaml
volumes:
  - /path/to/your/music:/music:ro   # always read-only
  - ./music/data:/data              # database, avatars
  - ./music/cache:/cache            # transcodes, thumbnails
```

The container runs as uid/gid `1000`, so `data` and `cache` must be writable by that user:

```bash
mkdir -p music/data music/cache
sudo chown -R 1000:1000 music/data music/cache
docker compose up -d --build
```

Open <http://localhost:9873>. The first account you create is the **admin**; the initial library scan starts automatically, and you can rescan anytime from **Settings**.

> **HTTPS:** session cookies are `Secure` by default. Put Muzik behind a reverse proxy with TLS (Caddy, Traefik, nginx…). If you only access it over plain HTTP on your LAN, set `MUZIK_SECURE_COOKIES: "false"`.

### Reverse proxy

Muzik listens on port `9873`. When it runs behind a proxy, list the proxy's address in `MUZIK_TRUSTED_PROXIES` so login rate limiting and auth logs see the real client IP:

```yaml
environment:
  MUZIK_TRUSTED_PROXIES: "172.16.0.0/12"
```

---

## Configuration

All settings are environment variables.

| Variable | Default | Description |
|---|---|---|
| `MUZIK_LIBRARY_PATHS` | `/music` | Library folders, comma-separated. Mount them read-only. |
| `MUZIK_DATA_DIR` | `/data` | SQLite database and avatars. |
| `MUZIK_CACHE_DIR` | `/cache` | Transcodes and cover thumbnails. |
| `MUZIK_PORT` | `9873` | HTTP port. |
| `MUZIK_SCAN_INTERVAL` | *(off)* | Periodic rescan, e.g. `3600`, `30m`, `6h`. |
| `MUZIK_CACHE_MAX_SIZE` | `10G` | Cache size limit (least recently used entries are pruned). |
| `MUZIK_MAX_TRANSCODES` | CPUs (max 8) | Concurrent ffmpeg processes. |
| `MUZIK_SECURE_COOKIES` | `true` | `false` only for plain-HTTP access. Also controls HSTS. |
| `MUZIK_TRUSTED_PROXIES` | *(empty)* | IPs/CIDRs whose `X-Forwarded-For` header is trusted. |
| `MUZIK_FFMPEG` | `ffmpeg` | Path to the ffmpeg binary. |
| `MUZIK_WEB_DIR` | `/app/web` | Built web UI. |
| `MUZIK_LOG_FORMAT` | `json` in Docker | `json` for structured logs, otherwise human-readable. |
| `RUST_LOG` | `info` | Log filter, e.g. `muzik_server=debug`. |
| `MUZIK_INKULT_LIBRARY` | *(none)* | Inkult's `library.json`, for [separate libraries](#separate-libraries-per-inkult-user). |
| `MUZIK_INKULT_MUSIC_DIR` | first library path | Where Inkult's music folder is mounted in Muzik (the paths in `library.json` are relative to it). |

### Inkult sign-in (optional)

Muzik can let users sign in with their accounts from an Inkult server. It is off by default and configured by an admin under **Settings → Inkult**. When it is enabled, every Inkult account can sign in to Muzik, and Inkult's own two-factor authentication applies. Muzik works fully without it.

#### Separate libraries per Inkult user

Inkult stores each song once in a shared music folder and records in its `library.json` which users downloaded it. Muzik can read that file so that each Inkult user only hears their own songs, as in Inkult. Turn it on under **Settings → Inkult → Separate Libraries**. With it on:

- An Inkult user sees and streams only the songs they own in Inkult. Songs that are not in Inkult's index at all, because they were copied into the folder by hand, are shared with everyone.
- Inkult captains, Muzik admins and accounts with a Muzik password see the whole library.
- Songs outside your library show as *Unavailable* in other people's shared playlists, and you can't add them to a playlist.
- Ownership follows Inkult within about 30 seconds, with no rescan. New files still need a scan, as usual.

Mount Inkult's **data folder** read-only and point Muzik at the index. Mount the folder, not just the file: Inkult replaces `library.json` atomically, and a single-file bind mount would keep showing the old copy.

```yaml
    volumes:
      - /srv/inkult/music:/music:ro          # Inkult's MUSIC_DIR as the library
      - /srv/inkult/data:/inkult:ro          # Inkult's DATA_DIR
    environment:
      MUZIK_INKULT_LIBRARY: /inkult/library.json
```

Inkult's data folder also holds its account and Soulseek credentials. Inkult writes those files as owner-only (`0600`), so run Muzik under a different uid than Inkult's `PUID` to keep them unreadable to Muzik. `library.json` stays world-readable. Muzik refuses to turn the feature on if it can't read the index, or if none of the files listed in it are in the library (a wrong `MUZIK_INKULT_MUSIC_DIR`). If the index becomes unreadable later, Muzik keeps its last good copy rather than showing everything.

---

## Development

### Requirements

- Rust (stable) with the WebAssembly target: `rustup target add wasm32-unknown-unknown`
- [trunk](https://trunkrs.dev) 0.21: `cargo install --locked trunk`
- sqlx-cli: `cargo install sqlx-cli --no-default-features --features sqlite,rustls`
- `ffmpeg` (only needed for transcoded qualities)

### Run locally

```bash
# Dev database used by sqlx's compile-time query checks (DATABASE_URL is in .env)
sqlx database setup

# Build the web UI once (or use `trunk serve` below for live reload)
(cd crates/muzik-web && trunk build --release)

# Start the server
MUZIK_SECURE_COOKIES=false cargo run
```

Without `MUZIK_*` path variables, the server uses `./music/library`, `./music/data`, `./music/cache` and `crates/muzik-web/dist`. Put some music in `./music/library` and open <http://localhost:9873>.

For frontend work, `cd crates/muzik-web && trunk serve` serves the UI on <http://localhost:8080> with live reload and proxies `/api` to the server on `:9873`.

### Checks

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

### Database changes

Every schema change is a new migration in `migrations/` (never edit an applied one). After adding or changing a query, refresh the offline query metadata used by Docker builds and commit `.sqlx/`:

```bash
sqlx migrate run
cargo sqlx prepare --workspace
```

To build without a database: `SQLX_OFFLINE=true cargo build`.

### Project layout

```
crates/
  muzik-server/   axum backend: API, streaming, scanner, auth
  muzik-web/      Leptos frontend (WASM, built with trunk)
  muzik-common/   types shared by server and client
migrations/       SQLite migrations (sqlx)
docker/           Dockerfile
```

---

## Security

- Passwords hashed with argon2id; login rate limiting per user and IP.
- Server-side sessions in `HttpOnly`, `Secure`, `SameSite=Lax` cookies, plus same-origin checks on every state-changing request.
- Strict Content Security Policy, no inline scripts.
- Clients only ever reference IDs; file paths are resolved server-side and confined to the library roots.
- Container runs as a non-root user on a read-only filesystem, with the library mounted read-only.

Found a vulnerability? Please report it privately through GitHub's security advisories instead of opening a public issue.

---

## License

[AGPL-3.0-or-later](LICENSE).

The bundled Inter font is distributed under the SIL Open Font License (`crates/muzik-web/public/fonts/Inter-LICENSE.txt`).

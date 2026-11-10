# CLAUDE.md — Muzik

## What Muzik is

Muzik is an open-source, self-hostable music streaming server with a Spotify-like experience. It reads an existing music library from a directory, indexes it, and streams it (up to lossless) to multiple users through a web UI that works on desktop and mobile.

Priorities, in order:

1. **Speed**: playback starts almost instantly, browsing feels native, scans are fast.
2. **Quality**: original files (FLAC, WAV, ALAC…) can be streamed bit-perfect. Lossy transcodes exist only as an opt-in for weak connections.
3. **Clean UI**: Apple iOS + Spotify inspired. Glass panels, rounded corners, smooth motion, swipe gestures on mobile.

## Hard rules (never break these)

- **The library is read-only.** Muzik never writes, moves, renames, deletes or uploads files in the music directory. There is no "upload" or "add music" feature anywhere, in the API or the UI. Users add music by putting files in the folder on the host, then trigger a **rescan** (like Jellyfin).
- The library is mounted `:ro` in docker-compose. Code must work with a read-only mount.
- **Never serve a file from a client-supplied path.** Clients only ever send track/album/artist IDs; the server resolves IDs to paths from the database. Canonicalize every path and check it stays inside the library root (path traversal protection).
- Do not use Spotify's name, logo, icons, fonts or any of its assets. The UI is *inspired by* Spotify/iOS, it is not a copy of their branding.
- No telemetry, no external calls at runtime (except optional, explicit metadata/cover lookups and the Inkult connector, all off by default).
- **Inkult is optional.** Muzik must work fully without an Inkult server; the connector only adds a way to sign in.

## Tech stack

- **Language:** Rust (stable, edition 2021), everywhere.
- **Backend:** `axum` + `tokio`, `tower-http` (compression, CORS, tracing, static files).
- **Database:** SQLite via `sqlx` (compile-time checked queries, migrations in `migrations/`). WAL mode on.
- **Metadata:** `lofty` for tags and embedded cover art; `symphonia` for duration/format probing when tags are missing.
- **Transcoding:** `ffmpeg` binary (shipped in the Docker image), invoked as a subprocess, output streamed. Transcodes are cached on disk.
- **Images:** `image` crate for profile pictures and cover thumbnails (resized + cached, served as WebP).
- **Auth:** `argon2` (argon2id) for passwords, server-side sessions in SQLite, `HttpOnly` + `Secure` + `SameSite=Lax` cookies.
- **Frontend:** Leptos (CSR, built with `trunk`) served as static files by the backend. Shared types between front and back live in the `muzik-common` crate.
- **Deployment:** Docker multi-stage build + `docker-compose.yml`.

## Repository layout

```
crates/
  muzik-server/    # axum app: API, streaming, scanner, auth
  muzik-web/       # Leptos frontend (trunk)
  muzik-common/    # shared DTOs and types (serde)
migrations/        # sqlx migrations
docker/            # Dockerfile, entrypoint
docker-compose.yml
```

## Core features

### Library & scanning
- Input: one or more library roots (env `MUZIK_LIBRARY_PATHS`, default `/music`).
- Supported: FLAC, WAV, ALAC/M4A, AIFF, MP3, AAC, OGG Vorbis, Opus.
- Scan is **incremental**: compare path + size + mtime to the DB, only re-read changed files, remove missing ones.
- Scans run in a background task with progress reported to the UI (SSE). Only one scan at a time.
- Admins can trigger a full or incremental rescan from the settings page. Optional periodic scan (`MUZIK_SCAN_INTERVAL`).
- Grouping: artist → album → track, using album artist, disc and track numbers. Fall back to folder structure when tags are missing.
- Cover art: embedded art first, then `cover.jpg` / `folder.jpg` / `front.*` in the album folder.

### Streaming
- `GET /api/stream/{track_id}?quality=original|high|normal|low`
  - `original`: the raw file, served with full **HTTP Range** support (seeking, resuming). No re-encoding.
  - `high`: FLAC for lossless sources (WAV → FLAC saves bandwidth with zero quality loss).
  - `normal` / `low`: Opus 160 / 96 kbps (AAC fallback for Safari if needed).
- Correct `Content-Type`, `Accept-Ranges`, `Content-Length`, `ETag`. Cover/thumbnail responses get long `Cache-Control`.
- The client **preloads the next track** in the queue and supports gapless playback where the format allows.
- Users pick quality per network type (Wi-Fi / mobile data) in their settings.

### Users
- Multi-user. First user created at setup is **admin**. Admins create/disable users; no open self-registration by default.
- Each user has: username, display name, password, **profile picture** (uploaded, resized, stored in `/data/avatars` — this is the only upload in the app), settings.
- Per-user data: playlists (private, shared, or collaborative; others can save shared ones), play history, recently played, queue state (resumable across devices).

### UI
- Layout like Spotify: sidebar (library, playlists) + main view + persistent player bar on desktop; bottom tab bar + mini-player + full-screen "now playing" sheet on mobile.
- Style: must feel like a native Apple app. Two themes, **light** (white) and **dark** (true black, OLED), plus "Automatic" (follows the OS). Apple semantic tokens (`--label`, `--label-2`, `--fill`, `--sep`, `--material`…), hairline separators, iOS inset grouped lists in Settings, system materials (blur) only on floating chrome (tab bar, player bar, mini-player, toasts), never on content panels.
- **One accent color**, user-selectable in Settings (presets + custom), is the only brand color. Every gradient is derived from it (`--accent-grad`: lighter → accent → deeper): logo, primary buttons, page header wash, progress. No multi-hue gradients, no glows, no decorative background blobs.
- Logo: app-icon squircle filled with the accent gradient, white "M" drawn as five equalizer bars (`icons::Logo`).
- Typography: system font stack (SF Pro on Apple devices).
- **Mobile swipe gestures:**
  - Swipe left/right on the mini-player or now-playing artwork → next / previous track.
  - Swipe up on the mini-player → open now-playing; swipe down → close it.
  - Swipe right on a track row → add to queue.
  - Swipe from the left edge → navigate back.
- Gestures must feel physical: follow the finger, spring back, velocity-based thresholds.
- Media Session API for lock-screen / headset / Bluetooth controls. Installable as a PWA.
- Respect `prefers-reduced-motion` and `prefers-color-scheme` (the Automatic theme). Keyboard shortcuts on desktop (space, arrows, `s` shuffle, `r` repeat, `/` for search).
- Accessible: real buttons, labels, focus states, sufficient contrast on glass surfaces.

### Search
- Instant search across artists, albums, tracks and playlists (SQLite FTS5), results as you type.

## Performance targets

- Time to first audio < 300 ms on LAN for `original`.
- API list endpoints paginated and < 50 ms on a 50k-track library.
- Scan of 50k tracks (unchanged) < 30 s.
- Frontend: lazy-load images, virtualize long lists, keep the WASM bundle small (`opt-level = "z"`, `wasm-opt`).

## Security

- argon2id for passwords, rate limiting on login, constant-time comparisons.
- Strict CSP, no inline scripts, security headers via `tower-http`.
- Validate and re-encode uploaded avatars (never store the raw upload); enforce size and dimension limits.
- Container runs as a non-root user. Library mounted read-only.
- Log authentication events (success, failure, lockout) in a structured way.
- 2FA (TOTP, any authenticator app) with single-use recovery codes.
- CSRF: state-changing `/api` requests must be same-origin (`Sec-Fetch-Site`, else `Origin` vs `Host`), on top of `SameSite=Lax`.
- API responses default to `Cache-Control: no-store`; HSTS is sent when `MUZIK_SECURE_COOKIES` is on.

## Docker

- Volumes: `/music` (read-only), `/data` (DB, avatars), `/cache` (transcodes, thumbnails).
- Env: `MUZIK_LIBRARY_PATHS` (comma-separated), `MUZIK_DATA_DIR`, `MUZIK_CACHE_DIR`, `MUZIK_PORT` (default 9873), `MUZIK_SCAN_INTERVAL` (`3600`, `30m`, `6h`), `MUZIK_CACHE_MAX_SIZE` (`10G`).
- Extra env: `MUZIK_WEB_DIR` (built frontend, default `/app/web`), `MUZIK_SECURE_COOKIES` (default `true`; `false` only for plain-HTTP dev), `MUZIK_FFMPEG` (binary path), `MUZIK_LOG_FORMAT=json`, `RUST_LOG`, `MUZIK_TRUSTED_PROXIES` (comma-separated IPs/CIDRs whose `X-Forwarded-For` is trusted; empty = ignore the header), `MUZIK_MAX_TRANSCODES` (concurrent ffmpeg processes, default = CPUs capped at 8).
- Image: multi-stage (Rust builder → slim runtime with `ffmpeg`, no curl), healthcheck via `muzik-server healthcheck` (GETs `/api/health` on the local port).

## Commands

```bash
cargo build --workspace                 # build everything
cargo test --workspace                  # run tests
cargo clippy --workspace -- -D warnings # lint (must pass)
cargo fmt --all                         # format (must pass)
cd crates/muzik-web && trunk build --release  # build the web UI into dist/ (served by the backend)
cd crates/muzik-web && trunk serve      # frontend dev server on :8080, proxies /api to :9873
sqlx migrate run                        # apply migrations to the dev DB (see below)
docker compose up --build               # full stack
```

### sqlx compile-time checks

- `.env` sets `DATABASE_URL=sqlite:dev-data/muzik.db`; the dev DB must have the current schema for `query!` macros to compile. sqlx-cli needs the SQLite driver: `cargo install sqlx-cli --no-default-features --features sqlite,rustls`, then `sqlx database setup`.
- After adding/changing a query, refresh the offline metadata used by Docker builds: `SQLX_OFFLINE_DIR=$PWD/.sqlx cargo build -p muzik-server` (or `cargo sqlx prepare --workspace`). Commit `.sqlx/`.
- Build without a DB: `SQLX_OFFLINE=true cargo build`.

## Conventions

- No `unwrap()` / `expect()` outside tests and startup. Use `thiserror` for library errors, `anyhow` only in binaries.
- Errors returned by the API are JSON `{ "error": "code", "message": "..." }` with proper HTTP status codes.
- Use `tracing` for logs, never `println!`.
- Every DB schema change goes through a new sqlx migration; never edit an applied one.
- Blocking work (tag reading, image resizing) runs in `spawn_blocking` or a dedicated pool.
- Add tests for: scanner (fixtures with tagged sample files), range requests, auth, path resolution.
- Keep PRs focused; update this file when an architectural decision changes.

## Implementation notes

- `muzik-server` is a lib + thin `main.rs`; `muzik_server::app(state)` builds the router (used by `tests/api.rs`).
- Path safety lives in `library/paths.rs` (`LibraryRoots::resolve`): canonicalize + component-wise `starts_with` on a root. Every file open of library content goes through it.
- Scanner never removes tracks under a root that is missing or empty (protects against unmounted drives).
- Sessions: random 256-bit token in the cookie, only its SHA-256 is stored in `sessions.id`. Cookie is `__Host-muzik_session` with secure cookies, `muzik_session` without. Expired sessions/challenges are purged hourly. Password change, enabling 2FA, admin password/2FA reset and disabling a user revoke sessions (`session::revoke_all`).
- Client IP (`auth/client_ip.rs`, `ClientIp` extractor): TCP peer, or the right-most untrusted `X-Forwarded-For` hop when the peer is in `MUZIK_TRUSTED_PROXIES`. Used for rate limiting and auth logs.
- Auth rate limits (`auth/rate_limit.rs`, `AuthLimiter`): failures per user+IP (10/15 min), per IP (30), per user (100), so one client can't lock a user out everywhere. Password and second-factor failures (sign-in and re-auth in Settings) all count. Wrong second-factor codes (sign-in, Inkult 2FA, Settings, `/me/2fa/enable`) also count in a strict per-account tier (10/hour, `record_second_factor_failure`): only someone holding the password or a session can trigger it. Pending second-factor sign-ins are capped at 3 per account (`login_challenges`, `InkultClient::stash`), oldest dropped.
- 2FA (`auth/totp.rs`, migration `0004`): RFC 6238 SHA-1/6 digits/30 s, ±1 step, a step is never accepted twice (`users.totp_last_step`, conditional UPDATE). `users.totp_secret` = active secret (NULL = off); enrollment goes through `totp_pending_secret` (15 min) and is confirmed with a code. Sign-in: `POST /auth/login` answers `{status:"mfa_required"}` + an HttpOnly `muzik_mfa` challenge cookie (5 min, `login_challenges`, max 5 codes), finished by `POST /auth/login/2fa`. 10 recovery codes (`XXXXX-XXXXX`, SHA-256 stored, single-use). `/me/2fa/*` needs the password (and a code to disable/regenerate). Admins reset other users' 2FA/password via `PATCH /users/{id}` (not their own). QR is an SVG rendered server-side (`qrcode` crate), shown as a `data:` image.
- Mandatory 2FA (migration `0005`): admins require it per user (`users.totp_required`, via create/`PATCH /users/{id}`) or server-wide (`server_settings.require_totp`, `GET/PUT /api/admin/security`; the admin must use 2FA before turning it on). Effective = either flag (`User::totp_enforced`). `CurrentUser` rejects sessions with mandatory-but-missing 2FA with `403 mfa_setup_required`; only `SessionUser` handlers (`/me`, logout, `/me/2fa`, `/me/2fa/setup`, `/me/2fa/enable`) accept them. Enforcing never signs anyone out. `/me/2fa/disable` answers `403 mfa_required_by_policy` while enforced. The client shows `pages::auth::RequiredTotpSetup` instead of the shell when `User::needs_totp_setup()`, and re-checks `/me` on any `mfa_setup_required`.
- Transcodes are produced fully into `/cache/transcodes` (temp file + rename), then served with Range support; LRU-pruned to `MUZIK_CACHE_MAX_SIZE`.
- Handlers use the extractors in `api/extract.rs` (`Path`, `Query`, `Json`) so rejections follow the JSON error format.
- Test fixtures (tagged FLAC/MP3, untagged WAV + cover.jpg) are in `crates/muzik-server/tests/fixtures/library`.
- Without `MUZIK_*` path vars (local `cargo run`), the server defaults to `./music/library`, `./music/data`, `./music/cache` and `crates/muzik-web/dist`, relative to the repo root. Docker sets every path explicitly.
- Frontend: Leptos 0.8 CSR, a `cdylib` library crate (entry: `#[wasm_bindgen(start)]` in `lib.rs`) so the workspace has a single binary and `cargo run` starts the server. Built with `trunk build --release` in `crates/muzik-web` (trunk 0.21.x; `rustup target add wasm32-unknown-unknown`). The CSP forbids inline scripts, so the post-build hook `scripts/extract-loader.sh` moves trunk's inline loader into `/loader.js`. Styles set from Rust must use `style:prop=` (CSSOM), never a `style="..."` attribute.
- Shuffle/repeat (`player.rs`) are per-device `Settings` (`localStorage`). Shuffle on shuffles the songs after the current one and keeps the original order in `unshuffled` (enqueue/play-next update both) so turning it off restores it without interrupting playback; `play_list` with shuffle on plays the chosen song first, then the rest shuffled. Repeat cycles Off → All (wraps next/prev) → One (native `loop` on the active element, Next still skips).
- Playback uses two `<audio>` elements: the next queue item is preloaded into the inactive one and swapped in on `ended`. Quality per network type is stored per device in `localStorage`.
- Cover sizes are restricted server-side to `covers::SIZES` (96, 300, 600).
- Playlists (`api/playlists.rs`, migration `0002`): visible to the owner, or to everyone when `public`; invisible ones answer 404. Songs are editable by the owner, or by anyone when `public` + `collaborative`; details/sharing/delete are owner-only. Other users "save" (follow) shared playlists into their library. Entries have their own id (duplicates allowed, opt-in via `allow_duplicates`) and a contiguous 0-based `position`. Every write transaction starts with `touch()` (an UPDATE) so it takes SQLite's write lock before reading.
- Playlist entries store a title/artist/album snapshot; `track_id` is `ON DELETE SET NULL`. The scanner's finalize step (`relink_user_tracks`) refreshes snapshots and reattaches orphaned entries by title + artist + album, so moved/re-encoded files keep their playlists. Orphans show as "Unavailable".
- There is no "Liked Songs" feature (removed in migration `0003`); saving songs goes through playlists.
- Playlist covers are a client-side 2×2 mosaic of the first four albums with art (`cover_album_ids`); there are no playlist cover uploads (avatars stay the only upload).
- Frontend: `collection.rs` holds the user's playlists (loaded once, updated after each change; `touch(id)` makes an open playlist page refetch). `overlays.rs` hosts every menu/dialog once in the shell (popover at the pointer on desktop, action/bottom sheet on phones). Track rows open the track menu from ⋯ or right-click. No native `<select>`: single-choice menus use `overlays::Picker` (opens `Overlay::Choice`, a checkmark popover / titled action sheet). Reordering uses pointer events on a drag handle (desktop: on hover; touch: in Edit mode) with ↑/↓ on the focused handle as the keyboard alternative.
- Edge swipe back (`backswipe.rs`, Safari-style, wired on `<main>` in `lib.rs`): capture-phase touch handlers claim touches starting within 24 px of the left edge (rows don't see them). The live page follows the finger; the previous page is shown underneath from a snapshot (the detached DOM node a `MutationObserver` on `<main>` keeps when a navigation removes it, plus its scroll position) in the `.back-prev` layer. On commit it calls `history.back()` and the snapshot covers the swap until the real page is rendered and its scroll restored. Only active when an in-app back stack exists; disabled in iOS Safari tabs (`navigator.standalone === false`), which have their own back swipe.
- Now-playing sheet (`components::NowPlaying`): a scroll container. Header: close chevron, "Playing from …" (`Player::source`), ⋯ track menu. The player fills the first screen minus `--peek` (header + first song of the "Up Next" card); the artwork is sized from the viewport height (`--chrome-h`) so everything fits; swipe up on the artwork scrolls to the cards. Cards (`.sheet-card`): **Up Next** (rest of the queue, tap = jump; "Playing from …" comes from `Player::source`, which `play_list` copies from the label the visible album/playlist page sets via `player.here(name)`) and **Suggestions** (`GET /api/tracks/{id}/suggestions?limit=`, local-only: ranks same genre > same album artist > year ±5, random tie-break, excludes the seed's album; the client hides already-queued tracks, tap = enqueue).
- Appearance (`theme.rs`): theme + accent stored per device in `localStorage` (`muzik.appearance`); applied by setting `data-theme` on `<html>` and `--accent` / `--on-accent` via CSSOM. CSS defines light tokens on `:root` and dark tokens on `:root[data-theme="dark"]`. Accent presets must keep ≥ 3:1 contrast with white text.
- Inkult connector (`inkult.rs`, migration `0006`, like Jellyseerr's Jellyfin sign-in): admins set it in Settings > Inkult (`GET/PUT /api/admin/inkult`, `POST /api/admin/inkult/test` probes Inkult's public `/api/auth/state`); stored in `server_settings` (`inkult_enabled`, `inkult_url`). Turning it on or changing the URL decides where users' passwords go, so `PUT` needs `confirm_admin` (`api/users.rs`): the admin's password (+ 2FA code when on), else `403 reauth_required`; Inkult admins instead need a session created in the last 10 min (`403 recent_sign_in_required`). Turning it off or re-saving is free. `insecure` (on settings and test results, `inkult::is_insecure_url`) flags plain `http://` to a host off the local network (not loopback/private/CGNAT/ULA, single-label or `.local`/`.internal`/`.lan`/`.home.arpa` names); the panel shows a warning. While it is on, **every Inkult account is a Muzik user** (no opt-in or invite list; the `inkult_auto_create` switch was dropped in migration `0007`). `users.auth_provider` is `local` or `inkult`; Inkult accounts have `password_hash = '!inkult'` (never verifies) and `inkult_user_id` (Inkult's stable id, NULL until the first sign-in of an admin-created account). `POST /auth/login`: a local account is always checked locally (never falls back to Inkult); an Inkult account, or an unknown name while the connector is on, is checked by forwarding the credentials to Inkult's `POST /api/auth/login` (with `X-Forwarded-For` = client IP); Inkult's 2FA challenge is kept in memory (`InkultClient::stash`, 5 min, 5 codes) behind the usual `muzik_mfa` cookie and finished through `/auth/login/2fa` → Inkult's `/api/auth/login/2fa`. The Inkult session Muzik is handed is logged out right away. Account resolution (`link_inkult_account`): linked id (survives Inkult renames) → Inkult account an admin set up beforehand for that handle (e.g. as an administrator) → new account. A local account of the same name is never taken over (`inkult_name_taken`). Inkult accounts on a temporary password / pending 2FA enrollment are refused (`inkult_account_setup`). Muzik's mandatory 2FA is met by Inkult's own 2FA (`users.inkult_two_factor`, refreshed at each sign-in; `User::totp_enabled` reports it for Inkult accounts). Password/2FA endpoints answer `409 managed_by_inkult`. Admins switch an account with `PATCH /users/{id}` (`inkult: true`; or a `password` to make it local again), which signs it out. No HTTP redirects are followed, answers are capped at 64 KB, timeouts 5 s connect / 15 s total. Sessions don't re-check Inkult afterwards: disabling the Muzik account is what cuts access.
- Separate libraries per Inkult account (`inkult_index.rs`, migration `0008`, `PUT /api/admin/inkult/libraries`, `server_settings.inkult_separate_libraries`): Inkult keeps one shared music folder, and its `library.json` lists the owners (Inkult ids) of each file. Muzik reads that file read-only (`MUZIK_INKULT_LIBRARY`; paths are relative to `MUZIK_INKULT_MUSIC_DIR`, default the first library root) and mirrors it into `inkult_files` / `inkult_owners`, keyed by canonical path like `tracks.path`, so it doesn't depend on scan order. Polled every 30 s (stat; re-read only on size/mtime change) and after each scan's writes; only differences are written, grants before revokes. A failed read (missing, corrupt, `version` ≠ 1) keeps the previous copy. `CurrentUser::library_owner` is `Some(inkult_user_id)` only for an Inkult account that is not a Muzik admin and not an Inkult captain (`users.inkult_captain`, from Inkult's `role` at each sign-in) while the setting is on; else `None` (sees all). **Every query that exposes tracks must apply the ownership predicate** `(?N IS NULL OR NOT EXISTS (inkult_files for t.path) OR EXISTS (inkult_owners for t.path and ?N))`: library lists/details (albums/artists only appear with a visible track, and their counts only include visible tracks), track, suggestions, search, stream, album cover, and playlists (entries outside it come back with `track: None` ("Unavailable"), they're left out of duration and covers, and can't be added). Files not in Inkult's index are shared. Turning it on forces a sync and refuses when the index can't be read or none of its files match a track.

## Out of scope

- Uploading, editing tags, or deleting music from the app.
- Podcasts, video, social features, recommendations based on external services.
- Proprietary DRM or any integration with Spotify's API.

-- Users & sessions -----------------------------------------------------------

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT    NOT NULL,
    password_hash TEXT    NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    disabled      INTEGER NOT NULL DEFAULT 0,
    avatar_file   TEXT,
    totp_secret   TEXT,
    created_at    INTEGER NOT NULL
);

-- `id` is the SHA-256 of the session token; the raw token only lives in the cookie.
CREATE TABLE sessions (
    id           TEXT    PRIMARY KEY NOT NULL,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    user_agent   TEXT
);
CREATE INDEX sessions_user ON sessions(user_id);

-- Library ------------------------------------------------------------------

CREATE TABLE artists (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    sort_name TEXT NOT NULL
);

CREATE TABLE albums (
    id             INTEGER PRIMARY KEY,
    artist_id      INTEGER NOT NULL REFERENCES artists(id),
    title          TEXT    NOT NULL COLLATE NOCASE,
    year           INTEGER,
    -- External cover file (absolute, canonical path inside a library root).
    cover_path     TEXT,
    -- Track whose embedded picture is used as cover when no external file wins.
    cover_track_id INTEGER,
    UNIQUE (artist_id, title)
);
CREATE INDEX albums_artist ON albums(artist_id);

CREATE TABLE tracks (
    id                 INTEGER PRIMARY KEY,
    -- Absolute canonical path. Never exposed through the API.
    path               TEXT    NOT NULL UNIQUE,
    size               INTEGER NOT NULL,
    mtime              INTEGER NOT NULL,
    album_id           INTEGER NOT NULL REFERENCES albums(id),
    artist_name        TEXT    NOT NULL,
    title              TEXT    NOT NULL,
    disc_no            INTEGER,
    track_no           INTEGER,
    year               INTEGER,
    genre              TEXT,
    duration_ms        INTEGER NOT NULL DEFAULT 0,
    codec              TEXT    NOT NULL,
    mime               TEXT    NOT NULL,
    sample_rate        INTEGER,
    bit_depth          INTEGER,
    channels           INTEGER,
    bitrate_kbps       INTEGER,
    lossless           INTEGER NOT NULL DEFAULT 0,
    has_embedded_cover INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX tracks_album ON tracks(album_id, disc_no, track_no);

-- Full-text search over artists, albums and tracks. Rebuilt after each scan.
CREATE VIRTUAL TABLE search_fts USING fts5(
    kind UNINDEXED,
    ref_id UNINDEXED,
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);

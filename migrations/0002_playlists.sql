-- Playlists, saved (followed) playlists and liked songs ----------------------
--
-- Entries keep a snapshot of the track's title / artist / album. When a file
-- disappears from the library its `track_id` becomes NULL (the entry stays,
-- shown as unavailable) and the scanner relinks it to a track with the same
-- title, artist and album after the next scan, so moving or renaming files
-- does not empty playlists.

CREATE TABLE playlists (
    id            INTEGER PRIMARY KEY,
    owner_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    description   TEXT    NOT NULL DEFAULT '',
    -- Visible to every user of this server (who can then save it).
    public        INTEGER NOT NULL DEFAULT 0,
    -- Anyone who can see it may add, remove and reorder songs. Requires public.
    collaborative INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE INDEX playlists_owner ON playlists(owner_id);
CREATE INDEX playlists_public ON playlists(public, updated_at);

CREATE TABLE playlist_tracks (
    -- Entry id: the same track may appear more than once in a playlist.
    id           INTEGER PRIMARY KEY,
    playlist_id  INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id     INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    -- 0-based, contiguous within a playlist.
    position     INTEGER NOT NULL,
    added_by     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    added_at     INTEGER NOT NULL,
    title        TEXT    NOT NULL,
    artist_name  TEXT    NOT NULL,
    album_title  TEXT    NOT NULL,
    duration_ms  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX playlist_tracks_order ON playlist_tracks(playlist_id, position);
CREATE INDEX playlist_tracks_track ON playlist_tracks(track_id);

CREATE TABLE playlist_follows (
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (user_id, playlist_id)
);
CREATE INDEX playlist_follows_playlist ON playlist_follows(playlist_id);

CREATE TABLE liked_tracks (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    track_id    INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    liked_at    INTEGER NOT NULL,
    title       TEXT    NOT NULL,
    artist_name TEXT    NOT NULL,
    album_title TEXT    NOT NULL
);
CREATE UNIQUE INDEX liked_tracks_user_track ON liked_tracks(user_id, track_id);
CREATE INDEX liked_tracks_recent ON liked_tracks(user_id, liked_at);
CREATE INDEX liked_tracks_track ON liked_tracks(track_id);

-- Used to relink entries after a rescan.
CREATE INDEX tracks_title ON tracks(title COLLATE NOCASE);

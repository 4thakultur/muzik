-- Separate libraries per Inkult account ----------------------------------------
--
-- Inkult stores every download once in a shared music folder and records in
-- its `library.json` which Inkult accounts own each file. Muzik mirrors that
-- index here (`inkult_index.rs`), keyed by the file's absolute canonical path
-- (the same form as `tracks.path`), so a track is classified as soon as it is
-- scanned, whichever happens first.
--
-- With `inkult_separate_libraries` on, an Inkult account that is neither a
-- Muzik admin nor an Inkult captain only sees tracks it owns, plus tracks
-- whose file is not in Inkult's index at all (copied in by hand: shared).

ALTER TABLE server_settings ADD COLUMN inkult_separate_libraries INTEGER NOT NULL DEFAULT 0;

-- The Inkult account was a captain (Inkult administrator) at its last sign-in.
ALTER TABLE users ADD COLUMN inkult_captain INTEGER NOT NULL DEFAULT 0;

-- Every file listed in Inkult's index, owned or not.
CREATE TABLE inkult_files (
    path TEXT PRIMARY KEY NOT NULL
) WITHOUT ROWID;

-- Who owns each indexed file (Inkult account ids, as in `users.inkult_user_id`).
CREATE TABLE inkult_owners (
    path     TEXT    NOT NULL,
    owner_id INTEGER NOT NULL,
    PRIMARY KEY (path, owner_id)
) WITHOUT ROWID;

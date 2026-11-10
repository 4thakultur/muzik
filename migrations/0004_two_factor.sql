-- Two-factor authentication (TOTP) ------------------------------------------
--
-- `users.totp_secret` (from 0001) holds the active base32 secret; NULL = 2FA off.
-- A new secret first lives in `totp_pending_secret` until the user proves
-- their app works by entering a code.

ALTER TABLE users ADD COLUMN totp_enabled_at INTEGER;
ALTER TABLE users ADD COLUMN totp_pending_secret TEXT;
ALTER TABLE users ADD COLUMN totp_pending_at INTEGER;
-- Last accepted time step: a code is never accepted twice.
ALTER TABLE users ADD COLUMN totp_last_step INTEGER;

-- Single-use recovery codes (SHA-256 of the normalized code).
CREATE TABLE recovery_codes (
    id        INTEGER PRIMARY KEY,
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT    NOT NULL,
    used_at   INTEGER
);
CREATE INDEX recovery_codes_user ON recovery_codes(user_id, code_hash);

-- Sign-ins waiting for their second factor. `id` is the SHA-256 of the token
-- kept in the short-lived HttpOnly challenge cookie.
CREATE TABLE login_challenges (
    id         TEXT    PRIMARY KEY NOT NULL,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX login_challenges_user ON login_challenges(user_id);

-- Inkult connector: sign in to Muzik with an Inkult account -------------------
--
-- `auth_provider` says who checks the password: 'local' (argon2 hash in
-- `password_hash`) or 'inkult' (the configured Inkult server; `password_hash`
-- then holds an unusable marker). `inkult_user_id` is Inkult's stable account
-- id, recorded at the first successful Inkult sign-in, so a renamed Inkult
-- account still reaches the same Muzik account. It is NULL for an account an
-- admin created for an Inkult handle that hasn't signed in yet.
-- `inkult_two_factor`: the Inkult account used 2FA at its last sign-in (for
-- Muzik's mandatory-2FA policy, which Inkult accounts satisfy through Inkult).

ALTER TABLE users ADD COLUMN auth_provider TEXT NOT NULL DEFAULT 'local'
    CHECK (auth_provider IN ('local', 'inkult'));
ALTER TABLE users ADD COLUMN inkult_user_id INTEGER;
ALTER TABLE users ADD COLUMN inkult_two_factor INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX users_inkult_user ON users(inkult_user_id) WHERE inkult_user_id IS NOT NULL;

-- Connection settings, edited by admins in Settings. Off by default: Muzik
-- makes no request to Inkult until an admin turns it on.
ALTER TABLE server_settings ADD COLUMN inkult_enabled INTEGER NOT NULL DEFAULT 0;
-- Base URL, e.g. 'http://inkult:9876' (no trailing slash).
ALTER TABLE server_settings ADD COLUMN inkult_url TEXT;
-- Create a Muzik account on the first sign-in of an unknown Inkult user.
ALTER TABLE server_settings ADD COLUMN inkult_auto_create INTEGER NOT NULL DEFAULT 0;

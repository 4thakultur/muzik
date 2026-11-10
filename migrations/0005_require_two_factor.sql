-- Admin-controlled 2FA policy. 2FA is mandatory for a user when either their
-- own `totp_required` flag or the server-wide `require_totp` switch is on.
-- Until such a user has set up an authenticator app, their session can only
-- reach the enrollment endpoints.

ALTER TABLE users ADD COLUMN totp_required INTEGER NOT NULL DEFAULT 0;

-- Server-wide settings: exactly one row.
CREATE TABLE server_settings (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    require_totp INTEGER NOT NULL DEFAULT 0
);
INSERT INTO server_settings (id) VALUES (1);

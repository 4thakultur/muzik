-- With the Inkult connector on, every Inkult account is a Muzik user: its
-- Muzik account is created at its first sign-in. The opt-in switch is gone.
ALTER TABLE server_settings DROP COLUMN inkult_auto_create;

-- Separate libraries: Inkult captains choose ----------------------------------
--
-- Inkult captains hear the whole library by default while separate libraries
-- are on. Each captain can instead choose, in their own Settings, to only hear
-- the songs they own in Inkult (plus songs added to the folder by hand), like
-- any other Inkult account.

ALTER TABLE users ADD COLUMN inkult_own_library INTEGER NOT NULL DEFAULT 0;

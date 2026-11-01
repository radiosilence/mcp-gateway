-- Session ids are now stored as their SHA-256, so a read of this table no
-- longer yields usable sessions. Existing rows hold raw ids that no cookie
-- hashes to, so they are removed and everyone signs in again once.
DELETE FROM sessions;

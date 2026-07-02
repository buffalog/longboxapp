-- Cover URL for a cached CV volume (CV's medium cover image). NULL when
-- not yet fetched or when CV has no cover. Used by Discovery to show a
-- thumbnail per not-owned volume.
ALTER TABLE cv_volume_cache ADD COLUMN cover_url TEXT;

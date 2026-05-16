-- File-presence tracking for the scanner. `is_present` is the source of
-- truth for "is this file still on disk"; `last_seen_at` is the timestamp of
-- the most recent scan that touched the row. The scan's final pass marks
-- rows it didn't see as `is_present = false` via
-- `file_repo::mark_files_not_seen_since`. Status (`owned`/`needs_review`/
-- `ignored`) remains an independent column — presence and match
-- classification are different concerns.

ALTER TABLE files ADD COLUMN is_present BOOLEAN NOT NULL DEFAULT 1;
ALTER TABLE files ADD COLUMN last_seen_at TEXT NOT NULL DEFAULT (datetime('now'));
CREATE INDEX idx_files_root_present ON files(library_root_id, is_present);

-- Library Tidy Step 1: catalog/disk reconciliation schema.

-- Per-series count of matched (owned, present) files at the end of the
-- last full scan. The scanner updates it at scan end; the tidy logic
-- compares it against the *current* owned count to classify a
-- zero-owned series as a "transition" phantom (last_matched_count > 0
-- — the series just lost its files) versus steady-state.
--
-- Backfilled here from the current owned-file count so transition
-- detection is live from the very first post-migration scan rather
-- than the second. A series that is already zero-owned backfills to 0
-- and so reads as steady-state, not transition — correct for the
-- existing phantom backlog.
ALTER TABLE series ADD COLUMN last_matched_count INTEGER NOT NULL DEFAULT 0;

UPDATE series SET last_matched_count = (
    SELECT COUNT(*)
    FROM files f
    JOIN issues i ON f.issue_id = i.id
    WHERE i.series_id = series.id
      AND f.status = 'owned'
      AND f.is_present = 1
);

-- Top-level library subfolders that look series-shaped (contain CBZ
-- files) but don't resolve to any tracked series. Surfaced in
-- /library/tidy for the user to add or dismiss; never auto-resolved to
-- ComicVine (folder-name-to-CV mapping needs user disambiguation).
--
-- `dismissed_at` is set when the user opts out of tracking a folder; a
-- dismissed row is left untouched on re-detection (the upsert carries
-- a `WHERE dismissed_at IS NULL` guard).
CREATE TABLE discovered_folders (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_name    TEXT NOT NULL UNIQUE,
    first_seen_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    dismissed_at   TIMESTAMP,
    file_count     INTEGER NOT NULL DEFAULT 0
);

-- Task 3: dashboard activity feed needs to know "when did this file
-- become matched?" Existing matched files stay NULL on this column
-- (no good backfill source: last_seen_at updates every scan, series
-- created_at is the wrong thing, no transition history exists). The
-- column populates organically from now forward.
--
-- Update rule (enforced in longbox-db's file_repo + caller helpers):
--   - issue_id NULL -> Some(N)      : matched_at = NOW
--   - issue_id Some(a) -> Some(b!=a): matched_at = NOW
--   - issue_id Some(N) -> Some(N)   : matched_at unchanged
--   - issue_id Some(_) -> NULL      : matched_at = NULL (mark-ignored,
--                                     scanner reverting to unmatched)

ALTER TABLE files ADD COLUMN matched_at TIMESTAMP;

CREATE INDEX idx_files_matched_at_desc ON files (matched_at DESC)
    WHERE matched_at IS NOT NULL;

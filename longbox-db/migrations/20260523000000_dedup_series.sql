-- A.9 hot-fix: clean up (sort_title, start_year) duplicate series.
--
-- Bulk-convert (6a) inserted shallow series directly via series_repo
-- with no idempotency check on (sort_title, start_year) — the cv_id
-- idempotency in add_or_get_from_cv didn't cover the shallow path.
-- The result is duplicate series in three patterns: CV + shallow (the
-- shallow's synthesized issues stranded after the scanner re-cascaded
-- files back to the CV survivor), CV + shallow with 0 synthesized
-- issues (filename parser gaps — separate deferred item), and shallow
-- + shallow with NULL start_year (folder names lacking a `(YYYY)`).
--
-- For each (sort_title, start_year) group with >1 rows, rank a
-- survivor — cv_id-set first, then highest owned+present file count,
-- then earliest created_at — and DELETE every non-survivor whose
-- owned+present file count is 0. The owned-file guard is load-bearing:
-- a row with file attachments is never dropped, no matter how it
-- ranked.
--
-- ON DELETE CASCADE nukes the dupe's (synthesized) issues; files that
-- pointed at those issues become `issue_id = NULL` via ON DELETE SET
-- NULL and re-match on the next scan. For Pattern A folders the user
-- re-runs bulk-convert post-deploy — link mode then attaches files to
-- the CV survivor with FilenameRegex / owned / confidence 1.0, the
-- only attachment shape that survives the next scan's cascade.
--
-- Idempotent: re-running on a clean catalog deletes nothing.
--
-- PARTITION BY treats NULL as a single bucket in SQLite (Pattern C's
-- NULL-year rows dedup against each other), but COALESCE(start_year,
-- -1) is defensive against future SQLite quirks. -1 is not a valid
-- four-digit year so the sentinel cannot collide with real data.

WITH owned_counts AS (
    SELECT s.id,
           s.sort_title,
           s.start_year,
           s.cv_id,
           s.created_at,
           (SELECT COUNT(*) FROM files f JOIN issues i ON f.issue_id = i.id
            WHERE i.series_id = s.id
              AND f.status = 'owned' AND f.is_present = 1) AS owned_count
    FROM series s
),
ranked AS (
    SELECT id, owned_count,
           ROW_NUMBER() OVER (
               PARTITION BY sort_title, COALESCE(start_year, -1)
               ORDER BY (cv_id IS NULL) ASC, owned_count DESC, created_at ASC
           ) AS rn,
           COUNT(*) OVER (PARTITION BY sort_title, COALESCE(start_year, -1)) AS group_size
    FROM owned_counts
)
DELETE FROM series WHERE id IN (
    SELECT id FROM ranked
    WHERE group_size > 1 AND rn > 1 AND owned_count = 0
);

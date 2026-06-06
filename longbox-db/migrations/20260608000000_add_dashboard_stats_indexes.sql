-- Dashboard load was 13 seconds, with `/api/stats` alone taking
-- 4.8 s on a real catalog. The stats handler runs ~10 correlated
-- subqueries against `files`, `issues`, and `pull_attempts`; the
-- two costliest patterns were:
--
--   1. `SELECT COUNT(*) FROM files WHERE status=? AND is_present=1`
--      — four times, once per status enum. The existing
--      `idx_files_status` covers the leading column but SQLite has
--      to scan-filter on `is_present` per matching row. A composite
--      lets the index serve the whole predicate.
--
--   2. `EXISTS (SELECT 1 FROM files f WHERE f.issue_id=? AND
--               f.status='owned' AND f.is_present=1)`
--      — runs once per issue inside `missing_issues` /
--      `series_with_missing`. Existing `idx_files_issue` covers
--      the join column; the three-column composite below covers
--      the WHOLE predicate so the lookup never touches the row
--      payload at all.
--
-- The pull-failures count
--   `WHERE status IN ('failed','mismatched') AND id = (SELECT MAX...)`
-- benefits from a `pull_attempts(status)` index — the existing
-- `idx_pull_attempts_series_issue` doesn't help because status is
-- not its leading column.
--
-- All three indexes are additive: zero queries change semantics, the
-- write path takes one more B-tree update per file/attempt insert
-- (sub-millisecond). Existing single-column indexes stay; they're
-- still useful for query plans that don't hit the new composites.
--
-- Expected impact on the production catalog: stats query drops from
-- ~4.8 s to <200 ms; dashboard cold load goes 13 s → ~3 s once the
-- caches warm.

CREATE INDEX IF NOT EXISTS idx_files_status_present
    ON files (status, is_present);

CREATE INDEX IF NOT EXISTS idx_files_issue_status_present
    ON files (issue_id, status, is_present);

CREATE INDEX IF NOT EXISTS idx_pull_attempts_status
    ON pull_attempts (status);

-- Refresh SQLite's planner stats so the new indexes get picked
-- immediately rather than after enough query traffic forces
-- automatic stat refresh.
ANALYZE;

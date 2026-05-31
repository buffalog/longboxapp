-- Item E v2: a per-volume CV cache so the calendar's publisher grouping
-- can render correctly for entries that LongBox does NOT track as a
-- series (~90% of any given week's calendar).
--
-- The 6c.5 series-JOIN path (calendar.rs::enrich, cv_volume_id -> series.publisher)
-- only resolves for catalog volumes. CV's /issues/ endpoint never
-- carries publisher (see calendar.rs:5-8), so for untracked volumes
-- the previous shape rendered every row as "Unknown Publisher". This
-- cache is the second-source fallback: the calendar handler queries
-- cv_volume_cache for any cv_volume_id missing from the series JOIN,
-- and uses publisher from there when available.
--
-- Lifecycle:
--   1. Calendar response sees an uncached cv_volume_id (no series, no
--      cache row). It bulk INSERT-OR-IGNORE's a pending row
--      (publisher NULL, description NULL, start_year NULL,
--      fetched_at NULL, first_seen_at = CURRENT_TIMESTAMP).
--   2. The enrichment worker's third pass (after shallow + refresh,
--      6c.5 piece 2 follow-on) drains rows with fetched_at IS NULL —
--      calls fetch_volume per cv_volume_id, writes publisher /
--      description / start_year, sets fetched_at = CURRENT_TIMESTAMP.
--   3. Next calendar load looks up the now-filled row and surfaces
--      publisher in the response.
--
-- Cache is permanent: cv_volume_id -> publisher is structurally stable
-- (CV doesn't reassign volumes between publishers), and the table is
-- bounded by "volumes that have ever shipped in a calendar window
-- LongBox has loaded" — slow-growth, no eviction needed.
--
-- description and start_year are cached too even though Item E only
-- consumes publisher: fetch_volume returns all three in one call, and
-- discarding them would repeat the 6c.5 / Bug 5 lesson about not
-- throwing away payload data already paid for. Currently unused but
-- on hand for future calendar tooltips, series detail enhancements,
-- and — notably — the deferred year-mismatch surface in the pull
-- engine, which compares indexer-reported release year against
-- series.start_year and would benefit from a cache fallback for
-- volumes not yet in the catalog.

CREATE TABLE cv_volume_cache (
    cv_volume_id    INTEGER PRIMARY KEY,
    publisher       TEXT,
    description     TEXT,
    start_year      INTEGER,
    -- NULL = pending (queued by a calendar response, awaiting the
    -- worker's cache-fill pass). NOT NULL = successfully fetched and
    -- written. A successful fetch with no publisher returned by CV
    -- still sets fetched_at; the worker uses fetched_at, not publisher,
    -- to mean "I've done the work for this id".
    fetched_at      TIMESTAMP,
    first_seen_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- The worker's pending query is the hot path; an index makes the
-- partial scan over fetched_at IS NULL cheap once the table grows.
CREATE INDEX idx_cv_volume_cache_pending
    ON cv_volume_cache(fetched_at)
    WHERE fetched_at IS NULL;

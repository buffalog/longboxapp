-- Add `series.finished` — whether the run is marked finished (Metron series
-- `status` = Completed | Cancelled, i.e. "nothing more is coming"). Drives
-- the series-card purple "complete collection" badge.
--
-- Metron-only signal: ComicVine carries no equivalent, so CV-only series
-- (and every series until the Metron finished-state enrichment runs) stay
-- false. Populated on the Metron series-detail fetch path and by the
-- on-demand `POST /api/series/enrich-finished` backfill.
ALTER TABLE series ADD COLUMN finished BOOLEAN NOT NULL DEFAULT 0;

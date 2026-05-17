-- Task C: persisted scan history.
--
-- The scan_runs table exists since Step 2 but has been dead code. This
-- migration adds the one column we need to make it usable: `kind`, so
-- the UI can distinguish full scans from rescans (and ignore internal
-- rematch_for_series rows). All three enum arms are accepted at the DB
-- level even though no INSERT path currently writes `rematch_for_series`
-- (auto-rematches stay in-memory per Task C design decision); the arm
-- is kept here so we can change that policy without a migration.

ALTER TABLE scan_runs
    ADD COLUMN kind TEXT NOT NULL
        DEFAULT 'full'
        CHECK (kind IN ('full', 'rescan_unmatched', 'rematch_for_series'));

CREATE INDEX idx_scan_runs_recent
    ON scan_runs (started_at DESC, id DESC);

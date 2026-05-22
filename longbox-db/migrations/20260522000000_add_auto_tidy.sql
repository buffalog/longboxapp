-- A.9 Step 6b: auto-tidy on folder removal.
--
-- consecutive_empty_scans — number of consecutive full scans that left
-- this series with zero owned, present files. The scanner ticks it at
-- scan end: incremented when the series is empty, reset to 0 when an
-- owned+present file exists. Backfilled to 0 — there is no scan history
-- to reconstruct, so every existing series (the phantom backlog
-- included) starts its clock fresh, which is the conservative choice.
--
-- auto_tidy_due_at — when set, the series has been *marked* for
-- automatic removal: it stayed empty past the debounce threshold and is
-- not awaiting a first download. The value is the recovery deadline; a
-- scan-end purge hard-deletes the series once it passes. A scan that
-- finds the series owns files again clears it (and resets the counter)
-- — the auto-recovery path. NULL = not marked. A marked series stays a
-- fully-normal row: it is surfaced in /library/tidy with a countdown,
-- never hidden from any other surface, so no query needs a new filter.

ALTER TABLE series ADD COLUMN consecutive_empty_scans INTEGER NOT NULL DEFAULT 0;
ALTER TABLE series ADD COLUMN auto_tidy_due_at TIMESTAMP;

-- Auto-tidy master switch, defaulted on. Read by the scanner via
-- settings_repo::get_or_default; only the *marking* step is gated — the
-- counter ticks regardless, so flipping this on later takes effect on
-- the very next scan.
INSERT INTO settings (key, value) VALUES ('auto_tidy_enabled', 'true');

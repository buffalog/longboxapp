-- Lower the seeded min_file_size_mb from 35 to 10.
--
-- The 35 MB floor was over-tuned: it rejected legitimate older
-- comic releases as "phase_b.rejected_too_small" — Y The Last Man
-- issues at 26 MB, Fear Agent at 32 MB, Morning Glories at 33 MB,
-- Scalped at 14 MB. Older CBR/CBZ files commonly land in the
-- 10-30 MB range; 35 was incompatible with the user's actual
-- back-catalog.
--
-- 10 MB still catches the original load-bearing failure mode (an
-- Absolute Batman partial delivery at 16 MB with five rendered
-- pages — still above the new floor, but Phase B's confidence
-- threshold and the 24h stale-grabbed purge handle that path
-- regardless).
--
-- The WHERE clause guards against stomping a user who tuned the
-- value manually through the Settings UI — only the literal
-- "still on the original seed" case gets updated.

UPDATE settings SET value = '10' WHERE key = 'min_file_size_mb' AND value = '35';

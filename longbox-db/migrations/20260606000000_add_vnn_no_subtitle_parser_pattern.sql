-- New filename pattern covering `Series vNN (YYYY)` releases that
-- carry NO subtitle separator between the volume number and the year.
-- Surfaced by Phase B failing to import real SAB-downloaded filenames
-- like `EC - Cruel Universe v01 (2025) (digital) (Son of
-- Ultron-Empire).cbz` and `Hunt. Kill. Repeat. v01 (2023)
-- (Collection).cbz`.
--
-- Pre-existing patterns can't claim these:
--   - id=1 (Vol N #M) needs `#issue` after the volume number.
--   - id=8 (vN - Subtitle (YYYY)) needs ` - Subtitle ` between the
--     volume and the year.
--   - id=2, id=10 (bare-digit + year) treat the leading `v` as part
--     of the series and can't reach the `01` as a number token —
--     `\s+#?\d+` requires whitespace-then-digit, and `v01` has no
--     whitespace inside it.
--
-- This pattern captures the volume number AS the issue number — same
-- pragmatic choice id=8 makes for TPB-only collected editions. Hybrid
-- series with both single-issue #N and TPB vN would collide; in
-- practice the user's library overwhelmingly has one or the other.
--
-- Priority 9 — between id=9 (Book N at 7) and id=2 (#NNN at 10), so
-- the volume-specific shape wins before the bare-digit patterns get
-- their backtrack chance.
--
-- `longbox-core::filename::default_patterns()` mirrors this seed for
-- in-process tests; keep the two in lockstep.

INSERT INTO parsing_patterns (name, pattern, priority, enabled) VALUES
  ('Series vNN (YYYY) no subtitle',
   '^(?P<series>.+?)\s+(?i:v|vol|volume)\s*(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$',
   9, 1);

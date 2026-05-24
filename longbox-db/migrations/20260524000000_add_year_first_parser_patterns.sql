-- A.9 parser hot-fix: three new filename patterns covering shapes the
-- existing four seed patterns miss.
--
-- Surfaced by archaeology after the dedup hot-fix: 4,182 of ~7,500
-- library files were unmatched, with the dominant shape being
-- `Series (YYYY) NNN.ext` (year-in-parens BEFORE the number). The
-- existing catch-all pattern (priority 30) absorbed those filenames
-- but baked the year into `series_title` (e.g. "Wolverine (2024)"),
-- which then poisoned the scanner's title-similarity match against
-- CV's clean `sort_title = "wolverine"` and dropped files to
-- unmatched / needs_review. Bulk-convert was unaffected because it
-- uses only `parsed.number` and discards `series_title` — same parser,
-- different consumer.
--
-- The three new patterns at priorities 11, 12, 15 sit ABOVE the
-- catch-all (30) so they claim their shapes before it can. They sit
-- around the existing year-after-number patterns (10, 20), which
-- don't match these shapes at all (different operand order) so
-- there's no claim conflict.
--
-- `longbox-core::filename::default_patterns()` mirrors this seed for
-- in-process tests; keep the two in lockstep when adding more.
--
-- The scanner loads patterns via `parsing_pattern_repo::list_enabled`
-- on every scan, so no code wire-in is needed — new rows take effect
-- on the first scan after deploy.

INSERT INTO parsing_patterns (name, pattern, priority, enabled) VALUES
  ('Series N (Xf Y) (YYYY)',
   '^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(\d+f\s*\d+\)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$',
   11, 1),
  ('Series N - Subtitle (YYYY)',
   '^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+-\s+(?P<title>.+?)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$',
   12, 1),
  ('Series (YYYY) NNN',
   '^(?P<series>.+?)\s+\((?P<year>\d{4})\)\s+#?(?P<number>\d+(?:\.\d+)?).*?\.(?i:cbz|cbr|cb7)$',
   15, 1);

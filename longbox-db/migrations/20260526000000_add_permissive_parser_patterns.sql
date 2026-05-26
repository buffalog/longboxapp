-- A.9 Bug 1b: three new filename patterns covering shapes the existing
-- seven seed patterns miss. Surfaced by Bug 1a's rollback verification:
-- three ghost folders (Fear Agent v01, Promethea Book 1, The Authority
-- Revolution 001 (2004-12)) each had every file fail every pattern, so
-- convert tried to create empty series rows. Bug 1a's rollback hides
-- the bug; this migration removes it.
--
-- Priority placement follows the cascade principle re-derived during
-- the kickoff: specific markers low-priority (claim before generic),
-- permissive shapes high-priority (sit just above the catch-all so
-- they only fire on what the strict patterns miss).
--
--   id=8  priority=6   "Series vN - Subtitle (YYYY) ..."
--                      TPB volume + subtitle. Specific marker (v/vol)
--                      sits ahead of id=2 (priority 10) so a TPB-only
--                      series is recognized as such instead of being
--                      claimed by a strict issue-number pattern.
--   id=9  priority=7   "Series Book N (YYYY) ..."
--                      Literal "Book N" marker. Strips "Book" from
--                      series_title so the catalog records the series
--                      as "Promethea" not "Promethea Book".
--   id=10 priority=25  "Series NNN (YYYY[-MM]) ..."
--                      Permissive tail + optional year-month stamp.
--                      Sits BELOW all strict-year patterns (10, 11,
--                      12, 15, 20) so they win first — preserves the
--                      title capture for filenames like
--                      "Saga 001 (2014) - Volume One.cbz" via id=2.
--                      Only fires on the shapes id=2/3 cannot claim:
--                      year-month stamps `(YYYY-MM)`, or trailing
--                      scanlator markers like ` (digital).cbr`.
--
-- All three carry a permissive `.*?\.<ext>$` tail so scanlator
-- markers don't block the match (mirrors the A.9 priority-11/12/15
-- patterns).
--
-- `longbox-core::filename::default_patterns()` mirrors this seed for
-- in-process tests; keep the two in lockstep when adding more.

INSERT INTO parsing_patterns (name, pattern, priority, enabled) VALUES
  ('Series vN - Subtitle (YYYY)',
   '^(?P<series>.+?)\s+(?i:v|vol|volume)\s*(?P<number>\d+)\s+-\s+.+?\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$',
   6, 1),
  ('Series Book N (YYYY)',
   '^(?P<series>.+?)\s+(?i:book)\s+(?P<number>\d+)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$',
   7, 1),
  ('Series NNN (YYYY[-MM]) permissive',
   '^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})(?:-\d{1,2})?\)(?:\s+-\s+(?P<title>[^()]+))?.*?\.(?i:cbz|cbr|cb7)$',
   25, 1);

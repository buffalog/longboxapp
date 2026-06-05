-- Three new filename patterns covering the `(of N)` part-of-mini
-- marker. Surfaced by HTTP 422 on filenames like
-- `A Vicious Circle 002 (of 03) (2023) (Digital-Empire).cbr` —
-- existing patterns either required a year immediately after the
-- number (id=2, id=3, id=10), required a different part-of marker
-- shape (id=5's `(Xf Y)`), or had a strict end (id=4 catchall),
-- none of which accept the `(of NN)` group between the number and
-- the rest of the filename. With all patterns missing,
-- `parse_basename` returns None and the scanner errors out.
--
-- Priority placement: the `(of N) (YYYY)` variant sits at 13 — same
-- specific-shape neighborhood as id=5 (priority 11) and id=6
-- (priority 12) — so it claims its filenames before the strict
-- year-after-number patterns (id=2 at 10 would NOT match anyway
-- because of the intervening `(of NN)`, but lower priority is the
-- consistent intent). The year-first `(YYYY) N (of M)` variant
-- sits at 14, just under the strict year-first pattern (id=7 at
-- 15). The year-less catcher sits at 28 — just above the catchall
-- (id=4 at 30) so it only fires when nothing strict claims the
-- shape.
--
-- All three carry the permissive `.*?\.<ext>$` tail so trailing
-- scanlator markers like `(Digital-Empire)` don't block the match,
-- mirroring the A.9 hot-fix conventions.
--
-- `longbox-core::filename::default_patterns()` mirrors this seed for
-- in-process tests; keep the two in lockstep when adding more.

INSERT INTO parsing_patterns (name, pattern, priority, enabled) VALUES
  ('Series N (of M) (YYYY)',
   '^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(of\s+\d+\)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$',
   13, 1),
  ('Series (YYYY) N (of M)',
   '^(?P<series>.+?)\s+\((?P<year>\d{4})\)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(of\s+\d+\).*?\.(?i:cbz|cbr|cb7)$',
   14, 1),
  ('Series N (of M) yearless',
   '^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(of\s+\d+\).*?\.(?i:cbz|cbr|cb7)$',
   28, 1);

-- Teach every filename pattern that `.pdf` is a comic extension.
--
-- Each seeded pattern ends in the anchor `\.(?i:cbz|cbr|cb7)$`, so a PDF's
-- filename matched nothing and every PDF fell out of the cascade with "no
-- series hint". That is fatal rather than merely lossy for this one format:
-- a CBZ that the filename parser can't claim still has its embedded
-- ComicInfo to fall back on, and a PDF never does. The filename (or the SAB
-- job folder, parsed with these same patterns) is the ONLY evidence a PDF
-- carries about which issue it is.
--
-- Rewriting the anchor rather than re-seeding the rows: the patterns have
-- been edited across nine migrations and are operator-editable at runtime,
-- so the shipped text is not the only text out there. `replace` reaches
-- every row that uses the idiom, including any the operator added.
-- No-ops on a row that already lists pdf, so re-running changes nothing.
--
-- `longbox-core::filename::default_patterns()` mirrors this seed for
-- in-process tests; keep the two in lockstep.

UPDATE parsing_patterns
   SET pattern = replace(pattern, '(?i:cbz|cbr|cb7)', '(?i:cbz|cbr|cb7|pdf)')
 WHERE pattern LIKE '%(?i:cbz|cbr|cb7)%';

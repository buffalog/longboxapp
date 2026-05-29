-- Bug 4: repair-then-constrain for the issue-number padding mismatch.
--
-- The catalog stores parser-padded issue numbers ("001") and CV
-- returns unpadded ("1"). The original UNIQUE(series_id, number)
-- uses raw string equality, so the two forms coexist as distinct
-- rows. Three biting code paths produce this state:
--   1. 6c.2 CV enrichment (shallow → CV-linked).
--   2. Bulk-convert link-mode dropping zero-padded folder files
--      onto an already-CV-linked series (verified historical:
--      Ferocious id=878).
--   3. add_or_get_from_cv onto a pre-existing shallow series.
--
-- The matcher's in-memory `IssueNumber::matches` already treats
-- "001" and "1" as equal — the gap is the DB-level uniqueness.
--
-- Strategy: add a STORED generated `canonical_number` column with
-- a pure CASE expression that strips leading zeros from purely-
-- numeric values (with or without fractional parts) and preserves
-- everything else verbatim. Then create UNIQUE(series_id,
-- canonical_number). The original `number` column stays untouched
-- so display, library paths, and ComicInfo `<Number>` keep their
-- current visual forms — only the conflict semantic changes.
--
-- The canonical_number expression was validated as a STORED
-- generated column before writing this migration. Test cases
-- (input → canonical):
--   "001"      → "1"        leading zero stripped
--   "1"        → "1"        idempotent
--   "100"      → "100"
--   "01.5"     → "1.5"      fraction preserved
--   "1.5"      → "1.5"
--   "Annual 1" → "Annual 1" preserved verbatim
--   "½"        → "½"        preserved verbatim
--   "v01"      → "v01"      preserved verbatim
--   "001a"     → "001a"     non-numeric → preserve verbatim
--
-- The pre-merge sweep (Step 1) is load-bearing: the UNIQUE index
-- creation in Step 3 fails if any padding-duplicate pair survives,
-- and the catalog DOES have at least one such pair (Ferocious)
-- predating 6c. The file-FK re-pointing inside the sweep is the
-- single most dangerous operation — if any file's issue_id is
-- left pointing at a deleted loser row, a real comic file is
-- orphaned. Guarded by a CHECK-constrained temp table that
-- aborts the whole migration transaction if pre/post file counts
-- per affected series diverge.
--
-- Survivor selection per padding-dup group:
--   1. Prefer the row with cv_issue_id set (the CV-canonical row).
--   2. Then earliest created_at.
--   3. Then lowest id.
-- The cv_issue_id preference is the rule even when files live on
-- the OTHER row in the group (Ferocious's exact divergence:
-- files on synthesized "001", cv_issue_id on TPB "1"). The sweep
-- re-points the loser's files onto the survivor before DELETE so
-- the file attachment survives the collapse.

-- ============================================================
-- Step 1: pre-merge sweep
-- ============================================================
--
-- Sub-step 1.a: identify padding-dup groups. Compute canonical
-- inline since the column doesn't exist yet — same CASE
-- expression that the generated column will use in Step 2.

CREATE TEMP TABLE _bug4_canonicalized AS
SELECT id, series_id, number, cv_issue_id, created_at,
    CASE
        WHEN number GLOB '[0-9]*' AND number NOT GLOB '*[^0-9]*'
            THEN CAST(CAST(number AS INTEGER) AS TEXT)
        WHEN number GLOB '[0-9]*.[0-9]*' AND number NOT GLOB '*[^0-9.]*'
            THEN CAST(CAST(substr(number, 1, instr(number, '.') - 1) AS INTEGER) AS TEXT)
                 || substr(number, instr(number, '.'))
        ELSE number
    END AS canonical
FROM issues;

CREATE TEMP TABLE _bug4_groups AS
SELECT series_id, canonical
FROM _bug4_canonicalized
GROUP BY series_id, canonical
HAVING COUNT(*) > 1;

-- Sub-step 1.b: choose a survivor per group.

CREATE TEMP TABLE _bug4_survivors AS
SELECT g.series_id, g.canonical,
    (SELECT n.id FROM _bug4_canonicalized n
     WHERE n.series_id = g.series_id AND n.canonical = g.canonical
     ORDER BY (n.cv_issue_id IS NULL) ASC, n.created_at ASC, n.id ASC
     LIMIT 1) AS survivor_id
FROM _bug4_groups g;

-- Sub-step 1.c: identify losers.

CREATE TEMP TABLE _bug4_losers AS
SELECT n.id AS loser_id, s.survivor_id, s.series_id
FROM _bug4_canonicalized n
JOIN _bug4_survivors s ON s.series_id = n.series_id
                       AND s.canonical = n.canonical
WHERE n.id != s.survivor_id;

-- Sub-step 1.d: snapshot pre-sweep file counts per affected
-- series. The invariant: this number must equal the post-sweep
-- count for the same series; if it doesn't, a file was orphaned.

CREATE TEMP TABLE _bug4_pre_file_counts AS
SELECT i.series_id, COUNT(f.id) AS file_count
FROM issues i
LEFT JOIN files f ON f.issue_id = i.id
WHERE i.series_id IN (SELECT series_id FROM _bug4_survivors)
GROUP BY i.series_id;

-- Sub-step 1.e: re-point files from losers onto survivors. This
-- MUST happen BEFORE the DELETE so file FKs survive.

UPDATE files
SET issue_id = (SELECT survivor_id FROM _bug4_losers WHERE loser_id = files.issue_id)
WHERE issue_id IN (SELECT loser_id FROM _bug4_losers);

-- Sub-step 1.f: delete losers. Files now point at survivors;
-- nothing should orphan.

DELETE FROM issues WHERE id IN (SELECT loser_id FROM _bug4_losers);

-- Sub-step 1.g: invariant lock. The CHECK constraint on this
-- temp table aborts the whole migration transaction if any row's
-- pre != post — i.e., if a file count changed for any affected
-- series, meaning a file was orphaned or duplicated by the
-- sweep.

CREATE TEMP TABLE _bug4_invariant_check (
    series_id INTEGER NOT NULL,
    pre INTEGER NOT NULL,
    post INTEGER NOT NULL,
    CHECK (pre = post)
);

INSERT INTO _bug4_invariant_check (series_id, pre, post)
SELECT pre.series_id, pre.file_count,
    (SELECT COUNT(f.id)
     FROM issues i LEFT JOIN files f ON f.issue_id = i.id
     WHERE i.series_id = pre.series_id) AS post_count
FROM _bug4_pre_file_counts pre;

-- Bonus invariant: no file should now have an issue_id pointing
-- at a deleted row.

INSERT INTO _bug4_invariant_check (series_id, pre, post)
SELECT 0, 0,
    (SELECT COUNT(*) FROM files f
     WHERE f.issue_id IS NOT NULL
       AND NOT EXISTS (SELECT 1 FROM issues i WHERE i.id = f.issue_id));
-- The above row inserts pre=0, post=<orphan_count>. CHECK (pre=post)
-- fires unless orphan_count==0, aborting the migration if even
-- one file lost its issue.

-- ============================================================
-- Step 2: add the canonical_number VIRTUAL generated column
-- ============================================================
--
-- SQLite restriction: ALTER TABLE ADD COLUMN supports VIRTUAL
-- generated columns but NOT STORED ones (per
-- https://www.sqlite.org/gencol.html). VIRTUAL recomputes the
-- value on each read — but for our use case (the UNIQUE INDEX in
-- Step 3 + ON CONFLICT clauses in the upserts), the index
-- materializes the computed values, so conflict detection stays
-- fast. The expression is deterministic and pure (same allowed
-- function set as STORED), so VIRTUAL works.

ALTER TABLE issues ADD COLUMN canonical_number TEXT
    GENERATED ALWAYS AS (
        CASE
            WHEN number GLOB '[0-9]*' AND number NOT GLOB '*[^0-9]*'
                THEN CAST(CAST(number AS INTEGER) AS TEXT)
            WHEN number GLOB '[0-9]*.[0-9]*' AND number NOT GLOB '*[^0-9.]*'
                THEN CAST(CAST(substr(number, 1, instr(number, '.') - 1) AS INTEGER) AS TEXT)
                     || substr(number, instr(number, '.'))
            ELSE number
        END
    ) VIRTUAL;

-- ============================================================
-- Step 3: UNIQUE index on (series_id, canonical_number)
-- ============================================================
--
-- This is the new defense-in-depth: every INSERT path
-- (enrichment, bulk-convert, refresh, add_or_get_from_cv, scanner)
-- now hits a UNIQUE conflict if it tries to write a padding-form
-- duplicate. The existing UNIQUE(series_id, number) constraint on
-- the CREATE TABLE stays — it's redundant but harmless, and
-- SQLite doesn't support ALTER TABLE DROP CONSTRAINT without a
-- full table rebuild.

CREATE UNIQUE INDEX idx_issues_series_canonical
    ON issues(series_id, canonical_number);

-- ============================================================
-- Cleanup
-- ============================================================
--
-- Temp tables drop automatically at transaction end. No explicit
-- DROP needed.

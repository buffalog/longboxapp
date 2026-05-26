-- A.9 Bug 2: clean up sort_title duplicates that span the NULL/year-set
-- boundary. The 20260523 migration partitioned by `(sort_title,
-- COALESCE(start_year, -1))`, so a row with year=NULL and a row with
-- year=2024 sharing the same sort_title were never merged — even when
-- they were the same series tracked under two folder names (the user
-- has both `Enfield Gang Massacre (2024)` and
-- `The Enfield Gang Massacre` on disk, each carrying different issues).
--
-- This migration partitions by sort_title ALONE but applies a safety
-- guard: only merge groups where AT MOST ONE row has a year set. If
-- multiple rows have different non-NULL years (e.g. 1964 + 2024
-- Daredevil reboots), skip the group — they're legitimately different
-- series. The same guard `find_for_dedup`'s phase-2 fallback applies
-- at convert time.
--
-- Survivor preference: cv_id-set first, then year-set, then earliest
-- created_at. Issues from non-survivors reassign to the survivor;
-- number collisions are skipped (left on the non-survivor, blocking
-- its deletion — surfaces as an anomaly the user can resolve).
-- Files follow their issues automatically.
--
-- Idempotent: re-running on a clean catalog matches no groups, does
-- nothing. The `>1 group_size` and `year_set_count <= 1` filters
-- both keep it safe against future re-runs.

-- Step 1: stage the merge plan in a temp table so the subsequent
-- UPDATEs and DELETE can reference it consistently.
CREATE TEMP TABLE _bug2_merge_plan AS
WITH ranked AS (
    SELECT s.id,
           s.sort_title,
           s.cv_id,
           s.start_year,
           s.created_at,
           ROW_NUMBER() OVER (
               PARTITION BY s.sort_title
               ORDER BY (s.cv_id IS NULL) ASC,
                        (s.start_year IS NULL) ASC,
                        s.created_at ASC
           ) AS rn,
           COUNT(*) OVER (PARTITION BY s.sort_title) AS group_size,
           SUM(CASE WHEN s.start_year IS NOT NULL THEN 1 ELSE 0 END)
               OVER (PARTITION BY s.sort_title) AS year_set_count
    FROM series s
)
SELECT
    survivor.id AS survivor_id,
    non_survivor.id AS non_survivor_id
FROM ranked survivor
JOIN ranked non_survivor ON survivor.sort_title = non_survivor.sort_title
WHERE survivor.rn = 1
  AND non_survivor.rn > 1
  AND survivor.group_size > 1
  AND survivor.year_set_count <= 1;

-- Step 2: reassign issues from non-survivors to survivors. Skip
-- issues whose `number` already exists on the survivor — leaving
-- them on the non-survivor blocks the non-survivor's deletion in
-- step 3, surfacing the collision as an anomaly rather than
-- silently merging.
UPDATE issues
SET series_id = (
    SELECT survivor_id
    FROM _bug2_merge_plan
    WHERE non_survivor_id = issues.series_id
)
WHERE series_id IN (SELECT non_survivor_id FROM _bug2_merge_plan)
  AND NOT EXISTS (
      SELECT 1
      FROM issues survivor_issue, _bug2_merge_plan mp
      WHERE mp.non_survivor_id = issues.series_id
        AND survivor_issue.series_id = mp.survivor_id
        AND survivor_issue.number = issues.number
  );

-- Step 3: delete non-survivor series rows that no longer have any
-- issues attached (i.e. all their issues reassigned successfully).
-- A non-survivor that still has collision-issues stays — the user
-- sees both rows in the catalog and can resolve the merge manually.
DELETE FROM series WHERE id IN (
    SELECT non_survivor_id FROM _bug2_merge_plan
    WHERE NOT EXISTS (
        SELECT 1 FROM issues
        WHERE issues.series_id = _bug2_merge_plan.non_survivor_id
    )
);

DROP TABLE _bug2_merge_plan;

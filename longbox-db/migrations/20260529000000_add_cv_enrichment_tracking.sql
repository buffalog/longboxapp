-- Phase A.9 Step 6c.1: shallow-series CV enrichment tracking.
--
-- The auto-pick worker (6c.2) writes attempt outcome + timestamp on
-- the series row so failed-or-refused attempts can be cooled-down
-- (default 7 days) without re-running CV searches that won't help.
-- A successful match (outcome='matched') stamps cv_id directly on
-- the series row via the existing schema, so the row drops out of
-- the worker's shallow-series queue naturally — no cooldown needed
-- for the success path.
--
-- `last_enrichment_outcome` is an open enum (TEXT, no CHECK
-- constraint) mirroring the precedent set by `files.match_method`
-- and `files.status`. Validated at the application layer in
-- `longbox-comicvine::enrichment::PickOutcome` + the worker's
-- post-merge classification. Values:
--   matched              auto-pick succeeded; cv_id assigned;
--                        issues merged via the row-id-preserving
--                        upsert_by_series_id_and_number_with_cv_fields.
--   partial_merge        match + merge succeeded, but ≥ 1 catalog
--                        issue number didn't appear in CV's issue
--                        list — the file's provenance is stranded
--                        from CV's canonical numbering. Surfaces
--                        in /library/tidy for review; not the
--                        same as a clean match (catalog-only
--                        numbers stay attached to the synthesized
--                        rows, but their provenance can't be
--                        ratified by CV).
--   no_results           CV search returned 0 candidates.
--   low_confidence       best candidate scored below the title
--                        threshold (0.85 year-known / 0.95
--                        year-unknown).
--   multi_match          two or more candidates above threshold;
--                        dominant-gap guard (0.20) refused.
--   year_mismatch        sole above-threshold survivor failed the
--                        catalog start_year gate.
--   count_mismatch       sole above-threshold survivor failed the
--                        issue-count window guard (±20% year-known
--                        / ±10% year-unknown).
--   collision_disabled   pre-filter refused: year-unknown catalog
--                        row has ≥ 2 same-titled siblings in the
--                        catalog. Never attempted CV. Routed
--                        straight to manual via /library/tidy.
--   cv_id_collision      auto-pick produced a cv_id that another
--                        catalog series already claims. Surfaces
--                        as the candidate-merge prompt connecting
--                        back to the matcher-disambiguation
--                        deferred item.
--   error                CV API or DB error during attempt;
--                        details captured in last_enrichment_error.
--
-- Wrong auto-link is asymmetrically worse than refusal: a wrong
-- link overwrites cv_id + title + cover_date + summary + cover_url
-- on every merged issue, after which the series looks confidently
-- correct while being wrong. A refusal sits visibly in /library/tidy
-- waiting for a human pick. Default thresholds below are the
-- conservative-to-a-fault choices from the 6c kickoff.

ALTER TABLE series ADD COLUMN last_enrichment_attempt_at TIMESTAMP;
ALTER TABLE series ADD COLUMN last_enrichment_outcome TEXT;
ALTER TABLE series ADD COLUMN last_enrichment_error TEXT;

-- Worker tunables. All read via `settings_repo::get_or_default`
-- with the type-appropriate defaults so flipping a value is a
-- next-cycle change with no restart needed.
INSERT INTO settings (key, value) VALUES
  ('cv_enrichment_title_threshold_year_known',   '0.85'),
  ('cv_enrichment_title_threshold_year_unknown', '0.95'),
  ('cv_enrichment_count_window_year_known',      '0.20'),
  ('cv_enrichment_count_window_year_unknown',    '0.10'),
  ('cv_enrichment_dominant_gap',                 '0.20'),
  ('cv_enrichment_cooldown_days',                '7'),
  ('cv_enrichment_request_interval_seconds',     '30'),
  -- Bounded-sample gate. First deploy stops after this many
  -- attempts so 6c.3 can observe the outcome distribution before
  -- the full backlog ships. Worker reads this per-cycle; setting
  -- to 0 disables the bound (releases full backlog).
  ('cv_enrichment_max_run',                      '20');

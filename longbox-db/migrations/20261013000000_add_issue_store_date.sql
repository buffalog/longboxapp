-- CV on-sale date (YYYY-MM-DD). cover_date runs 1-2 months ahead of
-- on-sale, so pull eligibility and the solicited/missing split key off
-- COALESCE(store_date, cover_date). Nullable: older/indie CV issues
-- have no store_date, and existing rows backfill on the first full
-- refresh.
ALTER TABLE issues ADD COLUMN store_date TEXT;

-- One definition of "does the catalog own this issue?".
--
-- Before this view the predicate was hand-written in a dozen queries
-- across four crates. Nothing was broken -- the copies agreed -- but
-- adding a third ownership state means editing every copy correctly,
-- and "edit all N copies correctly" is what failed twice in one week
-- here: three separator normalisers where only one knew about `_`, and
-- two digest-freshness rules where only one validated against disk.
-- Both shipped as silent failures.
--
-- `is_owned` is SQLite integer 0/1 and sqlx types it i64; call sites
-- compare `= 1` explicitly so the storage type stays visible.
--
-- Verified 2026-08-04 against a copy of the live catalog: SQLite
-- inlines this view completely, so the query plan is unchanged and the
-- covering index from 20260608000000_add_dashboard_stats_indexes.sql
-- is still used.
CREATE VIEW issue_ownership AS
SELECT i.id        AS issue_id,
       i.series_id AS series_id,
       EXISTS (SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1) AS is_owned
FROM issues i;

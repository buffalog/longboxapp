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
-- `is_owned` is SQLite integer 0/1 at runtime, but sqlx CANNOT infer a
-- type for it: its SQLite describe sees an `EXISTS (...)` expression,
-- reports type_info NULL, and lowers the column to `()`. Verified
-- 2026-08-04 -- selecting it bare fails to compile with
-- `unsupported type NULL of column ("is_owned")`.
--
-- Consequence for call sites: using it as a PREDICATE
-- (`NOT EXISTS (SELECT 1 FROM issue_ownership o WHERE ... o.is_owned = 1)`)
-- compiles unannotated, which is the shape every current call site uses.
-- SELECTING it requires an explicit override:
--     SELECT is_owned AS "is_owned!: i64" FROM issue_ownership
-- An earlier version of this comment claimed sqlx types it as i64. That
-- was asserted, not tested, and it is wrong.
--
-- Performance, measured rather than assumed, and NOT uniform:
--
--   * simple predicate use (`WHERE is_owned = 0`) -- SQLite flattens the
--     view entirely; plan and timing identical to the hand-written form
--     (1.07 ms either way on the live catalog).
--   * nested inside an aggregate (`COUNT(DISTINCT CASE WHEN NOT EXISTS
--     (...)`) -- NOT fully flattened. One extra rowid seek and a level
--     of subquery nesting appear. Measured at 20x live scale:
--     find_all_with_counts 36.4 ms -> 57.0 ms. At live scale ~2 -> ~3 ms.
--
-- The covering index from 20260608000000_add_dashboard_stats_indexes.sql
-- is still used in both shapes. An earlier version of this comment
-- claimed the plan was unchanged full stop; that generalised from the
-- simple case and was wrong.
CREATE VIEW issue_ownership AS
SELECT i.id        AS issue_id,
       i.series_id AS series_id,
       EXISTS (SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1) AS is_owned
FROM issues i;

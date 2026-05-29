use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesRow {
    pub id: i64,
    pub cv_id: Option<i64>,
    pub metron_id: Option<String>,
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_at: PrimitiveDateTime,
    pub updated_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewSeries {
    pub cv_id: Option<i64>,
    pub metron_id: Option<String>,
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

/// Fields that can be overwritten via `update`. `cv_id` and `metron_id` are
/// identity columns and not updatable here; use a new insert if you need a
/// different external ID.
#[derive(Debug, Clone)]
pub struct SeriesUpdate {
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Find an existing series matching `(sort_title, start_year)` for
/// the bulk-convert dedup path (A.9 hot-fix). Two-phase semantics:
///
/// **Phase 1 (strict):** NULL-safe equality on `start_year` via
/// SQLite's `IS`. Two folders both lacking `(YYYY)` and converted
/// under the same normalized title dedup against each other; two
/// folders with different year-set values are kept distinct (1980
/// Daredevil vs 2024 Daredevil reboot — same title, different
/// series).
///
/// **Phase 2 (NULL-incoming fallback, A.9 Bug 2):** when phase 1
/// returns nothing AND the incoming `start_year` is NULL, match
/// against any year-set row sharing the sort_title. Handles the
/// observed shape where the user has the same series under two
/// folder names — `Enfield Gang Massacre (2024)` and
/// `The Enfield Gang Massacre`. The "The "-stripped sort_title is
/// identical; the NULL-incoming convert links to the year-set
/// survivor instead of creating a dupe.
///
/// Phase 2 is **asymmetric on purpose**: a year-set incoming does
/// NOT fall back to a NULL-year row, because that risks linking a
/// reboot to a stale shallow row.
///
/// Phase 2 also has a **multi-match guard**: refuse the link if
/// more than one year-set row shares the sort_title (e.g. both
/// 1964 + 2024 Daredevil catalogued — can't pick confidently;
/// preference tiebreaker would just pick blindly). Ambiguous case
/// returns None; the convert creates a fresh shallow row and the
/// user resolves manually.
///
/// In steady state the new idempotency prevents duplicates, so at
/// most one row matches each phase. The `ORDER BY` is the survivor
/// rule for any stale pre-cleanup dupes: phase-1 wins over phase-2,
/// then cv_id-set first, then year-set first, then earliest
/// created_at.
pub async fn find_for_dedup<'e, E>(
    executor: E,
    sort_title: &str,
    start_year: Option<i32>,
) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    // Single CTE keeps the generic executor signature working for
    // both pool-based test callers and transaction-bound convert
    // callers. The `?2 IS NULL` guard inside `fallback_pool` is what
    // makes phase 2 asymmetric — it never fires for year-set incomings.
    let row = sqlx::query_as!(
        SeriesRow,
        r#"WITH strict AS (
               SELECT id, cv_id, metron_id, title, sort_title, start_year,
                      publisher, description, cover_url, created_at, updated_at
               FROM series
               WHERE sort_title = ?1 AND start_year IS ?2
           ),
           fallback_pool AS (
               SELECT id, cv_id, metron_id, title, sort_title, start_year,
                      publisher, description, cover_url, created_at, updated_at
               FROM series
               WHERE ?2 IS NULL
                 AND sort_title = ?1
                 AND start_year IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM strict)
           ),
           fallback AS (
               SELECT * FROM fallback_pool
               WHERE (SELECT COUNT(*) FROM fallback_pool) = 1
           ),
           combined AS (
               SELECT 1 AS phase, id, cv_id, metron_id, title, sort_title,
                      start_year, publisher, description, cover_url,
                      created_at, updated_at
               FROM strict
               UNION ALL
               SELECT 2 AS phase, id, cv_id, metron_id, title, sort_title,
                      start_year, publisher, description, cover_url,
                      created_at, updated_at
               FROM fallback
           )
           SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM combined
           ORDER BY phase ASC,
                    (cv_id IS NULL) ASC,
                    (start_year IS NULL) ASC,
                    created_at ASC
           LIMIT 1"#,
        sort_title,
        start_year
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn find_by_cv_id<'e, E>(executor: E, cv_id: i64) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series WHERE cv_id = ?"#,
        cv_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Promote a shallow series to CV-linked (Step 6c.2). Single-purpose;
/// the existing [`SeriesUpdate`] excludes `cv_id` as immutable. This
/// is the one sanctioned path to assign cv_id post-creation.
///
/// Defensive predicate: `WHERE id = ? AND cv_id IS NULL`. The
/// predicate is *not* redundant with the worker's queue filter — it's
/// a race guard. Two workers attempting the same series, or a worker
/// racing a manual user link via the UI, both resolve correctly:
/// whichever's UPDATE lands first wins; the second writer's UPDATE
/// matches zero rows. The DB-level `UNIQUE(cv_id)` constraint is the
/// backstop against two different series receiving the same cv_id.
///
/// Returns the number of rows updated. **The caller MUST treat
/// `rows_affected == 0` as a real outcome** ("someone else claimed
/// this series first") and log it explicitly — not silently succeed.
pub async fn set_cv_id<'e, E>(executor: E, series_id: i64, cv_id: i64) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET cv_id = ?, updated_at = CURRENT_TIMESTAMP
           WHERE id = ? AND cv_id IS NULL"#,
        cv_id,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn find_by_metron_id<'e, E>(executor: E, metron_id: &str) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series WHERE metron_id = ?"#,
        metron_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Series row plus the per-status issue counts the web layer's series list
/// surface needs. Single JOIN'd query — no N+1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesWithCounts {
    #[serde(flatten)]
    pub series: SeriesRow,
    pub total_count: i64,
    pub owned_count: i64,
    pub needs_review_count: i64,
    pub ignored_count: i64,
    pub unmatched_count: i64,
    pub missing_count: i64,
}

/// Like [`find_all`] but augments each row with the per-status issue
/// counts. `missing_count` is "issues for which no present owned file
/// exists" — not derivable from `total_count - owned_count`, because that
/// conflates with needs_review and ignored.
pub async fn find_all_with_counts<'e, E>(executor: E) -> Result<Vec<SeriesWithCounts>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT
             s.id AS "id!: i64",
             s.cv_id, s.metron_id, s.title, s.sort_title, s.start_year,
             s.publisher, s.description, s.cover_url,
             s.created_at AS "created_at: time::PrimitiveDateTime",
             s.updated_at AS "updated_at: time::PrimitiveDateTime",
             COUNT(DISTINCT i.id) AS "total_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'owned' AND f.is_present = 1
               THEN i.id END) AS "owned_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'needs_review' AND f.is_present = 1
               THEN i.id END) AS "needs_review_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'ignored' AND f.is_present = 1
               THEN i.id END) AS "ignored_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'unmatched' AND f.is_present = 1
               THEN i.id END) AS "unmatched_count!: i64",
             COUNT(DISTINCT CASE
               WHEN NOT EXISTS (
                 SELECT 1 FROM files f2
                 WHERE f2.issue_id = i.id
                   AND f2.status = 'owned'
                   AND f2.is_present = 1
               )
               THEN i.id END) AS "missing_count!: i64"
           FROM series s
           LEFT JOIN issues i ON i.series_id = s.id
           LEFT JOIN files f ON f.issue_id = i.id
           GROUP BY s.id
           ORDER BY s.sort_title"#
    )
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesWithCounts {
            series: SeriesRow {
                id: r.id,
                cv_id: r.cv_id,
                metron_id: r.metron_id,
                title: r.title,
                sort_title: r.sort_title,
                start_year: r.start_year,
                publisher: r.publisher,
                description: r.description,
                cover_url: r.cover_url,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
            total_count: r.total_count,
            owned_count: r.owned_count,
            needs_review_count: r.needs_review_count,
            ignored_count: r.ignored_count,
            unmatched_count: r.unmatched_count,
            missing_count: r.missing_count,
        })
        .collect())
}

/// Like [`find_all_with_counts`] but newest-first by `created_at` and
/// limited. Used by the dashboard activity feed's "Recently added series"
/// section.
pub async fn list_recent_with_counts<'e, E>(
    executor: E,
    limit: u32,
) -> Result<Vec<SeriesWithCounts>>
where
    E: SqliteExecutor<'e>,
{
    let limit_i64 = i64::from(limit);
    let rows = sqlx::query!(
        r#"SELECT
             s.id AS "id!: i64",
             s.cv_id, s.metron_id, s.title, s.sort_title, s.start_year,
             s.publisher, s.description, s.cover_url,
             s.created_at AS "created_at: time::PrimitiveDateTime",
             s.updated_at AS "updated_at: time::PrimitiveDateTime",
             COUNT(DISTINCT i.id) AS "total_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'owned' AND f.is_present = 1
               THEN i.id END) AS "owned_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'needs_review' AND f.is_present = 1
               THEN i.id END) AS "needs_review_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'ignored' AND f.is_present = 1
               THEN i.id END) AS "ignored_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'unmatched' AND f.is_present = 1
               THEN i.id END) AS "unmatched_count!: i64",
             COUNT(DISTINCT CASE
               WHEN NOT EXISTS (
                 SELECT 1 FROM files f2
                 WHERE f2.issue_id = i.id
                   AND f2.status = 'owned'
                   AND f2.is_present = 1
               )
               THEN i.id END) AS "missing_count!: i64"
           FROM series s
           LEFT JOIN issues i ON i.series_id = s.id
           LEFT JOIN files f ON f.issue_id = i.id
           GROUP BY s.id
           ORDER BY s.created_at DESC, s.id DESC
           LIMIT ?"#,
        limit_i64
    )
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesWithCounts {
            series: SeriesRow {
                id: r.id,
                cv_id: r.cv_id,
                metron_id: r.metron_id,
                title: r.title,
                sort_title: r.sort_title,
                start_year: r.start_year,
                publisher: r.publisher,
                description: r.description,
                cover_url: r.cover_url,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
            total_count: r.total_count,
            owned_count: r.owned_count,
            needs_review_count: r.needs_review_count,
            ignored_count: r.ignored_count,
            unmatched_count: r.unmatched_count,
            missing_count: r.missing_count,
        })
        .collect())
}

pub async fn find_all<'e, E>(executor: E) -> Result<Vec<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series ORDER BY sort_title"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewSeries) -> Result<SeriesRow>
where
    E: SqliteExecutor<'e>,
{
    let start_year = input.start_year.map(i64::from);
    let row = sqlx::query_as!(
        SeriesRow,
        r#"INSERT INTO series (cv_id, metron_id, title, sort_title, start_year,
                               publisher, description, cover_url)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", cv_id, metron_id, title, sort_title,
                     start_year, publisher, description, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        input.cv_id,
        input.metron_id,
        input.title,
        input.sort_title,
        start_year,
        input.publisher,
        input.description,
        input.cover_url
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(executor: E, id: i64, patch: SeriesUpdate) -> Result<SeriesRow>
where
    E: SqliteExecutor<'e>,
{
    let start_year = patch.start_year.map(i64::from);
    let row = sqlx::query_as!(
        SeriesRow,
        r#"UPDATE series
           SET title = ?, sort_title = ?, start_year = ?,
               publisher = ?, description = ?, cover_url = ?,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?
           RETURNING id AS "id!: i64", cv_id, metron_id, title, sort_title,
                     start_year, publisher, description, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        patch.title,
        patch.sort_title,
        start_year,
        patch.publisher,
        patch.description,
        patch.cover_url,
        id
    )
    .fetch_optional(executor)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(row)
}

/// A zero-owned series — a "phantom" — carrying its last-scan matched
/// count. `last_matched_count > 0` marks a *transition* phantom (the
/// series held files at the last scan and has since lost them all);
/// the transition-vs-steady-state partition is the route's job.
///
/// Deliberately a narrow struct rather than a `SeriesRow` field — the
/// new column stays out of the widely-used row type and its many
/// SELECTs.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PhantomSeries {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i64>,
    pub publisher: Option<String>,
    pub cover_url: Option<String>,
    pub last_matched_count: i64,
    /// True when the series is empty *by intent* — it is on the pull
    /// list or has a pull attempt, i.e. awaiting a first download
    /// rather than having lost files. Auto-tidy never marks such a
    /// series; `/library/tidy` renders it in its own subsection.
    pub awaiting_first_download: bool,
    /// Recovery deadline when the series has been marked for automatic
    /// removal; `None` when unmarked.
    pub auto_tidy_due_at: Option<PrimitiveDateTime>,
}

/// Every series with no owned, present file — the phantom set. The
/// reconciliation route partitions the result on `last_matched_count`
/// for its transition vs steady-state surfaces.
pub async fn list_phantoms<'e, E>(executor: E) -> Result<Vec<PhantomSeries>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        PhantomSeries,
        r#"SELECT s.id AS "id!: i64", s.title, s.sort_title,
                  s.start_year, s.publisher, s.cover_url,
                  s.last_matched_count AS "last_matched_count!: i64",
                  (EXISTS (SELECT 1 FROM pull_list pl WHERE pl.series_id = s.id)
                   OR EXISTS (SELECT 1 FROM pull_attempts pa WHERE pa.series_id = s.id))
                      AS "awaiting_first_download!: bool",
                  s.auto_tidy_due_at AS "auto_tidy_due_at: _"
           FROM series s
           WHERE NOT EXISTS (
               SELECT 1 FROM files f
               JOIN issues i ON f.issue_id = i.id
               WHERE i.series_id = s.id
                 AND f.status = 'owned'
                 AND f.is_present = 1
           )
           ORDER BY s.sort_title COLLATE NOCASE"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Set one series' `last_matched_count` directly. Used by the "Keep"
/// reconciliation action (Step 5) to reset a transition phantom to 0,
/// dropping it to the steady-state list. The scanner refreshes counts
/// in bulk via [`refresh_last_matched_counts`] instead.
pub async fn update_last_matched_count<'e, E>(executor: E, series_id: i64, count: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series SET last_matched_count = ? WHERE id = ?"#,
        count,
        series_id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Refresh every series' `last_matched_count` to its current owned,
/// present file count — the scanner calls this once at the end of a
/// full scan. Returns the number of series whose count was refreshed.
///
/// Series whose current count is zero are deliberately **skipped** (the
/// `WHERE` clause): leaving `last_matched_count` at its last non-zero
/// value is exactly what marks a series that just lost all its files as
/// a *transition* phantom rather than steady-state. A series that has
/// been zero-owned all along stays at 0.
///
/// The owned-AND-present predicate is identical to the one
/// [`list_phantoms`] uses for an "owned file", so `last_matched_count`
/// and the phantom check measure the same thing — the route's
/// transition partition (zero owned now AND `last_matched_count > 0`)
/// is a coherent comparison.
pub async fn refresh_last_matched_counts<'e, E>(executor: E) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series SET last_matched_count = (
               SELECT COUNT(*) FROM files f
               JOIN issues i ON f.issue_id = i.id
               WHERE i.series_id = series.id
                 AND f.status = 'owned' AND f.is_present = 1
           )
           WHERE (
               SELECT COUNT(*) FROM files f
               JOIN issues i ON f.issue_id = i.id
               WHERE i.series_id = series.id
                 AND f.status = 'owned' AND f.is_present = 1
           ) > 0"#
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Scan-end tick of every series' `consecutive_empty_scans` (A.9 Step
/// 6b). A series with no owned, present file has its counter
/// incremented; a series that owns at least one such file is reset to 0
/// **and** has any `auto_tidy_due_at` mark cleared — the auto-recovery
/// path for a folder that came back on disk. Run once per full scan,
/// after the mark-missing pass and `refresh_last_matched_counts`.
pub async fn tick_empty_scan_counters<'e, E>(executor: E) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE series SET
               consecutive_empty_scans = CASE WHEN EXISTS (
                   SELECT 1 FROM files f JOIN issues i ON f.issue_id = i.id
                   WHERE i.series_id = series.id
                     AND f.status = 'owned' AND f.is_present = 1
               ) THEN 0 ELSE consecutive_empty_scans + 1 END,
               auto_tidy_due_at = CASE WHEN EXISTS (
                   SELECT 1 FROM files f JOIN issues i ON f.issue_id = i.id
                   WHERE i.series_id = series.id
                     AND f.status = 'owned' AND f.is_present = 1
               ) THEN NULL ELSE auto_tidy_due_at END"#
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Mark every series eligible for automatic removal: empty for at least
/// `threshold` consecutive scans, not already marked, and not awaiting a
/// first download (no `pull_list` entry, no `pull_attempts` row — the
/// pull list is an explicit "I want this" signal auto-tidy must never
/// override). `due_at` is the recovery deadline written to the
/// newly-marked rows. Returns the count of series newly marked. The
/// caller gates this call on the `auto_tidy_enabled` setting.
pub async fn mark_for_auto_tidy<'e, E>(
    executor: E,
    threshold: i64,
    due_at: PrimitiveDateTime,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series SET auto_tidy_due_at = ?
           WHERE auto_tidy_due_at IS NULL
             AND consecutive_empty_scans >= ?
             AND NOT EXISTS (
                 SELECT 1 FROM files f JOIN issues i ON f.issue_id = i.id
                 WHERE i.series_id = series.id
                   AND f.status = 'owned' AND f.is_present = 1
             )
             AND NOT EXISTS (
                 SELECT 1 FROM pull_list pl WHERE pl.series_id = series.id
             )
             AND NOT EXISTS (
                 SELECT 1 FROM pull_attempts pa WHERE pa.series_id = series.id
             )"#,
        due_at,
        threshold
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Hard-delete every series whose auto-tidy recovery window has elapsed
/// (`auto_tidy_due_at <= now`). Dependent `issues`/`files` rows cascade.
/// Safe to run unguarded: [`tick_empty_scan_counters`] runs first in the
/// scan and clears `auto_tidy_due_at` on any series that regained files,
/// so every still-marked series is confirmed empty as of this scan.
/// Returns the count of series purged.
pub async fn purge_due_auto_tidy<'e, E>(executor: E, now: PrimitiveDateTime) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"DELETE FROM series
           WHERE auto_tidy_due_at IS NOT NULL AND auto_tidy_due_at <= ?"#,
        now
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// One shallow-series candidate for the enrichment worker (Step
/// 6c.2). The `catalog_title_collision` flag is computed in the
/// candidate query via a `sort_title` aggregate so [`pick_volume`]
/// gets it without a second round-trip — the pure function stays
/// pure (it receives the flag, doesn't compute it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShallowEnrichmentCandidate {
    pub series_id: i64,
    pub title: String,
    pub start_year: Option<i64>,
    pub issue_count: i64,
    pub owned_count: i64,
    /// `true` when ≥ 2 catalog rows share this series's normalized
    /// `sort_title`. The enrichment auto-pick disables CV attempts
    /// when this is `true` *and* `start_year IS NULL` (year-known
    /// collision is disambiguated by the year gate; year-unknown
    /// collision is the highest false-positive surface).
    pub catalog_title_collision: bool,
}

/// Sub-population modes for the bounded-sample first-run gate
/// (6c.3). The worker reads `cv_enrichment_first_run_sample_mode`
/// per cycle; when `true`, the candidate query enforces the
/// deliberate three-bucket mix (highest-owned year-known
/// title-unique + year-unknown title-unique + collision-disabled)
/// so 6c.3's outcome distribution exercises both auto-link and
/// refusal paths, not just the easy path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSelectionMode {
    /// First-run sample composition. The query returns at most:
    /// - `easy_quota` highest-owned year-known title-unique
    /// - `discipline_quota` year-unknown title-unique
    /// - `collision_quota` collision-disabled (year-unknown + ≥ 2
    ///   catalog siblings) — included as explicit positive records
    ///   so 6c.3's bucketed report shows "N: collision_disabled" as
    ///   a signal the pre-filter ran, not as absence.
    FirstRunSample {
        easy_quota: i64,
        discipline_quota: i64,
        collision_quota: i64,
    },
    /// Steady-state: plain priority order (owned_count DESC,
    /// last_matched_count DESC, id ASC), bounded only by the
    /// caller's `limit`.
    PriorityOrder,
}

/// List shallow-series candidates for the next enrichment cycle.
/// Filters:
/// - `cv_id IS NULL` (only shallow)
/// - cooldown window: `last_enrichment_attempt_at IS NULL OR < now
///   - cooldown_days`
///
/// Sort + grouping depends on [`CandidateSelectionMode`]. The
/// `catalog_title_collision` flag is computed in-query via a
/// `sort_title`-grouping subquery so each candidate carries the
/// correct flag and the worker doesn't have to fan out per-series.
pub async fn list_shallow_for_enrichment<'e, E>(
    executor: E,
    cooldown_days: i64,
    mode: CandidateSelectionMode,
) -> Result<Vec<ShallowEnrichmentCandidate>>
where
    E: SqliteExecutor<'e>,
{
    let cooldown = format!("-{cooldown_days} days");
    match mode {
        CandidateSelectionMode::PriorityOrder => {
            let rows = sqlx::query_as!(
                ShallowEnrichmentCandidate,
                r#"
                WITH title_collision AS (
                    SELECT sort_title FROM series
                    GROUP BY sort_title HAVING COUNT(*) >= 2
                ),
                owned_counts AS (
                    SELECT i.series_id, COUNT(DISTINCT i.id) AS owned_count
                    FROM issues i
                    JOIN files f ON f.issue_id = i.id
                    WHERE f.status = 'owned' AND f.is_present = 1
                    GROUP BY i.series_id
                ),
                issue_counts AS (
                    SELECT series_id, COUNT(*) AS issue_count
                    FROM issues GROUP BY series_id
                )
                SELECT
                    s.id AS "series_id!: i64",
                    s.title AS "title!: String",
                    s.start_year,
                    COALESCE(ic.issue_count, 0) AS "issue_count!: i64",
                    COALESCE(oc.owned_count, 0) AS "owned_count!: i64",
                    CASE
                        WHEN tc.sort_title IS NOT NULL THEN 1
                        ELSE 0
                    END AS "catalog_title_collision!: bool"
                FROM series s
                LEFT JOIN owned_counts oc ON oc.series_id = s.id
                LEFT JOIN issue_counts ic ON ic.series_id = s.id
                LEFT JOIN title_collision tc ON tc.sort_title = s.sort_title
                WHERE s.cv_id IS NULL
                  AND (
                      s.last_enrichment_attempt_at IS NULL
                      OR s.last_enrichment_attempt_at < datetime('now', ?)
                  )
                ORDER BY owned_count DESC, s.last_matched_count DESC, s.id ASC
                "#,
                cooldown,
            )
            .fetch_all(executor)
            .await?;
            Ok(rows)
        }
        CandidateSelectionMode::FirstRunSample {
            easy_quota,
            discipline_quota,
            collision_quota,
        } => {
            // The first-run query is three unioned slices, each
            // already ordered + limited. The worker treats the
            // combined Vec as its cycle's candidates.
            let rows = sqlx::query_as!(
                ShallowEnrichmentCandidate,
                r#"
                WITH title_collision AS (
                    SELECT sort_title FROM series
                    GROUP BY sort_title HAVING COUNT(*) >= 2
                ),
                owned_counts AS (
                    SELECT i.series_id, COUNT(DISTINCT i.id) AS owned_count
                    FROM issues i
                    JOIN files f ON f.issue_id = i.id
                    WHERE f.status = 'owned' AND f.is_present = 1
                    GROUP BY i.series_id
                ),
                issue_counts AS (
                    SELECT series_id, COUNT(*) AS issue_count
                    FROM issues GROUP BY series_id
                ),
                eligible AS (
                    SELECT
                        s.id AS series_id,
                        s.title AS title,
                        s.start_year,
                        COALESCE(ic.issue_count, 0) AS issue_count,
                        COALESCE(oc.owned_count, 0) AS owned_count,
                        CASE WHEN tc.sort_title IS NOT NULL THEN 1 ELSE 0 END AS collision_flag,
                        s.last_matched_count AS lmc
                    FROM series s
                    LEFT JOIN owned_counts oc ON oc.series_id = s.id
                    LEFT JOIN issue_counts ic ON ic.series_id = s.id
                    LEFT JOIN title_collision tc ON tc.sort_title = s.sort_title
                    WHERE s.cv_id IS NULL
                      AND (
                          s.last_enrichment_attempt_at IS NULL
                          OR s.last_enrichment_attempt_at < datetime('now', ?1)
                      )
                ),
                easy_bucket AS (
                    SELECT * FROM eligible
                    WHERE start_year IS NOT NULL AND collision_flag = 0
                    ORDER BY owned_count DESC, lmc DESC, series_id ASC
                    LIMIT ?2
                ),
                discipline_bucket AS (
                    SELECT * FROM eligible
                    WHERE start_year IS NULL AND collision_flag = 0
                    ORDER BY owned_count DESC, lmc DESC, series_id ASC
                    LIMIT ?3
                ),
                collision_bucket AS (
                    SELECT * FROM eligible
                    WHERE collision_flag = 1
                    ORDER BY owned_count DESC, lmc DESC, series_id ASC
                    LIMIT ?4
                )
                SELECT
                    series_id AS "series_id!: i64",
                    title AS "title!: String",
                    start_year,
                    issue_count AS "issue_count!: i64",
                    owned_count AS "owned_count!: i64",
                    collision_flag AS "catalog_title_collision!: bool"
                FROM (
                    SELECT * FROM easy_bucket
                    UNION ALL
                    SELECT * FROM discipline_bucket
                    UNION ALL
                    SELECT * FROM collision_bucket
                )
                ORDER BY owned_count DESC, lmc DESC, series_id ASC
                "#,
                cooldown,
                easy_quota,
                discipline_quota,
                collision_quota,
            )
            .fetch_all(executor)
            .await?;
            Ok(rows)
        }
    }
}

/// Whether the three 6c.1 enrichment-tracking columns exist on
/// `series`. The enrichment worker calls this at startup; missing
/// columns trip the loud-fail-at-boot defense around the deferred
/// migration-not-applied-on-startup gremlin. Returns the missing
/// columns sorted (empty vec = schema OK).
pub async fn enrichment_schema_check<'e, E>(executor: E) -> Result<Vec<String>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(r#"PRAGMA table_info(series)"#)
        .fetch_all(executor)
        .await?;
    let present: std::collections::HashSet<String> = rows.into_iter().map(|r| r.name).collect();
    let required = [
        "last_enrichment_attempt_at",
        "last_enrichment_outcome",
        "last_enrichment_error",
    ];
    let missing: Vec<String> = required
        .iter()
        .filter(|c| !present.contains(**c))
        .map(|c| (*c).to_string())
        .collect();
    Ok(missing)
}

/// List `(series_id, number)` of issues still synthesized
/// (`cv_issue_id IS NULL`) for a given series. Called *inside* the
/// enrichment merge transaction, *after* the per-CV-issue upserts.
/// At that point CV-numbered rows that just got matched-and-updated
/// by number already have `cv_issue_id` set and drop out of this
/// filter — so what comes back is genuinely the orphan set (catalog
/// numbers CV's issue list didn't cover). Order is load-bearing in
/// the worker; pre-upsert this would count every synthesized row
/// as orphan.
pub async fn list_orphan_synthesized_numbers<'e, E>(
    executor: E,
    series_id: i64,
) -> Result<Vec<String>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT number AS "number!: String"
           FROM issues
           WHERE series_id = ? AND cv_issue_id IS NULL
           ORDER BY id ASC"#,
        series_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| r.number).collect())
}

/// Record the result of an enrichment attempt on a shallow series.
/// Called inside the per-attempt transaction so the outcome write
/// rolls back atomically with the merge writes — the attempt
/// timestamp staying unset on rollback is the proof the
/// transaction boundary holds.
pub async fn record_enrichment_outcome<'e, E>(
    executor: E,
    series_id: i64,
    outcome: &str,
    error_or_diagnostic: Option<&str>,
) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE series
           SET last_enrichment_attempt_at = CURRENT_TIMESTAMP,
               last_enrichment_outcome = ?,
               last_enrichment_error = ?,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        outcome,
        error_or_diagnostic,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// "Keep" a phantom: clear every auto-tidy signal in one shot —
/// `last_matched_count` to 0 (demotes a transition phantom to
/// steady-state), `consecutive_empty_scans` to 0, and `auto_tidy_due_at`
/// to NULL (cancels a scheduled removal). The user has reviewed the
/// series and decided it stays. `NotFound` for an unknown id.
pub async fn keep_phantom_series<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET last_matched_count = 0,
               consecutive_empty_scans = 0,
               auto_tidy_due_at = NULL
           WHERE id = ?"#,
        series_id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

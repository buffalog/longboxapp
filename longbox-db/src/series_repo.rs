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

/// Map of `cv_id -> local series id` for every CV-linked series. Used by
/// Add-search to flag results already in the library without a per-result
/// query (search returns up to ~100 rows; one scan of the CV-linked set is
/// cheaper and simpler than dynamic `IN (...)` SQL). A personal library is
/// small enough that fetching all owned cv_ids is trivial.
pub async fn owned_cv_ids<'e, E>(executor: E) -> Result<std::collections::HashMap<i64, i64>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", cv_id AS "cv_id!: i64"
           FROM series WHERE cv_id IS NOT NULL"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.cv_id, r.id)).collect())
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

/// Race-guarded write of `series.metron_id`. Idempotent: re-writing the
/// same value is a clean no-op (matches `WHERE id = ?` AND the value
/// is either NULL or already what we're setting). Concurrent writes of
/// DIFFERENT values are refused — the existing value stays.
///
/// Returns rows-affected: 1 = wrote, 0 = no change. **The caller must
/// treat 0 as "nothing to do, not an error"** — used by the Option C
/// subscription path's best-effort lazy backfill, where a zero-row
/// outcome means either a concurrent resolution already wrote the id
/// (fine) or the series_id no longer exists (shouldn't happen but is
/// harmless). Either case is logged at warn and not surfaced.
pub async fn set_metron_id<'e, E>(executor: E, series_id: i64, metron_id: &str) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET metron_id = ?, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?
             AND (metron_id IS NULL OR metron_id = ?)"#,
        metron_id,
        series_id,
        metron_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Set `series.finished` (Metron Completed | Cancelled → true). Returns
/// rows-affected; 0 means the series row no longer exists. Idempotent —
/// safe to re-run with the same value.
pub async fn set_finished<'e, E>(executor: E, series_id: i64, finished: bool) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET finished = ?, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        finished,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// The finished-state enrichment work-list (GH #7): series that have a
/// `metron_id` but aren't yet marked finished. Returns `(series_id,
/// metron_id)`. Re-running the enrichment is cheap because already-
/// finished series drop out of this set.
pub async fn list_metron_linked_unfinished<'e, E>(executor: E) -> Result<Vec<(i64, String)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", metron_id AS "metron_id!: String"
           FROM series
           WHERE metron_id IS NOT NULL AND finished = 0
           ORDER BY id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.metron_id)).collect())
}

/// Count of catalog series with no `metron_id` — these can't be enriched
/// for finished-state (the enrichment endpoint reports it as `skipped`,
/// the coverage gap the spec accepts: CV-only series stay non-purple
/// until their `metron_id` is backfilled).
pub async fn count_without_metron_id<'e, E>(executor: E) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(r#"SELECT COUNT(*) AS "n!: i64" FROM series WHERE metron_id IS NULL"#)
        .fetch_one(executor)
        .await?;
    Ok(row.n)
}

/// Persist the descriptive volume detail (`publisher`, `description`,
/// `cover_url`) from a fetched `CvVolumeDetail` onto an existing
/// series row. Separate from [`set_cv_id`] on purpose: the latter
/// establishes CV identity (the race-guarded identity column), this
/// writes the descriptive payload. Reused by the enrichment merge
/// (inside the per-attempt transaction so the three fields roll back
/// atomically with `set_cv_id` + per-issue upserts) and the volume-
/// refresh pass (standalone update for already-linked series whose
/// publisher was previously discarded).
///
/// Returns rows-affected. 0 means the series row no longer exists
/// (deleted between fetch and write) — caller's choice whether to
/// treat that as a real outcome or absorb silently.
pub async fn update_series_volume_detail<'e, E>(
    executor: E,
    series_id: i64,
    publisher: Option<&str>,
    description: Option<&str>,
    cover_url: Option<&str>,
    aliases: Option<&str>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET publisher = ?, description = ?, cover_url = ?, aliases = ?,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        publisher,
        description,
        cover_url,
        aliases,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Minimum alias length (chars) we'll feed the search/match path. CV aliases
/// are arbitrary user data; a 1-2 char or empty alias is junk that, once it
/// reaches the alias-token strip in `filter_by_series_title`, could erode a
/// legitimate token and manufacture a false match. Drop them at the source —
/// the match-layer issue/year gates only narrow that window, they don't close
/// it. A real generic-but-longer alias colliding remains theoretically
/// possible and is bounded by those gates; widen this floor only if a real
/// false positive surfaces (deterministic-list philosophy: extend on evidence).
const MIN_ALIAS_LEN: usize = 3;

/// Read a series' aliases as a split, trimmed, sanitized list. CV stores them
/// newline-separated; `None`/empty column → empty vec. Entries shorter than
/// `MIN_ALIAS_LEN` are dropped (junk guard, see above). Used only by the pull
/// search path (kept off `SeriesRow` to avoid widening every series SELECT).
pub async fn get_aliases<'e, E>(executor: E, series_id: i64) -> Result<Vec<String>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(r#"SELECT aliases FROM series WHERE id = ?"#, series_id)
        .fetch_optional(executor)
        .await?;
    let raw = row.and_then(|r| r.aliases).unwrap_or_default();
    Ok(raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.chars().count() >= MIN_ALIAS_LEN)
        .collect())
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
///
/// Issue-status taxonomy (all **derived** — there is no `status` column on
/// `issues`; an issue's state is computed from its files + `cover_date`):
///   - **Available** = `owned_count`            (has an owned, present file)
///   - **Solicited** = `solicited_count`        (no owned file AND
///     `cover_date >= today` — future)
///   - **Missing**   = `(total - solicited) - owned`  (released but not owned;
///     a derived render-time state)
///     The series-card badge denominator is `available = total - solicited`.
///     "Solicited" keys off `cover_date` (the only date stored per issue), not
///     the on-sale `store_date` — see `SeriesCard.svelte` for the caveat.
///
/// There is no "finished/ended" signal here yet: ComicVine carries none,
/// and the Metron `status` (Completed | Cancelled) that would unlock the
/// card's purple "complete collection" state is not yet fetched/stored —
/// see GH #7. The badge wires the purple branch but gates it on a
/// `finished` flag that stays false until that lands.
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
    /// Subset of `missing_count`: issues with no present owned file whose
    /// `cover_date` is today or later — i.e. solicited / pre-release. Lets
    /// the UI treat "owns everything that has shipped" as a green complete
    /// state rather than an orange deficit.
    pub solicited_count: i64,
    /// Whether the run is marked finished (Metron `status` = Completed |
    /// Cancelled). Drives the purple complete-collection badge. Always
    /// false for CV-only series and until the finished-state enrichment
    /// runs — populated only via the Metron detail path (GH #7).
    pub finished: bool,
}

/// Like [`find_all`] but augments each row with the per-status issue
/// counts. `missing_count` is "issues for which no present owned file
/// exists" — not derivable from `total_count - owned_count`, because that
/// conflates with needs_review and ignored.
/// Note on the ownership predicates inside: the `missing_count` and
/// `solicited_count` columns go through the `issue_ownership` view, but
/// `owned_count` deliberately does NOT. `owned_count` is one arm of a
/// per-STATUS breakdown (it sits beside needs_review/ignored/unmatched)
/// and the view answers only a boolean "is it owned" -- routing it
/// through the view would collapse the breakdown into a different
/// question. Do not "finish the job" by converting it.
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
             s.finished AS "finished!: bool",
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
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
               )
               THEN i.id END) AS "missing_count!: i64",
             COUNT(DISTINCT CASE
               WHEN NOT EXISTS (
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
               )
               AND i.cover_date IS NOT NULL
               AND i.cover_date >= date('now')
               THEN i.id END) AS "solicited_count!: i64"
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
            solicited_count: r.solicited_count,
            finished: r.finished,
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
             s.finished AS "finished!: bool",
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
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
               )
               THEN i.id END) AS "missing_count!: i64",
             COUNT(DISTINCT CASE
               WHEN NOT EXISTS (
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
               )
               AND i.cover_date IS NOT NULL
               AND i.cover_date >= date('now')
               THEN i.id END) AS "solicited_count!: i64"
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
            solicited_count: r.solicited_count,
            finished: r.finished,
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

/// Every series with no present file in any catalog-linked status — the
/// phantom set. The predicate is "no files on disk associated with this
/// series", not "no owned files": a series with only `needs_review` or
/// `unmatched` files still has bytes on disk and is not phantom. Ignored
/// files don't count (user-suppressed; treated as absent for tidy
/// purposes). The reconciliation route partitions the result on
/// `last_matched_count` for its transition vs steady-state surfaces.
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
           WHERE s.tidy_exempt = 0
             AND NOT EXISTS (
               SELECT 1 FROM files f
               JOIN issues i ON f.issue_id = i.id
               WHERE i.series_id = s.id
                 AND f.status IN ('owned', 'needs_review', 'unmatched')
                 AND f.is_present = 1
           )
           ORDER BY s.sort_title COLLATE NOCASE"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// The "Kept series" set: phantoms the user explicitly exempted from
/// tidy via [`keep_phantom_series`] (`tidy_exempt = 1`). The complement
/// of [`list_phantoms`] on the exemption flag — same shape, so the UI
/// can render it with the same row component. These are file-less by
/// definition (a kept series that regains files just stays exempt and
/// invisible to tidy, which is fine). Library Tidy renders this in its
/// own collapsed section with an "Un-keep" action.
pub async fn list_kept_series<'e, E>(executor: E) -> Result<Vec<PhantomSeries>>
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
           WHERE s.tidy_exempt = 1
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
/// value is exactly what marks a series that just lost all its
/// previously-owned files as a *transition* phantom rather than
/// steady-state. A series that has been zero-owned all along stays at 0.
///
/// This counts owned-AND-present specifically — narrower than the
/// phantom-set predicate in [`list_phantoms`], which considers any
/// present file (owned/needs_review/unmatched) as keeping a series
/// alive. The transition signal is "did this series ever *match* and
/// then lose its files", not "are there any bytes on disk" — so owned
/// is the right reference here. A series that is `list_phantoms`-phantom
/// (no files on disk) with `last_matched_count > 0` is the transition
/// phantom the route surfaces; a `list_phantoms`-phantom with
/// `last_matched_count = 0` is steady-state.
pub async fn refresh_last_matched_counts<'e, E>(executor: E) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    // Counts owned FILES, not owned ISSUES. Two files on one issue
    // count twice here and once in `issue_ownership`, so the view
    // cannot express this and must not be substituted in.
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
/// 6b). A series with no present file (in any catalog-linked status) has
/// its counter incremented; a series with at least one present file
/// — owned, needs_review, or unmatched — is reset to 0 **and** has any
/// `auto_tidy_due_at` mark cleared. The auto-recovery path includes
/// needs_review/unmatched so a series sitting at sub-threshold
/// confidence (real files on disk, just not promoted) is never tidied
/// away. Run once per full scan, after the mark-missing pass and
/// `refresh_last_matched_counts`. Tidy-exempt series (kept by the user)
/// are skipped entirely — their counter stays at the 0 that Keep set.
pub async fn tick_empty_scan_counters<'e, E>(executor: E) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE series SET
               consecutive_empty_scans = CASE WHEN EXISTS (
                   SELECT 1 FROM files f JOIN issues i ON f.issue_id = i.id
                   WHERE i.series_id = series.id
                     AND f.status IN ('owned', 'needs_review', 'unmatched')
                     AND f.is_present = 1
               ) THEN 0 ELSE consecutive_empty_scans + 1 END,
               auto_tidy_due_at = CASE WHEN EXISTS (
                   SELECT 1 FROM files f JOIN issues i ON f.issue_id = i.id
                   WHERE i.series_id = series.id
                     AND f.status IN ('owned', 'needs_review', 'unmatched')
                     AND f.is_present = 1
               ) THEN NULL ELSE auto_tidy_due_at END
           WHERE tidy_exempt = 0"#
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Mark every series eligible for automatic removal: empty for at least
/// `threshold` consecutive scans, not already marked, and not awaiting a
/// first download (no `pull_list` entry, no `pull_attempts` row — the
/// pull list is an explicit "I want this" signal auto-tidy must never
/// override). The emptiness check is "no present file in owned /
/// needs_review / unmatched" — a series with files on disk in any
/// catalog-linked status is never tidied, even if none are matched
/// above threshold. `due_at` is the recovery deadline written to the
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
             AND tidy_exempt = 0
             AND consecutive_empty_scans >= ?
             AND NOT EXISTS (
                 SELECT 1 FROM files f JOIN issues i ON f.issue_id = i.id
                 WHERE i.series_id = series.id
                   AND f.status IN ('owned', 'needs_review', 'unmatched')
                   AND f.is_present = 1
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

/// One row in the Library Tidy enrichment-review queue: a shallow
/// series the enrichment worker refused to auto-link (outcome ∈
/// `multi_match` / `low_confidence` / `year_mismatch` /
/// `collision_disabled` / `error`). Carries everything the UI
/// needs to render the row + drive the disambiguation picker:
/// title + start_year for display, the outcome + error text for
/// the badge and tooltip, and the owned-file count so the queue
/// can sort by priority (biggest libraries surface first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichmentQueueRow {
    pub id: i64,
    pub title: String,
    pub start_year: Option<i64>,
    pub last_enrichment_outcome: String,
    pub last_enrichment_error: Option<String>,
    pub owned_count: i64,
}

/// List the shallow series that the enrichment worker landed on a
/// review-required outcome — the disambiguation surface the
/// Library Tidy "Enrichment needs review" section renders. Same
/// definition as the `enrichment-summary` endpoint's
/// review-eligible filter (`cv_id IS NULL` + outcome in the named
/// set), with the owned-file aggregate joined in so the UI can
/// sort by impact.
///
/// Ordering: `owned_count DESC` (most files at stake first) then
/// `title ASC` (deterministic tiebreaker for the rendered list).
pub async fn list_enrichment_queue<'e, E>(executor: E) -> Result<Vec<EnrichmentQueueRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT
             s.id AS "id!: i64",
             s.title,
             s.start_year,
             s.last_enrichment_outcome AS "last_enrichment_outcome!: String",
             s.last_enrichment_error,
             COALESCE(oc.owned_count, 0) AS "owned_count!: i64"
           FROM series s
           LEFT JOIN (
             SELECT i.series_id,
                    COUNT(DISTINCT i.id) AS owned_count
               FROM issues i
               JOIN files f ON f.issue_id = i.id
              WHERE f.status = 'owned' AND f.is_present = 1
              GROUP BY i.series_id
           ) oc ON oc.series_id = s.id
           WHERE s.cv_id IS NULL
             AND s.last_enrichment_outcome IN (
                'multi_match', 'low_confidence', 'year_mismatch',
                'collision_disabled', 'error'
             )
           ORDER BY owned_count DESC, s.title ASC"#,
    )
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EnrichmentQueueRow {
            id: r.id,
            title: r.title,
            start_year: r.start_year,
            last_enrichment_outcome: r.last_enrichment_outcome,
            last_enrichment_error: r.last_enrichment_error,
            owned_count: r.owned_count,
        })
        .collect())
}

/// Unconditional cv_id write. Unlike [`set_cv_id`]'s
/// `WHERE cv_id IS NULL` race-guard, this overwrites whatever's
/// there. Used by the Library Tidy disambiguation PATCH: the user
/// has explicitly picked a CV volume for this series, so the
/// race-guard's "refuse if someone else got there first" is the
/// wrong semantics — the user's choice wins. The DB-level
/// `UNIQUE(cv_id)` constraint is still the backstop against two
/// series pointing at the same cv_id (the handler does a
/// pre-check + maps the constraint violation to a 409 anyway).
///
/// Returns rows-affected. 0 means the series_id no longer exists
/// (deleted between the queue load and the PATCH); caller should
/// surface that as a 404.
pub async fn force_set_cv_id<'e, E>(executor: E, series_id: i64, cv_id: i64) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET cv_id = ?, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        cv_id,
        series_id,
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
                ),
                -- Bug 4 bucket-ordering fix: rank each bucket
                -- internally, then interleave via (rank, bucket_order).
                -- 6c.3's first run had the discipline bucket
                -- monopolize the first 6 attempts because year-unknown
                -- series have systematically higher owned counts than
                -- easy-bucket year-known ones. Interleaving guarantees
                -- the easy bucket actually gets reached before
                -- max_run cuts off the cycle.
                ranked AS (
                    SELECT series_id, title, start_year, issue_count,
                           owned_count, collision_flag,
                           1 AS bucket_order,
                           ROW_NUMBER() OVER (ORDER BY owned_count DESC, lmc DESC, series_id ASC) AS rank
                    FROM easy_bucket
                    UNION ALL
                    SELECT series_id, title, start_year, issue_count,
                           owned_count, collision_flag,
                           2 AS bucket_order,
                           ROW_NUMBER() OVER (ORDER BY owned_count DESC, lmc DESC, series_id ASC) AS rank
                    FROM discipline_bucket
                    UNION ALL
                    SELECT series_id, title, start_year, issue_count,
                           owned_count, collision_flag,
                           3 AS bucket_order,
                           ROW_NUMBER() OVER (ORDER BY owned_count DESC, lmc DESC, series_id ASC) AS rank
                    FROM collision_bucket
                )
                SELECT
                    series_id AS "series_id!: i64",
                    title AS "title!: String",
                    start_year,
                    issue_count AS "issue_count!: i64",
                    owned_count AS "owned_count!: i64",
                    collision_flag AS "catalog_title_collision!: bool"
                FROM ranked
                ORDER BY rank ASC, bucket_order ASC
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

/// A CV-linked series whose descriptive volume detail (`publisher`)
/// was never persisted — the secondary "refresh pass" candidate set.
/// All series the enrichment worker promoted before 6c.5 land here
/// (the merge wrote only `cv_id`, discarding the publisher field
/// carried in `CvVolumeDetail`). After 6c.5 ships, this set grows
/// only via race conditions or future series whose volume detail
/// fetch failed partway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeRefreshCandidate {
    pub series_id: i64,
    pub cv_id: i64,
    pub title: String,
    pub owned_count: i64,
}

/// CV-linked series with `publisher IS NULL`, ordered by the same
/// priority the shallow-candidate query uses (owned_count DESC,
/// last_matched_count DESC, id ASC). The worker drains this set as
/// a secondary pass *after* the primary shallow set exhausts, using
/// only `fetch_volume` (not the full search/fetch_volume/fetch_issues
/// chain) — same background-throttle, ~30s per series.
///
/// No cooldown filter: a CV-linked series whose publisher is NULL is
/// data we owe the user, not a previously-refused attempt to back off
/// from.
pub async fn list_volume_refresh_candidates<'e, E>(
    executor: E,
) -> Result<Vec<VolumeRefreshCandidate>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        VolumeRefreshCandidate,
        r#"
        WITH owned_counts AS (
            SELECT i.series_id, COUNT(DISTINCT i.id) AS owned_count
            FROM issues i
            JOIN files f ON f.issue_id = i.id
            WHERE f.status = 'owned' AND f.is_present = 1
            GROUP BY i.series_id
        )
        SELECT
            s.id AS "series_id!: i64",
            s.cv_id AS "cv_id!: i64",
            s.title AS "title!: String",
            COALESCE(oc.owned_count, 0) AS "owned_count!: i64"
        FROM series s
        LEFT JOIN owned_counts oc ON oc.series_id = s.id
        WHERE s.cv_id IS NOT NULL
          AND s.publisher IS NULL
        ORDER BY owned_count DESC, s.last_matched_count DESC, s.id ASC
        "#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
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
/// steady-state), `consecutive_empty_scans` to 0, `auto_tidy_due_at`
/// to NULL (cancels a scheduled removal) — and set `tidy_exempt = 1`,
/// the durable flag that makes the decision stick. Resetting the
/// counters alone never held: the next scan ticked them right back up.
/// With `tidy_exempt = 1` the tick skips the series, auto-tidy never
/// marks it, and it leaves the phantom list entirely. The user has
/// reviewed the series and decided it stays. `NotFound` for an unknown
/// id. [`unkeep_phantom_series`] reverses it.
pub async fn keep_phantom_series<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET last_matched_count = 0,
               consecutive_empty_scans = 0,
               auto_tidy_due_at = NULL,
               tidy_exempt = 1
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

/// Reverse a [`keep_phantom_series`]: clear `tidy_exempt` so the series
/// is re-evaluated normally. Counters are also reset to 0 so it returns
/// as a steady-state phantom rather than re-appearing pre-marked — the
/// scanner will only re-schedule it for removal after it accrues a fresh
/// run of empty scans. `NotFound` for an unknown id.
pub async fn unkeep_phantom_series<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET last_matched_count = 0,
               consecutive_empty_scans = 0,
               auto_tidy_due_at = NULL,
               tidy_exempt = 0
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

/// One side of a duplicate-pair row. Each pair surfaces both series'
/// identity fields + the owned-and-present file count, so the UI can
/// show the user which one to merge into which without an additional
/// query.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct DuplicatePair {
    /// Detection criterion: `'same_cv_id'` (both series link to the
    /// same CV volume — technically impossible under the UNIQUE
    /// constraint, but kept for completeness) or `'same_title_close_year'`
    /// (same normalized title with start_year within 2 years, or both
    /// year-null).
    pub kind: String,
    pub a_id: i64,
    pub a_title: String,
    pub a_start_year: Option<i64>,
    pub a_cv_id: Option<i64>,
    pub a_owned_count: i64,
    pub b_id: i64,
    pub b_title: String,
    pub b_start_year: Option<i64>,
    pub b_cv_id: Option<i64>,
    pub b_owned_count: i64,
}

/// Find pairs of series that look like duplicates. Two criteria:
///
/// 1. **`same_cv_id`** — two series carrying the same `cv_id`.
///    Schema-level `UNIQUE(cv_id)` should prevent this so the query
///    returns nothing in practice. Kept defensively (a future schema
///    relaxation, a historical row that pre-dates the constraint, or
///    a manual UPDATE that bypassed it would all surface here).
///
/// 2. **`same_title_close_year`** — same `sort_title` AND either both
///    `start_year` NULL, or both set with absolute difference ≤ 2.
///    The 2-year window accommodates CV's year drift on volume
///    renumbers (2024 reboot vs 2023 special) without flagging
///    distinct reboots (a 2024 Daredevil and a 1964 Daredevil stay
///    distinct).
///
/// Both criteria emit ordered pairs (`a_id < b_id`) so a self-pair
/// is impossible and each duplicate-relation is one row, not two.
/// If a pair matches both criteria the dedup picks `same_cv_id` via
/// MIN(kind) (alphabetical ordering puts it first).
///
/// Owned-and-present file counts ride along so the UI can render the
/// "merge smaller into larger" call without a follow-up roundtrip.
pub async fn find_duplicate_pairs<'e, E>(executor: E) -> Result<Vec<DuplicatePair>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        DuplicatePair,
        r#"WITH owned_counts AS (
               SELECT i.series_id AS series_id, COUNT(*) AS n
               FROM files f JOIN issues i ON f.issue_id = i.id
               WHERE f.status = 'owned' AND f.is_present = 1
               GROUP BY i.series_id
           ),
           cv_id_pairs AS (
               SELECT 'same_cv_id' AS kind, s1.id AS a_id, s2.id AS b_id
               FROM series s1
               JOIN series s2 ON s1.cv_id = s2.cv_id
               WHERE s1.cv_id IS NOT NULL AND s1.id < s2.id
           ),
           title_pairs AS (
               SELECT 'same_title_close_year' AS kind, s1.id AS a_id, s2.id AS b_id
               FROM series s1
               JOIN series s2 ON s1.sort_title = s2.sort_title
               WHERE s1.id < s2.id
                 AND (
                   (s1.start_year IS NULL AND s2.start_year IS NULL)
                   OR (s1.start_year IS NOT NULL AND s2.start_year IS NOT NULL
                       AND ABS(s1.start_year - s2.start_year) <= 2)
                 )
           ),
           all_pairs AS (
               SELECT * FROM cv_id_pairs
               UNION ALL
               SELECT * FROM title_pairs
           ),
           dedup AS (
               -- MIN(kind) puts 'same_cv_id' ahead of 'same_title_close_year'
               -- when a pair matches both, so we surface the stronger
               -- criterion in the UI.
               SELECT a_id, b_id, MIN(kind) AS kind
               FROM all_pairs
               GROUP BY a_id, b_id
           )
           SELECT
             d.kind AS "kind!: String",
             sa.id AS "a_id!: i64",
             sa.title AS "a_title!: String",
             sa.start_year AS "a_start_year?: i64",
             sa.cv_id AS "a_cv_id?: i64",
             COALESCE(oa.n, 0) AS "a_owned_count!: i64",
             sb.id AS "b_id!: i64",
             sb.title AS "b_title!: String",
             sb.start_year AS "b_start_year?: i64",
             sb.cv_id AS "b_cv_id?: i64",
             COALESCE(ob.n, 0) AS "b_owned_count!: i64"
           FROM dedup d
           JOIN series sa ON sa.id = d.a_id
           JOIN series sb ON sb.id = d.b_id
           LEFT JOIN owned_counts oa ON oa.series_id = sa.id
           LEFT JOIN owned_counts ob ON ob.series_id = sb.id
           ORDER BY sa.sort_title COLLATE NOCASE, sa.id, sb.id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Merge `source_series_id` into `target_series_id`: move every issue
/// belonging to source over to target (files follow automatically via
/// `files.issue_id → issues.id`), then delete the now-empty source
/// series row.
///
/// All in one transaction so a partial failure rolls back cleanly —
/// the catalog never lands in a "half-merged" state where issues are
/// on the new series but the old series row still exists, or vice
/// versa.
///
/// No collision handling on `cv_issue_id`: the schema's
/// `UNIQUE(cv_issue_id)` prevents two series from owning the same
/// CV issue, so the move can't produce a duplicate. NULL
/// `cv_issue_id` rows don't participate in UNIQUE so freely move
/// (the user may want to dedup duplicate-number issues afterward,
/// but that's a separate cleanup; this merge intentionally preserves
/// every issue row).
///
/// Returns `NotFound` if either series id doesn't exist; a
/// `BadRequest`-like error path if target == source (the route
/// rejects this earlier with a 400, but the repo also guards).
pub async fn merge_series<'e, E>(executor: E, target_id: i64, source_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    if target_id == source_id {
        return Err(DbError::NotFound);
    }
    let result = sqlx::query!(
        r#"UPDATE issues SET series_id = ? WHERE series_id = ?"#,
        target_id,
        source_id
    )
    .execute(executor)
    .await?;
    let _ = result;
    // We do NOT require issues.rows_affected > 0 — a source series
    // with no issues (just a shallow stub) is a valid merge target.
    Ok(())
}

/// Hard-delete a series row. Used as the second step of the merge
/// flow (after `merge_series` moves the issues off), and as the
/// underlying delete primitive for any caller that wants the
/// `ON DELETE CASCADE` on dependent rows. `NotFound` when the id
/// doesn't resolve to a row.
pub async fn hard_delete<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM series WHERE id = ?"#, series_id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// `(local series id, cv_id)` for every catalog series that has a CV volume
/// id. Discovery joins a creator's `volume_credits` against this to split
/// in-library vs not, and to link owned volumes to their local series row.
pub async fn existing_cv_id_pairs<'e, E>(executor: E) -> Result<Vec<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", cv_id AS "cv_id!: i64"
           FROM series WHERE cv_id IS NOT NULL"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.cv_id)).collect())
}

/// `(series_id, cv_id)` for CV-linked series not yet Metron-linked and not yet
/// checked. Drives the Metron-linking resolver; excludes already-linked and
/// already-checked-no-match series so the resolver converges.
pub async fn list_metron_link_candidates<'e, E>(executor: E, limit: i64) -> Result<Vec<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", cv_id AS "cv_id!: i64"
           FROM series
           WHERE cv_id IS NOT NULL
             AND metron_id IS NULL
             AND metron_link_checked_at IS NULL
           ORDER BY id ASC
           LIMIT ?"#,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.cv_id)).collect())
}

/// `(series_id, metron_id)` for Metron-linked series whose issues haven't been
/// linked yet. Drives the issue-linking resolver.
pub async fn list_metron_issue_link_candidates<'e, E>(
    executor: E,
    limit: i64,
) -> Result<Vec<(i64, String)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", metron_id AS "metron_id!: String"
           FROM series
           WHERE metron_id IS NOT NULL
             AND metron_issues_linked_at IS NULL
           ORDER BY id ASC
           LIMIT ?"#,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.metron_id)).collect())
}

/// Stamp a series' issue-linking as done (matched, partially matched, or a
/// terminal fetch error) so it leaves the work-list.
pub async fn mark_metron_issues_linked<'e, E>(executor: E, series_id: i64) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET metron_issues_linked_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Record a Metron-link check: stamp `metron_link_checked_at` (so the series
/// leaves the work-list) and, on a match, set `metron_id`. `COALESCE` protects
/// an already-set metron_id from being clobbered by a racing writer.
/// `metron_id = None` = checked, no Metron match.
pub async fn mark_metron_link_checked<'e, E>(
    executor: E,
    series_id: i64,
    metron_id: Option<&str>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET metron_id = COALESCE(metron_id, ?),
               metron_link_checked_at = CURRENT_TIMESTAMP,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        metron_id,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

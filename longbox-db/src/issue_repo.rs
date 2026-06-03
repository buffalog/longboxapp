use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqliteExecutor};
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct IssueRow {
    pub id: i64,
    pub series_id: i64,
    pub cv_issue_id: Option<i64>,
    pub metron_issue_id: Option<String>,
    pub number: String,
    pub title: Option<String>,
    pub cover_date: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
    pub created_at: PrimitiveDateTime,
    pub updated_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewIssue {
    pub series_id: i64,
    pub cv_issue_id: Option<i64>,
    pub metron_issue_id: Option<String>,
    pub number: String,
    pub title: Option<String>,
    pub cover_date: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IssueUpdate {
    pub title: Option<String>,
    pub cover_date: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IssueRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  cv_issue_id, metron_issue_id, number, title, cover_date,
                  summary, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM issues WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn find_by_cv_issue_id<'e, E>(executor: E, cv_issue_id: i64) -> Result<Option<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IssueRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  cv_issue_id, metron_issue_id, number, title, cover_date,
                  summary, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM issues WHERE cv_issue_id = ?"#,
        cv_issue_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn find_by_metron_issue_id<'e, E>(
    executor: E,
    metron_issue_id: &str,
) -> Result<Option<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IssueRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  cv_issue_id, metron_issue_id, number, title, cover_date,
                  summary, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM issues WHERE metron_issue_id = ?"#,
        metron_issue_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn list_by_series<'e, E>(executor: E, series_id: i64) -> Result<Vec<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        IssueRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  cv_issue_id, metron_issue_id, number, title, cover_date,
                  summary, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM issues WHERE series_id = ? ORDER BY id"#,
        series_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Every "missing" issue for one series — mirrors the frontend
/// IssueRow's status derivation exactly: a row has status `missing`
/// when there's no owned-and-present file row AND the cover date is
/// today or earlier (i.e. the issue has shipped — not solicited).
///
/// Distinct from [`list_pull_candidates`] in two ways: this does
/// NOT exclude issues with in-flight attempts (the per-issue
/// engine guard handles dedup silently for fire-and-forget callers)
/// and does NOT exclude parked (`retry_count >= 3`) attempts —
/// the user's "Search missing" intent on the series page is a
/// manual override, same shape as the per-issue Search button
/// which also bypasses parking. The engine path (`fire_issue_search`
/// → `sweep_single_issue`) has its own load-bearing in-flight
/// guard, so handing it an already-in-flight issue is a no-op
/// rather than a duplicate submit.
pub async fn list_missing_for_series<'e, E>(
    executor: E,
    series_id: i64,
) -> Result<Vec<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        IssueRow,
        r#"SELECT i.id AS "id!: i64", i.series_id AS "series_id!: i64",
                  i.cv_issue_id, i.metron_issue_id, i.number, i.title, i.cover_date,
                  i.summary, i.cover_url,
                  i.created_at AS "created_at: _", i.updated_at AS "updated_at: _"
           FROM issues i
           WHERE i.series_id = ?
             AND i.cover_date IS NOT NULL
             AND length(i.cover_date) = 10
             AND i.cover_date <= date('now')
             AND NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1
             )
           ORDER BY i.cover_date ASC, i.id ASC"#,
        series_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Issues eligible for an auto-pull attempt for one series: shipped
/// (`cover_date` a full `YYYY-MM-DD`, today or earlier), not already
/// owned, and not already settled or parked in `pull_attempts`.
///
/// Excluded `pull_attempts` states: in-flight (`pending`/`submitted`),
/// done (`grabbed`), and parked — any attempt with `retry_count >= 3`,
/// the give-up threshold. `failed` and `mismatched` (Bug 3) below the
/// threshold leave the issue eligible (the engine retries it). The
/// `start_issue` floor is applied by the caller — natural issue-number
/// order isn't expressible in SQL.
pub async fn list_pull_candidates<'e, E>(executor: E, series_id: i64) -> Result<Vec<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        IssueRow,
        r#"SELECT i.id AS "id!: i64", i.series_id AS "series_id!: i64",
                  i.cv_issue_id, i.metron_issue_id, i.number, i.title, i.cover_date,
                  i.summary, i.cover_url,
                  i.created_at AS "created_at: _", i.updated_at AS "updated_at: _"
           FROM issues i
           WHERE i.series_id = ?
             AND i.cover_date IS NOT NULL
             AND length(i.cover_date) = 10
             AND i.cover_date <= date('now')
             AND NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1
             )
             AND NOT EXISTS (
               SELECT 1 FROM pull_attempts pa
               WHERE pa.issue_id = i.id
                 AND (pa.status IN ('pending', 'submitted', 'grabbed')
                      OR pa.retry_count >= 3)
             )
           ORDER BY i.cover_date ASC, i.id ASC"#,
        series_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewIssue) -> Result<IssueRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IssueRow,
        r#"INSERT INTO issues (series_id, cv_issue_id, metron_issue_id, number,
                               title, cover_date, summary, cover_url)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     cv_issue_id, metron_issue_id, number, title, cover_date,
                     summary, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        input.series_id,
        input.cv_issue_id,
        input.metron_issue_id,
        input.number,
        input.title,
        input.cover_date,
        input.summary,
        input.cover_url
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Delete every issue belonging to `series_id`. Cascades to
/// `files` via the FK definition. Used by the Library Tidy
/// disambiguation PATCH when the user picks a new CV volume for a
/// shallow series — the old issues belonged to the wrong volume
/// and would collide with the upcoming bulk_insert from the new
/// volume's CV fetch.
///
/// Returns the number of rows deleted.
pub async fn delete_by_series<'e, E>(executor: E, series_id: i64) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM issues WHERE series_id = ?"#, series_id)
        .execute(executor)
        .await?;
    Ok(result.rows_affected())
}

/// Single-statement multi-row insert. Up to ~4000 issues at once before
/// hitting SQLite's `SQLITE_MAX_VARIABLE_NUMBER` (default 32766 in modern
/// builds; 8 bind params per row). Returns inserted rows in the input order.
pub async fn bulk_insert<'e, E>(executor: E, inputs: Vec<NewIssue>) -> Result<Vec<IssueRow>>
where
    E: SqliteExecutor<'e>,
{
    if inputs.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "INSERT INTO issues (series_id, cv_issue_id, metron_issue_id, number, \
         title, cover_date, summary, cover_url) ",
    );

    qb.push_values(inputs, |mut b, input| {
        b.push_bind(input.series_id)
            .push_bind(input.cv_issue_id)
            .push_bind(input.metron_issue_id)
            .push_bind(input.number)
            .push_bind(input.title)
            .push_bind(input.cover_date)
            .push_bind(input.summary)
            .push_bind(input.cover_url);
    });

    qb.push(
        " RETURNING id, series_id, cv_issue_id, metron_issue_id, number, \
          title, cover_date, summary, cover_url, created_at, updated_at",
    );

    let rows: Vec<IssueRow> = qb.build_query_as().fetch_all(executor).await?;
    Ok(rows)
}

/// For `series_id`, ensure each issue number in `numbers` exists as a
/// number-only issue (no CV/Metron id), and return the resolved
/// `(id, number)` pairs for *every* number — newly synthesized rows
/// plus any pre-existing rows (CV-canonical or otherwise) that already
/// occupied those numbers. Pre-existing rows are returned untouched:
/// the `DO UPDATE SET number = excluded.number` is a no-op assignment
/// chosen solely to make `RETURNING` fire on conflict; their CV ids,
/// titles, dates, and summaries are preserved.
///
/// Used by the bulk-convert link mode (A.9 hot-fix): when convert
/// links a folder to an existing CV-tracked series, parsed numbers
/// missing from CV are filled in as shallow rows so every file can
/// attach, but CV-canonical issues for matching numbers stay
/// authoritative.
pub async fn upsert_number_only_returning<'e, E>(
    executor: E,
    series_id: i64,
    numbers: &[String],
) -> Result<Vec<(i64, String)>>
where
    E: SqliteExecutor<'e>,
{
    if numbers.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("INSERT INTO issues (series_id, number) ");
    qb.push_values(numbers.iter(), |mut b, n| {
        b.push_bind(series_id).push_bind(n);
    });
    // Bug 4: conflict on canonical_number — bulk-convert is the
    // second biting path. "001" and "1" now collide as expected.
    qb.push(" ON CONFLICT(series_id, canonical_number) DO UPDATE SET number = excluded.number");
    qb.push(" RETURNING id, number");
    let rows: Vec<(i64, String)> = qb.build_query_as().fetch_all(executor).await?;
    Ok(rows)
}

/// Insert-or-update by `cv_issue_id`. Used by `POST /api/series/:id/refresh`
/// when re-fetching from ComicVine: existing rows have their mutable fields
/// refreshed, new rows are inserted. `cv_issue_id` MUST be `Some(...)` — the
/// upsert keys on it and a `None` would never conflict.
pub async fn upsert_by_cv_id<'e, E>(executor: E, input: NewIssue) -> Result<IssueRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IssueRow,
        r#"INSERT INTO issues (series_id, cv_issue_id, metron_issue_id, number,
                               title, cover_date, summary, cover_url)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(cv_issue_id) DO UPDATE
           SET number = excluded.number,
               title = excluded.title,
               cover_date = excluded.cover_date,
               summary = excluded.summary,
               cover_url = excluded.cover_url,
               updated_at = CURRENT_TIMESTAMP
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     cv_issue_id, metron_issue_id, number, title, cover_date,
                     summary, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        input.series_id,
        input.cv_issue_id,
        input.metron_issue_id,
        input.number,
        input.title,
        input.cover_date,
        input.summary,
        input.cover_url
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Row-id-preserving upsert for CV enrichment (Step 6c.1).
///
/// Inserts a new `(series_id, number)` row OR updates the existing
/// match — keyed on the `UNIQUE(series_id, number)` index — writing
/// the CV-side fields while preserving the row id. This preservation
/// is the entire correctness point: `files.issue_id` references the
/// surviving row, so the file attachment carries through the
/// shallow-to-CV-linked promotion without re-attribution.
///
/// Used by the cv_enrichment worker (Step 6c.2) when a shallow
/// series acquires a `cv_id` and its issue list is fetched from
/// ComicVine. Each CV issue gets upserted; existing synthesized
/// issues (`cv_issue_id IS NULL`) for the same `(series_id, number)`
/// have their CV-side fields filled in.
///
/// Catalog-only numbers (rows in the catalog but absent from CV's
/// issue list — i.e., numbers in the catalog that this method is
/// never called for) are NOT touched. The worker counts them
/// separately and records `partial_merge` on the series row so the
/// stranded provenance is visible. This is the loudness-not-silence
/// discipline applied to the merge surface (compare Bug 3's
/// `pull_attempts.status='mismatched'`).
pub async fn upsert_by_series_id_and_number_with_cv_fields<'e, E>(
    executor: E,
    input: NewIssue,
) -> Result<IssueRow>
where
    E: SqliteExecutor<'e>,
{
    // Bug 4: conflict target is (series_id, canonical_number) — the
    // STORED generated column that strips leading zeros so "001" and
    // "1" collide as the same conceptual issue. `excluded.number`
    // stays the original-write form on UPDATE so display is preserved
    // verbatim. The matcher's `IssueNumber::matches()` already
    // canonicalizes at lookup time; this aligns the DB-level
    // uniqueness with that semantic.
    let row = sqlx::query_as!(
        IssueRow,
        r#"INSERT INTO issues (series_id, cv_issue_id, metron_issue_id, number,
                               title, cover_date, summary, cover_url)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(series_id, canonical_number) DO UPDATE
           SET cv_issue_id = excluded.cv_issue_id,
               title = excluded.title,
               cover_date = excluded.cover_date,
               summary = excluded.summary,
               cover_url = excluded.cover_url,
               updated_at = CURRENT_TIMESTAMP
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     cv_issue_id, metron_issue_id, number, title, cover_date,
                     summary, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        input.series_id,
        input.cv_issue_id,
        input.metron_issue_id,
        input.number,
        input.title,
        input.cover_date,
        input.summary,
        input.cover_url,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(executor: E, id: i64, patch: IssueUpdate) -> Result<IssueRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IssueRow,
        r#"UPDATE issues
           SET title = ?, cover_date = ?, summary = ?, cover_url = ?,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     cv_issue_id, metron_issue_id, number, title, cover_date,
                     summary, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        patch.title,
        patch.cover_date,
        patch.summary,
        patch.cover_url,
        id
    )
    .fetch_optional(executor)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(row)
}

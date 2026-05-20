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

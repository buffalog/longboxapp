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

pub async fn find_by_metron_id<'e, E>(
    executor: E,
    metron_id: &str,
) -> Result<Option<SeriesRow>>
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

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<SeriesRow>>
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

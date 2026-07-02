//! Creator + per-issue credit persistence and read queries. Credits are
//! role-atomic (CV's comma-delimited role strings are split upstream).
use serde::Serialize;
use sqlx::SqliteExecutor;

use longbox_comicvine::CvPersonCredit;

use crate::error::Result;
use crate::Pool;

/// Insert-or-update a creator by CV person id, returning its local id.
/// ON CONFLICT keeps the name fresh (CV name corrections propagate).
pub async fn upsert_creator<'e, E>(executor: E, cv_person_id: i64, name: &str) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"INSERT INTO creators (cv_person_id, name) VALUES (?, ?)
           ON CONFLICT(cv_person_id) DO UPDATE SET name = excluded.name
           RETURNING id AS "id!: i64""#,
        cv_person_id,
        name,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.id)
}

/// One issue awaiting credit resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueNeedingCredits {
    pub issue_id: i64,
    pub cv_issue_id: i64,
}

/// Owned, CV-keyed issues whose credits haven't been fetched yet, oldest
/// first. Drives the background resolver. Skips non-owned (out of scope for
/// search) and the ~36 issues with no cv_issue_id (can't be fetched).
pub async fn list_issues_needing_credits<'e, E>(
    executor: E,
    limit: i64,
) -> Result<Vec<IssueNeedingCredits>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT i.id AS "issue_id!: i64", i.cv_issue_id AS "cv_issue_id!: i64"
           FROM issues i
           WHERE i.credits_fetched = 0
             AND i.cv_issue_id IS NOT NULL
             AND EXISTS (SELECT 1 FROM files f
                         WHERE f.issue_id = i.id AND f.status = 'owned' AND f.is_present = 1)
           ORDER BY i.id ASC
           LIMIT ?"#,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| IssueNeedingCredits {
            issue_id: r.issue_id,
            cv_issue_id: r.cv_issue_id,
        })
        .collect())
}

/// Persist a fully-resolved set of atomic credits for one issue and flip
/// `credits_fetched`. Transactional + idempotent: creators dedupe on
/// cv_person_id, credit rows `INSERT OR IGNORE` against the UNIQUE
/// (issue_id, creator_id, role). An empty slice marks the issue done with
/// no rows (the CV-NotFound path).
pub async fn insert_issue_credits(
    pool: &Pool,
    issue_id: i64,
    credits: &[CvPersonCredit],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    for c in credits {
        let creator_id = upsert_creator(&mut *tx, c.cv_person_id, &c.name).await?;
        sqlx::query!(
            r#"INSERT OR IGNORE INTO issue_credits (issue_id, creator_id, role) VALUES (?, ?, ?)"#,
            issue_id,
            creator_id,
            c.role,
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        r#"UPDATE issues SET credits_fetched = 1 WHERE id = ?"#,
        issue_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Read query result structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorSearchRow {
    pub id: i64,
    pub name: String,
    pub cv_person_id: Option<i64>,
    pub series_count: i64,
    pub issue_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RoleCount {
    pub role: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorSeries {
    pub series_id: i64,
    pub name: String,
    pub issue_count: i64,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorDetail {
    pub id: i64,
    pub name: String,
    pub cv_person_id: Option<i64>,
    pub roles: Vec<RoleCount>,
    pub series: Vec<CreatorSeries>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorIssueRow {
    pub issue_id: i64,
    pub series_name: String,
    pub issue_number: String,
    pub cover_date: Option<String>,
    pub cover_url: Option<String>,
    pub role: String,
}

// ---------------------------------------------------------------------------
// Read queries (owned issues only)
// ---------------------------------------------------------------------------

/// Creator name search (owned issues only). `q` is wrapped in `%…%`.
pub async fn search_creators<'e, E>(executor: E, q: &str) -> Result<Vec<CreatorSearchRow>>
where
    E: SqliteExecutor<'e>,
{
    let like = format!("%{q}%");
    let rows = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", c.name AS "name!", c.cv_person_id,
                  COUNT(DISTINCT i.series_id) AS "series_count!: i64",
                  COUNT(DISTINCT i.id)        AS "issue_count!: i64"
           FROM creators c
           JOIN issue_credits ic ON ic.creator_id = c.id
           JOIN issues i         ON i.id = ic.issue_id
           WHERE c.name LIKE ? COLLATE NOCASE
             AND EXISTS (SELECT 1 FROM files f
                         WHERE f.issue_id = i.id AND f.status='owned' AND f.is_present=1)
           GROUP BY c.id
           -- ORDER BY repeats the aggregate: the sqlx type-cast alias
           -- ("issue_count!: i64") isn't a plain column name, so it can't be
           -- referenced in ORDER BY.
           ORDER BY COUNT(DISTINCT i.id) DESC
           LIMIT 20"#,
        like,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CreatorSearchRow {
            id: r.id,
            name: r.name,
            cv_person_id: r.cv_person_id,
            series_count: r.series_count,
            issue_count: r.issue_count,
        })
        .collect())
}

/// Returns `Some` for ANY existing creator row (the base lookup is not
/// ownership-gated); `roles` and `series` are owned-issues-only, so a creator
/// that currently owns nothing returns `Some` with empty facets. `search`
/// surfaces only creators with an owned issue, so this asymmetry (invisible in
/// search, empty-but-present on direct fetch) is intentional.
pub async fn creator_detail(pool: &Pool, id: i64) -> Result<Option<CreatorDetail>> {
    let base = sqlx::query!(
        r#"SELECT id AS "id!: i64", name AS "name!", cv_person_id FROM creators WHERE id = ?"#,
        id
    )
    .fetch_optional(pool)
    .await?;
    let Some(base) = base else {
        return Ok(None);
    };

    let roles = sqlx::query!(
        r#"SELECT ic.role AS "role!", COUNT(DISTINCT ic.issue_id) AS "count!: i64"
           FROM issue_credits ic JOIN issues i ON i.id = ic.issue_id
           WHERE ic.creator_id = ?
             AND EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id AND f.status='owned' AND f.is_present=1)
           GROUP BY ic.role
           -- ORDER BY repeats the aggregate: the sqlx type-cast alias
           -- ("count!: i64") isn't a plain column name, so it can't be
           -- referenced in ORDER BY.
           ORDER BY COUNT(DISTINCT ic.issue_id) DESC"#,
        id
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| RoleCount {
        role: r.role,
        count: r.count,
    })
    .collect();

    let series = sqlx::query!(
        r#"SELECT s.id AS "series_id!: i64", s.title AS "name!", s.cover_url,
                  COUNT(DISTINCT i.id) AS "issue_count!: i64"
           FROM issue_credits ic JOIN issues i ON i.id = ic.issue_id JOIN series s ON s.id = i.series_id
           WHERE ic.creator_id = ?
             AND EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id AND f.status='owned' AND f.is_present=1)
           GROUP BY s.id
           -- ORDER BY repeats the aggregate: the sqlx type-cast alias
           -- ("issue_count!: i64") isn't a plain column name, so it can't be
           -- referenced in ORDER BY.
           ORDER BY COUNT(DISTINCT i.id) DESC"#,
        id
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| CreatorSeries {
        series_id: r.series_id,
        name: r.name,
        issue_count: r.issue_count,
        cover_url: r.cover_url,
    })
    .collect();

    Ok(Some(CreatorDetail {
        id: base.id,
        name: base.name,
        cv_person_id: base.cv_person_id,
        roles,
        series,
    }))
}

/// The CV person id for a creator. `None` when the creator doesn't exist OR
/// has no `cv_person_id` — both mean "no discovery possible" to the caller.
pub async fn cv_person_id_of<'e, E>(executor: E, creator_id: i64) -> Result<Option<i64>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT cv_person_id FROM creators WHERE id = ?"#,
        creator_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.and_then(|r| r.cv_person_id))
}

/// All `(cv_person_id, local creator id)` pairs for creators that carry a
/// CV person id. Small table (~2k rows); used to flag CV person-search hits
/// that are already in the library.
pub async fn cv_person_id_map<'e, E>(executor: E) -> Result<Vec<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT cv_person_id AS "cv_person_id!: i64", id AS "id!: i64"
           FROM creators WHERE cv_person_id IS NOT NULL"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.cv_person_id, r.id)).collect())
}

/// Paginated in-library issues for a creator, optional role + series filters.
/// Page is 1-based; page size 50; ordered by cover_date ASC.
pub async fn creator_issues<'e, E>(
    executor: E,
    creator_id: i64,
    role: Option<&str>,
    series_id: Option<i64>,
    page: i64,
) -> Result<Vec<CreatorIssueRow>>
where
    E: SqliteExecutor<'e>,
{
    let offset = (page.max(1) - 1) * 50;
    let rows = sqlx::query!(
        r#"SELECT i.id AS "issue_id!: i64", s.title AS "series_name!",
                  i.number AS "issue_number!", i.cover_date, i.cover_url, ic.role AS "role!"
           FROM issue_credits ic JOIN issues i ON i.id = ic.issue_id JOIN series s ON s.id = i.series_id
           WHERE ic.creator_id = ?
             AND EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id AND f.status='owned' AND f.is_present=1)
             AND (? IS NULL OR ic.role = ?)
             AND (? IS NULL OR i.series_id = ?)
           ORDER BY i.cover_date ASC
           LIMIT 50 OFFSET ?"#,
        creator_id,
        role,
        role,
        series_id,
        series_id,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CreatorIssueRow {
            issue_id: r.issue_id,
            series_name: r.series_name,
            issue_number: r.issue_number,
            cover_date: r.cover_date,
            cover_url: r.cover_url,
            role: r.role,
        })
        .collect())
}

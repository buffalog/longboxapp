//! Creator + per-issue credit persistence. Credits are role-atomic
//! (CV's comma-delimited role strings are split upstream in longbox-comicvine).
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

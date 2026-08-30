//! The view must answer exactly what the hand-written predicate
//! answered. This is the whole risk of the consolidation: not "does the
//! view work" but "did meaning change".

mod common;

use common::fresh_pool;
use sqlx::SqlitePool;

async fn seed(db: &SqlitePool) {
    sqlx::query("INSERT INTO series (id, title, sort_title) VALUES (1, 'S', 's')")
        .execute(db)
        .await
        .unwrap();
    for (id, num) in [(1, "1"), (2, "2"), (3, "3"), (4, "4")] {
        sqlx::query("INSERT INTO issues (id, series_id, number) VALUES (?, 1, ?)")
            .bind(id)
            .bind(num)
            .execute(db)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO library_roots (id, path) VALUES (1, '/x')")
        .execute(db)
        .await
        .unwrap();
    // issue 1: owned+present -> owned
    // issue 2: owned but absent -> NOT owned
    // issue 3: present but needs_review -> NOT owned
    // issue 4: no file at all -> NOT owned
    for (fid, iid, status, present) in [
        (1, Some(1), "owned", 1),
        (2, Some(2), "owned", 0),
        (3, Some(3), "needs_review", 1),
    ] {
        sqlx::query(
            "INSERT INTO files (id, issue_id, library_root_id, path_relative, size_bytes,
                                mtime, last_scanned_at, match_method, match_confidence,
                                status, is_present, last_seen_at)
             VALUES (?, ?, 1, 'p' || ?, 1, datetime('now'), datetime('now'), 'test', 1.0, ?, ?, datetime('now'))",
        )
        .bind(fid)
        .bind(iid)
        .bind(fid)
        .bind(status)
        .bind(present)
        .execute(db)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn the_view_agrees_with_the_hand_written_predicate() {
    let db = fresh_pool().await;
    seed(&db).await;

    let hand: Vec<i64> = sqlx::query_scalar(
        "SELECT i.id FROM issues i
         WHERE NOT EXISTS (SELECT 1 FROM files f
                           WHERE f.issue_id = i.id
                             AND f.status = 'owned' AND f.is_present = 1)
         ORDER BY i.id",
    )
    .fetch_all(&db)
    .await
    .unwrap();

    let view: Vec<i64> = sqlx::query_scalar(
        "SELECT issue_id FROM issue_ownership WHERE is_owned = 0 ORDER BY issue_id",
    )
    .fetch_all(&db)
    .await
    .unwrap();

    assert_eq!(hand, vec![2, 3, 4], "fixture sanity: 2,3,4 are missing");
    assert_eq!(view, hand, "the view must not change meaning");
}

/// The view's `series_id` column had no direct coverage: mutating
/// `i.series_id` to a constant left this file green and was caught only
/// indirectly, by a web-crate stats test. `series_id` is what the
/// per-series ownership conversion in stats.rs depends on, so it gets
/// pinned here.
#[tokio::test]
async fn the_view_carries_the_issues_series_id() {
    let db = fresh_pool().await;
    seed(&db).await;
    sqlx::query("INSERT INTO series (id, title, sort_title) VALUES (2, 'T', 't')")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues (id, series_id, number) VALUES (9, 2, '1')")
        .execute(&db)
        .await
        .unwrap();

    let pairs: Vec<(i64, i64)> =
        sqlx::query_as("SELECT issue_id, series_id FROM issue_ownership ORDER BY issue_id")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(
        pairs,
        vec![(1, 1), (2, 1), (3, 1), (4, 1), (9, 2)],
        "each row must carry its own issue's series_id"
    );
}

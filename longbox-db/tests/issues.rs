mod common;

use std::time::Instant;

use common::fresh_pool;
use longbox_db::{issue_repo, series_repo, DbError, IssueUpdate, NewIssue, NewSeries};

async fn seed_series(pool: &sqlx::SqlitePool) -> i64 {
    series_repo::insert(
        pool,
        NewSeries {
            cv_id: Some(1),
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2012),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

fn new_issue(series_id: i64, number: &str, cv: Option<i64>) -> NewIssue {
    NewIssue {
        series_id,
        cv_issue_id: cv,
        metron_issue_id: None,
        number: number.to_string(),
        title: None,
        cover_date: None,
        summary: None,
        cover_url: None,
    }
}

#[tokio::test]
async fn insert_and_find_by_id() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::insert(&pool, new_issue(series_id, "1", Some(100)))
        .await
        .unwrap();
    assert_eq!(row.number, "1");
    let found = issue_repo::find_by_id(&pool, row.id).await.unwrap();
    assert_eq!(found, Some(row));
}

#[tokio::test]
async fn find_by_cv_issue_id() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::insert(&pool, new_issue(series_id, "1", Some(42)))
        .await
        .unwrap();
    let found = issue_repo::find_by_cv_issue_id(&pool, 42).await.unwrap();
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn find_by_metron_issue_id() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let input = NewIssue {
        metron_issue_id: Some("saga-1-2012".to_string()),
        ..new_issue(series_id, "1", None)
    };
    let row = issue_repo::insert(&pool, input).await.unwrap();
    let found = issue_repo::find_by_metron_issue_id(&pool, "saga-1-2012")
        .await
        .unwrap();
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn list_by_series() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    issue_repo::insert(&pool, new_issue(series_id, "2", Some(2)))
        .await
        .unwrap();
    let rows = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    let numbers: Vec<&str> = rows.iter().map(|r| r.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2"]);
}

#[tokio::test]
async fn update_overwrites_metadata_fields() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    let updated = issue_repo::update(
        &pool,
        row.id,
        IssueUpdate {
            title: Some("One Small Step".into()),
            cover_date: Some("2012-03-14".into()),
            summary: Some("A summary.".into()),
            cover_url: Some("https://example.com/saga-1.jpg".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.title.as_deref(), Some("One Small Step"));
    assert_eq!(updated.cover_date.as_deref(), Some("2012-03-14"));
}

#[tokio::test]
async fn update_missing_returns_not_found() {
    let pool = fresh_pool().await;
    let err = issue_repo::update(
        &pool,
        999,
        IssueUpdate {
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn bulk_insert_returns_rows_in_input_order() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let inputs: Vec<NewIssue> = (1..=5)
        .map(|n| new_issue(series_id, &n.to_string(), Some(1000 + i64::from(n))))
        .collect();
    let rows = issue_repo::bulk_insert(&pool, inputs).await.unwrap();
    let numbers: Vec<&str> = rows.iter().map(|r| r.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2", "3", "4", "5"]);
}

#[tokio::test]
async fn bulk_insert_500_in_under_100ms() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let inputs: Vec<NewIssue> = (1..=500)
        .map(|n| new_issue(series_id, &n.to_string(), Some(10_000 + i64::from(n))))
        .collect();
    let start = Instant::now();
    let rows = issue_repo::bulk_insert(&pool, inputs).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(rows.len(), 500);
    assert!(
        elapsed.as_millis() < 100,
        "bulk_insert 500 took {elapsed:?}, expected < 100ms"
    );
}

#[tokio::test]
async fn bulk_insert_empty_returns_empty() {
    let pool = fresh_pool().await;
    let rows = issue_repo::bulk_insert(&pool, vec![]).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn duplicate_series_number_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    let err = issue_repo::insert(&pool, new_issue(series_id, "1", Some(2)))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            DbError::UniqueViolation {
                field: "issues_series_id_number"
            }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn duplicate_cv_issue_id_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(99)))
        .await
        .unwrap();
    let err = issue_repo::insert(&pool, new_issue(series_id, "2", Some(99)))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            DbError::UniqueViolation {
                field: "cv_issue_id"
            }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn cascade_delete_when_series_dropped() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    sqlx::query!(r#"DELETE FROM series WHERE id = ?"#, series_id)
        .execute(&pool)
        .await
        .unwrap();
    let rows = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    assert!(rows.is_empty());
}

// -------- 6c.1: upsert_by_series_id_and_number_with_cv_fields --------
//
// The row-id-preservation property is the entire correctness point —
// files.issue_id references the surviving row, so the file attachment
// must carry through the shallow-to-CV-linked promotion without
// re-attribution.

fn cv_filled_issue(series_id: i64, number: &str, cv_id: i64) -> NewIssue {
    NewIssue {
        series_id,
        cv_issue_id: Some(cv_id),
        metron_issue_id: None,
        number: number.to_string(),
        title: Some(format!("CV title for #{number}")),
        cover_date: Some("2024-01-15".to_string()),
        summary: Some(format!("CV summary for #{number}")),
        cover_url: Some(format!("https://cv.example/cover-{number}.jpg")),
    }
}

#[tokio::test]
async fn upsert_with_cv_fields_inserts_when_no_existing_row() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "1", 1001),
    )
    .await
    .unwrap();
    assert_eq!(row.series_id, series_id);
    assert_eq!(row.number, "1");
    assert_eq!(row.cv_issue_id, Some(1001));
    assert_eq!(row.title.as_deref(), Some("CV title for #1"));
}

#[tokio::test]
async fn upsert_with_cv_fields_preserves_row_id_on_existing_match() {
    // The load-bearing test: a synthesized issue exists with
    // cv_issue_id IS NULL. The upsert fills CV fields in place.
    // The row id MUST be unchanged so files.issue_id references
    // survive.
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;

    // Synthesized row first (matches what bulk-convert creates).
    let synthesized = issue_repo::insert(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "5".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    let original_id = synthesized.id;
    assert!(synthesized.cv_issue_id.is_none());

    // Now upsert with CV fields — should UPDATE not INSERT, id stable.
    let after_upsert = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "5", 5005),
    )
    .await
    .unwrap();

    assert_eq!(
        after_upsert.id, original_id,
        "row id must be preserved (files.issue_id depends on it)"
    );
    assert_eq!(after_upsert.cv_issue_id, Some(5005));
    assert_eq!(after_upsert.title.as_deref(), Some("CV title for #5"));
    assert_eq!(after_upsert.cover_date.as_deref(), Some("2024-01-15"));
    assert_eq!(after_upsert.summary.as_deref(), Some("CV summary for #5"));
}

#[tokio::test]
async fn upsert_with_cv_fields_keeps_files_issue_id_attached() {
    // The end-to-end correctness property: a file attached to the
    // synthesized issue MUST still resolve to the same issue after
    // the upsert. If the upsert deleted+re-inserted instead of
    // updated, the file row's issue_id would orphan.
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let synthesized = issue_repo::insert(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "10".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Seed a library_root + file row attached to this issue.
    let library_root_id = longbox_db::library_root_repo::insert(
        &pool,
        longbox_db::NewLibraryRoot {
            path: "/tmp/upsert-test".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let file_row = sqlx::query!(
        r#"INSERT INTO files (issue_id, library_root_id, path_relative, size_bytes,
                              mtime, last_scanned_at, match_method, match_confidence,
                              status, is_present, last_seen_at)
           VALUES (?, ?, 'series/issue-10.cbz', 100, '2024-01-01 00:00:00',
                   '2024-01-01 00:00:00', 'phase_b', 1.0, 'owned', 1,
                   '2024-01-01 00:00:00')
           RETURNING id AS "id!: i64""#,
        synthesized.id,
        library_root_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let file_id = file_row.id;

    // Promote the synthesized issue via the upsert.
    let promoted = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "10", 10010),
    )
    .await
    .unwrap();
    assert_eq!(promoted.id, synthesized.id);

    // The file row's issue_id is unchanged and still points at the
    // promoted (no longer synthesized) issue.
    let post = sqlx::query!(
        r#"SELECT issue_id AS "issue_id?: i64" FROM files WHERE id = ?"#,
        file_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(post.issue_id, Some(synthesized.id));
}

#[tokio::test]
async fn upsert_with_cv_fields_distinct_numbers_get_distinct_rows() {
    // Sanity: different (series_id, number) pairs don't collapse.
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let r1 = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "1", 1),
    )
    .await
    .unwrap();
    let r2 = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "2", 2),
    )
    .await
    .unwrap();
    assert_ne!(r1.id, r2.id);
    assert_eq!(r1.number, "1");
    assert_eq!(r2.number, "2");
}

#[tokio::test]
async fn upsert_with_cv_fields_handles_fractional_issue_number() {
    // Issue numbers are TEXT — the "½" edge case used by Promethea
    // Book 0.5 and similar literal-character numbers must survive
    // verbatim through the upsert. No normalization or coercion.
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(9999),
            metron_issue_id: None,
            number: "½".into(),
            title: Some("Promethea Half".into()),
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(row.number, "½");
    // Re-upsert the same key — UPDATE branch, same row id.
    let after = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(9999),
            metron_issue_id: None,
            number: "½".into(),
            title: Some("Promethea Half — refreshed".into()),
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(after.id, row.id);
    assert_eq!(after.title.as_deref(), Some("Promethea Half — refreshed"));
}

// -------- Bug 4: canonical_number unique index --------

/// The canonical key collapses padding-equivalent strings ("001"
/// and "1") onto the same conceptual issue. The upsert MUST see
/// them as the same key and UPDATE rather than INSERT.
#[tokio::test]
async fn upsert_with_cv_fields_padding_equivalent_keys_collapse() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    // Insert padded form first (matches what the parser would produce).
    let padded = issue_repo::insert(&pool, new_issue(series_id, "001", None))
        .await
        .unwrap();
    // Now upsert unpadded form (what CV returns). Bug 4's canonical
    // key MUST collapse these: same row id, no new INSERT.
    let after = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "1", 4001),
    )
    .await
    .unwrap();
    assert_eq!(
        after.id, padded.id,
        "padded \"001\" and unpadded \"1\" must collapse onto the same row"
    );
    // The original-write form is preserved on UPDATE (excluded.number
    // is NOT in the SET clause), so the row's number stays "001"
    // even though we upserted with "1".
    assert_eq!(after.number, "001", "display form preserved verbatim");
    assert_eq!(after.cv_issue_id, Some(4001));
}

/// Distinct issue forms that AREN'T padding-equivalent must still
/// stay distinct. The disagreement-coexist test from 6c.1 still
/// holds under Bug 4 — "½" and "1/2" are genuinely different
/// string forms with different canonicalizations.
#[tokio::test]
async fn upsert_with_cv_fields_genuine_disagreement_still_coexists() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let half = issue_repo::insert(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "½".into(),
            ..new_issue(series_id, "½", None)
        },
    )
    .await
    .unwrap();
    let one_half = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        cv_filled_issue(series_id, "1/2", 5001),
    )
    .await
    .unwrap();
    assert_ne!(one_half.id, half.id, "½ and 1/2 stay distinct");
}

/// 6c.1 amendment: the disagreement-coexist case for issue
/// numbers. The risk path is CV returning the textual form
/// `"1/2"` while the parser captured the literal character `"½"`
/// from a folder/filename. These are genuinely-different keys
/// under SQLite's `(series_id, number)` UNIQUE — no normalization
/// happens at the column level — so the upsert must INSERT a new
/// row rather than silently collapsing onto the ½ row.
///
/// Locks the "no silent collapse" property: a naive normalize-and-
/// merge would clobber the synthesized ½ row's identity and orphan
/// any files attached to it. The two rows must coexist; the user
/// disambiguates via the Change Match modal if needed.
#[tokio::test]
async fn upsert_with_cv_fields_disagreement_coexists_no_silent_collapse() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;

    // Synthesized row from the parser, literal-character form.
    let half = issue_repo::insert(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "½".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Attach a file to the synthesized ½ row so the silent-collapse
    // failure would be observable: if the upsert clobbers ½, this
    // file orphans.
    let library_root_id = longbox_db::library_root_repo::insert(
        &pool,
        longbox_db::NewLibraryRoot {
            path: "/tmp/disagreement-test".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let file_row = sqlx::query!(
        r#"INSERT INTO files (issue_id, library_root_id, path_relative, size_bytes,
                              mtime, last_scanned_at, match_method, match_confidence,
                              status, is_present, last_seen_at)
           VALUES (?, ?, 'series/issue-half-literal.cbz', 100, '2024-01-01 00:00:00',
                   '2024-01-01 00:00:00', 'phase_b', 1.0, 'owned', 1,
                   '2024-01-01 00:00:00')
           RETURNING id AS "id!: i64""#,
        half.id,
        library_root_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let half_file_id = file_row.id;

    // CV returns the textual form "1/2" for the same conceptual
    // issue. The upsert MUST treat this as a distinct
    // (series_id, number) key and INSERT a fresh row — not collapse
    // onto the ½ row.
    let textual_form = issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(8888),
            metron_issue_id: None,
            number: "1/2".into(),
            title: Some("Promethea Half (CV)".into()),
            cover_date: Some("2000-06-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // The two rows have distinct ids and distinct numbers.
    assert_ne!(textual_form.id, half.id, "must not collapse onto ½ row");
    assert_eq!(textual_form.number, "1/2");

    // The original ½ row is untouched — still synthesized, still
    // referenced by the same file.
    let half_after = issue_repo::find_by_id(&pool, half.id)
        .await
        .unwrap()
        .expect("½ row must still exist");
    assert_eq!(half_after.number, "½");
    assert!(
        half_after.cv_issue_id.is_none(),
        "½ row's cv_issue_id must NOT be clobbered"
    );
    assert!(
        half_after.title.is_none(),
        "½ row's title must NOT be overwritten"
    );

    // The attached file still resolves to the ½ row, not the CV row.
    let file_check = sqlx::query!(
        r#"SELECT issue_id AS "issue_id?: i64" FROM files WHERE id = ?"#,
        half_file_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        file_check.issue_id,
        Some(half.id),
        "file's issue_id must still point at the ½ row, not orphan or re-attribute to CV"
    );

    // Both rows present in the series listing — the user sees both,
    // can disambiguate via Change Match if needed.
    let listed = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    let numbers: Vec<&str> = listed.iter().map(|r| r.number.as_str()).collect();
    assert!(numbers.contains(&"½"));
    assert!(numbers.contains(&"1/2"));
    assert_eq!(listed.len(), 2, "two distinct issue numbers expected");
}

//! Duplicate-file detector: detection query, and the permanent-delete
//! resolve path. This feature deletes real user files, so the tests cover
//! both the happy path (loser gone from disk AND DB, keeper untouched) and
//! every refusal guard (keep-not-a-candidate, mismatched issue numbers).

mod common;

use axum::http::StatusCode;
use common::{build_test_app, empty_request, json_request, response_json, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};
use time::{OffsetDateTime, PrimitiveDateTime};

fn now_pdt() -> PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    PrimitiveDateTime::new(n.date(), n.time())
}

async fn seed_series_issue(app: &TestApp, title: &str, number: &str) -> i64 {
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.to_owned(),
            sort_title: title.to_lowercase(),
            start_year: Some(2012),
            publisher: Some("Image".to_owned()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: series,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.to_owned(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// Insert a file row and (optionally) write real bytes to disk under the
/// library root. `size` is stored on the row independently of the bytes on
/// disk, so size-heuristic tests don't have to write 90 MB.
async fn seed_file(
    app: &TestApp,
    issue_id: i64,
    rel: &str,
    is_present: bool,
    size: i64,
    write_disk: bool,
) -> i64 {
    if write_disk {
        let full = app.library_path().join(rel);
        tokio::fs::create_dir_all(full.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&full, b"stub-comic-bytes").await.unwrap();
    }
    let now = now_pdt();
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: app.library_root_id,
            path_relative: rel.to_owned(),
            size_bytes: size,
            mtime: now,
            last_scanned_at: now,
            match_method: "test".to_owned(),
            match_confidence: 1.0,
            status: "owned".to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present,
            last_seen_at: now,
            matched_at: Some(now),
        },
    )
    .await
    .unwrap()
    .id
}

fn on_disk(app: &TestApp, rel: &str) -> bool {
    app.library_path().join(rel).exists()
}

async fn db_row_exists(app: &TestApp, file_id: i64) -> bool {
    file_repo::find_by_id(&app.state.db, file_id)
        .await
        .unwrap()
        .is_some()
}

// -------- detection --------

#[tokio::test]
async fn detection_lists_only_multi_present_groups() {
    let app = build_test_app().await;
    // Group A: two present files → a duplicate group.
    let a = seed_series_issue(&app, "Saga", "1").await;
    seed_file(&app, a, "Saga/Saga 1.cbz", true, 90_000_000, false).await;
    seed_file(&app, a, "Saga/Saga 1.cbr", true, 80_000_000, false).await;
    // Issue B: a single present file → not a group.
    let b = seed_series_issue(&app, "Nailbiter", "1").await;
    seed_file(
        &app,
        b,
        "Nailbiter/Nailbiter 1.cbz",
        true,
        50_000_000,
        false,
    )
    .await;
    // Issue C: two files but one is absent → not a group (present count 1).
    let c = seed_series_issue(&app, "Paper Girls", "1").await;
    seed_file(
        &app,
        c,
        "Paper Girls/Paper Girls 1.cbz",
        true,
        50_000_000,
        false,
    )
    .await;
    seed_file(&app, c, "Paper Girls/old.cbz", false, 50_000_000, false).await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["total"], 1);
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["issue_id"], a);
    assert_eq!(groups[0]["kind"], "duplicate");
    assert_eq!(groups[0]["files"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn detection_suggests_healthy_copy_and_flags_corrupt() {
    let app = build_test_app().await;
    // The real "Darkness" case: a 607-byte cbz next to a healthy 100 MB cbr.
    let issue = seed_series_issue(&app, "The Darkness", "1").await;
    let tiny = seed_file(
        &app,
        issue,
        "The Darkness/The Darkness 1.cbz",
        true,
        607,
        false,
    )
    .await;
    let big = seed_file(
        &app,
        issue,
        "The Darkness/The Darkness 1.cbr",
        true,
        100_000_000,
        false,
    )
    .await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = &body["groups"][0];
    assert_eq!(g["kind"], "duplicate");
    // Suggests the healthy cbr despite cbz normally winning on format.
    assert_eq!(g["suggested_keep_file_id"], big);
    // The tiny cbz is flagged corrupt.
    let files = g["files"].as_array().unwrap();
    let tiny_row = files.iter().find(|f| f["file_id"] == tiny).unwrap();
    assert_eq!(tiny_row["suspect_corrupt"], true);
    let big_row = files.iter().find(|f| f["file_id"] == big).unwrap();
    assert_eq!(big_row["suspect_corrupt"], false);
}

// -------- resolve: happy path --------

#[tokio::test]
async fn resolve_deletes_losers_on_disk_and_in_db_keeps_chosen() {
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let loser = seed_file(&app, issue, "Saga (dup)/Saga 1.cbr", true, 80_000_000, true).await;
    assert!(on_disk(&app, "Saga/Saga 1.cbz"));
    assert!(on_disk(&app, "Saga (dup)/Saga 1.cbr"));

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "resolved");
    assert_eq!(r["kept_file_id"], keep);
    assert_eq!(r["deleted_file_ids"], serde_json::json!([loser]));
    assert_eq!(r["failed"].as_array().unwrap().len(), 0);

    // Loser: gone from disk AND DB. Keeper: both intact.
    assert!(
        !on_disk(&app, "Saga (dup)/Saga 1.cbr"),
        "loser file must be deleted"
    );
    assert!(
        !db_row_exists(&app, loser).await,
        "loser row must be deleted"
    );
    assert!(on_disk(&app, "Saga/Saga 1.cbz"), "kept file must survive");
    assert!(db_row_exists(&app, keep).await, "kept row must survive");
}

#[tokio::test]
async fn resolve_treats_already_missing_file_as_success() {
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    // Loser row exists but its file was never written to disk.
    let loser = seed_file(&app, issue, "Saga/Saga 1.cbr", true, 80_000_000, false).await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "resolved");
    // Already-missing disk file → still cleaned from DB, reported deleted.
    assert!(!db_row_exists(&app, loser).await);
    assert_eq!(r["failed"].as_array().unwrap().len(), 0);
}

// -------- resolve: refusal guards --------

#[tokio::test]
async fn resolve_refuses_when_keep_is_not_a_candidate() {
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let f1 = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let f2 = seed_file(&app, issue, "Saga (dup)/Saga 1.cbr", true, 80_000_000, true).await;

    // keep_file_id 999999 is not one of this issue's files.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":999999}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "refused");
    assert!(r["reason"].as_str().unwrap().contains("keep_file_id"));
    // Nothing deleted, on disk or in DB.
    assert!(on_disk(&app, "Saga/Saga 1.cbz") && on_disk(&app, "Saga (dup)/Saga 1.cbr"));
    assert!(db_row_exists(&app, f1).await && db_row_exists(&app, f2).await);
}

#[tokio::test]
async fn resolve_refuses_mismatched_issue_numbers() {
    let app = build_test_app().await;
    // Distinct issues (#1 and #2) wrongly matched to one issue row — the real
    // "Ferocious"/"Void Rivals" case. Filenames parse to different numbers.
    let issue = seed_series_issue(&app, "Ferocious", "1").await;
    let f1 = seed_file(
        &app,
        issue,
        "Ferocious/Ferocious 1.cbz",
        true,
        90_000_000,
        true,
    )
    .await;
    let f2 = seed_file(
        &app,
        issue,
        "Ferocious/Ferocious 2.cbz",
        true,
        90_000_000,
        true,
    )
    .await;

    // Detection classifies it as a mismatch (read-only).
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    assert_eq!(body["groups"][0]["kind"], "mismatch");
    assert_eq!(
        body["groups"][0]["suggested_keep_file_id"],
        serde_json::Value::Null
    );

    // And resolve independently refuses it — the hard safety net.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{f1}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "refused");
    assert!(r["reason"].as_str().unwrap().contains("mismatched"));
    // Both distinct issues' files survive.
    assert!(
        on_disk(&app, "Ferocious/Ferocious 1.cbz") && on_disk(&app, "Ferocious/Ferocious 2.cbz")
    );
    assert!(db_row_exists(&app, f1).await && db_row_exists(&app, f2).await);
}

#[tokio::test]
async fn resolve_refuses_group_with_fewer_than_two_present_files() {
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let only = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{only}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "refused");
    assert!(db_row_exists(&app, only).await && on_disk(&app, "Saga/Saga 1.cbz"));
}

#[tokio::test]
async fn resolve_deletes_all_losers_in_a_multi_copy_group() {
    // The feature's actual target: a comic split across 3+ folders.
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let l1 = seed_file(&app, issue, "dup-a/Saga 1.cbr", true, 80_000_000, true).await;
    let l2 = seed_file(&app, issue, "dup-b/Saga 1.cb7", true, 70_000_000, true).await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "resolved");
    let deleted: Vec<i64> = r["deleted_file_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&l1) && deleted.contains(&l2));
    assert!(!deleted.contains(&keep));
    assert_eq!(r["failed"].as_array().unwrap().len(), 0);

    // Both losers gone from disk AND DB; keeper fully intact.
    assert!(!on_disk(&app, "dup-a/Saga 1.cbr") && !db_row_exists(&app, l1).await);
    assert!(!on_disk(&app, "dup-b/Saga 1.cb7") && !db_row_exists(&app, l2).await);
    assert!(on_disk(&app, "Saga/Saga 1.cbz") && db_row_exists(&app, keep).await);
}

#[tokio::test]
async fn resolve_refuses_to_delete_a_non_contained_loser_path() {
    // A tampered/malformed `path_relative` with a `..` escape must never reach
    // remove_file. The basename still parses to the keeper's number, so the
    // group passes the mismatch guard and the loser reaches the delete loop —
    // where is_contained rejects it: reported in `failed`, DB row preserved.
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let evil = seed_file(&app, issue, "../Saga 1.cbr", true, 80_000_000, false).await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    // The group resolves, but the non-contained loser is refused, not deleted.
    assert_eq!(r["status"], "resolved");
    assert_eq!(r["deleted_file_ids"].as_array().unwrap().len(), 0);
    let failed = r["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["file_id"], evil);
    // Its DB row is preserved (nothing to retry-lose), keeper untouched.
    assert!(db_row_exists(&app, evil).await);
    assert!(on_disk(&app, "Saga/Saga 1.cbz") && db_row_exists(&app, keep).await);
}

#[tokio::test]
async fn resolve_batch_mixes_resolved_and_refused_without_cross_contamination() {
    let app = build_test_app().await;
    // Issue 1: a clean duplicate.
    let dup = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, dup, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let loser = seed_file(&app, dup, "dup/Saga 1.cbr", true, 80_000_000, true).await;
    // Issue 2: a mismatch (distinct issues).
    let mism = seed_series_issue(&app, "Ferocious", "1").await;
    let m1 = seed_file(
        &app,
        mism,
        "Ferocious/Ferocious 1.cbz",
        true,
        90_000_000,
        true,
    )
    .await;
    let m2 = seed_file(
        &app,
        mism,
        "Ferocious/Ferocious 2.cbz",
        true,
        90_000_000,
        true,
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(
                r#"{{"resolutions":[{{"issue_id":{dup},"keep_file_id":{keep}}},{{"issue_id":{mism},"keep_file_id":{m1}}}]}}"#
            ),
        ))
        .await;
    let results = &response_json(resp).await["results"];
    // Order preserved: the duplicate resolves, the mismatch is refused —
    // neither affects the other.
    assert_eq!(results[0]["status"], "resolved");
    assert_eq!(results[1]["status"], "refused");
    assert!(!on_disk(&app, "dup/Saga 1.cbr") && !db_row_exists(&app, loser).await);
    assert!(db_row_exists(&app, keep).await);
    assert!(db_row_exists(&app, m1).await && db_row_exists(&app, m2).await);
    assert!(
        on_disk(&app, "Ferocious/Ferocious 1.cbz") && on_disk(&app, "Ferocious/Ferocious 2.cbz")
    );
}

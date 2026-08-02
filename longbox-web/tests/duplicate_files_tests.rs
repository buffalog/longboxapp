//! Duplicate-file detector: detection query, the permanent-delete resolve
//! path, and the re-point ("correct") path for mismatch groups.
//!
//! Resolve deletes real user files, so the tests cover both the happy path
//! (loser gone from disk AND DB, keeper untouched) and every refusal guard
//! (keep-not-a-candidate, mismatched issue numbers).
//!
//! Correct deletes nothing, but it re-points real rows: a bad re-point
//! silently mis-files a comic and could hand the delete resolver a bogus
//! "duplicate" group later. So its guards get the same treatment — every
//! refusal is asserted to have written nothing.

mod common;

use axum::http::StatusCode;
use common::{build_test_app, empty_request, json_request, response_json, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};
use time::{OffsetDateTime, PrimitiveDateTime};

fn now_pdt() -> PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    PrimitiveDateTime::new(n.date(), n.time())
}

async fn seed_series(app: &TestApp, title: &str) -> i64 {
    series_repo::insert(
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
    .id
}

async fn seed_issue(app: &TestApp, series_id: i64, number: &str) -> i64 {
    issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id,
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

async fn seed_series_issue(app: &TestApp, title: &str, number: &str) -> i64 {
    let series = seed_series(app, title).await;
    seed_issue(app, series, number).await
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
    seed_file_with_bytes(
        app,
        issue_id,
        rel,
        is_present,
        size,
        write_disk.then_some(DEFAULT_BYTES),
    )
    .await
}

/// The bytes every fixture file gets unless a test asks for its own. Files
/// sharing these bytes are genuine content duplicates, which is what most
/// resolve tests want.
const DEFAULT_BYTES: &[u8] = b"stub-comic-bytes";

/// As [`seed_file`], but the caller chooses the bytes — `None` writes no file
/// at all. Content identity now drives every delete decision, so a fixture
/// that wants two DISTINCT issues has to write distinct bytes; identical
/// bytes are, correctly, a duplicate no matter what the filenames say.
///
/// Also stores the file's real BLAKE3 digest stamped with its on-disk
/// size/mtime, standing in for the background hash pass. Without it every
/// group would classify as `pending_analysis` and every resolve would refuse.
async fn seed_file_with_bytes(
    app: &TestApp,
    issue_id: i64,
    rel: &str,
    is_present: bool,
    size: i64,
    content: Option<&[u8]>,
) -> i64 {
    if let Some(bytes) = content {
        let full = app.library_path().join(rel);
        tokio::fs::create_dir_all(full.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&full, bytes).await.unwrap();
    }
    let now = now_pdt();
    let id = file_repo::insert(
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
    .id;
    if content.is_some() {
        let full = app.library_path().join(rel);
        let meta = std::fs::metadata(&full).unwrap();
        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(std::fs::File::open(&full).unwrap())
            .unwrap();
        let off =
            time::OffsetDateTime::from(meta.modified().unwrap()).to_offset(time::UtcOffset::UTC);
        file_repo::set_content_hash(
            &app.state.db,
            id,
            hasher.finalize().to_hex().as_ref(),
            meta.len() as i64,
            time::PrimitiveDateTime::new(off.date(), off.time()),
        )
        .await
        .unwrap();
    }
    id
}

/// A file whose bytes are unique to its path — i.e. a genuinely different
/// comic. Mismatch fixtures need this: two DISTINCT issues wrongly shelved
/// under one record are not duplicates, and with identical bytes they would
/// (correctly) be classified as duplicates instead.
async fn seed_distinct_file(
    app: &TestApp,
    issue_id: i64,
    rel: &str,
    is_present: bool,
    size: i64,
) -> i64 {
    seed_file_with_bytes(app, issue_id, rel, is_present, size, Some(rel.as_bytes())).await
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
    seed_file(&app, a, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    seed_file(&app, a, "Saga/Saga 1.cbr", true, 80_000_000, true).await;
    // Issue B: a single present file → not a group.
    let b = seed_series_issue(&app, "Nailbiter", "1").await;
    seed_file(&app, b, "Nailbiter/Nailbiter 1.cbz", true, 50_000_000, true).await;
    // Issue C: two files but one is absent → not a group (present count 1).
    let c = seed_series_issue(&app, "Paper Girls", "1").await;
    seed_file(
        &app,
        c,
        "Paper Girls/Paper Girls 1.cbz",
        true,
        50_000_000,
        true,
    )
    .await;
    seed_file(&app, c, "Paper Girls/old.cbz", false, 50_000_000, true).await;

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
        true,
    )
    .await;
    let big = seed_file(
        &app,
        issue,
        "The Darkness/The Darkness 1.cbr",
        true,
        100_000_000,
        true,
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
    let loser = seed_file(&app, issue, "Saga/Saga 1.cbr", true, 80_000_000, true).await;
    assert!(on_disk(&app, "Saga/Saga 1.cbz"));
    assert!(on_disk(&app, "Saga/Saga 1.cbr"));

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
        !on_disk(&app, "Saga/Saga 1.cbr"),
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
    let f2 = seed_file(&app, issue, "Saga/Saga 1.cbr", true, 80_000_000, true).await;

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
    assert!(on_disk(&app, "Saga/Saga 1.cbz") && on_disk(&app, "Saga/Saga 1.cbr"));
    assert!(db_row_exists(&app, f1).await && db_row_exists(&app, f2).await);
}

#[tokio::test]
async fn resolve_refuses_mismatched_issue_numbers() {
    let app = build_test_app().await;
    // Distinct issues (#1 and #2) wrongly matched to one issue row — the real
    // "Ferocious"/"Void Rivals" case. Filenames parse to different numbers.
    let issue = seed_series_issue(&app, "Ferocious", "1").await;
    let f1 = seed_distinct_file(&app, issue, "Ferocious/Ferocious 1.cbz", true, 90_000_000).await;
    let f2 = seed_distinct_file(&app, issue, "Ferocious/Ferocious 2.cbz", true, 90_000_000).await;

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
    assert!(
        r["reason"].as_str().unwrap().contains("different content"),
        "refusal must cite content, not filenames: {}",
        r["reason"]
    );
    // Both distinct issues' files survive.
    assert!(
        on_disk(&app, "Ferocious/Ferocious 1.cbz") && on_disk(&app, "Ferocious/Ferocious 2.cbz")
    );
    assert!(db_row_exists(&app, f1).await && db_row_exists(&app, f2).await);
}

/// The live data-loss bug, end to end.
///
/// Two volumes of "The Authority" each have a #4. The matcher bound both onto
/// the 1999 series' issue #4, and because both filenames parse to 4 the group
/// passed every guard that existed and presented as a textbook duplicate with
/// a "Delete 1 other" button. 26 of the 28 groups on the live library were
/// this shape.
///
/// The refusal has to hold on the WRITE path, not just in the GET payload — a
/// delete prevented only by the UI is not prevented.
#[tokio::test]
async fn resolve_refuses_two_volumes_of_one_title_even_though_numbers_agree() {
    let app = build_test_app().await;
    // TWO catalog rows share this title, as the live catalog has three. The
    // second row is what makes the differing folder years name different
    // volumes rather than one series spelled two ways.
    let issue = seed_series_issue(&app, "The Authority", "4").await;
    seed_series(&app, "The Authority").await;
    let v1999 = seed_file(
        &app,
        issue,
        "The Authority (1999)/The Authority 4.cbz",
        true,
        90_000_000,
        true,
    )
    .await;
    let v2008 = seed_file(
        &app,
        issue,
        "The Authority (2008)/The Authority 4.cbr",
        true,
        80_000_000,
        true,
    )
    .await;

    // Detection refuses to call it a duplicate and offers no keep.
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    assert_eq!(body["groups"][0]["kind"], "cross_folder_wrong_series");
    assert_eq!(
        body["groups"][0]["suggested_keep_file_id"],
        serde_json::Value::Null
    );

    // And a hand-rolled POST — bypassing the UI entirely — is refused too.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{v1999}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "refused");
    assert!(r["reason"]
        .as_str()
        .unwrap()
        .contains("more than one series folder"));

    // Both comics survive, on disk and in the catalog.
    assert!(
        on_disk(&app, "The Authority (1999)/The Authority 4.cbz")
            && on_disk(&app, "The Authority (2008)/The Authority 4.cbr"),
        "neither volume may be deleted"
    );
    assert!(db_row_exists(&app, v1999).await && db_row_exists(&app, v2008).await);
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
    let l1 = seed_file(&app, issue, "Saga/Saga 1.cbr", true, 80_000_000, true).await;
    let l2 = seed_file(&app, issue, "Saga/Saga 1.cb7", true, 70_000_000, true).await;

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
    assert!(!on_disk(&app, "Saga/Saga 1.cbr") && !db_row_exists(&app, l1).await);
    assert!(!on_disk(&app, "Saga/Saga 1.cb7") && !db_row_exists(&app, l2).await);
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
    let evil = seed_file(
        &app,
        issue,
        "Saga/../../Saga 1.cbr",
        true,
        80_000_000,
        false,
    )
    .await;

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

// -------- correct: re-pointing a mismatched file --------

/// The live Ferocious shape: issue rows #1–#5 all exist and are distinct, but
/// the files for #2 and #5 got matched onto #1's row because their embedded
/// ComicInfo `<Number>` wrongly says 1. Returns (series, [issue ids #1..#5]).
async fn seed_ferocious(app: &TestApp) -> (i64, Vec<i64>) {
    let series = seed_series(app, "Ferocious").await;
    let mut issues = Vec::new();
    for n in 1..=5 {
        issues.push(seed_issue(app, series, &n.to_string()).await);
    }
    (series, issues)
}

#[tokio::test]
async fn correct_repoints_a_mismatched_file_and_pins_it_against_rescan() {
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    let home = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001.cbz",
        true,
        90_000_000,
    )
    .await;
    let stray = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 002.cbz",
        true,
        90_000_000,
    )
    .await;

    // Detection surfaces it as a mismatch AND names the target for the stray.
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = &body["groups"][0];
    assert_eq!(g["kind"], "mismatch");
    let files = g["files"].as_array().unwrap();
    let stray_row = files.iter().find(|f| f["file_id"] == stray).unwrap();
    assert_eq!(stray_row["suggested_issue_id"], issues[1]);
    // The file already on its own issue gets no suggestion.
    let home_row = files.iter().find(|f| f["file_id"] == home).unwrap();
    assert_eq!(home_row["suggested_issue_id"], serde_json::Value::Null);

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{stray},"issue_id":{}}}"#, issues[1]),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let r = response_json(resp).await;
    assert_eq!(r["from_issue_id"], issues[0]);
    assert_eq!(r["to_issue_id"], issues[1]);

    // The row now points at #2 — and is stamped manual, so the next scan's
    // Tier 2 can't drag it back to #1 on the strength of the bad XML.
    let row = file_repo::find_by_id(&app.state.db, stray)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.issue_id, Some(issues[1]));
    assert_eq!(row.match_method, "manual");
    assert_eq!(row.status, "owned");
    // Nothing was deleted. This is a pointer fix, not a tidy-up.
    assert!(on_disk(&app, "Ferocious/Ferocious 002.cbz"));
    assert!(db_row_exists(&app, home).await && on_disk(&app, "Ferocious/Ferocious 001.cbz"));

    // The group is gone: each issue now holds exactly one present file.
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    assert_eq!(response_json(resp).await["total"], 0);
}

#[tokio::test]
async fn correct_refuses_a_target_the_filename_does_not_agree_with() {
    // The core guard: the client asks to move Ferocious 002 onto issue #5.
    // The file's own name says 2. Refuse — a blindly-POSTed issue_id must be
    // harmless.
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001.cbz",
        true,
        90_000_000,
    )
    .await;
    let stray = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 002.cbz",
        true,
        90_000_000,
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{stray},"issue_id":{}}}"#, issues[4]),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let row = file_repo::find_by_id(&app.state.db, stray)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.issue_id, Some(issues[0]), "nothing may be written");
}

#[tokio::test]
async fn correct_refuses_a_target_in_a_different_series() {
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001.cbz",
        true,
        90_000_000,
    )
    .await;
    let stray = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 002.cbz",
        true,
        90_000_000,
    )
    .await;
    // A #2 in a different series — same number, wrong book.
    let other = seed_series_issue(&app, "Saga", "2").await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{stray},"issue_id":{other}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let row = file_repo::find_by_id(&app.state.db, stray)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.issue_id, Some(issues[0]));
}

#[tokio::test]
async fn correct_refuses_when_the_target_already_holds_a_different_issue() {
    // Issue #2 is itself already mismatched (it holds a file for #3). Moving
    // our #2 in there would tangle it further — refuse, and don't suggest it.
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001.cbz",
        true,
        90_000_000,
    )
    .await;
    let stray = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 002.cbz",
        true,
        90_000_000,
    )
    .await;
    seed_file(
        &app,
        issues[1],
        "Ferocious/Ferocious 003.cbz",
        true,
        90_000_000,
        true,
    )
    .await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["issue_id"] == issues[0])
        .unwrap();
    let stray_row = g["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["file_id"] == stray)
        .unwrap();
    assert_eq!(stray_row["suggested_issue_id"], serde_json::Value::Null);

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{stray},"issue_id":{}}}"#, issues[1]),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let row = file_repo::find_by_id(&app.state.db, stray)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.issue_id, Some(issues[0]));
}

#[tokio::test]
async fn correct_allows_a_target_whose_occupant_agrees_on_the_number() {
    // Issue #2 already holds a genuine second copy of #2. Re-pointing our
    // stray there is correct: the result is an honest duplicate group, which
    // the delete resolver then handles.
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001.cbz",
        true,
        90_000_000,
    )
    .await;
    let stray = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 002.cbz",
        true,
        90_000_000,
    )
    .await;
    // Same BYTES as the stray: this is a genuine second copy of #2, which is
    // what makes re-pointing produce an honest, resolvable duplicate group.
    seed_file_with_bytes(
        &app,
        issues[1],
        "Ferocious/Ferocious 2.cbr",
        true,
        80_000_000,
        Some("Ferocious/Ferocious 002.cbz".as_bytes()),
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{stray},"issue_id":{}}}"#, issues[1]),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Issue #2 now has two present files that agree → a resolvable duplicate.
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["issue_id"], issues[1]);
    assert_eq!(groups[0]["kind"], "duplicate");
}

#[tokio::test]
async fn correct_refuses_a_no_op_and_404s_an_unknown_file() {
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    let f = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001.cbz",
        true,
        90_000_000,
    )
    .await;

    // Already there.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{f},"issue_id":{}}}"#, issues[0]),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown file.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":999999,"issue_id":{}}}"#, issues[1]),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Unknown issue.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{f},"issue_id":999999}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resolve_batch_mixes_resolved_and_refused_without_cross_contamination() {
    let app = build_test_app().await;
    // Issue 1: a clean duplicate.
    let dup = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, dup, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let loser = seed_file(&app, dup, "Saga/Saga 1.cbr", true, 80_000_000, true).await;
    // Issue 2: a mismatch (distinct issues).
    let mism = seed_series_issue(&app, "Ferocious", "1").await;
    let m1 = seed_distinct_file(&app, mism, "Ferocious/Ferocious 1.cbz", true, 90_000_000).await;
    let m2 = seed_distinct_file(&app, mism, "Ferocious/Ferocious 2.cbz", true, 90_000_000).await;

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
    assert!(!on_disk(&app, "Saga/Saga 1.cbr") && !db_row_exists(&app, loser).await);
    assert!(db_row_exists(&app, keep).await);
    assert!(db_row_exists(&app, m1).await && db_row_exists(&app, m2).await);
    assert!(
        on_disk(&app, "Ferocious/Ferocious 1.cbz") && on_disk(&app, "Ferocious/Ferocious 2.cbz")
    );
}

/// Regression: a scene-named file must never let two DISTINCT comics present
/// as a deletable duplicate.
///
/// `Ferocious.002.(2025).(Digital).cbz` is physically issue 2. The strict
/// filename patterns can't read that shape at all (verified: the raw parser
/// returns None), so the classifier used to fall back to the file's cached
/// ComicInfo `<Number>` — which, in this exact failure mode, wrongly says 1.
/// It would then "agree" with the genuine issue-1 file it had been mis-matched
/// onto, and the group would be served as `kind: duplicate` with a pre-filled
/// keep — one click from permanently deleting a real, distinct comic.
///
/// The number now comes from the filename alone, via the same normalizing
/// cascade the scanner's Tier 3 uses. The group stays a mismatch, the stray
/// gets a suggestion, and the delete resolver refuses it.
#[tokio::test]
async fn a_scene_named_file_is_never_mistaken_for_a_duplicate() {
    let app = build_test_app().await;
    let (_series, issues) = seed_ferocious(&app).await;
    let real_one = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious 001 (2025).cbz",
        true,
        90_000_000,
    )
    .await;
    let scene = seed_distinct_file(
        &app,
        issues[0],
        "Ferocious/Ferocious.002.(2025).(Digital).cbz",
        true,
        90_000_000,
    )
    .await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = &body["groups"][0];
    assert_eq!(
        g["kind"], "mismatch",
        "two distinct comics must not be offered for deletion"
    );
    assert_eq!(
        g["suggested_keep_file_id"],
        serde_json::Value::Null,
        "and must not get a pre-filled keep"
    );
    let files = g["files"].as_array().unwrap();
    let scene_row = files.iter().find(|f| f["file_id"] == scene).unwrap();
    assert_eq!(scene_row["parsed_number"], "002");
    assert_eq!(scene_row["suggested_issue_id"], issues[1]);

    // The resolver independently refuses it too.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(
                r#"{{"resolutions":[{{"issue_id":{},"keep_file_id":{real_one}}}]}}"#,
                issues[0]
            ),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "refused");
    assert!(
        db_row_exists(&app, scene).await
            && on_disk(&app, "Ferocious/Ferocious.002.(2025).(Digital).cbz")
    );
}

// -------- "the target issue does not exist" --------

/// The originating bug report, as a state rather than a shrug.
///
/// A file parsing to #5 in a series whose catalog stops at #4 has no move
/// target. The old UI reported that as "no confident suggestion — your call"
/// while instructing the user to "move each stray file to the issue its
/// filename says it is": an instruction to do something impossible. It is an
/// absence in the catalog, and it says so.
#[tokio::test]
async fn a_stray_naming_an_issue_the_series_lacks_is_reported_as_missing() {
    let app = build_test_app().await;
    let series = seed_series(&app, "Death Fight Forever").await;
    let issue_3 = seed_issue(&app, series, "3").await;
    for n in ["1", "2", "4"] {
        seed_issue(&app, series, n).await;
    }
    // Two DIFFERENT comics wrongly sharing issue 3, so the group is a
    // mismatch and the move path is live. One of them names #5, which the
    // catalog does not have.
    seed_distinct_file(
        &app,
        issue_3,
        "Death Fight Forever/Death Fight Forever 003.cbz",
        true,
        90_000_000,
    )
    .await;
    let stray = seed_distinct_file(
        &app,
        issue_3,
        "Death Fight Forever/Death Fight Forever 005.cbz",
        true,
        80_000_000,
    )
    .await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = &body["groups"][0];
    assert_eq!(g["kind"], "mismatch");
    let row = g["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["file_id"] == stray)
        .unwrap();
    assert_eq!(
        row["missing_target_number"], "5",
        "must name the issue the catalog lacks, not shrug"
    );
    assert_eq!(
        row["suggested_issue_id"],
        serde_json::Value::Null,
        "and there is genuinely nowhere to move it"
    );
}

/// THE REFRESH HAZARD.
///
/// Refresh Metadata creates issue records, and issue records are move
/// targets. A byte-identical copy must never be promoted into a move
/// candidate just because its number finally exists — taking that move would
/// mark the new issue owned while it holds another issue's content, which is
/// the phantom-ownership pattern this feature exists to eliminate, produced
/// by the repair action for it.
#[tokio::test]
async fn creating_the_missing_issue_does_not_make_an_identical_copy_movable() {
    let app = build_test_app().await;
    let series = seed_series(&app, "Death Fight Forever").await;
    let issue_3 = seed_issue(&app, series, "3").await;

    // The real shape: 005.cbz is a byte-identical copy of 003.cbz.
    let keep = seed_file(
        &app,
        issue_3,
        "Death Fight Forever/Death Fight Forever 003.cbz",
        true,
        90_000_000,
        true,
    )
    .await;
    let copy = seed_file(
        &app,
        issue_3,
        "Death Fight Forever/Death Fight Forever 005.cbz",
        true,
        90_000_000,
        true,
    )
    .await;

    // Identical content → duplicate, delete-only, no move UI.
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    assert_eq!(body["groups"][0]["kind"], "duplicate");

    // Now ComicVine gains issue 5 and a refresh creates the record — exactly
    // what the "Refresh Metadata" action does.
    let issue_5 = seed_issue(&app, series, "5").await;

    // The group must NOT reclassify: content identity outranks the fact that
    // a target now exists.
    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = &body["groups"][0];
    assert_eq!(
        g["kind"], "duplicate",
        "a new issue record must not promote a duplicate into a move candidate"
    );
    for f in g["files"].as_array().unwrap() {
        assert_eq!(
            f["suggested_issue_id"],
            serde_json::Value::Null,
            "no move may be suggested for byte-identical files"
        );
    }

    // And the write path refuses a hand-rolled POST that bypasses the UI.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/correct",
            format!(r#"{{"file_id":{copy},"issue_id":{issue_5}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_json(resp).await;
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("byte-identical"),
        "refusal must cite content identity: {}",
        body["error"]["message"]
    );

    // Nothing moved: both rows still on issue 3.
    for id in [keep, copy] {
        let row = file_repo::find_by_id(&app.state.db, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.issue_id, Some(issue_3), "file {id} must not have moved");
    }
}

// -------- cross-folder LABEL, resolved against the catalog --------

/// The live Hello Darkness case. Two folders, `(2024)` and `(2025)`, but the
/// catalog holds exactly ONE series by that name — so the differing year is
/// spelling, not a second volume.
///
/// Labelling it "Wrong series match" is a false statement, and a dangerous
/// one: it sends the user hunting for a duplicate series that does not exist,
/// and something destructive could come out of trying to reconcile it.
#[tokio::test]
async fn one_catalog_series_across_two_folders_is_labelled_same_series() {
    let app = build_test_app().await;
    let series = seed_series(&app, "Hello Darkness").await;
    let issue = seed_issue(&app, series, "1").await;
    let a = seed_file(
        &app,
        issue,
        "Hello Darkness (2024)/Hello Darkness 001.cbz",
        true,
        90_000_000,
        true,
    )
    .await;
    let b = seed_file(
        &app,
        issue,
        "Hello Darkness (2025)/Hello Darkness 001.cbz",
        true,
        90_000_000,
        true,
    )
    .await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    let g = &body["groups"][0];
    assert_eq!(
        g["kind"], "cross_folder_same_series",
        "one catalog series → the folder difference is spelling, not a volume"
    );

    // The guard is unchanged: still not deletable, even byte-identical.
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{a}}}]}}"#),
        ))
        .await;
    let r = &response_json(resp).await["results"][0];
    assert_eq!(r["status"], "refused", "wording changed; the guard did not");
    assert!(r["reason"]
        .as_str()
        .unwrap()
        .contains("more than one series folder"));
    assert!(db_row_exists(&app, a).await && db_row_exists(&app, b).await);
}

/// The Authority case, same folder shape, opposite catalog. Three rows share
/// the title, so the differing folder year DOES name a different volume and
/// the strong wording is correct.
#[tokio::test]
async fn several_catalog_volumes_across_two_folders_keeps_the_wrong_series_label() {
    let app = build_test_app().await;
    // Two catalog rows sharing a normalized title.
    let old = seed_series(&app, "The Authority").await;
    let _new = seed_series(&app, "The Authority").await;
    let issue = seed_issue(&app, old, "4").await;
    seed_file(
        &app,
        issue,
        "The Authority (1999)/The Authority 004.cbz",
        true,
        90_000_000,
        true,
    )
    .await;
    seed_file(
        &app,
        issue,
        "The Authority (2008)/The Authority 004.cbr",
        true,
        80_000_000,
        true,
    )
    .await;

    let resp = app
        .request(empty_request("GET", "/api/library/tidy/duplicate-files"))
        .await;
    let body = response_json(resp).await;
    assert_eq!(
        body["groups"][0]["kind"], "cross_folder_wrong_series",
        "more than one catalog volume by this name → the year names a volume"
    );
}

/// The one test that can distinguish the two possible orderings.
///
/// Everything else here asserts the end state — row gone AND file gone
/// — which either order satisfies, so from PR #32 until the shared
/// operation landed nothing constrained it. This makes the *second*
/// write fail and asserts the *first* one happened anyway.
///
/// Row-first (correct): the catalog row is gone, the bytes survive as
/// an orphan, and the failure is reported. Bytes-first (the previous
/// behaviour): the unlink fails, the function returns early, and the
/// row is still there — leaving a row whose issue stays `owned`, which
/// is the state that silently defeats the revert-to-missing payoff.
#[tokio::test]
async fn row_is_gone_even_when_the_unlink_fails() {
    use std::os::unix::fs::PermissionsExt;

    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;
    let keep = seed_file(&app, issue, "Saga/Saga 1.cbz", true, 90_000_000, true).await;
    let loser = seed_file(&app, issue, "Saga/Saga 1.cbr", true, 80_000_000, true).await;

    // Remove write permission from the containing directory: the file
    // itself stays readable, but it cannot be unlinked from it.
    let dir = app.library_path().join("Saga");
    let original = std::fs::metadata(&dir).unwrap().permissions();
    let mut locked = original.clone();
    locked.set_mode(0o555);
    std::fs::set_permissions(&dir, locked).unwrap();

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    let status = resp.status();
    let body = response_json(resp).await;

    // Restore before asserting, so a failure doesn't leave an
    // undeletable temp directory behind.
    std::fs::set_permissions(&dir, original).unwrap();

    assert_eq!(status, StatusCode::OK);

    // Self-detecting skip: if the unlink succeeded anyway (running as
    // root, or a filesystem that ignores the mode) then the second
    // write did not fail and there is no ordering to observe. Keyed on
    // the observed outcome rather than on guessing the cause.
    if !on_disk(&app, "Saga/Saga 1.cbr") {
        eprintln!("skipped: the environment allowed the unlink, so ordering is unobservable here");
        return;
    }

    assert!(
        !db_row_exists(&app, loser).await,
        "the catalog row must be gone even though the unlink failed — \
         a surviving row keeps the issue owned and the issue never reverts to missing"
    );
    assert!(
        on_disk(&app, "Saga/Saga 1.cbr"),
        "the bytes should still be present; this is the orphan case reconciliation reports"
    );

    let r = &body["results"][0];
    // `orphaned`, not `failed`. The row IS gone, so the duplicate is
    // resolved as far as the library is concerned and there is nothing
    // to retry — what remains is bytes on disk for the next scan to
    // surface. Reporting it as "could not be deleted" would name the
    // wrong problem and invite a retry against a row that no longer
    // exists.
    assert_eq!(
        r["failed"].as_array().unwrap().len(),
        0,
        "a failed unlink is not a failed delete: {r}"
    );
    let orphaned = r["orphaned"].as_array().unwrap();
    assert_eq!(
        orphaned.len(),
        1,
        "the orphaned file must be reported, not swallowed: {r}"
    );
    assert_eq!(orphaned[0]["file_id"], loser);

    assert!(on_disk(&app, "Saga/Saga 1.cbz"), "kept file must survive");
    assert!(db_row_exists(&app, keep).await, "kept row must survive");
}

/// Tidy's half of the alias hole: the keeper and a loser are the same
/// file reached by two names.
///
/// `metadata()` follows symlinks, so both stat identically — same size,
/// same mtime, same validated digest — and the keep-one guard reads
/// them as two copies. Deleting the "loser" then destroys the bytes the
/// keeper points at, leaving a kept row resolving to nothing.
///
/// Reachable without any symlink too: one file under two library roots
/// produces the same alias pair. The scanner walks `follow_links(true)`,
/// so anything aliased inside a root is guaranteed to present as a
/// content-duplicate group.
#[tokio::test]
async fn resolve_refuses_a_group_whose_files_are_the_same_file_aliased() {
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Saga", "1").await;

    // Real file with real bytes and a real digest, then a symlink to it
    // seeded the same way. Both must be content-analysed, or the
    // resolve refuses as `Unknown` before ever reaching the alias
    // check — which is how the first version of this test passed while
    // the guard was disabled.
    let keep = seed_file_with_bytes(&app, issue, "Saga/Saga 1.cbz", true, 5, Some(b"bytes")).await;
    std::os::unix::fs::symlink(
        app.library_path().join("Saga/Saga 1.cbz"),
        app.library_path().join("Saga/Saga 1.cbr"),
    )
    .unwrap();
    // No `content` — the bytes already exist through the link — but the
    // digest is stamped from the resolved file, exactly as analyze
    // would see it.
    let alias = seed_file_with_bytes(&app, issue, "Saga/Saga 1.cbr", true, 5, None).await;
    {
        let full = app.library_path().join("Saga/Saga 1.cbr");
        let meta = std::fs::metadata(&full).unwrap();
        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(std::fs::File::open(&full).unwrap())
            .unwrap();
        let off =
            time::OffsetDateTime::from(meta.modified().unwrap()).to_offset(time::UtcOffset::UTC);
        file_repo::set_content_hash(
            &app.state.db,
            alias,
            hasher.finalize().to_hex().as_ref(),
            meta.len() as i64,
            time::PrimitiveDateTime::new(off.date(), off.time()),
        )
        .await
        .unwrap();
    }

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let r = &response_json(resp).await["results"][0];

    assert_eq!(
        r["status"], "refused",
        "nothing here is a redundant copy of anything: {r}"
    );
    assert!(
        on_disk(&app, "Saga/Saga 1.cbz"),
        "the only real bytes must survive"
    );
    assert!(db_row_exists(&app, keep).await);
    assert!(db_row_exists(&app, alias).await);
}

/// A loser whose path is not a regular file must still face the content
/// verdict, not slip under the threshold that triggers it.
///
/// The content gate only runs when at least two present files are in
/// hand. An earlier version reported a non-file as ABSENT — the same
/// answer as "nothing is there" — which dropped it from that count, took
/// the group under the threshold, and skipped the entire
/// Identical/Distinct/Unknown block. The keeper rail still passed,
/// because the keeper was a real file, so a resolve that had correctly
/// refused on differing content began deleting instead.
///
/// Present-but-unreadable keeps it in the count and refuses: a
/// directory has no digest, and `classify_content` answers `Unknown`
/// on a missing digest and `Distinct` on a size mismatch.
///
/// The fixture is built so that the SIZE proof cannot be what refuses
/// it. `classify_content` checks sizes before digests, so a directory
/// of the usual few dozen bytes against a 5-byte comic returns
/// `Distinct` whether or not a non-file is special-cased at all — and
/// the first version of this test passed for exactly that reason,
/// constraining nothing. Here the keeper is padded to the directory's
/// own `st_size` and the directory row is stamped with the keeper's
/// digest against the directory's real stat. Without the non-file
/// branch that group classifies `Identical` and the resolve DELETES.
#[tokio::test]
async fn resolve_refuses_when_a_loser_is_not_a_regular_file() {
    let app = build_test_app().await;
    let issue = seed_series_issue(&app, "Directory", "1").await;

    // A catalogued row whose path is a DIRECTORY holding a real comic.
    std::fs::create_dir_all(app.library_path().join("Dir/Dir 1.cbr")).unwrap();
    std::fs::write(app.library_path().join("Dir/Dir 1.cbr/inner.cbz"), b"bytes").unwrap();
    let dir_meta = std::fs::metadata(app.library_path().join("Dir/Dir 1.cbr")).unwrap();
    let dir_size = dir_meta.len();

    // Keeper padded to the directory's own size, so the size proof is
    // silent and only the digest can decide.
    let padded = vec![b'x'; usize::try_from(dir_size).unwrap()];
    let keep = seed_file_with_bytes(
        &app,
        issue,
        "Dir/Dir 1.cbz",
        true,
        dir_size as i64,
        Some(&padded),
    )
    .await;
    let loser =
        seed_file_with_bytes(&app, issue, "Dir/Dir 1.cbr", true, dir_size as i64, None).await;
    // Stamp the directory row with the KEEPER's digest, against the
    // directory's real size and mtime so the stamp reads as fresh.
    {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&padded);
        let off = time::OffsetDateTime::from(dir_meta.modified().unwrap())
            .to_offset(time::UtcOffset::UTC);
        file_repo::set_content_hash(
            &app.state.db,
            loser,
            hasher.finalize().to_hex().as_ref(),
            dir_size as i64,
            time::PrimitiveDateTime::new(off.date(), off.time()),
        )
        .await
        .unwrap();
    }

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicate-files/resolve",
            format!(r#"{{"resolutions":[{{"issue_id":{issue},"keep_file_id":{keep}}}]}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let r = &response_json(resp).await["results"][0];

    assert_eq!(
        r["status"], "refused",
        "a non-file carries no bytes to compare, so this cannot be a keep-one decision: {r}"
    );
    assert!(
        db_row_exists(&app, loser).await,
        "the loser row must survive — it was never content-verified"
    );
    assert!(
        app.library_path().join("Dir/Dir 1.cbr/inner.cbz").exists(),
        "and the directory contents must be untouched"
    );
    assert!(db_row_exists(&app, keep).await);
}

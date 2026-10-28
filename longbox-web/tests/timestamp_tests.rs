//! Every timestamp the API sends must be one a browser's `Date` parses the
//! same way everywhere: ECMA-262's Date Time String Format
//! (`YYYY-MM-DDTHH:mm:ss[.sss]Z` or `±HH:mm`). V8 is lenient (it reads
//! `2026-10-28 15:59:00` as LOCAL time and accepts six-digit years); Safari
//! rejects both. So the check is the spec grammar, not `Date.parse`.

mod common;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use common::{build_test_app, empty_request, response_json};
use longbox_db::{issue_repo, pull_attempt_repo, pull_list_repo, series_repo};
use longbox_db::{NewIssue, NewPullAttempt, NewPullEntry, NewSeries};
use longbox_postprocess::{InterventionReason, PendingIntervention};
use serde_json::Value;

/// ECMA-262 §21.4.1.32 Date Time String Format, date-time form with
/// seconds: `YYYY-MM-DDTHH:mm:ss`, optional `.sss`, then `Z` or `±HH:mm`.
fn is_browser_datetime(s: &str) -> bool {
    let shape = |s: &str, tpl: &str| {
        s.len() == tpl.len()
            && s.bytes().zip(tpl.bytes()).all(|(c, t)| match t {
                b'd' => c.is_ascii_digit(),
                b's' => c == b'+' || c == b'-',
                _ => c == t,
            })
    };
    let (head, rest) = s.split_at(s.len().min(19));
    if !shape(head, "dddd-dd-ddTdd:dd:dd") {
        return false;
    }
    let rest = rest.strip_prefix('.').map_or(Some(rest), |r| {
        (r.len() >= 3 && r.as_bytes()[..3].iter().all(u8::is_ascii_digit)).then(|| &r[3..])
    });
    let zone_ok = rest.is_some_and(|z| z == "Z" || shape(z, "sdd:dd"));
    zone_ok
        && time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_ok()
}

/// Strings that are trying to be a date-time: optional sign, 4-6 digit
/// year, `-MM-DD`, then `T` or a space and an hour. Date-only values
/// (`2026-10-28`) are a different, valid form and aren't checked here.
fn looks_like_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    let start = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let digits = b[start..].iter().take_while(|c| c.is_ascii_digit()).count();
    let i = start + digits;
    (4..=6).contains(&digits)
        && b.len() > i + 9
        && b[i] == b'-'
        && b[i + 3] == b'-'
        && matches!(b[i + 6], b'T' | b' ')
        && b[i + 7].is_ascii_digit()
}

fn collect(v: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => m
            .iter()
            .for_each(|(k, v)| collect(v, &format!("{path}.{k}"), out)),
        Value::Array(a) => a.iter().for_each(|v| collect(v, &format!("{path}[]"), out)),
        Value::String(s) if looks_like_datetime(s) => out.push((path.to_owned(), s.clone())),
        _ => {}
    }
}

#[test]
fn the_grammar_matches_what_browsers_agree_on() {
    for ok in [
        "2026-10-28T15:59:00Z",
        "2026-10-28T15:59:00.123Z",
        "2026-10-28T08:59:00-07:00",
    ] {
        assert!(is_browser_datetime(ok), "{ok}");
    }
    for bad in [
        "+002026-10-28T14:59:06.007996593Z", // six-digit year (iso8601 serde)
        "2026-10-28 16:18:17.44200425",      // time's default PrimitiveDateTime
        "2026-10-28 16:18:17.0 +00:00:00",   // time's default OffsetDateTime
        "2026-10-28T16:18:17",               // no zone: local in some engines
        "2026-02-30T00:00:00Z",              // not a date
    ] {
        assert!(looks_like_datetime(bad), "{bad} must be checked");
        assert!(!is_browser_datetime(bad), "{bad}");
    }
    assert!(!looks_like_datetime("2026-10-28"));
}

fn write_cbz(path: &Path, comic_info: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("page.jpg", opts).unwrap();
    zip.write_all(b"\xFF\xD8\xFF").unwrap();
    zip.start_file("ComicInfo.xml", opts).unwrap();
    zip.write_all(comic_info.as_bytes()).unwrap();
    zip.finish().unwrap();
}

#[tokio::test]
async fn every_endpoint_sends_browser_parseable_timestamps() {
    let app = build_test_app().await;
    let db = &app.state.db;
    let new_series = |title: &str, cv_id| NewSeries {
        cv_id,
        metron_id: None,
        title: title.into(),
        sort_title: title.to_lowercase(),
        start_year: Some(2012),
        publisher: Some("Image".into()),
        description: None,
        cover_url: None,
    };
    let new_issue = |series_id, number: &str, cv_issue_id| NewIssue {
        series_id,
        cv_issue_id,
        metron_issue_id: None,
        number: number.into(),
        title: None,
        cover_date: Some("2012-03-14".into()),
        summary: None,
        cover_url: None,
        store_date: None,
    };

    // An owned, scanned issue: files, scans, health, dashboard matches.
    let saga = series_repo::insert(db, new_series("Saga", Some(1)))
        .await
        .unwrap()
        .id;
    let owned = issue_repo::insert(db, new_issue(saga, "1", Some(101)))
        .await
        .unwrap()
        .id;
    write_cbz(
        &app.library_path().join("Saga/Saga 001.cbz"),
        "<ComicInfo><Series>Saga</Series><Number>1</Number>\
         <Web>https://comicvine.gamespot.com/issue/4000-101/</Web></ComicInfo>",
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    // A missing issue on the pull list, with a failed pull and a refresh.
    let missing = issue_repo::insert(db, new_issue(saga, "2", None))
        .await
        .unwrap()
        .id;
    pull_list_repo::add(
        db,
        NewPullEntry {
            series_id: saga,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    pull_attempt_repo::insert(
        db,
        NewPullAttempt {
            series_id: saga,
            issue_id: missing,
            indexer_id: None,
            release_id: None,
            status: "failed".into(),
            error_message: Some("boom".into()),
            retry_count: 3,
            download_handle: None,
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE pull_list SET last_pull_attempt_at = CURRENT_TIMESTAMP, \
         last_successful_pull_at = CURRENT_TIMESTAMP",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query("UPDATE series SET last_enrichment_attempt_at = CURRENT_TIMESTAMP")
        .execute(db)
        .await
        .unwrap();
    app.state.pending_cache.push(PendingIntervention {
        source_path: "/watch/Saga 002.cbz".into(),
        target_path: "/comics/Saga (2012)/Saga (2012) 002.cbz".into(),
        reason: InterventionReason::Conflict,
        size: 5,
        last_attempt: time::OffsetDateTime::now_utc(),
        issue_id: Some(missing),
    });
    let file_id = response_json(app.request(empty_request("GET", "/api/files")).await).await[0]
        ["id"]
        .as_i64()
        .unwrap();

    let routes = [
        "/api/dashboard/activity".to_owned(),
        "/api/files".to_owned(),
        format!("/api/files/{file_id}"),
        "/api/health".to_owned(),
        format!("/api/issues/{owned}"),
        "/api/indexers".to_owned(),
        "/api/library-roots".to_owned(),
        "/api/library/tidy/duplicate-files".to_owned(),
        "/api/library/tidy/duplicates".to_owned(),
        "/api/missing".to_owned(),
        "/api/needs-attention/pull-failures".to_owned(),
        "/api/postprocess/pending".to_owned(),
        "/api/publishers/filters".to_owned(),
        "/api/pull-list".to_owned(),
        "/api/pull-list/export".to_owned(),
        "/api/reconcile/phantoms".to_owned(),
        "/api/reconcile/untracked".to_owned(),
        "/api/refresh/status".to_owned(),
        "/api/scans/current".to_owned(),
        "/api/scans/recent".to_owned(),
        "/api/series".to_owned(),
        format!("/api/series/{saga}"),
        "/api/settings".to_owned(),
        "/api/webhooks".to_owned(),
    ];
    let mut seen = BTreeSet::new();
    let mut bad = Vec::new();
    for route in &routes {
        let resp = app.request(empty_request("GET", route)).await;
        assert!(resp.status().is_success(), "{route}: {}", resp.status());
        let mut found = Vec::new();
        collect(&response_json(resp).await, "", &mut found);
        for (path, value) in found {
            if !is_browser_datetime(&value) {
                bad.push(format!("{route} {path} = {value}"));
            }
            seen.insert(format!("{route} {path}"));
        }
    }
    assert!(bad.is_empty(), "not browser-parseable:\n{}", bad.join("\n"));

    // Not vacuous: the fields the live API was sending wrong (2026-10-28).
    let series = format!("/api/series/{saga}");
    let issue = format!("/api/issues/{owned}");
    for want in [
        "/api/dashboard/activity .recent_matches[].matched_at",
        "/api/dashboard/activity .recent_series[].created_at",
        "/api/files [].mtime",
        "/api/files [].matched_at",
        "/api/health .last_scan_at",
        "/api/health .last_enrichment_at",
        &format!("{issue} .created_at"),
        "/api/library-roots [].created_at",
        "/api/missing .missing[].issue_created_at",
        "/api/needs-attention/pull-failures [].attempted_at",
        "/api/postprocess/pending .items[].last_attempt",
        "/api/publishers/filters [].created_at",
        "/api/pull-list [].added_at",
        "/api/pull-list [].last_pull_attempt_at",
        "/api/pull-list [].last_successful_pull_at",
        "/api/pull-list/export [].subscribed_at",
        "/api/scans/recent [].started_at",
        "/api/scans/recent [].finished_at",
        "/api/series [].created_at",
        &format!("{series} .issues[].updated_at"),
    ] {
        assert!(seen.contains(want), "never saw {want}; saw {seen:#?}");
    }
}

/// Every datetime field on a `Serialize` struct carries the rfc3339
/// attribute, so a new endpoint can't fall back to `time`'s default.
#[test]
fn every_serialized_datetime_field_uses_rfc3339() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    let mut dirs: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path().join("src")))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    let is_dt_field = |l: &str| {
        let l = l.trim();
        !l.starts_with("//")
            && l.ends_with(',')
            && l.contains(':')
            && ["PrimitiveDateTime", "OffsetDateTime"].iter().any(|t| {
                let ty = l.split_once(':').unwrap().1.trim().trim_end_matches(',');
                let ty = ty
                    .strip_prefix("Option<")
                    .map_or(ty, |t| t.trim_end_matches('>'));
                ty.trim_start_matches("time::") == *t
            })
    };
    let mut missing = Vec::new();
    let mut checked = 0;
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        let mut serialize = false;
        let mut end: Option<String> = None;
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim();
            if let Some(close) = &end {
                if *l == close.as_str() {
                    end = None;
                } else if is_dt_field(l) {
                    checked += 1;
                    let attrs = lines[..i]
                        .iter()
                        .rev()
                        .take_while(|p| p.trim().starts_with("#[") || p.trim().starts_with("//"));
                    if !attrs.into_iter().any(|a| a.contains("rfc3339")) {
                        missing.push(format!("{}:{}: {t}", f.display(), i + 1));
                    }
                }
                continue;
            }
            if t.starts_with("#[derive(") {
                serialize = t.contains("Serialize");
            } else if serialize && t.contains("struct ") && t.ends_with('{') {
                end = Some(format!("{}}}", &l[..l.len() - l.trim_start().len()]));
                serialize = false;
            } else if !t.starts_with("#[") && !t.starts_with("//") {
                serialize = false;
            }
        }
    }
    assert!(
        checked >= 40,
        "only {checked} fields checked: the scan is broken"
    );
    assert!(
        missing.is_empty(),
        "add #[serde(with = \"longbox_core::rfc3339\")]:\n{}",
        missing.join("\n")
    );
}

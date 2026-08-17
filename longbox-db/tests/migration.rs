mod common;

use common::fresh_pool;

#[tokio::test]
async fn migration_creates_all_tables() {
    let pool = fresh_pool().await;
    let rows = sqlx::query!(
        r#"SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' ORDER BY name"#
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = rows
        .into_iter()
        .map(|r| r.name.unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec![
            "creators".to_string(),
            "cv_release_cache".to_string(),
            "cv_volume_cache".to_string(),
            "discovered_folders".to_string(),
            "downloader_config".to_string(),
            "files".to_string(),
            "indexer_configs".to_string(),
            "issue_credits".to_string(),
            "issues".to_string(),
            "library_roots".to_string(),
            "metron_calendar_cache".to_string(),
            "opds_users".to_string(),
            "parsing_patterns".to_string(),
            "publisher_filters".to_string(),
            "pull_attempts".to_string(),
            "pull_list".to_string(),
            "reading_progress".to_string(),
            "scan_runs".to_string(),
            "series".to_string(),
            "settings".to_string(),
            "webhook_configs".to_string(),
        ]
    );
}

#[tokio::test]
async fn migration_creates_all_indexes() {
    let pool = fresh_pool().await;
    let rows = sqlx::query!(
        r#"SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name"#
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = rows
        .into_iter()
        .map(|r| r.name.unwrap_or_default())
        .collect();
    assert!(names.contains(&"idx_series_sort_title".to_string()));
    assert!(names.contains(&"idx_issues_series".to_string()));
    assert!(names.contains(&"idx_files_issue".to_string()));
    assert!(names.contains(&"idx_files_status".to_string()));
    assert!(names.contains(&"idx_files_match_method".to_string()));
    assert!(names.contains(&"idx_creators_name".to_string()));
    assert!(names.contains(&"idx_issue_credits_creator".to_string()));
    assert!(names.contains(&"idx_issue_credits_issue".to_string()));
}

#[tokio::test]
async fn seed_settings_row_present() {
    let pool = fresh_pool().await;
    let value = longbox_db::settings_repo::get(&pool, longbox_db::KEY_MATCH_CONFIDENCE_THRESHOLD)
        .await
        .unwrap();
    assert_eq!(value.as_deref(), Some("0.85"));
}

/// The seeded patterns must claim a `.pdf` filename, not just `.cbz`/`.cbr`.
///
/// Asserted here rather than only end-to-end in Phase B because this is where
/// it can break: the patterns are seeded and rewritten across ten migrations,
/// and a new one that forgets `.pdf` would surface as an unrelated import
/// failure two crates away. The stakes are higher for PDFs than for the
/// archive formats — a PDF has no embedded ComicInfo to fall back on, so a
/// pattern that stops at the extension doesn't weaken its match, it ends it.
#[tokio::test]
async fn seeded_parsing_patterns_claim_pdf_filenames() {
    let pool = fresh_pool().await;
    let patterns: Vec<longbox_core::ParsingPattern> =
        longbox_db::parsing_pattern_repo::list_enabled(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| longbox_core::ParsingPattern {
                id: r.id,
                name: r.name,
                pattern: r.pattern,
                priority: i32::try_from(r.priority).unwrap(),
                enabled: r.enabled,
            })
            .collect();

    let parsed = longbox_core::filename::parse("Saga 001 (2012).pdf", &patterns)
        .expect("no seeded pattern claims a .pdf filename");
    assert_eq!(parsed.series_title, "Saga");
    assert_eq!(parsed.number, "001");
    assert_eq!(parsed.year, Some(2012));
}

#[tokio::test]
async fn seed_parsing_patterns_at_correct_priorities() {
    // The original four (5/10/20/30), the A.9 parser hot-fix three
    // (11/12/15) for year-before-number / subtitled / part-of-N
    // shapes, the A.9 Bug 1b three (6/7/25) for TPB-vN / TPB-Book /
    // permissive-date-stamp shapes, the Tier 4 `(of N)` three
    // (13/14/28) for part-of-mini markers, and the SAB-vNN-no-subtitle
    // pattern (priority 9, id=14) for `Series vNN (YYYY)` collected
    // editions like `EC - Cruel Universe v01 (2025)`. Update both this
    // assertion and `longbox-core::filename::default_patterns` together
    // when adding more.
    let pool = fresh_pool().await;
    let patterns = longbox_db::parsing_pattern_repo::list_enabled(&pool)
        .await
        .unwrap();
    let prio_names: Vec<(i64, &str)> = patterns
        .iter()
        .map(|p| (p.priority, p.name.as_str()))
        .collect();
    assert_eq!(
        prio_names,
        vec![
            (5, "Series Vol N #M"),
            (6, "Series vN - Subtitle (YYYY)"),
            (7, "Series Book N (YYYY)"),
            (9, "Series vNN (YYYY) no subtitle"),
            (10, "Series #NNN (YYYY)"),
            (11, "Series N (Xf Y) (YYYY)"),
            (12, "Series N - Subtitle (YYYY)"),
            (13, "Series N (of M) (YYYY)"),
            (14, "Series (YYYY) N (of M)"),
            (15, "Series (YYYY) NNN"),
            (20, "Series NNN (YYYY)"),
            (25, "Series NNN (YYYY[-MM]) permissive"),
            (28, "Series N (of M) yearless"),
            (30, "Series_NNN or Series NNN"),
        ]
    );
}

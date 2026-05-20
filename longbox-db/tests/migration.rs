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
            "cv_release_cache".to_string(),
            "downloader_config".to_string(),
            "files".to_string(),
            "indexer_configs".to_string(),
            "issues".to_string(),
            "library_roots".to_string(),
            "parsing_patterns".to_string(),
            "publisher_filters".to_string(),
            "pull_attempts".to_string(),
            "pull_list".to_string(),
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
}

#[tokio::test]
async fn seed_settings_row_present() {
    let pool = fresh_pool().await;
    let value = longbox_db::settings_repo::get(&pool, longbox_db::KEY_MATCH_CONFIDENCE_THRESHOLD)
        .await
        .unwrap();
    assert_eq!(value.as_deref(), Some("0.85"));
}

#[tokio::test]
async fn seed_parsing_patterns_at_correct_priorities() {
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
            (10, "Series #NNN (YYYY)"),
            (20, "Series NNN (YYYY)"),
            (30, "Series_NNN or Series NNN"),
        ]
    );
}

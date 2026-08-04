//! Recall measurement — how much of the missing list is actually
//! available and being hidden by the search path?
//!
//! # THIS MAKES LIVE NETWORK CALLS WITH REAL CREDENTIALS
//!
//! It queries every enabled indexer over the internet, authenticating
//! with the API keys stored in the catalog you point it at. It is
//! `#[ignore]`d and therefore **never runs in CI** and never runs under
//! a plain `cargo test`; it only runs when invoked by name with
//! `--ignored`, as below.
//!
//! Nothing is embedded here. Indexer base URLs and API keys come from
//! `indexer_config_repo` (the `indexer_configs` table), thresholds and
//! exclusion keywords from `settings`, and the catalog path from the
//! `LONGBOX_MEASURE_DB` environment variable. Point it at a COPY of the
//! live DB — `longbox_db::open` runs migrations against whatever it is
//! given.
//!
//! It is read-only with respect to the catalog and the indexers: it
//! searches, it never grabs.
//!
//! # Why it exists
//!
//! This is how the pull-search path's recall gets re-checked — the
//! before/after numbers in the ladder work came from here, and the only
//! way to know whether a future change helps or hurts is to run it
//! again. Reading the code cannot answer it; the indexers' behaviour is
//! the variable.
//!
//! Run explicitly:
//!
//! ```text
//! LONGBOX_MEASURE_DB=/path/to/longbox.db \
//! LONGBOX_MEASURE_OUT=/tmp/before.tsv \
//!   cargo test -p longbox-pull --test recall_measurement -- --ignored --nocapture
//! ```
//!
//! Reports per issue: candidates returned, candidates parsed, best
//! similarity, and grabbable yes/no — taken from the SAME
//! `find_release_excluding_filtered` the engine calls, so the numbers
//! describe production behaviour rather than a reconstruction of it.

use longbox_db::{indexer_config_repo, parsing_pattern_repo, series_repo, settings_repo};
use longbox_newznab::{FindOutcome, IndexerConfig, IndexerId};

/// Issues to sample. Spread across several series deliberately — a
/// single series would measure one title's indexer coverage, not the
/// search path.
/// Stratified: at most two issues per series, so the sample measures the
/// search path across many titles rather than one title's indexer
/// coverage. An unstratified `LIMIT 24` ordered by title spent the whole
/// budget on the first three series.
const SAMPLE_SQL: &str = r#"
WITH missing AS (
    SELECT s.id AS sid, s.title AS stitle, i.id AS iid, i.number AS inum,
           i.cover_date AS icover,
           ROW_NUMBER() OVER (PARTITION BY s.id ORDER BY CAST(i.number AS INTEGER)) AS rn
    FROM issues i JOIN series s ON s.id = i.series_id
    WHERE NOT EXISTS (
        SELECT 1 FROM files f
        WHERE f.issue_id = i.id AND f.status = 'owned' AND f.is_present = 1
    )
)
SELECT sid, stitle, iid, inum, icover FROM missing
WHERE rn <= 2
ORDER BY (stitle LIKE '%Brother Lono%') DESC, stitle, CAST(inum AS INTEGER)
"#;

#[tokio::test]
#[ignore]
async fn measure_recall() {
    let db_path = std::env::var("LONGBOX_MEASURE_DB").expect("LONGBOX_MEASURE_DB");
    let out_path = std::env::var("LONGBOX_MEASURE_OUT").expect("LONGBOX_MEASURE_OUT");
    let limit: usize = std::env::var("LONGBOX_MEASURE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);

    let db = longbox_db::open(&db_path).await.expect("open catalog");

    let indexers: Vec<IndexerConfig> = indexer_config_repo::list_enabled(&db)
        .await
        .expect("indexers")
        .into_iter()
        .map(|r| IndexerConfig {
            id: IndexerId(r.id),
            name: r.name,
            base_url: r.base_url,
            api_key: r.api_key,
            priority: r.priority as i32,
            maxage_days: r.maxage_days as u32,
        })
        .collect();
    assert!(!indexers.is_empty(), "no enabled indexers");

    let patterns: Vec<longbox_core::ParsingPattern> = parsing_pattern_repo::list_enabled(&db)
        .await
        .expect("patterns")
        .into_iter()
        .map(|r| longbox_core::ParsingPattern {
            id: r.id,
            name: r.name,
            pattern: r.pattern,
            priority: r.priority as i32,
            enabled: r.enabled,
        })
        .collect();

    // Production-faithful settings. An earlier run used
    // DEFAULT_MATCH_THRESHOLD (0.85) where the engine actually reads
    // `pull_indexer_match_threshold` (0.55) — a harness stricter than
    // production measures the harness, not the engine.
    let threshold: f64 =
        settings_repo::get_or_default(&db, "pull_indexer_match_threshold", 0.55_f64)
            .await
            .unwrap_or(0.55);
    let raw_excl: String =
        settings_repo::get_or_default(&db, "pull_exclusion_keywords", String::new())
            .await
            .unwrap_or_default();
    let exclusion_keywords: Vec<String> = raw_excl
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let min_size_mb: i64 = settings_repo::get_or_default(&db, "min_file_size_mb", 10_i64)
        .await
        .unwrap_or(10);
    let min_size_bytes = Some(min_size_mb * 1024 * 1024);
    eprintln!("settings: threshold={threshold} min_size_mb={min_size_mb} exclusions={exclusion_keywords:?}\n");

    let rows: Vec<(i64, String, i64, String, Option<String>)> = sqlx::query_as(SAMPLE_SQL)
        .fetch_all(&db)
        .await
        .expect("sample query");

    let mut out = String::from("series\tissue\treturned\tparsed\tbest_sim\tgrabbable\n");
    let (mut grabbable, mut total) = (0usize, 0usize);

    for (series_id, series_title, _issue_id, number, cover_date) in rows.into_iter().take(limit) {
        let aliases = series_repo::get_aliases(&db, series_id)
            .await
            .unwrap_or_default();
        let outcome = longbox_newznab::find_release_excluding_filtered(
            &indexers,
            &series_title,
            &number,
            None, // year gate off: measuring recall, not volume disambiguation
            &[],
            &patterns,
            threshold,
            &exclusion_keywords,
            cover_date.as_deref(),
            min_size_bytes,
            &aliases,
        )
        .await;

        total += 1;
        let line = match outcome {
            Ok(FindOutcome::Match { release, .. }) => {
                grabbable += 1;
                format!("{series_title}\t{number}\t?\t?\t?\tYES\t{}", release.title)
            }
            Ok(FindOutcome::Mismatch { diagnostic, .. }) => format!(
                "{series_title}\t{number}\t{}\t{}\t{}\tno",
                diagnostic.total_results,
                diagnostic.parseable_count,
                diagnostic
                    .best_similarity
                    .map(|s| format!("{s:.2}"))
                    .unwrap_or_else(|| "-".into()),
            ),
            Ok(FindOutcome::NoMatch) => format!("{series_title}\t{number}\t0\t0\t-\tno"),
            Err(e) => format!("{series_title}\t{number}\tERR\tERR\t-\tno\t{e}"),
        };
        eprintln!("  {line}");
        out.push_str(&line);
        out.push('\n');
    }

    eprintln!("\n=== {grabbable}/{total} grabbable ===");
    out.push_str(&format!("# grabbable {grabbable}/{total}\n"));
    std::fs::write(&out_path, out).expect("write results");
}

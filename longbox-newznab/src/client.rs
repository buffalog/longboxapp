//! HTTP orchestration — single-indexer search and the high-level
//! cross-indexer [`find_release`].

use std::time::Duration;

use longbox_core::ParsingPattern;

use crate::error::{IndexerError, NewznabError};
use crate::parse::parse_response;
use crate::query::{build_search_term, build_url};
use crate::select::{filter_by_series_title, select_best, MismatchDiagnostic};
use crate::types::{IndexerConfig, IndexerId, Release};

/// Outcome of a multi-indexer search with the Bug 3 series-title filter.
/// `find_release_excluding_filtered` returns this rather than a bare
/// `Option<(IndexerId, Release)>` so the caller can distinguish a real
/// match from a series-title mismatch (worth surfacing on
/// `/needs-attention`) from a silent no-match (retried next sweep).
#[derive(Debug, Clone)]
pub enum FindOutcome {
    /// A release passed both gates and was selected.
    Match {
        indexer: IndexerId,
        release: Release,
    },
    /// At least one indexer returned releases but none survived the
    /// series-title filter. Diagnostic is from the highest-priority
    /// indexer that produced a mismatch (lower-priority indexers'
    /// mismatches are dropped — one row per sweep is enough surfacing).
    Mismatch {
        indexer: IndexerId,
        diagnostic: MismatchDiagnostic,
    },
    /// No indexer returned a usable pool — either every indexer was
    /// empty, every indexer's results were year-filtered out, or both.
    /// Retry next sweep, no `pull_attempts` row.
    NoMatch,
}

/// Per-request timeout. Newznab indexers are usually fast; a slow one
/// shouldn't stall the whole pull sweep.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // Pose as Mylar3, the most widely-allowlisted comic-pulling
        // client. Several Newznab indexers gate on User-Agent (a
        // generic reqwest UA gets 403s or empty results); the Mylar3
        // string is the one comic-tagger fleets coalesce around.
        .user_agent("Mylar3/3.0.0")
        .build()
        // Only fails on TLS-backend init — a process-level invariant,
        // not a recoverable runtime condition.
        .expect("reqwest client build failed")
}

/// Search a single indexer with one explicit search term. Public for
/// callers/tests that want the low-level entry point; [`find_release`]
/// is the normal way in.
pub async fn search_indexer(
    indexer: &IndexerConfig,
    search_term: &str,
) -> Result<Vec<Release>, IndexerError> {
    search_with(&http_client(), indexer, search_term).await
}

/// Probe an indexer: confirm it is reachable and the API key is
/// accepted, without committing to a real search. Implemented as a
/// minimal `t=search` — the lightest call that actually exercises the
/// apikey (`t=caps`, the obvious alternative, is served unauthenticated
/// by many indexers). `Ok(())` means reachable *and* authenticated; on
/// failure the [`IndexerError`] variant distinguishes a rejected key
/// (`BadCredentials`) from an unreachable host (`HttpFailure`).
pub async fn test_connection(indexer: &IndexerConfig) -> Result<(), IndexerError> {
    search_indexer(indexer, "test").await.map(|_| ())
}

/// The real single-request implementation, parameterized on a shared
/// `reqwest::Client` so `find_release` can reuse one connection pool
/// across its indexer loop.
async fn search_with(
    client: &reqwest::Client,
    indexer: &IndexerConfig,
    search_term: &str,
) -> Result<Vec<Release>, IndexerError> {
    let url = build_url(indexer, search_term)?;
    let resp = client.get(&url).send().await.map_err(|e| {
        IndexerError::HttpFailure(format!("request to {} failed: {e}", indexer.name))
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(IndexerError::HttpFailure(format!(
            "indexer {} returned HTTP {status}",
            indexer.name
        )));
    }
    let body = resp.text().await.map_err(|e| {
        IndexerError::HttpFailure(format!("reading body from {} failed: {e}", indexer.name))
    })?;
    parse_response(&body)
}

/// Search one indexer with the two-variation strategy: zero-padded
/// issue first, then unpadded on zero hits. A variation-1 failure is
/// the indexer's failure (propagated). A variation-2 failure *after* a
/// clean (empty) variation-1 is best-effort — logged, swallowed,
/// empty returned.
async fn search_one_indexer(
    client: &reqwest::Client,
    indexer: &IndexerConfig,
    series: &str,
    issue: &str,
    year: Option<i32>,
) -> Result<Vec<Release>, IndexerError> {
    let padded = build_search_term(series, issue, year, true);
    let first = search_with(client, indexer, &padded).await?;
    if !first.is_empty() {
        return Ok(first);
    }

    let unpadded = build_search_term(series, issue, year, false);
    if unpadded == padded {
        // Non-numeric issue — both variations identical, no retry.
        return Ok(first);
    }
    match search_with(client, indexer, &unpadded).await {
        Ok(second) => Ok(second),
        Err(e) => {
            tracing::debug!(
                target: "longbox_newznab",
                indexer = %indexer.name,
                error = %e,
                "variation-2 retry failed; variation-1 already returned cleanly"
            );
            Ok(first)
        }
    }
}

/// Find the best release for an issue across a list of indexers.
///
/// Indexers are queried in ascending `priority` order; the first one
/// that returns any results wins, and the best release from that
/// indexer's pool is selected. Per-indexer failures are collected —
/// if *every* indexer errors, returns
/// [`NewznabError::AllIndexersFailed`] carrying each failure (so the
/// caller can tell permanent from transient). Indexers that respond
/// with zero results are not failures: an all-zero search is
/// `Ok(None)`.
///
/// `year`, when `Some`, is appended to the query for volume
/// disambiguation — the caller decides whether to pass it.
pub async fn find_release(
    indexers: &[IndexerConfig],
    series: &str,
    issue: &str,
    year: Option<i32>,
) -> Result<Option<Release>, NewznabError> {
    Ok(find_release_excluding(indexers, series, issue, year, &[])
        .await?
        .map(|(_indexer, release)| release))
}

/// [`find_release`] with retry-exclusion: any release whose `guid` is
/// in `exclude_guids` is dropped before selection. The pull engine's
/// grab-retry path passes the guids of releases already tried for the
/// issue, so the re-query picks a *different* NZB.
///
/// Returns the serving indexer's [`IndexerId`] alongside the chosen
/// release — the caller records it on the pull attempt for error
/// attribution. An indexer whose every result is excluded is treated
/// like a zero-result response: not a failure, fall through.
pub async fn find_release_excluding(
    indexers: &[IndexerConfig],
    series: &str,
    issue: &str,
    year: Option<i32>,
    exclude_guids: &[String],
) -> Result<Option<(IndexerId, Release)>, NewznabError> {
    if indexers.is_empty() {
        return Ok(None);
    }

    // Defensive: sort by priority so the caller need not pre-order.
    let mut ordered: Vec<&IndexerConfig> = indexers.iter().collect();
    ordered.sort_by_key(|i| i.priority);

    let client = http_client();
    let mut failures: Vec<(IndexerId, IndexerError)> = Vec::new();

    for indexer in ordered {
        match search_one_indexer(&client, indexer, series, issue, year).await {
            Ok(results) => {
                let kept: Vec<Release> = results
                    .into_iter()
                    .filter(|r| !exclude_guids.iter().any(|g| g == &r.guid))
                    .collect();
                // First indexer with a usable result wins (matches
                // Mylar). select_best is `None` only on an empty pool —
                // an all-excluded indexer falls through like a zero-hit.
                if let Some(best) = select_best(kept) {
                    return Ok(Some((indexer.id, best)));
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "longbox_newznab",
                    indexer = %indexer.name,
                    error = %e,
                    "indexer query failed"
                );
                failures.push((indexer.id, e));
            }
        }
    }

    // Reached the end with no results. Every indexer erroring is a
    // hard failure; a mix of errors + clean-empties is just no-match.
    if failures.len() == indexers.len() {
        Err(NewznabError::AllIndexersFailed(failures))
    } else {
        Ok(None)
    }
}

/// [`find_release_excluding`] with the Bug 3 series-title correctness
/// filter applied per-indexer before selection. The bare variant remains
/// for tests and lower-level callers; production (the pull engine) goes
/// through this one.
///
/// `patterns` are the catalog's filename-parsing rules (`parsing_patterns`
/// table). Reused on release titles so the same parser hardening covers
/// both file imports and newznab pre-grab.
///
/// `similarity_threshold` is the cutoff below which a release is dropped
/// as a series mismatch. Default
/// [`longbox_core::PULL_INDEXER_MATCH_THRESHOLD`] (0.75); the engine
/// re-reads it per-sweep from the `pull_indexer_match_threshold` setting
/// so tuning needs no restart.
///
/// Multi-indexer policy: first indexer whose post-filter pool is
/// non-empty wins (Match). A pool that is empty post-filter due to
/// series-mismatch is remembered (the *first* such — highest-priority);
/// later indexers can still produce a Match and override. If no indexer
/// produces a Match but at least one produced a mismatch, the remembered
/// diagnostic is returned. Pools emptied by the year-filter alone leave
/// no diagnostic — they're silent fall-through.
pub async fn find_release_excluding_filtered(
    indexers: &[IndexerConfig],
    series: &str,
    issue: &str,
    year: Option<i32>,
    exclude_guids: &[String],
    patterns: &[ParsingPattern],
    similarity_threshold: f64,
) -> Result<FindOutcome, NewznabError> {
    if indexers.is_empty() {
        return Ok(FindOutcome::NoMatch);
    }

    // Defensive: sort by priority so the caller need not pre-order.
    let mut ordered: Vec<&IndexerConfig> = indexers.iter().collect();
    ordered.sort_by_key(|i| i.priority);

    let client = http_client();
    let mut failures: Vec<(IndexerId, IndexerError)> = Vec::new();
    let mut first_mismatch: Option<(IndexerId, MismatchDiagnostic)> = None;

    for indexer in ordered {
        match search_one_indexer(&client, indexer, series, issue, year).await {
            Ok(results) => {
                let kept_pre_filter: Vec<Release> = results
                    .into_iter()
                    .filter(|r| !exclude_guids.iter().any(|g| g == &r.guid))
                    .collect();
                let outcome =
                    filter_by_series_title(kept_pre_filter, patterns, series, year, similarity_threshold);
                if !outcome.kept.is_empty() {
                    if let Some(best) = select_best(outcome.kept) {
                        return Ok(FindOutcome::Match {
                            indexer: indexer.id,
                            release: best,
                        });
                    }
                }
                if let Some(diag) = outcome.mismatch {
                    if first_mismatch.is_none() {
                        first_mismatch = Some((indexer.id, diag));
                    }
                }
                // outcome.mismatch.is_none() AND kept.is_empty() = silent
                // (year-only or no results) — fall through.
            }
            Err(e) => {
                tracing::warn!(
                    target: "longbox_newznab",
                    indexer = %indexer.name,
                    error = %e,
                    "indexer query failed"
                );
                failures.push((indexer.id, e));
            }
        }
    }

    if failures.len() == indexers.len() {
        return Err(NewznabError::AllIndexersFailed(failures));
    }
    if let Some((indexer, diagnostic)) = first_mismatch {
        return Ok(FindOutcome::Mismatch {
            indexer,
            diagnostic,
        });
    }
    Ok(FindOutcome::NoMatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(base_url: String) -> IndexerConfig {
        IndexerConfig {
            id: IndexerId(1),
            name: "test-indexer".into(),
            base_url,
            api_key: "KEY".into(),
            priority: 0,
            maxage_days: 1500,
        }
    }

    #[tokio::test]
    async fn test_connection_ok_on_well_formed_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("t", "search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<rss><channel></channel></rss>"),
            )
            .mount(&server)
            .await;
        // A zero-result search is still a successful, authenticated probe.
        assert!(test_connection(&cfg(server.uri())).await.is_ok());
    }

    #[tokio::test]
    async fn test_connection_surfaces_bad_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"<error code="100" description="Incorrect user credentials"/>"#,
                ),
            )
            .mount(&server)
            .await;
        match test_connection(&cfg(server.uri())).await {
            Err(IndexerError::BadCredentials { code, .. }) => assert_eq!(code, 100),
            other => panic!("expected BadCredentials, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_connection_unreachable_host_is_http_failure() {
        // Nothing listening — connection refused, not a timeout.
        let result = test_connection(&cfg("http://127.0.0.1:1".into())).await;
        assert!(matches!(result, Err(IndexerError::HttpFailure(_))));
    }

    /// Two releases for the same issue — a cbz (guid abc-123) and a cbr
    /// (guid def-456). `select_best` prefers the cbz absent exclusion.
    const TWO_ITEM_RSS: &str = r#"<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
      <channel>
        <item><title>Wolverine 005.cbz</title><guid>abc-123</guid>
              <enclosure url="https://idx/nzb/abc-123"/></item>
        <item><title>Wolverine 005.cbr</title><guid>def-456</guid>
              <enclosure url="https://idx/nzb/def-456"/></item>
      </channel>
    </rss>"#;

    #[tokio::test]
    async fn find_release_excluding_skips_tried_guids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_string(TWO_ITEM_RSS))
            .mount(&server)
            .await;
        let indexers = [cfg(server.uri())];

        // No exclusion: the cbz wins, and the serving indexer id comes back.
        let (id, rel) = find_release_excluding(&indexers, "Wolverine", "5", None, &[])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(id, IndexerId(1));
        assert_eq!(rel.guid, "abc-123");

        // Exclude the cbz: the cbr is picked instead.
        let (_, rel) =
            find_release_excluding(&indexers, "Wolverine", "5", None, &["abc-123".into()])
                .await
                .unwrap()
                .unwrap();
        assert_eq!(rel.guid, "def-456");

        // Exclude both: nothing usable — Ok(None), not an error.
        let exhausted = find_release_excluding(
            &indexers,
            "Wolverine",
            "5",
            None,
            &["abc-123".into(), "def-456".into()],
        )
        .await
        .unwrap();
        assert!(exhausted.is_none());
    }

    /// Bug 3 integration test. An indexer pool that contains both a
    /// wrong-series release (would have grabbed pre-Bug-3) and the
    /// correct release. The filter drops the wrong-series candidate and
    /// `select_best` lands on the right one.
    #[tokio::test]
    async fn find_release_excluding_filtered_picks_correct_series_from_mixed_pool() {
        const MIXED_RSS: &str = r#"<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
          <channel>
            <item>
              <title>Beware the Eye of Odin 001 (2022) (digital).cbr</title>
              <guid>wrong-series</guid>
              <enclosure url="https://idx/nzb/wrong"/>
              <newznab:attr name="grabs" value="150"/>
            </item>
            <item>
              <title>Odin 001 (2014).cbz</title>
              <guid>correct-series</guid>
              <enclosure url="https://idx/nzb/correct"/>
              <newznab:attr name="grabs" value="20"/>
            </item>
          </channel>
        </rss>"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_string(MIXED_RSS))
            .mount(&server)
            .await;
        let indexers = [cfg(server.uri())];
        let patterns = longbox_core::filename::default_patterns();

        let outcome = find_release_excluding_filtered(
            &indexers,
            "Odin",
            "1",
            None,
            &[],
            &patterns,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        )
        .await
        .unwrap();

        match outcome {
            FindOutcome::Match { indexer, release } => {
                assert_eq!(indexer, IndexerId(1));
                assert_eq!(
                    release.guid, "correct-series",
                    "filter must drop wrong-series despite its higher grabs"
                );
            }
            other => panic!("expected Match, got {other:?}"),
        }
    }

    /// All-wrong pool surfaces as Mismatch with a populated diagnostic
    /// the engine renders into the `pull_attempts.error_message`.
    #[tokio::test]
    async fn find_release_excluding_filtered_returns_mismatch_when_pool_is_all_wrong() {
        const WRONG_ONLY: &str = r#"<rss><channel>
          <item><title>Beware the Eye of Odin 001 (2022).cbr</title>
                <guid>wrong-1</guid><enclosure url="https://idx/1"/></item>
          <item><title>Justice League - Road To Dark Crisis 001 (2022).cbz</title>
                <guid>wrong-2</guid><enclosure url="https://idx/2"/></item>
        </channel></rss>"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_string(WRONG_ONLY))
            .mount(&server)
            .await;
        let indexers = [cfg(server.uri())];
        let patterns = longbox_core::filename::default_patterns();

        let outcome = find_release_excluding_filtered(
            &indexers,
            "Odin",
            "1",
            None,
            &[],
            &patterns,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        )
        .await
        .unwrap();
        let diag = match outcome {
            FindOutcome::Mismatch { diagnostic, .. } => diagnostic,
            other => panic!("expected Mismatch, got {other:?}"),
        };
        assert_eq!(diag.total_results, 2);
        assert_eq!(diag.parseable_count, 2);
        assert!(diag.best_similarity.unwrap() < 0.5);
    }
}

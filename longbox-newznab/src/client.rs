//! HTTP orchestration — single-indexer search and the high-level
//! cross-indexer [`find_release`].

use std::time::Duration;

use crate::error::{IndexerError, NewznabError};
use crate::parse::parse_response;
use crate::query::{build_search_term, build_url};
use crate::select::select_best;
use crate::types::{IndexerConfig, IndexerId, Release};

/// Per-request timeout. Newznab indexers are usually fast; a slow one
/// shouldn't stall the whole pull sweep.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
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
            Ok(results) if !results.is_empty() => {
                // First indexer with results wins (matches Mylar).
                return Ok(select_best(results));
            }
            Ok(_) => {
                // Clean zero-results — fall through to the next indexer.
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

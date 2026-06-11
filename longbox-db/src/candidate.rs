//! Candidate series lookup for the matcher.
//!
//! Lifted from `longbox-scanner` during Phase B step 6 so both the
//! walker-driven scanner and the watcher-driven post-process pipeline
//! can call it without duplicating logic or coupling postprocess to
//! scanner. Behavior is byte-identical to the prior scanner-local
//! version; scanner's existing integration tests preserve coverage.
//!
//! Phase A strategy: fetch all series, rank by similarity to the title
//! hint, return the top N with their issues eagerly loaded. Trivial
//! against the ~hundreds of series Phase A users will have. Phase B/C
//! scale will swap this for a SQL similarity index — known scaling
//! work, not Phase A/B's problem.

use std::cmp::Ordering;

use longbox_core::{
    matcher::Candidate, normalize_title, similarity::similarity, Issue, IssueNumber, Series,
};

use crate::{error::Result, issue_repo, series_repo, IssueRow, Pool, SeriesRow};

const TOP_N_CANDIDATES: usize = 10;

pub async fn find_candidates(
    db: &Pool,
    series_title_hint: &str,
    year_hint: Option<i32>,
) -> Result<Vec<Candidate>> {
    let normalized_hint = normalize_title(series_title_hint);
    let all = series_repo::find_all(db).await?;

    if all.is_empty() {
        return Ok(Vec::new());
    }

    // Apply `normalize_title` to BOTH sides at comparison time. The
    // stored `sort_title` is supposed to be normalized at insert time,
    // but real-world data shows ~50% of catalog rows have un-normalized
    // sort_titles from historical insertion paths. Symmetric
    // normalization here keeps candidate ranking robust against that
    // drift; see the matching guard in `longbox_core::matcher`.
    let mut scored: Vec<(f64, SeriesRow)> = all
        .into_iter()
        .map(|row| {
            let score = similarity(&normalized_hint, &normalize_title(&row.sort_title));
            (score, row)
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                let a_year = year_hint.is_some() && a.1.start_year.map(|y| y as i32) == year_hint;
                let b_year = year_hint.is_some() && b.1.start_year.map(|y| y as i32) == year_hint;
                b_year.cmp(&a_year)
            })
            .then_with(|| a.1.id.cmp(&b.1.id))
    });

    let top = scored.into_iter().take(TOP_N_CANDIDATES);

    // Fetch issues per top candidate. Sequential — N <= 10, each query cheap.
    let mut candidates = Vec::with_capacity(TOP_N_CANDIDATES);
    for (_score, row) in top {
        let series_id = row.id;
        let series = row_to_series(row);
        let issue_rows = issue_repo::list_by_series(db, series_id).await?;
        let issues = issue_rows.into_iter().map(row_to_issue).collect();
        candidates.push(Candidate { series, issues });
    }
    Ok(candidates)
}

pub fn row_to_series(row: SeriesRow) -> Series {
    Series {
        id: row.id,
        cv_id: row.cv_id,
        metron_id: row.metron_id,
        title: row.title,
        sort_title: row.sort_title,
        start_year: row.start_year.map(|y| y as i32),
        publisher: row.publisher,
        description: row.description,
        cover_url: row.cover_url,
    }
}

pub fn row_to_issue(row: IssueRow) -> Issue {
    Issue {
        id: row.id,
        series_id: row.series_id,
        cv_id: row.cv_issue_id,
        metron_id: row.metron_issue_id,
        number: IssueNumber::new(row.number),
        title: row.title,
        cover_date: row.cover_date,
        summary: row.summary,
        cover_url: row.cover_url,
    }
}

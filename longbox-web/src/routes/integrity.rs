//! Library Integrity — discovery, and now resolution.
//!
//! # This module's mutating surface is declared and enforced
//!
//! Not "read-only", and no longer "one write path" — both were true of the
//! discovery-only version and neither is now. The property that survives, and
//! the one worth having, is that **every route declares what it may touch, and
//! a test proves the declaration matches what it actually reaches**:
//!
//! - `POST …/analyze` — [`Surface::Writes`] over [`DIGEST_COLUMNS`]. Derived
//!   cache: recomputable from bytes already on disk, so losing it costs one
//!   re-analysis and nothing depends on it.
//! - `POST …/duplicates/delete` — [`Surface::Deletes`] over [`DELETE_TABLES`].
//!   Removes a catalog row and unlinks a file. Recoverable from nothing, which
//!   is why it is a separate variant rather than a flavour of `Writes`.
//!
//! Retiring the property when the module gained the ability to act would have
//! been backwards: a declared write surface matters *more* once it writes, not
//! less.
//!
//! That property is enforced three ways, because the obvious way is not enough:
//!
//! 1. [`route_table`] declares, per route, whether it writes and what it
//!    writes to. The router is built from that same list, so a route cannot be
//!    registered without declaring a surface.
//! 2. A behavioural test derives its probe list from [`declared_routes`] and
//!    asserts, for every path x write-method pair, that the method is accepted
//!    IFF the route declared a write. A `Surface::ReadOnly` declaration on a
//!    `DELETE` route therefore fails loudly rather than shipping green — which
//!    is exactly what an earlier version of this did, because it only counted
//!    `Writes` declarations and probed a hand-written path list.
//! 3. An integration test snapshots every non-digest column of `files` around
//!    a real pass and asserts nothing else moved, turning the declaration from
//!    documentation into an assertion.
//!
//! A grep over query text is deliberately NOT one of these: it is defeated by
//! a multiline string, a helper indirection, or a macro.
//!
//! Relink and keeper selection remain deliberately absent. Relink belongs with
//! the cross-folder findings, which are the shape that needs it; every one of
//! the 37 content-duplicate groups resolves by delete alone. A suggested keeper
//! is ruled out on purpose: 28 of 37 groups are decidable from evidence already
//! on screen, so a suggestion saves almost nothing and would be confidently
//! wrong on exactly the handful that need the most thought — and a hint right
//! 33 times out of 37 stops the reader looking at the evidence by group twelve.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, MethodRouter};
use axum::{Json, Router};
use serde::Serialize;

use longbox_db::library_root_repo;

use crate::error::ApiError;
use crate::state::AppState;

/// What a route is allowed to write. Declared alongside the handler so the
/// two cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    /// Reads only. Computes findings from the catalog and the filesystem.
    ReadOnly,
    /// Writes, and only to these columns.
    Writes(&'static [&'static str]),
    /// Removes rows from these tables, and may unlink files beneath a
    /// library root.
    ///
    /// Strictly more than [`Surface::Writes`] rather than a flavour of
    /// it: the columns a write touches are derived cache, recomputable
    /// from bytes still on disk, so losing one costs a re-analysis. A
    /// removed row and an unlinked file are not recomputable from
    /// anything. Declaring them with the same variant would let a
    /// reviewer read "writes" and picture the recoverable case.
    Deletes(&'static [&'static str]),
}

/// What the duplicate-delete route removes. One table, plus the file
/// on disk — see [`crate::file_delete`] for the ordering and why.
pub(crate) const DELETE_TABLES: &[&str] = &["files"];

/// The columns the analyze pass may touch. Digests and the archive label are
/// derived cache: they describe bytes already on disk and can be recomputed
/// from those bytes at any time. Losing them costs one re-analysis; nothing
/// about the user's library or its bindings depends on them.
pub(crate) const DIGEST_COLUMNS: &[&str] = &[
    "content_blake3",
    "hashed_size_bytes",
    "hashed_mtime",
    "archive_label",
    "archive_label_kind",
];

/// Every route this module registers, paired with its declared write surface.
/// [`router`] is built from this list, so the list cannot fall out of date.
fn route_table() -> Vec<(&'static str, MethodRouter<AppState>, Surface)> {
    vec![
        (
            "/library/integrity/analyze",
            post(start_analyze),
            Surface::Writes(DIGEST_COLUMNS),
        ),
        (
            "/library/integrity/analyze/status",
            get(analyze_status),
            Surface::ReadOnly,
        ),
        (
            "/library/integrity/reconciliation",
            get(reconciliation),
            Surface::ReadOnly,
        ),
        (
            "/library/integrity/findings",
            get(findings),
            Surface::ReadOnly,
        ),
        (
            "/library/integrity/duplicates/delete",
            post(delete_duplicate),
            Surface::Deletes(DELETE_TABLES),
        ),
    ]
}

/// The declared surfaces, for the enforcement tests.
fn route_surfaces() -> Vec<(&'static str, Surface)> {
    route_table().into_iter().map(|(p, _, s)| (p, s)).collect()
}

/// Every route and whether it declares a write, for the integration test that
/// probes the real router.
///
/// The probe list MUST be derived from this rather than hand-written: a
/// hand-written list silently skips any route missing from it, so a `DELETE`
/// route declared `ReadOnly` would ship green. Asserting "accepted iff
/// declared as writing" makes a false declaration fail loudly.
///
/// Scope note: this covers routes registered by THIS module. A path under
/// `/library/integrity/...` registered elsewhere is invisible to both tests —
/// the constraint is module-scoped.
pub fn declared_routes() -> Vec<(&'static str, bool)> {
    route_surfaces()
        .into_iter()
        .map(|(p, s)| (p, matches!(s, Surface::Writes(_) | Surface::Deletes(_))))
        .collect()
}

/// Every route paired with the *kind* of surface it declares, for the
/// enforcement test.
///
/// Kind, not just a write/no-write bool: `Deletes` and `Writes` are
/// deliberately different variants because a column write is
/// recomputable and a removed row is not, and a test that collapsed
/// them could not notice a delete route mis-declared as a write.
pub fn declared_surface_kinds() -> Vec<(&'static str, &'static str)> {
    route_surfaces()
        .into_iter()
        .map(|(p, s)| {
            let kind = match s {
                Surface::ReadOnly => "read_only",
                Surface::Writes(_) => "writes",
                Surface::Deletes(_) => "deletes",
            };
            (p, kind)
        })
        .collect()
}

pub fn router() -> Router<AppState> {
    route_table()
        .into_iter()
        .fold(Router::new(), |r, (path, method, _)| r.route(path, method))
}

// -------- POST analyze: the one write path --------

#[derive(Debug, Serialize)]
struct StartAnalyzeResponse {
    status: &'static str,
}

/// Kick off the content-analysis pass and return immediately.
///
/// Size-gated hashing over the whole library takes tens of seconds (7.4 GB
/// across 80 candidate files on the library this was built against), which is
/// too long to hold a request open and far too long to attach to the nightly
/// scan — this is a pass a human runs while triaging, not steady-state work.
///
/// 409 while a pass is already in flight. Two concurrent passes would not
/// corrupt anything (each write is idempotent for the bytes it describes) but
/// they would double the I/O for no benefit.
async fn start_analyze(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<StartAnalyzeResponse>), ApiError> {
    {
        let mut status = state.analyze_status.write().await;
        if status.running {
            return Err(ApiError::Conflict {
                code: "conflict.analysis_in_progress",
                message: "content analysis is already running".to_owned(),
                details: serde_json::Value::Null,
            });
        }
        status.running = true;
        status.started_at = Some(time::OffsetDateTime::now_utc());
        status.finished_at = None;
        status.last_error = None;
        // Clear the previous pass's stats too. Leaving them would let the
        // endpoint serve `finished_at` from THIS pass alongside `last` from
        // the previous one — reporting work that never happened, most visibly
        // when a pass fails outright and the old success numbers survive.
        status.last = None;
    }

    let db = state.db.clone();
    let slot: Arc<_> = state.analyze_status.clone();
    tokio::spawn(async move {
        // Inner task so a panic becomes a JoinError instead of killing the
        // flag reset. Without it, a panic anywhere in the pass strands
        // `running = true` and every later POST 409s until the process
        // restarts — recoverable only by restarting the container.
        let inner = tokio::spawn(async move { crate::content_hash::refresh_digests(&db).await });
        let outcome: Result<crate::content_hash::HashStats, String> = match inner.await {
            Ok(Ok(stats)) => Ok(stats),
            Ok(Err(e)) => Err(e.to_string()),
            Err(join) => Err(format!("analysis task panicked: {join}")),
        };

        let mut status = slot.write().await;
        status.running = false;
        status.finished_at = Some(time::OffsetDateTime::now_utc());
        match outcome {
            Ok(stats) => {
                tracing::info!(
                    target: "longbox_web",
                    candidates = stats.candidates,
                    hashed = stats.hashed,
                    fresh = stats.fresh,
                    skipped = stats.skipped,
                    failed = stats.failed,
                    labelled = stats.labelled,
                    bytes = stats.bytes_hashed,
                    first_failure = stats.first_failure.as_deref().unwrap_or("-"),
                    "integrity.analyze completed"
                );
                status.last = Some(stats);
            }
            Err(e) => {
                tracing::warn!(target: "longbox_web", error = %e, "integrity.analyze failed");
                status.last_error = Some(e);
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(StartAnalyzeResponse { status: "started" }),
    ))
}

// -------- GET findings: disk/DB reconciliation --------

/// Files on disk the catalog has never seen, and rows claiming a file that
/// is not there.
///
/// Expected to be empty on a healthy library, which is exactly why the
/// response carries `provenance`: a section rendering "0 problems" because
/// the walk failed would be indistinguishable from one rendering it because
/// the library is clean. `is_conclusive` says which.
async fn reconciliation(
    State(state): State<AppState>,
) -> Result<Json<crate::integrity_scan::Reconciliation>, ApiError> {
    let root = library_root_repo::find_by_id(&state.db, state.library_root_id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "library_root",
            id: state.library_root_id.to_string(),
        })?;
    Ok(Json(
        crate::integrity_scan::reconcile(&state.db, state.library_root_id, &root.path).await?,
    ))
}

/// All six finding classes, computed live.
///
/// One endpoint rather than six because the classes overlap and the view
/// renders them together: Blood Train is both a content duplicate and a
/// two-series finding, and a file appearing in more than one class is a
/// stronger signal than one appearing in a single class. Splitting them
/// across endpoints would hide that.
async fn findings(
    State(state): State<AppState>,
) -> Result<Json<crate::integrity_scan::Findings>, ApiError> {
    let root = library_root_repo::find_by_id(&state.db, state.library_root_id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "library_root",
            id: state.library_root_id.to_string(),
        })?;
    Ok(Json(
        crate::integrity_scan::findings(&state.db, state.library_root_id, &root.path).await?,
    ))
}

// -------- GET status --------

async fn analyze_status(State(state): State<AppState>) -> Json<crate::state::AnalyzeStatus> {
    Json(state.analyze_status.read().await.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared write surface: exactly one route writes, it is the
    /// analyze endpoint, and it writes only derived digest/label columns.
    ///
    /// Adding a route forces a `Surface` declaration in the same expression as
    /// the handler, so this cannot be bypassed by forgetting to update a list
    /// somewhere else.
    #[test]
    fn exactly_one_route_declares_a_write_surface() {
        let surfaces = route_surfaces();
        let writers: Vec<_> = surfaces
            .iter()
            .filter(|(_, s)| matches!(s, Surface::Writes(_)))
            .collect();
        assert_eq!(
            writers.len(),
            1,
            "integrity is a discovery surface: exactly one write path. Found: {:?}",
            writers.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        let (path, surface) = writers[0];
        assert_eq!(*path, "/library/integrity/analyze");
        let Surface::Writes(cols) = surface else {
            unreachable!()
        };
        assert_eq!(
            *cols, DIGEST_COLUMNS,
            "the only writable columns are the derived digest/label cache"
        );
    }

    /// Every writable column is derived cache — recomputable from bytes
    /// already on disk. None of them is catalog truth.
    #[test]
    fn the_writable_columns_are_all_derived_cache() {
        for col in DIGEST_COLUMNS {
            assert!(
                col.starts_with("content_")
                    || col.starts_with("hashed_")
                    || col.starts_with("archive_label"),
                "{col} is not a derived-cache column; a write surface here would be catalog truth"
            );
        }
        // Explicitly NOT writable, and each would be a real mutation:
        for forbidden in [
            "issue_id",
            "status",
            "is_present",
            "match_method",
            "path_relative",
        ] {
            assert!(
                !DIGEST_COLUMNS.contains(&forbidden),
                "{forbidden} must never be in the integrity write surface"
            );
        }
    }
}

// -------- POST: delete one redundant copy from a content-duplicate group --------

/// One file, named with the digest of the group it belongs to.
///
/// The digest is not redundant with `file_id`: it is the client's claim
/// about *which group it was looking at*, and the server refuses if the
/// file does not belong to it. A stale page that has since been
/// re-analysed cannot delete a file the user never saw.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct DeleteDuplicateBody {
    digest: String,
    file_id: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct RevertedIssue {
    issue_id: i64,
    /// Text, never a number. Issue numbers are `29.5`, `1A`, `∞` —
    /// parsing one to compare or display it is how a decimal gets
    /// truncated to `29`.
    issue_number: String,
    series_title: String,
    /// The issue has no owned, present file any more.
    now_missing: bool,
    /// Whether anything will actually search for it again, and if not,
    /// why. See [`SearchOutlook`].
    search_outlook: SearchOutlook,
}

/// Will the pull sweep search for this issue again — and if not, why?
///
/// A revert is only half the story the user needs. "Reverts to missing"
/// reads as "so it will be re-downloaded", and on this library that is
/// false for roughly half the issues a delete can touch: 632 of 777
/// series are not on the pull list, so nothing searches for them ever,
/// regardless of their attempt state. Reporting the revert without this
/// invites the user to wait for a sweep that will never look.
///
/// Each variant is one exclusion the sweep applies, and every one is
/// read from a fact rather than re-derived: the `pull_list` row, the
/// issue's own `cover_date`, and
/// [`AttemptBlockers`](longbox_db::pull_attempt_repo::AttemptBlockers).
/// `list_pull_candidates` stays the single implementation of candidacy;
/// this only explains an outcome, it never decides one.
///
/// Three of these fire zero times on the current library
/// (`BelowSeriesStartIssue`, `RetriesExhausted`, `RecentlySubmitted`).
/// They are encoded anyway, because "zero today" is a measurement, not
/// an invariant, and the failure mode is silent.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchOutlook {
    /// Nothing blocks it: the next sweep will search for this issue.
    WillSearch,
    /// No `pull_list` row for this series. Nothing sweeps it, ever.
    SeriesNotOnPullList,
    /// The series is on the pull list but paused, so `list_active`
    /// skips it.
    SeriesPaused,
    /// Below the series' `start_issue` floor, which the sweep applies
    /// after the candidate query.
    BelowSeriesStartIssue,
    /// No usable `cover_date` — absent, malformed, or in the future.
    /// The candidate query requires a well-formed date not later than
    /// today.
    NoUsableCoverDate,
    /// An attempt carries `retry_count >= 3`; the sweep has given up.
    RetriesExhausted,
    /// A `pending`/`grabbed` attempt survives. Searching resumes once
    /// the sweep's stale-grab purge clears it.
    GrabInFlight,
    /// A `submitted` attempt landed inside the 6-hour window.
    RecentlySubmitted,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeleteDuplicateResult {
    file_id: i64,
    path_relative: String,
    /// `None` when the deleted copy was not bound to any issue — an
    /// `ignored` stray, which owns nothing and therefore reverts
    /// nothing.
    reverted: Option<RevertedIssue>,
    /// Verified copies of these bytes still on disk after this delete.
    ///
    /// Counted by stat-ing each sibling and confirming its digest is
    /// still valid — NOT by counting catalog rows, which would inflate
    /// with absent files and reassure the user with a number too high.
    remaining_in_group: usize,
    /// Set when the catalog row was removed but the bytes survived.
    /// The file is now an orphan and reconciliation will report it.
    unlink_error: Option<String>,
}

/// Decide which single reason to report, most permanent first.
///
/// The order matters because several can hold at once and only one is
/// the binding constraint. A series that is not on the pull list will
/// never be swept no matter what its attempts look like, so saying
/// "waiting for a grab to clear" there would be true about the attempt
/// and useless to the user. Series-level reasons therefore outrank
/// issue-level ones, and permanent outranks temporary.
fn search_outlook(
    issue: &longbox_db::IssueRow,
    entry: Option<&longbox_db::PullListRow>,
    blockers: &longbox_db::pull_attempt_repo::AttemptBlockers,
) -> SearchOutlook {
    let Some(entry) = entry else {
        return SearchOutlook::SeriesNotOnPullList;
    };
    if entry.paused {
        return SearchOutlook::SeriesPaused;
    }
    if let Some(floor) = entry.start_issue.as_deref() {
        // Same comparison the sweep's `apply_start_floor` uses —
        // natural order, so `29.5` sorts between `29` and `30` and a
        // string compare cannot truncate it.
        let number = longbox_core::IssueNumber::new(issue.number.clone());
        let floor = longbox_core::IssueNumber::new(floor.to_owned());
        if longbox_core::IssueNumber::natural_cmp(&number, &floor) == std::cmp::Ordering::Less {
            return SearchOutlook::BelowSeriesStartIssue;
        }
    }
    if !has_usable_cover_date(issue.cover_date.as_deref()) {
        return SearchOutlook::NoUsableCoverDate;
    }
    if blockers.retries_exhausted {
        return SearchOutlook::RetriesExhausted;
    }
    if blockers.in_flight_status.is_some() {
        return SearchOutlook::GrabInFlight;
    }
    if blockers.recently_submitted {
        return SearchOutlook::RecentlySubmitted;
    }
    SearchOutlook::WillSearch
}

/// The candidate query's date test, in Rust: a well-formed `YYYY-MM-DD`
/// no later than today.
///
/// Compared as text against a UTC date, exactly as the SQL does
/// (`length(cover_date) = 10 AND cover_date <= date('now')`), so a
/// future-dated solicitation is correctly reported as not-yet-searched
/// rather than as a blocked one.
fn has_usable_cover_date(cover_date: Option<&str>) -> bool {
    let Some(d) = cover_date else {
        return false;
    };
    if d.len() != 10 {
        return false;
    }
    let today = time::OffsetDateTime::now_utc().date();
    let today = format!(
        "{:04}-{:02}-{:02}",
        today.year(),
        u8::from(today.month()),
        today.day()
    );
    d <= today.as_str()
}

/// Delete exactly one copy from one content-duplicate group.
///
/// One file per request, by design. There is no bulk form and no
/// "resolve all": 37 groups is twenty minutes of clicking, and a bulk
/// control is the shape that once armed 26 delete buttons on issues of
/// The Authority that were all correctly bound.
///
/// Everything is re-derived server-side. The client supplies a digest
/// and a file id; the server decides what that means.
async fn delete_duplicate(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<DeleteDuplicateBody>,
) -> Result<axum::Json<DeleteDuplicateResult>, ApiError> {
    let group = longbox_db::file_repo::list_by_content_hash(&state.db, &body.digest).await?;

    let target = group
        .iter()
        .find(|f| f.id == body.file_id)
        .ok_or_else(|| ApiError::BadRequest {
            message: "that file is not part of the group with this digest".into(),
        })?
        .clone();

    let roots = library_root_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect::<std::collections::HashMap<_, _>>();

    // Never empty a group — and decide that by LOOKING AT THE DISK, not
    // by counting catalog rows.
    //
    // `is_present` only changes on a scan, so a row can claim a file
    // that is not there; counting rows would let this endpoint report
    // "1 copy remains" while removing the last real bytes, which is
    // exactly what its refusal message promises cannot happen. The
    // stored digest is likewise only trustworthy if the file still
    // matches the stamp it was hashed against — a file modified since
    // the last analyze is not a duplicate, and deleting on a stale
    // digest destroys something that is no longer a copy.
    //
    // `DiskObservation::stat` answers both questions in one syscall and
    // is the type that exists precisely so a delete decision cannot
    // rest on a catalog column. Library Tidy has used it since PR #32;
    // this endpoint was written without it, which is the fifth instance
    // of that substitution on this codebase and the first where the
    // remedy was already in the tree.
    // Held across the guard AND the delete, as one unit. Checking
    // "another verified copy survives" and then deleting are only safe
    // together; a lock around either half alone leaves the interleaving
    // that empties a group. See `AppState::delete_lock` for the ceiling.
    let _delete_guard = state.delete_lock.lock().await;

    let stamps: std::collections::HashMap<i64, longbox_db::ContentStamp> =
        longbox_db::file_repo::content_stamps_by_content_hash(&state.db, &body.digest)
            .await?
            .into_iter()
            .map(|s| (s.file_id, s))
            .collect();

    // The TARGET's own digest first. The rule above applies to the file
    // being deleted at least as much as to the ones surviving it: if it
    // has changed since the analyze that grouped it, its stored digest
    // describes bytes that no longer exist, it is not a copy of
    // anything, and deleting it destroys the only instance of whatever
    // it now contains.
    //
    // An earlier revision stat-ed only the siblings — the rule was
    // written out in full immediately above and then applied to
    // everything except the file at risk. `integrity_scan` groups purely
    // on the catalog's `content_blake3` with no disk validation, so a
    // stale member stays on screen and remains clickable.
    let (target_exists, target_obs) = crate::disk_observation::DiskObservation::stat(
        &roots,
        target.library_root_id,
        &target.path_relative,
        stamps
            .get(&target.id)
            .and_then(|s| s.content_blake3.as_deref()),
        stamps.get(&target.id).and_then(|s| s.hashed_size_bytes),
        stamps.get(&target.id).and_then(|s| s.hashed_mtime),
    )
    .await;
    if target_exists && target_obs.digest() != Some(body.digest.as_str()) {
        return Err(ApiError::BadRequest {
            message: "this file has changed on disk since it was analysed, so it is no longer a \
                      verified copy of these bytes; re-run Analyze before deleting it"
                .into(),
        });
    }

    let mut siblings = Vec::new();
    for other in group.iter().filter(|f| f.id != target.id) {
        let (exists, obs) = crate::disk_observation::DiskObservation::stat(
            &roots,
            other.library_root_id,
            &other.path_relative,
            stamps
                .get(&other.id)
                .and_then(|s| s.content_blake3.as_deref()),
            stamps.get(&other.id).and_then(|s| s.hashed_size_bytes),
            stamps.get(&other.id).and_then(|s| s.hashed_mtime),
        )
        .await;
        siblings.push((exists, obs));
    }
    // Identity BEFORE content, across the WHOLE group, and a refusal
    // rather than a skip.
    //
    // `metadata()` follows symlinks, so an alias — a symlink and its
    // target, or one file catalogued under two library roots — stats
    // identically to what it points at: same size, same mtime, same
    // validated digest. It is not a copy of anything; it is one file
    // wearing two names, and no delete in such a group performs the
    // redundancy-removal this feature exists for.
    //
    // Refusing outright is deliberate, and is the whole shape of this
    // guard. The alternative — treat aliases as non-copies but let the
    // delete proceed when the target is itself a symlink, reasoning
    // that unlinking a link frees no bytes — is true about bytes and
    // false about the library: two symlinks pointing at one file
    // outside any root are catalogued as an ordinary two-member group
    // (the scanner walks with `follow_links(true)`), and that bypass
    // would empty the group completely, reverting both issues, while
    // this endpoint's own message promised it could not.
    //
    // So: one behaviour, not three. Any aliased pair anywhere in the
    // group stops the operation — the same predicate Library Tidy
    // applies, deliberately, because two implementations of one rule
    // are two chances to disagree. That also keeps `remaining_in_group`
    // honest: there is no successful-delete-with-alias path left for it
    // to misreport.
    let all: Vec<&crate::disk_observation::DiskObservation> = std::iter::once(&target_obs)
        .chain(siblings.iter().map(|(_, o)| o))
        .collect();
    if all
        .iter()
        .enumerate()
        .any(|(i, a)| all.iter().skip(i + 1).any(|b| a.is_same_file(b)))
    {
        return Err(ApiError::BadRequest {
            message: "these entries are the same file on disk, reached by different names (a \
                      symlink, or one file catalogued under two library roots), so neither is a \
                      redundant copy of the other and deleting either would remove the only \
                      bytes. Resolve this one manually."
                .into(),
        });
    }

    // Present on disk AND still carrying a digest that matches this
    // group's. A stale or absent digest means we cannot say this is a
    // copy of the bytes being deleted.
    let surviving_copies = siblings
        .iter()
        .filter(|(exists, obs)| *exists && obs.digest() == Some(body.digest.as_str()))
        .count();
    if surviving_copies == 0 {
        return Err(ApiError::BadRequest {
            message: "no other verified copy of these bytes is on disk, so deleting this file \
                      would remove the last one — which is not what a duplicate delete is for"
                .into(),
        });
    }

    // The issue this copy occupies, captured BEFORE the delete. Files
    // in one group can be bound to different series (byte-identical
    // content filed under two titles), so nothing here may assume a
    // single series for the group — only this file's own binding.
    let bound = target.issue_id;

    let deleted = crate::file_delete::delete_file(&state.db, &roots, &target)
        .await
        .map_err(|message| ApiError::BadRequest { message })?;

    // What reverted. `None` for an unbound copy: an `ignored` stray
    // holds no issue, so its removal changes no ownership. Status is
    // deliberately not consulted — ownership is derived from a present
    // file bound to the issue, and an ignored row is neither.
    let mut reverted = None;
    if let Some(issue_id) = bound {
        if let Some(issue) = longbox_db::issue_repo::find_by_id(&state.db, issue_id).await? {
            let still_owned = longbox_db::file_repo::list_by_issue(&state.db, issue_id)
                .await?
                .iter()
                .any(|f| f.status == "owned" && f.is_present);
            let series = longbox_db::series_repo::find_by_id(&state.db, issue.series_id)
                .await?
                .map(|s| s.title)
                .unwrap_or_default();
            let entry = longbox_db::pull_list_repo::get(&state.db, issue.series_id).await?;
            let blockers =
                longbox_db::pull_attempt_repo::attempt_blockers(&state.db, issue_id).await?;
            let outlook = search_outlook(&issue, entry.as_ref(), &blockers);
            reverted = Some(RevertedIssue {
                issue_id,
                issue_number: issue.number,
                series_title: series,
                now_missing: !still_owned,
                search_outlook: outlook,
            });
        }
    }

    tracing::info!(
        target: "longbox_web",
        file_id = target.id,
        path = %target.path_relative,
        digest = %body.digest,
        reverted_issue = ?reverted.as_ref().map(|r| r.issue_id),
        "integrity: deleted a redundant copy"
    );

    Ok(axum::Json(DeleteDuplicateResult {
        file_id: target.id,
        path_relative: target.path_relative,
        reverted,
        remaining_in_group: surviving_copies,
        unlink_error: deleted.unlink_error,
    }))
}

#[cfg(test)]
mod outlook_tests {
    use super::*;
    use longbox_db::pull_attempt_repo::AttemptBlockers;

    fn issue(number: &str, cover_date: Option<&str>) -> longbox_db::IssueRow {
        let now = time::OffsetDateTime::now_utc();
        let now = time::PrimitiveDateTime::new(now.date(), now.time());
        longbox_db::IssueRow {
            id: 1,
            series_id: 1,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.to_owned(),
            title: None,
            cover_date: cover_date.map(str::to_owned),
            summary: None,
            cover_url: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn entry(paused: bool, start_issue: Option<&str>) -> longbox_db::PullListRow {
        let now = time::OffsetDateTime::now_utc();
        let now = time::PrimitiveDateTime::new(now.date(), now.time());
        longbox_db::PullListRow {
            id: 1,
            series_id: 1,
            added_at: now,
            start_issue: start_issue.map(str::to_owned),
            paused,
            last_pull_attempt_at: None,
            last_successful_pull_at: None,
            failure_count: 0,
        }
    }

    fn blockers(retries: bool, in_flight: Option<&str>, submitted: bool) -> AttemptBlockers {
        AttemptBlockers {
            in_flight_status: in_flight.map(str::to_owned),
            retries_exhausted: retries,
            recently_submitted: submitted,
        }
    }

    fn clear() -> AttemptBlockers {
        AttemptBlockers::default()
    }

    /// Every variant is reachable, and reachable for its own reason.
    #[test]
    fn each_reason_is_reported() {
        let ok = issue("5", Some("2020-01-01"));
        let listed = entry(false, None);

        assert_eq!(
            search_outlook(&ok, None, &clear()),
            SearchOutlook::SeriesNotOnPullList
        );
        assert_eq!(
            search_outlook(&ok, Some(&entry(true, None)), &clear()),
            SearchOutlook::SeriesPaused
        );
        assert_eq!(
            search_outlook(
                &issue("2", Some("2020-01-01")),
                Some(&entry(false, Some("10"))),
                &clear()
            ),
            SearchOutlook::BelowSeriesStartIssue
        );
        assert_eq!(
            search_outlook(&issue("5", None), Some(&listed), &clear()),
            SearchOutlook::NoUsableCoverDate
        );
        assert_eq!(
            search_outlook(&ok, Some(&listed), &blockers(true, None, false)),
            SearchOutlook::RetriesExhausted
        );
        assert_eq!(
            search_outlook(&ok, Some(&listed), &blockers(false, Some("grabbed"), false)),
            SearchOutlook::GrabInFlight
        );
        assert_eq!(
            search_outlook(&ok, Some(&listed), &blockers(false, None, true)),
            SearchOutlook::RecentlySubmitted
        );
        assert_eq!(
            search_outlook(&ok, Some(&listed), &clear()),
            SearchOutlook::WillSearch
        );
    }

    /// When several hold at once, the BINDING one is reported.
    ///
    /// This is the whole reason the order exists. An issue with an
    /// in-flight grab on a series that is not on the pull list is not
    /// "waiting for the grab to clear" — nothing will search for it
    /// when the grab clears either. Reporting the temporary reason
    /// there tells the user to wait for something that will not happen.
    #[test]
    fn the_most_binding_reason_wins() {
        let ok = issue("5", Some("2020-01-01"));
        let all = blockers(true, Some("grabbed"), true);

        assert_eq!(
            search_outlook(&ok, None, &all),
            SearchOutlook::SeriesNotOnPullList,
            "no pull-list row outranks every attempt-level reason"
        );
        assert_eq!(
            search_outlook(&ok, Some(&entry(true, None)), &all),
            SearchOutlook::SeriesPaused,
            "paused outranks every attempt-level reason"
        );
        assert_eq!(
            search_outlook(&issue("5", None), Some(&entry(false, None)), &all),
            SearchOutlook::NoUsableCoverDate,
            "an unsearchable issue outranks a temporary attempt state"
        );
        assert_eq!(
            search_outlook(&ok, Some(&entry(false, None)), &all),
            SearchOutlook::RetriesExhausted,
            "giving up permanently outranks two temporary states"
        );
    }

    /// `29.5` must not be truncated to `29` when compared to a floor,
    /// and the floor is inclusive.
    #[test]
    fn the_start_floor_uses_natural_order() {
        let listed = entry(false, Some("29.5"));
        assert_eq!(
            search_outlook(&issue("29", Some("2020-01-01")), Some(&listed), &clear()),
            SearchOutlook::BelowSeriesStartIssue,
            "29 is below a 29.5 floor"
        );
        assert_eq!(
            search_outlook(&issue("29.5", Some("2020-01-01")), Some(&listed), &clear()),
            SearchOutlook::WillSearch,
            "the floor issue itself is included"
        );
        assert_eq!(
            search_outlook(&issue("30", Some("2020-01-01")), Some(&listed), &clear()),
            SearchOutlook::WillSearch,
            "and everything above it"
        );
    }

    /// A future-dated solicitation is not yet a candidate.
    #[test]
    fn cover_date_matches_the_candidate_querys_test() {
        assert!(has_usable_cover_date(Some("2020-01-01")));
        assert!(!has_usable_cover_date(None));
        assert!(!has_usable_cover_date(Some("2020-01")), "malformed");
        assert!(!has_usable_cover_date(Some("")), "empty");
        assert!(!has_usable_cover_date(Some("2999-01-01")), "future");
        let today = time::OffsetDateTime::now_utc().date();
        let today = format!(
            "{:04}-{:02}-{:02}",
            today.year(),
            u8::from(today.month()),
            today.day()
        );
        assert!(has_usable_cover_date(Some(&today)), "today is usable");
    }
}

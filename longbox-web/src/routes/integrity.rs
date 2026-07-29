//! Library Integrity — discovery.
//!
//! # This module has exactly ONE write path
//!
//! Not "read-only" — that would be a claim the code does not support. The
//! precise property is: **one non-GET route, `POST …/analyze`, and its entire
//! write surface is `files.content_blake3`, `files.hashed_size_bytes`,
//! `files.hashed_mtime`, `files.archive_label` and `files.archive_label_kind`.**
//! Nothing here deletes a file, moves a file, re-points a binding, changes a
//! status, or triggers a rematch.
//!
//! That property is enforced two ways, because the obvious way is not enough:
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
//! Resolution — delete, relink, revert-to-missing, keeper selection — is
//! deliberately absent. It ships separately, designed against this module's
//! real output rather than a reconstruction of it.

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
}

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
        .map(|(p, s)| (p, matches!(s, Surface::Writes(_))))
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

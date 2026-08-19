//! Per-file processing pipeline. Owns the full lifecycle for one
//! watch-folder file (CBZ, CBR or PDF):
//! stability check → ComicInfo + filename extraction → match → either
//! (write ComicInfo, move to library, upsert as owned) OR (leave the
//! file where it sits in the watch folder and emit a `phase_b.skipped`
//! WARN with the reason). The watch folder IS the holding pen for
//! unplaceable files — there is no `_unsorted/` parking lot.
//!
//! Public for test reach. Production consumers go through
//! [`process_one`] directly.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use longbox_core::{
    classify_status, match_file, ComicInfo, ComicInfoMetadata, FileStatus, LibraryPath,
    MatchMethod, MetronInfoMetadata, ParsingPattern,
};
use longbox_db::{
    file_repo, find_candidates, issue_repo, parsing_pattern_repo, pull_attempt_repo, series_repo,
    IssueRow, Pool, SeriesRow,
};
use time::OffsetDateTime;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::PostprocessError;
use crate::intervention::InterventionReason;
use crate::Result;

/// Stability check: skip files whose mtime is within this many seconds
/// of now. The 2 s value is a fixed compromise — handles the common
/// `.partial → .cbz` rename case (downloader writes to .partial,
/// renames when complete; rename event arrives well after writes
/// settle) without trying to solve the direct-to-.cbz slow-write
/// edge (Phase B+ if it surfaces in practice).
const STABILITY_WINDOW: Duration = Duration::from_secs(2);

/// Process exactly one file. Errors are returned to the caller (the
/// consumer task) which logs and continues — a per-file failure does
/// not crash the pipeline.
///
/// `owned_threshold` is the live `match_confidence_threshold` value
/// the caller read from `settings` (see [`load_owned_threshold`]).
/// `min_file_size_mb` is the size floor — anything smaller is
/// presumed a partial/corrupt SAB delivery and gets rejected with a
/// WARN log; see [`load_min_file_size_mb`]. Both are passed as
/// parameters so `sweep_now` amortizes the SELECT across the whole
/// batch and `process_one` stays pure-ish.
pub async fn process_one(
    source: &Path,
    library_root: &Path,
    library_root_id: i64,
    db: &Pool,
    owned_threshold: f64,
    min_file_size_mb: u32,
) -> Result<Outcome> {
    let started_at = std::time::Instant::now();
    wait_for_stability(source).await;

    // The source file might disappear between detection and processing
    // (user moved it, another process deleted it). Recheck before
    // doing any work.
    let meta = match std::fs::metadata(source) {
        Ok(m) => m,
        Err(e) => return Err(PostprocessError::Io(e)),
    };
    let size = meta.len() as i64;

    // Size floor — reject suspicious partial deliveries BEFORE any
    // expensive work (archive open, ComicInfo parse, candidate
    // lookup). A complete-looking CBR that's 16 MB when it should be
    // 50+ MB is almost always SAB handing off a half-downloaded job.
    // Operator action: investigate or replace the bad NZB. File
    // stays in /watch/.
    let threshold_bytes = i64::from(min_file_size_mb) * 1024 * 1024;
    if size < threshold_bytes {
        let outcome = Outcome::RejectedTooSmall {
            size,
            threshold_mb: min_file_size_mb,
        };
        log_outcome(&outcome, started_at, source);
        return Ok(outcome);
    }

    let mtime = system_time_to_offset(meta.modified()?);

    let patterns = load_patterns(db).await?;

    // TIER 1: folder-name match. SAB creates one job folder per
    // download whose name IS the search query LongBox sent the
    // indexer — so by construction it carries `{series} {issue}` in
    // a form the parser can recognise. The scene group then renames
    // the inner file to whatever it wants (`Thunder.007.(2025).
    // (Zone-Empire).cbr` inside a `Blood & Thunder 7/` folder; the
    // filename loses "Blood &" entirely). Trying the folder first
    // short-circuits a whole class of "scene-name dropped a token"
    // skip-to-/watch failures.
    //
    // The folder match overrides ComicInfo and filename — if the
    // SAB job folder matches a catalog issue at the owned-confidence
    // threshold, we take that match unconditionally. ComicInfo may
    // point at a different volume (CV-URL drift on re-releases),
    // and the scene-stripped filename may be ambiguous; the folder
    // is the user's authoritative signal because it mirrors the
    // search query LongBox itself just issued.
    if let Some(folder_issue_id) = try_folder_match(source, db, &patterns, owned_threshold).await? {
        let outcome = import_as_owned(
            source,
            folder_issue_id,
            library_root,
            library_root_id,
            size,
            mtime,
            db,
        )
        .await?;
        log_outcome(&outcome, started_at, source);
        return Ok(outcome);
    }

    // TIER 2: existing ComicInfo + filename flow. Reached when Tier
    // 1 had no folder context (file dropped directly in /watch/), no
    // catalog series matched the folder-name parse at the owned
    // threshold, or the folder name itself was unparseable.
    let comic_info_xml = read_comic_info_xml(source)?;
    let comic_info = comic_info_xml
        .as_deref()
        .and_then(|xml| ComicInfo::parse(xml.as_bytes()).ok());

    let basename = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    // Cascade through the dot-to-space normalizer when the strict
    // patterns can't claim the raw basename — covers scene/NZB-style
    // names like `Absolute.Green.Lantern.007.(2025).(Digital).cbz`
    // that would otherwise dead-end at `_unsorted/` because the
    // patterns require space separators between tokens.
    let filename_parse = longbox_core::parse_filename_with_normalization(basename, &patterns);

    let title_hint = comic_info
        .as_ref()
        .and_then(|c| c.series.clone())
        .or_else(|| filename_parse.as_ref().map(|p| p.series_title.clone()));

    let Some(hint) = title_hint.filter(|s| !s.trim().is_empty()) else {
        // No usable hint at all — neither the embedded ComicInfo nor
        // the filename parser produced a series title to look up.
        // The file stays in the watch folder; the operator can place
        // it manually or delete it.
        let outcome = Outcome::Skipped {
            reason: "no series hint from filename or ComicInfo".into(),
        };
        log_outcome(&outcome, started_at, source);
        return Ok(outcome);
    };

    let year_hint = comic_info
        .as_ref()
        .and_then(|c| c.year)
        .or_else(|| filename_parse.as_ref().and_then(|p| p.year));

    let candidates = find_candidates(db, &hint, year_hint).await?;
    // A watch-folder file has no library series folder, so there is nothing
    // there to be silent — its absence carries no information and must not
    // trigger the volume abstention. The SAB job folder is NOT a substitute:
    // it mirrors a scene release name and is parsed with the very same
    // filename patterns (see `try_folder_match`), so its year is a release
    // year, on the wrong side of the volume-evidence line.
    let match_result = match_file(
        comic_info.as_ref(),
        filename_parse.as_ref(),
        longbox_core::matcher::FolderEvidence::NoFolder,
        &candidates,
    );

    // Phase B owned-classification uses the live
    // `match_confidence_threshold` from settings (the same row the
    // scanner reads per scan run). Needs-review-tier matches stay in
    // the watch folder rather than silently claiming status='owned'.
    let status = classify_status(
        match_result.issue_id,
        match_result.confidence,
        match_result.method,
        owned_threshold,
        match_result.ambiguous,
    );

    // Pull-engine trust override: a sub-threshold match for an issue
    // we ourselves asked an indexer for is accepted as Owned. The
    // pull engine already cleared the series-title similarity
    // filter at submit time, so the local confidence floor is
    // redundant evidence we can afford to skip. See
    // `pull_attempt_repo::issue_has_attempt` for the rationale.
    // …but an *ambiguous* match is never trusted through, pull attempt or not.
    // The pull-list override exists to forgive a weak *confidence* score on an
    // issue we ourselves asked for. Ambiguity isn't weak confidence — it's the
    // file's ComicInfo and its filename naming two different issues, and
    // nothing about a pull attempt adjudicates that. It matters here more than
    // anywhere: `import_as_owned` doesn't just set a status, it MOVES the
    // archive into the target issue's canonical path and REWRITES its embedded
    // metadata. Guessing wrong is not a row we can flip back.
    let trust_via_pull = match match_result.issue_id {
        Some(_) if match_result.ambiguous => false,
        Some(id) => pull_attempt_repo::issue_has_attempt(db, id).await?,
        None => false,
    };

    let outcome = match (match_result.issue_id, status, trust_via_pull) {
        (Some(issue_id), FileStatus::Owned, _) => {
            import_as_owned(
                source,
                issue_id,
                library_root,
                library_root_id,
                size,
                mtime,
                db,
            )
            .await?
        }
        (Some(issue_id), _, true) => {
            tracing::info!(
                target: "longbox_postprocess",
                source = %source.display(),
                issue_id,
                confidence = match_result.confidence,
                threshold = owned_threshold,
                method = ?match_result.method,
                "phase_b.match_trusted_via_pull_attempt"
            );
            import_as_owned(
                source,
                issue_id,
                library_root,
                library_root_id,
                size,
                mtime,
                db,
            )
            .await?
        }
        // An ambiguous match usually scores ABOVE the threshold (0.90 vs
        // 0.85) — saying "below owned threshold" would be a flat lie in the
        // logs of exactly the files someone is trying to debug.
        (Some(_), _, false) if match_result.ambiguous => Outcome::Skipped {
            reason: format!(
                "ambiguous match for series hint {hint:?}: this file's ComicInfo \
                 and its filename name different issues (confidence={:.2}) — \
                 left in the watch folder for a human",
                match_result.confidence
            ),
        },
        (Some(_), _, false) => Outcome::Skipped {
            reason: format!(
                "needs-review-tier match for series hint {hint:?} \
                 (confidence={:.2}, method={:?}); below owned threshold {:.2}",
                match_result.confidence, match_result.method, owned_threshold
            ),
        },
        (None, _, _) => Outcome::Skipped {
            reason: format!(
                "no catalog match for series hint {hint:?} \
                 (candidates considered: {}, year_hint: {:?})",
                candidates.len(),
                year_hint
            ),
        },
    };

    log_outcome(&outcome, started_at, source);
    Ok(outcome)
}

/// What happened to the file. Returned so callers (and tests) can
/// assert on the result without scraping logs.
///
/// `Imported` is the only outcome that moves the source; everything
/// else leaves the file where it sits in the watch folder. The cache
/// disposition is uniform across non-Failed outcomes (evict any prior
/// entry for the source path) — the watch folder IS the holding pen,
/// the pending-intervention list exists only for Failed (stage-specific
/// errors needing operator action). `size` rides along on stuck
/// variants so the consumer doesn't have to re-stat the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Matched + moved + catalogued as owned.
    Imported {
        target: PathBuf,
        series_id: i64,
        issue_id: i64,
        file_id: i64,
    },
    /// No usable hint, no catalog match, or a sub-threshold (needs-review)
    /// match. The file stays in the watch folder; no DB write, no disk
    /// move. `reason` is the WARN-log payload. Replaces the old
    /// `Unsorted { target, file_id }` variant per Jeremy's directive
    /// to eliminate `_unsorted/` — the watch folder is the holding pen
    /// for unplaceable files.
    Skipped { reason: String },
    /// Target path already existed; the duplicate source was removed
    /// from the watch folder by `cleanup_conflict_source`. No pending
    /// intervention — the library has canonical bytes.
    Conflict { target: PathBuf, size: i64 },
    /// Stage-specific failure during the owned-import flow (ComicInfo
    /// write, move). The source stays in the watch folder and the
    /// outcome pushes a `PendingIntervention` so the operator sees it
    /// on the dashboard.
    Failed {
        reason: InterventionReason,
        target: PathBuf,
        size: i64,
    },
    /// File size is below `min_file_size_mb` — almost certainly a
    /// partial/corrupt SAB delivery (e.g. an Absolute Batman issue
    /// that should be 50+ MB arriving at 16 MB with five rendered
    /// pages). The file stays in the watch folder; no archive open,
    /// no ComicInfo parse, no candidate lookup happened. WARN log
    /// `phase_b.rejected_too_small` carries the size + threshold so
    /// the operator can investigate or replace the bad download.
    RejectedTooSmall { size: i64, threshold_mb: u32 },
}

async fn wait_for_stability(source: &Path) {
    let Ok(meta) = std::fs::metadata(source) else {
        return;
    };
    let Ok(mtime) = meta.modified() else {
        return;
    };
    let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
    if age < STABILITY_WINDOW {
        tokio::time::sleep(STABILITY_WINDOW - age).await;
    }
}

async fn load_patterns(db: &Pool) -> Result<Vec<ParsingPattern>> {
    let rows = parsing_pattern_repo::list_enabled(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| ParsingPattern {
            id: r.id,
            name: r.name,
            pattern: r.pattern,
            priority: i32::try_from(r.priority).unwrap_or(i32::MAX),
            enabled: r.enabled,
        })
        .collect())
}

/// Read `ComicInfo.xml` from the source archive — CBZ or CBR — via
/// `longbox-archive`, so the ZIP/RAR dispatch lives in exactly one
/// place. `Ok(None)` means the archive carries no ComicInfo (the
/// untagged common case), which is also every PDF: a PDF is matched on
/// its filename or its SAB job folder alone. A genuinely unreadable
/// archive (corrupt ZIP/RAR, I/O failure, non-UTF-8 ComicInfo payload)
/// propagates as an error, leaving the file in the watch folder for
/// manual intervention.
fn read_comic_info_xml(source: &Path) -> Result<Option<String>> {
    Ok(longbox_archive::read_comic_info(source)?)
}

/// Best-effort unlink of a watch-folder source after a Phase B conflict.
/// On success emits `phase_b.conflict_source_removed` at WARN — the
/// outcome is both expected (library has canonical bytes) and worth
/// logging at default verbosity so operators can audit what got
/// auto-cleaned. On failure emits `phase_b.conflict_cleanup_failed`,
/// also WARN; the caller still returns `Outcome::Conflict` so the
/// outer cache logic can react.
///
/// Same-path short-circuit: if `source` and `target` resolve to the
/// same file (re-processing a file that's already sitting at its
/// canonical library path — see `pipeline::idempotent_reprocessing`),
/// deleting the source would destroy the library's canonical bytes.
/// Detect that case and no-op. Comparison goes through `canonicalize`
/// so symlinks, `.` segments, and relative-vs-absolute differences
/// don't fool it; PathBuf equality is the fallback when one or both
/// canonicalize calls fail (e.g. target only existed up to the dir).
/// TIER 1 matcher. Pull `source`'s parent-directory name (the SAB
/// job folder), feed it through the same parser-with-normalization
/// the file path uses, and look up the resulting series+number in
/// the catalog. Returns `Some(issue_id)` only when the match clears
/// the owned-confidence threshold — partial-confidence matches fall
/// through to Tier 2 so ComicInfo can still rescue them.
///
/// Returns `None` (without an error) for every benign skip case:
///   - source has no parent (`/file.cbz` style — unreachable in
///     practice, defensive),
///   - source is at the watch root directly (no job folder context
///     — the parent folder name is "watch" or whatever the operator
///     called the bind mount),
///   - the folder name is empty or unparseable,
///   - the parsed series has no catalog hit,
///   - the matcher returns sub-threshold confidence.
async fn try_folder_match(
    source: &Path,
    db: &Pool,
    patterns: &[ParsingPattern],
    owned_threshold: f64,
) -> Result<Option<i64>> {
    let Some(parent) = source.parent() else {
        return Ok(None);
    };
    let Some(folder_name) = parent.file_name().and_then(|n| n.to_str()) else {
        return Ok(None);
    };
    if folder_name.trim().is_empty() {
        return Ok(None);
    }

    // Append `.cbz` so the parser's `\.(?i:cbz|cbr|cb7|pdf)$` anchor
    // matches. The folder doesn't have an extension; the parser
    // doesn't care that the trailing `.cbz` is synthetic — it's
    // shape, not provenance. Same trick the newznab pull engine
    // uses for indexer-returned search-result titles.
    let synthetic = format!("{folder_name}.cbz");
    let Some(folder_parse) = longbox_core::parse_filename_with_normalization(&synthetic, patterns)
    else {
        return Ok(None);
    };
    let hint = folder_parse.series_title.trim();
    if hint.is_empty() {
        return Ok(None);
    }

    let candidates = find_candidates(db, hint, folder_parse.year).await?;
    if candidates.is_empty() {
        return Ok(None);
    }
    // No ComicInfo passed — Tier 1 is explicitly folder-driven; the
    // file's embedded metadata gets a chance in Tier 2 if the
    // folder doesn't carry us home.
    // The job folder's year already arrives as the year hint via
    // `folder_parse.year`; it is not a library series folder.
    let match_result = match_file(
        None,
        Some(&folder_parse),
        longbox_core::matcher::FolderEvidence::NoFolder,
        &candidates,
    );
    let status = classify_status(
        match_result.issue_id,
        match_result.confidence,
        match_result.method,
        owned_threshold,
        match_result.ambiguous,
    );
    // Pull-engine trust override (mirrors Tier 2 in `process_one`):
    // a sub-threshold folder match for an issue with pull-attempt
    // history is accepted as Owned. The pull engine's indexer-time
    // series-title filter already cleared the title; the local
    // floor is redundant.
    //
    // The ambiguous arm mirrors `process_one` too, and is deliberately not
    // omitted as dead code. It IS dead today — this path calls `match_file`
    // with `comic_info: None`, and `ambiguous` is only ever set where BOTH
    // tiers resolve, so Tier 2 returning None makes it structurally
    // unreachable. But "unreachable" here is a property of one argument at one
    // call site, not of this function. The day anyone threads ComicInfo into
    // the folder match, the hole silently reopens — and it reopens onto
    // `import_as_owned`, which MOVES the archive and REWRITES its metadata.
    // A guess there is not a row you flip back. One line to make it
    // unreachable for a reason instead of by luck.
    let trust_via_pull = match match_result.issue_id {
        Some(_) if match_result.ambiguous => false,
        Some(id) => pull_attempt_repo::issue_has_attempt(db, id).await?,
        None => false,
    };
    let issue_id = match (match_result.issue_id, status, trust_via_pull) {
        (Some(id), FileStatus::Owned, _) => id,
        (Some(id), _, true) => {
            tracing::info!(
                target: "longbox_postprocess",
                source = %source.display(),
                folder_name,
                issue_id = id,
                confidence = match_result.confidence,
                threshold = owned_threshold,
                "phase_b.folder_match_trusted_via_pull_attempt"
            );
            id
        }
        _ => {
            tracing::debug!(
                target: "longbox_postprocess",
                source = %source.display(),
                folder_name,
                series_hint = hint,
                confidence = match_result.confidence,
                threshold = owned_threshold,
                "phase_b.folder_match_below_threshold (falling through to Tier 2)"
            );
            return Ok(None);
        }
    };

    tracing::info!(
        target: "longbox_postprocess",
        source = %source.display(),
        folder_name,
        issue_id,
        confidence = match_result.confidence,
        method = ?match_result.method,
        "phase_b.folder_match_hit"
    );
    Ok(Some(issue_id))
}

fn cleanup_conflict_source(source: &Path, target: &Path, size: i64) {
    let same_file = match (std::fs::canonicalize(source), std::fs::canonicalize(target)) {
        (Ok(a), Ok(b)) => a == b,
        _ => source == target,
    };
    if same_file {
        tracing::debug!(
            target: "longbox_postprocess",
            source = %source.display(),
            "phase_b.conflict_cleanup_skipped (source IS target)"
        );
        return;
    }
    match std::fs::remove_file(source) {
        Ok(()) => tracing::warn!(
            target: "longbox_postprocess",
            source = %source.display(),
            target = %target.display(),
            size,
            "phase_b.conflict_source_removed"
        ),
        Err(e) => tracing::warn!(
            target: "longbox_postprocess",
            source = %source.display(),
            target = %target.display(),
            err = %e,
            "phase_b.conflict_cleanup_failed"
        ),
    }
}

async fn import_as_owned(
    source: &Path,
    issue_id: i64,
    library_root: &Path,
    library_root_id: i64,
    size: i64,
    mtime: OffsetDateTime,
    db: &Pool,
) -> Result<Outcome> {
    let issue =
        issue_repo::find_by_id(db, issue_id)
            .await?
            .ok_or(PostprocessError::CatalogNotFound {
                what: format!("issue {issue_id}"),
            })?;
    let series = series_repo::find_by_id(db, issue.series_id).await?.ok_or(
        PostprocessError::CatalogNotFound {
            what: format!("series {}", issue.series_id),
        },
    )?;

    let library_path = LibraryPath::new(
        &series.title,
        series.start_year.map(|y| y as i32),
        issue.number.as_str(),
    );
    // The convention's extension is `.cbz` because every import that CAN be
    // repacked is — a CBR is converted on the way in. A PDF cannot be: it is
    // read-only forever, so it keeps its own extension and lands verbatim.
    // Same folder, same filename, different container.
    let target_abs = if longbox_archive::is_pdf(source) {
        library_path.full(library_root).with_extension("pdf")
    } else {
        library_path.full(library_root)
    };

    if target_abs.exists() {
        // The library already owns this exact (series, issue). The
        // source in the watch folder is — by definition of the path
        // collision — a duplicate of canonical bytes already on disk.
        // Leaving it pending forever (the prior behavior) just clogs
        // the complete folder; clean it up so SAB / the watcher don't
        // re-fire it on the next sweep. Failure to delete is logged
        // and treated as resolved anyway — the catalog is what matters
        // and operator action on the stale file is easy.
        cleanup_conflict_source(source, &target_abs, size);
        return Ok(Outcome::Conflict {
            target: target_abs,
            size,
        });
    }

    let metadata = compose_metadata(&series, &issue);
    let xml = metadata.to_xml();
    // MetronInfo.xml lives alongside ComicInfo.xml in the same archive
    // for maximum compatibility (Perdoo, ComicRack CE, Comicbox, Codex,
    // Metron-Tagger all read it). Built from the same (SeriesRow,
    // IssueRow) — no extra DB queries.
    let metron_metadata = compose_metroninfo_metadata(&series, &issue);
    let metron_xml = metron_metadata.to_xml();

    // Two-stage move so the consumer can distinguish ComicInfo-write
    // failures from move failures in the pending-intervention list.
    // Stage 1: rewrite source CBZ into a temp file in the target dir
    // (no rename yet). Stage 2: persist the temp to the final path and
    // unlink the source. Each stage is its own spawn_blocking so the
    // failure surface is precise.
    let source_owned = source.to_path_buf();
    let target_owned = target_abs.clone();
    let rewrite_res = tokio::task::spawn_blocking(move || {
        rewrite_to_temp(&source_owned, &target_owned, &xml, &metron_xml)
    })
    .await
    .map_err(|e| PostprocessError::Io(std::io::Error::other(format!("join: {e}"))))?;

    let temp = match rewrite_res {
        Ok(t) => t,
        Err(RewriteFailure::SourceUnreadable(e)) => {
            // The download itself is bad. Settle any in-flight attempt
            // NOW with what actually happened, instead of leaving it to
            // age out three sweeps later as "lost track of download".
            // Symmetric with the `mark_grabbed_for_issue` call on the
            // success path below.
            let message = format!("downloaded archive is unreadable: {e}");
            let settled =
                pull_attempt_repo::record_failure_for_issue(db, series.id, issue.id, &message)
                    .await?;
            if settled > 0 {
                tracing::warn!(
                    target: "longbox_postprocess",
                    series_id = series.id,
                    issue_id = issue.id,
                    attempts = settled,
                    error = %e,
                    "phase_b.pull_failed_corrupt_download"
                );
            }
            return Ok(Outcome::Failed {
                reason: InterventionReason::SourceArchiveUnreadable(e.to_string()),
                target: target_abs,
                size,
            });
        }
        Err(other) => {
            // Local write problem — a full disk, a read-only library.
            // The release is fine; leave the attempt alone so it retries
            // once the local cause is fixed.
            return Ok(Outcome::Failed {
                reason: InterventionReason::ComicInfoWriteFailed(other.into_error().to_string()),
                target: target_abs,
                size,
            });
        }
    };

    let source_owned = source.to_path_buf();
    let target_owned = target_abs.clone();
    let commit_res =
        tokio::task::spawn_blocking(move || commit_move(temp, &target_owned, &source_owned))
            .await
            .map_err(|e| PostprocessError::Io(std::io::Error::other(format!("join: {e}"))))?;

    if let Err(e) = commit_res {
        return Ok(Outcome::Failed {
            reason: InterventionReason::MoveFailed(e.to_string()),
            target: target_abs,
            size,
        });
    }

    let path_relative = target_abs
        .strip_prefix(library_root)
        .map_err(|_| {
            PostprocessError::Io(std::io::Error::other(
                "computed target falls outside library_root",
            ))
        })?
        .to_string_lossy()
        .into_owned();

    // Re-stat AFTER the rewrite. `size` / `mtime` describe the file in the
    // watch folder, and the file that just landed in the library is not that
    // file: ComicInfo.xml and MetronInfo.xml were injected, and a CBR source
    // was decompressed and recompressed as a CBZ — a large delta, not a
    // rounding error. Cataloguing the source's numbers records metadata for
    // bytes that no longer exist anywhere.
    //
    // That matters beyond tidiness. Duplicate detection groups files by size
    // before comparing content, so a row carrying its source's size never
    // lands in the same group as its true twin and the duplicate is never
    // found — blinding the detector precisely where new duplicates enter the
    // library, since Phase B import is how they get here.
    //
    // A stat failure on a file we just wrote successfully is strange but not
    // worth turning into an orphan: returning early here would leave the file
    // on disk with no catalog row at all. Fall back to the source values,
    // loudly, and let the next scan correct them.
    let (size, mtime) = match std::fs::metadata(&target_abs) {
        Ok(m) => (
            i64::try_from(m.len()).unwrap_or(i64::MAX),
            m.modified().map(OffsetDateTime::from).unwrap_or(mtime),
        ),
        Err(e) => {
            tracing::warn!(
                target: "longbox_postprocess",
                path = %target_abs.display(),
                error = %e,
                "phase_b.post_rewrite_stat_failed"
            );
            (size, mtime)
        }
    };

    // Pull-engine attribution: a file whose (series, issue) has an
    // in-flight `pull_attempt` was auto-downloaded by the Step 6 pull
    // engine — catalogue it `pull_list` and settle the attempt(s).
    // Otherwise it is an ordinary Phase B catch (`phase_b`).
    let pulled = pull_attempt_repo::has_in_flight_attempt(db, series.id, issue.id).await?;
    let match_method = if pulled {
        MatchMethod::PullList
    } else {
        MatchMethod::PhaseB
    };

    let row = file_repo::upsert_imported(
        db,
        library_root_id,
        &path_relative,
        series.id,
        issue.id,
        match_method.as_db_str(),
        size,
        mtime,
    )
    .await?;

    // Ghost-row cleanup: drop any prior `(library_root_id, issue.id)` rows
    // that point at a different path and are marked absent. Catches the
    // `_unsorted/<basename>` shape — a file dropped into a watched
    // sub-folder of the library, cataloged by the scanner, then moved
    // here by Phase B — without which the absent row resurfaces as a
    // duplicate `needs_review` on the next match sweep.
    let ghosts =
        file_repo::purge_absent_ghosts_for_issue(db, library_root_id, issue.id, &path_relative)
            .await?;
    if ghosts > 0 {
        tracing::info!(
            target: "longbox_postprocess",
            library_root_id,
            issue_id = issue.id,
            kept_path = %path_relative,
            ghosts,
            "phase_b.ghost_rows_purged"
        );
    }

    if pulled {
        // Multi-row by design: 2+ in-flight attempts for one issue (the
        // race the Step 3 brief calls out) all settle to `grabbed`.
        let settled = pull_attempt_repo::mark_grabbed_for_issue(db, series.id, issue.id).await?;
        tracing::info!(
            target: "longbox_postprocess",
            series_id = series.id,
            issue_id = issue.id,
            attempts = settled,
            "phase_b.pull_attributed"
        );
    }

    Ok(Outcome::Imported {
        target: target_abs,
        series_id: series.id,
        issue_id: issue.id,
        file_id: row.id,
    })
}

fn compose_metadata(series: &SeriesRow, issue: &IssueRow) -> ComicInfoMetadata {
    let web = issue
        .cv_issue_id
        .map(|id| format!("https://comicvine.gamespot.com/issue/4000-{id}/"));
    ComicInfoMetadata {
        series: series.title.clone(),
        number: issue.number.clone(),
        start_year: series.start_year.map(|y| y as i32),
        publisher: series.publisher.clone(),
        title: issue.title.clone(),
        web,
        summary: issue.summary.clone(),
    }
}

/// Build the MetronInfo write set from the same catalog rows. Mirrors
/// [`compose_metadata`] — same inputs, different schema. `sort_title`
/// only surfaces as `<SortName>` when it differs from the canonical
/// title; the writer omits the element when they're equal.
fn compose_metroninfo_metadata(series: &SeriesRow, issue: &IssueRow) -> MetronInfoMetadata {
    let series_sort = if series.sort_title == series.title {
        None
    } else {
        Some(series.sort_title.clone())
    };
    MetronInfoMetadata {
        cv_issue_id: issue.cv_issue_id,
        metron_issue_id: issue.metron_issue_id.clone(),
        publisher: series.publisher.clone(),
        cv_series_id: series.cv_id,
        series: series.title.clone(),
        series_sort,
        start_year: series.start_year.map(|y| y as i32),
        number: issue.number.clone(),
        summary: issue.summary.clone(),
        last_modified: OffsetDateTime::now_utc(),
    }
}

/// Whether `name` is one of the metadata entries Phase B regenerates
/// on import. Case-insensitive — different writers use different
/// casing, and we want to drop every variant before re-emitting the
/// canonical form. Anything matching here is skipped during the
/// rewrite so the output archive contains only LongBox's freshly-
/// written copies.
fn is_metadata_entry(name: &str) -> bool {
    name.eq_ignore_ascii_case("ComicInfo.xml") || name.eq_ignore_ascii_case("MetronInfo.xml")
}

/// Stage 1 of the rewrite-and-move flow: re-emit the source archive
/// into a fresh ZIP temp sited next to the target (so the eventual
/// rename is same-filesystem), with freshly regenerated
/// `ComicInfo.xml` and `MetronInfo.xml` replacing any the source
/// carried. Both files land at the archive root (the spec for each
/// format — readers don't search subdirectories). Returns the
/// `NamedTempFile` for stage 2 to persist.
///
/// A **CBZ** source is raw-copied entry-by-entry — compressed bytes
/// move across without recompression. A **CBR** source is read via
/// `longbox-archive` (libunrar): there is no RAR writer, so a matched
/// CBR is necessarily converted here — its entries are decompressed
/// and recompressed into the ZIP. Either way the output is a `.cbz`.
///
/// A **PDF** is the exception to all of that: it is copied byte-for-byte
/// and stays a `.pdf`. Nothing is injected into it, because a PDF is
/// read-only forever — the catalog is the only place its metadata lives.
///
/// All errors here are "ComicInfoWriteFailed" semantically — we
/// couldn't produce the rewritten archive. The temp drops on Err and
/// the source stays where it was.
/// Which side of the rewrite failed.
///
/// Load-bearing, and not derivable from the error type alone: reading a
/// source CBZ and writing the target CBZ both raise `ZipError`, and
/// opening the source and creating the temp both raise `io::Error`. The
/// only place the distinction exists is at the callsite, so it is
/// recorded there rather than reconstructed later from a message.
pub(crate) enum RewriteFailure {
    /// The downloaded archive could not be read. A verdict about the
    /// release.
    SourceUnreadable(PostprocessError),
    /// Creating, writing or finishing the target. A local problem.
    LocalWrite(PostprocessError),
}

impl RewriteFailure {
    pub(crate) fn into_error(self) -> PostprocessError {
        match self {
            Self::SourceUnreadable(e) | Self::LocalWrite(e) => e,
        }
    }
}

fn rewrite_to_temp(
    source: &Path,
    target: &Path,
    comic_info_xml: &str,
    metron_info_xml: &str,
) -> std::result::Result<tempfile::NamedTempFile, RewriteFailure> {
    use RewriteFailure::{LocalWrite, SourceUnreadable};
    let local = |e: PostprocessError| LocalWrite(e);
    let src = |e: PostprocessError| SourceUnreadable(e);

    let target_dir = target.parent().ok_or_else(|| {
        local(PostprocessError::Io(std::io::Error::other(
            "target has no parent directory",
        )))
    })?;
    std::fs::create_dir_all(target_dir).map_err(|e| local(e.into()))?;

    let is_pdf = longbox_archive::is_pdf(source);

    let temp = tempfile::Builder::new()
        .prefix(".longbox-phase-b-")
        .suffix(if is_pdf { ".pdf.tmp" } else { ".cbz.tmp" })
        .tempfile_in(target_dir)
        .map_err(|e| local(e.into()))?;

    if is_pdf {
        // A PDF is staged verbatim — nothing to rewrite. There is no
        // ComicInfo/MetronInfo to inject (no PDF writer exists, and none
        // will) and no archive to re-emit, so the bytes cross unchanged.
        // It still goes through the temp rather than renaming the source
        // straight to the target: /watch and the library are routinely
        // separate mounts, and stage 2's rename has to stay same-filesystem.
        let mut source_file = std::fs::File::open(source).map_err(|e| src(e.into()))?;
        std::io::copy(&mut source_file, &mut temp.as_file()).map_err(|e| local(e.into()))?;
        return Ok(temp);
    }

    {
        let mut writer = ZipWriter::new(temp.as_file());
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        if longbox_archive::is_rar(source) {
            // CBR: no RAR writer exists, so decompress every entry via
            // libunrar and recompress it into the ZIP.
            for entry in longbox_archive::read_entries(source).map_err(|e| src(e.into()))? {
                if is_metadata_entry(&entry.name) {
                    continue;
                }
                writer
                    .start_file(&entry.name, deflated)
                    .map_err(|e| local(e.into()))?;
                writer.write_all(&entry.data).map_err(|e| local(e.into()))?;
            }
        } else {
            // CBZ: raw-copy compressed entries, no recompression.
            let src_file = std::fs::File::open(source).map_err(|e| src(e.into()))?;
            let mut archive = ZipArchive::new(src_file).map_err(|e| src(e.into()))?;
            for i in 0..archive.len() {
                let entry = archive.by_index(i).map_err(|e| src(e.into()))?;
                if is_metadata_entry(entry.name()) {
                    continue;
                }
                writer.raw_copy_file(entry).map_err(|e| local(e.into()))?;
            }
        }

        writer
            .start_file("ComicInfo.xml", deflated)
            .map_err(|e| local(e.into()))?;
        writer
            .write_all(comic_info_xml.as_bytes())
            .map_err(|e| local(e.into()))?;
        writer
            .start_file("MetronInfo.xml", deflated)
            .map_err(|e| local(e.into()))?;
        writer
            .write_all(metron_info_xml.as_bytes())
            .map_err(|e| local(e.into()))?;
        writer.finish().map_err(|e| local(e.into()))?;
    }

    Ok(temp)
}

/// Stage 2 of the rewrite-and-move flow: rename the temp into place,
/// then unlink the source. `NamedTempFile::persist` is an atomic
/// same-fs rename; on error the temp drops and cleans itself up.
///
/// All errors here are "MoveFailed" semantically.
fn commit_move(temp: tempfile::NamedTempFile, target: &Path, source: &Path) -> Result<()> {
    temp.persist(target)
        .map_err(|e| PostprocessError::Io(e.error))?;
    std::fs::remove_file(source)?;
    Ok(())
}

fn system_time_to_offset(t: SystemTime) -> OffsetDateTime {
    OffsetDateTime::from(t)
}

fn log_outcome(outcome: &Outcome, started_at: std::time::Instant, source: &Path) {
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Outcome::Imported {
            target,
            series_id,
            issue_id,
            file_id,
        } => {
            tracing::info!(
                target: "longbox_postprocess",
                source = %source.display(),
                target = %target.display(),
                series_id,
                issue_id,
                file_id,
                duration_ms,
                "phase_b.processed"
            );
        }
        Outcome::Skipped { reason } => {
            // WARN per Jeremy's directive: unplaceable files staying in
            // /watch/ should be visible at the default log verbosity so
            // the operator can audit what didn't get filed. Includes
            // the basename (not the full /watch/.../filename path) to
            // keep grep ergonomics close to what a user would see in
            // the file browser.
            let basename = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unparseable basename>");
            tracing::warn!(
                target: "longbox_postprocess",
                source = %source.display(),
                basename,
                reason = %reason,
                duration_ms,
                "phase_b.skipped (left in watch folder)"
            );
        }
        Outcome::Conflict { target, .. } => {
            tracing::warn!(
                target: "longbox_postprocess",
                source = %source.display(),
                target = %target.display(),
                reason = "conflict",
                "phase_b.skipped (conflict; source cleaned up)"
            );
        }
        Outcome::Failed { reason, target, .. } => {
            tracing::warn!(
                target: "longbox_postprocess",
                source = %source.display(),
                target = %target.display(),
                reason = ?reason,
                duration_ms,
                "phase_b.failed"
            );
        }
        Outcome::RejectedTooSmall { size, threshold_mb } => {
            // WARN at default verbosity per the spec — the operator
            // needs to see when SAB delivers a half-downloaded file.
            // Format: `file=X size=NMB threshold=NMB` matches the
            // user-stated log shape.
            let size_mb = *size / (1024 * 1024);
            let basename = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unparseable basename>");
            tracing::warn!(
                target: "longbox_postprocess",
                file = basename,
                source = %source.display(),
                size_mb,
                size_bytes = size,
                threshold_mb,
                duration_ms,
                "phase_b.rejected_too_small (left in watch folder)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn series_fixture() -> SeriesRow {
        SeriesRow {
            id: 1,
            cv_id: Some(42215),
            metron_id: Some("10959".into()),
            title: "Saga".into(),
            sort_title: "Saga".into(),
            start_year: Some(2012),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
            created_at: datetime!(2026-01-01 00:00:00),
            updated_at: datetime!(2026-01-01 00:00:00),
        }
    }

    fn issue_fixture() -> IssueRow {
        IssueRow {
            id: 10,
            series_id: 1,
            cv_issue_id: Some(364354),
            metron_issue_id: Some("99999".into()),
            number: "1".into(),
            title: Some("The Will".into()),
            cover_date: Some("2012-03-14".into()),
            summary: Some("Galactic war epic.".into()),
            cover_url: None,
            created_at: datetime!(2026-01-01 00:00:00),
            updated_at: datetime!(2026-01-01 00:00:00),
        }
    }

    #[test]
    fn compose_full_fixture_populates_every_field() {
        let m = compose_metroninfo_metadata(&series_fixture(), &issue_fixture());
        assert_eq!(m.cv_issue_id, Some(364354));
        assert_eq!(m.metron_issue_id.as_deref(), Some("99999"));
        assert_eq!(m.publisher.as_deref(), Some("Image"));
        assert_eq!(m.cv_series_id, Some(42215));
        assert_eq!(m.series, "Saga");
        // sort_title == title, so series_sort lands as None.
        assert_eq!(m.series_sort, None);
        assert_eq!(m.start_year, Some(2012));
        assert_eq!(m.number, "1");
        assert_eq!(m.summary.as_deref(), Some("Galactic war epic."));
    }

    #[test]
    fn compose_with_no_cv_issue_id_leaves_field_none() {
        let issue = IssueRow {
            cv_issue_id: None,
            ..issue_fixture()
        };
        let m = compose_metroninfo_metadata(&series_fixture(), &issue);
        assert_eq!(m.cv_issue_id, None);
        // Metron path still populates its ID.
        assert_eq!(m.metron_issue_id.as_deref(), Some("99999"));
    }

    #[test]
    fn compose_with_no_metron_issue_id_leaves_field_none() {
        let issue = IssueRow {
            metron_issue_id: None,
            ..issue_fixture()
        };
        let m = compose_metroninfo_metadata(&series_fixture(), &issue);
        assert_eq!(m.metron_issue_id, None);
        // CV side still populates.
        assert_eq!(m.cv_issue_id, Some(364354));
    }

    #[test]
    fn compose_distinct_sort_title_surfaces_as_series_sort() {
        let series = SeriesRow {
            title: "The Walking Dead Deluxe".into(),
            sort_title: "Walking Dead Deluxe".into(),
            ..series_fixture()
        };
        let m = compose_metroninfo_metadata(&series, &issue_fixture());
        assert_eq!(m.series_sort.as_deref(), Some("Walking Dead Deluxe"));
    }

    #[test]
    fn is_metadata_entry_catches_both_filenames_case_insensitively() {
        for n in [
            "ComicInfo.xml",
            "comicinfo.xml",
            "COMICINFO.XML",
            "MetronInfo.xml",
            "metroninfo.xml",
            "METRONINFO.XML",
        ] {
            assert!(is_metadata_entry(n), "{n} should be treated as metadata");
        }
        for n in [
            "page-001.jpg",
            "subdir/ComicInfo.xml", // path-prefixed: NOT a root metadata entry
            "ComicInfo.json",
            "MetronInfo",
        ] {
            assert!(!is_metadata_entry(n), "{n} should not match");
        }
    }
}

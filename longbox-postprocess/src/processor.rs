//! Per-file processing pipeline. Owns the full lifecycle for one CBZ:
//! stability check → ComicInfo + filename extraction → match → either
//! (write ComicInfo, move to library, upsert as owned) OR
//! (move to `_unsorted/`, insert as unmatched). Errors at any stage
//! land as `phase_b.failed` logs at the consumer caller, file left
//! where it sits for manual intervention.
//!
//! Public for test reach. Production consumers go through
//! [`process_one`] directly.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use longbox_core::{
    classify_status, match_file, ComicInfo, ComicInfoMetadata, CoverDate, FileStatus,
    LibraryPath, MatchMethod, ParsingPattern,
};
use longbox_db::{
    file_repo, find_candidates, issue_repo, parsing_pattern_repo, series_repo, FileRow,
    FileUpdate, IssueRow, NewFile, Pool, SeriesRow,
};
use time::{OffsetDateTime, PrimitiveDateTime};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::PostprocessError;
use crate::Result;

/// Stability check: skip files whose mtime is within this many seconds
/// of now. The 2 s value is a fixed compromise — handles the common
/// `.partial → .cbz` rename case (downloader writes to .partial,
/// renames when complete; rename event arrives well after writes
/// settle) without trying to solve the direct-to-.cbz slow-write
/// edge (Phase B+ if it surfaces in practice).
const STABILITY_WINDOW: Duration = Duration::from_secs(2);

/// Phase B match-threshold floor for owned classification. Below this,
/// the file lands in `_unsorted/` even if the matcher returned a
/// non-Unmatched method. Set to Phase A's `DEFAULT_MATCH_THRESHOLD` so
/// needs-review-tier matches (which the scanner would have flagged for
/// user review) aren't silently auto-imported as owned.
const PHASE_B_OWNED_THRESHOLD: f64 = longbox_core::DEFAULT_MATCH_THRESHOLD;

/// Subfolder for unmatched arrivals. Leading underscore is deliberate —
/// sorts before alphabetical entries in directory listings.
const UNSORTED_DIR: &str = "_unsorted";

/// Process exactly one file. Errors are returned to the caller (the
/// consumer task) which logs and continues — a per-file failure does
/// not crash the pipeline.
pub async fn process_one(
    source: &Path,
    library_root: &Path,
    library_root_id: i64,
    db: &Pool,
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
    let mtime = system_time_to_offset(meta.modified()?);

    let comic_info_xml = read_comic_info_xml(source)?;
    let comic_info = comic_info_xml
        .as_deref()
        .and_then(|xml| ComicInfo::parse(xml.as_bytes()).ok());

    let patterns = load_patterns(db).await?;
    let basename = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let filename_parse = longbox_core::parse_filename(basename, &patterns);

    let title_hint = comic_info
        .as_ref()
        .and_then(|c| c.series.clone())
        .or_else(|| filename_parse.as_ref().map(|p| p.series_title.clone()));

    let Some(hint) = title_hint.filter(|s| !s.trim().is_empty()) else {
        // No usable hint at all — straight to _unsorted.
        let outcome = move_to_unsorted(source, library_root, library_root_id, size, mtime, db)
            .await?;
        log_outcome(&outcome, started_at, source);
        return Ok(outcome);
    };

    let year_hint = comic_info
        .as_ref()
        .and_then(|c| c.year)
        .or_else(|| filename_parse.as_ref().and_then(|p| p.year));

    let candidates = find_candidates(db, &hint, year_hint).await?;
    let match_result = match_file(comic_info.as_ref(), filename_parse.as_ref(), &candidates);

    // Phase B owned-classification uses the same threshold as Phase A's
    // post-hoc classify_status; needs-review-tier matches go to
    // _unsorted/ rather than silently claiming status='owned'.
    let status = classify_status(
        match_result.issue_id,
        match_result.confidence,
        match_result.method,
        PHASE_B_OWNED_THRESHOLD,
    );

    let outcome = match (match_result.issue_id, status) {
        (Some(issue_id), FileStatus::Owned) => {
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
        _ => move_to_unsorted(source, library_root, library_root_id, size, mtime, db).await?,
    };

    log_outcome(&outcome, started_at, source);
    Ok(outcome)
}

/// What happened to the file. Returned so callers (and tests) can
/// assert on the result without scraping logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Matched + moved + catalogued as owned.
    Imported {
        target: PathBuf,
        series_id: i64,
        issue_id: i64,
        file_id: i64,
    },
    /// No match (or sub-threshold match); moved to `_unsorted/`.
    Unsorted { target: PathBuf, file_id: i64 },
    /// Target path already existed; source left in watch folder for
    /// manual intervention (Step 7 will surface this via the dashboard).
    Conflict { target: PathBuf },
}

async fn wait_for_stability(source: &Path) {
    let Ok(meta) = std::fs::metadata(source) else {
        return;
    };
    let Ok(mtime) = meta.modified() else {
        return;
    };
    let age = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or_default();
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

fn read_comic_info_xml(source: &Path) -> Result<Option<String>> {
    let file = std::fs::File::open(source)?;
    let mut archive = ZipArchive::new(file)?;
    let mut found: Option<usize> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if entry.name().eq_ignore_ascii_case("ComicInfo.xml") {
            found = Some(i);
            break;
        }
    }
    let Some(idx) = found else {
        return Ok(None);
    };
    let mut entry = archive.by_index(idx)?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None), // non-UTF-8 ComicInfo is rare; treat as missing
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
    let issue = issue_repo::find_by_id(db, issue_id)
        .await?
        .ok_or(PostprocessError::CatalogNotFound {
            what: format!("issue {issue_id}"),
        })?;
    let series = series_repo::find_by_id(db, issue.series_id)
        .await?
        .ok_or(PostprocessError::CatalogNotFound {
            what: format!("series {}", issue.series_id),
        })?;

    let library_path = LibraryPath::new(
        &series.title,
        series.start_year.map(|y| y as i32),
        issue.number.as_str(),
    );
    let target_abs = library_path.full(library_root);

    if target_abs.exists() {
        return Ok(Outcome::Conflict { target: target_abs });
    }

    let metadata = compose_metadata(&series, &issue);
    let xml = metadata.to_xml();

    // Synchronous CBZ rewrite + filesystem move. Offload via
    // spawn_blocking so the async runtime stays unblocked even for
    // large archives.
    let source_owned = source.to_path_buf();
    let target_owned = target_abs.clone();
    tokio::task::spawn_blocking(move || rewrite_and_move(&source_owned, &target_owned, &xml))
        .await
        .map_err(|e| PostprocessError::Io(std::io::Error::other(format!("join: {e}"))))??;

    let path_relative = target_abs
        .strip_prefix(library_root)
        .map_err(|_| {
            PostprocessError::Io(std::io::Error::other(
                "computed target falls outside library_root",
            ))
        })?
        .to_string_lossy()
        .into_owned();

    let row = file_repo::upsert_imported(
        db,
        library_root_id,
        &path_relative,
        series.id,
        issue.id,
        size,
        mtime,
    )
    .await?;

    Ok(Outcome::Imported {
        target: target_abs,
        series_id: series.id,
        issue_id: issue.id,
        file_id: row.id,
    })
}

async fn move_to_unsorted(
    source: &Path,
    library_root: &Path,
    library_root_id: i64,
    size: i64,
    mtime: OffsetDateTime,
    db: &Pool,
) -> Result<Outcome> {
    let basename = source
        .file_name()
        .ok_or_else(|| PostprocessError::Io(std::io::Error::other("source has no filename")))?;
    let unsorted_dir = library_root.join(UNSORTED_DIR);
    let target_abs = unsorted_dir.join(basename);

    if target_abs.exists() {
        return Ok(Outcome::Conflict { target: target_abs });
    }

    // No ComicInfo rewrite — just move the file as-is. The scanner's
    // future passes might enrich it, but Phase B's job for unmatched
    // is "get it out of the watch folder without claiming a match."
    let source_owned = source.to_path_buf();
    let target_owned = target_abs.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(parent) = target_owned.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copy_then_unlink(&source_owned, &target_owned)
    })
    .await
    .map_err(|e| PostprocessError::Io(std::io::Error::other(format!("join: {e}"))))??;

    let path_relative = format!("{UNSORTED_DIR}/{}", basename.to_string_lossy());
    let row = upsert_unmatched(db, library_root_id, &path_relative, size, mtime).await?;

    Ok(Outcome::Unsorted {
        target: target_abs,
        file_id: row.id,
    })
}

/// Insert-or-update for the unmatched path. Mirrors the policy of
/// [`file_repo::upsert_imported`] but with issue_id=None,
/// status='unmatched', match_confidence=0.0. Idempotent on
/// `path_relative`.
async fn upsert_unmatched(
    db: &Pool,
    library_root_id: i64,
    path_relative: &str,
    size: i64,
    mtime: OffsetDateTime,
) -> Result<FileRow> {
    let mtime_p = PrimitiveDateTime::new(mtime.date(), mtime.time());
    let now_p = {
        let n = OffsetDateTime::now_utc();
        PrimitiveDateTime::new(n.date(), n.time())
    };

    let row = if let Some(existing) = file_repo::find_by_path(db, library_root_id, path_relative)
        .await?
    {
        // next_matched_at handles the issue_id=None case (clears
        // matched_at). The brief's "match_confidence=null" can't be
        // honored (schema is REAL NOT NULL); we store 0.0, matching
        // scanner's own unmatched-insert pattern.
        let matched_at = file_repo::next_matched_at(
            existing.issue_id,
            None,
            existing.matched_at,
            now_p,
        );
        let patch = FileUpdate {
            issue_id: None,
            size_bytes: size,
            mtime: mtime_p,
            last_scanned_at: now_p,
            match_method: MatchMethod::PhaseB.as_db_str().to_owned(),
            match_confidence: 0.0,
            status: FileStatus::Unmatched.as_db_str().to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now_p,
            matched_at,
        };
        file_repo::update(db, existing.id, patch).await?
    } else {
        let new = NewFile {
            issue_id: None,
            library_root_id,
            path_relative: path_relative.to_owned(),
            size_bytes: size,
            mtime: mtime_p,
            last_scanned_at: now_p,
            match_method: MatchMethod::PhaseB.as_db_str().to_owned(),
            match_confidence: 0.0,
            status: FileStatus::Unmatched.as_db_str().to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now_p,
            matched_at: None,
        };
        file_repo::insert(db, new).await?
    };
    Ok(row)
}

fn compose_metadata(series: &SeriesRow, issue: &IssueRow) -> ComicInfoMetadata {
    let cover_date = parse_cover_date(issue.cover_date.as_deref());
    let web = issue
        .cv_issue_id
        .map(|id| format!("https://comicvine.gamespot.com/issue/4000-{id}/"));
    ComicInfoMetadata {
        series: series.title.clone(),
        number: issue.number.clone(),
        start_year: series.start_year.map(|y| y as i32),
        publisher: series.publisher.clone(),
        title: issue.title.clone(),
        cover_date,
        web,
        summary: issue.summary.clone(),
    }
}

/// CV's cover_date is `YYYY-MM-DD` or `YYYY-MM` or just `YYYY`.
/// Only the full form populates Year/Month/Day; partials drop.
fn parse_cover_date(raw: Option<&str>) -> Option<CoverDate> {
    let s = raw?;
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    Some(CoverDate {
        year: parts[0].parse().ok()?,
        month: parts[1].parse().ok()?,
        day: parts[2].parse().ok()?,
    })
}

/// Rewrite source CBZ to `target` with a new ComicInfo.xml, then
/// delete source. Atomic at the rename step (temp in target_dir →
/// target via same-fs rename); on any failure prior to rename the
/// temp file is cleaned up by tempfile's Drop and source stays put.
fn rewrite_and_move(source: &Path, target: &Path, comic_info_xml: &str) -> Result<()> {
    let target_dir = target.parent().ok_or_else(|| {
        PostprocessError::Io(std::io::Error::other("target has no parent directory"))
    })?;
    std::fs::create_dir_all(target_dir)?;

    let src_file = std::fs::File::open(source)?;
    let mut archive = ZipArchive::new(src_file)?;

    // Build the new archive next to the target so the final rename
    // is same-filesystem and atomic.
    let temp = tempfile::Builder::new()
        .prefix(".longbox-phase-b-")
        .suffix(".cbz.tmp")
        .tempfile_in(target_dir)?;

    {
        let mut writer = ZipWriter::new(temp.as_file());
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            if entry.name().eq_ignore_ascii_case("ComicInfo.xml") {
                continue;
            }
            writer.raw_copy_file(entry)?;
        }
        writer.start_file(
            "ComicInfo.xml",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )?;
        writer.write_all(comic_info_xml.as_bytes())?;
        writer.finish()?;
    }

    // Persist the temp file to the final path. NamedTempFile::persist
    // returns the temp on error so it Drops and cleans up; on success
    // the temp is renamed atomically (same fs) and the handle is
    // consumed.
    temp.persist(target).map_err(|e| PostprocessError::Io(e.error))?;

    // Source is no longer needed.
    std::fs::remove_file(source)?;
    Ok(())
}

/// As-is move of a file into the library — used by the unmatched
/// path where no ComicInfo rewrite happens. Copies bytes into a temp
/// in the target dir, renames, deletes source. Same atomicity
/// guarantee as [`rewrite_and_move`].
fn copy_then_unlink(source: &Path, target: &Path) -> Result<()> {
    let target_dir = target.parent().ok_or_else(|| {
        PostprocessError::Io(std::io::Error::other("target has no parent directory"))
    })?;
    let temp = tempfile::Builder::new()
        .prefix(".longbox-phase-b-")
        .suffix(".cbz.tmp")
        .tempfile_in(target_dir)?;
    {
        let mut src = std::fs::File::open(source)?;
        let mut dst = temp.as_file();
        std::io::copy(&mut src, &mut dst)?;
        dst.flush()?;
    }
    temp.persist(target).map_err(|e| PostprocessError::Io(e.error))?;
    std::fs::remove_file(source)?;
    Ok(())
}

fn system_time_to_offset(t: SystemTime) -> OffsetDateTime {
    t.try_into()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
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
        Outcome::Unsorted { target, file_id } => {
            tracing::info!(
                target: "longbox_postprocess",
                source = %source.display(),
                target = %target.display(),
                file_id,
                duration_ms,
                "phase_b.unmatched (moved to _unsorted)"
            );
        }
        Outcome::Conflict { target } => {
            tracing::warn!(
                target: "longbox_postprocess",
                source = %source.display(),
                target = %target.display(),
                reason = "conflict",
                "phase_b.skipped"
            );
        }
    }
}


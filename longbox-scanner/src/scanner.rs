use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use longbox_core::{
    classify_status, extract_cv_issue_id_from_url, extract_metron_issue_id_from_url, match_file,
    matcher::Candidate, ComicInfo, FileStatus, MatchMethod, MatchResult, ParsedFilename,
    ParsingPattern,
};
use longbox_db::{
    discovered_folders_repo, file_repo, find_candidates, issue_repo, library_root_repo,
    parsing_pattern_repo, row_to_issue, row_to_series, scan_run_repo, series_repo,
    DiscoveredFolder, FileUpdate, NewFile, NewScanRun, Pool, ScanCompletion, ScanRunKind,
};
use time::{OffsetDateTime, PrimitiveDateTime};
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};

use crate::cbz::extract_comic_info;
use crate::error::ScanError;
use crate::report::{ScanFileError, ScanReport};
use crate::walker::{walk_library, DiscoveredFile};

#[derive(Debug, Clone)]
pub struct ScannerConfig {
    pub match_threshold: f64,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            match_threshold: longbox_core::DEFAULT_MATCH_THRESHOLD,
        }
    }
}

pub struct Scanner {
    db: Pool,
    config: ScannerConfig,
    scan_lock: Arc<Mutex<()>>,
}

impl Scanner {
    pub fn new(db: Pool, config: ScannerConfig) -> Self {
        Self {
            db,
            config,
            scan_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Full disk walk + full match cascade for every file + mark-missing pass.
    #[instrument(target = "longbox_scanner", skip(self), fields(library_root_id = library_root_id))]
    pub async fn scan_full(&self, library_root_id: i64) -> Result<ScanReport, ScanError> {
        let _guard = self
            .scan_lock
            .try_lock()
            .map_err(|_| ScanError::AlreadyRunning)?;

        let library_root = library_root_repo::find_by_id(&self.db, library_root_id)
            .await?
            .ok_or(ScanError::LibraryRootNotFound {
                id: library_root_id,
            })?;

        let scan_run_id = scan_run_repo::insert(
            &self.db,
            NewScanRun {
                library_root_id,
                kind: ScanRunKind::Full,
            },
        )
        .await?
        .id;

        let started_at = utc_now();
        let result = self
            .scan_full_body(&library_root, library_root_id, started_at)
            .await;
        record_outcome(&self.db, scan_run_id, &result).await;
        result
    }

    async fn scan_full_body(
        &self,
        library_root: &longbox_db::LibraryRootRow,
        library_root_id: i64,
        started_at: PrimitiveDateTime,
    ) -> Result<ScanReport, ScanError> {
        let mut report = ScanReport::new(library_root_id, started_at);
        let patterns = load_patterns(&self.db).await?;

        let root_path = PathBuf::from(&library_root.path);
        for entry in walk_library(&root_path) {
            let discovered = match entry {
                Ok(d) => d,
                Err(e) => {
                    report.errors.push(ScanFileError {
                        path_relative: String::new(),
                        error_message: e.to_string(),
                    });
                    continue;
                }
            };
            report.files_seen += 1;

            if let Err(e) = self
                .process_file(&discovered, library_root_id, &patterns, &mut report)
                .await
            {
                report.errors.push(ScanFileError {
                    path_relative: discovered.path_relative.clone(),
                    error_message: e.to_string(),
                });
            }
        }

        let marked =
            file_repo::mark_files_not_seen_since(&self.db, library_root_id, started_at).await?;
        report.files_marked_missing = u32::try_from(marked).unwrap_or(u32::MAX);

        // Library Tidy reconciliation, after the mark-missing pass so the
        // `files` rows reflect this scan's disk reality: refresh every
        // series' matched count (the phantom-transition signal), then
        // detect top-level folders that resolved to no tracked series.
        let series_refreshed = series_repo::refresh_last_matched_counts(&self.db).await?;
        let folders_discovered = detect_discovered_folders(&self.db, library_root_id).await?;
        info!(
            target: "longbox_scanner",
            series_refreshed,
            folders_discovered,
            "library_tidy.reconcile_complete"
        );

        let completed_at = utc_now();
        report.completed_at = completed_at;
        report.duration_ms = duration_ms(started_at, completed_at);
        Ok(report)
    }

    /// Re-run the match cascade against every `needs_review` file in the
    /// given library root. No filesystem walk. No mark-missing pass.
    #[instrument(target = "longbox_scanner", skip(self), fields(library_root_id = library_root_id))]
    pub async fn rescan_unmatched(&self, library_root_id: i64) -> Result<ScanReport, ScanError> {
        let _guard = self
            .scan_lock
            .try_lock()
            .map_err(|_| ScanError::AlreadyRunning)?;

        let library_root = library_root_repo::find_by_id(&self.db, library_root_id)
            .await?
            .ok_or(ScanError::LibraryRootNotFound {
                id: library_root_id,
            })?;

        let scan_run_id = scan_run_repo::insert(
            &self.db,
            NewScanRun {
                library_root_id,
                kind: ScanRunKind::RescanUnmatched,
            },
        )
        .await?
        .id;

        let started_at = utc_now();
        let result = self
            .rescan_unmatched_body(&library_root, library_root_id, started_at)
            .await;
        record_outcome(&self.db, scan_run_id, &result).await;
        result
    }

    async fn rescan_unmatched_body(
        &self,
        library_root: &longbox_db::LibraryRootRow,
        library_root_id: i64,
        started_at: PrimitiveDateTime,
    ) -> Result<ScanReport, ScanError> {
        let mut report = ScanReport::new(library_root_id, started_at);
        let patterns = load_patterns(&self.db).await?;

        let files = file_repo::list_by_status(
            &self.db,
            library_root_id,
            FileStatus::NeedsReview.as_db_str(),
        )
        .await?;
        let root_path = PathBuf::from(&library_root.path);

        for existing in files {
            report.files_seen += 1;
            let path_absolute = root_path.join(&existing.path_relative);
            let discovered = match rebuild_discovered(&existing, &path_absolute) {
                Some(d) => d,
                None => {
                    report.errors.push(ScanFileError {
                        path_relative: existing.path_relative.clone(),
                        error_message: "file disappeared between query and rescan".into(),
                    });
                    continue;
                }
            };
            if let Err(e) = self
                .process_file(&discovered, library_root_id, &patterns, &mut report)
                .await
            {
                report.errors.push(ScanFileError {
                    path_relative: existing.path_relative,
                    error_message: e.to_string(),
                });
            }
        }

        let completed_at = utc_now();
        report.completed_at = completed_at;
        report.duration_ms = duration_ms(started_at, completed_at);
        Ok(report)
    }

    /// Rematch needs_review files against a single specific series — used by
    /// the "add series" handler to retroactively claim files that couldn't
    /// match before this series existed.
    ///
    /// Note: the brief refers to "files in this series's library_root", but
    /// the schema has no `library_root_id` column on series. Phase A
    /// interpretation: rematch every needs_review file across every library
    /// root. With one root typical, the two are equivalent.
    #[instrument(target = "longbox_scanner", skip(self), fields(series_id = series_id))]
    pub async fn rematch_for_series(&self, series_id: i64) -> Result<ScanReport, ScanError> {
        let _guard = self
            .scan_lock
            .try_lock()
            .map_err(|_| ScanError::AlreadyRunning)?;

        let started_at = utc_now();

        // Build the single-candidate pool: the target series + its issues.
        let series_row = longbox_db::series_repo::find_by_id(&self.db, series_id)
            .await?
            .ok_or_else(|| ScanError::Db(longbox_db::DbError::NotFound))?;
        let target_series = row_to_series(series_row);
        let issue_rows = issue_repo::list_by_series(&self.db, series_id).await?;
        let target_issues: Vec<_> = issue_rows.into_iter().map(row_to_issue).collect();
        let candidates = vec![Candidate {
            series: target_series,
            issues: target_issues,
        }];

        let mut report = ScanReport::new(0, started_at);
        let patterns = load_patterns(&self.db).await?;

        for library_root in library_root_repo::list_all(&self.db).await? {
            report.library_root_id = library_root.id;
            let needs_review = file_repo::list_by_status(
                &self.db,
                library_root.id,
                FileStatus::NeedsReview.as_db_str(),
            )
            .await?;
            let root_path = PathBuf::from(&library_root.path);

            for existing in needs_review {
                report.files_seen += 1;
                let path_absolute = root_path.join(&existing.path_relative);
                let discovered = match rebuild_discovered(&existing, &path_absolute) {
                    Some(d) => d,
                    None => continue,
                };
                if let Err(e) = self
                    .rematch_one(&discovered, &existing, &candidates, &patterns, &mut report)
                    .await
                {
                    report.errors.push(ScanFileError {
                        path_relative: existing.path_relative.clone(),
                        error_message: e.to_string(),
                    });
                }
            }
        }

        let completed_at = utc_now();
        report.completed_at = completed_at;
        report.duration_ms = duration_ms(started_at, completed_at);
        Ok(report)
    }

    async fn process_file(
        &self,
        discovered: &DiscoveredFile,
        library_root_id: i64,
        patterns: &[ParsingPattern],
        report: &mut ScanReport,
    ) -> Result<(), ScanError> {
        let existing =
            file_repo::find_by_path(&self.db, library_root_id, &discovered.path_relative).await?;

        let (comic_info, comic_info_xml) = read_comic_info(discovered, report);
        let filename_parse = parse_basename(&discovered.path_relative, patterns);

        let match_result = self
            .run_match_cascade(&comic_info, filename_parse.as_ref())
            .await?;

        let new_status = compute_status(
            existing.as_ref(),
            &match_result,
            self.config.match_threshold,
        );

        self.persist(
            discovered,
            library_root_id,
            existing.as_ref(),
            &match_result,
            new_status,
            comic_info_xml.as_deref(),
        )
        .await?;

        increment_status_counter(report, new_status);
        if existing.is_some() {
            report.files_updated += 1;
        } else {
            report.files_added += 1;
        }
        Ok(())
    }

    async fn rematch_one(
        &self,
        discovered: &DiscoveredFile,
        existing: &longbox_db::FileRow,
        candidates: &[Candidate],
        patterns: &[ParsingPattern],
        report: &mut ScanReport,
    ) -> Result<(), ScanError> {
        let (comic_info, _comic_info_xml) = read_comic_info(discovered, report);
        let filename_parse = parse_basename(&discovered.path_relative, patterns);

        // Skip Tier 1 (the new series wouldn't help if Web URL pointed
        // elsewhere). Run only Tier 2/3 against the single-candidate pool.
        let match_result = match_file(comic_info.as_ref(), filename_parse.as_ref(), candidates);

        let new_status = compute_status(Some(existing), &match_result, self.config.match_threshold);

        // Only update if the status actually changed.
        if existing.status != new_status.as_db_str() {
            self.persist(
                discovered,
                existing.library_root_id,
                Some(existing),
                &match_result,
                new_status,
                None, // don't overwrite cached_comicinfo on rematch
            )
            .await?;
            report.files_updated += 1;
        }
        increment_status_counter(report, new_status);
        Ok(())
    }

    async fn run_match_cascade(
        &self,
        comic_info: &Option<ComicInfo>,
        filename_parse: Option<&ParsedFilename>,
    ) -> Result<MatchResult, ScanError> {
        // Tier 1: Web URL extraction with direct DB lookups.
        if let Some(ci) = comic_info {
            for url in &ci.web_urls {
                if let Some(cv_id) = extract_cv_issue_id_from_url(url) {
                    if let Some(issue) = issue_repo::find_by_cv_issue_id(&self.db, cv_id).await? {
                        return Ok(MatchResult {
                            issue_id: Some(issue.id),
                            method: MatchMethod::WebUrlCv,
                            confidence: 1.0,
                        });
                    }
                }
                if let Some(slug) = extract_metron_issue_id_from_url(url) {
                    if let Some(issue) =
                        issue_repo::find_by_metron_issue_id(&self.db, &slug).await?
                    {
                        return Ok(MatchResult {
                            issue_id: Some(issue.id),
                            method: MatchMethod::WebUrlMetron,
                            confidence: 1.0,
                        });
                    }
                }
            }
        }

        // Tier 2 + 3: similarity match against candidates.
        let title_hint = comic_info
            .as_ref()
            .and_then(|c| c.series.as_deref())
            .or_else(|| filename_parse.map(|f| f.series_title.as_str()));
        let year_hint = comic_info
            .as_ref()
            .and_then(|c| c.year)
            .or_else(|| filename_parse.and_then(|f| f.year));

        let candidates = match title_hint {
            Some(hint) => find_candidates(&self.db, hint, year_hint).await?,
            None => Vec::new(),
        };

        Ok(match_file(comic_info.as_ref(), filename_parse, &candidates))
    }

    async fn persist(
        &self,
        discovered: &DiscoveredFile,
        library_root_id: i64,
        existing: Option<&longbox_db::FileRow>,
        match_result: &MatchResult,
        new_status: FileStatus,
        comic_info_xml: Option<&str>,
    ) -> Result<(), ScanError> {
        let now = utc_now();
        let cached_xml = comic_info_xml.map(|s| s.to_owned());
        let cached_at = cached_xml.as_ref().map(|_| now);

        if let Some(row) = existing {
            // When the row's status is preserved as Ignored, the user has
            // explicitly disclaimed this file. Don't overwrite their decision
            // with whatever the matcher just produced — preserve issue_id /
            // match_method / match_confidence too. Only refresh disk-state
            // columns (mtime, size, last_scanned_at, last_seen_at, is_present).
            let preserve_ignored =
                new_status == FileStatus::Ignored && row.status == FileStatus::Ignored.as_db_str();
            let new_issue_id = if preserve_ignored {
                row.issue_id
            } else {
                match_result.issue_id
            };
            let patch = FileUpdate {
                issue_id: new_issue_id,
                size_bytes: i64::try_from(discovered.size_bytes).unwrap_or(i64::MAX),
                mtime: discovered.mtime,
                last_scanned_at: now,
                match_method: if preserve_ignored {
                    row.match_method.clone()
                } else {
                    match_result.method.as_db_str().to_owned()
                },
                match_confidence: if preserve_ignored {
                    row.match_confidence
                } else {
                    match_result.confidence
                },
                status: new_status.as_db_str().to_owned(),
                // Preserve any prior cached XML if the scanner didn't pass new bytes.
                cached_comicinfo_xml: cached_xml.or_else(|| row.cached_comicinfo_xml.clone()),
                cached_at: cached_at.or(row.cached_at),
                is_present: true,
                last_seen_at: now,
                matched_at: file_repo::next_matched_at(
                    row.issue_id,
                    new_issue_id,
                    row.matched_at,
                    now,
                ),
            };
            file_repo::update(&self.db, row.id, patch).await?;
        } else {
            let new = NewFile {
                issue_id: match_result.issue_id,
                library_root_id,
                path_relative: discovered.path_relative.clone(),
                size_bytes: i64::try_from(discovered.size_bytes).unwrap_or(i64::MAX),
                mtime: discovered.mtime,
                last_scanned_at: now,
                match_method: match_result.method.as_db_str().to_owned(),
                match_confidence: match_result.confidence,
                status: new_status.as_db_str().to_owned(),
                cached_comicinfo_xml: cached_xml,
                cached_at,
                is_present: true,
                last_seen_at: now,
                matched_at: match_result.issue_id.map(|_| now),
            };
            file_repo::insert(&self.db, new).await?;
        }
        Ok(())
    }
}

/// Scan-end reconciliation pass: group the library root's present
/// files by their top-level folder and upsert any folder whose CBZs
/// all failed to resolve to a tracked series into `discovered_folders`
/// for user review. Returns the count of folders upserted.
///
/// See `longbox-library-tidy-prompt.md` §2 — the predicate is
/// match-result-based, and publisher-grouped library layouts are a
/// documented non-goal (a publisher tier would collapse every series
/// under one top-level folder).
async fn detect_discovered_folders(db: &Pool, library_root_id: i64) -> Result<u32, ScanError> {
    let files = file_repo::list_by_library_root(db, library_root_id).await?;

    // top-level folder -> (present file count, any file resolved to a series)
    let mut folders: HashMap<String, (i64, bool)> = HashMap::new();
    for file in &files {
        if !file.is_present {
            continue;
        }
        let Some(folder) = top_level_folder(&file.path_relative) else {
            continue; // a file loose in the library root is not a series folder
        };
        let entry = folders.entry(folder).or_insert((0, false));
        entry.0 += 1;
        if file.issue_id.is_some() {
            entry.1 = true;
        }
    }

    let mut discovered: u32 = 0;
    for (folder_name, (file_count, any_resolved)) in folders {
        if any_resolved {
            continue; // a resolved file means this is a tracked series's folder
        }
        discovered_folders_repo::upsert(
            db,
            DiscoveredFolder {
                folder_name,
                file_count,
            },
        )
        .await?;
        discovered += 1;
    }
    Ok(discovered)
}

/// The first path component of a relative path, when the path has a
/// containing folder. `None` for a file sitting loose in the library
/// root (no folder component).
fn top_level_folder(path_relative: &str) -> Option<String> {
    let mut components = Path::new(path_relative).components();
    let first = components.next()?;
    components.next()?; // require at least one more component (the file itself)
    match first {
        std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    }
}

async fn load_patterns(db: &Pool) -> Result<Vec<ParsingPattern>, ScanError> {
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

fn read_comic_info(
    discovered: &DiscoveredFile,
    report: &mut ScanReport,
) -> (Option<ComicInfo>, Option<String>) {
    let xml = match extract_comic_info(&discovered.path_absolute) {
        Ok(xml) => xml,
        Err(e) => {
            warn!(
                target: "longbox_scanner",
                path = %discovered.path_relative,
                err = %e,
                "failed to extract ComicInfo.xml"
            );
            report.errors.push(ScanFileError {
                path_relative: discovered.path_relative.clone(),
                error_message: format!("ComicInfo extraction: {e}"),
            });
            return (None, None);
        }
    };
    let parsed = xml
        .as_ref()
        .and_then(|x| match ComicInfo::parse(x.as_bytes()) {
            Ok(ci) => Some(ci),
            Err(e) => {
                warn!(
                    target: "longbox_scanner",
                    path = %discovered.path_relative,
                    err = %e,
                    "ComicInfo.xml parse error; falling back to filename-only"
                );
                report.errors.push(ScanFileError {
                    path_relative: discovered.path_relative.clone(),
                    error_message: format!("ComicInfo parse: {e}"),
                });
                None
            }
        });
    (parsed, xml)
}

fn parse_basename(path_relative: &str, patterns: &[ParsingPattern]) -> Option<ParsedFilename> {
    let basename = Path::new(path_relative)
        .file_name()
        .and_then(|s| s.to_str())?;
    longbox_core::parse_filename(basename, patterns)
}

fn compute_status(
    existing: Option<&longbox_db::FileRow>,
    match_result: &MatchResult,
    threshold: f64,
) -> FileStatus {
    // Preserve a user's Ignored decision even if the underlying file would
    // re-classify differently.
    if let Some(row) = existing {
        if row.status == FileStatus::Ignored.as_db_str() {
            return FileStatus::Ignored;
        }
    }
    classify_status(
        match_result.issue_id,
        match_result.confidence,
        match_result.method,
        threshold,
    )
}

fn increment_status_counter(report: &mut ScanReport, status: FileStatus) {
    match status {
        FileStatus::Owned => report.matched_owned += 1,
        FileStatus::NeedsReview => report.matched_needs_review += 1,
        FileStatus::Ignored => report.matched_ignored += 1,
        FileStatus::Unmatched => report.unmatched += 1,
    }
}

fn rebuild_discovered(
    existing: &longbox_db::FileRow,
    path_absolute: &Path,
) -> Option<DiscoveredFile> {
    if !path_absolute.is_file() {
        debug!(
            target: "longbox_scanner",
            path = %path_absolute.display(),
            "file no longer on disk during rescan"
        );
        return None;
    }
    let metadata = std::fs::metadata(path_absolute).ok()?;
    let modified = metadata.modified().ok()?;
    let off = OffsetDateTime::from(modified).to_offset(time::UtcOffset::UTC);
    Some(DiscoveredFile {
        path_absolute: path_absolute.to_path_buf(),
        path_relative: existing.path_relative.clone(),
        size_bytes: metadata.len(),
        mtime: PrimitiveDateTime::new(off.date(), off.time()),
    })
}

fn utc_now() -> PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    PrimitiveDateTime::new(n.date(), n.time())
}

fn duration_ms(start: PrimitiveDateTime, end: PrimitiveDateTime) -> u64 {
    let duration = end.assume_utc() - start.assume_utc();
    u64::try_from(duration.whole_milliseconds().max(0)).unwrap_or(0)
}

/// Persists the final outcome of a scan into the `scan_runs` row that was
/// inserted at the start. Best-effort: a DB failure here is logged WARN
/// and swallowed so the caller's `Result` reflects the scan's outcome,
/// not the bookkeeping write's.
async fn record_outcome(db: &Pool, scan_run_id: i64, result: &Result<ScanReport, ScanError>) {
    match result {
        Ok(report) => {
            let stats = ScanCompletion {
                files_seen: i64::from(report.files_seen),
                files_added: i64::from(report.files_added),
                files_updated: i64::from(report.files_updated),
                files_matched: i64::from(report.matched_owned + report.matched_needs_review),
                files_needs_review: i64::from(report.matched_needs_review),
                files_unmatched: i64::from(report.unmatched),
            };
            if let Err(e) = scan_run_repo::complete_with_stats(db, scan_run_id, stats).await {
                warn!(target: "longbox_scanner", scan_run_id, err = %e,
                      "failed to persist scan_runs complete row");
            }
        }
        Err(e) => {
            if let Err(write_err) = scan_run_repo::fail(db, scan_run_id, &e.to_string()).await {
                warn!(target: "longbox_scanner", scan_run_id, err = %write_err,
                      "failed to persist scan_runs fail row");
            }
        }
    }
}

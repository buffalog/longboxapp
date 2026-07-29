use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use longbox_core::{
    classify_status, extract_cv_issue_id_from_url, extract_metron_issue_id_from_url, match_file,
    matcher::Candidate, ComicInfo, FileStatus, MatchMethod, MatchResult, ParsedFilename,
    ParsingPattern,
};
use longbox_db::{
    discovered_folders_repo, file_repo, find_candidates, issue_repo, library_root_repo,
    parsing_pattern_repo, row_to_issue, row_to_series, scan_run_repo, series_repo, settings_repo,
    DiscoveredFolder, FileUpdate, NewFile, NewScanRun, Pool, ScanCompletion, ScanRunKind,
    KEY_MATCH_CONFIDENCE_THRESHOLD,
};
use time::{OffsetDateTime, PrimitiveDateTime};
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};

use crate::error::ScanError;
use crate::report::{ScanFileError, ScanReport};
use crate::walker::{walk_library, DiscoveredFile};

#[derive(Debug, Clone)]
pub struct ScannerConfig {
    /// Fallback match-confidence threshold used only when the
    /// `match_confidence_threshold` settings row is missing or
    /// unparseable. Each scan re-reads the live value via
    /// [`Scanner::load_match_threshold`] so an in-DB change takes effect
    /// on the very next scan without a restart.
    pub match_threshold: f64,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            match_threshold: longbox_core::DEFAULT_MATCH_THRESHOLD,
        }
    }
}

/// Consecutive empty scans before a series is marked for automatic
/// removal (A.9 Step 6b). N=3 absorbs a transient mount blip — one bad
/// scan — without dragging the mark out indefinitely.
const AUTO_TIDY_THRESHOLD: i64 = 3;

/// Days a marked series waits in its recovery window before the
/// scan-end purge hard-deletes it. A wall-clock guarantee, independent
/// of scan cadence.
const AUTO_TIDY_RECOVERY_DAYS: i64 = 14;

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

    /// Live match-confidence threshold. Read once at the start of each
    /// scan run so a change to the `match_confidence_threshold` row
    /// (Settings UI, direct SQL) takes effect on the very next scan
    /// without a container restart — the previous implementation cached
    /// the boot-time env value on `ScannerConfig` and silently ignored
    /// the row. Falls back to the boot config's threshold when the row
    /// is absent or unparseable.
    async fn load_match_threshold(&self) -> Result<f64, ScanError> {
        let value = settings_repo::get_or_default(
            &self.db,
            KEY_MATCH_CONFIDENCE_THRESHOLD,
            self.config.match_threshold,
        )
        .await?;
        Ok(value)
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

        // Mount-health preflight. Returning Err here short-circuits the
        // scan BEFORE scan_run_repo::insert, so:
        //   - no scan_run row records a phantom scan,
        //   - file_repo::mark_files_not_seen_since never runs,
        //   - series_repo::tick_empty_scan_counters never runs,
        //   - auto-tidy never sees this scan.
        // Caller (scheduler or manual route) gets a clean Err; the
        // scheduler's next-day tick retries naturally.
        preflight_library_root(Path::new(&library_root.path))?;

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
        let match_threshold = self.load_match_threshold().await?;

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
                .process_file(
                    &discovered,
                    library_root_id,
                    &patterns,
                    match_threshold,
                    &mut report,
                )
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

        // Auto-tidy (A.9 Step 6b): tick every series' empty-scan
        // counter, mark series past the debounce threshold for removal,
        // then purge any whose recovery window has elapsed. The tick
        // runs first so a series that regained files this scan has its
        // mark cleared before the purge looks at it.
        series_repo::tick_empty_scan_counters(&self.db).await?;
        let auto_tidy_enabled =
            settings_repo::get_or_default(&self.db, settings_repo::KEY_AUTO_TIDY_ENABLED, true)
                .await?;
        let series_marked = if auto_tidy_enabled {
            let due_at = add_days(utc_now(), AUTO_TIDY_RECOVERY_DAYS);
            series_repo::mark_for_auto_tidy(&self.db, AUTO_TIDY_THRESHOLD, due_at).await?
        } else {
            0
        };
        let series_purged = series_repo::purge_due_auto_tidy(&self.db, utc_now()).await?;
        report.series_auto_marked = u32::try_from(series_marked).unwrap_or(u32::MAX);
        report.series_auto_purged = u32::try_from(series_purged).unwrap_or(u32::MAX);

        let (folders_discovered, folders_auto_dismissed) =
            detect_discovered_folders(&self.db, library_root_id).await?;
        report.folders_auto_dismissed = folders_auto_dismissed;
        info!(
            target: "longbox_scanner",
            series_refreshed,
            auto_tidy_enabled,
            series_marked,
            series_purged,
            folders_discovered,
            folders_auto_dismissed,
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

        // No mark-missing here, but a degraded mount would still
        // produce a 600-per-file IO-error report — confusing UX. Same
        // guard as scan_full for symmetry.
        preflight_library_root(Path::new(&library_root.path))?;

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
        let match_threshold = self.load_match_threshold().await?;

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
                .process_file(
                    &discovered,
                    library_root_id,
                    &patterns,
                    match_threshold,
                    &mut report,
                )
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
        let match_threshold = self.load_match_threshold().await?;

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
                    .rematch_one(
                        &discovered,
                        &existing,
                        &candidates,
                        &patterns,
                        match_threshold,
                        &mut report,
                    )
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

    /// Walk a single series folder and feed every discovered comic file
    /// through `process_file` — the full insert-or-update + match
    /// cascade Phase A normally runs across the whole library. Used by
    /// `PATCH /api/series/:id/cv-id` so the disambiguation flow picks
    /// up files that exist on disk but never made it into the `files`
    /// table (the user dropped a folder in without scanning).
    ///
    /// `path_relative` on every persisted row is library-root-relative
    /// (`DC K.O. Knightfight (2026)/issue 001.cbz`), matching the
    /// convention `scan_full` uses, so subsequent rematches and the
    /// dashboard queries all line up.
    ///
    /// `Ok(report)` with `files_seen = 0` when the series folder doesn't
    /// exist in any configured library root — not an error: it means
    /// the user hasn't dropped files in yet, which is a perfectly valid
    /// state.
    #[instrument(target = "longbox_scanner", skip(self), fields(series_id = series_id))]
    pub async fn scan_series_folder(&self, series_id: i64) -> Result<ScanReport, ScanError> {
        let _guard = self
            .scan_lock
            .try_lock()
            .map_err(|_| ScanError::AlreadyRunning)?;

        let started_at = utc_now();
        let series = longbox_db::series_repo::find_by_id(&self.db, series_id)
            .await?
            .ok_or_else(|| ScanError::Db(longbox_db::DbError::NotFound))?;
        let folder_name = match series.start_year {
            Some(year) => format!("{} ({})", series.title, year),
            None => series.title.clone(),
        };

        let patterns = load_patterns(&self.db).await?;
        let match_threshold = self.load_match_threshold().await?;
        let mut report = ScanReport::new(0, started_at);

        // Walk every configured library_root. Phase A is one-root
        // typical but the schema supports multi-root; the series might
        // live under any of them, and a stale entry in one root
        // shouldn't block discovery in another.
        for library_root in library_root_repo::list_all(&self.db).await? {
            let root_path = PathBuf::from(&library_root.path);
            let series_folder = root_path.join(&folder_name);
            if !series_folder.is_dir() {
                continue;
            }
            report.library_root_id = library_root.id;

            for entry in walk_library(&series_folder) {
                let inner = match entry {
                    Ok(d) => d,
                    Err(e) => {
                        report.errors.push(ScanFileError {
                            path_relative: String::new(),
                            error_message: e.to_string(),
                        });
                        continue;
                    }
                };
                // `walk_library` computed `path_relative` against the
                // SERIES folder. Rewrite it against the LIBRARY ROOT so
                // `process_file`'s `find_by_path` index lookup matches
                // the convention every other write site uses.
                let path_relative_to_root = match inner.path_absolute.strip_prefix(&root_path) {
                    Ok(rel) => rel
                        .to_string_lossy()
                        .replace('\\', "/")
                        .trim_start_matches('/')
                        .to_string(),
                    Err(_) => continue,
                };
                let discovered = DiscoveredFile {
                    path_absolute: inner.path_absolute,
                    path_relative: path_relative_to_root,
                    size_bytes: inner.size_bytes,
                    mtime: inner.mtime,
                };
                report.files_seen += 1;
                if let Err(e) = self
                    .process_file(
                        &discovered,
                        library_root.id,
                        &patterns,
                        match_threshold,
                        &mut report,
                    )
                    .await
                {
                    report.errors.push(ScanFileError {
                        path_relative: discovered.path_relative.clone(),
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
        match_threshold: f64,
        report: &mut ScanReport,
    ) -> Result<(), ScanError> {
        let existing =
            file_repo::find_by_path(&self.db, library_root_id, &discovered.path_relative).await?;

        let (comic_info, comic_info_xml) = read_comic_info(discovered, report);
        let filename_parse = parse_basename(&discovered.path_relative, patterns);

        // The series folder this file sits in declares the VOLUME year;
        // a scene filename's year is usually the RELEASE year. Where two
        // volumes share a title, only the former identifies which one.
        let folder_year = longbox_core::library_path::folder_volume_year(&discovered.path_relative);

        let match_result = self
            .run_match_cascade(&comic_info, filename_parse.as_ref(), folder_year)
            .await?;

        let new_status = compute_status(existing.as_ref(), &match_result, match_threshold);

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
        match_threshold: f64,
        report: &mut ScanReport,
    ) -> Result<(), ScanError> {
        let (comic_info, _comic_info_xml) = read_comic_info(discovered, report);
        let filename_parse = parse_basename(&discovered.path_relative, patterns);

        // Skip Tier 1 (the new series wouldn't help if Web URL pointed
        // elsewhere). Run only Tier 2/3 against the single-candidate pool.
        let folder_year = longbox_core::library_path::folder_volume_year(&discovered.path_relative);
        let match_result = match_file(
            comic_info.as_ref(),
            filename_parse.as_ref(),
            folder_year,
            candidates,
        );

        let new_status = compute_status(Some(existing), &match_result, match_threshold);

        // Update when the status changed OR the pointer did. Status alone
        // isn't enough: an ambiguous match is `needs_review`, and these rows
        // are already `needs_review` — so a rematch that finally resolves an
        // issue_id would compute it and throw it away, leaving the file
        // pointed at nothing with no way to retry.
        if existing.status != new_status.as_db_str() || existing.issue_id != match_result.issue_id {
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
        folder_year: Option<i32>,
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
                            ambiguous: false,
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
                            ambiguous: false,
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

        Ok(match_file(
            comic_info.as_ref(),
            filename_parse,
            folder_year,
            &candidates,
        ))
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
            // Same deal for a row a human has already pointed by hand (Accept
            // Match, or a Library Tidy mismatch correction): the *pointer* is
            // theirs, not ours. `compute_status` already keeps such a row
            // Owned, but that alone protected the status while leaving
            // `issue_id` to be silently rewritten by the matcher on the very
            // next scan — which is exactly how a hand-fixed file gets dragged
            // back to the wrong issue by the bad `<Number>` that put it there.
            // A manual decision outranks every automatic tier, permanently.
            let preserve_manual = row.match_method == MatchMethod::Manual.as_db_str();
            let preserve = preserve_ignored || preserve_manual;
            let new_issue_id = if preserve {
                row.issue_id
            } else {
                match_result.issue_id
            };
            let patch = FileUpdate {
                issue_id: new_issue_id,
                size_bytes: i64::try_from(discovered.size_bytes).unwrap_or(i64::MAX),
                mtime: discovered.mtime,
                last_scanned_at: now,
                match_method: if preserve {
                    row.match_method.clone()
                } else {
                    match_result.method.as_db_str().to_owned()
                },
                match_confidence: if preserve {
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
/// files by their top-level folder, upsert any folder whose comics all
/// failed to resolve to a tracked series into `discovered_folders`,
/// and auto-dismiss any open row whose folder is no longer untracked
/// (A.9 hot-fix item F6 — stale rows post-CV-add were the proximate
/// cause of bulk-convert duplicate-series creation). Returns
/// `(folders_discovered, folders_auto_dismissed)`.
///
/// See `longbox-library-tidy-prompt.md` §2 — the predicate is
/// match-result-based, and publisher-grouped library layouts are a
/// documented non-goal (a publisher tier would collapse every series
/// under one top-level folder).
async fn detect_discovered_folders(
    db: &Pool,
    library_root_id: i64,
) -> Result<(u32, u32), ScanError> {
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
    let mut currently_untracked: HashSet<String> = HashSet::new();
    for (folder_name, (file_count, any_resolved)) in folders {
        if any_resolved {
            continue; // a resolved file means this is a tracked series's folder
        }
        discovered_folders_repo::upsert(
            db,
            DiscoveredFolder {
                folder_name: folder_name.clone(),
                file_count,
            },
        )
        .await?;
        currently_untracked.insert(folder_name);
        discovered += 1;
    }

    // F6: auto-dismiss any still-`auto_dismissed_at`-NULL row whose
    // folder is no longer in the untracked set — its files have
    // since resolved or its folder is gone. Writes
    // `auto_dismissed_at`; user-permanent `dismissed_at` stays
    // untouched. The upsert above clears `auto_dismissed_at` on
    // re-detection, so an auto-dismissed folder resurfaces the next
    // time its files re-qualify as untracked (the F6 trap fix).
    let auto_dismissed =
        discovered_folders_repo::auto_dismiss_not_in(db, &currently_untracked).await?;
    let auto_dismissed = u32::try_from(auto_dismissed).unwrap_or(u32::MAX);

    Ok((discovered, auto_dismissed))
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
    let xml = match longbox_archive::read_comic_info(&discovered.path_absolute) {
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
    // Use the cascading variant so scene/NZB-style names with `_` or `.`
    // separators parse correctly — same shape Phase B already uses. The
    // cascade prefers the original input on any pattern hit, so
    // canonical filenames keep their id=2/id=7/id=10 captures
    // unchanged; the normalized retry only fires when nothing claims
    // the raw input.
    longbox_core::parse_filename_with_normalization(basename, patterns)
}

fn compute_status(
    existing: Option<&longbox_db::FileRow>,
    match_result: &MatchResult,
    threshold: f64,
) -> FileStatus {
    if let Some(row) = existing {
        // Preserve a user's Ignored decision even if the underlying file
        // would re-classify differently.
        if row.status == FileStatus::Ignored.as_db_str() {
            return FileStatus::Ignored;
        }
        // Preserve a user's Accept Match decision. Without this guard, a
        // rescan whose Tier-2 confidence falls below `threshold` would
        // downgrade a manually-accepted row from Owned back to NeedsReview,
        // silently undoing the click.
        if row.match_method == MatchMethod::Manual.as_db_str() {
            return FileStatus::Owned;
        }
    }
    classify_status(
        match_result.issue_id,
        match_result.confidence,
        match_result.method,
        threshold,
        match_result.ambiguous,
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

/// `ts` advanced by whole days. Saturates at the representable range
/// rather than panicking — a recovery deadline far in the future is
/// harmless.
fn add_days(ts: PrimitiveDateTime, days: i64) -> PrimitiveDateTime {
    ts.saturating_add(time::Duration::days(days))
}

fn duration_ms(start: PrimitiveDateTime, end: PrimitiveDateTime) -> u64 {
    let duration = end.assume_utc() - start.assume_utc();
    u64::try_from(duration.whole_milliseconds().max(0)).unwrap_or(0)
}

/// Mount-health guard for library-root scans. Two failure modes
/// matter inside the container:
///
/// 1. **Mount inaccessible** — virtiofs/SMB share dropped, the bind
///    mount evaporates, `read_dir` errors with ENOENT/EIO.
/// 2. **Mount stale and empty** — the bind mount still resolves but
///    the SMB layer is in a degraded state where it reports zero
///    entries. `read_dir` succeeds, the iterator yields nothing.
///
/// Both modes look identical to walkdir: zero files surfaced. Without
/// this guard, the scan proceeds, `mark_files_not_seen_since` flips
/// every catalogued file to `is_present=false`, and auto-tidy starts
/// its 14-day countdown to hard-delete every series. A multi-day SMB
/// outage would silently nuke the catalog.
///
/// The check is cheap: one `read_dir` syscall and one iterator step.
/// On a healthy 600-series library the cost is ~1ms; on a broken
/// mount the syscall fails fast.
///
/// A genuinely empty fresh-install library trips this too, which is
/// the right behavior — there is nothing for the scanner to do and
/// no catalog entries to corrupt. The user will see the WARN log and
/// retry once they have content.
fn preflight_library_root(root: &Path) -> Result<(), ScanError> {
    let mut entries = std::fs::read_dir(root).map_err(|e| ScanError::PreflightFailed {
        reason: format!("library root unreadable at {}: {e}", root.display()),
    })?;
    if entries.next().is_none() {
        return Err(ScanError::PreflightFailed {
            reason: format!(
                "library root {} is empty — refusing to scan (likely mount failure; \
                 scanning would mark every catalogued file missing and start auto-tidy)",
                root.display()
            ),
        });
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use longbox_db::FileRow;
    use time::{Date, Month, Time};

    fn row(status: FileStatus, method: MatchMethod) -> FileRow {
        let ts = PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::January, 1).unwrap(),
            Time::MIDNIGHT,
        );
        FileRow {
            id: 1,
            issue_id: Some(42),
            library_root_id: 1,
            path_relative: "Foo/Foo 001.cbz".into(),
            size_bytes: 1024,
            mtime: ts,
            last_scanned_at: ts,
            match_method: method.as_db_str().to_string(),
            match_confidence: 1.0,
            status: status.as_db_str().to_string(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: ts,
            matched_at: Some(ts),
        }
    }

    fn rescan_result(confidence: f64) -> MatchResult {
        MatchResult {
            issue_id: Some(42),
            method: MatchMethod::FilenameRegex,
            confidence,
            ambiguous: false,
        }
    }

    #[test]
    fn compute_status_preserves_ignored_against_high_confidence_rematch() {
        let existing = row(FileStatus::Ignored, MatchMethod::Ignored);
        let status = compute_status(Some(&existing), &rescan_result(0.95), 0.85);
        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn compute_status_preserves_manual_against_low_confidence_rematch() {
        // The regression: a user clicked Accept Match (method=Manual,
        // status=Owned), then a rescan's Tier-2 score came in below the
        // threshold. Without the guard, this would downgrade to NeedsReview.
        let existing = row(FileStatus::Owned, MatchMethod::Manual);
        let status = compute_status(Some(&existing), &rescan_result(0.40), 0.85);
        assert_eq!(status, FileStatus::Owned);
    }

    #[test]
    fn compute_status_classifies_normally_when_no_user_decision() {
        let existing = row(FileStatus::Owned, MatchMethod::FilenameRegex);
        let status = compute_status(Some(&existing), &rescan_result(0.40), 0.85);
        assert_eq!(status, FileStatus::NeedsReview);
    }
}

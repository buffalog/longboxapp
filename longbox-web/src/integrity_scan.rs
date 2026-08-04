//! Library Integrity — finding computation.
//!
//! Every finding is derived live from the catalog and the filesystem. There is
//! no findings table: a persisted finding is stale the moment a file moves,
//! and computing on read also makes the write surface trivially auditable —
//! this module holds no `INSERT`, `UPDATE` or `DELETE` at all.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

/// What a disk walk actually did.
///
/// Reported alongside the findings because class (b) reads 0/0 on a healthy
/// library, and its entire value is as a regression guard. A section showing
/// zero because the walk failed, or never ran, is the same lie as telling a
/// user to run an analysis that cannot reach their files. With this, zero is
/// a measurement.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalkProvenance {
    /// Absolute library root that was walked.
    pub root: String,
    /// Comic files seen on disk, by the scanner's own rules.
    pub files_seen: usize,
    /// Catalog rows compared against them (`is_present = 1`).
    pub rows_compared: usize,
    pub duration_ms: u64,
    /// Paths the walk could not read. Non-empty means the counts below are a
    /// floor, not a total — say so rather than reporting a clean zero.
    pub unreadable: Vec<String>,
}

/// Disk and catalog disagreeing about what exists.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reconciliation {
    pub provenance: WalkProvenance,
    /// On disk, with no catalog row at all.
    pub orphans: Vec<String>,
    /// Catalog says `is_present = 1`, nothing on disk.
    pub ghosts: Vec<String>,
    /// Catalog says `is_present = 0`, but the file is right there. Not one of
    /// the two classes asked for, and reported separately rather than folded
    /// in: a scan fixes these on its own, so they are drift, not corruption.
    pub present_but_marked_absent: Vec<String>,
}

impl Reconciliation {
    /// True when the walk produced a usable comparison. A caller must not
    /// render "0 problems" when this is false.
    pub fn is_conclusive(&self) -> bool {
        self.provenance.unreadable.is_empty() && self.provenance.files_seen > 0
    }
}

/// Walk the library root and compare it against the catalog.
///
/// Uses `longbox_scanner::walk_library`, deliberately, rather than walking
/// here: reconciliation is only meaningful against the same definition of "a
/// comic file" that the scanner catalogues by. A second walker would drift and
/// every file it disagreed about would become a permanent false finding.
pub async fn reconcile(
    db: &longbox_db::Pool,
    library_root_id: i64,
    root: &str,
) -> Result<Reconciliation, longbox_db::DbError> {
    let started = std::time::Instant::now();

    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    let mut unreadable: Vec<String> = Vec::new();
    for entry in longbox_scanner::walk_library(std::path::Path::new(root)) {
        match entry {
            Ok(f) => {
                on_disk.insert(f.path_relative);
            }
            Err(e) => unreadable.push(e.to_string()),
        }
    }

    // Every catalog row for this root, with its presence flag — one query,
    // because both directions of the comparison need the same set.
    let rows: Vec<(String, bool)> =
        sqlx::query_as("SELECT path_relative, is_present FROM files WHERE library_root_id = ?")
            .bind(library_root_id)
            .fetch_all(db)
            .await?;

    let mut known: BTreeMap<&str, bool> = BTreeMap::new();
    for (path, present) in &rows {
        // A path can only appear once per root (UNIQUE constraint), so no
        // merge logic is needed here.
        known.insert(path.as_str(), *present);
    }
    let rows_compared = known.values().filter(|p| **p).count();

    let orphans: Vec<String> = on_disk
        .iter()
        .filter(|p| !known.contains_key(p.as_str()))
        .cloned()
        .collect();
    let ghosts: Vec<String> = known
        .iter()
        .filter(|(p, present)| **present && !on_disk.contains(**p))
        .map(|(p, _)| (*p).to_owned())
        .collect();
    let present_but_marked_absent: Vec<String> = known
        .iter()
        .filter(|(p, present)| !**present && on_disk.contains(**p))
        .map(|(p, _)| (*p).to_owned())
        .collect();

    Ok(Reconciliation {
        provenance: WalkProvenance {
            root: root.to_owned(),
            files_seen: on_disk.len(),
            rows_compared,
            duration_ms: started.elapsed().as_millis() as u64,
            unreadable,
        },
        orphans,
        ghosts,
        present_but_marked_absent,
    })
}

// ============ per-file evidence, shared by several classes ============

/// One file, with everything the finding classes surface about it.
///
/// Raw evidence only. No suggested keeper, no ranking, no interpretation of
/// which source to believe — those are judgement calls that belong with the
/// actions they drive, designed against this output rather than against a
/// reconstruction of it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileEvidence {
    pub file_id: i64,
    /// Which library root `path_relative` is relative to. Needed to stat
    /// the file — a path alone cannot be resolved.
    pub library_root_id: i64,
    pub path_relative: String,
    pub size_bytes: i64,
    pub content_blake3: Option<String>,
    pub issue_id: Option<i64>,
    pub issue_number: Option<String>,
    pub series_id: Option<i64>,
    pub series_title: Option<String>,
    pub series_start_year: Option<i64>,
    pub match_method: String,
    pub match_confidence: f64,
    /// Identity the archive claims from its own internal paths.
    pub archive_label: Option<String>,
    pub archive_label_kind: Option<String>,
    pub archive_issue: Option<String>,
    pub archive_series: Option<String>,
    /// From the file's embedded ComicInfo.xml, where it has one.
    pub comicinfo_number: Option<String>,
    pub comicinfo_series: Option<String>,
    /// The issue number this file's own filename claims, parsed with the
    /// production parser and the live `parsing_patterns`.
    pub filename_issue: Option<String>,
}

/// The row shape every class reads. One query, reused, because the classes
/// overlap and a file appearing in two of them must be the same file.
type EvidenceRow = (
    i64,
    String,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    String,
    f64,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

const EVIDENCE_SQL: &str = "SELECT f.id, f.path_relative, f.size_bytes, f.content_blake3, \
     f.issue_id, i.number, i.series_id, s.title, s.start_year, f.match_method, \
     f.match_confidence, f.archive_label, f.archive_label_kind, f.cached_comicinfo_xml, \
     f.library_root_id \
     FROM files f \
     LEFT JOIN issues i ON f.issue_id = i.id \
     LEFT JOIN series s ON i.series_id = s.id \
     WHERE f.is_present = 1 ORDER BY f.id";

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let v = xml[start..end].trim();
    (!v.is_empty()).then(|| v.to_owned())
}

/// Load every present file with its evidence.
///
/// The filename is parsed with `longbox_core`'s production parser and the
/// live `parsing_patterns` rows — never a local reimplementation. A second
/// parser would report every difference between itself and production as a
/// finding, which measures the parsers rather than the library.
async fn load_evidence(db: &longbox_db::Pool) -> Result<Vec<FileEvidence>, longbox_db::DbError> {
    let patterns: Vec<longbox_core::ParsingPattern> =
        longbox_db::parsing_pattern_repo::list_enabled(db)
            .await?
            .into_iter()
            .map(|r| longbox_core::ParsingPattern {
                id: r.id,
                name: r.name,
                pattern: r.pattern,
                priority: r.priority as i32,
                enabled: r.enabled,
            })
            .collect();

    let rows: Vec<EvidenceRow> = sqlx::query_as(EVIDENCE_SQL).fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let basename = r.1.rsplit('/').next().unwrap_or(&r.1);
            let archive = r.11.as_deref().map(|label| {
                let kind = match r.12.as_deref() {
                    Some("dir") => longbox_core::archive_label::LabelKind::Dir,
                    _ => longbox_core::archive_label::LabelKind::Page,
                };
                longbox_core::archive_label::parse_label(label, kind)
            });
            FileEvidence {
                file_id: r.0,
                library_root_id: r.14,
                size_bytes: r.2,
                content_blake3: r.3,
                issue_id: r.4,
                issue_number: r.5,
                series_id: r.6,
                series_title: r.7,
                series_start_year: r.8,
                match_method: r.9,
                match_confidence: r.10,
                archive_label: r.11.clone(),
                archive_label_kind: r.12,
                archive_issue: archive.as_ref().and_then(|a| a.issue.clone()),
                archive_series: archive.as_ref().and_then(|a| a.series.clone()),
                comicinfo_number: r.13.as_deref().and_then(|x| xml_tag(x, "Number")),
                comicinfo_series: r.13.as_deref().and_then(|x| xml_tag(x, "Series")),
                filename_issue: longbox_core::parse_filename_with_normalization(
                    basename, &patterns,
                )
                .map(|p| p.number),
                path_relative: r.1,
            }
        })
        .collect())
}

// ============ the finding classes ============

/// Files whose bytes are identical, wherever they sit and whatever they are
/// bound to. The class Library Tidy structurally cannot see: Tidy groups by
/// `issue_id`, and most of these span different issues.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContentDuplicateGroup {
    pub digest: String,
    pub size_bytes: i64,
    /// Bytes reclaimable if all but one copy went away.
    pub redundant_bytes: i64,
    /// Distinct issues these copies are bound to. More than one means an
    /// issue is marked owned while holding another issue's content.
    pub distinct_issue_ids: Vec<i64>,
    /// True when the copies span more than one SERIES — its own finding,
    /// because the resolution differs and the user must see which series the
    /// content really belongs to before choosing.
    pub spans_multiple_series: bool,
    pub files: Vec<FileEvidence>,
}

/// A file bound to a series whose other files live somewhere else.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CrossFolderFinding {
    pub file: FileEvidence,
    /// The folder most of this series lives in.
    pub main_folder: String,
    pub actual_folder: String,
    pub category: CrossFolderCategory,
}

/// Why a file sits outside its series' main folder. Three distinct problems
/// that a single count would flatten into "82 things to look at".
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CrossFolderCategory {
    /// The stray folder names a DIFFERENT catalog volume of the same title.
    /// This is the shape that bound 26 Authority files to the wrong volume.
    /// Expected to be empty now — kept because an empty category that can
    /// populate is a regression indicator, and deleting it would let the next
    /// wrong-volume binding land in `benign_variant` and be dismissed.
    WrongVolume,
    /// Same series, differently-spelled folder. Cosmetic.
    BenignVariant,
    /// Neither: a collected edition or a differently-titled folder holding
    /// this series' issues. Needs human judgement.
    TradeOrCollection,
}

/// A file whose own filename names an issue other than the one it is bound to.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FilenameDisagreement {
    pub file: FileEvidence,
    pub filename_says: String,
    pub bound_to: String,
}

/// `issue_id IS NULL AND status = 'owned'` — owned, pointing at nothing, and
/// unreachable by rematch because that selects `needs_review` only.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OrphanedOwnedRow {
    pub file_id: i64,
    pub path_relative: String,
    pub match_method: String,
    pub is_present: bool,
}

/// Everything, computed live.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Findings {
    pub reconciliation: Reconciliation,
    pub content_duplicates: Vec<ContentDuplicateGroup>,
    /// Files with no trustworthy digest yet, so (a) is a floor rather than a
    /// total until analysis has run.
    pub unanalyzed_candidates: usize,
    pub cross_folder: Vec<CrossFolderFinding>,
    pub filename_disagreements: Vec<FilenameDisagreement>,
    pub orphaned_owned_rows: Vec<OrphanedOwnedRow>,
    /// file_id -> how many classes it appears in. Two independent detectors
    /// agreeing is a stronger signal than one, and those are worth triaging
    /// first. Display only, over data already computed.
    pub classes_per_file: BTreeMap<i64, usize>,
}

pub async fn findings(
    db: &longbox_db::Pool,
    library_root_id: i64,
    root: &str,
) -> Result<Findings, longbox_db::DbError> {
    let reconciliation = reconcile(db, library_root_id, root).await?;
    let evidence = load_evidence(db).await?;

    // ---- (a) + (e) content duplicates ----
    let mut by_digest: BTreeMap<&str, Vec<&FileEvidence>> = BTreeMap::new();
    for f in &evidence {
        if let Some(d) = f.content_blake3.as_deref() {
            by_digest.entry(d).or_default().push(f);
        }
    }
    // A stored digest is only evidence about bytes that are still there.
    //
    // Grouping on `content_blake3` alone reports a file replaced since it
    // was analysed as a copy of whatever else carries its old digest —
    // describing content that no longer exists anywhere. Seen live: two
    // Ferocious issues with different sizes, different ComicInfo numbers
    // and different pages, both still carrying one November digest
    // stamped against a size neither file has had since.
    //
    // Analyze cannot clear it: `refresh_digests` only visits
    // size-collision candidates, and a file whose size changed no longer
    // collides with its former twin. So the stale row is permanent and
    // unactionable — the delete endpoint refuses it (correctly), leaving
    // a finding that can only ever be dismissed by not believing it.
    //
    // Freshness is decided by `DiskObservation::stat` — the same stat the
    // delete path performs — and never by comparing `files.size_bytes` to
    // `files.hashed_size_bytes`. Both are catalog columns; agreeing with
    // each other says nothing about the bytes on disk, and reaching for
    // them here would be the sixth instance of the substitution
    // `DiskObservation` exists to prevent.
    let roots: std::collections::HashMap<i64, String> = longbox_db::library_root_repo::list_all(db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect();

    let mut content_duplicates: Vec<ContentDuplicateGroup> = Vec::new();
    for (digest, files) in by_digest.into_iter().filter(|(_, files)| files.len() > 1) {
        let stamps: std::collections::HashMap<i64, longbox_db::ContentStamp> =
            longbox_db::file_repo::content_stamps_by_content_hash(db, digest)
                .await?
                .into_iter()
                .map(|s| (s.file_id, s))
                .collect();
        let mut verified: Vec<&FileEvidence> = Vec::new();
        for f in files {
            let stamp = stamps.get(&f.file_id);
            let (_, obs) = crate::disk_observation::DiskObservation::stat(
                &roots,
                f.library_root_id,
                &f.path_relative,
                stamp.and_then(|s| s.content_blake3.as_deref()),
                stamp.and_then(|s| s.hashed_size_bytes),
                stamp.and_then(|s| s.hashed_mtime),
            )
            .await;
            if obs.digest() == Some(digest) {
                verified.push(f);
            }
        }
        // One surviving member is not a duplicate of anything.
        if verified.len() < 2 {
            continue;
        }
        let issues: BTreeSet<i64> = verified.iter().filter_map(|f| f.issue_id).collect();
        let series: BTreeSet<i64> = verified.iter().filter_map(|f| f.series_id).collect();
        content_duplicates.push(ContentDuplicateGroup {
            digest: digest.to_owned(),
            size_bytes: verified[0].size_bytes,
            redundant_bytes: verified[0].size_bytes * (verified.len() as i64 - 1),
            distinct_issue_ids: issues.into_iter().collect(),
            spans_multiple_series: series.len() > 1,
            files: verified.into_iter().cloned().collect(),
        });
    }
    // Biggest reclaimable first: triage where the space is.
    content_duplicates.sort_by_key(|g| std::cmp::Reverse(g.redundant_bytes));

    // Size-collision candidates with no digest: (a) cannot be a total yet.
    let mut size_counts: BTreeMap<i64, usize> = BTreeMap::new();
    for f in &evidence {
        *size_counts.entry(f.size_bytes).or_insert(0) += 1;
    }
    let unanalyzed_candidates = evidence
        .iter()
        .filter(|f| f.content_blake3.is_none() && size_counts.get(&f.size_bytes) > Some(&1))
        .count();

    // ---- (c) cross-folder ----
    let mut folders_by_series: BTreeMap<i64, BTreeMap<String, usize>> = BTreeMap::new();
    for f in &evidence {
        if let Some(sid) = f.series_id {
            let top = f.path_relative.split('/').next().unwrap_or("").to_owned();
            *folders_by_series
                .entry(sid)
                .or_default()
                .entry(top)
                .or_insert(0) += 1;
        }
    }
    // Every catalog series, for deciding whether a stray folder names a real
    // different volume. Same normalization the matcher ranks with.
    let all_series = longbox_db::series_repo::find_all(db).await?;
    let mut by_norm_title: BTreeMap<String, Vec<(i64, Option<i64>)>> = BTreeMap::new();
    for row in &all_series {
        by_norm_title
            .entry(longbox_core::normalize_title(&row.title))
            .or_default()
            .push((row.id, row.start_year));
    }

    let mut cross_folder = Vec::new();
    for f in &evidence {
        let Some(sid) = f.series_id else { continue };
        let Some(counts) = folders_by_series.get(&sid) else {
            continue;
        };
        if counts.len() < 2 {
            continue;
        }
        let main = counts
            .iter()
            .max_by_key(|(name, n)| (**n, std::cmp::Reverse((*name).clone())))
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        let actual = f.path_relative.split('/').next().unwrap_or("").to_owned();
        if actual == main {
            continue;
        }
        let (stray_title, stray_year) = longbox_core::library_path::folder_identity(&actual);
        let (main_title, _) = longbox_core::library_path::folder_identity(&main);
        // `series_matches`, not raw equality: two folder spellings of one
        // title drift by punctuation, and `normalize_title` deliberately
        // preserves hyphens (load-bearing in X-Men). The same relaxation was
        // already needed to compare archive labels against catalog titles —
        // reusing it keeps one definition of "same series" rather than
        // growing a second that disagrees.
        let same_title = longbox_core::archive_label::series_matches(&stray_title, &main_title);
        let category = if !same_title
            || longbox_core::normalize::has_author_attribution(
                &actual
                    .rsplit_once(" (")
                    .map_or(actual.clone(), |(t, _)| t.to_owned()),
            ) {
            // Either the folder names something else entirely, or it names
            // this series after its creators — `Fire Power By Kirkman And
            // Samnee (2023)` normalizes to `fire power` ONLY because
            // normalization strips the attribution, which is exactly the
            // collected-edition signal.
            CrossFolderCategory::TradeOrCollection
        } else {
            // Same title. Does the stray folder's year name a DIFFERENT
            // catalog volume? That is the wrong-volume shape.
            let names_other_volume = stray_year.is_some_and(|y| {
                by_norm_title.get(&stray_title).is_some_and(|vols| {
                    vols.iter()
                        .any(|(id, sy)| *id != sid && *sy == Some(y as i64))
                })
            });
            if names_other_volume {
                CrossFolderCategory::WrongVolume
            } else {
                CrossFolderCategory::BenignVariant
            }
        };
        cross_folder.push(CrossFolderFinding {
            file: f.clone(),
            main_folder: main,
            actual_folder: actual,
            category,
        });
    }

    // ---- (d) filename vs assigned issue ----
    let norm = |s: &str| {
        let t = s.trim_start_matches('0');
        if t.is_empty() {
            "0".to_owned()
        } else {
            t.to_lowercase()
        }
    };
    let filename_disagreements: Vec<FilenameDisagreement> = evidence
        .iter()
        .filter_map(|f| {
            let says = f.filename_issue.as_deref()?;
            let bound = f.issue_number.as_deref()?;
            (norm(says) != norm(bound)).then(|| FilenameDisagreement {
                file: f.clone(),
                filename_says: says.to_owned(),
                bound_to: bound.to_owned(),
            })
        })
        .collect();

    // ---- (f) orphaned owned rows ----
    let orphan_rows: Vec<(i64, String, String, bool)> = sqlx::query_as(
        "SELECT id, path_relative, match_method, is_present FROM files \
         WHERE issue_id IS NULL AND status = 'owned' AND match_method <> '' ORDER BY id",
    )
    .fetch_all(db)
    .await?;
    let orphaned_owned_rows: Vec<OrphanedOwnedRow> = orphan_rows
        .into_iter()
        .map(
            |(file_id, path_relative, match_method, is_present)| OrphanedOwnedRow {
                file_id,
                path_relative,
                match_method,
                is_present,
            },
        )
        .collect();

    // ---- cross-reference ----
    let mut classes_per_file: BTreeMap<i64, usize> = BTreeMap::new();
    for g in &content_duplicates {
        for f in &g.files {
            *classes_per_file.entry(f.file_id).or_insert(0) += 1;
        }
    }
    for c in &cross_folder {
        *classes_per_file.entry(c.file.file_id).or_insert(0) += 1;
    }
    for d in &filename_disagreements {
        *classes_per_file.entry(d.file.file_id).or_insert(0) += 1;
    }
    for o in &orphaned_owned_rows {
        *classes_per_file.entry(o.file_id).or_insert(0) += 1;
    }
    classes_per_file.retain(|_, n| *n > 1);

    Ok(Findings {
        reconciliation,
        content_duplicates,
        unanalyzed_candidates,
        cross_folder,
        filename_disagreements,
        orphaned_owned_rows,
        classes_per_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_findings_are_only_conclusive_when_the_walk_actually_walked() {
        let clean = Reconciliation {
            provenance: WalkProvenance {
                root: "/comics".into(),
                files_seen: 7115,
                rows_compared: 7115,
                duration_ms: 2000,
                unreadable: vec![],
            },
            orphans: vec![],
            ghosts: vec![],
            present_but_marked_absent: vec![],
        };
        assert!(clean.is_conclusive(), "a real walk finding nothing is news");

        // Walk saw nothing: 0/0 here means "did not look", not "all clear".
        let empty = Reconciliation {
            provenance: WalkProvenance {
                files_seen: 0,
                ..clean.provenance.clone()
            },
            ..clean.clone()
        };
        assert!(
            !empty.is_conclusive(),
            "a walk that saw no files cannot report a clean library"
        );

        // Partially readable: the counts are a floor, not a total.
        let partial = Reconciliation {
            provenance: WalkProvenance {
                unreadable: vec!["permission denied: /comics/x".into()],
                ..clean.provenance.clone()
            },
            ..clean
        };
        assert!(!partial.is_conclusive());
    }
}

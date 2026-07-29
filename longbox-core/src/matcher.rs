//! Tier 2 + Tier 3 of the match cascade.
//!
//! Tier 1 (`<Web>` URL extraction) is the caller's responsibility — it needs
//! direct DB access to look up a CV / Metron issue ID, which `longbox-core`
//! cannot have. See [`crate::comicinfo::extract_cv_issue_id_from_url`] and
//! [`crate::comicinfo::extract_metron_issue_id_from_url`] for the per-URL
//! atoms the scanner uses to orchestrate Tier 1.
//!
//! This module handles:
//!
//! - **Tier 2** — ComicInfo `<Series>` + `<Number>`. Series text is compared
//!   against candidate `sort_title`s in normalized space via
//!   [`crate::similarity::similarity`]. Confidence = similarity score in
//!   `[0.65, 1.0)`. Below 0.65, fall through to Tier 3.
//! - **Tier 3** — filename-parse `series_title` + `number`. Same machinery,
//!   but confidence is capped at [`crate::FILENAME_CONFIDENCE_CEILING`]
//!   (0.90) so embedded metadata always wins a tie.
//!
//! No match in either tier → `MatchMethod::Unmatched`, confidence 0.0. The
//! matcher returns raw confidence only; classification (`owned` vs
//! `needs_review` vs `unmatched`) is the caller's responsibility via
//! [`crate::classify_status`].

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::comicinfo::ComicInfo;
use crate::file::MatchMethod;
use crate::filename::ParsedFilename;
use crate::issue::{Issue, IssueNumber};
use crate::normalize::normalize_title;
use crate::series::Series;
use crate::similarity::similarity;
use crate::{FILENAME_CONFIDENCE_CEILING, NEEDS_REVIEW_FLOOR};

/// A series and the issues belonging to it, fetched from the DB by the
/// scanner (or another caller) and handed to [`match_file`].
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub series: Series,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    pub issue_id: Option<i64>,
    pub method: MatchMethod,
    pub confidence: f64,
    /// Tier 2 and Tier 3 both resolved an issue — and disagreed about which.
    /// The file's own two sources of truth contradict each other, so no
    /// confidence score is meaningful; a human has to look. Callers must
    /// classify this as `needs_review` regardless of confidence (see
    /// [`crate::classify_status`]).
    pub ambiguous: bool,
}

/// Tier 2 → Tier 3 cascade against a pre-fetched candidate pool. Tier 1
/// (`<Web>` URL → issue lookup) is the caller's responsibility — call this
/// only after Tier 1 has failed to produce a match.
///
/// Returns the strongest non-failing result. `Unmatched` (confidence 0.0)
/// when neither tier produces a score ≥ [`NEEDS_REVIEW_FLOOR`].
///
/// **Tier 2 does not get to win unchallenged — about the issue number.** Real
/// libraries contain files whose embedded ComicInfo `<Number>` is simply wrong:
/// "Ferocious (2025) 002.cbz" is physically issue 2, correctly named and
/// foldered, and its `<Number>` says 1. Tier 2 running first meant that lie was
/// believed and the file was silently filed under issue 1, colliding with the
/// real #1. So Tier 3 is always computed too, and when both tiers land in the
/// **same series** but name different issues, we neither trust Tier 2 nor
/// guess: we take the filename's issue (in the failure mode we have actually
/// observed, the filename is the one telling the truth) and mark the result
/// `ambiguous`, so it lands in `needs_review` for a human instead of being
/// quietly filed.
///
/// **The cross-check is deliberately confined to one series.** When the tiers
/// pick *different series*, Tier 2 still wins outright and nothing is flagged.
/// That isn't a contradiction — it's Tier 3 having less to go on. Two volumes
/// of one title (Ultimate Spider-Man 2000 and 2024) score identically on title
/// similarity, so the year hint decides, and a filename's year is very often
/// absent while ComicInfo's `<Volume>` is right there. Letting a yearless
/// filename overrule ComicInfo about *which volume* would drag correctly-tagged
/// files onto the wrong series — manufacturing the very collisions this exists
/// to prevent. The evidence is about wrong numbers inside the right series;
/// this fix goes exactly that far and no further.
///
/// Tier 3 producing no parse, or agreeing with Tier 2, changes nothing —
/// Tier 2's result stands, exactly as before.
pub fn match_file(
    comic_info: Option<&ComicInfo>,
    filename_parse: Option<&ParsedFilename>,
    folder: FolderEvidence,
    candidates: &[Candidate],
) -> MatchResult {
    let tier3 = tier3_filename(filename_parse, folder, candidates);
    match (tier2_comicinfo(comic_info, folder, candidates), tier3) {
        (Some((tier2, series2)), Some((tier3, series3))) => {
            if series2 == series3 && tier2.issue_id != tier3.issue_id {
                MatchResult {
                    ambiguous: true,
                    ..tier3
                }
            } else {
                tier2
            }
        }
        (Some((tier2, _)), None) => tier2,
        (None, Some((tier3, _))) => tier3,
        (None, None) => MatchResult {
            issue_id: None,
            method: MatchMethod::Unmatched,
            confidence: 0.0,
            ambiguous: false,
        },
    }
}

/// Each tier returns its result **and the series it landed in**, so
/// [`match_file`] can tell "these two disagree about the issue" (a real
/// contradiction) from "these two are looking at different volumes entirely"
/// (Tier 3 lacking a year hint, not evidence of anything).
type TierMatch = (MatchResult, i64);

fn tier2_comicinfo(
    comic_info: Option<&ComicInfo>,
    folder: FolderEvidence,
    candidates: &[Candidate],
) -> Option<TierMatch> {
    let ci = comic_info?;
    let series_text = ci.series.as_deref()?;
    let number_text = ci.number.as_deref()?;
    match_in_candidates(
        series_text,
        number_text,
        YearEvidence {
            hint: ci.year,
            // ComicInfo's `year` is parsed from `<Volume>`, so it is genuine
            // volume metadata — the one year hint that may select a volume.
            volume: ci.year,
            folder,
        },
        candidates,
        // No ceiling for ComicInfo matches — they go up to 1.0.
        f64::INFINITY,
        MatchMethod::ComicInfoXml,
    )
}

fn tier3_filename(
    filename_parse: Option<&ParsedFilename>,
    folder: FolderEvidence,
    candidates: &[Candidate],
) -> Option<TierMatch> {
    let fp = filename_parse?;
    match_in_candidates(
        &fp.series_title,
        &fp.number,
        YearEvidence {
            hint: fp.year,
            // A filename's year is the RELEASE year far more often than the
            // volume year. It may order the sort; it may never select.
            volume: None,
            folder,
        },
        candidates,
        FILENAME_CONFIDENCE_CEILING,
        MatchMethod::FilenameRegex,
    )
}

fn match_in_candidates(
    title_text: &str,
    number_text: &str,
    years: YearEvidence,
    candidates: &[Candidate],
    confidence_ceiling: f64,
    method: MatchMethod,
) -> Option<TierMatch> {
    let pick = best_candidate_match(title_text, years, candidates)?;
    let confidence = pick.score.min(confidence_ceiling);
    if confidence < NEEDS_REVIEW_FLOOR {
        return None;
    }
    let needle = IssueNumber::new(number_text);
    let issue = pick
        .candidate
        .issues
        .iter()
        .find(|i| i.number.matches(&needle))?;
    Some((
        MatchResult {
            issue_id: Some(issue.id),
            method,
            confidence,
            // An unearned volume pick is flagged here, not confidence-clamped.
            // Lowering confidence would only move the silent failure below a
            // threshold; the flag makes a human look.
            ambiguous: pick.ambiguous,
        },
        pick.candidate.series.id,
    ))
}

/// What a file's location says about which volume it belongs to.
///
/// The distinction between "the folder said nothing" and "there is no folder"
/// is load-bearing, and it is the same one that governs digest freshness: a
/// folder that is silent is evidence, a folder that does not exist is not.
///
/// A library file always sits in a series folder that LongBox itself named
/// `{title} ({start_year})`, so that folder's silence is informative — it
/// means the volume genuinely wasn't recorded. A watch-folder import has no
/// series folder at all; there is nothing there to be silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderEvidence {
    /// Not in a library series folder — a watch-folder import. Says nothing,
    /// and its silence is not evidence of anything.
    NoFolder,
    /// In a series folder that declares no year.
    Silent,
    /// In a series folder declaring this volume year.
    Year(i32),
}

impl FolderEvidence {
    fn year(self) -> Option<i32> {
        match self {
            FolderEvidence::Year(y) => Some(y),
            _ => None,
        }
    }
}

/// The year signals a tier can offer, and what each is allowed to do.
///
/// Splitting these is the whole point: `hint` may ORDER the sort but may be a
/// release year; only `volume` may SELECT a volume, and only genuine volume
/// metadata goes there.
#[derive(Debug, Clone, Copy)]
struct YearEvidence {
    hint: Option<i32>,
    volume: Option<i32>,
    folder: FolderEvidence,
}

/// The winning candidate, plus whether the win was actually earned.
struct Pick<'a> {
    score: f64,
    candidate: &'a Candidate,
    /// True when several same-titled volumes remained tied after every
    /// signal was spent, so the winner was decided by `series.id` order.
    ambiguous: bool,
}

/// Score every candidate against `title_text` (after normalization), return
/// the highest-scoring one.
///
/// Tie-breaks, in order: exact `folder_year` match, exact `year_hint` match,
/// larger issue count, lower `series.id`.
///
/// **`folder_year` outranks `year_hint` deliberately.** Two volumes of one
/// title score identically on title similarity, so the year decides — and the
/// two years available are not equally trustworthy. A scene filename carries
/// the RELEASE year while the folder carries the VOLUME year. Live evidence:
/// 26 files in `The Authority (2008)/` are named `... 004 (2009) ...` because
/// 2009 is when the digital edition shipped. Feeding 2009 matched no candidate
/// (the volumes start 1999/2003/2008), so the year rung silently no-opped, the
/// issue-count rung tied 29-to-29, and the decision fell to lower-id — which
/// picked the 1999 volume, wrongly, 26 times out of 26. Every one of those
/// files sits in a folder that names 2008 exactly.
///
/// **When same-titled volumes survive every rung, we abstain.** `series.id`
/// order carries no information about which volume a file belongs to; it
/// records which one the user happened to add first. Letting it decide a
/// volume question at 0.9 confidence is how the Authority files were bound
/// silently and wrongly. The result is flagged `ambiguous`, which
/// [`crate::classify_status`] turns into `needs_review` — visible, instead of
/// a confident guess.
///
/// The abstention replaces ONLY the id rung. Issue count still decides, and
/// deliberately so: it is weak evidence but it is evidence, added for a live
/// bug where a 1-issue stub kept stealing files from a 24-issue catalog entry.
/// Abstaining there would strand those files in `/watch/` again.
fn best_candidate_match<'a>(
    title_text: &str,
    years: YearEvidence,
    candidates: &'a [Candidate],
) -> Option<Pick<'a>> {
    if candidates.is_empty() {
        return None;
    }
    let normalized = normalize_title(title_text);

    // Apply `normalize_title` to BOTH sides at comparison time. The
    // stored `sort_title` is supposed to be normalized at insert time,
    // but real-world data shows ~50% of catalog rows have un-normalized
    // sort_titles (mixed case, etc.) from historical paths that
    // skipped normalization. Symmetric normalization here is durable
    // against that drift — without it `Y The Last Man` filenames score
    // ~0.71 against the stored `Y The Last Man` sort_title (case
    // mismatch destroys jaccard), falling below the 0.65 needs-review
    // floor and dead-ending as unmatched.
    // Normalize the stored TITLE rather than the stored `sort_title`. Every
    // insert path sets `sort_title = normalize_title(title)`, so for a
    // hygienic row this is identical — but ~50% of catalog rows historically
    // carried un-normalized sort_titles, and a row whose sort_title
    // normalizes differently from its sibling's ("Authority, The" vs
    // "authority") would both score lower AND drop out of the rival set,
    // silently disabling the volume guard for exactly the rows most likely to
    // need it.
    let mut scored: Vec<(f64, &Candidate)> = candidates
        .iter()
        .map(|c| {
            (
                similarity(&normalized, &normalize_title(&c.series.title)),
                c,
            )
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            // Folder year first: it names the volume, where a filename year
            // often names the release.
            .then_with(|| {
                let fy = years.folder.year();
                let a_f = fy.is_some() && a.1.series.start_year == fy;
                let b_f = fy.is_some() && b.1.series.start_year == fy;
                b_f.cmp(&a_f)
            })
            .then_with(|| {
                let yh = years.hint;
                let a_year = yh.is_some() && a.1.series.start_year == yh;
                let b_year = yh.is_some() && b.1.series.start_year == yh;
                b_year.cmp(&a_year)
            })
            // Prefer the candidate with more issues. When two series
            // share a title and neither matches the year hint (common:
            // NZB filenames carry the scan year, not the series
            // start_year), the larger catalog entry is almost always
            // the correct one — the smaller is typically a 1-issue
            // stub from an earlier import that the user hasn't pruned.
            // Live repro: "Sam and Twitch Case Files" had id=381 (2025,
            // 1 issue) vs id=918 (2024, 24 issues); files with a 2026
            // scan-year hint kept losing to the stub, sat in /watch/
            // indefinitely.
            .then_with(|| b.1.issues.len().cmp(&a.1.issues.len()))
            // Lower id as the final determinism guarantee — only
            // reached when title similarity, year-hint match, AND
            // issue count are all equal.
            .then_with(|| a.1.series.id.cmp(&b.1.series.id))
    });

    let (score, candidate) = scored.first().copied()?;

    // Did anything actually pick this winner, or did id order?
    //
    // Only same-titled candidates can be confused for each other — everything
    // else already lost on similarity. Note that a shared normalized title
    // means an identical similarity score by construction, so this set is
    // exactly the top tie group whenever a collision exists.
    let key = normalize_title(&candidate.series.title);
    let rivals: Vec<&Candidate> = scored
        .iter()
        .map(|(_, c)| *c)
        .filter(|c| normalize_title(&c.series.title) == key)
        .collect();

    let ambiguous = rivals.len() > 1 && !decided_by_evidence(&rivals, years);

    Some(Pick {
        score,
        candidate,
        ambiguous,
    })
}

/// Whether VOLUME evidence uniquely selects one of several same-titled
/// volumes.
///
/// The axis is volume evidence vs release year, not folder vs everything
/// else. Two sources genuinely name a volume:
///
///   * the series folder's year — LongBox names those folders itself as
///     `{title} ({start_year})`, so the year in one IS the volume year;
///   * ComicInfo `<Volume>` — volume metadata by definition (the field is
///     called `year` in [`ComicInfo`] only for readability).
///
/// A filename-parsed year is NOT volume evidence. It usually carries the
/// release year: the 26 misbound Authority files are named `... 004 (2009)`
/// because that is when the digital edition of the 2008 volume shipped. The
/// same goes for a SABnzbd job folder, which mirrors a scene release name and
/// is parsed with the very same filename patterns.
///
/// **Issue count may break a tie between candidates volume evidence SELECTED.
/// It may not substitute for volume evidence that is absent or contradictory.**
/// Two rows sharing a title AND a start year have no remaining discriminator
/// but catalog completeness, which is the real-vs-stub shape that rung exists
/// for. With no volume evidence at all, the largest catalog simply wins a
/// volume question it has no bearing on — the same objection as `series.id`
/// order, one rung up.
///
/// The one exemption is [`FolderEvidence::NoFolder`]. A watch-folder import
/// has no series folder to be silent, so its absence carries no information
/// and the prior behaviour stands. That path is also where abstaining is most
/// expensive: `ambiguous` routes to `needs_review`, which
/// `longbox-postprocess` refuses to import, so the file stays in `/watch/`.
fn decided_by_evidence(rivals: &[&Candidate], years: YearEvidence) -> bool {
    let hits = |year: Option<i32>| -> Option<Vec<&Candidate>> {
        let year = year?;
        let v: Vec<&Candidate> = rivals
            .iter()
            .copied()
            .filter(|c| c.series.start_year == Some(year))
            .collect();
        (!v.is_empty()).then_some(v)
    };
    // A clear leader on catalog size, among an already-selected set.
    let leader = |set: &[&Candidate]| {
        let most = set.iter().map(|c| c.issues.len()).max().unwrap_or(0);
        most > 0 && set.iter().filter(|c| c.issues.len() == most).count() == 1
    };

    match years.folder {
        // No series folder exists — nothing to be silent. Prior behaviour.
        FolderEvidence::NoFolder => match hits(years.hint) {
            Some(set) if set.len() == 1 => true,
            Some(set) => leader(&set),
            None => leader(rivals),
        },
        // The folder named a volume year.
        FolderEvidence::Year(_) => match hits(years.folder.year()) {
            Some(set) if set.len() == 1 => true,
            Some(set) => leader(&set),
            // It named a year no candidate has: the folder and the catalog
            // disagree. That is not licence for a release year, or for the
            // biggest catalog, to settle a volume question.
            None => false,
        },
        // The folder said nothing. Only ComicInfo `<Volume>` can still speak.
        FolderEvidence::Silent => match hits(years.volume) {
            Some(set) if set.len() == 1 => true,
            Some(set) => leader(&set),
            None => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_MATCH_THRESHOLD;

    fn series(id: i64, title: &str, year: Option<i32>) -> Series {
        Series::new(id, None, title, year)
    }

    fn issue(id: i64, series_id: i64, number: &str) -> Issue {
        Issue {
            id,
            series_id,
            cv_id: None,
            metron_id: None,
            number: IssueNumber::new(number),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        }
    }

    fn candidate(s: Series, issues: Vec<Issue>) -> Candidate {
        Candidate { series: s, issues }
    }

    // -------- Tier 2 (ComicInfo) --------

    #[test]
    fn tier2_exact_series_match_owned_confidence() {
        let s = series(1, "The Walking Dead", Some(2003));
        let issues = vec![issue(10, 1, "1")];
        let candidates = vec![candidate(s, issues)];
        let ci = ComicInfo {
            series: Some("The Walking Dead".into()),
            number: Some("1".into()),
            year: Some(2003),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert_eq!(r.issue_id, Some(10));
        assert!(r.confidence >= DEFAULT_MATCH_THRESHOLD);
    }

    #[test]
    fn tier2_typo_in_series_lands_in_needs_review_zone() {
        let s = series(1, "The Walking Dead", Some(2003));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Wlking Dead".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (NEEDS_REVIEW_FLOOR..1.0).contains(&r.confidence),
            "expected needs_review zone, got {}",
            r.confidence
        );
    }

    #[test]
    fn tier2_below_floor_falls_through_to_tier3() {
        let s = series(1, "The Walking Dead", Some(2003));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("ZZZZZZZZZ".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let parsed = ParsedFilename {
            series_title: "The Walking Dead".into(),
            number: "1".into(),
            volume: None,
            year: Some(2003),
            title: None,
            pattern_id: 1,
        };
        let r = match_file(
            Some(&ci),
            Some(&parsed),
            FolderEvidence::NoFolder,
            &candidates,
        );
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
    }

    #[test]
    fn tier2_falls_through_when_series_matches_but_number_missing() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("99".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn tier2_year_tiebreaks_two_identical_series() {
        let candidates = vec![
            candidate(series(1, "Spider-Man", Some(1990)), vec![issue(10, 1, "1")]),
            candidate(series(2, "Spider-Man", Some(2014)), vec![issue(20, 2, "1")]),
        ];
        let ci = ComicInfo {
            series: Some("Spider-Man".into()),
            number: Some("1".into()),
            year: Some(2014),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(
            r.issue_id,
            Some(20),
            "2014 series should win on year tie-break"
        );
    }

    #[test]
    fn tier2_id_ascending_tiebreaks_when_no_year() {
        let candidates = vec![
            candidate(series(1, "Spider-Man", Some(1990)), vec![issue(10, 1, "1")]),
            candidate(series(2, "Spider-Man", Some(2014)), vec![issue(20, 2, "1")]),
        ];
        let ci = ComicInfo {
            series: Some("Spider-Man".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.issue_id, Some(10), "lower-id wins when no year hint");
    }

    // -------- volume disambiguation by folder year --------

    /// The three real `sort_title = "authority"` rows, with their real issue
    /// counts. 1076 and 1078 tie at 29, which is what made the old chain fall
    /// all the way through to `series.id`.
    fn authority_volumes() -> Vec<Candidate> {
        [(1076i64, 1999i32, 29), (1077, 2003, 15), (1078, 2008, 29)]
            .into_iter()
            .map(|(id, year, n)| {
                let issues = (1..=n)
                    .map(|k| issue(id * 1000 + k, id, &k.to_string()))
                    .collect();
                candidate(series(id, "The Authority", Some(year)), issues)
            })
            .collect()
    }

    /// `The Authority (2008)/The Authority 004 (2009) (Digital)...cbr` — the
    /// filename's 2009 is the digital RELEASE year and matches no volume.
    fn authority_filename() -> ParsedFilename {
        ParsedFilename {
            series_title: "The Authority".into(),
            number: "004".into(),
            volume: None,
            year: Some(2009),
            title: None,
            pattern_id: 10,
        }
    }

    /// The live bug, exactly: 26 files bound to the 1999 volume because the
    /// filename year matched nothing and `series.id` broke the tie.
    #[test]
    fn folder_year_picks_the_right_volume_when_the_filename_year_is_a_release_year() {
        let candidates = authority_volumes();
        let fp = authority_filename();

        // Without folder evidence the chain still exhausts itself — and now
        // says so instead of silently picking the oldest volume.
        let blind = match_file(None, Some(&fp), FolderEvidence::NoFolder, &candidates);
        assert!(
            blind.ambiguous,
            "three tied volumes and nothing to separate them must abstain"
        );

        // With the folder's 2008 the answer is unambiguous.
        let seeing = match_file(None, Some(&fp), FolderEvidence::Year(2008), &candidates);
        assert_eq!(
            seeing.issue_id,
            Some(1078 * 1000 + 4),
            "must bind to the 2008 volume's issue 4"
        );
        assert!(!seeing.ambiguous, "folder year earned the pick");
    }

    /// Folder year must outrank the filename's year, not merely supplement
    /// it: here they name two different real volumes.
    #[test]
    fn folder_year_outranks_a_conflicting_filename_year() {
        let mut candidates = authority_volumes();
        // Give 1999 a start_year the filename names, so the two hints
        // genuinely disagree about which volume is right.
        candidates[0].series.start_year = Some(2009);
        let r = match_file(
            None,
            Some(&authority_filename()),
            FolderEvidence::Year(2008),
            &candidates,
        );
        assert_eq!(
            r.issue_id,
            Some(1078 * 1000 + 4),
            "the folder names the volume; the filename names the release"
        );
        assert!(!r.ambiguous);
    }

    #[test]
    fn abstains_when_a_title_collision_has_no_folder_year_at_all() {
        let mut fp = authority_filename();
        fp.year = None;
        let r = match_file(
            None,
            Some(&fp),
            FolderEvidence::NoFolder,
            &authority_volumes(),
        );
        assert!(
            r.ambiguous,
            "no year anywhere, two 29-issue volumes → abstain"
        );
    }

    #[test]
    fn abstains_when_the_folder_year_matches_no_volume() {
        let r = match_file(
            None,
            Some(&authority_filename()),
            FolderEvidence::Year(2011),
            &authority_volumes(),
        );
        assert!(
            r.ambiguous,
            "a folder year naming no volume is not evidence for any of them"
        );
    }

    /// The gate is strictly for collisions. A library with no same-titled
    /// volumes must behave exactly as before, folder year or not.
    #[test]
    fn a_single_matching_series_is_never_ambiguous() {
        let candidates = vec![candidate(
            series(1, "Saga", Some(2012)),
            vec![issue(10, 1, "1")],
        )];
        let fp = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2099),
            title: None,
            pattern_id: 1,
        };
        for folder in [
            FolderEvidence::NoFolder,
            FolderEvidence::Silent,
            FolderEvidence::Year(1999),
            FolderEvidence::Year(2012),
        ] {
            let r = match_file(None, Some(&fp), folder, &candidates);
            assert_eq!(r.issue_id, Some(10), "folder={folder:?}");
            assert!(!r.ambiguous, "no collision → no abstention");
        }
    }

    /// Abstention replaces ONLY the `series.id` rung. Issue count is weak
    /// evidence but it is evidence, added for a live bug where a 1-issue stub
    /// stole files from a 24-issue catalog entry; abstaining there would
    /// strand those files in /watch/ again.
    #[test]
    fn a_decisive_issue_count_still_resolves_a_collision() {
        let stub = candidate(
            series(381, "Sam and Twitch Case Files", Some(2025)),
            vec![issue(10, 381, "1")],
        );
        let real = candidate(
            series(918, "Sam and Twitch Case Files", Some(2024)),
            (1..=24)
                .map(|n| issue(100 + n, 918, &n.to_string()))
                .collect(),
        );
        let ci = ComicInfo {
            series: Some("Sam and Twitch Case Files".into()),
            number: Some("23".into()),
            year: Some(2026), // matches neither
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &[stub, real]);
        assert_eq!(r.issue_id, Some(123), "the 24-issue entry still wins");
        assert!(
            !r.ambiguous,
            "a clear catalog-size leader is evidence, so this must not abstain"
        );
    }

    /// A filename year that happens to equal a sibling volume's start year is
    /// NOT proof of which volume a file belongs to — the filename usually
    /// carries the release year, which is the entire premise of this guard.
    /// Accepting it as decisive bound the wrong volume at `owned`, silently.
    #[test]
    fn a_folder_year_naming_no_volume_does_not_let_the_filename_year_decide() {
        // Folder says 2000; no volume starts in 2000 (catalog has 2001/2004).
        // The filename's 2004 matches one — but the folder just contradicted
        // the catalog, which is not licence for the release year to settle it.
        let candidates = vec![
            candidate(series(1, "Powers", Some(2001)), vec![issue(10, 1, "30")]),
            candidate(series(2, "Powers", Some(2004)), vec![issue(20, 2, "30")]),
        ];
        let fp = ParsedFilename {
            series_title: "Powers".into(),
            number: "30".into(),
            volume: None,
            year: Some(2004),
            title: None,
            pattern_id: 1,
        };
        let r = match_file(None, Some(&fp), FolderEvidence::Year(2000), &candidates);
        assert!(
            r.ambiguous,
            "folder named a year the catalog does not have — must not resolve on the release year"
        );

        // A watch-folder import has no series folder to be silent, so the
        // exemption applies and prior behaviour stands.
        let r = match_file(None, Some(&fp), FolderEvidence::NoFolder, &candidates);
        assert!(!r.ambiguous, "no series folder → nothing to abstain over");
        assert_eq!(r.issue_id, Some(20));
    }

    /// The evidence test must narrow as the comparator does. Asking each rung
    /// about the whole field reports a correctly-earned pick as unearned.
    #[test]
    fn issue_count_is_judged_among_the_year_matchers_not_the_whole_field() {
        let candidates = vec![
            candidate(
                series(1, "The Authority", Some(2008)),
                (1..=29)
                    .map(|n| issue(1000 + n, 1, &n.to_string()))
                    .collect(),
            ),
            candidate(
                series(2, "The Authority", Some(2008)),
                (1..=15)
                    .map(|n| issue(2000 + n, 2, &n.to_string()))
                    .collect(),
            ),
            candidate(
                series(3, "The Authority", Some(1999)),
                (1..=29)
                    .map(|n| issue(3000 + n, 3, &n.to_string()))
                    .collect(),
            ),
        ];
        let fp = ParsedFilename {
            series_title: "The Authority".into(),
            number: "4".into(),
            volume: None,
            year: None,
            title: None,
            pattern_id: 1,
        };
        let r = match_file(None, Some(&fp), FolderEvidence::Year(2008), &candidates);
        assert_eq!(r.issue_id, Some(1004), "the 29-issue 2008 volume wins");
        assert!(
            !r.ambiguous,
            "folder year narrowed to the 2008 pair, and 29 beats 15 within it"
        );
    }

    /// A sort_title whose normalization drifts from its sibling's would both
    /// score lower AND fall out of the rival set — disabling the guard for
    /// exactly the rows most likely to be wrong.
    #[test]
    fn a_drifted_sort_title_does_not_disable_the_volume_guard() {
        let mut old = candidate(
            series(1, "The Authority", Some(1999)),
            (1..=29)
                .map(|n| issue(1000 + n, 1, &n.to_string()))
                .collect(),
        );
        let mut new = candidate(
            series(2, "The Authority", Some(2008)),
            (1..=29)
                .map(|n| issue(2000 + n, 2, &n.to_string()))
                .collect(),
        );
        // Historical drift: same title, differently-shaped stored sort_title.
        old.series.sort_title = "authority".into();
        new.series.sort_title = "Authority, The".into();

        let fp = ParsedFilename {
            series_title: "The Authority".into(),
            number: "4".into(),
            volume: None,
            year: Some(2009),
            title: None,
            pattern_id: 1,
        };
        let r = match_file(None, Some(&fp), FolderEvidence::Year(2008), &[old, new]);
        assert_eq!(
            r.issue_id,
            Some(2004),
            "the drifted row must still be reachable and still win on folder year"
        );
        assert!(!r.ambiguous);
    }

    /// Row 3, the case that killed the strict rule: a correctly-tagged file
    /// in a yearless series folder. ComicInfo `<Volume>` is real volume
    /// metadata and must still select — otherwise every multi-volume series
    /// with a yearless folder floods needs_review.
    #[test]
    fn a_silent_folder_still_defers_to_comicinfo_volume() {
        let candidates = vec![
            candidate(
                series(1, "Ultimate Spider-Man", Some(2000)),
                vec![issue(101, 1, "1")],
            ),
            candidate(
                series(2, "Ultimate Spider-Man", Some(2024)),
                vec![issue(201, 2, "1")],
            ),
        ];
        let ci = ComicInfo {
            series: Some("Ultimate Spider-Man".into()),
            number: Some("1".into()),
            year: Some(2024), // <Volume> — genuine volume metadata
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::Silent, &candidates);
        assert_eq!(r.issue_id, Some(201), "ComicInfo <Volume> names the volume");
        assert!(
            !r.ambiguous,
            "volume metadata is evidence, so this is earned"
        );
    }

    /// Row 3 with nothing to defer to. The folder exists and declined to say;
    /// a filename year is a release year and may not stand in for it.
    #[test]
    fn a_silent_folder_with_only_a_filename_year_abstains() {
        let fp = ParsedFilename {
            series_title: "The Authority".into(),
            number: "4".into(),
            volume: None,
            year: Some(1999), // coincides with a real volume — still not evidence
            title: None,
            pattern_id: 1,
        };
        let r = match_file(
            None,
            Some(&fp),
            FolderEvidence::Silent,
            &authority_volumes(),
        );
        assert!(
            r.ambiguous,
            "a release year must not select a volume just because it happens to match one"
        );
    }

    /// Issue count may break a tie among candidates volume evidence SELECTED,
    /// but may not substitute for volume evidence that is absent.
    #[test]
    fn issue_count_cannot_substitute_for_missing_volume_evidence() {
        let candidates = vec![
            candidate(
                series(1, "The Authority", Some(1999)),
                (1..=29)
                    .map(|n| issue(1000 + n, 1, &n.to_string()))
                    .collect(),
            ),
            candidate(
                series(2, "The Authority", Some(2008)),
                vec![issue(2004, 2, "4")],
            ),
        ];
        let fp = ParsedFilename {
            series_title: "The Authority".into(),
            number: "4".into(),
            volume: None,
            year: None,
            title: None,
            pattern_id: 1,
        };
        // Silent folder: the 29-issue catalog must NOT win a volume question.
        let r = match_file(None, Some(&fp), FolderEvidence::Silent, &candidates);
        assert!(
            r.ambiguous,
            "catalog size has no bearing on which volume this is"
        );

        // Same pool, same file, but volume evidence selects — then the pick
        // is earned even though it is the smaller catalog.
        let r = match_file(None, Some(&fp), FolderEvidence::Year(2008), &candidates);
        assert_eq!(r.issue_id, Some(2004));
        assert!(!r.ambiguous);
    }

    #[test]
    fn tiebreak_prefers_more_issues_over_lower_id() {
        // Live repro: "Sam and Twitch Case Files" had id=381
        // (start_year=2025, 1 issue) and id=918 (start_year=2024, 24
        // issues). Files arrived with a 2026 scan-year hint so neither
        // matched on year. Pre-fix the lower id won and the
        // 1-issue stub didn't have the file's issue number — files
        // sat in /watch/ forever. Post-fix the larger catalog entry
        // wins.
        let small_id_few_issues = candidate(
            series(381, "Sam and Twitch Case Files", Some(2025)),
            vec![issue(10, 381, "1")],
        );
        let mut bigger_issues = Vec::new();
        for n in 1..=24 {
            bigger_issues.push(issue(100 + n, 918, &n.to_string()));
        }
        let large_id_many_issues = candidate(
            series(918, "Sam and Twitch Case Files", Some(2024)),
            bigger_issues,
        );
        let candidates = vec![small_id_few_issues, large_id_many_issues];
        let ci = ComicInfo {
            series: Some("Sam and Twitch Case Files".into()),
            number: Some("23".into()),
            year: Some(2026), // matches neither start_year
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        // The 24-issue series carries #23; the 1-issue stub does
        // not. Pre-fix this returned Unmatched (lower-id won,
        // didn't have #23); post-fix issue_id is the catalog #23.
        assert_eq!(r.issue_id, Some(100 + 23));
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        // ASSERT THE FLAG, not just the pointer. Without this the test passes
        // while the behaviour it protects regresses: `ambiguous` routes to
        // needs_review, which longbox-postprocess refuses to import, so the
        // file goes straight back to sitting in /watch/ — the exact bug this
        // rung was added to fix, returning invisibly past its own guard.
        assert!(
            !r.ambiguous,
            "a clear catalog-size leader with no series folder must import, not abstain"
        );
    }

    #[test]
    fn tiebreak_year_still_beats_issue_count() {
        // Year-hint match is a stronger signal than issue count — a
        // 24-issue catalog entry must NOT win over the 1-issue entry
        // when the year hint exactly matches the 1-issue entry's
        // start_year. Issue-count tiebreak only kicks in when year
        // hint doesn't discriminate.
        let small_with_year_match = candidate(
            series(381, "Sam and Twitch Case Files", Some(2025)),
            vec![issue(10, 381, "1")],
        );
        let mut bigger_issues = Vec::new();
        for n in 1..=24 {
            bigger_issues.push(issue(100 + n, 918, &n.to_string()));
        }
        let large_without_year_match = candidate(
            series(918, "Sam and Twitch Case Files", Some(2024)),
            bigger_issues,
        );
        let candidates = vec![small_with_year_match, large_without_year_match];
        let ci = ComicInfo {
            series: Some("Sam and Twitch Case Files".into()),
            number: Some("1".into()),
            year: Some(2025), // matches start_year=2025 only
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(
            r.issue_id,
            Some(10),
            "year-hint match must outrank issue count"
        );
    }

    #[test]
    fn tier2_handles_leading_zero_issue_numbers() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("001".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.issue_id, Some(10));
    }

    // -------- Tier 3 (filename) --------

    #[test]
    fn tier3_clean_filename_caps_at_ceiling() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(None, Some(&parsed), FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (r.confidence - FILENAME_CONFIDENCE_CEILING).abs() < 1e-9,
            "expected ceiling 0.90, got {}",
            r.confidence
        );
    }

    #[test]
    fn tier3_unmatched_when_no_candidates() {
        let parsed = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(None, Some(&parsed), FolderEvidence::NoFolder, &[]);
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn tier3_unmatched_when_series_unrecognized() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Some Other Comic".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(None, Some(&parsed), FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.method, MatchMethod::Unmatched);
    }

    // -------- Cross-cutting --------

    #[test]
    fn no_match_when_both_inputs_are_none() {
        let r = match_file(None, None, FolderEvidence::NoFolder, &[]);
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn full_cascade_prefers_higher_tier_even_when_lower_scores_higher() {
        // Tier 2: typo in ComicInfo → ~0.7 confidence (needs_review zone).
        // Tier 3: clean filename → would score ~1.0 but capped at 0.90.
        // Tier 2 still wins because it ran first.
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Saqa".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let parsed = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(
            Some(&ci),
            Some(&parsed),
            FolderEvidence::NoFolder,
            &candidates,
        );
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert!(
            r.confidence < FILENAME_CONFIDENCE_CEILING,
            "expected Tier 2 confidence below filename ceiling, got {}",
            r.confidence
        );
    }

    #[test]
    fn symmetric_normalize_rescues_un_normalized_sort_title() {
        // Live data shows ~50% of catalog rows have un-normalized
        // sort_titles (mixed case, etc.) — `Series::new` builds them
        // via `normalize_title` for new rows, but historical paths
        // (older inserts, batch imports) stored them raw. Without
        // symmetric normalization at match time, a filename hint
        // `Y The Last Man` (normalized to `y the last man`) scored
        // ~0.71 against a stored `Y The Last Man` sort_title — below
        // the 0.85 owned threshold, sometimes below the 0.65
        // needs-review floor, dead-ending as unmatched. With the
        // symmetric normalize this scores 1.0.
        let s = Series {
            id: 1,
            cv_id: None,
            metron_id: None,
            title: "Y The Last Man".into(),
            sort_title: "Y The Last Man".into(), // un-normalized!
            start_year: Some(2002),
            publisher: None,
            description: None,
            cover_url: None,
        };
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Y The Last Man".into(),
            number: "1".into(),
            volume: None,
            year: Some(2002),
            title: None,
            pattern_id: 4,
        };
        let r = match_file(None, Some(&parsed), FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (r.confidence - FILENAME_CONFIDENCE_CEILING).abs() < 1e-9,
            "expected ceiling 0.90 from a 1.0 raw similarity, got {}",
            r.confidence
        );
    }

    #[test]
    fn nfc_normalization_makes_decomposed_unicode_match_precomposed() {
        // A filename pulled from macOS HFS+ stores `é` as decomposed
        // `e` + combining-acute. The catalog's `sort_title` (built from
        // CV's NFC-encoded data) carries the precomposed `é`. Before
        // NFC in `normalize_title` the two compared as different
        // strings; jaccard treated `pérez` (NFD) and `pérez` (NFC) as
        // disjoint tokens.
        let s = Series {
            id: 1,
            cv_id: None,
            metron_id: None,
            title: "Pérez".into(),      // precomposed
            sort_title: "pérez".into(), // precomposed (NFC)
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        };
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Pe\u{0301}rez".into(), // decomposed (NFD)
            number: "1".into(),
            volume: None,
            year: None,
            title: None,
            pattern_id: 4,
        };
        let r = match_file(None, Some(&parsed), FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.issue_id, Some(10));
    }

    #[test]
    fn by_author_attribution_strip_makes_verbose_filename_match_terse_catalog() {
        // Real case: ComicInfo / filename carries `Stillwater by
        // Zdarsky & Pérez` (Amazon-scraped verbose form) but the
        // catalog row is just `Stillwater`. Token-set jaccard would
        // score 1/4 = 0.25 without the strip. The `" by ..."` strip
        // in `normalize_title` collapses both sides to `stillwater`.
        let s = Series {
            id: 1,
            cv_id: None,
            metron_id: None,
            title: "Stillwater".into(),
            sort_title: "stillwater".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        };
        let candidates = vec![candidate(s, vec![issue(10, 1, "12")])];
        let parsed = ParsedFilename {
            series_title: "Stillwater by Zdarsky & Pérez".into(),
            number: "12".into(),
            volume: None,
            year: None,
            title: None,
            pattern_id: 4,
        };
        let r = match_file(None, Some(&parsed), FolderEvidence::NoFolder, &candidates);
        assert_eq!(r.issue_id, Some(10));
    }

    // -------- Tier 2 / Tier 3 cross-check --------

    /// The live Ferocious case, end to end: the file is physically issue 2,
    /// named and foldered correctly, but its embedded `<Number>` says 1. Tier
    /// 2 used to win outright and file it under #1 as `owned`, colliding with
    /// the real #1 — which is how all 28 mismatch groups happened.
    fn ferocious() -> Vec<Candidate> {
        let s = series(1, "Ferocious", Some(2025));
        vec![candidate(
            s,
            (1..=5)
                .map(|n| issue(8467 + n, 1, &n.to_string()))
                .collect(),
        )]
    }

    fn lying_comicinfo() -> ComicInfo {
        ComicInfo {
            series: Some("Ferocious".into()),
            number: Some("1".into()), // the lie
            year: Some(2025),
            ..Default::default()
        }
    }

    fn honest_filename(number: &str) -> ParsedFilename {
        ParsedFilename {
            series_title: "Ferocious".into(),
            number: number.to_owned(),
            volume: None,
            year: Some(2025),
            title: None,
            pattern_id: 2,
        }
    }

    #[test]
    fn disagreement_takes_the_filename_issue_and_flags_it() {
        let r = match_file(
            Some(&lying_comicinfo()),
            Some(&honest_filename("2")),
            FolderEvidence::NoFolder,
            &ferocious(),
        );
        assert!(r.ambiguous, "tier2/tier3 disagreement must be flagged");
        assert_eq!(r.issue_id, Some(8469), "the filename's issue (#2) wins");
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        // And the flag — not the confidence — is what keeps it out of `owned`,
        // even at a threshold the file's 0.90 would otherwise clear.
        use crate::file::classify_status;
        assert_eq!(
            classify_status(r.issue_id, r.confidence, r.method, 0.85, r.ambiguous),
            crate::FileStatus::NeedsReview
        );
        // Even a user who dropped the threshold to the floor still gets a
        // human in the loop — the whole reason this is a flag, not a clamp.
        assert_eq!(
            classify_status(r.issue_id, r.confidence, r.method, 0.65, r.ambiguous),
            crate::FileStatus::NeedsReview
        );
    }

    #[test]
    fn agreement_is_unchanged_tier2_still_wins() {
        let ci = ComicInfo {
            series: Some("Ferocious".into()),
            number: Some("2".into()),
            year: Some(2025),
            ..Default::default()
        };
        let r = match_file(
            Some(&ci),
            Some(&honest_filename("2")),
            FolderEvidence::NoFolder,
            &ferocious(),
        );
        assert!(!r.ambiguous);
        assert_eq!(r.issue_id, Some(8469));
        // Tier 2 still owns the result (uncapped confidence, ComicInfo method).
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert!(r.confidence > FILENAME_CONFIDENCE_CEILING);
    }

    #[test]
    fn tier3_no_parse_leaves_tier2_standing() {
        // Nothing to cross-check against — Tier 2's word is all we have.
        let r = match_file(
            Some(&lying_comicinfo()),
            None,
            FolderEvidence::NoFolder,
            &ferocious(),
        );
        assert!(!r.ambiguous);
        assert_eq!(r.issue_id, Some(8468));
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
    }

    #[test]
    fn tier3_parsing_an_issue_the_series_lacks_leaves_tier2_standing() {
        // The filename says #9; the series only has #1–#5, so Tier 3 resolves
        // nothing. That's not a disagreement, it's an absence — don't flag.
        let r = match_file(
            Some(&lying_comicinfo()),
            Some(&honest_filename("9")),
            FolderEvidence::NoFolder,
            &ferocious(),
        );
        assert!(!r.ambiguous);
        assert_eq!(r.issue_id, Some(8468));
    }

    #[test]
    fn tier2_below_floor_still_falls_through_to_tier3_unflagged() {
        // Tier 2 never resolves, so there's nothing to contradict — the
        // pre-existing fall-through, not an ambiguity.
        let ci = ComicInfo {
            series: Some("ZZZZZZZZZ".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let r = match_file(
            Some(&ci),
            Some(&honest_filename("2")),
            FolderEvidence::NoFolder,
            &ferocious(),
        );
        assert!(!r.ambiguous);
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(8469));
    }

    #[test]
    fn a_yearless_filename_never_drags_a_file_onto_the_wrong_volume() {
        // Two volumes of one title — the relaunch case (Ultimate Spider-Man
        // 2000 and 2024, Daredevil, X-Men, take your pick). They score
        // identically on title similarity, so the year hint alone decides.
        //
        // ComicInfo knows the volume (2024) and is RIGHT. The filename carries
        // no year, so Tier 3 falls back to a tiebreak and picks the 2000
        // volume. That's not a contradiction — it's Tier 3 having strictly
        // less information. Treating it as one would take a correctly-tagged,
        // owned file, re-point it at the wrong volume's #1, and flag it for
        // review — colliding with the real 2000 #1 and manufacturing a fresh
        // mismatch group. The cure would be causing the disease.
        //
        // So: cross-series disagreement leaves Tier 2 standing, unflagged.
        let mut old_volume_issues = Vec::new();
        for n in 1..=20 {
            old_volume_issues.push(issue(100 + n, 1, &n.to_string()));
        }
        let candidates = vec![
            candidate(
                series(1, "Ultimate Spider-Man", Some(2000)),
                old_volume_issues,
            ),
            candidate(
                series(2, "Ultimate Spider-Man", Some(2024)),
                vec![issue(201, 2, "1")],
            ),
        ];
        let ci = ComicInfo {
            series: Some("Ultimate Spider-Man".into()),
            number: Some("1".into()),
            year: Some(2024), // correct, and the only thing that knows
            ..Default::default()
        };
        let parsed = ParsedFilename {
            series_title: "Ultimate Spider-Man".into(),
            number: "1".into(),
            volume: None,
            year: None, // the common case
            title: None,
            pattern_id: 2,
        };
        let r = match_file(
            Some(&ci),
            Some(&parsed),
            FolderEvidence::NoFolder,
            &candidates,
        );
        assert_eq!(
            r.issue_id,
            Some(201),
            "must stay on the 2024 volume ComicInfo names"
        );
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert!(
            !r.ambiguous,
            "a missing year hint is not a contradiction — flagging it would \
             bury the user in false positives on every multi-volume series"
        );
    }

    #[test]
    fn threshold_boundary_at_exact_value_classifies_as_owned() {
        // classify_status is the post-hoc step. Just verify it's reachable
        // from a matcher result.
        use crate::file::classify_status;
        let r = MatchResult {
            issue_id: Some(7),
            method: MatchMethod::ComicInfoXml,
            confidence: 0.85,
            ambiguous: false,
        };
        let s = classify_status(r.issue_id, r.confidence, r.method, 0.85, r.ambiguous);
        assert_eq!(s, crate::FileStatus::Owned);
    }
}

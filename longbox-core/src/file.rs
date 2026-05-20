//! `LocalFile` domain type plus the `MatchMethod` and `FileStatus` enums.
//! Status determination from a [`crate::MatchResult`] lives at the boundary
//! (see [`classify_status`]) — the matcher itself only emits the method and
//! confidence.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMethod {
    /// CV issue ID extracted from a `<Web>` URL in ComicInfo.xml.
    WebUrlCv,
    /// Metron issue slug extracted from a `<Web>` URL in ComicInfo.xml.
    /// Passive ingestion — Phase A captures these but does not call Metron.
    WebUrlMetron,
    /// Matched via `<Series>` + `<Number>` similarity in ComicInfo.xml.
    ComicInfoXml,
    /// Matched via filename parsing through `parsing_patterns`.
    FilenameRegex,
    /// User resolved via the disambiguation UI.
    Manual,
    /// Phase B's post-process pipeline imported the file with full
    /// caller-supplied identity (series_id + issue_id known at import
    /// time). Highest-confidence path — Phase B upserts override
    /// lower-confidence matches from the scanner's filename / ComicInfo
    /// tiers without prompting.
    PhaseB,
    /// No plausible match found in any tier.
    Unmatched,
    /// User-marked as not-a-comic; not surfaced in catalog views.
    Ignored,
}

impl MatchMethod {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::WebUrlCv => "web_url_cv",
            Self::WebUrlMetron => "web_url_metron",
            Self::ComicInfoXml => "comicinfo_xml",
            Self::FilenameRegex => "filename_regex",
            Self::Manual => "manual",
            Self::PhaseB => "phase_b",
            Self::Unmatched => "unmatched",
            Self::Ignored => "ignored",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "web_url_cv" => Self::WebUrlCv,
            "web_url_metron" => Self::WebUrlMetron,
            "comicinfo_xml" => Self::ComicInfoXml,
            "filename_regex" => Self::FilenameRegex,
            "manual" => Self::Manual,
            "phase_b" => Self::PhaseB,
            "unmatched" => Self::Unmatched,
            "ignored" => Self::Ignored,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// `match_confidence >= threshold` AND `issue_id IS NOT NULL`.
    Owned,
    /// A candidate match exists but confidence is below the threshold.
    NeedsReview,
    /// No candidate found.
    Unmatched,
    /// User-marked; suppressed from catalog views.
    Ignored,
}

impl FileStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Owned => "owned",
            Self::NeedsReview => "needs_review",
            Self::Unmatched => "unmatched",
            Self::Ignored => "ignored",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "owned" => Self::Owned,
            "needs_review" => Self::NeedsReview,
            "unmatched" => Self::Unmatched,
            "ignored" => Self::Ignored,
            _ => return None,
        })
    }
}

/// Derive a [`FileStatus`] from a match result and the caller's threshold.
/// The matcher itself returns only `(issue_id, method, confidence)` — this
/// helper is the post-hoc classification step.
pub fn classify_status(
    issue_id: Option<i64>,
    confidence: f64,
    method: MatchMethod,
    threshold: f64,
) -> FileStatus {
    match method {
        MatchMethod::Ignored => FileStatus::Ignored,
        MatchMethod::Unmatched => FileStatus::Unmatched,
        _ => {
            if issue_id.is_some() && confidence >= threshold {
                FileStatus::Owned
            } else if issue_id.is_some() {
                FileStatus::NeedsReview
            } else {
                FileStatus::Unmatched
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalFile {
    pub id: i64,
    pub issue_id: Option<i64>,
    pub library_root_id: i64,
    /// Path relative to the library root. Forward slashes; no leading slash.
    pub path_relative: String,
    pub size_bytes: i64,
    pub mtime: OffsetDateTime,
    pub last_scanned_at: OffsetDateTime,
    pub match_method: MatchMethod,
    pub match_confidence: f64,
    pub status: FileStatus,
    /// Cached ComicInfo.xml bytes so re-matching doesn't re-open the .cbz
    /// archive when `(size, mtime)` is unchanged.
    pub cached_comicinfo_xml: Option<String>,
    pub cached_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_method_db_round_trip() {
        for m in [
            MatchMethod::WebUrlCv,
            MatchMethod::WebUrlMetron,
            MatchMethod::ComicInfoXml,
            MatchMethod::FilenameRegex,
            MatchMethod::Manual,
            MatchMethod::PhaseB,
            MatchMethod::Unmatched,
            MatchMethod::Ignored,
        ] {
            let s = m.as_db_str();
            assert_eq!(
                MatchMethod::from_db_str(s),
                Some(m),
                "round-trip failed for {s}"
            );
        }
        assert_eq!(MatchMethod::from_db_str("bogus"), None);
    }

    #[test]
    fn file_status_db_round_trip() {
        for s in [
            FileStatus::Owned,
            FileStatus::NeedsReview,
            FileStatus::Unmatched,
            FileStatus::Ignored,
        ] {
            let str_form = s.as_db_str();
            assert_eq!(FileStatus::from_db_str(str_form), Some(s));
        }
        assert_eq!(FileStatus::from_db_str("bogus"), None);
    }

    #[test]
    fn classify_owned_when_above_threshold() {
        let status = classify_status(Some(7), 0.95, MatchMethod::ComicInfoXml, 0.85);
        assert_eq!(status, FileStatus::Owned);
    }

    #[test]
    fn classify_owned_at_exact_threshold() {
        // Spec uses `>=`. 0.85 exactly = owned at default threshold.
        let status = classify_status(Some(7), 0.85, MatchMethod::FilenameRegex, 0.85);
        assert_eq!(status, FileStatus::Owned);
    }

    #[test]
    fn classify_needs_review_below_threshold_with_candidate() {
        let status = classify_status(Some(7), 0.72, MatchMethod::ComicInfoXml, 0.85);
        assert_eq!(status, FileStatus::NeedsReview);
    }

    #[test]
    fn classify_unmatched_when_no_candidate() {
        let status = classify_status(None, 0.0, MatchMethod::Unmatched, 0.85);
        assert_eq!(status, FileStatus::Unmatched);
    }

    #[test]
    fn classify_ignored_overrides_confidence() {
        // Even with high confidence, Ignored method → Ignored status.
        let status = classify_status(Some(7), 1.0, MatchMethod::Ignored, 0.85);
        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn classify_manual_at_full_confidence() {
        let status = classify_status(Some(7), 1.0, MatchMethod::Manual, 0.85);
        assert_eq!(status, FileStatus::Owned);
    }
}

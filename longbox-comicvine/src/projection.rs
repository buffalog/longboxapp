//! Projection: convert internal CV DTOs to public, flatter types. Public
//! types intentionally do NOT depend on `longbox-core` — orchestration
//! (e.g. computing `sort_title`) is the web handler's job.

use serde::{Deserialize, Serialize};

use crate::models::{
    CvImage, CvIssueCreditsRaw, CvIssueFull, CvPersonVolumeCreditsRaw, CvPublisher, CvVolumeFull,
    CvVolumeSearchItem,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesSearchResult {
    pub cv_id: i64,
    pub name: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub issue_count: u32,
    pub cover_url: Option<String>,
    /// CV's short "deck" summary, when present.
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvVolumeDetail {
    pub cv_id: i64,
    pub name: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    /// Full HTML description from CV's `description` field.
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub site_detail_url: String,
    /// CV's newline-separated alternate titles (e.g. "Collider" for FBP).
    pub aliases: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvIssueDetail {
    pub cv_issue_id: i64,
    pub issue_number: String,
    pub name: Option<String>,
    pub cover_date: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub site_detail_url: String,
}

/// One release-calendar entry — an issue projected from a
/// `store_date`-filtered `/issues/` query. Carries the owning volume
/// (id + name) so the web layer can link a release back to a tracked
/// series, and the `store_date` the calendar groups on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvCalendarItem {
    pub cv_issue_id: i64,
    pub issue_number: String,
    pub store_date: String,
    pub cv_volume_id: i64,
    pub volume_name: String,
    pub cover_url: Option<String>,
    pub site_detail_url: String,
}

/// One atomic person+role credit on an issue (CV's comma-delimited role
/// strings are exploded into one of these per role). `cv_person_id` is the
/// CV person resource id used to dedupe creators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvPersonCredit {
    pub cv_person_id: i64,
    pub name: String,
    pub role: String,
}

/// Explode a raw per-issue credits payload into atomic `CvPersonCredit`
/// rows: split each comma-delimited `role` string, trim + lowercase each
/// role, drop empties. One output entry per person+atomic-role; a person's
/// order and CV's order are preserved.
pub(crate) fn project_issue_credits(raw: CvIssueCreditsRaw) -> Vec<CvPersonCredit> {
    let mut out = Vec::new();
    for c in raw.person_credits {
        for role in c.role.split(',') {
            let role = role.trim().to_lowercase();
            if role.is_empty() {
                continue;
            }
            out.push(CvPersonCredit {
                cv_person_id: c.id,
                name: c.name.clone(),
                role,
            });
        }
    }
    out
}

/// One series (CV volume) a creator is credited on — a bibliography entry.
/// Series-level only: CV's `volume_credits` carries no role/issue detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvVolumeCredit {
    pub cv_volume_id: i64,
    pub name: String,
}

/// Project a person's raw volume_credits into deduped `CvVolumeCredit` entries
/// (CV occasionally repeats a volume; keep first occurrence, preserve order).
// ponytail: expect(dead_code) bridges until Task 2 adds the client method.
#[allow(dead_code)]
pub(crate) fn project_person_volume_credits(raw: CvPersonVolumeCreditsRaw) -> Vec<CvVolumeCredit> {
    let mut seen = std::collections::HashSet::new();
    raw.volume_credits
        .into_iter()
        .filter(|v| seen.insert(v.id))
        .map(|v| CvVolumeCredit {
            cv_volume_id: v.id,
            name: v.name,
        })
        .collect()
}

pub(crate) fn project_search_item(item: CvVolumeSearchItem) -> SeriesSearchResult {
    SeriesSearchResult {
        cv_id: item.id,
        name: item.name,
        start_year: parse_year(item.start_year.as_deref()),
        publisher: extract_publisher_name(item.publisher),
        issue_count: item.count_of_issues,
        cover_url: extract_cover(item.image),
        description: item.deck,
    }
}

pub(crate) fn project_volume(item: CvVolumeFull) -> CvVolumeDetail {
    CvVolumeDetail {
        cv_id: item.id,
        name: item.name,
        start_year: parse_year(item.start_year.as_deref()),
        publisher: extract_publisher_name(item.publisher),
        description: item.description,
        cover_url: extract_cover(item.image),
        site_detail_url: item.site_detail_url,
        aliases: item.aliases,
    }
}

pub(crate) fn project_issue(item: CvIssueFull) -> CvIssueDetail {
    CvIssueDetail {
        cv_issue_id: item.id,
        issue_number: item.issue_number,
        name: item.name,
        cover_date: item.cover_date,
        description: item.description,
        cover_url: extract_cover(item.image),
        site_detail_url: item.site_detail_url,
    }
}

/// Project a CV issue into a calendar item, or `None` when it lacks the
/// `store_date` / `volume` the calendar requires. A `store_date`-filtered
/// query only returns dated issues, so the `None` path is defensive.
pub(crate) fn project_calendar_item(item: CvIssueFull) -> Option<CvCalendarItem> {
    let store_date = item.store_date?;
    let volume = item.volume?;
    Some(CvCalendarItem {
        cv_issue_id: item.id,
        issue_number: item.issue_number,
        store_date,
        cv_volume_id: volume.id,
        volume_name: volume.name.unwrap_or_default(),
        cover_url: extract_cover(item.image),
        site_detail_url: item.site_detail_url,
    })
}

fn parse_year(raw: Option<&str>) -> Option<i32> {
    raw.and_then(|s| s.trim().parse::<i32>().ok())
}

fn extract_publisher_name(p: Option<CvPublisher>) -> Option<String> {
    p.and_then(|p| p.name)
}

fn extract_cover(img: Option<CvImage>) -> Option<String> {
    img.and_then(|i| i.medium_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(medium: &str) -> CvImage {
        CvImage {
            medium_url: Some(medium.to_owned()),
        }
    }

    #[test]
    fn search_item_projects_medium_image_to_cover_url() {
        let item = CvVolumeSearchItem {
            id: 18166,
            name: "The Walking Dead".into(),
            start_year: Some("2003".into()),
            publisher: Some(CvPublisher {
                name: Some("Image".into()),
            }),
            count_of_issues: 193,
            image: Some(img("https://cdn/medium.jpg")),
            deck: Some("Apocalypse comic.".into()),
        };
        let projected = project_search_item(item);
        assert_eq!(
            projected.cover_url.as_deref(),
            Some("https://cdn/medium.jpg")
        );
        assert_eq!(projected.publisher.as_deref(), Some("Image"));
        assert_eq!(projected.start_year, Some(2003));
        assert_eq!(projected.issue_count, 193);
    }

    #[test]
    fn search_item_with_null_publisher_yields_none() {
        let item = CvVolumeSearchItem {
            id: 1,
            name: "X".into(),
            start_year: None,
            publisher: None,
            count_of_issues: 0,
            image: None,
            deck: None,
        };
        let projected = project_search_item(item);
        assert!(projected.publisher.is_none());
        assert!(projected.cover_url.is_none());
        assert!(projected.description.is_none());
        assert_eq!(projected.start_year, None);
    }

    #[test]
    fn publisher_present_but_name_null_yields_none() {
        // CV occasionally returns `publisher: { id: 1, name: null }`.
        let item = CvVolumeSearchItem {
            id: 1,
            name: "X".into(),
            start_year: None,
            publisher: Some(CvPublisher { name: None }),
            count_of_issues: 0,
            image: None,
            deck: None,
        };
        let projected = project_search_item(item);
        assert!(projected.publisher.is_none());
    }

    #[test]
    fn unparseable_start_year_yields_none() {
        let item = CvVolumeSearchItem {
            id: 1,
            name: "X".into(),
            start_year: Some("not a year".into()),
            publisher: None,
            count_of_issues: 0,
            image: None,
            deck: None,
        };
        assert!(project_search_item(item).start_year.is_none());
    }

    #[test]
    fn issue_projection_preserves_string_number() {
        let issue = CvIssueFull {
            id: 12345,
            issue_number: "1.MU".into(),
            name: Some("Marvel Universe Tie-In".into()),
            cover_date: Some("2009-08-15".into()),
            store_date: None,
            description: None,
            image: Some(img("https://cdn/issue-medium.jpg")),
            volume: None,
            site_detail_url: "https://cv/issue/4000-12345/".into(),
        };
        let projected = project_issue(issue);
        assert_eq!(projected.issue_number, "1.MU");
        assert_eq!(projected.cv_issue_id, 12345);
        assert_eq!(
            projected.cover_url.as_deref(),
            Some("https://cdn/issue-medium.jpg")
        );
    }

    #[test]
    fn calendar_item_projects_volume_and_store_date() {
        let issue = CvIssueFull {
            id: 700,
            issue_number: "12".into(),
            name: None,
            cover_date: Some("2026-07-01".into()),
            store_date: Some("2026-05-13".into()),
            description: None,
            image: Some(img("https://cdn/c.jpg")),
            volume: Some(crate::models::CvVolumeRef {
                id: 4050,
                name: Some("Saga".into()),
            }),
            site_detail_url: "https://cv/issue/4000-700/".into(),
        };
        let item = project_calendar_item(issue).expect("has store_date + volume");
        assert_eq!(item.store_date, "2026-05-13");
        assert_eq!(item.cv_volume_id, 4050);
        assert_eq!(item.volume_name, "Saga");
        assert_eq!(item.cv_issue_id, 700);
    }

    #[test]
    fn calendar_item_skipped_when_store_date_absent() {
        let issue = CvIssueFull {
            id: 701,
            issue_number: "1".into(),
            name: None,
            cover_date: None,
            store_date: None,
            description: None,
            image: None,
            volume: Some(crate::models::CvVolumeRef { id: 1, name: None }),
            site_detail_url: "https://cv/issue/4000-701/".into(),
        };
        assert!(project_calendar_item(issue).is_none());
    }

    #[test]
    fn volume_projection_carries_site_detail_url() {
        let v = CvVolumeFull {
            id: 18166,
            name: "Saga".into(),
            start_year: Some("2012".into()),
            publisher: None,
            description: Some("<p>Galaxy-spanning epic.</p>".into()),
            image: None,
            site_detail_url: "https://cv/volume/4050-18166/".into(),
            aliases: None,
        };
        let projected = project_volume(v);
        assert_eq!(projected.site_detail_url, "https://cv/volume/4050-18166/");
        assert!(projected.cover_url.is_none());
        assert_eq!(projected.start_year, Some(2012));
    }
}

#[cfg(test)]
mod credit_tests {
    use super::*;
    use crate::models::{CvCreditRaw, CvIssueCreditsRaw};

    #[test]
    fn splits_comma_delimited_roles_into_atomic_lowercase_rows() {
        // CV packs multi-role people into one comma-delimited `role` string.
        let raw = CvIssueCreditsRaw {
            person_credits: vec![
                CvCreditRaw {
                    id: 97470,
                    name: "Bob Quinn".into(),
                    role: "artist, colorist, cover".into(),
                },
                CvCreditRaw {
                    id: 130355,
                    name: "Ethan S. Parker".into(),
                    role: "Writer".into(),
                },
                CvCreditRaw {
                    id: 1,
                    name: "Blank".into(),
                    role: "  ".into(),
                }, // whitespace-only -> dropped
            ],
        };
        let out = project_issue_credits(raw);
        assert_eq!(
            out,
            vec![
                CvPersonCredit {
                    cv_person_id: 97470,
                    name: "Bob Quinn".into(),
                    role: "artist".into()
                },
                CvPersonCredit {
                    cv_person_id: 97470,
                    name: "Bob Quinn".into(),
                    role: "colorist".into()
                },
                CvPersonCredit {
                    cv_person_id: 97470,
                    name: "Bob Quinn".into(),
                    role: "cover".into()
                },
                CvPersonCredit {
                    cv_person_id: 130355,
                    name: "Ethan S. Parker".into(),
                    role: "writer".into()
                },
            ]
        );
    }
}

#[cfg(test)]
mod volume_credit_tests {
    use super::*;
    use crate::models::{CvPersonVolumeCreditsRaw, CvVolumeCreditRaw};

    #[test]
    fn projects_and_dedupes_by_volume_id() {
        let raw = CvPersonVolumeCreditsRaw {
            volume_credits: vec![
                CvVolumeCreditRaw {
                    id: 7084,
                    name: "Avengers".into(),
                },
                CvVolumeCreditRaw {
                    id: 18166,
                    name: "Uncanny X-Force".into(),
                },
                CvVolumeCreditRaw {
                    id: 7084,
                    name: "Avengers".into(),
                }, // dup -> dropped
            ],
        };
        let out = project_person_volume_credits(raw);
        assert_eq!(
            out,
            vec![
                CvVolumeCredit {
                    cv_volume_id: 7084,
                    name: "Avengers".into()
                },
                CvVolumeCredit {
                    cv_volume_id: 18166,
                    name: "Uncanny X-Force".into()
                },
            ]
        );
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;
    use crate::models::CvVolumeFull;

    #[test]
    fn project_volume_carries_aliases() {
        let raw = CvVolumeFull {
            id: 42,
            name: "FBP: Federal Bureau of Physics".into(),
            start_year: Some("2013".into()),
            publisher: None,
            description: None,
            image: None,
            site_detail_url: "https://x".into(),
            aliases: Some("Collider\nCollider Comics".into()),
        };
        let detail = project_volume(raw);
        assert_eq!(detail.aliases.as_deref(), Some("Collider\nCollider Comics"));
    }
}

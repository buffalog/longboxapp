//! Projection: convert internal CV DTOs to public, flatter types. Public
//! types intentionally do NOT depend on `longbox-core` — orchestration
//! (e.g. computing `sort_title`) is the web handler's job.

use serde::{Deserialize, Serialize};

use crate::models::{CvImage, CvIssueFull, CvPublisher, CvVolumeFull, CvVolumeSearchItem};

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
        assert_eq!(projected.cover_url.as_deref(), Some("https://cdn/medium.jpg"));
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
            description: None,
            image: Some(img("https://cdn/issue-medium.jpg")),
            site_detail_url: "https://cv/issue/4000-12345/".into(),
        };
        let projected = project_issue(issue);
        assert_eq!(projected.issue_number, "1.MU");
        assert_eq!(projected.cv_issue_id, 12345);
        assert_eq!(projected.cover_url.as_deref(), Some("https://cdn/issue-medium.jpg"));
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
        };
        let projected = project_volume(v);
        assert_eq!(projected.site_detail_url, "https://cv/volume/4050-18166/");
        assert!(projected.cover_url.is_none());
        assert_eq!(projected.start_year, Some(2012));
    }
}

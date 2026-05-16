//! Internal ComicVine response DTOs. These mirror CV's JSON shape and never
//! leave the crate — public surface uses [`crate::projection`] types.

use serde::Deserialize;

/// The envelope CV wraps every endpoint response in. `results` is wrapped in
/// `Option` so CV's `"results": null` on error responses (e.g. status_code
/// 101 "Object Not Found") deserializes cleanly; the caller then matches on
/// `status_code` and treats a `None` after status_code = 1 as malformed.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvResponse<T> {
    pub status_code: u16,
    #[serde(default)]
    pub error: String,
    #[serde(default = "Option::default")]
    pub results: Option<T>,
    #[serde(default)]
    pub number_of_total_results: u32,
}

/// CV's image-URL bundle. We project `medium_url` for `cover_url`.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CvImage {
    #[serde(default)]
    pub medium_url: Option<String>,
}

/// CV's "small object reference" — used for `publisher`, `volume`, etc.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvPublisher {
    #[serde(default)]
    pub name: Option<String>,
}

/// Result item from `/search/?resources=volume`. CV returns `start_year` as a
/// quoted string, hence the `Option<String>` rather than an integer.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvVolumeSearchItem {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub start_year: Option<String>,
    #[serde(default)]
    pub publisher: Option<CvPublisher>,
    #[serde(default)]
    pub count_of_issues: u32,
    #[serde(default)]
    pub image: Option<CvImage>,
    #[serde(default)]
    pub deck: Option<String>,
}

/// Full volume detail from `/volume/4050-<id>/`. Same base fields as search
/// plus the long-form `description` and `site_detail_url`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvVolumeFull {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub start_year: Option<String>,
    #[serde(default)]
    pub publisher: Option<CvPublisher>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<CvImage>,
    pub site_detail_url: String,
}

/// Full issue detail from `/issues/?filter=volume:<id>` results.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvIssueFull {
    pub id: i64,
    #[serde(default)]
    pub issue_number: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cover_date: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<CvImage>,
    pub site_detail_url: String,
}

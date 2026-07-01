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

/// CV's volume reference embedded in an issue payload. Unlike
/// [`CvPublisher`] the calendar needs the `id` (to link a release back
/// to a tracked series), so this carries both.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvVolumeRef {
    pub id: i64,
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
    #[serde(default)]
    pub aliases: Option<String>,
}

/// One entry of an issue's `person_credits` array, from
/// `/issue/4000-<id>/?field_list=id,person_credits`. CV packs multiple
/// roles for one person into a single comma-delimited `role` string.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvCreditRaw {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
}

/// The `results` object of a per-issue credits fetch. Only the fields
/// requested via `field_list`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvIssueCreditsRaw {
    #[serde(default)]
    pub person_credits: Vec<CvCreditRaw>,
}

/// One entry of a person's `volume_credits` — a series they're credited on.
/// From `/person/4040-<id>/?field_list=volume_credits`. Only id+name used.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvVolumeCreditRaw {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

/// The `results` object of a person volume-credits fetch (field_list-limited).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvPersonVolumeCreditsRaw {
    #[serde(default)]
    pub volume_credits: Vec<CvVolumeCreditRaw>,
}

/// Full issue detail from `/issues/` results — both the
/// `filter=volume:<id>` (per-volume) and `filter=store_date:<from>|<to>`
/// (release-calendar) queries deserialize into this.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvIssueFull {
    pub id: i64,
    #[serde(default)]
    pub issue_number: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cover_date: Option<String>,
    /// On-sale date. Absent on many older / indie issues; the
    /// release-calendar projection skips rows without it.
    #[serde(default)]
    pub store_date: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<CvImage>,
    /// The owning volume. Always present on a real CV issue payload;
    /// `Option` for deserialization safety.
    #[serde(default)]
    pub volume: Option<CvVolumeRef>,
    pub site_detail_url: String,
}

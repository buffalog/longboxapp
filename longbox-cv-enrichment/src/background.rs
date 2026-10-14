//! The background-lane ComicVine handle.
//!
//! Enrichment, the credits resolver and the catalog refresh call CV
//! through this wrapper. It is a [`ComicVineClient`] pinned to
//! [`Lane::Background`]: every request spends the background bucket in
//! the shared [`longbox_comicvine::CvCoordinator`], so background work
//! can't drain the interactive budget. The coordinator enforces the lane
//! per HTTP request, so a multi-page fetch is paced page by page.

use longbox_comicvine::{
    ComicVineClient, CvCalendarItem, CvCoordinator, CvError, CvIssueDetail, CvPersonCredit,
    CvVolumeDetail, Lane, SeriesSearchResult,
};

/// Cheap to clone; every clone shares the one coordinator.
#[derive(Clone)]
pub struct BackgroundCvClient {
    inner: ComicVineClient,
}

impl BackgroundCvClient {
    /// The background lane of `client` (whatever lane `client` is on).
    pub fn new(client: &ComicVineClient) -> Self {
        Self {
            inner: client.with_lane(Lane::Background),
        }
    }

    /// The same client on [`Lane::Delta`], for the delta refresh.
    pub fn delta(&self) -> Self {
        Self {
            inner: self.inner.with_lane(Lane::Delta),
        }
    }

    pub fn coordinator(&self) -> &CvCoordinator {
        self.inner.coordinator()
    }

    pub async fn search_volumes(&self, query: &str) -> Result<Vec<SeriesSearchResult>, CvError> {
        self.inner.search_volumes(query).await
    }

    pub async fn fetch_volume(&self, cv_volume_id: i64) -> Result<CvVolumeDetail, CvError> {
        self.inner.fetch_volume(cv_volume_id).await
    }

    pub async fn fetch_issues(&self, cv_volume_id: i64) -> Result<Vec<CvIssueDetail>, CvError> {
        self.inner.fetch_issues(cv_volume_id).await
    }

    pub async fn fetch_issue_credits(
        &self,
        cv_issue_id: i64,
    ) -> Result<Vec<CvPersonCredit>, CvError> {
        self.inner.fetch_issue_credits(cv_issue_id).await
    }

    pub async fn fetch_release_calendar(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<CvCalendarItem>, CvError> {
        self.inner.fetch_release_calendar(from, to).await
    }
}

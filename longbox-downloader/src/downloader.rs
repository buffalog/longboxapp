//! The `Downloader` trait + `AnyDownloader` dispatch enum.

use std::future::Future;

use crate::error::DownloaderError;
use crate::nzbget::NzbgetClient;
use crate::sabnzbd::SabnzbdClient;
use crate::types::{DownloadHandle, DownloadStatus, DownloaderAuth, DownloaderConfig};

/// Common surface over the supported Usenet downloaders.
///
/// Declared with explicit `impl Future + Send` return types (rather
/// than `async fn`) so the futures are `Send` — the pull engine
/// (Step 6) runs inside a spawned tokio task. Implementors write
/// ordinary `async fn`.
pub trait Downloader {
    /// Submit an NZB by URL. The downloader fetches the NZB itself;
    /// `name` is a human-readable label for the job.
    fn submit(
        &self,
        nzb_url: &str,
        name: &str,
    ) -> impl Future<Output = Result<DownloadHandle, DownloaderError>> + Send;

    /// Query the status of a previously-submitted download.
    fn status(
        &self,
        handle: &DownloadHandle,
    ) -> impl Future<Output = Result<DownloadStatus, DownloaderError>> + Send;
}

/// The configured downloader — exactly one is active at a time
/// (brief). The pull engine holds this and calls [`Downloader`]
/// methods; the match-dispatch keeps SABnzbd / NZBGet specifics out
/// of the engine.
pub enum AnyDownloader {
    Sabnzbd(SabnzbdClient),
    Nzbget(NzbgetClient),
}

impl Downloader for AnyDownloader {
    async fn submit(&self, nzb_url: &str, name: &str) -> Result<DownloadHandle, DownloaderError> {
        match self {
            AnyDownloader::Sabnzbd(c) => c.submit(nzb_url, name).await,
            AnyDownloader::Nzbget(c) => c.submit(nzb_url, name).await,
        }
    }

    async fn status(&self, handle: &DownloadHandle) -> Result<DownloadStatus, DownloaderError> {
        match self {
            AnyDownloader::Sabnzbd(c) => c.status(handle).await,
            AnyDownloader::Nzbget(c) => c.status(handle).await,
        }
    }
}

/// Construct the active downloader from config. The [`DownloaderAuth`]
/// variant selects the client: `ApiKey` → SABnzbd, `Basic` → NZBGet.
pub fn connect(config: &DownloaderConfig) -> AnyDownloader {
    match &config.auth {
        DownloaderAuth::ApiKey(key) => AnyDownloader::Sabnzbd(SabnzbdClient::new(
            config.base_url.clone(),
            key.clone(),
            config.category.clone(),
        )),
        DownloaderAuth::Basic { username, password } => AnyDownloader::Nzbget(NzbgetClient::new(
            config.base_url.clone(),
            username.clone(),
            password.clone(),
            config.category.clone(),
        )),
    }
}

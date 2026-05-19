//! LongBox Usenet downloader client.
//!
//! Submits NZBs to, and queries status from, the configured Usenet
//! downloader. Built for Phase A.8's auto-pull workflow: the pull
//! engine hands a Newznab release's NZB URL to the downloader, which
//! fetches it; the completed file lands in the watch folder and
//! Phase B catalogs it.
//!
//! Two downloaders behind one [`Downloader`] trait:
//! - **SABnzbd** — REST-ish HTTP API, `apikey` auth.
//! - **NZBGet** — JSON-RPC API, HTTP Basic auth.
//!
//! [`connect`] builds the active downloader from a [`DownloaderConfig`];
//! [`AnyDownloader`] is the dispatch enum the pull engine holds. Each
//! client maps its native status vocabulary onto the downloader-
//! agnostic [`DownloadStatus`].
//!
//! Scope (Phase A.8 Step 2): submit + status only. No torrenting
//! (Usenet only — permanent scope exclusion).

mod downloader;
mod error;
mod nzbget;
mod sabnzbd;
mod types;

pub use downloader::{connect, AnyDownloader, Downloader};
pub use error::DownloaderError;
pub use nzbget::NzbgetClient;
pub use sabnzbd::SabnzbdClient;
pub use types::{DownloadHandle, DownloadStatus, DownloaderAuth, DownloaderConfig};

/// Shared `reqwest::Client` builder. 30 s timeout — downloaders are
/// usually local-network, a slow one shouldn't stall a pull sweep.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        // Fails only on TLS-backend init — a process-level invariant.
        .expect("reqwest client build failed")
}

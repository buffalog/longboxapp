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
use serde::Deserialize;

/// Deserialise a Unix-seconds timestamp that a downloader may report
/// as a number, as a string, as zero, or not at all.
///
/// Everything that is not a positive integer becomes `None`. The point
/// is that a non-timestamp must never reach the caller looking like a
/// timestamp: the pull engine refuses to condemn a download without
/// one, so a `0` flowing through as a usable value would silently
/// disable that guard rather than visibly disable the feature.
///
/// Tolerating a string rather than erroring is deliberate too — a
/// string-typed field would otherwise fail the whole response and
/// break status polling entirely, which is a much worse outcome than
/// one absent timestamp.
fn lenient_unix_secs<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(i64),
        Str(String),
        /// Anything else — a float, a bool, an object. Without this
        /// arm the untagged enum ERRORS on those, which fails the
        /// whole history response, which stops **all** status polling:
        /// `Failed` and `Unknown` handling included, leaving every
        /// attempt stuck `submitted` indefinitely. A total-outage
        /// blast radius for one unexpected JSON type, in a function
        /// whose entire purpose is to be total at a trust boundary.
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<Raw>::deserialize(deserializer)? {
        Some(Raw::Num(n)) => (n > 0).then_some(n),
        Some(Raw::Str(s)) => s.trim().parse::<i64>().ok().filter(|n| *n > 0),
        Some(Raw::Other(_)) | None => None,
    })
}

pub use types::{DownloadHandle, DownloadStatus, DownloaderAuth, DownloaderConfig, RemoteStorage};

/// Shared `reqwest::Client` builder. 30 s timeout — downloaders are
/// usually local-network, a slow one shouldn't stall a pull sweep.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        // Fails only on TLS-backend init — a process-level invariant.
        .expect("reqwest client build failed")
}

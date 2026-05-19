//! Shared types for the downloader clients.

/// How a downloader authenticates. The variant doubles as the
/// downloader-kind discriminant for v1's two downloaders: SABnzbd uses
/// an `apikey` query param, NZBGet uses HTTP Basic auth — there is no
/// NZBGet "api key". [`connect`](crate::connect) picks the client from
/// the variant.
#[derive(Debug, Clone)]
pub enum DownloaderAuth {
    /// SABnzbd — the `apikey`.
    ApiKey(String),
    /// NZBGet — ControlUsername / ControlPassword.
    Basic { username: String, password: String },
}

/// Connection config for the configured downloader. `longbox-downloader`
/// owns this input type; Step 3's `downloader_config` row converts into
/// it (the row's `type` column selects the [`DownloaderAuth`] variant).
#[derive(Debug, Clone)]
pub struct DownloaderConfig {
    /// Base URL, e.g. `http://localhost:8080` (SAB) or
    /// `http://localhost:6789` (NZBGet). Endpoint paths are appended
    /// by the client.
    pub base_url: String,
    pub auth: DownloaderAuth,
    /// Category submitted with every NZB (`cat` for SAB, `Category`
    /// for NZBGet). Empty string = the downloader's default.
    pub category: String,
}

/// Identifies a submitted download for later status queries. Wraps
/// SABnzbd's `nzo_id` (a string) and NZBGet's `NZBID` (an integer,
/// stringified) behind one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadHandle(pub String);

/// Downloader-agnostic status of a submitted download. Each client
/// maps its own native status vocabulary onto this — the pull engine
/// never sees SABnzbd / NZBGet specifics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStatus {
    /// Accepted, not yet downloading.
    Queued,
    /// Actively downloading or post-processing (par-check, unpack).
    Downloading,
    /// Finished — the file should be landing in the watch folder.
    Completed,
    /// The downloader gave up. Carries its reason where one is
    /// available (SAB `fail_message`, NZBGet `FAILURE/*` status).
    Failed(String),
    /// The handle isn't in the active queue or the history — either
    /// never accepted, or aged out of history.
    Unknown,
}

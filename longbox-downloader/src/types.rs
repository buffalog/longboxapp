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

/// Where a downloader says it put a completed job's output.
///
/// This is the downloader's **own** path, in the downloader's
/// filesystem namespace — not ours. SABnzbd reports e.g.
/// `/Users/jeremy/Downloads/complete/The Amazing Spider-Man 25.1`
/// while LongBox, running in a container, sees that same directory
/// mounted at `/watch`. The full path is therefore not resolvable
/// here.
///
/// The failure mode this type exists to prevent is deploy-dependent
/// and therefore invisible in development: on a bare-metal install the
/// downloader's path *would* resolve, so code that opens it directly
/// works locally and silently finds nothing under Docker. So the raw
/// string is private and only [`basename`](Self::basename) is
/// reachable — callers join that onto their own watch root.
///
/// Reading the basename the downloader actually reports is also the
/// only way to get de-duplicated job folders right: SAB renames a
/// second job with a colliding name to `<name>.1`, which no
/// reconstruction from the original job name can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStorage(String);

impl RemoteStorage {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Final path component, or `None` when the string carries no
    /// usable name (empty, all separators, or a `.`/`..` tail).
    /// Splits on both separators: the downloader may run on Windows
    /// even when LongBox does not.
    pub fn basename(&self) -> Option<&str> {
        let trimmed = self.0.trim_end_matches(['/', '\\']);
        let name = trimmed.rsplit(['/', '\\']).next()?;
        if name.is_empty() || name == "." || name == ".." {
            None
        } else {
            Some(name)
        }
    }
}

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
    ///
    /// `storage` is the downloader's record of where it put the
    /// output, when it reports one. `None` means the downloader did
    /// not tell us; consumers must degrade to their previous
    /// behaviour rather than guess a location.
    ///
    /// `completed_at` is the downloader's own completion time, Unix
    /// seconds. A job folder is named from a namespace the downloader
    /// re-allocates once the previous folder is gone, so the recorded
    /// path outlives the directory it named; this timestamp is what
    /// lets a consumer tell that the directory now sitting at that
    /// path belongs to a *later* job.
    ///
    /// Absent, zero, or unparseable becomes `None` at the
    /// deserialisation boundary — a value that is not a timestamp must
    /// never flow through looking like one, the same rule
    /// [`RemoteStorage`] applies to paths.
    Completed {
        storage: Option<RemoteStorage>,
        completed_at: Option<i64>,
    },
    /// The downloader gave up. Carries its reason where one is
    /// available (SAB `fail_message`, NZBGet `FAILURE/*` status).
    Failed(String),
    /// The handle isn't in the active queue or the history — either
    /// never accepted, or aged out of history.
    Unknown,
}

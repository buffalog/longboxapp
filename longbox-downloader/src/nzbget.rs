//! NZBGet JSON-RPC API client.
//!
//! All calls POST to `{base_url}/jsonrpc` with HTTP Basic auth. Submit
//! uses the `append` method (Content = NZB URL); status checks
//! `listgroups` then `history`.
//!
//! NZBGet's `SUCCESS/* | FAILURE/* | WARNING/*` history vocabulary is
//! mapped to the common [`DownloadStatus`] *inside this client* — the
//! pull engine never sees NZBGet-specific status strings. `WARNING/*`
//! (download OK, post-script or health warning) maps to `Completed`:
//! a file did land.

use serde::Deserialize;
use serde_json::json;

use crate::downloader::Downloader;
use crate::error::DownloaderError;
use crate::types::{DownloadHandle, DownloadStatus, RemoteStorage};

pub struct NzbgetClient {
    base_url: String,
    username: String,
    password: String,
    category: String,
    http: reqwest::Client,
}

impl NzbgetClient {
    pub fn new(base_url: String, username: String, password: String, category: String) -> Self {
        Self {
            base_url,
            username,
            password,
            category,
            http: crate::http_client(),
        }
    }

    /// Issue one JSON-RPC call. Returns the `result` value. HTTP 401 →
    /// `AuthFailed`; a JSON-RPC `error` envelope → `ApiError`.
    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, DownloaderError> {
        let url = format!("{}/jsonrpc", self.base_url.trim_end_matches('/'));
        let req = json!({ "method": method, "params": params, "id": 1 });

        let resp = self
            .http
            .post(&url)
            .basic_auth(&self.username, Some(&self.password))
            .json(&req)
            .send()
            .await
            .map_err(|e| DownloaderError::HttpFailure(format!("NZBGet request failed: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DownloaderError::AuthFailed(
                "NZBGet rejected the Basic-auth credentials".into(),
            ));
        }
        if !status.is_success() {
            return Err(DownloaderError::HttpFailure(format!(
                "NZBGet returned HTTP {status}"
            )));
        }

        let envelope: NzbgetEnvelope = resp
            .json()
            .await
            .map_err(|e| DownloaderError::MalformedResponse(format!("NZBGet JSON-RPC: {e}")))?;
        if let Some(err) = envelope.error {
            return Err(DownloaderError::ApiError(format!(
                "NZBGet JSON-RPC error {}: {}",
                err.code, err.message
            )));
        }
        envelope
            .result
            .ok_or_else(|| DownloaderError::MalformedResponse("NZBGet JSON-RPC: no result".into()))
    }
}

impl Downloader for NzbgetClient {
    async fn submit(&self, nzb_url: &str, name: &str) -> Result<DownloadHandle, DownloaderError> {
        // `append` positional params (current NZBGet): Filename,
        // Content, Category, Priority, AddToTop, AddPaused, DupeKey,
        // DupeScore, DupeMode, AutoCategory, PPParameters. Content is
        // the NZB URL — NZBGet fetches it and reads the real filename
        // from the HTTP headers.
        let params = json!([
            name,          // Filename — display label
            nzb_url,       // Content — URL
            self.category, // Category
            0,             // Priority — normal
            false,         // AddToTop
            false,         // AddPaused
            "",            // DupeKey
            0,             // DupeScore
            "SCORE",       // DupeMode
            false,         // AutoCategory
            [],            // PPParameters
        ]);
        let result = self.rpc("append", params).await?;
        let nzbid = result.as_i64().ok_or_else(|| {
            DownloaderError::MalformedResponse("NZBGet append: result is not an integer".into())
        })?;
        // append returns a positive NZBID on success; 0 / negative is
        // an error code.
        if nzbid <= 0 {
            return Err(DownloaderError::ApiError(format!(
                "NZBGet append rejected the NZB (returned {nzbid})"
            )));
        }
        Ok(DownloadHandle(nzbid.to_string()))
    }

    async fn status(&self, handle: &DownloadHandle) -> Result<DownloadStatus, DownloaderError> {
        let target: i64 = handle.0.parse().map_err(|_| {
            DownloaderError::MalformedResponse(format!(
                "handle {:?} is not an NZBGet NZBID",
                handle.0
            ))
        })?;

        // Active queue first.
        let groups_raw = self.rpc("listgroups", json!([0])).await?;
        let groups: Vec<NzbgetGroup> = serde_json::from_value(groups_raw)
            .map_err(|e| DownloaderError::MalformedResponse(format!("NZBGet listgroups: {e}")))?;
        if let Some(g) = groups.iter().find(|g| g.nzbid == target) {
            return Ok(map_group_status(&g.status));
        }

        // Then history.
        let history_raw = self.rpc("history", json!([false])).await?;
        let history: Vec<NzbgetHistoryItem> = serde_json::from_value(history_raw)
            .map_err(|e| DownloaderError::MalformedResponse(format!("NZBGet history: {e}")))?;
        if let Some(h) = history.iter().find(|h| h.nzbid == target) {
            return Ok(map_history_status(
                &h.status,
                &h.final_dir,
                &h.dest_dir,
                h.history_time,
            ));
        }

        Ok(DownloadStatus::Unknown)
    }

    async fn test_connection(&self) -> Result<(), DownloaderError> {
        // Every NZBGet JSON-RPC call requires Basic auth; `version` is
        // the lightest, and a 401 surfaces as AuthFailed.
        self.rpc("version", json!([])).await.map(|_| ())
    }
}

/// listgroups `Status` → common status. `QUEUED`/`PAUSED` are waiting;
/// everything else (DOWNLOADING, FETCHING, POST-PROCESSING, …) is
/// in-progress.
fn map_group_status(raw: &str) -> DownloadStatus {
    match raw {
        "QUEUED" | "PAUSED" => DownloadStatus::Queued,
        _ => DownloadStatus::Downloading,
    }
}

/// history `Status` → common status. NZBGet status strings are
/// `<BAND>/<DETAIL>`: `SUCCESS/*` and `WARNING/*` both mean a file
/// landed (warnings are post-script / health notes, not download
/// failure); anything else (`FAILURE/*`, `DELETED/*`) is a failure.
///
/// Output location, per NZBGet's documented history fields:
/// `FinalDir` is "final destination if set by one of post-processing
/// scripts" and `DestDir` is "destination directory for output
/// files", so `FinalDir` wins when a pp-script relocated the job and
/// `DestDir` is the fallback. Both empty → `None`, which degrades the
/// caller to its prior behaviour rather than guessing a path.
///
/// **Untested against a live NZBGet.** This project runs SABnzbd; the
/// field names and their precedence come from the documented RPC
/// surface, not from an observed response. The `None` fallback is the
/// safe failure mode if either field turns out to be absent in
/// practice.
fn map_history_status(
    raw: &str,
    final_dir: &str,
    dest_dir: &str,
    history_time: Option<i64>,
) -> DownloadStatus {
    if raw.starts_with("SUCCESS") || raw.starts_with("WARNING") {
        let location = [final_dir, dest_dir]
            .into_iter()
            .map(str::trim)
            .find(|d| !d.is_empty());
        DownloadStatus::Completed {
            storage: location.map(RemoteStorage::new),
            completed_at: history_time,
        }
    } else {
        DownloadStatus::Failed(raw.to_string())
    }
}

#[derive(Deserialize)]
struct NzbgetEnvelope {
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<NzbgetRpcError>,
}

#[derive(Deserialize)]
struct NzbgetRpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct NzbgetGroup {
    #[serde(rename = "NZBID")]
    nzbid: i64,
    #[serde(rename = "Status")]
    status: String,
}

#[derive(Deserialize)]
struct NzbgetHistoryItem {
    #[serde(rename = "NZBID")]
    nzbid: i64,
    #[serde(rename = "Status")]
    status: String,
    /// See [`map_history_status`] for the precedence rule and the
    /// note that this backend is untested here.
    #[serde(rename = "FinalDir", default)]
    final_dir: String,
    #[serde(rename = "DestDir", default)]
    dest_dir: String,
    /// `HistoryTime` — "Date/time when the file was added to history
    /// (Time is in C/Unix format)" per NZBGet's documented history
    /// fields. Nearest equivalent to SABnzbd's `completed`. Same
    /// untested caveat as the directory fields; 0 reads as unknown,
    /// which degrades the caller safely.
    #[serde(
        rename = "HistoryTime",
        default,
        deserialize_with = "crate::lenient_unix_secs"
    )]
    history_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_connection_ok_when_version_responds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/jsonrpc"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"result":"24.3","error":null}"#),
            )
            .mount(&server)
            .await;
        let client = NzbgetClient::new(server.uri(), "user".into(), "pass".into(), String::new());
        assert!(client.test_connection().await.is_ok());
    }

    #[tokio::test]
    async fn test_connection_rejects_bad_basic_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/jsonrpc"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = NzbgetClient::new(server.uri(), "user".into(), "bad".into(), String::new());
        assert!(matches!(
            client.test_connection().await,
            Err(DownloaderError::AuthFailed(_))
        ));
    }

    #[test]
    fn group_status_mapping() {
        assert_eq!(map_group_status("QUEUED"), DownloadStatus::Queued);
        assert_eq!(map_group_status("PAUSED"), DownloadStatus::Queued);
        assert_eq!(map_group_status("DOWNLOADING"), DownloadStatus::Downloading);
        assert_eq!(
            map_group_status("POST-PROCESSING"),
            DownloadStatus::Downloading
        );
    }

    #[test]
    fn history_success_and_warning_both_complete() {
        for raw in ["SUCCESS/ALL", "SUCCESS/HEALTH"] {
            assert!(matches!(
                map_history_status(raw, "", "/done/job", Some(1700000000)),
                DownloadStatus::Completed { .. }
            ));
        }
        // WARNING band: a file landed — mapped to Completed inside the
        // client so the pull engine never sees NZBGet specifics.
        for raw in ["WARNING/SCRIPT", "WARNING/HEALTH"] {
            assert!(matches!(
                map_history_status(raw, "", "/done/job", Some(1700000000)),
                DownloadStatus::Completed { .. }
            ));
        }
    }

    #[test]
    fn final_dir_wins_over_dest_dir_and_absent_dirs_report_no_location() {
        // Documented precedence: FinalDir is set by a post-processing
        // script that relocated the job, so it describes where the
        // files actually are.
        let with_final =
            map_history_status("SUCCESS/ALL", "/final/job", "/dest/job", Some(1700000000));
        let DownloadStatus::Completed { storage, .. } = with_final else {
            panic!("expected Completed");
        };
        assert_eq!(storage.expect("location").basename(), Some("job"));

        // DestDir is the fallback when no script moved anything.
        let dest_only = map_history_status("SUCCESS/ALL", "", "/dest/elsewhere", Some(1700000000));
        let DownloadStatus::Completed { storage, .. } = dest_only else {
            panic!("expected Completed");
        };
        assert_eq!(storage.expect("location").basename(), Some("elsewhere"));

        // Neither reported → None, so the caller degrades to its prior
        // behaviour rather than guessing a path.
        let neither = map_history_status("SUCCESS/ALL", "", "   ", Some(1700000000));
        assert_eq!(
            neither,
            DownloadStatus::Completed {
                storage: None,
                completed_at: Some(1700000000)
            }
        );
    }

    #[test]
    fn history_failure_and_deleted_are_failed() {
        assert!(matches!(
            map_history_status("FAILURE/PAR", "", "", Some(1700000000)),
            DownloadStatus::Failed(s) if s == "FAILURE/PAR"
        ));
        assert!(matches!(
            map_history_status("FAILURE/UNPACK", "", "", Some(1700000000)),
            DownloadStatus::Failed(_)
        ));
        assert!(matches!(
            map_history_status("DELETED/MANUAL", "", "", Some(1700000000)),
            DownloadStatus::Failed(_)
        ));
    }
}

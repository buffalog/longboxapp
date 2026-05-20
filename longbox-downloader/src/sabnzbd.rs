//! SABnzbd HTTP API client.
//!
//! All requests hit `{base_url}/api` with the `apikey` query param and
//! `output=json`. Submit uses `mode=addurl`; status checks `mode=queue`
//! then `mode=history`.

use serde::Deserialize;

use crate::downloader::Downloader;
use crate::error::DownloaderError;
use crate::types::{DownloadHandle, DownloadStatus};

pub struct SabnzbdClient {
    base_url: String,
    api_key: String,
    category: String,
    http: reqwest::Client,
}

impl SabnzbdClient {
    pub fn new(base_url: String, api_key: String, category: String) -> Self {
        Self {
            base_url,
            api_key,
            category,
            http: crate::http_client(),
        }
    }

    /// GET `{base}/api` with `apikey` + `output=json` appended. Returns
    /// the raw body. Surfaces SABnzbd's plain-text auth errors (`error:
    /// API Key Incorrect`) — those are NOT JSON.
    async fn get(&self, params: &[(&str, &str)]) -> Result<String, DownloaderError> {
        let url = format!("{}/api", self.base_url.trim_end_matches('/'));
        let mut all: Vec<(&str, &str)> = params.to_vec();
        all.push(("apikey", &self.api_key));
        all.push(("output", "json"));

        let resp =
            self.http.get(&url).query(&all).send().await.map_err(|e| {
                DownloaderError::HttpFailure(format!("SABnzbd request failed: {e}"))
            })?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| DownloaderError::HttpFailure(format!("SABnzbd body read failed: {e}")))?;

        let trimmed = body.trim();
        // SABnzbd reports bad/missing api keys as a plain-text body,
        // not JSON.
        if let Some(rest) = trimmed.strip_prefix("error:") {
            let msg = rest.trim().to_string();
            return Err(if msg.to_lowercase().contains("api key") {
                DownloaderError::AuthFailed(msg)
            } else {
                DownloaderError::ApiError(msg)
            });
        }
        if !status.is_success() {
            return Err(DownloaderError::HttpFailure(format!(
                "SABnzbd returned HTTP {status}"
            )));
        }
        Ok(body)
    }
}

impl Downloader for SabnzbdClient {
    async fn submit(&self, nzb_url: &str, name: &str) -> Result<DownloadHandle, DownloaderError> {
        let body = self
            .get(&[
                ("mode", "addurl"),
                ("name", nzb_url),
                ("nzbname", name),
                ("cat", &self.category),
            ])
            .await?;
        let parsed: SabAddUrlResponse = serde_json::from_str(body.trim())
            .map_err(|e| DownloaderError::MalformedResponse(format!("SABnzbd addurl: {e}")))?;
        if !parsed.status {
            return Err(DownloaderError::ApiError(
                parsed
                    .error
                    .unwrap_or_else(|| "unknown SABnzbd error".into()),
            ));
        }
        parsed
            .nzo_ids
            .into_iter()
            .flatten()
            .next()
            .map(DownloadHandle)
            .ok_or_else(|| {
                DownloaderError::MalformedResponse("SABnzbd addurl returned no nzo_ids".into())
            })
    }

    async fn status(&self, handle: &DownloadHandle) -> Result<DownloadStatus, DownloaderError> {
        // Active queue first — a job lives there until it completes.
        let queue_body = self.get(&[("mode", "queue")]).await?;
        let queue: SabQueueResponse = serde_json::from_str(queue_body.trim())
            .map_err(|e| DownloaderError::MalformedResponse(format!("SABnzbd queue: {e}")))?;
        if let Some(slot) = queue.queue.slots.iter().find(|s| s.nzo_id == handle.0) {
            return Ok(map_queue_status(&slot.status));
        }

        // Then history — completed / failed / mid-post-processing.
        let history_body = self.get(&[("mode", "history"), ("limit", "200")]).await?;
        let history: SabHistoryResponse = serde_json::from_str(history_body.trim())
            .map_err(|e| DownloaderError::MalformedResponse(format!("SABnzbd history: {e}")))?;
        if let Some(slot) = history.history.slots.iter().find(|s| s.nzo_id == handle.0) {
            return Ok(map_history_status(&slot.status, &slot.fail_message));
        }

        Ok(DownloadStatus::Unknown)
    }

    async fn test_connection(&self) -> Result<(), DownloaderError> {
        // `mode=queue` requires the apikey (unlike the public
        // `mode=version`), so it verifies auth, not just reachability.
        // `limit=0` keeps the response body tiny.
        self.get(&[("mode", "queue"), ("limit", "0")])
            .await
            .map(|_| ())
    }
}

/// Active-queue status → common status. Anything in the queue that
/// isn't explicitly waiting is treated as in-progress.
fn map_queue_status(raw: &str) -> DownloadStatus {
    match raw {
        "Queued" | "Paused" => DownloadStatus::Queued,
        _ => DownloadStatus::Downloading,
    }
}

/// History status → common status. `Completed`/`Failed` are terminal;
/// everything else (Verifying, Repairing, Extracting, …) is still
/// post-processing.
fn map_history_status(raw: &str, fail_message: &str) -> DownloadStatus {
    match raw {
        "Completed" => DownloadStatus::Completed,
        "Failed" => DownloadStatus::Failed(if fail_message.is_empty() {
            "SABnzbd reported a failure".into()
        } else {
            fail_message.to_string()
        }),
        _ => DownloadStatus::Downloading,
    }
}

#[derive(Deserialize)]
struct SabAddUrlResponse {
    status: bool,
    #[serde(default)]
    nzo_ids: Option<Vec<String>>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct SabQueueResponse {
    queue: SabQueue,
}

#[derive(Deserialize)]
struct SabQueue {
    #[serde(default)]
    slots: Vec<SabQueueSlot>,
}

#[derive(Deserialize)]
struct SabQueueSlot {
    nzo_id: String,
    status: String,
}

#[derive(Deserialize)]
struct SabHistoryResponse {
    history: SabHistory,
}

#[derive(Deserialize)]
struct SabHistory {
    #[serde(default)]
    slots: Vec<SabHistorySlot>,
}

#[derive(Deserialize)]
struct SabHistorySlot {
    nzo_id: String,
    status: String,
    #[serde(default)]
    fail_message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_connection_ok_when_queue_responds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "queue"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue":{"slots":[]}}"#))
            .mount(&server)
            .await;
        let client = SabnzbdClient::new(server.uri(), "KEY".into(), String::new());
        assert!(client.test_connection().await.is_ok());
    }

    #[tokio::test]
    async fn test_connection_rejects_bad_api_key() {
        let server = MockServer::start().await;
        // SABnzbd reports a bad apikey as a plain-text body, not JSON.
        Mock::given(method("GET"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_string("error: API Key Incorrect"))
            .mount(&server)
            .await;
        let client = SabnzbdClient::new(server.uri(), "BAD".into(), String::new());
        assert!(matches!(
            client.test_connection().await,
            Err(DownloaderError::AuthFailed(_))
        ));
    }

    #[test]
    fn queue_status_mapping() {
        assert_eq!(map_queue_status("Queued"), DownloadStatus::Queued);
        assert_eq!(map_queue_status("Paused"), DownloadStatus::Queued);
        assert_eq!(map_queue_status("Downloading"), DownloadStatus::Downloading);
        assert_eq!(map_queue_status("Fetching"), DownloadStatus::Downloading);
        assert_eq!(map_queue_status("Propagating"), DownloadStatus::Downloading);
    }

    #[test]
    fn history_status_mapping() {
        assert_eq!(
            map_history_status("Completed", ""),
            DownloadStatus::Completed
        );
        assert_eq!(
            map_history_status("Failed", "par2 verification failed"),
            DownloadStatus::Failed("par2 verification failed".into())
        );
        // Failed with no message still carries a reason.
        assert!(matches!(
            map_history_status("Failed", ""),
            DownloadStatus::Failed(_)
        ));
        // Mid-post-processing states are still in-progress.
        assert_eq!(
            map_history_status("Repairing", ""),
            DownloadStatus::Downloading
        );
        assert_eq!(
            map_history_status("Extracting", ""),
            DownloadStatus::Downloading
        );
    }
}

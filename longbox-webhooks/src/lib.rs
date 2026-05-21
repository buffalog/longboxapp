//! LongBox webhook delivery — POST a [`WebhookEvent`] to a configured
//! URL. Slack hosts (`hooks.slack.com`) receive a block-kit body; every
//! other host receives a plain `{ event, message }` JSON object.
//!
//! A pure delivery client: it knows nothing about the database or which
//! webhooks subscribe to what. The caller (the dispatch layer in
//! `longbox-pull`) decides who to deliver to; this crate just formats
//! and POSTs, with a small count-based retry.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

/// Slack incoming-webhook host — a URL here gets a block-kit body.
const SLACK_HOST: &str = "hooks.slack.com";
/// Total POST attempts before giving up. Count-based, not time-based: a
/// webhook endpoint that stays down simply misses the event — there is
/// no persistent retry queue.
const MAX_ATTEMPTS: u32 = 3;
/// Fixed delay between retry attempts.
const RETRY_BACKOFF: Duration = Duration::from_millis(500);
/// Per-request timeout, so a hung endpoint can't stall delivery.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum WebhookError {
    #[error("invalid webhook URL: {0}")]
    InvalidUrl(String),
    #[error("delivery failed after {attempts} attempt(s): {last}")]
    DeliveryFailed { attempts: u32, last: String },
}

/// One notification to deliver.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookEvent {
    /// Stable event identifier — `pull_failed`, `pull_engine_error`,
    /// `test`. Lets a generic JSON consumer branch on the event kind.
    pub event: String,
    /// Human-readable one-line summary — the body of a Slack message.
    pub message: String,
}

/// Deliver `event` to `url`. Returns `Ok` on the first 2xx response;
/// retries up to [`MAX_ATTEMPTS`] on a network error or non-2xx status,
/// then returns [`WebhookError::DeliveryFailed`].
pub async fn deliver(url: &str, event: &WebhookEvent) -> Result<(), WebhookError> {
    let body = body_for(url, event)?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut last = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        match client.post(url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => last = format!("HTTP {}", resp.status().as_u16()),
            Err(e) => last = e.to_string(),
        }
        if attempt < MAX_ATTEMPTS {
            tokio::time::sleep(RETRY_BACKOFF).await;
        }
    }
    tracing::warn!(target: "longbox_webhooks", url, error = %last, "webhook.delivery_failed");
    Err(WebhookError::DeliveryFailed {
        attempts: MAX_ATTEMPTS,
        last,
    })
}

/// The JSON body for `url` — Slack block-kit when the host matches,
/// plain `{ event, message }` otherwise.
fn body_for(url: &str, event: &WebhookEvent) -> Result<Value, WebhookError> {
    let parsed = url::Url::parse(url).map_err(|e| WebhookError::InvalidUrl(e.to_string()))?;
    Ok(if parsed.host_str() == Some(SLACK_HOST) {
        slack_body(event)
    } else {
        generic_body(event)
    })
}

fn generic_body(event: &WebhookEvent) -> Value {
    json!({ "event": event.event, "message": event.message })
}

fn slack_body(event: &WebhookEvent) -> Value {
    json!({
        "blocks": [
            { "type": "section", "text": { "type": "mrkdwn", "text": event.message } }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> WebhookEvent {
        WebhookEvent {
            event: "pull_failed".into(),
            message: "Pull failed: Saga".into(),
        }
    }

    #[test]
    fn slack_host_gets_a_block_kit_body() {
        let body = body_for("https://hooks.slack.com/services/T/B/x", &ev()).unwrap();
        assert_eq!(body["blocks"][0]["type"], "section");
        assert_eq!(body["blocks"][0]["text"]["text"], "Pull failed: Saga");
    }

    #[test]
    fn non_slack_host_gets_a_generic_body() {
        let body = body_for("https://example.com/hook", &ev()).unwrap();
        assert_eq!(body["event"], "pull_failed");
        assert_eq!(body["message"], "Pull failed: Saga");
        assert!(body.get("blocks").is_none());
    }

    #[test]
    fn a_bad_url_is_an_error() {
        assert!(matches!(
            body_for("not a url", &ev()),
            Err(WebhookError::InvalidUrl(_))
        ));
    }
}

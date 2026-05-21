//! Integration tests for `longbox_webhooks::deliver` against a wiremock
//! HTTP server. The Slack-vs-generic *body* selection is unit-tested in
//! the crate (a wiremock URL's host is never `hooks.slack.com`); these
//! tests cover the actual POST and the retry policy.

use longbox_webhooks::{deliver, WebhookError, WebhookEvent};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn event() -> WebhookEvent {
    WebhookEvent {
        event: "test".into(),
        message: "hello".into(),
    }
}

#[tokio::test]
async fn delivers_a_generic_json_body_on_2xx() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_json(
            serde_json::json!({ "event": "test", "message": "hello" }),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    deliver(&format!("{}/hook", server.uri()), &event())
        .await
        .expect("delivery succeeds on a 200");
}

#[tokio::test]
async fn retries_then_gives_up_on_persistent_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(500))
        .expect(3) // MAX_ATTEMPTS — every attempt hits the server.
        .mount(&server)
        .await;

    let err = deliver(&format!("{}/hook", server.uri()), &event())
        .await
        .expect_err("a persistent 500 exhausts the retries");
    assert!(matches!(
        err,
        WebhookError::DeliveryFailed { attempts: 3, .. }
    ));
}

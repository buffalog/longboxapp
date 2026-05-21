//! Webhook dispatch — fan a pull-engine event out to every enabled
//! webhook subscribed to it.
//!
//! [`dispatch`] is fire-and-forget: it spawns the query + delivery so a
//! slow or dead webhook endpoint never blocks a sweep. The awaitable
//! core is [`dispatch_inner`] (the unit of test). A delivery failure is
//! logged and dropped — a missed notification must never affect the
//! pull workflow.

use longbox_db::{webhook_config_repo, Pool};
use longbox_webhooks::{deliver, WebhookEvent};

/// Fan `event` out to subscribers of `event_bit`, spawned fire-and-forget.
pub(crate) fn dispatch(db: Pool, event_bit: i64, event: WebhookEvent) {
    tokio::spawn(async move {
        dispatch_inner(&db, event_bit, &event).await;
    });
}

/// Query the enabled webhooks subscribed to `event_bit` and deliver
/// `event` to each. Delivery failures are logged, never propagated.
pub(crate) async fn dispatch_inner(db: &Pool, event_bit: i64, event: &WebhookEvent) {
    let subscribers = match webhook_config_repo::list_subscribed(db, event_bit).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(target: "longbox_pull", error = %e, "webhook.dispatch_query_failed");
            return;
        }
    };
    for wh in subscribers {
        if let Err(e) = deliver(&wh.url, event).await {
            tracing::warn!(
                target: "longbox_pull",
                webhook = %wh.name,
                error = %e,
                "webhook.delivery_failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use longbox_db::NewWebhookConfig;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn dispatch_inner_delivers_only_to_enabled_subscribers() {
        let pool = longbox_db::open(":memory:").await.unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let url = format!("{}/hook", server.uri());

        // Enabled + subscribed to the event — should receive.
        webhook_config_repo::insert(
            &pool,
            NewWebhookConfig {
                name: "subscribed".into(),
                url: url.clone(),
                event_mask: webhook_config_repo::EVENT_PULL_FAILED,
                enabled: true,
            },
        )
        .await
        .unwrap();
        // Enabled but subscribed to a *different* event — should not.
        webhook_config_repo::insert(
            &pool,
            NewWebhookConfig {
                name: "other-event".into(),
                url: url.clone(),
                event_mask: webhook_config_repo::EVENT_PULL_ENGINE_ERROR,
                enabled: true,
            },
        )
        .await
        .unwrap();
        // Subscribed to the event but disabled — should not.
        webhook_config_repo::insert(
            &pool,
            NewWebhookConfig {
                name: "disabled".into(),
                url: url.clone(),
                event_mask: webhook_config_repo::EVENT_PULL_FAILED,
                enabled: false,
            },
        )
        .await
        .unwrap();

        let event = WebhookEvent {
            event: "pull_failed".into(),
            message: "x".into(),
        };
        dispatch_inner(&pool, webhook_config_repo::EVENT_PULL_FAILED, &event).await;

        // Only the one enabled, event-matching webhook was POSTed to.
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
    }
}

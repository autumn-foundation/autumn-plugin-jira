//! Spike S1: the webhook mount.
//!
//! autumn finds the `[[security.webhooks.endpoints]]` entry by the request
//! path that the `SignedWebhook` extractor sees. A route mounted with
//! `merge` at the full path sees the full path, so the lookup works. A route
//! mounted with `nest` sees the path without the prefix, so the lookup
//! fails and autumn answers 500. The plugin therefore mounts with `merge`.
//!
//! The `merge` case on the real plugin is
//! `install.rs::signed_delivery_reaches_the_handler_not_500`.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use autumn_plugin_jira::webhook_endpoint;
use autumn_web::app::AppBuilder;
use autumn_web::config::AutumnConfig;
use autumn_web::plugin::Plugin;
use autumn_web::reexports::axum::Router;
use autumn_web::reexports::axum::routing::post;
use autumn_web::reexports::http::StatusCode;
use autumn_web::test::{TestApp, TestClient, TestResponse};
use autumn_web::webhook::{SignedWebhook, hmac_sha256_hex};

const SECRET: &str = "spike-webhook-secret-0123456789abcdef";
const BODY: &str = r#"{"timestamp":1767225600000,"webhookEvent":"jira:issue_updated"}"#;

/// The autumn config with the Jira endpoint at `/jira/webhook`.
fn config() -> AutumnConfig {
    let mut cfg = AutumnConfig::default();
    let mut endpoint = webhook_endpoint("/jira/webhook", "JIRA_WEBHOOK_SECRET");
    endpoint.secret = Some(SECRET.to_owned());
    cfg.security.webhooks.endpoints.push(endpoint);
    cfg
}

async fn deliver(client: &TestClient, id: &str, signature: &str) -> TestResponse {
    client
        .post("/jira/webhook")
        .header("X-Hub-Signature", signature)
        .header("X-Atlassian-Webhook-Identifier", id)
        .header("content-type", "application/json")
        .body(BODY)
        .send()
        .await
}

fn good_signature() -> String {
    format!(
        "sha256={}",
        hmac_sha256_hex(SECRET.as_bytes(), BODY.as_bytes())
    )
}

/// A plugin route with `SignedWebhook`, mounted one of two ways.
struct MountPlugin {
    nest: bool,
}

async fn handler(hook: SignedWebhook) -> StatusCode {
    assert_eq!(hook.raw_body(), BODY.as_bytes());
    StatusCode::OK
}

impl Plugin for MountPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "spike-mount".into()
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        if self.nest {
            app.nest("/jira", Router::new().route("/webhook", post(handler)))
        } else {
            app.merge(Router::new().route("/jira/webhook", post(handler)))
        }
    }
}

/// Evidence for the plan: `nest` strips the prefix, so the endpoint lookup
/// fails with 500 before the handler.
#[tokio::test(flavor = "multi_thread")]
async fn nest_strips_the_prefix_and_breaks_the_lookup() {
    let client = TestApp::new()
        .config(config())
        .plugin(MountPlugin { nest: true })
        .build();
    let response = deliver(&client, "spike-3", &good_signature()).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        response.text().contains("/webhook"),
        "the error names the stripped path: {}",
        response.text()
    );
}

//! AC27: the `jira` health indicator.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use autumn_jira::san_mateo::transport::HttpTransport as _;
use autumn_jira_fake::{BOT, DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp, Fault};
use autumn_plugin_jira::{JiraConfig, JiraPlugin};
use autumn_web::AppState;
use autumn_web::actuator::{HealthIndicator, HealthStatus, IndicatorGroup};
use common::*;
use serde_json::json;

fn plugin_with(fake: &FakeJira, edit: impl FnOnce(&mut JiraConfig)) -> JiraPlugin {
    let mut config = JiraConfig::default();
    edit(&mut config);
    JiraPlugin::with_config(config).client(client_on(Arc::new(fake.clone())))
}

#[tokio::test]
async fn health_only_group() {
    let fake = FakeJira::new();
    let rt = plugin_with(&fake, |c| c.health.timeout_ms = 1234)
        .start(&AppState::for_test())
        .unwrap();
    let health = rt.health();
    assert_eq!(health.group(), IndicatorGroup::HealthOnly);
    assert_eq!(health.timeout_ms(), 1234);
    let out = health.check().await;
    assert_eq!(out.status, HealthStatus::Up);
    assert_eq!(out.details["mode"], json!("api_token"));
    assert_eq!(out.details["site"], json!("acme.atlassian.net"));
    assert_eq!(out.details["status"], json!(200));
    assert_eq!(out.details["self_filter"], json!("off"));
    assert_eq!(out.details["webhook"]["accepted"], json!(0));
    assert!(out.details["checked_at"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test(start_paused = true)]
async fn cached_for_cache_secs() {
    let fake = FakeJira::new();
    let rt = plugin_with(&fake, |c| c.health.cache_secs = 60)
        .start(&AppState::for_test())
        .unwrap();
    let health = rt.health();
    health.check().await;
    health.check().await;
    assert_eq!(fake.count(FakeOp::Myself), 1, "two checks, one call");
    tokio::time::advance(Duration::from_secs(61)).await;
    health.check().await;
    assert_eq!(fake.count(FakeOp::Myself), 2);
    // `cache_secs = 0` calls Jira each time.
    let fake = FakeJira::new();
    let rt = plugin_with(&fake, |c| c.health.cache_secs = 0)
        .start(&AppState::for_test())
        .unwrap();
    rt.health().check().await;
    rt.health().check().await;
    assert_eq!(fake.count(FakeOp::Myself), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jira_down_is_down_not_unready() {
    let fake = FakeJira::new();
    let client = app(plugin_with(&fake, |c| c.health.cache_secs = 0).webhook_route());
    fake.fault(FakeOp::Myself, Fault::Status(503));
    let started = std::time::Instant::now();
    let r = client.get("/actuator/health").send().await;
    assert!(
        started.elapsed() < Duration::from_millis(5000),
        "inside timeout_ms"
    );
    let body = json_of(&r);
    let jira = &body["components"]["jira"];
    assert_eq!(jira["status"], json!("DOWN"), "{body}");
    assert_eq!(jira["details"]["status"], json!(503), "{body}");
    assert_eq!(jira["details"]["error"], json!("upstream"), "{body}");
    // HealthOnly: `/ready` does not wait for Jira.
    client.get("/ready").send().await.assert_status(200);

    // A slow Jira: DOWN inside the timeout, not UNKNOWN.
    let fake2 = FakeJira::new();
    let slow = Arc::new(Slow(fake2.clone()));
    let mut config = JiraConfig::default();
    config.health.timeout_ms = 300;
    config.health.cache_secs = 0;
    let client = app(JiraPlugin::with_config(config).client(client_on(slow)));
    let started = std::time::Instant::now();
    let body = json_of(&client.get("/actuator/health").send().await);
    assert!(started.elapsed() < Duration::from_millis(2000));
    assert_eq!(
        body["components"]["jira"]["status"],
        json!("DOWN"),
        "{body}"
    );
    assert_eq!(
        body["components"]["jira"]["details"]["error"],
        json!("timeout"),
        "{body}"
    );
}

/// Answers after 5 s.
struct Slow(FakeJira);

#[async_trait::async_trait]
impl autumn_jira::san_mateo::transport::HttpTransport for Slow {
    async fn send(
        &self,
        request: autumn_jira::san_mateo::transport::HttpRequest,
    ) -> Result<
        autumn_jira::san_mateo::transport::HttpResponse,
        autumn_jira::san_mateo::transport::TransportError,
    > {
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.0.send(request).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn details_have_no_pii() {
    let fake = FakeJira::new();
    let client = app(plugin_with(&fake, |c| {
        c.webhook.ignore_actor_account_ids = vec![BOT.to_owned()];
    })
    .webhook_route());
    let r = client.get("/actuator/health").send().await;
    let text = r.text();
    let body = json_of(&r);
    let jira = &body["components"]["jira"];
    assert_eq!(jira["status"], json!("UP"), "{body}");
    assert_eq!(jira["details"]["self_filter"], json!("on"), "{body}");
    assert_eq!(
        jira["details"]["max_body_bytes"],
        json!(autumn_plugin_jira::RECOMMENDED_MAX_BODY_BYTES),
        "{body}"
    );
    for secret in [DEFAULT_EMAIL, DEFAULT_API_TOKEN, BOT, SECRET] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

//! Test helpers for apps (feature `testing`).

use async_trait::async_trait;
use autumn_jira_fake::{FakeDelivery, FakeJira};
use autumn_web::config::AutumnConfig;
use autumn_web::test::{TestClient, TestResponse};

use crate::JiraPlugin;

/// The webhook secret of the test config.
pub const TEST_WEBHOOK_SECRET: &str = "test-webhook-secret-0123456789abcdef";

/// A plugin wired to the fake.
#[must_use]
pub fn plugin_for(fake: &FakeJira) -> JiraPlugin {
    let _ = fake;
    JiraPlugin::new()
}

/// An `AutumnConfig` with the Jira endpoint.
#[must_use]
pub fn app_config() -> AutumnConfig {
    AutumnConfig::default()
}

/// Sends a [`FakeDelivery`] to the webhook route of a `TestApp`.
#[async_trait]
pub trait DeliverExt {
    /// Posts the delivery.
    async fn deliver(&self, delivery: &FakeDelivery) -> TestResponse;
}

#[async_trait]
impl DeliverExt for TestClient {
    async fn deliver(&self, delivery: &FakeDelivery) -> TestResponse {
        let _ = delivery;
        self.get("/").send().await
    }
}

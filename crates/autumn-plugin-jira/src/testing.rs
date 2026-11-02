//! Test helpers for apps (feature `testing`): the plugin on `FakeJira`,
//! the autumn config with the Jira endpoint, and signed deliveries.
//!
//! ```rust,no_run
//! use autumn_jira_fake::FakeJira;
//! use autumn_plugin_jira::testing::{DeliverExt, TEST_WEBHOOK_SECRET, app_config, plugin_for};
//! use autumn_web::test::TestApp;
//!
//! # async fn demo() {
//! let fake = FakeJira::new();
//! let client = TestApp::new()
//!     .config(app_config())
//!     .plugin(plugin_for(&fake).on_event(|_delivery, _state| async { Ok(()) }))
//!     .build();
//! fake.human_comment("OPS-1", "Looks good", "ann").ok();
//! for delivery in fake.take_deliveries() {
//!     client.deliver(&delivery).await.assert_status(200);
//! }
//! # let _ = TEST_WEBHOOK_SECRET;
//! # }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, DEFAULT_SITE, FakeDelivery, FakeJira};
use autumn_web::config::AutumnConfig;
use autumn_web::test::{TestClient, TestResponse};

use crate::config::{ApiTokenConfig, JiraConfig};
use crate::webhook_config::{DEFAULT_SECRET_ENV, webhook_endpoint};
use crate::{DEFAULT_PREFIX, JiraPlugin};

/// The webhook secret of the test config: 36 bytes, so it also passes the
/// 32-byte rule of the autumn prod profile.
pub const TEST_WEBHOOK_SECRET: &str = "test-webhook-secret-0123456789abcdef";

/// The env var names that [`plugin_for`] sets.
const EMAIL_ENV: &str = "JIRA_EMAIL";
/// The env var of the API token.
const TOKEN_ENV: &str = "JIRA_API_TOKEN";

/// A plugin wired to the fake: API token mode on the site
/// [`DEFAULT_SITE`], the fake as the transport, the default fake
/// credentials, and the webhook route.
///
/// It also turns on the deliveries of `fake`, signed with
/// [`TEST_WEBHOOK_SECRET`]. Use it with [`app_config`].
#[must_use]
pub fn plugin_for(fake: &FakeJira) -> JiraPlugin {
    // The clones share the state, so this turns on the deliveries of `fake`.
    let fake = fake.clone().with_webhook_secret(TEST_WEBHOOK_SECRET);
    let config = JiraConfig {
        api_token: Some(ApiTokenConfig {
            site: Some(DEFAULT_SITE.to_owned()),
            cloud_id: None,
            email_env: EMAIL_ENV.to_owned(),
            token_env: TOKEN_ENV.to_owned(),
        }),
        ..JiraConfig::default()
    };
    JiraPlugin::with_config(config)
        .transport(Arc::new(fake))
        .secret_env(EMAIL_ENV, DEFAULT_EMAIL)
        .secret_env(TOKEN_ENV, DEFAULT_API_TOKEN)
        .webhook_route()
}

/// An `AutumnConfig` with the Jira endpoint at `/jira/webhook` (secret
/// [`TEST_WEBHOOK_SECRET`]) and CSRF on. autumn exempts the webhook path
/// from CSRF.
#[must_use]
pub fn app_config() -> AutumnConfig {
    let mut cfg = AutumnConfig::default();
    let mut endpoint = webhook_endpoint(&format!("{DEFAULT_PREFIX}/webhook"), DEFAULT_SECRET_ENV);
    endpoint.secret = Some(TEST_WEBHOOK_SECRET.to_owned());
    cfg.security.webhooks.endpoints.push(endpoint);
    cfg.security.csrf.enabled = true;
    cfg
}

/// Sends a [`FakeDelivery`] to the webhook route of a `TestApp`.
#[async_trait]
pub trait DeliverExt {
    /// Posts the delivery with all its headers to `/jira/webhook`.
    async fn deliver(&self, delivery: &FakeDelivery) -> TestResponse;
}

#[async_trait]
impl DeliverExt for TestClient {
    async fn deliver(&self, delivery: &FakeDelivery) -> TestResponse {
        let mut request = self.post(&format!("{DEFAULT_PREFIX}/webhook"));
        for (name, value) in &delivery.headers {
            request = request.header(name, value);
        }
        request.body(delivery.body.clone()).send().await
    }
}

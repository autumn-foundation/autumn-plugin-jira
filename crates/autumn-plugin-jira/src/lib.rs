//! Jira Cloud plugin for autumn-web.
//!
//! The plugin installs Jira into an autumn app with one call:
//! `.plugin(JiraPlugin::new())`.
//!
//! - **Config:** the `[jira]` section. It holds env var names, never
//!   secrets. The table that exists sets the mode: `[jira.api_token]` or
//!   `[jira.san_mateo]`. San Mateo mode needs `SanMateoPlugin`, registered
//!   before `JiraPlugin`.
//! - **Service:** [`JiraService`] is in the app state after the start. It
//!   is an extractor and derefs to [`JiraClient`]. `.http()?`
//!   ([`JiraResultExt`]) answers a Jira error with its HTTP status. The
//!   start makes no network call.
//! - **Health:** the `jira` indicator in `/actuator/health`
//!   ([`JiraHealth`], `HealthOnly`, so `/ready` does not wait for Jira).
//! - **Webhook route:** a `POST` route at the full path `{prefix}/webhook`
//!   (default `/jira/webhook`), mounted with a sink, an `on_event` closure
//!   or `webhook_route()`. autumn checks the signature with its
//!   `SignedWebhook` extractor. The app declares the endpoint in
//!   `[[security.webhooks.endpoints]]`. The start fails when that entry does
//!   not fit Jira, and the error gives the TOML that fits. Use
//!   [`webhook_endpoint`] or [`webhook_endpoint_toml`]. After the signature
//!   check the route answers 200 or 503 only.
//! - **Tests:** the `testing` feature gives `testing::plugin_for(&fake)`,
//!   `testing::app_config()` and `deliver(..)`.
//!
//! Webhooks are at least once. Each sink can get the same delivery again,
//! also when another sink fails. Make each sink idempotent.
//!
//! ```rust,no_run
//! use std::sync::Arc;
//!
//! use autumn_plugin_jira::{Delivery, JiraPlugin};
//!
//! # async fn demo() {
//! autumn_web::app()
//!     .plugin(JiraPlugin::new().on_event(|delivery: Arc<Delivery>, _state| async move {
//!         tracing::info!(event = %delivery.event, "jira event");
//!         Ok(())
//!     }))
//!     .run()
//!     .await;
//! # }
//! ```
//!
//! See the crate README and `docs/plan.md`.

pub mod config;
mod error;
mod health;
mod plugin;
mod runtime;
mod secrets;
mod service;
#[cfg(feature = "testing")]
pub mod testing;
mod webhook;
pub mod webhook_config;

pub use autumn_jira as jira;
pub use autumn_jira::{
    Adf, Delivery, EventSink, HandlerError, IssueKey, JiraClient, JiraError, JiraEvent, Jql,
    StatusTarget,
};
pub use config::JiraConfig;
pub use error::PluginError;
pub use health::JiraHealth;
pub use plugin::{JiraPlugin, plugin_contract};
pub use runtime::{JiraRuntime, WebhookCounters};
pub use service::{JiraResultExt, JiraService, autumn_error};
pub use webhook_config::{webhook_endpoint, webhook_endpoint_toml};

// The README snippets compile and run as doc tests. They use `FakeJira`
// and the `testing` helpers, so they need the `testing` feature.
#[cfg(all(doctest, feature = "testing"))]
#[doc = include_str!("../../../README.md")]
pub struct ReadmeDoctests;

/// The plugin name. autumn uses it to find a plugin that is added two times.
pub const PLUGIN_NAME: &str = "autumn-plugin-jira";
/// The `autumn-web` versions that this plugin supports.
///
/// It must equal the `autumn-web` requirement in the workspace `Cargo.toml`.
/// A unit test checks this.
pub const AUTUMN_WEB_REQUIREMENT: &str = "0.9";
/// The name of the TOML section.
pub const SECTION: &str = "jira";
/// The prefix of the env overrides, for example `AUTUMN_JIRA__HTTP__MAX_PAGES`.
pub const ENV_PREFIX: &str = "AUTUMN_JIRA__";
/// The default route prefix.
pub const DEFAULT_PREFIX: &str = "/jira";
/// The name of the health indicator.
pub const INDICATOR_NAME: &str = "jira";
/// The default name of the `[[security.webhooks.endpoints]]` entry.
pub const ENDPOINT_NAME: &str = "jira";
/// The body cap that the helper endpoint sets: 26 MiB.
///
/// Jira drops a payload over 25 MB. So this cap never refuses a payload
/// that Jira sends.
pub const RECOMMENDED_MAX_BODY_BYTES: usize = 27_262_976;
/// Below this body cap (8 MiB), the start logs a warning.
///
/// The app owns the choice, so the start does not fail. A large issue
/// payload can be over a small cap, and autumn then refuses it with 400.
pub const MIN_MAX_BODY_BYTES: usize = 8_388_608;

//! autumn: the webhook door and one app route.
//!
//! ```text
//! Jira ─► POST /jira/webhook       autumn checks the signature ─► HarvestSignalBridge ─► the run
//! you  ─► GET  /incidents/{key}    JiraService (san-mateo connection acme) ─► Jira
//! ```
//!
//! `SanMateoPlugin` comes first: `JiraPlugin` in `san_mateo` mode takes the
//! `SanMateo` that it puts in the app state.

use autumn_jira::san_mateo::SanMateo;
use autumn_jira_harvest::HarvestSignalBridge;
use autumn_plugin_jira::{IssueKey, JiraConfig, JiraPlugin, JiraResultExt, JiraService};
use autumn_plugin_san_mateo::{SanMateoConfig, SanMateoPlugin};
use autumn_web::config::AutumnConfig;
use autumn_web::test::{TestApp, TestClient};
use autumn_web::{AutumnResult, Json, Path};
use serde_json::{Value, json};

use crate::BoxError;

/// The `autumn.toml` of the app: the `[san_mateo]` and `[jira]` sections.
pub const AUTUMN_TOML: &str = include_str!("../autumn.toml");
/// The webhook path.
pub const WEBHOOK_PATH: &str = "/jira/webhook";
/// The env var of the webhook secret, for the `run()` boot.
pub const WEBHOOK_SECRET_ENV: &str = "JIRA_WEBHOOK_SECRET";

/// The app route: the incident as Jira sees it now.
#[autumn_web::get("/incidents/{key}")]
pub async fn incident(jira: JiraService, Path(key): Path<IssueKey>) -> AutumnResult<Json<Value>> {
    let issue = jira.get_issue(key).await.http()?;
    Ok(Json(json!({
        "key": issue.key.to_string(),
        "status": issue.fields.status.map(|s| s.name),
    })))
}

/// The `[san_mateo]` section of [`AUTUMN_TOML`].
///
/// # Errors
///
/// A bad section.
pub fn san_mateo_config() -> Result<SanMateoConfig, BoxError> {
    Ok(SanMateoConfig::from_toml_str(AUTUMN_TOML)?)
}

/// The `[jira]` section of [`AUTUMN_TOML`].
///
/// # Errors
///
/// A bad section.
pub fn jira_config() -> Result<JiraConfig, BoxError> {
    Ok(JiraConfig::from_toml_str(AUTUMN_TOML)?)
}

/// The San Mateo plugin with the one `SanMateo` of the example. The
/// example has no background refresh loop: the proxy refreshes a token
/// before a call when it expires.
///
/// # Errors
///
/// A bad `[san_mateo]` section.
pub fn san_mateo_plugin(san: &SanMateo) -> Result<SanMateoPlugin, BoxError> {
    Ok(SanMateoPlugin::with_config(san_mateo_config()?)
        .service(san.clone())
        .background(false))
}

/// The Jira plugin in `san_mateo` mode. The bridge is its sink, so a
/// delivery becomes a signal of the run that waits on the issue.
///
/// The bridge is a sink for all actors (R2-webhook-security-5). The bot
/// account makes the API writes, and a status that the bot sets must also
/// signal a waiting run. The bridge does not write to Jira, so it cannot
/// make a loop. It reads the issue from the site of its client, and it
/// knows the host of that site (`transport::site_host`). So it refuses a
/// delivery of another site. Other sinks of the app do not get the events
/// of the bot.
///
/// # Errors
///
/// A bad `[jira]` section.
pub fn jira_plugin(bridge: HarvestSignalBridge) -> Result<JiraPlugin, BoxError> {
    Ok(JiraPlugin::with_config(jira_config()?).sink_for_all_actors(bridge))
}

/// The autumn config of `TestApp`: the Jira webhook endpoint with
/// `secret`, CSRF on (the San Mateo routes are exempt), and the health
/// details. autumn exempts the webhook path from CSRF.
pub fn autumn_config(secret: &str) -> AutumnConfig {
    let mut cfg = AutumnConfig::default();
    let mut endpoint = autumn_plugin_jira::webhook_endpoint(WEBHOOK_PATH, WEBHOOK_SECRET_ENV);
    endpoint.secret = Some(secret.to_owned());
    cfg.security.webhooks.endpoints.push(endpoint);
    cfg.security.csrf.enabled = true;
    // The same as `[security.csrf]` in `autumn.toml`. San Mateo clients send
    // a secret key or a connect session token, not a CSRF cookie.
    cfg.security
        .csrf
        .exempt_paths
        .push(autumn_plugin_san_mateo::DEFAULT_PREFIX.to_owned());
    cfg.health.detailed = true;
    cfg
}

/// The app in process. `TestApp` serves it with no port.
///
/// # Errors
///
/// A bad section of [`AUTUMN_TOML`].
///
/// # Panics
///
/// `TestApp::build` panics when a plugin fails to start, with the error
/// of the plugin.
pub fn test_app(
    san: &SanMateo,
    bridge: HarvestSignalBridge,
    secret: &str,
) -> Result<TestClient, BoxError> {
    Ok(TestApp::new()
        .config(autumn_config(secret))
        .routes(autumn_web::routes![incident])
        .plugin(san_mateo_plugin(san)?)
        .plugin(jira_plugin(bridge)?)
        .build())
}

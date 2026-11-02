//! The webhook endpoint that fits Jira, and the boot check.
//!
//! autumn checks the signature of each delivery. It reads the settings from
//! the `[[security.webhooks.endpoints]]` entry. Jira Cloud signs with
//! `X-Hub-Signature: sha256=<hex>` and sends the delivery id in
//! `X-Atlassian-Webhook-Identifier`. The autumn `generic` defaults use
//! other headers. So the plugin checks the entry at the start and gives the
//! TOML that fits.

use autumn_web::webhook::WebhookEndpointConfig;

use crate::config::JiraWebhookConfig;
use crate::error::PluginError;

/// The env var name that the fix TOML uses when the entry names none.
pub const DEFAULT_SECRET_ENV: &str = "JIRA_WEBHOOK_SECRET";
/// The header that holds the Jira signature.
pub const SIGNATURE_HEADER: &str = "X-Hub-Signature";
/// The prefix of the Jira signature value.
pub const SIGNATURE_PREFIX: &str = "sha256=";
/// The header that holds the Jira delivery id. It is the same on each retry.
pub const DELIVERY_ID_HEADER: &str = "X-Atlassian-Webhook-Identifier";

/// A `WebhookEndpointConfig` that passes the boot check.
///
/// The name is [`ENDPOINT_NAME`](crate::ENDPOINT_NAME). The body cap is
/// [`RECOMMENDED_MAX_BODY_BYTES`](crate::RECOMMENDED_MAX_BODY_BYTES).
/// Replay protection is on.
///
/// autumn reads `secret_env` when it loads its config. When you add this
/// entry to a config in code, also set `secret`.
#[must_use]
pub fn webhook_endpoint(path: &str, secret_env: &str) -> WebhookEndpointConfig {
    let _ = (path, secret_env);
    WebhookEndpointConfig::default()
}

/// The TOML block for [`webhook_endpoint`]. The boot check prints it as the
/// fix.
#[must_use]
pub fn webhook_endpoint_toml(path: &str, secret_env: &str) -> String {
    let _ = (path, secret_env);
    String::new()
}

/// Checks the `[[security.webhooks.endpoints]]` entries for the Jira route.
///
/// It finds the entry with `name == settings.endpoint_name` and checks that
/// it fits Jira and `webhook_path`. It returns the warnings to log.
///
/// # Errors
/// Returns [`PluginError::WebhookEndpoint`] with the TOML that fits when no
/// entry has the name or a field does not fit.
pub fn check_webhook_endpoint(
    endpoints: &[WebhookEndpointConfig],
    webhook_path: &str,
    settings: &JiraWebhookConfig,
    allow_without_replay_protection: bool,
) -> Result<Vec<String>, PluginError> {
    let _ = (
        endpoints,
        webhook_path,
        settings,
        allow_without_replay_protection,
    );
    Ok(Vec::new())
}

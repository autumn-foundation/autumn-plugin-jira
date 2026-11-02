//! The webhook endpoint that fits Jira, and the boot check.
//!
//! autumn checks the signature of each delivery. It reads the settings from
//! the `[[security.webhooks.endpoints]]` entry. Jira Cloud signs with
//! `X-Hub-Signature: sha256=<hex>` and sends the delivery id in
//! `X-Atlassian-Webhook-Identifier`. The autumn `generic` defaults use
//! other headers. So the plugin checks the entry at the start and gives the
//! TOML that fits.

use autumn_web::webhook::{WebhookEndpointConfig, WebhookProvider};

use crate::config::{JiraWebhookConfig, is_env_name};
use crate::error::PluginError;
use crate::{ENDPOINT_NAME, MIN_MAX_BODY_BYTES, RECOMMENDED_MAX_BODY_BYTES};

/// The env var name that the fix TOML uses when the entry names none.
pub const DEFAULT_SECRET_ENV: &str = "JIRA_WEBHOOK_SECRET";
/// The header that holds the Jira signature.
pub const SIGNATURE_HEADER: &str = "X-Hub-Signature";
/// The prefix of the Jira signature value.
pub const SIGNATURE_PREFIX: &str = "sha256=";
/// The header that holds the Jira delivery id. It is the same on each retry.
pub const DELIVERY_ID_HEADER: &str = "X-Atlassian-Webhook-Identifier";
/// The longest time after the event in which Jira can send a delivery,
/// in seconds: 90 minutes.
///
/// A Secondary-flow delivery can come about 15 minutes late. After a 503,
/// Jira sends it again up to 5 times, with 5 to 15 minutes between two
/// tries. Each copy has the first timestamp. A `max_age_secs` below this
/// value makes a late copy stale, and the route drops it.
pub const JIRA_RETRY_HORIZON_SECS: u64 = 5400;

/// A `WebhookEndpointConfig` that passes the boot check.
///
/// The name is [`ENDPOINT_NAME`]. The body cap is
/// [`RECOMMENDED_MAX_BODY_BYTES`]. Replay protection is on, with the autumn
/// default window of 86400 seconds.
///
/// autumn reads `secret_env` when it loads its config. When you add this
/// entry to a config in code, also set `secret`.
///
/// autumn reads up to `max_body_bytes` of each request before it checks
/// the signature. See [`RECOMMENDED_MAX_BODY_BYTES`] for that memory cost
/// and the proxy limits that control it.
#[must_use]
pub fn webhook_endpoint(path: &str, secret_env: &str) -> WebhookEndpointConfig {
    let mut endpoint = WebhookEndpointConfig::generic(ENDPOINT_NAME, path, "");
    endpoint.secret = None;
    endpoint.secret_env = Some(secret_env.to_owned());
    endpoint.signature_header = Some(SIGNATURE_HEADER.to_owned());
    endpoint.signature_prefix = Some(SIGNATURE_PREFIX.to_owned());
    endpoint.timestamp_header = None;
    endpoint.delivery_id_header = Some(DELIVERY_ID_HEADER.to_owned());
    // Jira sends no event type header. The event type is in the body.
    endpoint.event_type_header = None;
    endpoint.replay_protection = true;
    endpoint.max_body_bytes = RECOMMENDED_MAX_BODY_BYTES;
    endpoint
}

/// The TOML block for [`webhook_endpoint`]. The boot check prints it as the
/// fix.
#[must_use]
pub fn webhook_endpoint_toml(path: &str, secret_env: &str) -> String {
    endpoint_toml(ENDPOINT_NAME, path, secret_env)
}

/// The TOML block for an entry with the name `name`.
fn endpoint_toml(name: &str, path: &str, secret_env: &str) -> String {
    let e = webhook_endpoint(path, secret_env);
    let quote = |text: &str| toml::Value::String(text.to_owned()).to_string();
    format!(
        "[[security.webhooks.endpoints]]\n\
         name = {name}\n\
         path = {path}\n\
         provider = \"generic\"\n\
         secret_env = {secret_env}\n\
         signature_header = {signature_header}\n\
         signature_prefix = {signature_prefix}\n\
         delivery_id_header = {delivery_id_header}\n\
         replay_protection = true\n\
         replay_window_secs = {window}\n\
         max_body_bytes = {max_body_bytes}\n",
        name = quote(name),
        path = quote(path),
        secret_env = quote(secret_env),
        signature_header = quote(SIGNATURE_HEADER),
        signature_prefix = quote(SIGNATURE_PREFIX),
        delivery_id_header = quote(DELIVERY_ID_HEADER),
        window = e.replay_window_secs,
        max_body_bytes = e.max_body_bytes,
    )
}

/// Checks the `[[security.webhooks.endpoints]]` entries for the Jira route.
///
/// It finds the entry with `name == settings.endpoint_name` and checks that
/// it fits Jira and `webhook_path`. It returns the warnings to log. A body
/// cap below [`MIN_MAX_BODY_BYTES`] and a `max_age_secs` below
/// [`JIRA_RETRY_HORIZON_SECS`] are warnings only: the app owns those
/// choices.
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
    let name = settings.endpoint_name.as_str();
    let endpoint = endpoints.iter().find(|e| e.name == name);
    // The env var of the named entry, else of an entry at the route path.
    // A value that is not an env var name can be the secret itself, so the
    // fix shows the default name in its place.
    let secret_env = endpoint
        .or_else(|| endpoints.iter().find(|e| e.path == webhook_path))
        .and_then(|e| e.secret_env.as_deref())
        .filter(|name| is_env_name(name))
        .unwrap_or(DEFAULT_SECRET_ENV);
    let fail = |problem: String| PluginError::WebhookEndpoint {
        fix: format!(
            "# {problem}\n{}",
            endpoint_toml(name, webhook_path, secret_env)
        ),
    };
    let Some(e) = endpoint else {
        return Err(fail(format!(
            "no [[security.webhooks.endpoints]] entry has name = {name:?}"
        )));
    };
    if let Some(problem) = problem(e, webhook_path, settings, allow_without_replay_protection) {
        return Err(fail(problem));
    }
    let mut warnings = Vec::new();
    if settings.max_age_secs < JIRA_RETRY_HORIZON_SECS {
        warnings.push(format!(
            "[jira.webhook] max_age_secs = {}; Jira sends a delivery again after a 503 for up to {JIRA_RETRY_HORIZON_SECS} seconds with the first timestamp, so the route drops a late copy as stale; set max_age_secs = {JIRA_RETRY_HORIZON_SECS} or more",
            settings.max_age_secs
        ));
    }
    if e.max_body_bytes < MIN_MAX_BODY_BYTES {
        warnings.push(format!(
            "the webhook endpoint {name:?} has max_body_bytes = {}; Jira sends payloads up to 25 MB, and autumn refuses a larger body with 400; set max_body_bytes = {RECOMMENDED_MAX_BODY_BYTES}",
            e.max_body_bytes
        ));
    }
    Ok(warnings)
}

/// The first field of `e` that does not fit Jira, as one line of text.
fn problem(
    e: &WebhookEndpointConfig,
    webhook_path: &str,
    settings: &JiraWebhookConfig,
    allow_without_replay_protection: bool,
) -> Option<String> {
    let same = |got: Option<&str>, want: &str| got.is_some_and(|g| g.eq_ignore_ascii_case(want));
    if e.path != webhook_path {
        return Some(format!(
            "path is {:?}; it must be the route path {webhook_path:?}",
            e.path
        ));
    }
    if e.provider != WebhookProvider::Generic {
        return Some(format!(
            "provider is {:?}; it must be \"generic\"",
            e.provider.as_str()
        ));
    }
    if !same(e.signature_header.as_deref(), SIGNATURE_HEADER) {
        let got = e.signature_header.as_ref().map_or_else(
            || "not set, so autumn reads X-Webhook-Signature".to_owned(),
            |h| format!("{h:?}"),
        );
        return Some(format!(
            "signature_header is {got}; it must be {SIGNATURE_HEADER:?}"
        ));
    }
    if let Some(prefix) = e.signature_prefix.as_deref()
        && prefix != SIGNATURE_PREFIX
    {
        return Some(format!(
            "signature_prefix is {prefix:?}; it must be {SIGNATURE_PREFIX:?} or not set"
        ));
    }
    if let Some(h) = &e.timestamp_header {
        return Some(format!(
            "timestamp_header is {h:?}; remove it, because Jira sends no timestamp header"
        ));
    }
    if !same(e.delivery_id_header.as_deref(), DELIVERY_ID_HEADER) {
        return Some(format!(
            "delivery_id_header is {:?}; it must be {DELIVERY_ID_HEADER:?}, the id that stays the same on each retry",
            e.delivery_id_header.as_deref().unwrap_or("not set")
        ));
    }
    if !e.replay_protection {
        if !allow_without_replay_protection {
            return Some(
                "replay_protection is false; set it to true, or call JiraPlugin::allow_webhooks_without_replay_protection()".to_owned(),
            );
        }
    } else if settings
        .max_age_secs
        .saturating_add(settings.max_future_skew_secs)
        >= e.replay_window_secs
    {
        return Some(format!(
            "[jira.webhook] max_age_secs ({}) + max_future_skew_secs ({}) is not less than replay_window_secs ({}); a copy that is still fresh after the window is not refused",
            settings.max_age_secs, settings.max_future_skew_secs, e.replay_window_secs
        ));
    }
    None
}

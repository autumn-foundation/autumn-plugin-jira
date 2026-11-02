//! AC28: the webhook boot check.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use autumn_plugin_jira::config::JiraWebhookConfig;
use autumn_plugin_jira::webhook_config::{DEFAULT_SECRET_ENV, check_webhook_endpoint};
use autumn_plugin_jira::{
    ENDPOINT_NAME, JiraConfig, JiraPlugin, MIN_MAX_BODY_BYTES, PluginError,
    RECOMMENDED_MAX_BODY_BYTES, webhook_endpoint, webhook_endpoint_toml,
};
use autumn_web::ProcessRole;
use autumn_web::config::AutumnConfig;
use autumn_web::test::TestApp;
use autumn_web::webhook::{WebhookEndpointConfig, WebhookProvider};

const PATH: &str = "/jira/webhook";
const SECRET_ENV: &str = "ACME_JIRA_HOOK_SECRET";
const SECRET_VALUE: &str = "boot-check-secret-0123456789abcdef";

fn good() -> WebhookEndpointConfig {
    let mut e = webhook_endpoint(PATH, SECRET_ENV);
    e.secret = Some(SECRET_VALUE.to_owned());
    e
}

fn check(endpoint: WebhookEndpointConfig) -> Result<Vec<String>, PluginError> {
    check_webhook_endpoint(&[endpoint], PATH, &JiraWebhookConfig::default(), false)
}

/// A plugin with a prebuilt client: the start needs no secret.
fn plugin() -> JiraPlugin {
    common::plugin(&autumn_jira_fake::FakeJira::new())
}

fn autumn_config(endpoint: WebhookEndpointConfig, role: ProcessRole) -> AutumnConfig {
    let mut cfg = AutumnConfig::default();
    cfg.security.webhooks.endpoints.push(endpoint);
    cfg.role = role;
    cfg
}

/// Builds a `TestApp` with the plugin. Returns the panic text when the
/// start fails.
fn boot(plugin: JiraPlugin, cfg: AutumnConfig) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| {
        let _client = TestApp::new().config(cfg).plugin(plugin).build();
    }))
    .map_err(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default()
    })
}

/// `err` is `WebhookEndpoint`. Its fix is a comment that names `field`,
/// then the TOML `fix`. It holds no secret.
fn assert_fix(label: &str, err: &PluginError, field: &str, fix: &str) {
    let PluginError::WebhookEndpoint { fix: got } = err else {
        panic!("{label}: wrong variant {err:?}");
    };
    assert!(got.ends_with(fix), "{label}: the fix TOML is in {got}");
    let problem = got.lines().next().unwrap();
    assert!(problem.starts_with("# "), "{label}: {problem}");
    assert!(problem.contains(field), "{label}: {problem}");
    let text = err.to_string();
    assert!(
        text.starts_with("the webhook endpoint does not fit Jira; use:\n"),
        "{label}: {text}"
    );
    assert!(!text.contains(SECRET_VALUE), "{label}: {text}");
}

#[test]
fn each_wrong_field_fails_with_the_fix() {
    type Edit = fn(&mut WebhookEndpointConfig);
    let cases: [(&str, Edit, &str); 11] = [
        ("path", |e| e.path = "/hooks/jira".to_owned(), "path"),
        (
            "provider",
            |e| e.provider = WebhookProvider::Github,
            "provider",
        ),
        (
            "signature header unset",
            |e| e.signature_header = None,
            "signature_header",
        ),
        (
            "generic signature header",
            |e| e.signature_header = Some("X-Webhook-Signature".to_owned()),
            "signature_header",
        ),
        (
            "prefix",
            |e| e.signature_prefix = Some("v0=".to_owned()),
            "signature_prefix",
        ),
        (
            "timestamp header",
            |e| e.timestamp_header = Some("X-Timestamp".to_owned()),
            "timestamp_header",
        ),
        (
            "delivery header unset",
            |e| e.delivery_id_header = None,
            "delivery_id_header",
        ),
        (
            "other delivery header",
            |e| e.delivery_id_header = Some("X-Webhook-Delivery".to_owned()),
            "delivery_id_header",
        ),
        (
            "replay off",
            |e| e.replay_protection = false,
            "replay_protection",
        ),
        (
            "replay window under max_age_secs",
            |e| e.replay_window_secs = 3600,
            "max_age_secs",
        ),
        (
            "no entry with the name",
            |e| e.name = "other".to_owned(),
            "name",
        ),
    ];
    let fix = webhook_endpoint_toml(PATH, SECRET_ENV);
    for (label, edit, field) in cases {
        let mut endpoint = good();
        edit(&mut endpoint);
        assert_fix(label, &check(endpoint).unwrap_err(), field, &fix);
    }

    // The header names compare without case. `sha256=` and no prefix pass.
    let mut endpoint = good();
    endpoint.signature_header = Some("x-hub-signature".to_owned());
    endpoint.delivery_id_header = Some("x-atlassian-webhook-identifier".to_owned());
    endpoint.signature_prefix = None;
    assert_eq!(check(endpoint).unwrap(), Vec::<String>::new());

    // The fix uses the env var of the entry, else the default name.
    let mut endpoint = good();
    endpoint.provider = WebhookProvider::Slack;
    endpoint.secret_env = None;
    let PluginError::WebhookEndpoint { fix: got } = check(endpoint).unwrap_err() else {
        panic!("wrong variant");
    };
    assert!(
        got.contains(&format!("secret_env = \"{DEFAULT_SECRET_ENV}\"")),
        "{got}"
    );

    // A custom endpoint name goes into the fix.
    let mut settings = JiraWebhookConfig::default();
    settings.endpoint_name = "jira-in".to_owned();
    let err = check_webhook_endpoint(&[good()], PATH, &settings, false).unwrap_err();
    assert!(err.to_string().contains("name = \"jira-in\""), "{err}");

    // The start fails with the fix.
    let mut endpoint = good();
    endpoint.signature_header = None;
    let text = boot(
        plugin().webhook_route(),
        autumn_config(endpoint, ProcessRole::Combined),
    )
    .unwrap_err();
    assert!(text.contains("X-Hub-Signature"), "{text}");
    assert!(text.contains("signature_header"), "{text}");
}

#[test]
fn small_body_cap_only_warns() {
    let mut endpoint = good();
    endpoint.max_body_bytes = MIN_MAX_BODY_BYTES - 1;
    let warnings = check(endpoint.clone()).unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("max_body_bytes"), "{warnings:?}");
    assert!(
        warnings[0].contains(&RECOMMENDED_MAX_BODY_BYTES.to_string()),
        "the warning gives the fix: {warnings:?}"
    );
    // At the bound: no warning.
    endpoint.max_body_bytes = MIN_MAX_BODY_BYTES;
    assert_eq!(check(endpoint.clone()).unwrap(), Vec::<String>::new());

    // The start passes.
    endpoint.max_body_bytes = 1024 * 1024;
    boot(
        plugin().webhook_route(),
        autumn_config(endpoint, ProcessRole::Combined),
    )
    .unwrap();
}

/// The shape of an `autumn.toml` with webhook endpoints.
#[derive(serde::Deserialize)]
struct Doc {
    security: Security,
}
#[derive(serde::Deserialize)]
struct Security {
    webhooks: Webhooks,
}
#[derive(serde::Deserialize)]
struct Webhooks {
    endpoints: Vec<WebhookEndpointConfig>,
}

#[test]
fn helper_endpoint_passes() {
    let e = webhook_endpoint(PATH, SECRET_ENV);
    assert_eq!(e.name, ENDPOINT_NAME);
    assert_eq!(e.path, PATH);
    assert_eq!(e.provider, WebhookProvider::Generic);
    assert_eq!(e.secret, None, "the helper holds no secret");
    assert_eq!(e.secret_env.as_deref(), Some(SECRET_ENV));
    assert_eq!(e.signature_header.as_deref(), Some("X-Hub-Signature"));
    assert_eq!(e.signature_prefix.as_deref(), Some("sha256="));
    assert_eq!(e.timestamp_header, None);
    assert_eq!(
        e.delivery_id_header.as_deref(),
        Some("X-Atlassian-Webhook-Identifier")
    );
    assert!(e.replay_protection);
    assert_eq!(e.max_body_bytes, RECOMMENDED_MAX_BODY_BYTES);
    assert_eq!(check(e.clone()).unwrap(), Vec::<String>::new());

    // The TOML block reads back as the same entry.
    let text = webhook_endpoint_toml(PATH, SECRET_ENV);
    let doc: Doc = toml::from_str(&text).unwrap();
    let [t] = doc.security.webhooks.endpoints.try_into().unwrap();
    assert_eq!(
        (&t.name, &t.path, t.provider, &t.secret, &t.secret_env),
        (&e.name, &e.path, e.provider, &e.secret, &e.secret_env)
    );
    assert_eq!(
        (
            &t.signature_header,
            &t.signature_prefix,
            &t.timestamp_header
        ),
        (
            &e.signature_header,
            &e.signature_prefix,
            &e.timestamp_header
        )
    );
    assert_eq!(
        (
            &t.delivery_id_header,
            t.replay_protection,
            t.replay_window_secs,
            t.max_body_bytes
        ),
        (
            &e.delivery_id_header,
            e.replay_protection,
            e.replay_window_secs,
            e.max_body_bytes
        )
    );
    assert_eq!(check(t).unwrap(), Vec::<String>::new());

    // The start passes with the helper entry.
    boot(
        plugin().webhook_route(),
        autumn_config(good(), ProcessRole::Combined),
    )
    .unwrap();
}

#[test]
fn replay_off_needs_the_opt_in() {
    let mut endpoint = good();
    endpoint.replay_protection = false;
    let settings = JiraWebhookConfig::default();
    let err = check_webhook_endpoint(&[endpoint.clone()], PATH, &settings, false).unwrap_err();
    assert!(err.to_string().contains("replay_protection"), "{err}");
    assert_eq!(
        check_webhook_endpoint(&[endpoint.clone()], PATH, &settings, true).unwrap(),
        Vec::<String>::new()
    );

    // The start: it fails without the opt-in and passes with it.
    let text = boot(
        plugin().webhook_route(),
        autumn_config(endpoint.clone(), ProcessRole::Combined),
    )
    .unwrap_err();
    assert!(text.contains("replay_protection"), "{text}");
    boot(
        plugin()
            .webhook_route()
            .allow_webhooks_without_replay_protection(),
        autumn_config(endpoint, ProcessRole::Combined),
    )
    .unwrap();
}

#[test]
fn worker_role_skips_the_check() {
    let mut wrong = good();
    wrong.signature_header = None;
    // A worker serves no user routes, so it does not check the endpoint.
    boot(
        plugin().webhook_route(),
        autumn_config(wrong.clone(), ProcessRole::Worker),
    )
    .unwrap();
    // No route: no check.
    boot(
        plugin(),
        autumn_config(wrong.clone(), ProcessRole::Combined),
    )
    .unwrap();
    // The web role checks.
    assert!(
        boot(
            plugin().webhook_route(),
            autumn_config(wrong, ProcessRole::Web),
        )
        .is_err()
    );
}

#[test]
fn a_bad_jira_config_fails_the_start() {
    // No mode table and no prebuilt client.
    let text = boot(
        JiraPlugin::with_config(JiraConfig::default()),
        AutumnConfig::default(),
    )
    .unwrap_err();
    assert!(
        text.contains("set [jira.api_token] or [jira.san_mateo]"),
        "{text}"
    );
}

//! The outside world of the example. One `FakeJira` is Jira Cloud, the
//! Atlassian OAuth server and the cloud id discovery. A `MemoryTransport`
//! is the team chat API. One `SanMateo` holds the credentials of both.
//!
//! san-mateo has one transport. `FakeJira` serves its own hosts and sends
//! every other host to its fallback, the chat fake. So the example needs no
//! port and no dev-only proxy flag.

use std::sync::Arc;
use std::time::Duration;

use autumn_jira::JiraClient;
use autumn_jira::backend::{SanMateoBackend, SanMateoTarget};
use autumn_jira::client::ClientSettings;
use autumn_jira::ids::SiteHost;
use autumn_jira::san_mateo::clock::{Entropy, OsEntropy};
use autumn_jira::san_mateo::credentials::OAuth2Credentials;
use autumn_jira::san_mateo::transport::HttpResponse;
use autumn_jira::san_mateo::{
    Catalog, ConnectionKey, Credentials, EnvironmentConfig, GetOptions, ImportConnection, KeyRing,
    MemoryTransport, NewIntegration, ProxySettings, SanMateo, Secret, Tags,
};
use autumn_jira_fake::{FakeJira, FakeProject};
use serde_json::{Map, Value, json};

use crate::BoxError;

/// The san-mateo environment of the example.
pub const ENV: &str = "ops";
/// The integration of Jira: provider `atlassian` (OAuth 2.0 3LO).
pub const JIRA_INTEGRATION: &str = "jira";
/// The connection of the tenant `acme`.
pub const JIRA_CONNECTION: &str = "acme";
/// The integration and the provider of the team chat API.
pub const CHAT_INTEGRATION: &str = "team-chat";
/// The connection of the chat API.
pub const CHAT_CONNECTION: &str = "team";
/// The chat endpoint that the `notify` route calls.
pub const CHAT_MESSAGES_URL: &str = "https://chat.example/v1/messages";
/// The URL of the Jira site of the example. It is the `url` of the site in
/// `accessible-resources`, and each delivery names it in `issue.self`.
pub const SITE_URL: &str = "https://acme.atlassian.net";
/// The project of the incidents.
pub const PROJECT: &str = "OPS";
/// The account id of the project lead. The escape hatch reads it.
pub const LEAD: &str = "acct-lead";

/// The OAuth client of the `atlassian` integration.
const CLIENT_ID: &str = "ops-client";
/// The access token that san-mateo holds at the start. It is expired.
const OLD_ACCESS_TOKEN: &str = "acme-at-old";
/// The refresh token that san-mateo holds at the start.
const REFRESH_TOKEN: &str = "acme-rt-0";

/// The team chat API of the example, in the Nango `providers.yaml` format.
/// The example has its own text for it. It is not a copy of a Nango
/// provider. The API key goes in `x-api-key`.
pub const CHAT_YAML: &str = r"
team-chat:
    display_name: Team chat
    auth_mode: API_KEY
    proxy:
        base_url: https://chat.example
        headers:
            x-api-key: ${apiKey}
    docs: https://chat.example/docs
";

/// The fakes of one run. Clones share state.
#[derive(Clone)]
pub struct Sites {
    /// Jira Cloud, the Atlassian OAuth server and the discovery.
    pub jira: FakeJira,
    /// The team chat API.
    pub chat: MemoryTransport,
}

impl Sites {
    /// The Jira site of the example: project OPS with lead `acct-lead`,
    /// OAuth on, signed webhooks with `webhook_secret`, and the chat fake
    /// for all other hosts.
    pub fn new(webhook_secret: &str, client_secret: &str) -> Self {
        let chat = MemoryTransport::new();
        chat.reply(
            "POST",
            CHAT_MESSAGES_URL,
            HttpResponse::json(201, &json!({"id": "msg-1"})),
        );
        let project = FakeProject::new(PROJECT, "10000", "Operations")
            .issue_type("10001", "Task")
            .issue_type("10002", "Incident")
            .issue_type("10003", "Bug")
            .lead(LEAD);
        // The fake gives out `OLD_ACCESS_TOKEN` as live. The import below
        // marks it as expired, so san-mateo refreshes it before the first
        // call (timeline line 2).
        let jira = FakeJira::new()
            .with_project(project)
            .with_oauth(CLIENT_ID, client_secret, OLD_ACCESS_TOKEN, REFRESH_TOKEN)
            .with_webhook_secret(webhook_secret)
            .with_fallback(Arc::new(chat.clone()));
        Self { jira, chat }
    }
}

/// The key of the Jira connection.
pub fn jira_key() -> ConnectionKey {
    ConnectionKey::new(JIRA_INTEGRATION, JIRA_CONNECTION)
}

/// Builds the one `SanMateo` of the example and adds the two integrations
/// and their connections.
///
/// In production, use `PgStore` (all workers share one store) and read the
/// keys from a secret store. Here the keys are new for each run.
pub async fn san_mateo(sites: &Sites, client_secret: &str) -> Result<SanMateo, BoxError> {
    let mut key = [0_u8; 32];
    OsEntropy.fill(&mut key);
    let catalog = Catalog::builtin()?.merge(Catalog::from_yaml_str(CHAT_YAML)?);
    let san = SanMateo::builder()
        .environment(EnvironmentConfig::new(
            ENV,
            format!("sk_ops_{}", OsEntropy.hex(16)),
        ))
        .key_ring(KeyRing::new("k1", key)?)
        .catalog(catalog)
        .transport(Arc::new(sites.jira.clone()))
        .proxy_settings(ProxySettings {
            initial_backoff_ms: 50,
            ..ProxySettings::default()
        })
        .build()?;
    san.create_integration(
        ENV,
        NewIntegration {
            unique_key: JIRA_INTEGRATION.to_owned(),
            provider: "atlassian".to_owned(),
            client_id: Some(CLIENT_ID.to_owned()),
            client_secret: Some(Secret::new(client_secret)),
            ..NewIntegration::default()
        },
    )
    .await?;
    // An expired access token and a refresh token, as after a long pause.
    let expired = autumn_harvest::chrono::Utc::now() - autumn_harvest::chrono::Duration::hours(1);
    san.import_connection(
        ENV,
        connection(
            JIRA_CONNECTION,
            JIRA_INTEGRATION,
            Credentials::OAuth2(OAuth2Credentials {
                access_token: Secret::new(OLD_ACCESS_TOKEN),
                refresh_token: Some(Secret::new(REFRESH_TOKEN)),
                expires_at: Some(expired),
                raw: json!({}),
            }),
        ),
    )
    .await?;
    san.create_integration(
        ENV,
        NewIntegration {
            unique_key: CHAT_INTEGRATION.to_owned(),
            provider: CHAT_INTEGRATION.to_owned(),
            ..NewIntegration::default()
        },
    )
    .await?;
    san.import_connection(
        ENV,
        connection(
            CHAT_CONNECTION,
            CHAT_INTEGRATION,
            Credentials::ApiKey {
                api_key: Secret::new(format!("chat-key-{}", OsEntropy.hex(8))),
            },
        ),
    )
    .await?;
    Ok(san)
}

/// An imported connection with no config, metadata or tags.
fn connection(id: &str, integration: &str, credentials: Credentials) -> ImportConnection {
    ImportConnection {
        connection_id: Some(id.to_owned()),
        provider_config_key: integration.to_owned(),
        credentials,
        connection_config: Map::new(),
        metadata: Map::new(),
        tags: Tags::new(),
        end_user: None,
        webhook_url_override: None,
    }
}

/// The target of the Jira connection. The site URL picks the site in
/// `accessible-resources`, so the discovery fails when the connection has
/// no site with [`SITE_URL`].
pub fn jira_target() -> SanMateoTarget {
    SanMateoTarget::new(ENV, jira_key()).site_url(SITE_URL)
}

/// The host of [`SITE_URL`], for the site check of the bridge.
///
/// A san-mateo client tells the bridge no host. With this host, the bridge
/// refuses a delivery whose `issue.self` names another site
/// (R2F-fixcheck-a-2). The signal of a delivery of its own site then holds
/// the status of the payload, so a short status still ends a wait
/// (R2C-harvest-4).
///
/// # Errors
///
/// [`SITE_URL`] has no good host.
pub fn site_host() -> Result<SiteHost, BoxError> {
    SiteHost::from_url(SITE_URL).ok_or_else(|| format!("no site host in {SITE_URL}").into())
}

/// Reads `accessible-resources` through san-mateo and stores the cloud id
/// in the connection metadata. Returns the stored value.
pub async fn discover_cloud_id(san: &SanMateo) -> Result<String, BoxError> {
    let id = SanMateoBackend::new(san.clone(), jira_target())
        .discover_cloud_id()
        .await?;
    let stored = stored_cloud_id(san).await?;
    if stored.as_deref() != Some(id.as_str()) {
        return Err("the discovery did not store the cloud id".into());
    }
    Ok(id.as_str().to_owned())
}

/// The `cloudId` in the metadata of the Jira connection.
pub async fn stored_cloud_id(san: &SanMateo) -> Result<Option<String>, BoxError> {
    let options = GetOptions {
        refresh_if_needed: false,
        ..GetOptions::default()
    };
    let conn = san.get_connection(ENV, &jira_key(), options).await?;
    Ok(conn
        .metadata
        .get("cloudId")
        .and_then(Value::as_str)
        .map(str::to_owned))
}

/// The Jira client through san-mateo, with short waits for the example.
pub fn jira_client(san: &SanMateo) -> JiraClient {
    let mut settings = ClientSettings::default();
    settings.base_backoff_ms = 50;
    JiraClient::san_mateo(san.clone(), jira_target()).with_settings(settings)
}

/// The client of a durable host (a camel route under harvest): harvest
/// owns the retries, so the client makes few attempts.
pub fn durable_client(san: &SanMateo) -> JiraClient {
    jira_client(san).durable()
}

/// A short pause for the polls of the driver.
pub const TICK: Duration = Duration::from_millis(50);

//! Helpers for the plugin tests: a client on `FakeJira`, the autumn config
//! with the Jira endpoint, signed deliveries and a counting transport.

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers: a failure is a test failure"
)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use autumn_jira::client::ClientSettings;
use autumn_jira::ids::SiteName;
use autumn_jira::san_mateo::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use autumn_jira::san_mateo::{
    ConnectionKey, Credentials, EnvironmentConfig, ImportConnection, KeyRing, NewIntegration,
    ProxySettings, SanMateo, Secret,
};
use autumn_jira::{BasicAuth, JiraClient, JiraSite};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira};
use autumn_plugin_jira::{JiraConfig, JiraPlugin, webhook_endpoint};
use autumn_web::config::AutumnConfig;
use autumn_web::test::{TestApp, TestClient, TestResponse};
use autumn_web::webhook::hmac_sha256_hex;
use serde_json::{Map, Value, json};

/// The webhook secret of the tests. autumn needs 32 bytes or more in prod.
pub const SECRET: &str = "plugin-test-webhook-secret-0123456789";
/// The webhook path.
pub const PATH: &str = "/jira/webhook";
/// The san-mateo environment.
pub const ENV: &str = "dev";
/// The `jira-basic` integration key.
pub const INTEGRATION: &str = "jira";
/// The connection id of the default tenant.
pub const CONNECTION: &str = "acme";

/// Settings for tests: small waits, no jitter.
pub fn fast_settings() -> ClientSettings {
    let mut s = ClientSettings::default();
    s.base_backoff_ms = 10;
    s.max_backoff_ms = 30_000;
    s.lookup_delay_ms = 0;
    s.jitter = false;
    s
}

/// An API token client on the fake.
pub fn client_on(transport: Arc<dyn HttpTransport>) -> JiraClient {
    JiraClient::api_token(
        JiraSite::Subdomain(SiteName::parse("acme").unwrap()),
        BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN)).unwrap(),
        transport,
    )
    .with_settings(fast_settings())
}

/// A plugin with a prebuilt client on `fake` and no mode table.
pub fn plugin(fake: &FakeJira) -> JiraPlugin {
    JiraPlugin::with_config(JiraConfig::default()).client(client_on(Arc::new(fake.clone())))
}

/// The `[jira]` config with an API token table.
pub fn api_token_config(email_env: &str, token_env: &str) -> JiraConfig {
    JiraConfig::from_toml_str(&format!(
        "[jira.api_token]\nsite = \"acme\"\nemail_env = \"{email_env}\"\ntoken_env = \"{token_env}\"\n"
    ))
    .unwrap()
}

/// The `[jira]` config in san_mateo mode.
pub fn san_mateo_config(connection: Option<&str>) -> JiraConfig {
    let connection = connection.map_or_else(String::new, |c| format!("connection_id = \"{c}\"\n"));
    JiraConfig::from_toml_str(&format!(
        "[jira.san_mateo]\nenvironment = \"{ENV}\"\nintegration = \"{INTEGRATION}\"\n{connection}"
    ))
    .unwrap()
}

/// The autumn config with the Jira endpoint at `/jira/webhook`.
pub fn autumn_config() -> AutumnConfig {
    let mut cfg = AutumnConfig::default();
    let mut endpoint = webhook_endpoint(PATH, "JIRA_WEBHOOK_SECRET");
    endpoint.secret = Some(SECRET.to_owned());
    cfg.security.webhooks.endpoints.push(endpoint);
    cfg
}

/// The `X-Hub-Signature` value for `body`.
pub fn signature(body: &[u8]) -> String {
    format!("sha256={}", hmac_sha256_hex(SECRET.as_bytes(), body))
}

/// Sends a signed delivery with the id `id`.
pub async fn deliver(client: &TestClient, id: &str, body: &[u8]) -> TestResponse {
    client
        .post(PATH)
        .header("X-Hub-Signature", &signature(body))
        .header("X-Atlassian-Webhook-Identifier", id)
        .header("content-type", "application/json")
        .body(body.to_vec())
        .send()
        .await
}

/// The time now in epoch ms.
pub fn now_ms() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(since.as_millis()).unwrap()
}

/// An `jira:issue_updated` body at `timestamp`, by `actor`.
pub fn issue_updated(timestamp: Option<i64>, actor: &str) -> Vec<u8> {
    let mut body = json!({
        "webhookEvent": "jira:issue_updated",
        "user": {"accountId": actor},
        "issue": {"id": "10001", "key": "OPS-1", "fields": {"summary": "Disk full"}},
        "changelog": {"id": "1", "items": [{"field": "status", "fromString": "To Do", "toString": "Done", "from": "10000", "to": "10002"}]},
    });
    if let Some(ts) = timestamp {
        body["timestamp"] = json!(ts);
    }
    body.to_string().into_bytes()
}

/// Builds a `TestApp`. Returns the panic text when the start fails.
pub fn boot(build: impl FnOnce() -> TestClient) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| drop(build()))).map_err(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default()
    })
}

/// A `TestApp` with the endpoint and the plugin.
pub fn app(plugin: JiraPlugin) -> TestClient {
    TestApp::new()
        .config(autumn_config())
        .plugin(plugin)
        .build()
}

/// Counts the requests, then sends them to the inner transport.
#[derive(Clone)]
pub struct Counting {
    inner: Arc<dyn HttpTransport>,
    count: Arc<AtomicUsize>,
}

impl Counting {
    /// Wraps `inner`.
    pub fn new(inner: Arc<dyn HttpTransport>) -> Self {
        Self {
            inner,
            count: Arc::default(),
        }
    }

    /// The requests so far.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HttpTransport for Counting {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.inner.send(request).await
    }
}

/// A san-mateo facade over a transport.
pub fn san_mateo(transport: Arc<dyn HttpTransport>) -> SanMateo {
    SanMateo::builder()
        .environment(EnvironmentConfig::new(ENV, "sm-secret-key-0123456789"))
        .key_ring(KeyRing::new("k1", [42; 32]).unwrap())
        .transport(transport)
        .proxy_settings(ProxySettings {
            initial_backoff_ms: 0,
            max_wait_ms: 5_000,
            ..ProxySettings::default()
        })
        .build()
        .unwrap()
}

/// Adds the `jira-basic` integration and a connection for the default site.
pub async fn basic_connection(san: &SanMateo, connection_id: &str) -> ConnectionKey {
    let _ = san
        .create_integration(
            ENV,
            NewIntegration {
                unique_key: INTEGRATION.to_owned(),
                provider: "jira-basic".to_owned(),
                ..NewIntegration::default()
            },
        )
        .await;
    san.import_connection(
        ENV,
        ImportConnection {
            connection_id: Some(connection_id.to_owned()),
            provider_config_key: INTEGRATION.to_owned(),
            credentials: Credentials::Basic {
                username: DEFAULT_EMAIL.to_owned(),
                password: Secret::new(DEFAULT_API_TOKEN),
            },
            connection_config: Map::from_iter([("subdomain".to_owned(), json!("acme"))]),
            metadata: Map::new(),
            tags: autumn_jira::san_mateo::Tags::new(),
            end_user: None,
            webhook_url_override: None,
        },
    )
    .await
    .unwrap()
    .key()
}

/// The JSON body of a response.
pub fn json_of(r: &TestResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

//! Helpers for the client tests: san-mateo setups, settings and a
//! scripted backend.

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers: a failure is a test failure"
)]

pub mod suite;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use autumn_jira::backend::{
    ApiRequest, ApiResponse, BackendError, BackendInfo, DirectBackend, JiraBackend,
};
use autumn_jira::client::ClientSettings;
use autumn_jira::ids::{CloudId, SiteName};
use autumn_jira::san_mateo::credentials::OAuth2Credentials;
use autumn_jira::san_mateo::transport::HttpTransport;
use autumn_jira::san_mateo::{
    ConnectionKey, Credentials, EnvironmentConfig, ImportConnection, KeyRing, NewIntegration,
    ProxySettings, SanMateo, Secret,
};
use autumn_jira::{BasicAuth, JiraClient, JiraSite};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_CLOUD_ID, DEFAULT_EMAIL, FakeJira};
use serde_json::{Map, Value, json};

/// The san-mateo environment of the tests.
pub const ENV: &str = "dev";
/// The `jira-basic` integration key.
pub const BASIC_INTEGRATION: &str = "jira";
/// The `atlassian` integration key.
pub const OAUTH_INTEGRATION: &str = "atl";
/// The connection id of each test connection.
pub const CONNECTION: &str = "acme";
/// The OAuth client of the fake.
pub const CLIENT_ID: &str = "cid-1";
/// The OAuth client secret of the fake.
pub const CLIENT_SECRET: &str = "csecret-1";
/// The first access token of the fake.
pub const ACCESS_TOKEN: &str = "at-0";
/// The first refresh token of the fake.
pub const REFRESH_TOKEN: &str = "rt-0";

/// Settings for tests: small waits, no jitter.
pub fn fast_settings() -> ClientSettings {
    let mut s = ClientSettings::default();
    s.base_backoff_ms = 50;
    s.max_backoff_ms = 30_000;
    s.lookup_delay_ms = 100;
    s.jitter = false;
    s
}

/// The default site of the fake.
pub fn site() -> JiraSite {
    JiraSite::Subdomain(SiteName::parse("acme").unwrap())
}

/// The default cloud id of the fake.
pub fn cloud_id() -> CloudId {
    CloudId::parse(DEFAULT_CLOUD_ID).unwrap()
}

/// The default Basic auth of the fake.
pub fn auth() -> BasicAuth {
    BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN)).unwrap()
}

/// A `DirectBackend` over any transport.
pub fn direct(transport: Arc<dyn HttpTransport>) -> DirectBackend {
    DirectBackend::new(site(), auth(), transport)
}

/// An API token client over a fake, with the fast settings.
pub fn api_token_client(fake: &FakeJira) -> JiraClient {
    JiraClient::api_token(site(), auth(), Arc::new(fake.clone())).with_settings(fast_settings())
}

/// A san-mateo facade over a transport.
pub fn san_mateo(transport: Arc<dyn HttpTransport>) -> SanMateo {
    SanMateo::builder()
        .environment(EnvironmentConfig::new(ENV, "sm-secret-key"))
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

/// Adds a `jira-basic` integration and connection for the default site.
pub async fn basic_connection(san: &SanMateo) -> ConnectionKey {
    san.create_integration(
        ENV,
        NewIntegration {
            unique_key: BASIC_INTEGRATION.to_owned(),
            provider: "jira-basic".to_owned(),
            ..NewIntegration::default()
        },
    )
    .await
    .unwrap();
    san.import_connection(
        ENV,
        ImportConnection {
            connection_id: Some(CONNECTION.to_owned()),
            provider_config_key: BASIC_INTEGRATION.to_owned(),
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

/// Adds an `atlassian` integration and an OAuth connection with these
/// metadata.
pub async fn oauth_connection(san: &SanMateo, metadata: Map<String, Value>) -> ConnectionKey {
    san.create_integration(
        ENV,
        NewIntegration {
            unique_key: OAUTH_INTEGRATION.to_owned(),
            provider: "atlassian".to_owned(),
            client_id: Some(CLIENT_ID.to_owned()),
            client_secret: Some(Secret::new(CLIENT_SECRET)),
            ..NewIntegration::default()
        },
    )
    .await
    .unwrap();
    san.import_connection(
        ENV,
        ImportConnection {
            connection_id: Some(CONNECTION.to_owned()),
            provider_config_key: OAUTH_INTEGRATION.to_owned(),
            credentials: Credentials::OAuth2(OAuth2Credentials {
                access_token: Secret::new(ACCESS_TOKEN),
                refresh_token: Some(Secret::new(REFRESH_TOKEN)),
                expires_at: None,
                raw: json!({}),
            }),
            connection_config: Map::new(),
            metadata,
            tags: autumn_jira::san_mateo::Tags::new(),
            end_user: None,
            webhook_url_override: None,
        },
    )
    .await
    .unwrap()
    .key()
}

/// Turns on OAuth on a fake with the test client and tokens.
pub fn with_oauth(fake: FakeJira) -> FakeJira {
    fake.with_oauth(CLIENT_ID, CLIENT_SECRET, ACCESS_TOKEN, REFRESH_TOKEN)
}

/// A hook of a [`Scripted`] backend: it gets the request and the number
/// of earlier requests. `Some` answers in place of the inner backend.
pub type Hook =
    dyn Fn(&ApiRequest, usize) -> Option<Result<ApiResponse, BackendError>> + Send + Sync + 'static;

/// A backend that answers some requests itself and sends the others to an
/// inner backend. It records each request.
pub struct Scripted {
    inner: Arc<dyn JiraBackend>,
    hook: Box<Hook>,
    seen: AtomicUsize,
    log: Mutex<Vec<ApiRequest>>,
}

impl Scripted {
    /// Makes the backend.
    pub fn new(
        inner: Arc<dyn JiraBackend>,
        hook: impl Fn(&ApiRequest, usize) -> Option<Result<ApiResponse, BackendError>>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            hook: Box::new(hook),
            seen: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
        })
    }

    /// Returns each request, in order.
    pub fn requests(&self) -> Vec<ApiRequest> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait]
impl JiraBackend for Scripted {
    async fn send(&self, request: ApiRequest) -> Result<ApiResponse, BackendError> {
        let n = self.seen.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push(request.clone());
        if let Some(answer) = (self.hook)(&request, n) {
            return answer;
        }
        self.inner.send(request).await
    }

    fn info(&self) -> BackendInfo {
        self.inner.info()
    }
}

/// A JSON reply.
pub fn json_reply(status: u16, value: &Value) -> ApiResponse {
    ApiResponse {
        status,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body: value.to_string().into_bytes(),
    }
}

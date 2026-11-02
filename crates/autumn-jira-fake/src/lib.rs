//! `autumn_jira_fake`: a stateful in-memory Jira Cloud for tests.

mod sign;
mod types;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use san_mateo::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};

pub use sign::sign;
pub use types::{FakeDelivery, FakeError, FakeOp, FakeProject, FakeRequest, Fault};

/// The default site subdomain.
pub const DEFAULT_SITE: &str = "acme";
/// The default cloud id.
pub const DEFAULT_CLOUD_ID: &str = "11111111-2222-4333-8444-555555555555";
/// The default email of the Basic auth user (the bot).
pub const DEFAULT_EMAIL: &str = "bot@acme.example";
/// The default API token of the Basic auth user (the bot).
pub const DEFAULT_API_TOKEN: &str = "fake-api-token";
/// The account id of ann, a person.
pub const ANN: &str = "acct-ann";
/// The account id of the bot, the API account.
pub const BOT: &str = "acct-bot";

/// A stateful in-memory Jira Cloud. Clones share state.
#[derive(Clone, Default)]
pub struct FakeJira {}

impl FakeJira {
    /// Makes the default fake.
    #[must_use]
    pub const fn new() -> Self {
        Self {}
    }
    /// Sets the site.
    #[must_use]
    pub const fn with_site(self, _subdomain: &str, _cloud_id: &str) -> Self {
        self
    }
    /// Sets Basic auth.
    #[must_use]
    pub const fn with_basic_auth(self, _email: &str, _token: &str) -> Self {
        self
    }
    /// Sets OAuth.
    #[must_use]
    pub const fn with_oauth(self, _a: &str, _b: &str, _c: &str, _d: &str) -> Self {
        self
    }
    /// Adds a project.
    #[must_use]
    pub fn with_project(self, _project: FakeProject) -> Self {
        self
    }
    /// Sets the webhook secret.
    #[must_use]
    pub const fn with_webhook_secret(self, _secret: &str) -> Self {
        self
    }
    /// Sets the fallback.
    #[must_use]
    pub fn with_fallback(self, _other: Arc<dyn HttpTransport>) -> Self {
        self
    }
    /// Handles one request.
    ///
    /// # Errors
    ///
    /// A transport error.
    pub fn handle(&self, _request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        Err(TransportError::Connect(String::new()))
    }
    /// Queues a fault.
    pub const fn fault(&self, _op: FakeOp, _fault: Fault) {}
    /// Queues 429 replies.
    pub const fn rate_limit(&self, _op: FakeOp, _times: usize, _retry_after_secs: u64) {}
    /// Sets the index lag.
    pub const fn index_lag(&self, _lag: Duration) {}
    /// Takes labels off the create screen.
    pub const fn labels_not_on_screen(&self, _project: &str) {}
    /// Expires the access tokens.
    pub const fn expire_access_token(&self) {}
    /// A person transitions an issue.
    ///
    /// # Errors
    ///
    /// No issue or no transition.
    pub const fn human_transition(
        &self,
        _key: &str,
        _status_name: &str,
        _actor: &str,
    ) -> Result<(), FakeError> {
        Ok(())
    }
    /// A person comments.
    ///
    /// # Errors
    ///
    /// No issue.
    pub const fn human_comment(
        &self,
        _key: &str,
        _text: &str,
        _actor: &str,
    ) -> Result<(), FakeError> {
        Ok(())
    }
    /// A person edits labels.
    ///
    /// # Errors
    ///
    /// No issue.
    pub const fn human_edit_labels(
        &self,
        _key: &str,
        _add: &[&str],
        _remove: &[&str],
        _actor: &str,
    ) -> Result<(), FakeError> {
        Ok(())
    }
    /// Takes the deliveries.
    #[must_use]
    pub const fn take_deliveries(&self) -> Vec<FakeDelivery> {
        Vec::new()
    }
    /// Returns the requests.
    #[must_use]
    pub const fn requests(&self) -> Vec<FakeRequest> {
        Vec::new()
    }
    /// Returns the call count of an op.
    #[must_use]
    pub const fn count(&self, _op: FakeOp) -> usize {
        0
    }
    /// Returns one issue.
    #[must_use]
    pub const fn issue(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
    /// Returns all issues.
    #[must_use]
    pub const fn issues(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }
    /// Returns the refresh count.
    #[must_use]
    pub const fn token_refreshes(&self) -> usize {
        0
    }
    /// Returns a snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

#[async_trait]
impl HttpTransport for FakeJira {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.handle(&request)
    }
}

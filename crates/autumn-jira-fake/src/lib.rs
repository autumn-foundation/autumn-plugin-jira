//! `autumn_jira_fake`: a stateful in-memory Jira Cloud for tests.
//!
//! [`FakeJira`] holds projects, issues, a workflow, labels, properties,
//! comments, a JQL subset, OAuth tokens and signed webhook deliveries. Its
//! core is the sans-IO [`FakeJira::handle`]: one request in, one reply out.
//! It also implements [`san_mateo::transport::HttpTransport`], so it sits
//! under any client that sends through that trait. It has no network
//! server.
//!
//! The fake reads the Jira wire format from the spec on its own. It shares
//! no code with the `autumn-jira` client, so the two readings of the spec
//! stay independent (ADR 0007).
//!
//! Hosts:
//!
//! - `https://{site}.atlassian.net/rest/api/3/...` takes Basic auth.
//! - `https://api.atlassian.com/ex/jira/{cloudId}/rest/api/3/...` takes a
//!   Bearer token or Basic auth.
//! - `https://api.atlassian.com/oauth/token/accessible-resources` takes a
//!   Bearer token.
//! - `https://auth.atlassian.com/oauth/token` refreshes and rotates tokens.
//! - Other hosts go to the fallback transport, else they fail to connect.
//!
//! ```
//! use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp, Fault};
//! use base64::Engine as _;
//! use san_mateo::transport::HttpRequest;
//!
//! let fake = FakeJira::new().with_webhook_secret("s3cret");
//! let auth = base64::engine::general_purpose::STANDARD
//!     .encode(format!("{DEFAULT_EMAIL}:{DEFAULT_API_TOKEN}"));
//! let body = r#"{"fields": {"project": {"key": "OPS"},
//!     "issuetype": {"name": "Task"}, "summary": "Disk full"}}"#;
//! let create = HttpRequest::new("POST", "https://acme.atlassian.net/rest/api/3/issue")
//!     .header("authorization", format!("Basic {auth}"))
//!     .header("content-type", "application/json")
//!     .body(body.as_bytes().to_vec());
//!
//! // The write happens, but the reply is lost.
//! fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
//! assert_eq!(fake.handle(&create).unwrap().status, 503);
//! assert_eq!(fake.issue("OPS-1").unwrap()["fields"]["summary"], "Disk full");
//! assert_eq!(fake.count(FakeOp::CreateIssue), 1);
//! assert_eq!(fake.take_deliveries().len(), 1);
//! ```

mod api;
mod jql;
mod oauth;
mod render;
mod router;
mod sign;
mod state;
mod types;
mod webhook;
mod wire;
mod workflow;

use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use san_mateo::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use serde_json::{Value, json};

use crate::router::Served;
use crate::state::{OAuth, Planned, State};

pub use sign::sign;
pub use types::{FakeDelivery, FakeError, FakeOp, FakeProject, FakeRequest, Fault};

/// The default site subdomain: `acme.atlassian.net`.
pub const DEFAULT_SITE: &str = "acme";
/// The default cloud id.
pub const DEFAULT_CLOUD_ID: &str = "11111111-2222-4333-8444-555555555555";
/// The default email of the Basic auth user (the bot).
pub const DEFAULT_EMAIL: &str = "bot@acme.example";
/// The default API token of the Basic auth user (the bot).
pub const DEFAULT_API_TOKEN: &str = "fake-api-token";
/// The account id of ann, a person. Person actions can also use `"ann"`.
pub const ANN: &str = "acct-ann";
/// The account id of the bot, the API account. Every API call acts as the
/// bot. Person actions can also use `"bot"`.
pub const BOT: &str = "acct-bot";

/// A stateful in-memory Jira Cloud. Clones share state.
#[derive(Clone)]
pub struct FakeJira {
    state: Arc<Mutex<State>>,
}

impl Default for FakeJira {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FakeJira {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("FakeJira")
            .field("site", &state.site)
            .field("issues", &state.issues.len())
            .finish_non_exhaustive()
    }
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "each call holds the lock for the whole call, so each call is atomic"
)]
impl FakeJira {
    /// Site "acme", cloud id `11111111-2222-4333-8444-555555555555`, project
    /// OPS (issue types Task 10001, Incident 10002, Bug 10003), users ann
    /// (a person) and bot (the API account), and the default workflow.
    ///
    /// Basic auth takes [`DEFAULT_EMAIL`] and [`DEFAULT_API_TOKEN`]. OAuth
    /// and webhooks are off.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::new())),
        }
    }

    /// Locks the state. A panic in another test thread does not block it.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the state and applies the delayed writes that are due.
    fn now(&self) -> MutexGuard<'_, State> {
        let mut state = self.lock();
        state.tick();
        state
    }

    /// Sets the site subdomain and the cloud id.
    #[must_use]
    pub fn with_site(self, subdomain: &str, cloud_id: &str) -> Self {
        {
            let mut state = self.lock();
            subdomain.clone_into(&mut state.site);
            cloud_id.clone_into(&mut state.cloud_id);
        }
        self
    }

    /// Sets the one Basic auth user: an email and an API token.
    #[must_use]
    pub fn with_basic_auth(self, email: &str, token: &str) -> Self {
        self.lock().basic = (email.to_owned(), token.to_owned());
        self
    }

    /// Turns on OAuth with one client, a live access token and a refresh
    /// token.
    #[must_use]
    pub fn with_oauth(
        self,
        client_id: &str,
        client_secret: &str,
        access_token: &str,
        refresh_token: &str,
    ) -> Self {
        self.lock().oauth = Some(OAuth {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            access: BTreeSet::from([access_token.to_owned()]),
            expired: BTreeSet::new(),
            refresh: refresh_token.to_owned(),
            issued: 0,
        });
        self
    }

    /// Adds a project, or replaces the project with the same key.
    #[must_use]
    pub fn with_project(self, project: FakeProject) -> Self {
        {
            let mut state = self.lock();
            match state.projects.iter_mut().find(|p| p.key == project.key) {
                Some(old) => *old = project,
                None => state.projects.push(project),
            }
        }
        self
    }

    /// Turns on webhooks. Each change then queues a signed delivery.
    #[must_use]
    pub fn with_webhook_secret(self, secret: &str) -> Self {
        self.lock().webhook_secret = Some(Arc::from(secret.as_bytes()));
        self
    }

    /// Sends requests for other hosts and other sites to `other`. Only the
    /// async [`HttpTransport::send`] uses it.
    #[must_use]
    pub fn with_fallback(self, other: Arc<dyn HttpTransport>) -> Self {
        self.lock().fallback = Some(other);
        self
    }

    /// Sans-IO core: one request in, one reply out.
    ///
    /// The fallback transport is not used here: a request for another host
    /// fails to connect.
    ///
    /// # Errors
    ///
    /// `TransportError::Invalid` for a URL that is not `https`,
    /// `TransportError::Connect` for another host, and the transport error
    /// of a planned fault.
    pub fn handle(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let served = self.lock().serve(request, false);
        match served {
            Served::Done(reply) => reply,
            Served::Delegate(_) => Err(TransportError::Connect(
                "the sans-IO core has no fallback".to_owned(),
            )),
        }
    }

    /// Plans a fault for the next call of `op`. Faults of one op are FIFO.
    pub fn fault(&self, op: FakeOp, fault: Fault) {
        self.plan(op, Planned::Fault(fault), 1);
    }

    /// Plans `times` replies of 429 with `Retry-After: retry_after_secs`
    /// for the next calls of `op`. No write happens.
    pub fn rate_limit(&self, op: FakeOp, times: usize, retry_after_secs: u64) {
        self.plan(op, Planned::RateLimit(retry_after_secs), times);
    }

    fn plan(&self, op: FakeOp, planned: Planned, times: usize) {
        self.lock()
            .faults
            .entry(op)
            .or_default()
            .extend(std::iter::repeat_n(planned, times));
    }

    /// Sets the search index lag in tokio time. A new or changed issue is
    /// visible to search only after `lag`. Get issue, properties and the
    /// comment list are always current.
    pub fn index_lag(&self, lag: Duration) {
        self.now().lag = lag;
    }

    /// Takes the Labels field off the create screen of a project. A create
    /// with labels then gives 400 with `errors.labels`.
    pub fn labels_not_on_screen(&self, project: &str) {
        self.lock().labels_off.insert(project.to_owned());
    }

    /// Ends every live access token. The next Bearer call gives 401.
    pub fn expire_access_token(&self) {
        let mut state = self.lock();
        if let Some(oauth) = state.oauth.as_mut() {
            let live = std::mem::take(&mut oauth.access);
            oauth.expired.extend(live);
        }
    }

    /// A person moves an issue to a status in the Jira UI. A transition to
    /// Done gets the resolution Done. It queues a signed delivery.
    ///
    /// # Errors
    ///
    /// [`FakeError::NoIssue`] for an unknown key, [`FakeError::NoTransition`]
    /// when the workflow has no transition from the current status.
    pub fn human_transition(
        &self,
        key: &str,
        status_name: &str,
        actor: &str,
    ) -> Result<(), FakeError> {
        let mut state = self.now();
        let id = issue_id(&state, key)?;
        let current = state.issues[&id].status;
        let t = workflow::transitions_from(current)
            .find(|t| {
                workflow::status(t.to)
                    .name
                    .eq_ignore_ascii_case(status_name)
            })
            .ok_or_else(|| FakeError::NoTransition(status_name.to_owned()))?;
        let resolution = t.needs_resolution.then_some(workflow::RESOLUTIONS[0].0);
        api::transition_issue(&mut state, id, t, resolution, &workflow::account_for(actor));
        Ok(())
    }

    /// A person adds a comment in the Jira UI. Each line becomes a
    /// paragraph. It queues a signed delivery.
    ///
    /// # Errors
    ///
    /// [`FakeError::NoIssue`] for an unknown key.
    pub fn human_comment(&self, key: &str, text: &str, actor: &str) -> Result<(), FakeError> {
        let mut state = self.now();
        let id = issue_id(&state, key)?;
        let author = workflow::account_for(actor);
        api::add_comment(&mut state, id, render::adf_from_text(text), &author, true);
        Ok(())
    }

    /// A person adds and removes labels in the Jira UI. It queues a signed
    /// delivery.
    ///
    /// # Errors
    ///
    /// [`FakeError::NoIssue`] for an unknown key.
    pub fn human_edit_labels(
        &self,
        key: &str,
        add: &[&str],
        remove: &[&str],
        actor: &str,
    ) -> Result<(), FakeError> {
        let mut state = self.now();
        let id = issue_id(&state, key)?;
        api::edit_labels(&mut state, id, add, remove, &workflow::account_for(actor));
        Ok(())
    }

    /// Takes the queued deliveries, oldest first.
    #[must_use]
    pub fn take_deliveries(&self) -> Vec<FakeDelivery> {
        std::mem::take(&mut self.now().deliveries)
    }

    /// Returns the requests that this fake answered, oldest first. They hold
    /// no headers, so no auth.
    #[must_use]
    pub fn requests(&self) -> Vec<FakeRequest> {
        self.lock().requests.clone()
    }

    /// Returns the number of calls of an op, faults and 401 replies too.
    #[must_use]
    pub fn count(&self, op: FakeOp) -> usize {
        self.lock().counts.get(&op).copied().unwrap_or_default()
    }

    /// Returns the current issue bean with every field and property.
    #[must_use]
    pub fn issue(&self, key: &str) -> Option<Value> {
        let state = self.now();
        let id = state.find_issue(key)?;
        Some(render::full_issue(&state, &state.issues[&id]))
    }

    /// Returns every current issue bean, in id order.
    #[must_use]
    pub fn issues(&self) -> Vec<Value> {
        let state = self.now();
        state
            .issues
            .values()
            .map(|i| render::full_issue(&state, i))
            .collect()
    }

    /// Returns the number of successful token refreshes.
    #[must_use]
    pub fn token_refreshes(&self) -> usize {
        self.now().refreshes
    }

    /// Returns the issues, comments, labels and properties. It has no times
    /// and no URLs, so equal histories give equal snapshots.
    #[must_use]
    pub fn snapshot(&self) -> Value {
        let state = self.now();
        let issues: Vec<Value> = state
            .issues
            .values()
            .map(|issue| {
                let comments: Vec<Value> = issue
                    .comments
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id.to_string(),
                            "author": c.author,
                            "text": render::adf_text(&c.body),
                        })
                    })
                    .collect();
                json!({
                    "id": issue.id.to_string(),
                    "key": issue.key,
                    "project": issue.project,
                    "issueType": state.issue_type_of(issue).name,
                    "summary": issue.summary,
                    "status": issue.status().name,
                    "resolution": issue.resolution.and_then(workflow::resolution).map(|r| r.1),
                    "labels": issue.labels,
                    "assignee": issue.assignee,
                    "properties": issue.properties,
                    "comments": comments,
                })
            })
            .collect();
        json!({
            "site": state.site,
            "cloudId": state.cloud_id,
            "issues": issues,
        })
    }
}

/// Returns the id of the issue with this key, for a person action.
fn issue_id(state: &State, key: &str) -> Result<u64, FakeError> {
    state
        .find_issue(key)
        .ok_or_else(|| FakeError::NoIssue(key.to_owned()))
}

#[async_trait]
impl HttpTransport for FakeJira {
    /// Yields to tokio once, then serves the request like
    /// [`FakeJira::handle`]. A request for another host or site goes to the
    /// fallback transport, when there is one.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        tokio::task::yield_now().await;
        let served = self.lock().serve(&request, true);
        match served {
            Served::Done(reply) => reply,
            Served::Delegate(other) => other.send(request).await,
        }
    }
}

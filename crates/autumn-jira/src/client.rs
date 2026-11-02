//! `JiraClient`: the typed Jira Cloud REST v3 client.
//!
//! Each call sends attempts through a [`JiraBackend`] and decides each
//! next step with the verified policy ([`crate::policy`]). Reads and
//! idempotent writes retry. A plain create, comment or transition is sent
//! at most once after a 5xx or a "maybe sent" network error. The
//! idempotent variants find a lost write by a marker or by a state check.

mod idempotent;
mod pager;
mod settings;

use std::fmt;
use std::sync::Arc;

use san_mateo::SanMateo;
use san_mateo::clock::{Entropy, OsEntropy};
use san_mateo::transport::HttpTransport;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::adf::Adf;
use crate::backend::{BackendInfo, DirectBackend, JiraBackend, SanMateoBackend, SanMateoTarget};
use crate::error::{JiraError, Result};
use crate::ids::{AccountId, IssueRef, ProjectKey, TransitionId};
use crate::jql::Jql;
use crate::model::{
    Comment, CommentPage, CreatedIssue, Issue, IssueRefs, IssueType, Page, Project, ServerInfo,
    StatusSummary, Transition, User,
};
use crate::request::{EditOptions, FieldSet, IssueEdit, NewIssue, SearchRequest, TransitionFields};
use crate::site::{BasicAuth, JiraSite};

pub use crate::error::MarkerRef;
pub use crate::ids::IdempotencyKey;
pub use pager::SearchPager;
pub use settings::ClientSettings;

/// The result of an idempotent create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    /// This call created the issue.
    Created(CreatedIssue),
    /// A lookup found the issue of the key. No new issue.
    Found(IssueRefs),
}

/// The result of an idempotent comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentOutcome {
    /// This call added the comment.
    Created(Comment),
    /// A lookup found the comment of the key. No new comment.
    Found(Comment),
}

/// The result of a transition to a status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// The issue is at the target already. No transition was sent.
    AlreadyThere(StatusSummary),
    /// The client did this transition.
    Moved {
        /// The transition.
        via: TransitionId,
        /// The new status.
        to: StatusSummary,
    },
}

/// The result of a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// Jira deleted the issue.
    Deleted,
    /// A retry after a "maybe sent" attempt got 404: an earlier attempt
    /// deleted the issue.
    GoneAfterRetry,
}

/// The typed Jira client. Clones share the backend and the caches.
#[derive(Clone)]
pub struct JiraClient {
    backend: Arc<dyn JiraBackend>,
    settings: ClientSettings,
    entropy: Arc<dyn Entropy>,
    myself: Arc<OnceCell<AccountId>>,
}

impl fmt::Debug for JiraClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JiraClient")
            .field("backend", &self.backend.info())
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

/// A stub error of the RED step.
const fn stub() -> JiraError {
    JiraError::Config {
        code: "not_written_yet",
    }
}

impl JiraClient {
    /// Makes a client over any backend, with `OsEntropy` for the jitter.
    #[must_use]
    pub fn new(backend: Arc<dyn JiraBackend>) -> Self {
        Self {
            backend,
            settings: ClientSettings::default(),
            entropy: Arc::new(OsEntropy),
            myself: Arc::new(OnceCell::new()),
        }
    }

    /// Makes an API token client: a [`DirectBackend`].
    #[must_use]
    pub fn api_token(site: JiraSite, auth: BasicAuth, transport: Arc<dyn HttpTransport>) -> Self {
        Self::new(Arc::new(DirectBackend::new(site, auth, transport)))
    }

    /// Makes a client over a san-mateo connection: a [`SanMateoBackend`].
    #[must_use]
    pub fn san_mateo(san: SanMateo, target: SanMateoTarget) -> Self {
        Self::new(Arc::new(SanMateoBackend::new(san, target)))
    }

    /// Sets the settings.
    #[must_use]
    pub fn with_settings(mut self, settings: ClientSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Sets the entropy of the jitter. Tests use `SeqEntropy`.
    #[must_use]
    pub fn with_entropy(mut self, entropy: Arc<dyn Entropy>) -> Self {
        self.entropy = entropy;
        self
    }

    /// Caps the settings for a durable host: `settings.durable()`.
    #[must_use]
    pub fn durable(mut self) -> Self {
        self.settings = self.settings.durable();
        self
    }

    /// Returns the settings.
    #[must_use]
    pub const fn settings(&self) -> &ClientSettings {
        &self.settings
    }

    /// Returns the mode and the site of the backend. No secrets.
    #[must_use]
    pub fn info(&self) -> BackendInfo {
        self.backend.info()
    }

    /// Reads the user of the credentials (`GET /myself`). It caches the
    /// account id.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn myself(&self) -> Result<User> {
        let _ = (&self.myself, &self.entropy);
        Err(stub())
    }

    /// Returns the account id of the credentials. The first call reads
    /// `GET /myself`, later calls use the cache.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn account_id(&self) -> Result<AccountId> {
        Err(stub())
    }

    /// Reads the site info (`GET /serverInfo`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn server_info(&self) -> Result<ServerInfo> {
        Err(stub())
    }

    /// Reads one project (`GET /project/{key}`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn get_project(&self, key: &ProjectKey) -> Result<Project> {
        let _ = key;
        Err(stub())
    }

    /// Reads one page of projects (`GET /project/search`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn projects(&self, start_at: u64, max_results: u32) -> Result<Page<Project>> {
        let _ = (start_at, max_results);
        Err(stub())
    }

    /// Reads all projects, at most `max_pages` pages.
    ///
    /// # Errors
    ///
    /// `PageLimit` at `max_pages`. The typed errors of each call.
    pub async fn all_projects(&self) -> Result<Vec<Project>> {
        Err(stub())
    }

    /// Reads the issue types of a project for a create
    /// (`GET /issue/createmeta/{key}/issuetypes`), all pages.
    ///
    /// # Errors
    ///
    /// `PageLimit` at `max_pages`. The typed errors of each call.
    pub async fn issue_types(&self, project: &ProjectKey) -> Result<Vec<IssueType>> {
        let _ = project;
        Err(stub())
    }

    /// Creates an issue (`POST /issue`). The client sends it at most once
    /// after a 5xx or a "maybe sent" error, and never after a 429.
    ///
    /// # Errors
    ///
    /// `OutcomeUnknown` with no marker after a lost reply. `RateLimited`
    /// after a 429. The typed errors of the call.
    pub async fn create_issue(&self, issue: &NewIssue) -> Result<CreatedIssue> {
        let _ = issue;
        Err(stub())
    }

    /// Reads an issue with the default fields.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn get_issue(&self, issue: impl Into<IssueRef> + Send) -> Result<Issue> {
        self.get_issue_with(issue, &FieldSet::default()).await
    }

    /// Reads an issue with these fields. A moved issue keeps the key that
    /// Jira returns.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn get_issue_with(
        &self,
        issue: impl Into<IssueRef> + Send,
        fields: &FieldSet,
    ) -> Result<Issue> {
        let _ = (issue.into(), fields);
        Err(stub())
    }

    /// Edits an issue (`PUT /issue/{issue}`). `notifyUsers` goes in the
    /// query only when it is set.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn edit_issue(
        &self,
        issue: impl Into<IssueRef> + Send,
        edit: &IssueEdit,
        options: EditOptions,
    ) -> Result<()> {
        let _ = (issue.into(), edit, options);
        Err(stub())
    }

    /// Deletes an issue (`DELETE /issue/{issue}`).
    ///
    /// # Errors
    ///
    /// `NotFoundOrForbidden` for a 404 on the first attempt. The typed
    /// errors of the call.
    pub async fn delete_issue(
        &self,
        issue: impl Into<IssueRef> + Send,
        delete_subtasks: bool,
    ) -> Result<DeleteOutcome> {
        let _ = (issue.into(), delete_subtasks);
        Err(stub())
    }

    /// Sets an issue property (`PUT /issue/{issue}/properties/{key}`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty key or a key over 255 chars. The typed
    /// errors of the call.
    pub async fn set_issue_property(
        &self,
        issue: impl Into<IssueRef> + Send,
        key: &str,
        value: &Value,
    ) -> Result<()> {
        let _ = (issue.into(), key, value);
        Err(stub())
    }

    /// Reads an issue property. `None` when Jira gives 404.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a bad key. The typed errors of the call.
    pub async fn get_issue_property(
        &self,
        issue: impl Into<IssueRef> + Send,
        key: &str,
    ) -> Result<Option<Value>> {
        let _ = (issue.into(), key);
        Err(stub())
    }

    /// Reads the transitions that the issue has now.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn transitions(&self, issue: impl Into<IssueRef> + Send) -> Result<Vec<Transition>> {
        let _ = issue.into();
        Err(stub())
    }

    /// Does one transition by id. The client sends it at most once after a
    /// 5xx or a "maybe sent" error.
    ///
    /// # Errors
    ///
    /// `OutcomeUnknown` with no marker after a lost reply. The typed errors
    /// of the call.
    pub async fn transition(
        &self,
        issue: impl Into<IssueRef> + Send,
        id: &TransitionId,
        fields: &TransitionFields,
    ) -> Result<()> {
        let _ = (issue.into(), id, fields);
        Err(stub())
    }

    /// Moves an issue to a status with no transition fields. See
    /// [`Self::transition_to_status_with`].
    ///
    /// # Errors
    ///
    /// As [`Self::transition_to_status_with`].
    pub async fn transition_to_status(
        &self,
        issue: impl Into<IssueRef> + Send,
        target: &crate::request::StatusTarget,
    ) -> Result<TransitionOutcome> {
        self.transition_to_status_with(issue, target, &TransitionFields::default())
            .await
    }

    /// Adds a comment (`POST /issue/{issue}/comment`). The client sends it
    /// at most once after a 5xx or a "maybe sent" error.
    ///
    /// # Errors
    ///
    /// `OutcomeUnknown` with no marker after a lost reply. The typed errors
    /// of the call.
    pub async fn add_comment(&self, issue: impl Into<IssueRef> + Send, body: &Adf) -> Result<Comment> {
        let _ = (issue.into(), body);
        Err(stub())
    }

    /// Reads one page of comments, oldest first.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn comments(
        &self,
        issue: impl Into<IssueRef> + Send,
        start_at: u64,
        max_results: u32,
    ) -> Result<CommentPage> {
        let _ = (issue.into(), start_at, max_results);
        Err(stub())
    }

    /// Reads all comments, at most `max_pages` pages.
    ///
    /// # Errors
    ///
    /// `PageLimit` at `max_pages`. The typed errors of each call.
    pub async fn all_comments(&self, issue: impl Into<IssueRef> + Send) -> Result<Vec<Comment>> {
        let _ = issue.into();
        Err(stub())
    }

    /// Starts a search (`POST /search/jql`). Call
    /// [`SearchPager::next_page`] for each page.
    #[must_use]
    pub fn search(&self, request: SearchRequest) -> SearchPager {
        SearchPager::new(self.clone(), request)
    }

    /// Reads all pages of a search, at most `max_issues` issues.
    ///
    /// # Errors
    ///
    /// The errors of [`SearchPager::next_page`].
    pub async fn search_all(&self, request: SearchRequest, max_issues: usize) -> Result<Vec<Issue>> {
        let _ = (request, max_issues);
        Err(stub())
    }

    /// Returns the approximate number of issues of a JQL query
    /// (`POST /search/approximate-count`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn approximate_count(&self, jql: &Jql) -> Result<u64> {
        let _ = jql;
        Err(stub())
    }
}

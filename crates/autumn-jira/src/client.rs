//! `JiraClient`: the typed Jira Cloud REST v3 client.
//!
//! Each call sends attempts through a [`JiraBackend`] and decides each
//! next step with the verified policy ([`crate::policy`]). Reads and
//! idempotent writes retry. A plain create, comment or transition is sent
//! at most once after a 5xx or a "maybe sent" network error. The
//! idempotent variants find a lost write by a marker or by a state check.

mod call;
mod idempotent;
mod pager;
mod settings;

use std::fmt;
use std::sync::Arc;

use san_mateo::SanMateo;
use san_mateo::clock::{Entropy, OsEntropy};
use san_mateo::transport::HttpTransport;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::adf::Adf;
use crate::backend::{
    ApiRequest, BackendInfo, DirectBackend, JiraBackend, Method, SanMateoBackend, SanMateoTarget,
    encode_segment,
};
use crate::error::{JiraError, Op, Result};
use crate::ids::{AccountId, IssueRef, PageToken, ProjectKey, TransitionId};
use crate::jql::Jql;
use crate::model::wire::{
    CountWire, EntityPropertyWire, IssueTypePageWire, SearchPage, SearchPageWire, TransitionsWire,
};
use crate::model::{
    Comment, CommentPage, CreatedIssue, Issue, IssueRefs, IssueType, Page, Project, ServerInfo,
    StatusSummary, Transition, User, decode,
};
use crate::policy::{self, OpClass};
use crate::request::{EditOptions, FieldSet, IssueEdit, NewIssue, SearchRequest, TransitionFields};
use crate::site::{BasicAuth, JiraSite};

pub(crate) use call::status_error;

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

/// One page of a search, for a host that reads one page for each call
/// (F26). `next_token` is `None` on the last page.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IssuePage {
    /// The issues of the page.
    pub issues: Vec<Issue>,
    /// The token of the next page.
    pub next_token: Option<PageToken>,
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

/// The REST API prefix.
const API: &str = "/rest/api/3";
/// The page size of the offset pagers.
const PAGE_SIZE: u32 = 50;
/// The page size of the comment pager.
const COMMENT_PAGE_SIZE: u32 = 100;
/// The longest issue property key.
const MAX_PROPERTY_KEY: usize = 255;

/// Returns `/rest/api/3/issue/{issue}` with the segment encoded.
fn issue_path(issue: &IssueRef) -> String {
    format!("{API}/issue/{}", encode_segment(issue.path_segment()))
}

/// Returns `count` as `u64`.
fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Checks an issue property key.
///
/// The key `.` or `..` is not valid. A URL parser treats such a segment as
/// a dot segment and removes it, so the request goes to another endpoint:
/// `PUT .../properties/..` becomes an issue edit. Percent-encoding does not
/// help, because a URL parser also treats `%2e` as a dot.
fn property_key(key: &str) -> Result<&str> {
    if key.is_empty() || key.chars().count() > MAX_PROPERTY_KEY {
        return Err(JiraError::invalid(
            "issue property key",
            "must have 1 to 255 chars",
        ));
    }
    if matches!(key, "." | "..") {
        return Err(JiraError::invalid(
            "issue property key",
            "must not be . or ..",
        ));
    }
    Ok(key)
}

/// One page of an offset pager.
struct OffsetPage<T> {
    values: Vec<T>,
    total: Option<u64>,
    is_last: bool,
}

impl<T> From<Page<T>> for OffsetPage<T> {
    fn from(page: Page<T>) -> Self {
        Self {
            values: page.values,
            total: page.total,
            is_last: page.is_last,
        }
    }
}

impl From<CommentPage> for OffsetPage<Comment> {
    fn from(page: CommentPage) -> Self {
        // `total` is the grand total (A4). An empty page also ends the list.
        Self {
            values: page.comments,
            total: Some(page.total),
            is_last: false,
        }
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
        let user: User = self
            .read(&ApiRequest::new(
                Op::Myself,
                Method::Get,
                format!("{API}/myself"),
            ))
            .await?;
        let _ = self.myself.set(user.account_id.clone());
        Ok(user)
    }

    /// Returns the account id of the credentials. The first call reads
    /// `GET /myself`, later calls use the cache.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn account_id(&self) -> Result<AccountId> {
        self.myself
            .get_or_try_init(|| async {
                let user: User = self
                    .read(&ApiRequest::new(
                        Op::Myself,
                        Method::Get,
                        format!("{API}/myself"),
                    ))
                    .await?;
                Ok::<_, JiraError>(user.account_id)
            })
            .await
            .cloned()
    }

    /// Reads the site info (`GET /serverInfo`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn server_info(&self) -> Result<ServerInfo> {
        self.read(&ApiRequest::new(
            Op::ServerInfo,
            Method::Get,
            format!("{API}/serverInfo"),
        ))
        .await
    }

    /// Reads one project (`GET /project/{key}`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn get_project(&self, key: &ProjectKey) -> Result<Project> {
        let path = format!("{API}/project/{}", encode_segment(key.as_str()));
        self.read(&ApiRequest::new(Op::GetProject, Method::Get, path))
            .await
    }

    /// Reads one page of projects (`GET /project/search`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn projects(&self, start_at: u64, max_results: u32) -> Result<Page<Project>> {
        let request = ApiRequest::new(
            Op::ListProjects,
            Method::Get,
            format!("{API}/project/search"),
        )
        .query("startAt", start_at.to_string())
        .query("maxResults", max_results.to_string());
        self.read(&request).await
    }

    /// Reads all projects, at most `max_pages` pages.
    ///
    /// # Errors
    ///
    /// `PageLimit` at `max_pages`. The typed errors of each call.
    pub async fn all_projects(&self) -> Result<Vec<Project>> {
        self.offset_pages(Op::ListProjects, async |start| {
            self.projects(start, PAGE_SIZE).await.map(OffsetPage::from)
        })
        .await
    }

    /// Reads the issue types of a project for a create
    /// (`GET /issue/createmeta/{key}/issuetypes`), all pages.
    ///
    /// # Errors
    ///
    /// `PageLimit` at `max_pages`. The typed errors of each call.
    pub async fn issue_types(&self, project: &ProjectKey) -> Result<Vec<IssueType>> {
        let path = format!(
            "{API}/issue/createmeta/{}/issuetypes",
            encode_segment(project.as_str())
        );
        self.offset_pages(Op::ListIssueTypes, async |start| {
            let request = ApiRequest::new(Op::ListIssueTypes, Method::Get, path.clone())
                .query("startAt", start.to_string())
                .query("maxResults", PAGE_SIZE.to_string());
            let page: IssueTypePageWire = self.read(&request).await?;
            Ok(OffsetPage::from(page.into_model()))
        })
        .await
    }

    /// Creates an issue (`POST /issue`). The client sends it at most once
    /// after a 5xx or a "maybe sent" error, and never after a 429.
    ///
    /// # Errors
    ///
    /// `OutcomeUnknown` with no marker after a lost reply. `RateLimited`
    /// after a 429. The typed errors of the call.
    pub async fn create_issue(&self, issue: &NewIssue) -> Result<CreatedIssue> {
        let request = ApiRequest::new(Op::CreateIssue, Method::Post, format!("{API}/issue"))
            .json(&issue.to_body());
        let reply = self.send_ok(OpClass::UnsafeWrite, &request).await?;
        decode(Op::CreateIssue, &reply.body)
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
        let request = ApiRequest::new(Op::GetIssue, Method::Get, issue_path(&issue.into()))
            .query("fields", fields.to_csv());
        self.read(&request).await
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
        let mut request = ApiRequest::new(Op::EditIssue, Method::Put, issue_path(&issue.into()))
            .json(&edit.to_body());
        request.query = options.to_query();
        // A 204 has no body, and a 200 body is not needed: no parse.
        self.send_ok(OpClass::IdempotentWrite, &request).await?;
        Ok(())
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
        let request = ApiRequest::new(Op::DeleteIssue, Method::Delete, issue_path(&issue.into()))
            .query("deleteSubtasks", delete_subtasks.to_string());
        match self.run(OpClass::IdempotentWrite, &request).await {
            Ok(_) => Ok(DeleteOutcome::Deleted),
            Err(failure) => match failure.error {
                JiraError::NotFoundOrForbidden { .. } if failure.after_maybe_sent => {
                    Ok(DeleteOutcome::GoneAfterRetry)
                }
                error => Err(error),
            },
        }
    }

    /// Sets an issue property (`PUT /issue/{issue}/properties/{key}`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty key, a key over 255 chars, or the key
    /// `.` or `..`. The typed errors of the call.
    pub async fn set_issue_property(
        &self,
        issue: impl Into<IssueRef> + Send,
        key: &str,
        value: &Value,
    ) -> Result<()> {
        let path = format!(
            "{}/properties/{}",
            issue_path(&issue.into()),
            encode_segment(property_key(key)?)
        );
        let request = ApiRequest::new(Op::SetProperty, Method::Put, path).json(value);
        self.send_ok(OpClass::IdempotentWrite, &request).await?;
        Ok(())
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
        let path = format!(
            "{}/properties/{}",
            issue_path(&issue.into()),
            encode_segment(property_key(key)?)
        );
        match self
            .read::<EntityPropertyWire>(&ApiRequest::new(Op::GetProperty, Method::Get, path))
            .await
        {
            Ok(property) => Ok(Some(property.value)),
            Err(JiraError::NotFoundOrForbidden { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Reads the transitions that the issue has now.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn transitions(&self, issue: impl Into<IssueRef> + Send) -> Result<Vec<Transition>> {
        let path = format!("{}/transitions", issue_path(&issue.into()));
        let list: TransitionsWire = self
            .read(&ApiRequest::new(Op::GetTransitions, Method::Get, path))
            .await?;
        Ok(list.transitions)
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
        let request = transition_request(&issue.into(), id, fields);
        self.send_ok(OpClass::UnsafeWrite, &request).await?;
        Ok(())
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
    pub async fn add_comment(
        &self,
        issue: impl Into<IssueRef> + Send,
        body: &Adf,
    ) -> Result<Comment> {
        let request = comment_request(&issue.into(), body);
        let reply = self.send_ok(OpClass::UnsafeWrite, &request).await?;
        decode(Op::AddComment, &reply.body)
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
        let path = format!("{}/comment", issue_path(&issue.into()));
        let request = ApiRequest::new(Op::ListComments, Method::Get, path)
            .query("startAt", start_at.to_string())
            .query("maxResults", max_results.to_string());
        self.read(&request).await
    }

    /// Reads all comments, at most `max_pages` pages.
    ///
    /// # Errors
    ///
    /// `PageLimit` at `max_pages`. The typed errors of each call.
    pub async fn all_comments(&self, issue: impl Into<IssueRef> + Send) -> Result<Vec<Comment>> {
        let issue = issue.into();
        self.offset_pages(Op::ListComments, async |start| {
            self.comments(issue.clone(), start, COMMENT_PAGE_SIZE)
                .await
                .map(OffsetPage::from)
        })
        .await
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
    pub async fn search_all(
        &self,
        request: SearchRequest,
        max_issues: usize,
    ) -> Result<Vec<Issue>> {
        let mut pager = self.search(request);
        let mut out = Vec::new();
        while out.len() < max_issues {
            match pager.next_page().await? {
                Some(issues) => out.extend(issues),
                None => break,
            }
        }
        out.truncate(max_issues);
        Ok(out)
    }

    /// Returns the approximate number of issues of a JQL query
    /// (`POST /search/approximate-count`).
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn approximate_count(&self, jql: &Jql) -> Result<u64> {
        let request = ApiRequest::new(
            Op::Count,
            Method::Post,
            format!("{API}/search/approximate-count"),
        )
        .json(&json!({"jql": jql.as_str()}));
        let count: CountWire = self.read(&request).await?;
        Ok(count.count)
    }

    /// Reads one page of a search at `token` (`POST /search/jql`). A host
    /// that keeps the token between calls uses it, for example a harvest
    /// activity that reads one page for each call. The host checks for a
    /// repeated token.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn search_page_at(
        &self,
        request: &SearchRequest,
        token: Option<&PageToken>,
    ) -> Result<IssuePage> {
        let page = self.search_page(request, token).await?;
        let next_token = page.next_page_token.filter(|_| !page.is_last);
        Ok(IssuePage {
            issues: page.issues,
            next_token,
        })
    }

    /// Reads one page of a search.
    pub(crate) async fn search_page(
        &self,
        request: &SearchRequest,
        token: Option<&PageToken>,
    ) -> Result<SearchPage> {
        let api = ApiRequest::new(Op::Search, Method::Post, format!("{API}/search/jql"))
            .json(&request.to_body(token)?);
        let page: SearchPageWire = self.read(&api).await?;
        Ok(page.into_model())
    }

    /// Reads all pages of an offset pager, at most `max_pages` pages.
    async fn offset_pages<T>(
        &self,
        op: Op,
        mut fetch: impl AsyncFnMut(u64) -> Result<OffsetPage<T>>,
    ) -> Result<Vec<T>> {
        let max = self.settings.max_pages;
        let mut out = Vec::new();
        let mut start = 0_u64;
        let mut pages = 0_u32;
        loop {
            if pages >= max {
                return Err(JiraError::PageLimit { op, max });
            }
            let page = fetch(start).await?;
            pages += 1;
            let returned = count(page.values.len());
            out.extend(page.values);
            if page.is_last {
                return Ok(out);
            }
            let next = page.total.map_or_else(
                || (returned > 0).then(|| start.saturating_add(returned)),
                |total| policy::offset_next(start, returned, total),
            );
            match next {
                Some(n) => start = n,
                None => return Ok(out),
            }
        }
    }
}

/// Returns the request of a transition.
fn transition_request(
    issue: &IssueRef,
    id: &TransitionId,
    fields: &TransitionFields,
) -> ApiRequest {
    let path = format!("{}/transitions", issue_path(issue));
    ApiRequest::new(Op::DoTransition, Method::Post, path).json(&fields.to_body(id))
}

/// Returns the request of a new comment.
fn comment_request(issue: &IssueRef, body: &Adf) -> ApiRequest {
    let path = format!("{}/comment", issue_path(issue));
    ApiRequest::new(Op::AddComment, Method::Post, path).json(&json!({"body": body.as_value()}))
}

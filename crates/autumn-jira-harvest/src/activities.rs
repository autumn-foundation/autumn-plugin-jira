//! The eight Jira activities (plan 9.2).
//!
//! Each input holds ids and options only. It never holds a secret or a body.
//! Each input has a work method with an explicit client,
//! `input.run(&client, ctx)`, so a test needs no worker. Each `#[activity]`
//! function is one line: it resolves the client with [`client_for`] and
//! calls the work method.
//!
//! The marked writes `jira_create_issue` and `jira_add_comment` follow the
//! durable-host rule of plan 7.2. After an attempt that can have sent the
//! write, the next attempt only looks for the write by its marker. Only the
//! opt-in `resend_after_negative_lookup` sends it again.

use std::collections::BTreeMap;
use std::time::Duration;

use autumn_harvest::context::ActivityContext;
use autumn_harvest::failure::ActivityFailure;
use autumn_harvest::info::ActivityInfo;
use autumn_harvest::policy::RetryPolicy;
use autumn_harvest::prelude::activity;
use autumn_jira::client::{CommentOutcome, CreateOutcome, DeleteOutcome, TransitionOutcome};
use autumn_jira::error::{Op, UnknownCause};
use autumn_jira::ids::{CommentId, FieldId, PageToken, TransitionId};
use autumn_jira::model::{IssueRefs, IssueSummary, StatusSummary};
use autumn_jira::policy::{Previous, host_may_send};
use autumn_jira::request::{
    EditOptions, FieldSet, IssueEdit, IssueTypeRef, SearchRequest, TransitionFields,
};
use autumn_jira::{
    AccountId, Adf, IdempotencyKey, IssueId, IssueKey, IssueRef, JiraClient, JiraError, Jql,
    NewIssue, ProjectKey, StatusTarget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::clients::{ClientRef, client_for};
use crate::failure::{
    OUTCOME_PENDING_ERROR, OUTCOME_UNKNOWN_ERROR, SETTLE_SECS, config_failure, failure_for,
    marked_write_failure, previous_attempt,
};
use crate::route::{ROUTES_PROPERTY, RouteBinding, RouteKey, SignalRoute, TRACKED_LABEL};

/// The name of the activity that reads one issue.
pub const GET_ISSUE: &str = "jira_get_issue";

/// The longest transport timeout that the `start_to_close` budgets hold
/// for: 60 seconds for each request.
///
/// The default of `ReqwestTransport` is
/// 60 s. A longer transport timeout can let harvest stop an attempt in the
/// middle of a call (R9-test-quality-6).
pub const MAX_TRANSPORT_TIMEOUT: Duration = Duration::from_secs(60);

/// The transport calls of one client send through san-mateo.
///
/// A `ClientRef::Connection` client can make a token refresh before the
/// send, the send, a refresh after a 401, and the send again
/// (R4-harvest-determinism-6).
pub const SAN_MATEO_CALLS_PER_SEND: u32 = 4;

/// The `start_to_close` of a read activity: 9 minutes (plan 7.2, B2).
///
/// One durable read through san-mateo can take 490 s: two sends of four
/// transport calls of 60 s, and one wait of 10 s.
pub const READ_START_TO_CLOSE: Duration = Duration::from_secs(540);

/// The `start_to_close` of a write activity with one client call: 9
/// minutes. The budget is the same as the budget of a read.
pub const WRITE_START_TO_CLOSE: Duration = Duration::from_secs(540);

/// The `start_to_close` of a marked write, a checked write and the watch:
/// 42 minutes.
///
/// The budget is for san-mateo, at four transport calls of 60 s for each
/// send (plan 7.2, B2). Then one durable marked write can take 1 470 s. One
/// checked write can take 2 450 s, and the watch 1 960 s. A direct client
/// needs less. Harvest retries an attempt that stops in the middle of a
/// write as an attempt with an unknown result. So the budget must cover the
/// slow path.
pub const LONG_WRITE_START_TO_CLOSE: Duration = Duration::from_secs(2_520);

/// The most comment pages that one marker lookup of `jira_add_comment`
/// reads: 3 pages of 100 comments, the newest first.
///
/// The cap keeps the `start_to_close` budget finite. An issue can get more
/// comments between two attempts than the cap holds. Then the lookup stops
/// with `PageLimit` and does not report "not found", so no attempt sends the
/// comment again. The activity fails, and it never writes a duplicate.
///
/// The cap applies only to a lookup after a possible send. Attempt 1 sends
/// with no lookup before the POST, so an issue with many comments still
/// gets the comment (R2A-write-safety-3).
pub const COMMENT_LOOKUP_PAGES: u32 = 3;

/// The `start_to_close` of `jira_add_comment`: 70 minutes.
///
/// One attempt can read `GET /myself` (a cold cache), and up to
/// [`COMMENT_LOOKUP_PAGES`] pages before the send and after it. Through
/// san-mateo, at four transport calls of 60 s for each send, that is
/// 3 920 s.
pub const COMMENT_START_TO_CLOSE: Duration = Duration::from_secs(4_200);

/// The retry policy of `jira_get_issue`: 5 attempts, from 1 s.
#[must_use]
pub fn get_issue_retry() -> RetryPolicy {
    RetryPolicy::exponential(5, Duration::from_secs(1))
}

/// The fields that `jira_watch_issue` reads.
const WATCH_FIELDS: [&str; 4] = ["summary", "status", "labels", "updated"];

/// The Jira idempotency key of this activity. It is the same for each
/// attempt [V].
fn activity_key(ctx: &ActivityContext) -> Result<IdempotencyKey, ActivityFailure> {
    let key = ctx
        .idempotency_key()
        .map_err(|_| config_failure("the activity context has no idempotency key"))?;
    Ok(IdempotencyKey::parse(key.as_str())
        .unwrap_or_else(|_| IdempotencyKey::derive(&[key.as_str()])))
}

/// The route key of the worker state.
fn route_key(ctx: &ActivityContext) -> Result<&RouteKey, ActivityFailure> {
    ctx.state::<RouteKey>()
        .ok_or_else(|| config_failure("the harvest state has no RouteKey for a route"))
}

/// The settle delay: `settle_secs`, else [`SETTLE_SECS`].
fn settle(settle_secs: Option<u64>) -> Duration {
    Duration::from_secs(settle_secs.unwrap_or(SETTLE_SECS))
}

/// The details of a failure after an attempt that can have sent a write.
fn maybe_sent_details(code: &str, status: Option<u16>, retryable: bool) -> Value {
    json!({"code": code, "status": status, "sent": true, "retryable": retryable})
}

/// The failure of a negative lookup after an attempt that can have sent
/// the write. It is not retryable: a resend can make a duplicate.
fn negative_lookup() -> ActivityFailure {
    ActivityFailure::non_retryable(
        OUTCOME_UNKNOWN_ERROR,
        "the write can have run, and the marker lookup found nothing; set \
         resend_after_negative_lookup to send it again (ADR 0003)",
    )
    .with_details(maybe_sent_details("outcome_unknown", None, false))
}

/// The failure of a lookup that failed after an attempt that can have sent
/// the write. A retryable failure stays `jira.outcome_pending`, so the next
/// attempt also looks up only.
fn lookup_failure(error: &JiraError, settle: Duration) -> ActivityFailure {
    if !error.is_retryable() {
        return failure_for(error);
    }
    ActivityFailure::retryable(
        OUTCOME_PENDING_ERROR,
        format!("the write can have run, and the marker lookup failed: {error}"),
    )
    .with_details(maybe_sent_details(error.code(), error.status(), true))
    .with_retry_after(settle)
}

/// The failure of a marked write. `jira.outcome_pending` gets the settle
/// delay of the input. A failed pre-lookup gets `sent = false`.
fn send_failure(error: &JiraError, settle: Duration) -> ActivityFailure {
    let failure = marked_write_failure(error);
    if failure.error_type == OUTCOME_PENDING_ERROR {
        failure.with_retry_after(settle)
    } else {
        failure
    }
}

/// The failure of an opt-in resend of a marked write. The resend comes
/// after an attempt that can have sent the write.
///
/// [`previous_attempt`] reads only the attempt just before. So a failure
/// that tells "not sent" here would hide the possible send of an earlier
/// attempt. The next attempt would then send with no lookup. A retryable
/// failure that tells "not sent" stays `jira.outcome_pending` instead. The
/// next attempt then looks up first again (R3-write-safety-1).
fn resend_failure(error: &JiraError, settle: Duration) -> ActivityFailure {
    let failure = send_failure(error, settle);
    let not_sent = failure
        .details
        .as_ref()
        .and_then(|details| details.get("sent"))
        == Some(&Value::Bool(false));
    if failure.non_retryable || !not_sent {
        return failure;
    }
    ActivityFailure::retryable(
        OUTCOME_PENDING_ERROR,
        format!("an earlier attempt can have sent the write, and this attempt failed: {error}"),
    )
    .with_details(maybe_sent_details(error.code(), error.status(), true))
    .with_retry_after(settle)
}

/// The field set of an input: the default set for `None`.
fn field_set(fields: Option<&[FieldId]>) -> FieldSet {
    fields.map_or_else(FieldSet::default, |list| {
        FieldSet::new(list.iter().cloned())
    })
}

/// Merges `route` into the route property. `None` when the property has
/// the entry already, so no PUT is needed. An entry of the same run with
/// another tag is replaced. Other entries stay.
fn merge_route(
    current: Option<&Value>,
    route: &SignalRoute,
    key: &RouteKey,
    binding: &RouteBinding,
) -> Option<Value> {
    let entry = route.property_entry(key, binding);
    let mut entries: Vec<Value> = current
        .and_then(|value| value.get("routes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if entries.contains(&entry) {
        return None;
    }
    let same_run = |e: &Value| {
        e.get("wf").and_then(Value::as_str) == Some(route.workflow_name.as_str())
            && e.get("id").and_then(Value::as_str) == Some(route.workflow_id.as_str())
    };
    let mut replaced = false;
    entries.retain_mut(|e| {
        if !same_run(e) {
            return true;
        }
        if replaced {
            return false;
        }
        e.clone_from(&entry);
        replaced = true;
        true
    });
    if !replaced {
        entries.push(entry);
    }
    Some(json!({"routes": entries}))
}

/// Which way a marked write ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateKind {
    /// This attempt made it.
    Created,
    /// A marker lookup found it. No new write.
    Found,
}

/// Which way a transition ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// The activity did a transition.
    Moved,
    /// The issue was at the target already. No transition.
    AlreadyThere,
}

/// Which way a delete ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteKind {
    /// Jira deleted the issue.
    Deleted,
    /// The issue was gone after an attempt that can have deleted it. Each
    /// failed attempt counts: its `sent` flag tells only the last client
    /// send, and an earlier send of that attempt can have deleted the issue.
    GoneAfterRetry,
}

/// The input of `jira_create_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIssueInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The project.
    pub project: ProjectKey,
    /// The issue type.
    pub issue_type: IssueTypeRef,
    /// The summary.
    pub summary: String,
    /// The description as plain text. Each line becomes a paragraph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_text: Option<String>,
    /// The labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// The assignee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<AccountId>,
    /// Other fields, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<FieldId, Value>,
    /// The run that waits on the issue. The create then adds the label
    /// `autumn-tracked` and the route property in the same call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<SignalRoute>,
    /// Opt-in: send the create again after a negative lookup (ADR 0003).
    #[serde(default)]
    pub resend_after_negative_lookup: bool,
    /// The settle delay before the next attempt after
    /// `jira.outcome_pending`, in seconds. `None` gives `SETTLE_SECS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle_secs: Option<u64>,
}

impl CreateIssueInput {
    /// Makes an input with the default client and no options.
    #[must_use]
    pub fn new(project: ProjectKey, issue_type: IssueTypeRef, summary: impl Into<String>) -> Self {
        Self {
            client: ClientRef::Default,
            project,
            issue_type,
            summary: summary.into(),
            description_text: None,
            labels: Vec::new(),
            assignee: None,
            fields: BTreeMap::new(),
            route: None,
            resend_after_negative_lookup: false,
            settle_secs: None,
        }
    }

    /// Creates the issue at most once for the activity idempotency key.
    ///
    /// # Errors
    ///
    /// The failures of plan 7.2 and [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<CreateIssueOutput, ActivityFailure> {
        let key = activity_key(ctx)?;
        let marker = key.marker_label(&client.settings().marker_label_prefix);
        let issue = self.new_issue(ctx, marker)?;
        let settle = settle(self.settle_secs);
        if !host_may_send(previous_attempt(ctx), self.resend_after_negative_lookup) {
            return match client.find_by_idempotency_key(&self.project, &key).await {
                Ok(Some(found)) => Ok(CreateIssueOutput::found(found)),
                Ok(None) => Err(negative_lookup()),
                Err(e) => Err(lookup_failure(&e, settle)),
            };
        }
        match client.create_issue_idempotent(&issue, &key).await {
            Ok(CreateOutcome::Created(created)) => Ok(CreateIssueOutput {
                id: created.id,
                key: created.key,
                outcome: CreateKind::Created,
            }),
            Ok(CreateOutcome::Found(found)) => Ok(CreateIssueOutput::found(found)),
            Err(e) => Err(send_failure(&e, settle)),
        }
    }

    /// Builds the create body, with the route when there is one. The route
    /// tag binds `marker`: the marker label that the client adds to the
    /// create body (R2F-fixcheck-a-3).
    fn new_issue(
        &self,
        ctx: &ActivityContext,
        marker: String,
    ) -> Result<NewIssue, ActivityFailure> {
        let mut issue = NewIssue::new(
            self.project.clone(),
            self.issue_type.clone(),
            self.summary.as_str(),
        )
        .labels(self.labels.iter().map(String::as_str));
        if let Some(text) = &self.description_text {
            issue = issue.description(Adf::text(text));
        }
        if let Some(account) = &self.assignee {
            issue = issue.assignee(account.clone());
        }
        for (id, value) in &self.fields {
            issue = issue.field(id.clone(), value.clone());
        }
        if let Some(route) = &self.route {
            let key = route_key(ctx)?;
            let binding = RouteBinding::MarkerLabel(marker);
            issue = issue.labels([TRACKED_LABEL]).property(
                ROUTES_PROPERTY,
                json!({"routes": [route.property_entry(key, &binding)]}),
            );
        }
        Ok(issue)
    }
}

impl CreateIssueOutput {
    /// The output of a found issue.
    fn found(refs: IssueRefs) -> Self {
        Self {
            id: refs.id,
            key: refs.key,
            outcome: CreateKind::Found,
        }
    }
}

/// The output of `jira_create_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIssueOutput {
    /// The issue id.
    pub id: IssueId,
    /// The issue key.
    pub key: IssueKey,
    /// Created or found.
    pub outcome: CreateKind,
}

/// The input of `jira_get_issue`. The output is an [`IssueSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetIssueInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue, by key or by id.
    pub issue: IssueRef,
    /// The fields to read. `None` reads the default field set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldId>>,
}

impl GetIssueInput {
    /// Reads `issue` with the default client and the default fields.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            fields: None,
        }
    }

    /// Uses `client`.
    #[must_use]
    pub fn client(mut self, client: ClientRef) -> Self {
        self.client = client;
        self
    }

    /// Reads the issue.
    ///
    /// # Errors
    ///
    /// The failure of [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<IssueSummary, ActivityFailure> {
        let _ = ctx;
        let issue = client
            .get_issue_with(self.issue.clone(), &field_set(self.fields.as_deref()))
            .await
            .map_err(|e| failure_for(&e))?;
        Ok(IssueSummary::from(&issue))
    }
}

/// The input of `jira_search_page`: one page for each call (F26).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPageInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The JQL text. The app owns it.
    pub jql: String,
    /// The fields to read. `None` reads the default field set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldId>>,
    /// The page size, 1 to 100.
    pub max_results: u32,
    /// The token of the page. `None` reads the first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl SearchPageInput {
    /// Reads the first page of `jql` with the default client.
    #[must_use]
    pub fn new(jql: impl Into<String>, max_results: u32) -> Self {
        Self {
            client: ClientRef::Default,
            jql: jql.into(),
            fields: None,
            max_results,
            token: None,
        }
    }

    /// Reads one page.
    ///
    /// # Errors
    ///
    /// `jira.invalid_input` for an empty token. `jira.pagination_stalled`
    /// when Jira gives the same token again. The failure of
    /// [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<SearchPageOutput, ActivityFailure> {
        let _ = ctx;
        let token = match self.token.as_deref() {
            Some(text) => Some(PageToken::parse(text).map_err(|e| failure_for(&e))?),
            None => None,
        };
        let request = SearchRequest::new(Jql::raw(self.jql.as_str()))
            .fields(field_set(self.fields.as_deref()))
            .max_results(self.max_results);
        let page = client
            .search_page_at(&request, token.as_ref())
            .await
            .map_err(|e| failure_for(&e))?;
        let next_token = page.next_token.map(|t| t.as_str().to_owned());
        if next_token.is_some() && next_token == self.token {
            return Err(failure_for(&JiraError::PaginationStalled {
                op: Op::Search,
                pages: 1,
            }));
        }
        Ok(SearchPageOutput {
            issues: page.issues.iter().map(IssueSummary::from).collect(),
            next_token,
        })
    }
}

/// The output of `jira_search_page`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPageOutput {
    /// The issues of the page.
    pub issues: Vec<IssueSummary>,
    /// The token of the next page. `None` on the last page.
    pub next_token: Option<String>,
}

/// The input of `jira_edit_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditIssueInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue.
    pub issue: IssueRef,
    /// A new summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_summary: Option<String>,
    /// Labels to add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_labels: Vec<String>,
    /// Labels to remove.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_labels: Vec<String>,
    /// Fields to set, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<FieldId, Value>,
    /// `notifyUsers`. Sent only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_users: Option<bool>,
}

impl EditIssueInput {
    /// Makes an empty edit of `issue` with the default client.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            set_summary: None,
            add_labels: Vec::new(),
            remove_labels: Vec::new(),
            fields: BTreeMap::new(),
            notify_users: None,
        }
    }

    /// Edits the issue. A PUT is idempotent, so a retry is safe.
    ///
    /// # Errors
    ///
    /// The failure of [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<EditIssueOutput, ActivityFailure> {
        let _ = ctx;
        let mut edit = IssueEdit::new();
        if let Some(summary) = &self.set_summary {
            edit = edit.set_summary(summary.as_str());
        }
        for label in &self.add_labels {
            edit = edit.add_label(label.as_str());
        }
        for label in &self.remove_labels {
            edit = edit.remove_label(label.as_str());
        }
        for (id, value) in &self.fields {
            edit = edit.set_field(id.clone(), value.clone());
        }
        let options = self
            .notify_users
            .map_or_else(EditOptions::default, EditOptions::notify_users);
        client
            .edit_issue(self.issue.clone(), &edit, options)
            .await
            .map_err(|e| failure_for(&e))?;
        Ok(EditIssueOutput {})
    }
}

/// The output of `jira_edit_issue`. It is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditIssueOutput {}

/// The input of `jira_transition_to_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue.
    pub issue: IssueRef,
    /// The target status.
    pub target: StatusTarget,
    /// The `fields` of the transition, for example a resolution.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

impl TransitionInput {
    /// Moves `issue` to `target` with the default client.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>, target: StatusTarget) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            target,
            fields: Map::new(),
        }
    }

    /// Moves the issue.
    ///
    /// Attempt 1, and an attempt after a failure that tells "not sent", call
    /// `transition_to_status_with`. The other attempts come after an attempt
    /// that can have sent the transition. A second send can run the
    /// transition two times, so such an attempt only reads the status
    /// (`resume_transition_to_status`). At the target, it gives
    /// `AlreadyThere`. Else it fails with `jira.outcome_unknown`, which is
    /// not retryable (R2A-write-safety-1).
    ///
    /// # Errors
    ///
    /// The failure of [`crate::failure_for`], for example
    /// `jira.transition_not_available` or `jira.outcome_unknown`.
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<TransitionOutput, ActivityFailure> {
        let outcome = match previous_attempt(ctx) {
            // After a "maybe sent" POST, the client turns each stop into
            // `OutcomeUnknown`. So a read error comes before any POST, and
            // it gets `sent = false` (R3-write-safety-2).
            Previous::First | Previous::NotSent => {
                let mut fields = TransitionFields::default();
                fields.fields.clone_from(&self.fields);
                client
                    .transition_to_status_with(self.issue.clone(), &self.target, &fields)
                    .await
                    .map_err(|e| marked_write_failure(&e))
            }
            Previous::MaybeSent => client
                .resume_transition_to_status(
                    self.issue.clone(),
                    &self.target,
                    UnknownCause::Network,
                )
                .await
                .map_err(|e| failure_for(&e)),
            Previous::Unknown => client
                .resume_transition_to_status(
                    self.issue.clone(),
                    &self.target,
                    UnknownCause::Timeout,
                )
                .await
                .map_err(|e| failure_for(&e)),
        }?;
        Ok(match outcome {
            TransitionOutcome::AlreadyThere(status) => TransitionOutput {
                outcome: TransitionKind::AlreadyThere,
                status,
                via: None,
            },
            TransitionOutcome::Moved { via, to } => TransitionOutput {
                outcome: TransitionKind::Moved,
                status: to,
                via: Some(via),
            },
        })
    }
}

/// The output of `jira_transition_to_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionOutput {
    /// Moved or already there.
    pub outcome: TransitionKind,
    /// The status after the activity.
    pub status: StatusSummary,
    /// The transition, when the activity did one.
    pub via: Option<TransitionId>,
}

/// The input of `jira_add_comment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddCommentInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue.
    pub issue: IssueRef,
    /// The comment as plain text. Each line becomes a paragraph.
    pub text: String,
    /// Opt-in: send the comment again after a negative lookup.
    #[serde(default)]
    pub resend_after_negative_lookup: bool,
    /// The settle delay after `jira.outcome_pending`, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle_secs: Option<u64>,
}

impl AddCommentInput {
    /// Adds `text` to `issue` with the default client.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>, text: impl Into<String>) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            text: text.into(),
            resend_after_negative_lookup: false,
            settle_secs: None,
        }
    }

    /// Adds the comment at most once for the activity idempotency key.
    ///
    /// # Errors
    ///
    /// The failures of plan 7.2 and [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<CommentOutput, ActivityFailure> {
        let key = activity_key(ctx)?;
        let settle = settle(self.settle_secs);
        let client = &comment_client(client);
        if !host_may_send(previous_attempt(ctx), self.resend_after_negative_lookup) {
            return match client
                .find_comment_by_idempotency_key(self.issue.clone(), &key)
                .await
            {
                Ok(Some(found)) => Ok(CommentOutput {
                    id: found.id,
                    outcome: CreateKind::Found,
                }),
                Ok(None) => Err(negative_lookup()),
                Err(e) => Err(lookup_failure(&e, settle)),
            };
        }
        let body = Adf::text(&self.text);
        // On attempt 1, and after an attempt that tells "not sent", no
        // earlier attempt sent the comment. So the pre-lookup can find
        // nothing, and its page cap must not stop the send (R2A-write-safety-3).
        // The opt-in resend after a possible send keeps the pre-lookup.
        let first = matches!(previous_attempt(ctx), Previous::First | Previous::NotSent);
        let sent = if first {
            client
                .add_comment_idempotent_first(self.issue.clone(), &body, &key)
                .await
        } else {
            client
                .add_comment_idempotent(self.issue.clone(), &body, &key)
                .await
        };
        match sent {
            Ok(CommentOutcome::Created(comment)) => Ok(CommentOutput {
                id: comment.id,
                outcome: CreateKind::Created,
            }),
            Ok(CommentOutcome::Found(comment)) => Ok(CommentOutput {
                id: comment.id,
                outcome: CreateKind::Found,
            }),
            Err(e) if first => Err(send_failure(&e, settle)),
            Err(e) => Err(resend_failure(&e, settle)),
        }
    }
}

/// The client of `jira_add_comment`: its comment lookups read at most
/// [`COMMENT_LOOKUP_PAGES`] pages.
fn comment_client(client: &JiraClient) -> JiraClient {
    let mut settings = client.settings().clone();
    settings.max_pages = settings.max_pages.min(COMMENT_LOOKUP_PAGES);
    client.clone().with_settings(settings)
}

/// The output of `jira_add_comment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentOutput {
    /// The comment id.
    pub id: CommentId,
    /// Created or found.
    pub outcome: CreateKind,
}

/// The input of `jira_delete_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteIssueInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue.
    pub issue: IssueRef,
    /// Delete the subtasks too.
    #[serde(default)]
    pub delete_subtasks: bool,
}

impl DeleteIssueInput {
    /// Deletes `issue` with the default client and no subtasks.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            delete_subtasks: false,
        }
    }

    /// Deletes the issue. A 404 on attempt 2 or later gives
    /// `gone_after_retry`. The failure of an earlier attempt tells only its
    /// last client send. An earlier send of that attempt can have deleted the
    /// issue, so each failed attempt can have deleted it (R1-write-safety-6).
    ///
    /// # Errors
    ///
    /// The failure of [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<DeleteOutput, ActivityFailure> {
        let outcome = match client
            .delete_issue(self.issue.clone(), self.delete_subtasks)
            .await
        {
            Ok(DeleteOutcome::Deleted) => DeleteKind::Deleted,
            Ok(DeleteOutcome::GoneAfterRetry) => DeleteKind::GoneAfterRetry,
            // An earlier attempt can have deleted the issue (plan 7.1 for
            // one call, here for the attempts of the host). The `sent` flag
            // of that attempt tells only its last send, so it does not count.
            Err(JiraError::NotFoundOrForbidden { .. })
                if previous_attempt(ctx) != Previous::First =>
            {
                DeleteKind::GoneAfterRetry
            }
            Err(e) => return Err(failure_for(&e)),
        };
        Ok(DeleteOutput { outcome })
    }
}

/// The output of `jira_delete_issue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteOutput {
    /// Deleted or gone after a retry.
    pub outcome: DeleteKind,
}

/// The input of `jira_watch_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchIssueInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue, for example one that a person made.
    pub issue: IssueRef,
    /// The run that waits on the issue.
    pub route: SignalRoute,
}

impl WatchIssueInput {
    /// Watches `issue` for `route` with the default client.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>, route: SignalRoute) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            route,
        }
    }

    /// Adds the route to the route property and the label
    /// `autumn-tracked`. Each step is idempotent (R6, G10).
    ///
    /// # Errors
    ///
    /// `jira.config` when the worker state has no `RouteKey`. The failure
    /// of [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<WatchOutput, ActivityFailure> {
        let key = route_key(ctx)?;
        let read = FieldSet::new(WATCH_FIELDS.iter().filter_map(|f| FieldId::parse(f).ok()));
        let issue = client
            .get_issue_with(self.issue.clone(), &read)
            .await
            .map_err(|e| failure_for(&e))?;
        let current = client
            .get_issue_property(self.issue.clone(), ROUTES_PROPERTY)
            .await
            .map_err(|e| failure_for(&e))?;
        // The tag binds the id of the issue that the watch read.
        let binding = RouteBinding::Issue(issue.id.clone());
        if let Some(merged) = merge_route(current.as_ref(), &self.route, key, &binding) {
            client
                .set_issue_property(self.issue.clone(), ROUTES_PROPERTY, &merged)
                .await
                .map_err(|e| failure_for(&e))?;
        }
        let mut summary = IssueSummary::from(&issue);
        if !summary.labels.iter().any(|l| l == TRACKED_LABEL) {
            let edit = IssueEdit::new().add_label(TRACKED_LABEL);
            client
                .edit_issue(self.issue.clone(), &edit, EditOptions::default())
                .await
                .map_err(|e| failure_for(&e))?;
            summary.labels.push(TRACKED_LABEL.to_owned());
        }
        Ok(WatchOutput { issue: summary })
    }
}

/// The output of `jira_watch_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchOutput {
    /// The issue after the watch, with its status.
    pub issue: IssueSummary,
}

/// Creates an issue at most once for the activity idempotency key (plan 7.2).
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "42m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(2))
)]
pub async fn jira_create_issue(
    ctx: &ActivityContext,
    input: CreateIssueInput,
) -> Result<CreateIssueOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Reads one issue.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(start_to_close = "9m", retry = get_issue_retry())]
pub async fn jira_get_issue(
    ctx: &ActivityContext,
    input: GetIssueInput,
) -> Result<IssueSummary, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Reads one page of a search.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "9m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_search_page(
    ctx: &ActivityContext,
    input: SearchPageInput,
) -> Result<SearchPageOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Edits an issue.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "9m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_edit_issue(
    ctx: &ActivityContext,
    input: EditIssueInput,
) -> Result<EditIssueOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Moves an issue to a status, with a state check.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "42m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(2))
)]
pub async fn jira_transition_to_status(
    ctx: &ActivityContext,
    input: TransitionInput,
) -> Result<TransitionOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Adds a comment at most once for the activity idempotency key.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "70m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(2))
)]
pub async fn jira_add_comment(
    ctx: &ActivityContext,
    input: AddCommentInput,
) -> Result<CommentOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Deletes an issue.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "9m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_delete_issue(
    ctx: &ActivityContext,
    input: DeleteIssueInput,
) -> Result<DeleteOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Adds a signal route and the label `autumn-tracked` to an issue.
///
/// # Errors
///
/// The failure of the work method, or `jira.config` when the worker
/// state has no client for `input.client`.
#[activity(
    start_to_close = "42m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_watch_issue(
    ctx: &ActivityContext,
    input: WatchIssueInput,
) -> Result<WatchOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// The eight activities, in the order of plan 9.2.
#[must_use]
pub fn activities() -> Vec<ActivityInfo> {
    vec![
        jira_create_issue_info(),
        jira_get_issue_info(),
        jira_search_page_info(),
        jira_edit_issue_info(),
        jira_transition_to_status_info(),
        jira_add_comment_info(),
        jira_delete_issue_info(),
        jira_watch_issue_info(),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_issue_input_shape() {
        let input = GetIssueInput::new(IssueId::parse("10001").unwrap())
            .client(ClientRef::Named { name: "ops".into() });
        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(
            value,
            json!({"client": {"type": "named", "name": "ops"}, "issue": "10001"})
        );
        let back: GetIssueInput = serde_json::from_value(json!({"issue": "OPS-1"})).unwrap();
        assert_eq!(back.client, ClientRef::Default);
        assert_eq!(back.fields, None);
        assert_eq!(get_issue_retry().max_attempts, 5);
    }
}

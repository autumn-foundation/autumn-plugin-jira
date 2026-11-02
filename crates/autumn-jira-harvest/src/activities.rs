//! The eight Jira activities (plan 9.2).
//!
//! Each input holds ids and options only. It never holds a secret or a body.
//! Each input has a work method with an explicit client,
//! `input.run(&client, ctx)`, so a test needs no worker. Each `#[activity]`
//! function is one line: it resolves the client with [`client_for`] and
//! calls the work method.
//!
//! The marked writes (`jira_create_issue`, `jira_add_comment`) follow the
//! durable-host rule of plan 7.2: after an attempt that can have sent the
//! write, the next attempt only looks for the write by its marker. Only the
//! opt-in `resend_after_negative_lookup` sends it again.

use std::collections::BTreeMap;
use std::time::Duration;

use autumn_harvest::context::ActivityContext;
use autumn_harvest::failure::ActivityFailure;
use autumn_harvest::info::ActivityInfo;
use autumn_harvest::policy::RetryPolicy;
use autumn_harvest::prelude::activity;
use autumn_jira::ids::{CommentId, FieldId, TransitionId};
use autumn_jira::model::{IssueSummary, StatusSummary};
use autumn_jira::request::IssueTypeRef;
use autumn_jira::{AccountId, IssueId, IssueKey, IssueRef, JiraClient, ProjectKey, StatusTarget};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::clients::{ClientRef, client_for};
use crate::failure::config_failure;
use crate::route::SignalRoute;

/// The name of the activity that reads one issue.
pub const GET_ISSUE: &str = "jira_get_issue";

/// The `start_to_close` of a read activity: 3 minutes (plan 7.2, B2).
pub const READ_START_TO_CLOSE: Duration = Duration::from_secs(180);

/// The `start_to_close` of a write activity with one client call: 5 minutes.
pub const WRITE_START_TO_CLOSE: Duration = Duration::from_secs(300);

/// The `start_to_close` of a marked write, a checked write and the watch:
/// 12 minutes. One durable marked write can take 390 s and one checked
/// write 650 s, at 60 s for each request (plan 7.2, B2).
pub const LONG_WRITE_START_TO_CLOSE: Duration = Duration::from_secs(720);

/// The retry policy of `jira_get_issue`: 5 attempts, from 1 s.
#[must_use]
pub fn get_issue_retry() -> RetryPolicy {
    RetryPolicy::exponential(5, Duration::from_secs(1))
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
    /// The issue was gone after an attempt that can have deleted it.
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
        let _ = (client, ctx);
        Err(config_failure("not yet"))
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
        let _ = (client, ctx);
        Err(config_failure("not yet"))
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
        let _ = (client, ctx);
        Err(config_failure("not yet"))
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
        let _ = (client, ctx);
        Err(config_failure("not yet"))
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

    /// Moves the issue. The client reads the status before each attempt,
    /// so a retry after a lost reply is safe.
    ///
    /// # Errors
    ///
    /// The failure of [`crate::failure_for`], for example
    /// `jira.transition_not_available`.
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<TransitionOutput, ActivityFailure> {
        let _ = (client, ctx);
        Err(config_failure("not yet"))
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
        let _ = (client, ctx);
        Err(config_failure("not yet"))
    }
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

    /// Deletes the issue. A 404 after an attempt that can have deleted it
    /// gives `gone_after_retry`.
    ///
    /// # Errors
    ///
    /// The failure of [`crate::failure_for`].
    pub async fn run(
        &self,
        client: &JiraClient,
        ctx: &ActivityContext,
    ) -> Result<DeleteOutput, ActivityFailure> {
        let _ = (client, ctx);
        Err(config_failure("not yet"))
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
        let _ = (client, ctx);
        Err(config_failure("not yet"))
    }
}

/// The output of `jira_watch_issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchOutput {
    /// The issue after the watch, with its status.
    pub issue: IssueSummary,
}

/// Creates an issue at most once for the activity idempotency key (plan 7.2).
#[activity(
    start_to_close = "12m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(2))
)]
pub async fn jira_create_issue(
    ctx: &ActivityContext,
    input: CreateIssueInput,
) -> Result<CreateIssueOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Reads one issue.
#[activity(start_to_close = "3m", retry = get_issue_retry())]
pub async fn jira_get_issue(
    ctx: &ActivityContext,
    input: GetIssueInput,
) -> Result<IssueSummary, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Reads one page of a search.
#[activity(
    start_to_close = "3m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_search_page(
    ctx: &ActivityContext,
    input: SearchPageInput,
) -> Result<SearchPageOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Edits an issue.
#[activity(
    start_to_close = "5m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_edit_issue(
    ctx: &ActivityContext,
    input: EditIssueInput,
) -> Result<EditIssueOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Moves an issue to a status, with a state check.
#[activity(
    start_to_close = "12m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(2))
)]
pub async fn jira_transition_to_status(
    ctx: &ActivityContext,
    input: TransitionInput,
) -> Result<TransitionOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Adds a comment at most once for the activity idempotency key.
#[activity(
    start_to_close = "12m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(2))
)]
pub async fn jira_add_comment(
    ctx: &ActivityContext,
    input: AddCommentInput,
) -> Result<CommentOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Deletes an issue.
#[activity(
    start_to_close = "5m",
    retry = RetryPolicy::exponential(5, Duration::from_secs(1))
)]
pub async fn jira_delete_issue(
    ctx: &ActivityContext,
    input: DeleteIssueInput,
) -> Result<DeleteOutput, ActivityFailure> {
    input.run(&client_for(ctx, &input.client)?, ctx).await
}

/// Adds a signal route and the label `autumn-tracked` to an issue.
#[activity(
    start_to_close = "12m",
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
    Vec::new()
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

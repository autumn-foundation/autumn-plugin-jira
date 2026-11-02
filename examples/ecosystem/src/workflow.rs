//! harvest: the durable flow `incident_flow`.
//!
//! Workflow code must replay the same way each time, so only activities do
//! I/O. Every activity goes to `ctx.queue_name()`: with no queue, harvest
//! sends an activity to `"default"`, and the worker of this run does not
//! read that queue.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use autumn_harvest::failure::ActivityFailure;
use autumn_harvest::interceptor::{
    ActivityInterceptor, ActivityInterceptorFuture, ActivityInterceptorNext, ActivityInvocation,
};
use autumn_harvest::prelude::*;
use autumn_jira::request::IssueTypeRef;
use autumn_jira::{AccountId, IssueKey, IssueRef, ProjectKey, StatusTarget};
use autumn_jira_harvest::{
    AddCommentInput, CommentOutput, CreateIssueInput, CreateIssueOutput, CreateKind, SignalRoute,
    StatusWait, TransitionInput, TransitionKind, TransitionOutput, WaitOptions, await_issue_status,
    jira_add_comment_info, jira_create_issue_info, jira_transition_to_status_info,
};
use san_mateo_harvest::{ProxyInput, ProxyOutput, san_mateo_proxy_info};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::routes::NOTIFY;
use crate::transport::{ENV, PROJECT, jira_key};

/// The name of the workflow. The bridge allows only this name.
pub const INCIDENT_FLOW: &str = "incident_flow";

/// The input of one run. The connector makes it from the alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertInput {
    /// The id of the alert, for example `A-1001`.
    pub alert_id: String,
    /// The summary of the new issue.
    pub summary: String,
    /// The cloud id of the Jira site, for the escape hatch path.
    pub cloud_id: String,
    /// The wait before the attempt after a lost create reply.
    pub settle_secs: u64,
    /// The longest wait for Done.
    pub wait_secs: u64,
    /// The poll period of the wait. `None`: only a webhook ends the wait.
    pub poll_every_secs: Option<u64>,
}

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// A person resolved the issue.
    Resolved,
    /// No one resolved it in time. The run added an escalation comment.
    TimedOut,
}

/// The reply of the `notify` route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatOutput {
    /// The HTTP status of the chat API.
    pub status: u16,
    /// The worker that ran the activity.
    pub worker: String,
}

/// The output of one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentOutput {
    /// The issue of the incident.
    pub issue_key: IssueKey,
    /// How the run ended.
    pub outcome: Outcome,
    /// The project lead, from the escape hatch.
    pub lead: AccountId,
    /// How the create ended.
    pub created: CreateKind,
    /// How the move to In Progress ended.
    pub moved: TransitionKind,
    /// The status name after the move.
    pub moved_to: String,
    /// The status the wait saw.
    pub wait: StatusWait,
    /// How the comment ended.
    pub comment: CreateKind,
    /// The chat message. Only for a resolved incident.
    pub chat: Option<ChatOutput>,
}

/// The name of the worker, in the harvest state. The example runs worker
/// A, stops it, and runs worker B.
#[derive(Debug, Clone)]
pub struct WorkerName(pub String);

/// The input of [`notify_chat`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyInput {
    /// The issue to tell the team about.
    pub issue: IssueKey,
}

/// Keeps the harvest error type in the workflow failure, for example
/// `jira.outcome_unknown`.
fn failed(error: &HarvestError) -> WorkflowFailure {
    WorkflowFailure::new(
        error.activity_error_type().unwrap_or("ecosystem.workflow"),
        error.to_string(),
    )
}

/// A failure that a retry cannot fix.
fn invalid(error: impl std::fmt::Display) -> WorkflowFailure {
    WorkflowFailure::new("ecosystem.invalid_input", error.to_string()).non_retryable()
}

/// Alert in, ticket, resolution, chat out.
#[workflow]
pub async fn incident_flow(
    ctx: &WorkflowContext,
    input: AlertInput,
) -> Result<IncidentOutput, WorkflowFailure> {
    let queue = ctx.queue_name().to_owned();

    // san-mateo-harvest: the one escape hatch, for a call that the Jira
    // client does not wrap. san-mateo adds the OAuth token.
    let lookup = ProxyInput::new(
        ENV,
        &jira_key(),
        "GET",
        format!("/ex/jira/{}/rest/api/3/project/{PROJECT}", input.cloud_id),
    )
    .query("expand", "lead");
    let project: ProxyOutput = ctx
        .execute_activity_with_opts(&san_mateo_proxy_info(), &lookup, Some(&queue), None, None)
        .await
        .map_err(|e| failed(&e))?;
    let lead = project.body["lead"]["accountId"]
        .as_str()
        .ok_or_else(|| invalid("the project reply has no lead"))
        .and_then(|id| AccountId::parse(id).map_err(invalid))?;

    // autumn-jira-harvest: every Jira write. The route lets the bridge
    // find this run when the issue changes.
    let route = SignalRoute::new(INCIDENT_FLOW, ctx.workflow_id()).map_err(invalid)?;
    let mut create = CreateIssueInput::new(
        ProjectKey::parse(PROJECT).map_err(invalid)?,
        IssueTypeRef::Name("Incident".to_owned()),
        input.summary.as_str(),
    );
    create.description_text = Some(format!("Alert {} fired.", input.alert_id));
    create.labels = vec!["alert".to_owned()];
    create.assignee = Some(lead.clone());
    create.route = Some(route);
    create.settle_secs = Some(input.settle_secs);
    let created: CreateIssueOutput = ctx
        .execute_activity_with_opts(&jira_create_issue_info(), &create, Some(&queue), None, None)
        .await
        .map_err(|e| failed(&e))?;
    let issue = IssueRef::Id(created.id.clone());
    let moved: TransitionOutput = ctx
        .execute_activity_with_opts(
            &jira_transition_to_status_info(),
            TransitionInput::new(issue.clone(), StatusTarget::in_progress()),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(|e| failed(&e))?;

    // The durable wait. A webhook ends it fast; a poll ends it without one.
    let mut options = WaitOptions::new(Duration::from_secs(input.wait_secs));
    options.poll_every = input.poll_every_secs.map(Duration::from_secs);
    let wait = await_issue_status(ctx, &created.id, &StatusTarget::done(), &options).await?;

    let (outcome, text) = match &wait {
        StatusWait::Reached { .. } => (
            Outcome::Resolved,
            format!("Resolved. Alert {} is closed.", input.alert_id),
        ),
        StatusWait::TimedOut { .. } => (
            Outcome::TimedOut,
            format!(
                "Escalation: no one resolved alert {} in {} s.",
                input.alert_id, input.wait_secs
            ),
        ),
    };
    let comment: CommentOutput = ctx
        .execute_activity_with_opts(
            &jira_add_comment_info(),
            AddCommentInput::new(issue, text),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(|e| failed(&e))?;

    // camel: the chat edge, for a resolved incident only.
    let chat = if outcome == Outcome::Resolved {
        let notify = NotifyInput {
            issue: created.key.clone(),
        };
        let chat: ChatOutput = ctx
            .execute_activity_with_opts(&notify_chat_info(), &notify, Some(&queue), None, None)
            .await
            .map_err(|e| failed(&e))?;
        Some(chat)
    } else {
        None
    };
    Ok(IncidentOutput {
        issue_key: created.key,
        outcome,
        lead,
        created: created.outcome,
        moved: moved.outcome,
        moved_to: moved.status.name,
        wait,
        comment: comment.outcome,
        chat,
    })
}

/// Tells the team through the camel route `direct:notify`. harvest-camel
/// sends the activity idempotency key in `HarvestIdempotencyKey`, and the
/// route gives it to the chat API, so a retry makes no second message.
#[activity(
    start_to_close = "60s",
    retry = RetryPolicy::exponential(3, Duration::from_millis(500))
)]
pub async fn notify_chat(
    ctx: &ActivityContext,
    input: NotifyInput,
) -> Result<ChatOutput, ActivityFailure> {
    let reply = harvest_camel::call_route_from_activity(ctx, NOTIFY, json!(input)).await?;
    let status = reply["status"]
        .as_u64()
        .and_then(|s| u16::try_from(s).ok())
        .ok_or_else(|| {
            ActivityFailure::non_retryable("ecosystem.chat", "the notify route gave no status")
        })?;
    let worker = ctx
        .state::<WorkerName>()
        .map(|w| w.0.clone())
        .unwrap_or_default();
    Ok(ChatOutput { status, worker })
}

/// One attempt of an activity, as a worker saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The activity name, for example `jira_create_issue`.
    pub activity: String,
    /// The attempt number, from 1.
    pub attempt: u32,
    /// The error type of a failed attempt, for example
    /// `jira.outcome_pending`. `None` for a success.
    pub error_type: Option<String>,
}

/// A harvest activity interceptor that logs each attempt of each activity.
/// The history keeps only the last result of an activity, so the timeline
/// reads the failed attempts from this log. It holds no input, output or
/// error text: only names and numbers.
#[derive(Debug, Clone, Default)]
pub struct AttemptLog(Arc<Mutex<Vec<Attempt>>>);

impl AttemptLog {
    /// The attempts of one activity, in order.
    pub fn of(&self, activity: &str) -> Vec<Attempt> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|a| a.activity == activity)
            .cloned()
            .collect()
    }

    fn push(&self, attempt: Attempt) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(attempt);
    }
}

/// The first `jira.<code>` or `ecosystem.<code>` type in an error text.
fn error_type(error: &str) -> String {
    ["jira.", "san_mateo.", "camel.", "ecosystem."]
        .iter()
        .find_map(|prefix| {
            let at = error.find(prefix)?;
            let code: String = error[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            Some(format!("{prefix}{code}"))
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

impl ActivityInterceptor for AttemptLog {
    fn intercept<'a>(
        &'a self,
        invocation: &'a ActivityInvocation<'a>,
        ctx: &'a ActivityContext,
        input: serde_json::Value,
        next: ActivityInterceptorNext<'a>,
    ) -> ActivityInterceptorFuture<'a> {
        Box::pin(async move {
            let result = next.run(input).await;
            self.push(Attempt {
                activity: invocation.name().to_owned(),
                attempt: ctx.attempt(),
                error_type: result.as_ref().err().map(|e| error_type(e)),
            });
            result
        })
    }
}

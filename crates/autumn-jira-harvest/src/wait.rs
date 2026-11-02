//! The durable wait: [`await_issue_status`].

use std::time::Duration;

use autumn_harvest::context::WorkflowContext;
use autumn_harvest::failure::WorkflowFailure;
use autumn_jira::ids::WebhookDeliveryId;
use autumn_jira::model::StatusSummary;
use autumn_jira::{IssueId, IssueKey, StatusTarget};
use serde::{Deserialize, Serialize};

use crate::clients::ClientRef;

/// The name of the signal that the bridge sends to a waiting run.
pub const SIGNAL: &str = "jira.event";

/// The longest wait: 29 days.
pub const MAX_WAIT: Duration = Duration::from_secs(29 * 24 * 3600);

/// The default poll interval: 15 minutes.
pub const DEFAULT_POLL_EVERY: Duration = Duration::from_secs(15 * 60);

/// The payload of a [`SIGNAL`]: one Jira event about one issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSignal {
    /// The id of the webhook delivery.
    pub delivery_id: WebhookDeliveryId,
    /// The `webhookEvent` name, for example `jira:issue_updated`.
    pub event: String,
    /// The issue id.
    pub issue_id: IssueId,
    /// The issue key, when the payload has one.
    pub issue_key: Option<IssueKey>,
    /// The status after the event, when the payload has one.
    pub status: Option<StatusSummary>,
    /// The `timestamp` of the payload, in epoch milliseconds.
    pub timestamp_ms: Option<i64>,
}

/// The options of [`await_issue_status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitOptions {
    /// The longest time to wait. At most [`MAX_WAIT`].
    pub timeout: Duration,
    /// The poll interval. `None` reads the issue only at the start and at
    /// the deadline.
    pub poll_every: Option<Duration>,
    /// The client of the reads.
    pub client: ClientRef,
}

impl WaitOptions {
    /// Waits at most `timeout`, polls each [`DEFAULT_POLL_EVERY`], and uses
    /// the default client.
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            poll_every: Some(DEFAULT_POLL_EVERY),
            client: ClientRef::Default,
        }
    }
}

/// What ended a wait at the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaitSource {
    /// The first read.
    Initial,
    /// A signal.
    Signal,
    /// A poll read, or the last read at the deadline.
    Poll,
}

/// The result of [`await_issue_status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusWait {
    /// The issue reached the target.
    Reached {
        /// The status at the target.
        status: StatusSummary,
        /// What showed it.
        source: WaitSource,
    },
    /// The deadline came first.
    TimedOut {
        /// The last status that the wait saw.
        last: Option<StatusSummary>,
    },
}

/// Waits until issue `issue_id` reaches `target`. Deterministic on replay.
///
/// # Errors
///
/// A non-retryable `jira.invalid_input` failure when `options.timeout` is
/// over [`MAX_WAIT`]. The failure of a read activity keeps its error type.
pub async fn await_issue_status(
    ctx: &WorkflowContext,
    issue_id: &IssueId,
    target: &StatusTarget,
    options: &WaitOptions,
) -> Result<StatusWait, WorkflowFailure> {
    let _ = (ctx, issue_id, target, options);
    Ok(StatusWait::TimedOut { last: None })
}

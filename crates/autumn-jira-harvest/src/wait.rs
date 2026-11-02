//! The durable wait: [`await_issue_status`].

use std::time::Duration;

use autumn_harvest::HarvestError;
use autumn_harvest::context::WorkflowContext;
use autumn_harvest::failure::WorkflowFailure;
use autumn_jira::ids::WebhookDeliveryId;
use autumn_jira::model::{IssueSummary, StatusSummary};
use autumn_jira::{IssueId, IssueKey, StatusTarget};
use serde::{Deserialize, Serialize};

use crate::activities::{GetIssueInput, jira_get_issue_info};
use crate::clients::ClientRef;
use crate::failure::{INVALID_INPUT_ERROR, WORKFLOW_ERROR};

/// The name of the signal that the bridge sends to a waiting run.
pub const SIGNAL: &str = "jira.event";

/// The longest wait: 29 days.
pub const MAX_WAIT: Duration = Duration::from_secs(29 * 24 * 3600);

/// The shortest poll interval: 1 second. A durable timer has whole seconds.
pub const MIN_POLL_EVERY: Duration = Duration::from_secs(1);

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
/// The steps (plan 9.3):
///
/// 1. A timeout over [`MAX_WAIT`] fails at once.
/// 2. One read with `jira_get_issue`. At the target, the wait ends with
///    [`WaitSource::Initial`].
/// 3. `deadline = ctx.system_now() + timeout`. The history records each
///    `system_now()`, so a replay sees the same times. The workflow clock
///    `now` of the context stays at the start time in production, so the
///    wait never reads it (W2, B1).
/// 4. Each round races the signal [`SIGNAL`] against a fresh timer of
///    `min(remaining, poll_every)`. A signal of another issue, or with a
///    timestamp older than the newest one seen, is skipped. A skipped signal
///    does not move the deadline. A timer win reads the issue when a poll is
///    due.
/// 5. At the deadline, one last read. At the target, the wait ends with
///    [`WaitSource::Poll`]. Else it ends with [`StatusWait::TimedOut`].
/// 6. At the end, the wait takes the signals that are already in the
///    history. They are about a wait that is over.
///
/// # Errors
///
/// A non-retryable `jira.invalid_input` failure when `options.timeout` is
/// over [`MAX_WAIT`], or `options.poll_every` is under [`MIN_POLL_EVERY`].
/// The failure of a read activity keeps its error type.
pub async fn await_issue_status(
    ctx: &WorkflowContext,
    issue_id: &IssueId,
    target: &StatusTarget,
    options: &WaitOptions,
) -> Result<StatusWait, WorkflowFailure> {
    if options.timeout > MAX_WAIT {
        return Err(WorkflowFailure::new(
            INVALID_INPUT_ERROR,
            "the wait timeout is over MAX_WAIT (29 days)",
        )
        .non_retryable());
    }
    if options
        .poll_every
        .is_some_and(|every| every < MIN_POLL_EVERY)
    {
        return Err(WorkflowFailure::new(
            INVALID_INPUT_ERROR,
            "the poll interval is under MIN_POLL_EVERY (1 s)",
        )
        .non_retryable());
    }
    let outcome = wait_loop(ctx, issue_id, target, options).await;
    // The signals that are already in the history are about a wait that is
    // over. The wait takes them, so the history has no signal that no code
    // reads. A strict replay of harvest 0.8 fails on such a signal. A later
    // wait reads the issue first, so it does not need these signals.
    let drained = ctx.drain_signals_raw(SIGNAL);
    match (outcome, drained) {
        (Ok(wait), Ok(_)) => Ok(wait),
        (Err(failure), _) => Err(failure),
        (Ok(_), Err(error)) => Err(harvest_failure(&error)),
    }
}

/// The steps 2 to 5 of [`await_issue_status`].
async fn wait_loop(
    ctx: &WorkflowContext,
    issue_id: &IssueId,
    target: &StatusTarget,
    options: &WaitOptions,
) -> Result<StatusWait, WorkflowFailure> {
    let read = GetIssueInput::new(issue_id).client(options.client.clone());
    let mut last = read_status(ctx, &read).await?;
    if let Some(status) = at_target(last.as_ref(), target) {
        return Ok(StatusWait::Reached {
            status,
            source: WaitSource::Initial,
        });
    }
    let timeout = chrono::Duration::from_std(options.timeout).map_err(|_| invalid_timeout())?;
    let deadline = ctx.system_now() + timeout;
    let mut newest_ms: Option<i64> = None;
    loop {
        let remaining = (deadline - ctx.system_now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            break;
        }
        let poll_due = options.poll_every.filter(|every| *every < remaining);
        let wait = poll_due.unwrap_or(remaining);
        let winner = ctx
            .race()
            .signal(SIGNAL)
            .label(SIGNAL_BRANCH)
            .timer(wait)
            .label(TIMER_BRANCH)
            .run()
            .await
            .map_err(|e| harvest_failure(&e))?;
        if winner.label.as_deref() == Some(SIGNAL_BRANCH) {
            let Ok(signal) = winner.decode::<IssueSignal>() else {
                ctx.log_warn("jira wait: skipped a signal that is not an IssueSignal");
                continue;
            };
            if signal.issue_id != *issue_id || is_older(signal.timestamp_ms, newest_ms) {
                continue;
            }
            newest_ms = newest_ms.max(signal.timestamp_ms);
            if signal.status.is_some() {
                last.clone_from(&signal.status);
            }
            if let Some(status) = at_target(signal.status.as_ref(), target) {
                return Ok(StatusWait::Reached {
                    status,
                    source: WaitSource::Signal,
                });
            }
        } else if poll_due.is_some() {
            last = read_status(ctx, &read).await?.or(last);
            if let Some(status) = at_target(last.as_ref(), target) {
                return Ok(StatusWait::Reached {
                    status,
                    source: WaitSource::Poll,
                });
            }
        }
    }
    last = read_status(ctx, &read).await?.or(last);
    Ok(
        at_target(last.as_ref(), target).map_or(StatusWait::TimedOut { last }, |status| {
            StatusWait::Reached {
                status,
                source: WaitSource::Poll,
            }
        }),
    )
}

/// The label of the signal branch of a race.
const SIGNAL_BRANCH: &str = "signal";

/// The label of the timer branch of a race.
const TIMER_BRANCH: &str = "timer";

/// Returns the status when it is at the target.
fn at_target(status: Option<&StatusSummary>, target: &StatusTarget) -> Option<StatusSummary> {
    status.filter(|s| target.matches(s)).cloned()
}

/// Returns `true` when a signal is older than the newest one seen (F45). A
/// signal with no timestamp is never older.
const fn is_older(timestamp_ms: Option<i64>, newest_ms: Option<i64>) -> bool {
    matches!((timestamp_ms, newest_ms), (Some(t), Some(newest)) if t < newest)
}

/// Reads the issue with `jira_get_issue` on the queue of the run [V GOTCHA].
async fn read_status(
    ctx: &WorkflowContext,
    read: &GetIssueInput,
) -> Result<Option<StatusSummary>, WorkflowFailure> {
    // The retry and the timeout come from the activity info: the same
    // history as an explicit `exponential(5, 1s)` and `3m`.
    let issue: IssueSummary = ctx
        .execute_activity_with_opts(
            &jira_get_issue_info(),
            read,
            Some(ctx.queue_name()),
            None,
            None,
        )
        .await
        .map_err(|e| harvest_failure(&e))?;
    Ok(issue.status)
}

/// The failure of a timeout that does not fit a chrono duration.
fn invalid_timeout() -> WorkflowFailure {
    WorkflowFailure::new(INVALID_INPUT_ERROR, "the wait timeout is too large").non_retryable()
}

/// Returns the workflow failure for a harvest error. An activity failure
/// keeps its error type and its details.
pub(crate) fn harvest_failure(error: &HarvestError) -> WorkflowFailure {
    let failure = WorkflowFailure::new(
        error.activity_error_type().unwrap_or(WORKFLOW_ERROR),
        error.to_string(),
    );
    match error.activity_details() {
        Some(details) => failure.with_details(details.clone()),
        None => failure,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;

    /// Guard (W2, M8): no workflow code of this crate reads the clock that
    /// stays at the start time in production.
    #[test]
    fn no_ctx_now_in_src() {
        let banned = ["ctx", ".now("].concat();
        for (name, text) in [
            ("lib.rs", include_str!("lib.rs")),
            ("activities.rs", include_str!("activities.rs")),
            ("bridge.rs", include_str!("bridge.rs")),
            ("clients.rs", include_str!("clients.rs")),
            ("failure.rs", include_str!("failure.rs")),
            ("route.rs", include_str!("route.rs")),
            ("wait.rs", include_str!("wait.rs")),
        ] {
            assert!(!text.contains(&banned), "{name} reads the start-time clock");
        }
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let files = std::fs::read_dir(dir).unwrap().count();
        assert_eq!(files, 7, "a new file in src/ needs a line in this guard");
    }

    #[test]
    fn helpers() {
        assert!(is_older(Some(1), Some(2)));
        assert!(!is_older(Some(2), Some(2)));
        assert!(!is_older(None, Some(2)));
        assert!(!is_older(Some(1), None));
        let options = WaitOptions::new(Duration::from_secs(60));
        assert_eq!(options.poll_every, Some(DEFAULT_POLL_EVERY));
        assert_eq!(options.client, ClientRef::Default);
        let failure = invalid_timeout();
        assert_eq!(failure.error_type, INVALID_INPUT_ERROR);
        assert!(failure.non_retryable);
        let plain = harvest_failure(&HarvestError::Config("no branch".into()));
        assert_eq!(plain.error_type, WORKFLOW_ERROR);
        assert_eq!(plain.details, None);
    }
}

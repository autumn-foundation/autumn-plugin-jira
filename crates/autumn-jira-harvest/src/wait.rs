//! The durable wait: [`await_issue_status`].

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use autumn_harvest::HarvestError;
use autumn_harvest::context::WorkflowContext;
use autumn_harvest::failure::WorkflowFailure;
use autumn_jira::ids::WebhookDeliveryId;
use autumn_jira::model::{IssueSummary, StatusSummary};
use autumn_jira::{IssueId, IssueKey, StatusTarget};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::activities::{GetIssueInput, jira_get_issue_info};
use crate::clients::ClientRef;
use crate::failure::{ERROR_PREFIX, INVALID_INPUT_ERROR, WORKFLOW_ERROR};

/// The name of the signal that the bridge sends to a waiting run.
pub const SIGNAL: &str = "jira.event";

/// The longest wait: 29 days.
pub const MAX_WAIT: Duration = Duration::from_secs(29 * 24 * 3600);

/// The shortest poll interval: 1 second. A durable timer has whole seconds.
pub const MIN_POLL_EVERY: Duration = Duration::from_secs(1);

/// The default poll interval: 15 minutes.
pub const DEFAULT_POLL_EVERY: Duration = Duration::from_secs(15 * 60);

/// The most poll rounds of one wait: `timeout / poll_every`, rounded up.
///
/// Each round adds events to the history. Harvest moves a run with 50 000
/// events to the DLQ. 3 000 poll rounds and [`MAX_SIGNAL_ROUNDS`] signal
/// rounds use less than half of that cap, so the rest of the workflow has
/// room. The default poll over [`MAX_WAIT`] is 2 784 rounds.
pub const MAX_POLL_ROUNDS: u64 = 3_000;

/// The most signal rounds of one wait that do not end it.
///
/// After them, the wait does not take signals, and only the polls and the
/// deadline can end it. A signal round adds events to the history too, so
/// this bound keeps a busy issue, or a copied route, from filling the
/// history.
pub const MAX_SIGNAL_ROUNDS: u32 = 1_000;

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
/// 1. A timeout over [`MAX_WAIT`], a poll under [`MIN_POLL_EVERY`], or more
///    than [`MAX_POLL_ROUNDS`] poll rounds fail at once. The waits of one run
///    run one at a time (R4-harvest-determinism-7). All waits share the
///    signal name [`SIGNAL`], so two waits at the same time can take the
///    signals of each other. A wait that starts while another wait of the
///    run is open waits until that wait ends. Its timeout starts then. Its
///    first read then shows each change that it did not get as a signal.
/// 2. One read with `jira_get_issue`. At the target, the wait ends with
///    [`WaitSource::Initial`]. A failure of this read ends the wait.
/// 3. `deadline = ctx.system_now() + timeout`. The history records each
///    `system_now()`, so a replay sees the same times. The workflow clock
///    `now` of the context stays at the start time in production, so the
///    wait never reads it (W2, B1).
/// 4. Each round races the signal [`SIGNAL`] against a fresh timer of
///    `min(remaining, poll_every)`. A signal of another issue, or with a
///    timestamp older than the newest time seen, is skipped. The newest time
///    comes from the signals and from the `updated` time of each read. Both
///    come from the Jira clock. A skipped signal does not move the deadline.
///    After [`MAX_SIGNAL_ROUNDS`] signals that do not end the wait, the race
///    has the timer only. A timer win reads the issue when a poll is due.
/// 5. At the deadline, one last read. At the target, the wait ends with
///    [`WaitSource::Poll`]. Else it ends with [`StatusWait::TimedOut`].
///
/// A poll read or the last read that fails with a retryable class is a
/// missed poll (R4-harvest-determinism-1). The wait logs the error type,
/// keeps the last status and goes on. A read has no side effect, and the
/// next poll, a signal or the deadline still ends the wait. The retryable
/// classes are a `jira.*` failure with `details.retryable = true`, a
/// timeout of the activity, and an activity failure with no `jira.` type.
/// 6. At the end, the wait takes the signals that are already in the
///    history. They are about a wait that is over. A strict replay of
///    harvest 0.8 fails on a history that ends with signals that no code
///    read, so this drain keeps every history replayable.
///
/// The drain has one cost: a workflow that reads [`SIGNAL`] itself after
/// the wait does not get the signals that came before the end of the
/// wait. A later wait reads the issue first, so it does not need them.
///
/// # Errors
///
/// A non-retryable `jira.invalid_input` failure when `options.timeout` is
/// over [`MAX_WAIT`], `options.poll_every` is under [`MIN_POLL_EVERY`], or
/// the poll rounds are over [`MAX_POLL_ROUNDS`]. The failure of the first
/// read, and a non-retryable failure of a later read, keep their error type.
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
    if options.poll_every.is_some_and(|every| {
        options.timeout.as_secs().div_ceil(every.as_secs().max(1)) > MAX_POLL_ROUNDS
    }) {
        return Err(WorkflowFailure::new(
            INVALID_INPUT_ERROR,
            "the wait has more than MAX_POLL_ROUNDS (3000) poll rounds; poll less often",
        )
        .non_retryable());
    }
    let _open = OpenWait::enter(ctx).await?;
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
    let first = read_issue(ctx, &read)
        .await
        .map_err(|e| harvest_failure(&e))?;
    let mut last = first.status;
    if let Some(status) = at_target(last.as_ref(), target) {
        return Ok(StatusWait::Reached {
            status,
            source: WaitSource::Initial,
        });
    }
    let mut newest_ms = first.updated.map(|t| t.epoch_ms());
    let timeout = chrono::Duration::from_std(options.timeout).map_err(|_| invalid_timeout())?;
    let deadline = ctx.system_now() + timeout;
    let mut signal_rounds: u32 = 0;
    loop {
        let remaining = (deadline - ctx.system_now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            break;
        }
        let poll_due = options.poll_every.filter(|every| *every < remaining);
        let wait = poll_due.unwrap_or(remaining);
        let race = ctx.race();
        let race = if signal_rounds < MAX_SIGNAL_ROUNDS {
            race.signal(SIGNAL).label(SIGNAL_BRANCH)
        } else {
            race
        };
        let winner = race
            .timer(wait)
            .label(TIMER_BRANCH)
            .run()
            .await
            .map_err(|e| harvest_failure(&e))?;
        if winner.label.as_deref() == Some(SIGNAL_BRANCH) {
            signal_rounds = signal_rounds.saturating_add(1);
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
        } else if poll_due.is_some()
            && let Some(issue) = poll(ctx, &read).await?
        {
            newest_ms = newest_ms.max(issue.updated.map(|t| t.epoch_ms()));
            last = issue.status.or(last);
            if let Some(status) = at_target(last.as_ref(), target) {
                return Ok(StatusWait::Reached {
                    status,
                    source: WaitSource::Poll,
                });
            }
        }
    }
    if let Some(issue) = poll(ctx, &read).await? {
        last = issue.status.or(last);
    }
    Ok(
        at_target(last.as_ref(), target).map_or(StatusWait::TimedOut { last }, |status| {
            StatusWait::Reached {
                status,
                source: WaitSource::Poll,
            }
        }),
    )
}

/// The open waits, by the address of their workflow context.
///
/// Two waits at the same time in one run share [`SIGNAL`], so each one can
/// take and drop a signal of the other. So the waits of one run run one at
/// a time. Each context has its own address while it lives.
static OPEN_WAITS: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());

/// One open wait. The drop closes it, also when the workflow future drops.
struct OpenWait(usize);

impl OpenWait {
    /// Opens a wait for `ctx`. While another wait of `ctx` is open, it waits
    /// with `await_condition`, and it opens the wait when that one ends.
    ///
    /// It never fails because of the other wait. A failure can come only in
    /// the first poll or after the other wait has ended, and a parked
    /// future and a replay give these at other points. Waiting gives the
    /// same commands in both: the second wait starts in the poll in which the
    /// first wait ends.
    async fn enter(ctx: &WorkflowContext) -> Result<Self, WorkflowFailure> {
        let key = std::ptr::from_ref(ctx).addr();
        loop {
            let inserted = OPEN_WAITS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key);
            // Make the guard only after the lock is free: a guard that drops
            // takes the lock again.
            if inserted {
                return Ok(Self(key));
            }
            ctx.await_condition(move || !Self::is_open(key))
                .await
                .map_err(|e| harvest_failure(&e))?;
        }
    }

    /// Returns `true` while a wait of the context `key` is open.
    fn is_open(key: usize) -> bool {
        OPEN_WAITS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&key)
    }
}

impl Drop for OpenWait {
    fn drop(&mut self) {
        OPEN_WAITS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// A poll read. A retryable failure is a missed poll: `Ok(None)`.
async fn poll(
    ctx: &WorkflowContext,
    read: &GetIssueInput,
) -> Result<Option<IssueSummary>, WorkflowFailure> {
    match read_issue(ctx, read).await {
        Ok(issue) => Ok(Some(issue)),
        Err(error) if is_missed_poll(&error) => {
            // The error type is a class. It holds no body and no secret.
            let class = error.activity_error_type().unwrap_or("timeout");
            ctx.log_warn(&format!(
                "jira wait: a read failed ({class}); the wait goes on"
            ));
            Ok(None)
        }
        Err(error) => Err(harvest_failure(&error)),
    }
}

/// Returns `true` when a failed read is a missed poll: a `jira.*` failure
/// with `details.retryable = true`, a timeout of the activity, or an
/// activity failure with no `jira.` type. A `jira.*` failure with no
/// `retryable` detail, and each error that is not about the activity, end
/// the wait.
fn is_missed_poll(error: &HarvestError) -> bool {
    if matches!(error, HarvestError::Timeout { .. }) {
        return true;
    }
    let Some(error_type) = error.activity_error_type() else {
        return false;
    };
    if !error_type.starts_with(ERROR_PREFIX) {
        return true;
    }
    error
        .activity_details()
        .and_then(|details| details.get("retryable"))
        .and_then(Value::as_bool)
        == Some(true)
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
async fn read_issue(
    ctx: &WorkflowContext,
    read: &GetIssueInput,
) -> Result<IssueSummary, HarvestError> {
    // The retry and the timeout come from the activity info: the same
    // history as an explicit `exponential(5, 1s)` and the read timeout.
    ctx.execute_activity_with_opts(
        &jira_get_issue_info(),
        read,
        Some(ctx.queue_name()),
        None,
        None,
    )
    .await
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

    /// R4-harvest-determinism-5: the wait ends before the execution deadline
    /// of the run, with room for the last read.
    #[test]
    fn the_deadline_fits_the_run() {
        let now = chrono::DateTime::from_timestamp(1_000_000, 0).unwrap();
        let hour = chrono::Duration::hours(1);
        let margin = chrono::Duration::from_std(LAST_READ_MARGIN).unwrap();
        // No execution deadline: the full timeout.
        assert_eq!(wait_deadline(now, hour, None), now + hour);
        // A far execution deadline: the full timeout.
        let far = now + hour * 24;
        assert_eq!(wait_deadline(now, hour, Some(far)), now + hour);
        // A near execution deadline: the wait ends one margin before it.
        let near = now + hour * 2;
        assert_eq!(wait_deadline(now, hour * 3, Some(near)), near - margin);
        // A deadline closer than the margin: the wait ends now.
        let close = now + chrono::Duration::minutes(5);
        assert_eq!(wait_deadline(now, hour, Some(close)), now);
        // The margin covers every attempt of the last read and its backoff.
        let retry = crate::activities::get_issue_retry();
        let attempts = crate::activities::READ_START_TO_CLOSE * retry.max_attempts;
        assert!(LAST_READ_MARGIN >= attempts + Duration::from_secs(15));
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

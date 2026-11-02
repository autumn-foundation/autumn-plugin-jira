//! Failure mapping: Jira errors to typed harvest failures.
//!
//! Each failure has the error type `jira.<code>`, where `<code>` is the
//! stable code of [`autumn_jira::JiraError::code`]. The retry class comes
//! from the one class table of `autumn-jira`. A failure never holds a body,
//! a header value or a secret.

use std::time::Duration;

use autumn_harvest::context::ActivityContext;
use autumn_harvest::failure::{ActivityFailure, parse_typed_payload};
use autumn_jira::JiraError;
use autumn_jira::policy::{OpClass, Previous};
use serde_json::{Value, json};

/// The prefix of each error type that this crate makes.
pub const ERROR_PREFIX: &str = "jira.";

/// The error type of a configuration failure, for example a worker state
/// with no Jira client.
pub const CONFIG_ERROR: &str = "jira.config";

/// The error type of an input that is not valid.
pub const INVALID_INPUT_ERROR: &str = "jira.invalid_input";

/// The error type of a marked write with an unknown outcome. A later
/// attempt looks for the write by its marker.
pub const OUTCOME_PENDING_ERROR: &str = "jira.outcome_pending";

/// The error type of a write with an unknown outcome that no lookup can
/// find. It is not retryable.
pub const OUTCOME_UNKNOWN_ERROR: &str = "jira.outcome_unknown";

/// The error type of a workflow failure with no activity error type.
pub const WORKFLOW_ERROR: &str = "jira.workflow";

/// The settle delay before the attempt after `jira.outcome_pending`, in
/// seconds. The search index of Jira can lag, so the next lookup waits.
pub const SETTLE_SECS: u64 = 30;

/// A non-retryable [`CONFIG_ERROR`] failure.
pub(crate) fn config_failure(message: &str) -> ActivityFailure {
    ActivityFailure::non_retryable(CONFIG_ERROR, message)
}

/// A non-retryable [`INVALID_INPUT_ERROR`] failure.
pub(crate) fn invalid_input(message: &str) -> ActivityFailure {
    ActivityFailure::non_retryable(INVALID_INPUT_ERROR, message)
}

/// `"jira.<code>"`; retryable iff `err.is_retryable()`; `RateLimited` -> `with_retry_after`;
/// `OutcomeUnknown` with marker -> `with_retry_after(SETTLE_SECS)`; details
/// `{code, status, sent, retryable}`.
///
/// The message is the `Display` text of the error. It has no body, no Jira
/// message and no secret (plan 4.6). `sent` is
/// [`JiraError::may_have_run`]: `false` only when the request cannot have
/// run in Jira. [`previous_attempt`] reads it. `retryable` is
/// `err.is_retryable()`. A workflow sees no retry class on a failed
/// activity, so `await_issue_status` reads this detail.
#[must_use]
pub fn failure_for(err: &JiraError) -> ActivityFailure {
    let code = err.code();
    let error_type = format!("{ERROR_PREFIX}{code}");
    let message = err.to_string();
    let failure = if err.is_retryable() {
        ActivityFailure::retryable(error_type, message)
    } else {
        ActivityFailure::non_retryable(error_type, message)
    }
    .with_details(json!({
        "code": code,
        "status": err.status(),
        "sent": err.may_have_run(),
        "retryable": err.is_retryable(),
    }));
    match err {
        JiraError::RateLimited {
            retry_after: Some(wait),
            ..
        } => failure.with_retry_after(*wait),
        JiraError::OutcomeUnknown {
            marker: Some(_), ..
        } => failure.with_retry_after(Duration::from_secs(SETTLE_SECS)),
        _ => failure,
    }
}

/// The failure of a marked write (`create_issue_idempotent`,
/// `add_comment_idempotent`) or of `transition_to_status_with`.
///
/// It is [`failure_for`], with one change: an error of a read operation
/// gets `details.sent = false`. A marked write gives a read error only from
/// the pre-lookup, before the send. A failed lookup after the send becomes
/// `OutcomeUnknown` in the client. So a read error tells that the write did
/// not go out, and the next attempt can send it (R5-spec-proof-1). A
/// transition is the same: after a "maybe sent" POST, the client turns each
/// stop into `OutcomeUnknown`. So a status read or a transition list read
/// that fails comes before any POST (R3-write-safety-2).
#[must_use]
pub(crate) fn marked_write_failure(err: &JiraError) -> ActivityFailure {
    let mut failure = failure_for(err);
    if is_read_error(err)
        && let Some(Value::Object(details)) = failure.details.as_mut()
    {
        details.insert("sent".to_owned(), Value::Bool(false));
    }
    failure
}

/// Returns `true` when the error comes from a read operation.
fn is_read_error(err: &JiraError) -> bool {
    let op = match err {
        JiraError::Upstream { op, .. }
        | JiraError::Network { op, .. }
        | JiraError::Redirect { op, .. }
        | JiraError::UnexpectedStatus { op, .. }
        | JiraError::Decode { op, .. }
        | JiraError::PaginationStalled { op, .. }
        | JiraError::PageLimit { op, .. }
        | JiraError::RateLimited { op, .. } => *op,
        _ => return false,
    };
    op.class() == OpClass::Read
}

/// Glue for [`autumn_jira::policy::host_may_send`] (plan section 7.2).
///
/// Attempt 1 gives `First`. Else the typed failure of the previous attempt
/// decides: `jira.outcome_pending` gives `MaybeSent`, and any other
/// `jira.*` failure with `details.sent = false` gives `NotSent`. A missing
/// or other payload gives `Unknown`: a worker crash or a `start_to_close`
/// timeout can come after a send.
#[must_use]
pub fn previous_attempt(ctx: &ActivityContext) -> Previous {
    if ctx.attempt() <= 1 {
        return Previous::First;
    }
    ctx.previous_failure()
        .and_then(parse_typed_payload)
        .map_or(Previous::Unknown, |failure| previous_of(&failure))
}

/// Maps a typed failure of the previous attempt to a [`Previous`].
fn previous_of(failure: &ActivityFailure) -> Previous {
    let sent = failure
        .details
        .as_ref()
        .and_then(|details| details.get("sent"))
        .and_then(Value::as_bool);
    match (failure.error_type.as_str(), sent) {
        (OUTCOME_PENDING_ERROR, _) => Previous::MaybeSent,
        (error_type, Some(false)) if error_type.starts_with(ERROR_PREFIX) => Previous::NotSent,
        _ => Previous::Unknown,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "tests with tables"
)]
mod tests {
    use super::*;
    use autumn_harvest::failure::IntoActivityErrorString;
    use autumn_jira::error::{MarkerRef, Op, UnknownCause};
    use serde_json::json;

    fn retry_ctx(attempt: u32, previous: Option<String>) -> ActivityContext {
        ActivityContext::new_test()
            .with_attempt(attempt)
            .with_max_attempts(5)
            .with_previous_failure(previous)
    }

    fn payload(err: &JiraError) -> String {
        failure_for(err).into_error_payload()
    }

    #[test]
    fn previous_attempt_table() {
        let op = Op::CreateIssue;
        let pending = JiraError::OutcomeUnknown {
            op,
            cause: UnknownCause::ServerError(503),
            marker: Some(MarkerRef::label("autumn-idem-1".into())),
        };
        let rate_limited = JiraError::RateLimited {
            op,
            retry_after: None,
            reason: None,
        };
        let not_sent = JiraError::Network {
            op,
            code: "store_error",
        };
        let maybe_sent = JiraError::Network {
            op,
            code: "connect",
        };
        let upstream = JiraError::Upstream { op, status: 503 };
        let hand_made = ActivityFailure::retryable("jira.rate_limited", "x")
            .with_details(json!({"code": "rate_limited", "sent": true}))
            .into_error_payload();
        let no_details = ActivityFailure::retryable("jira.network", "x").into_error_payload();
        let read = JiraError::Upstream {
            op: Op::Search,
            status: 503,
        };
        let pre_lookup = marked_write_failure(&read).into_error_payload();
        let not_jira = ActivityFailure::retryable("CircuitOpen", "x")
            .with_details(json!({"sent": false}))
            .into_error_payload();
        let rows: Vec<(&str, ActivityContext, Previous)> = vec![
            ("attempt 1", ActivityContext::new_test(), Previous::First),
            (
                "attempt 1 with a failure",
                retry_ctx(1, Some(payload(&pending))),
                Previous::First,
            ),
            (
                "outcome pending",
                retry_ctx(2, Some(payload(&pending))),
                Previous::MaybeSent,
            ),
            (
                "rate limited",
                retry_ctx(3, Some(payload(&rate_limited))),
                Previous::NotSent,
            ),
            (
                "network, not sent",
                retry_ctx(2, Some(payload(&not_sent))),
                Previous::NotSent,
            ),
            (
                "network, maybe sent",
                retry_ctx(2, Some(payload(&maybe_sent))),
                Previous::Unknown,
            ),
            (
                "upstream",
                retry_ctx(2, Some(payload(&upstream))),
                Previous::Unknown,
            ),
            (
                "a failed pre-lookup",
                retry_ctx(2, Some(pre_lookup)),
                Previous::NotSent,
            ),
            (
                "upstream of a read, not a marked write",
                retry_ctx(2, Some(payload(&read))),
                Previous::Unknown,
            ),
            (
                "not a jira failure",
                retry_ctx(2, Some(not_jira)),
                Previous::Unknown,
            ),
            (
                "sent = true by hand",
                retry_ctx(2, Some(hand_made)),
                Previous::Unknown,
            ),
            (
                "no details",
                retry_ctx(2, Some(no_details)),
                Previous::Unknown,
            ),
            (
                "a crash: plain text",
                retry_ctx(2, Some("worker lost".into())),
                Previous::Unknown,
            ),
            (
                "a timeout: no failure",
                retry_ctx(2, None),
                Previous::Unknown,
            ),
        ];
        for (name, ctx, expected) in rows {
            assert_eq!(previous_attempt(&ctx), expected, "{name}");
        }
    }
}

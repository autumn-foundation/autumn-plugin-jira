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
use autumn_jira::policy::Previous;
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

/// The `jira.<code>` of a rate limit.
const RATE_LIMITED_ERROR: &str = "jira.rate_limited";

/// The `jira.<code>` of a network error.
const NETWORK_ERROR: &str = "jira.network";

/// The codes of a `JiraError::Network` that did not leave this process.
///
/// The client gives `Network` after the last attempt of a
/// `BackendError::NotSent { transient: true }` or a `MaybeSent`. Only these
/// codes come from `NotSent` (plan 4.4: the san-mateo `Store`,
/// `RefreshCooldown` and `Provider` errors). Any other code, also a new one,
/// counts as "maybe sent". That is the safe side for a write.
const NOT_SENT_NETWORK_CODES: [&str; 3] = ["store_error", "connection_refresh_backoff", "provider"];

/// Returns `false` only when the request cannot have run in Jira.
///
/// Nothing is sent for an input, a config, an auth or a cloud id error. A
/// 4xx reply and a 429 reply say that Jira did not do the write. A
/// `Network` error with a code of [`NOT_SENT_NETWORK_CODES`] did not leave
/// this process. Every other error, also a future variant, gives `true`.
fn was_sent(err: &JiraError) -> bool {
    match err {
        JiraError::InvalidInput { .. }
        | JiraError::Config { .. }
        | JiraError::Auth { .. }
        | JiraError::CloudId(_)
        | JiraError::NotFoundOrForbidden { .. }
        | JiraError::Unauthorized { .. }
        | JiraError::CaptchaRequired { .. }
        | JiraError::Validation { .. }
        | JiraError::Conflict { .. }
        | JiraError::Gone { .. }
        | JiraError::PayloadTooLarge { .. }
        | JiraError::RateLimited { .. }
        | JiraError::TransitionNotAvailable { .. }
        | JiraError::LabelsNotOnScreen { .. } => false,
        JiraError::Network { code, .. } => !NOT_SENT_NETWORK_CODES.contains(code),
        _ => true,
    }
}

/// "jira.<code>"; retryable iff err.is_retryable(); RateLimited -> with_retry_after;
/// OutcomeUnknown with marker -> with_retry_after(SETTLE_SECS); details {code, status, sent}.
///
/// The message is the `Display` text of the error. It has no body, no Jira
/// message and no secret (plan 4.6). `sent` is `false` only when the request
/// cannot have run in Jira; [`previous_attempt`] reads it.
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
    .with_details(json!({"code": code, "status": err.status(), "sent": was_sent(err)}));
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

/// Glue for [`autumn_jira::policy::host_may_send`] (plan section 7.2).
///
/// Attempt 1 gives `First`. Else the typed failure of the previous attempt
/// decides: `jira.outcome_pending` gives `MaybeSent`, and
/// `jira.rate_limited` or `jira.network` with `details.sent = false` give
/// `NotSent`. A missing or other payload gives `Unknown`: a worker crash or a
/// `start_to_close` timeout can come after a send.
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
        (RATE_LIMITED_ERROR | NETWORK_ERROR, Some(false)) => Previous::NotSent,
        _ => Previous::Unknown,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
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

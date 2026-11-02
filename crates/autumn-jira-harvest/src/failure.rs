//! Failure mapping: Jira errors to typed harvest failures.
//!
//! Each failure has the error type `jira.<code>`, where `<code>` is the
//! stable code of [`autumn_jira::JiraError::code`]. The retry class comes
//! from the one class table of `autumn-jira`. A failure never holds a body,
//! a header value or a secret.

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

use autumn_harvest::context::ActivityContext;
use autumn_harvest::failure::ActivityFailure;
use autumn_jira::JiraError;
use autumn_jira::policy::Previous;

/// "jira.<code>"; retryable iff err.is_retryable(); RateLimited -> with_retry_after;
/// OutcomeUnknown with marker -> with_retry_after(SETTLE_SECS); details {code, status, sent}.
#[must_use]
pub fn failure_for(err: &JiraError) -> ActivityFailure {
    let _ = err;
    ActivityFailure::retryable("", "")
}

/// Glue for policy::host_may_send (section 7.2).
#[must_use]
pub fn previous_attempt(ctx: &ActivityContext) -> Previous {
    let _ = ctx;
    Previous::First
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

    fn payload(err: &JiraError) -> Option<String> {
        Some(failure_for(err).into_error_payload())
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
                retry_ctx(1, payload(&pending)),
                Previous::First,
            ),
            (
                "outcome pending",
                retry_ctx(2, payload(&pending)),
                Previous::MaybeSent,
            ),
            (
                "rate limited",
                retry_ctx(3, payload(&rate_limited)),
                Previous::NotSent,
            ),
            (
                "network, not sent",
                retry_ctx(2, payload(&not_sent)),
                Previous::NotSent,
            ),
            (
                "network, maybe sent",
                retry_ctx(2, payload(&maybe_sent)),
                Previous::Unknown,
            ),
            (
                "upstream",
                retry_ctx(2, payload(&upstream)),
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

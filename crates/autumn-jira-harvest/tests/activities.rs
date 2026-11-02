//! Activity glue over the public API (AC32). The activity tests that need
//! the client come with WP5 part H1.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::time::Duration;

use autumn_harvest::failure::{IntoActivityErrorString, parse_typed_payload};
use autumn_jira::error::{MarkerRef, Op, UnknownCause};
use autumn_jira::{JiraError, ProjectKey};
use autumn_jira_harvest::{
    ERROR_PREFIX, OUTCOME_PENDING_ERROR, OUTCOME_UNKNOWN_ERROR, SETTLE_SECS, failure_for,
};
use serde_json::{Value, json};

/// One sample of each `JiraError` variant, with the `sent` value and the
/// `retry_after` of its failure.
fn samples() -> Vec<(JiraError, bool, Option<Duration>)> {
    let op = Op::CreateIssue;
    let secret = "s3cr3t-token-value";
    vec![
        (
            JiraError::InvalidInput {
                what: "issue key",
                reason: "must match [A-Z]".into(),
            },
            false,
            None,
        ),
        (JiraError::NotFoundOrForbidden { op }, false, None),
        (
            JiraError::Unauthorized {
                op,
                status: 401,
                hint: autumn_jira::error::SCOPE_HINT,
            },
            false,
            None,
        ),
        (JiraError::CaptchaRequired { op }, false, None),
        (
            JiraError::Validation {
                op,
                status: 400,
                messages: vec![format!("bad value {secret}")],
                fields: vec!["summary".into()],
            },
            false,
            None,
        ),
        (JiraError::Conflict { op }, false, None),
        (JiraError::Gone { op }, false, None),
        (JiraError::PayloadTooLarge { op }, false, None),
        (
            JiraError::RateLimited {
                op,
                retry_after: Some(Duration::from_secs(7)),
                reason: Some("jira-burst-based".into()),
            },
            false,
            Some(Duration::from_secs(7)),
        ),
        (
            JiraError::RateLimited {
                op,
                retry_after: None,
                reason: None,
            },
            false,
            None,
        ),
        (JiraError::Upstream { op, status: 503 }, true, None),
        (
            JiraError::OutcomeUnknown {
                op,
                cause: UnknownCause::ServerError(503),
                marker: Some(MarkerRef::label("autumn-idem-abc".into())),
            },
            true,
            Some(Duration::from_secs(SETTLE_SECS)),
        ),
        (
            JiraError::OutcomeUnknown {
                op,
                cause: UnknownCause::Network,
                marker: None,
            },
            true,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "connect",
            },
            true,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "timeout",
            },
            true,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "store_error",
            },
            false,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "connection_refresh_backoff",
            },
            false,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "provider",
            },
            false,
            None,
        ),
        (JiraError::Redirect { op, status: 302 }, true, None),
        (JiraError::UnexpectedStatus { op, status: 202 }, true, None),
        (
            JiraError::Decode {
                op,
                reason: "missing field `id` at line 1 column 2".into(),
            },
            true,
            None,
        ),
        (
            JiraError::PaginationStalled {
                op: Op::Search,
                pages: 3,
            },
            true,
            None,
        ),
        (
            JiraError::PageLimit {
                op: Op::Search,
                max: 10,
            },
            true,
            None,
        ),
        (
            JiraError::TransitionNotAvailable {
                target: "category done".into(),
                available: vec!["Start work".into()],
            },
            false,
            None,
        ),
        (
            JiraError::LabelsNotOnScreen {
                project: ProjectKey::parse("OPS").unwrap(),
            },
            false,
            None,
        ),
        (JiraError::CloudId("no Jira site".into()), false, None),
        (JiraError::Config { code: "no_client" }, false, None),
        (
            JiraError::Auth {
                code: "refresh_exhausted",
            },
            false,
            None,
        ),
    ]
}

#[test]
fn failure_for_table() {
    for (err, sent, retry_after) in samples() {
        let failure = failure_for(&err);
        let name = format!("{err:?}");
        assert_eq!(
            failure.error_type,
            format!("{ERROR_PREFIX}{}", err.code()),
            "{name}"
        );
        assert_eq!(failure.non_retryable, !err.is_retryable(), "{name}");
        assert_eq!(failure.retry_after, retry_after, "{name}");
        assert_eq!(failure.message, err.to_string(), "{name}");
        assert_eq!(
            failure.details,
            Some(json!({"code": err.code(), "status": err.status(), "sent": sent})),
            "{name}"
        );
        // The wire payload keeps the type, the class and the hint.
        let wire = parse_typed_payload(&failure.clone().into_error_payload()).unwrap();
        assert_eq!(wire.error_type, failure.error_type, "{name}");
        assert_eq!(wire.non_retryable, failure.non_retryable, "{name}");
        assert_eq!(wire.retry_after, failure.retry_after, "{name}");
        // No Jira message and no secret in the failure.
        let text = serde_json::to_string(&failure).unwrap();
        assert!(!text.contains("s3cr3t"), "{name}: {text}");
        assert!(!text.contains("bad value"), "{name}: {text}");
    }
}

#[test]
fn outcome_codes_have_their_constants() {
    let op = Op::AddComment;
    let pending = failure_for(&JiraError::OutcomeUnknown {
        op,
        cause: UnknownCause::Timeout,
        marker: Some(MarkerRef::comment("autumn-ref: k".into())),
    });
    assert_eq!(pending.error_type, OUTCOME_PENDING_ERROR);
    assert!(!pending.non_retryable);
    let unknown = failure_for(&JiraError::OutcomeUnknown {
        op,
        cause: UnknownCause::BadReply,
        marker: None,
    });
    assert_eq!(unknown.error_type, OUTCOME_UNKNOWN_ERROR);
    assert!(unknown.non_retryable);
    assert_eq!(
        unknown.details.as_ref().and_then(|d| d.get("status")),
        Some(&Value::Null)
    );
}

//! The error map keeps the retry class that harvest-camel reads (AC37).
//!
//! harvest retries a camel route when `harvest_camel::is_retryable` says so.
//! The map must give `true` there exactly when the client calls the error
//! retryable. Else harvest retries a call that fails again, or gives up on a
//! call that a retry can fix.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "tests with tables"
)]

use std::collections::BTreeSet;
use std::time::Duration;

use autumn_jira::JiraError;
use autumn_jira::error::{MarkerRef, Op, UnknownCause};
use autumn_jira::ids::ProjectKey;
use autumn_jira_camel::{OPERATIONS, to_camel_error};
use camel_api::CamelError;

/// A pinned copy of `harvest_camel::is_retryable`.
///
/// Source: harvest-camel 0.2.0, `src/route.rs`, lines 247 to 270
/// (`pub fn is_retryable(err: &CamelError) -> bool`). The main workspace
/// cannot depend on harvest-camel: it turns on the C builds of libpq and
/// SQLite (plan D14, ADR 0008). The ecosystem test
/// `is_retryable_copy_matches_harvest_camel` compares this copy with the
/// real function on every mapped error. Change both together.
///
/// It is `pub(crate)`, because the ecosystem test includes this file with
/// `#[path]` and calls it.
pub(crate) fn is_retryable(err: &CamelError) -> bool {
    match err {
        CamelError::ValidationError(_)
        | CamelError::TypeConversionFailed(_)
        | CamelError::InvalidUri(_)
        | CamelError::EndpointUri(_)
        | CamelError::Config(_)
        | CamelError::ConfigValidation(_)
        | CamelError::ComponentNotFound(_)
        | CamelError::EndpointCreationFailed(_)
        | CamelError::EndpointCreationFailedWithSource(..)
        | CamelError::Unauthenticated(_)
        | CamelError::Unauthorized(_)
        | CamelError::UnsupportedMediaType { .. }
        | CamelError::NotAcceptable { .. }
        | CamelError::StreamLimitExceeded(_)
        | CamelError::AlreadyConsumed => false,
        CamelError::HttpOperationFailed { status_code, .. } => {
            !(400..500).contains(status_code) || matches!(status_code, 408 | 425 | 429)
        }
        _ => true,
    }
}

/// The codes of the class table in plan section 4.6: one for each of the 22
/// variants, and two for `OutcomeUnknown`.
const CODES: [&str; 23] = [
    "invalid_input",
    "not_found_or_forbidden",
    "unauthorized",
    "captcha_required",
    "validation",
    "conflict",
    "gone",
    "payload_too_large",
    "rate_limited",
    "upstream",
    "outcome_pending",
    "outcome_unknown",
    "network",
    "redirect",
    "unexpected_status",
    "decode",
    "pagination_stalled",
    "page_limit",
    "transition_not_available",
    "labels_not_on_screen",
    "cloud_id",
    "config",
    "auth",
];

/// One or more values of each `JiraError` variant, for each operation.
/// The ecosystem test uses the same samples, so it is `pub(crate)`.
pub(crate) fn every_error() -> Vec<JiraError> {
    let mut out = vec![
        JiraError::InvalidInput {
            what: "issue key",
            reason: "must match [A-Z]".into(),
        },
        JiraError::TransitionNotAvailable {
            target: "status Done".into(),
            available: vec!["Start work".into()],
        },
        JiraError::LabelsNotOnScreen {
            project: ProjectKey::parse("OPS").unwrap(),
        },
        JiraError::CloudId("no site".into()),
        JiraError::Config { code: "no_site" },
        JiraError::Auth {
            code: "token_expired",
        },
    ];
    let causes = [
        UnknownCause::ServerError(500),
        UnknownCause::ServerError(503),
        UnknownCause::Network,
        UnknownCause::Timeout,
        UnknownCause::BadReply,
    ];
    let markers = [
        None,
        Some(MarkerRef::label("jira-idem-abc".into())),
        Some(MarkerRef::comment("[jira-idem:abc]".into())),
    ];
    for op in Op::ALL {
        out.extend([
            JiraError::NotFoundOrForbidden { op },
            JiraError::Unauthorized {
                op,
                status: 401,
                hint: autumn_jira::error::SCOPE_HINT,
            },
            JiraError::Unauthorized {
                op,
                status: 403,
                hint: autumn_jira::error::SCOPE_HINT,
            },
            JiraError::CaptchaRequired { op },
            JiraError::Validation {
                op,
                status: 400,
                messages: vec!["Field 'x' is bad".into()],
                fields: vec!["x".into()],
            },
            JiraError::Validation {
                op,
                status: 422,
                messages: Vec::new(),
                fields: Vec::new(),
            },
            JiraError::Conflict { op },
            JiraError::Gone { op },
            JiraError::PayloadTooLarge { op },
            JiraError::RateLimited {
                op,
                retry_after: Some(Duration::from_secs(7)),
                reason: Some("jira-burst-based".into()),
            },
            JiraError::RateLimited {
                op,
                retry_after: None,
                reason: None,
            },
            JiraError::Network {
                op,
                code: "connect",
            },
            JiraError::Network {
                op,
                code: "timeout",
            },
            JiraError::Redirect { op, status: 302 },
            JiraError::UnexpectedStatus { op, status: 200 },
            JiraError::UnexpectedStatus { op, status: 418 },
            JiraError::Decode {
                op,
                reason: "missing field `id` at line 1 column 2".into(),
            },
            JiraError::PaginationStalled { op, pages: 3 },
            JiraError::PageLimit { op, max: 1_000 },
        ]);
        // The client gives `Upstream` only for a 5xx. The map must keep the
        // class also for a status that does not fit.
        for status in [
            100, 200, 302, 400, 404, 408, 425, 429, 499, 500, 502, 503, 504, 599, 600,
        ] {
            out.push(JiraError::Upstream { op, status });
        }
        for cause in causes {
            for marker in &markers {
                out.push(JiraError::OutcomeUnknown {
                    op,
                    cause,
                    marker: marker.clone(),
                });
            }
        }
    }
    out
}

#[test]
fn the_samples_cover_every_code() {
    let want: BTreeSet<&str> = CODES.into_iter().collect();
    assert_eq!(want.len(), CODES.len(), "no code is listed two times");
    let got: BTreeSet<&str> = every_error().iter().map(JiraError::code).collect();
    assert_eq!(got, want);
}

#[test]
fn map_keeps_the_retry_class_for_every_error() {
    let errors = every_error();
    assert!(errors.len() > 500, "{}", errors.len());
    for err in &errors {
        for operation in OPERATIONS {
            let mapped = to_camel_error(err, operation);
            assert_eq!(
                is_retryable(&mapped),
                err.is_retryable(),
                "{err:?} via {operation} gave {mapped:?}"
            );
        }
    }
}

#[test]
fn each_camel_kind_gives_its_camel_variant() {
    use autumn_jira::error::CamelKind;
    for err in every_error() {
        let mapped = to_camel_error(&err, "createIssue");
        let class = err.class();
        let ok = match (class.camel, &mapped) {
            (CamelKind::Validation, CamelError::ValidationError(_))
            | (CamelKind::Config, CamelError::Config(_))
            | (CamelKind::Unauthenticated, CamelError::Unauthenticated(_))
            | (CamelKind::Io, CamelError::Io(_)) => true,
            (CamelKind::Http(s), CamelError::HttpOperationFailed { status_code, .. }) => {
                s == *status_code
            }
            // `Upstream` with a status that does not fit the HTTP rule.
            (CamelKind::Http(_), CamelError::Io(_)) => err.is_retryable(),
            (CamelKind::Http(_), CamelError::ValidationError(_)) => !err.is_retryable(),
            _ => false,
        };
        assert!(ok, "{err:?} ({:?}) gave {mapped:?}", class.camel);
    }
}

#[test]
fn messages_hold_the_code_and_no_body() {
    for err in every_error() {
        let mapped = to_camel_error(&err, "search");
        let text = mapped.to_string();
        assert!(
            !text.contains("Field 'x' is bad"),
            "no Jira message: {text}"
        );
        match &mapped {
            CamelError::HttpOperationFailed {
                url,
                status_text,
                response_body,
                ..
            } => {
                assert_eq!(url, "jira:search");
                assert_eq!(*response_body, None, "never a reply body");
                assert!(
                    status_text.contains(&format!("(jira {})", err.code())),
                    "{status_text}"
                );
            }
            CamelError::ValidationError(m)
            | CamelError::Config(m)
            | CamelError::Unauthenticated(m)
            | CamelError::Io(m) => {
                assert_eq!(*m, format!("jira {}: {err}", err.code()));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn outcome_unknown_with_a_key_is_io_and_without_is_validation() {
    let with = JiraError::OutcomeUnknown {
        op: Op::CreateIssue,
        cause: UnknownCause::ServerError(503),
        marker: Some(MarkerRef::label("jira-idem-abc".into())),
    };
    match to_camel_error(&with, "createIssue") {
        CamelError::Io(m) => assert!(m.starts_with("jira outcome_pending: "), "{m}"),
        other => panic!("{other:?}"),
    }
    let without = JiraError::OutcomeUnknown {
        op: Op::CreateIssue,
        cause: UnknownCause::Network,
        marker: None,
    };
    match to_camel_error(&without, "createIssue") {
        CamelError::ValidationError(m) => assert!(m.starts_with("jira outcome_unknown: "), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn http_errors_have_the_plan_shape() {
    let err = JiraError::NotFoundOrForbidden { op: Op::GetIssue };
    match to_camel_error(&err, "getIssue") {
        CamelError::HttpOperationFailed {
            method,
            url,
            status_code,
            status_text,
            response_body,
        } => {
            assert_eq!(method, "GET");
            assert_eq!(url, "jira:getIssue");
            assert_eq!(status_code, 404);
            assert_eq!(status_text, "Not Found (jira not_found_or_forbidden)");
            assert_eq!(response_body, None);
        }
        other => panic!("{other:?}"),
    }
    let err = JiraError::RateLimited {
        op: Op::Search,
        retry_after: Some(Duration::from_secs(3)),
        reason: None,
    };
    let mapped = to_camel_error(&err, "search");
    assert_eq!(
        mapped.to_string(),
        "HTTP POST jira:search failed: 429 Too Many Requests (jira rate_limited)"
    );
}

//! Maps Jira client errors to camel errors.
//!
//! harvest-camel reads the camel error to decide on an activity retry
//! (`harvest_camel::is_retryable`). The map keeps that decision equal to
//! [`JiraError::is_retryable`]. It reads the camel kind from the one class
//! table of the core ([`JiraError::class`]):
//!
//! | `CamelKind` | Camel error | harvest-camel retries |
//! |---|---|---|
//! | `Validation` | `ValidationError` | no |
//! | `Config` | `Config` | no |
//! | `Unauthenticated` | `Unauthenticated` | no |
//! | `Http(status)` | `HttpOperationFailed` with that status | 5xx, 408, 425, 429 |
//! | `Io` | `Io` | yes |
//!
//! The text has the stable code of the core, for example
//! `jira rate_limited`. It never has a body, a header value or a token:
//! the `Display` of [`JiraError`] has none (plan section 4.6).

use autumn_jira::JiraError;
use autumn_jira::error::{CamelKind, Op};
use camel_api::CamelError;

/// Maps a [`JiraError`] to a camel error.
///
/// `operation` is the URI operation, for example `createIssue`. An HTTP
/// error has the URL `jira:<operation>`, the HTTP method of the Jira call
/// that failed, and no reply body.
///
/// `harvest_camel::is_retryable(&to_camel_error(err, op))` is
/// `err.is_retryable()` for every error. An `Http` class with a status that
/// does not agree with the HTTP rule of harvest-camel gives `Io` (retryable)
/// or `ValidationError` (not retryable). The client never makes such an
/// error, but the rule must hold for every value.
#[must_use]
pub fn to_camel_error(err: &JiraError, operation: &str) -> CamelError {
    let class = err.class();
    let message = format!("jira {}: {err}", class.code);
    match class.camel {
        CamelKind::Http(status) if http_rule(status) == class.retryable => {
            CamelError::HttpOperationFailed {
                method: op_of(err).map_or("POST", method).to_owned(),
                url: format!("jira:{operation}"),
                status_code: status,
                status_text: format!("{} (jira {})", reason(status), class.code),
                response_body: None,
            }
        }
        CamelKind::Io => CamelError::Io(message),
        CamelKind::Config => CamelError::Config(message),
        CamelKind::Unauthenticated => CamelError::Unauthenticated(message),
        CamelKind::Http(_) if class.retryable => CamelError::Io(message),
        CamelKind::Validation | CamelKind::Http(_) => CamelError::ValidationError(message),
    }
}

/// The retry rule of harvest-camel for `HttpOperationFailed`: a status
/// outside 4xx, or 408, 425 or 429.
const fn http_rule(status: u16) -> bool {
    !(400 <= status && status < 500) || matches!(status, 408 | 425 | 429)
}

/// Returns the operation of an error that has one.
const fn op_of(err: &JiraError) -> Option<Op> {
    match err {
        JiraError::NotFoundOrForbidden { op }
        | JiraError::Unauthorized { op, .. }
        | JiraError::CaptchaRequired { op }
        | JiraError::Validation { op, .. }
        | JiraError::Conflict { op }
        | JiraError::Gone { op }
        | JiraError::PayloadTooLarge { op }
        | JiraError::RateLimited { op, .. }
        | JiraError::Upstream { op, .. }
        | JiraError::OutcomeUnknown { op, .. }
        | JiraError::Network { op, .. }
        | JiraError::Redirect { op, .. }
        | JiraError::UnexpectedStatus { op, .. }
        | JiraError::Decode { op, .. }
        | JiraError::PaginationStalled { op, .. }
        | JiraError::PageLimit { op, .. } => Some(*op),
        _ => None,
    }
}

/// Returns the HTTP method of the Jira call of an operation.
const fn method(op: Op) -> &'static str {
    match op {
        Op::CreateIssue | Op::DoTransition | Op::AddComment | Op::Search | Op::Count => "POST",
        Op::EditIssue | Op::SetProperty => "PUT",
        Op::DeleteIssue => "DELETE",
        _ => "GET",
    }
}

/// Returns the reason phrase for a status, or `HTTP error` for a status
/// with no phrase here.
const fn reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        425 => "Too Early",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "HTTP error",
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn http_errors_name_the_method_of_the_failed_call() {
        let cases = [
            (Op::GetIssue, "GET"),
            (Op::CreateIssue, "POST"),
            (Op::EditIssue, "PUT"),
            (Op::DeleteIssue, "DELETE"),
            (Op::SetProperty, "PUT"),
            (Op::DoTransition, "POST"),
            (Op::AddComment, "POST"),
            (Op::Search, "POST"),
            (Op::Count, "POST"),
            (Op::GetTransitions, "GET"),
        ];
        for (op, method) in cases {
            match to_camel_error(&JiraError::Conflict { op }, "editIssue") {
                CamelError::HttpOperationFailed { method: m, url, .. } => {
                    assert_eq!(m, method, "{op}");
                    assert_eq!(url, "jira:editIssue");
                }
                other => panic!("{other:?}"),
            }
        }
        for op in Op::ALL {
            match to_camel_error(&JiraError::NotFoundOrForbidden { op }, "getIssue") {
                CamelError::HttpOperationFailed { method, .. } => {
                    assert!(
                        ["GET", "POST", "PUT", "DELETE"].contains(&method.as_str()),
                        "{op}: {method}"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn reason_phrases() {
        assert_eq!(reason(404), "Not Found");
        assert_eq!(reason(409), "Conflict");
        assert_eq!(reason(429), "Too Many Requests");
        assert_eq!(reason(500), "Internal Server Error");
        assert_eq!(reason(502), "Bad Gateway");
        assert_eq!(reason(503), "Service Unavailable");
        assert_eq!(reason(504), "Gateway Timeout");
        assert_eq!(reason(599), "HTTP error");
    }
}

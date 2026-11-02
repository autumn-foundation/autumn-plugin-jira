//! Maps Jira client errors to camel errors.

use autumn_jira::JiraError;
use camel_api::CamelError;

/// Maps a [`JiraError`] to a camel error.
///
/// `operation` is the URI operation, for example `createIssue`.
#[must_use]
pub fn to_camel_error(_err: &JiraError, _operation: &str) -> CamelError {
    CamelError::ProcessorError(String::new())
}

/// Returns the reason phrase for a status.
#[allow(dead_code, reason = "RED stub")]
const fn reason(_status: u16) -> &'static str {
    ""
}

#[cfg(test)]
#[allow(clippy::panic, reason = "tests")]
mod tests {
    use autumn_jira::error::Op;

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

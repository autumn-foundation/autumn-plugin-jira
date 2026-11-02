//! Typed errors and the one class table for all hosts.
//!
//! `Display` never holds a reply body, a header value, a query string or a
//! secret. Jira `errorMessages` are in `JiraError::messages`, not in
//! `Display`.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ids::ProjectKey;
use crate::policy::OpClass;

/// `Result` with `JiraError` as the default error.
pub type Result<T, E = JiraError> = std::result::Result<T, E>;

/// The scope hint of `JiraError::Unauthorized` for 401 and 403.
pub const SCOPE_HINT: &str =
    "needs read:jira-work write:jira-work read:jira-user (OAuth) or a valid API token";

/// One client operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Op {
    /// `GET /myself`.
    Myself,
    /// `GET /serverInfo`.
    ServerInfo,
    /// `GET /project/{key}`.
    GetProject,
    /// `GET /project/search`.
    ListProjects,
    /// `GET /issue/createmeta/{project}/issuetypes`.
    ListIssueTypes,
    /// `POST /issue`.
    CreateIssue,
    /// `GET /issue/{issue}`.
    GetIssue,
    /// `PUT /issue/{issue}`.
    EditIssue,
    /// `DELETE /issue/{issue}`.
    DeleteIssue,
    /// `PUT /issue/{issue}/properties/{key}`.
    SetProperty,
    /// `GET /issue/{issue}/properties/{key}`.
    GetProperty,
    /// `GET /issue/{issue}/transitions`.
    GetTransitions,
    /// `POST /issue/{issue}/transitions`.
    DoTransition,
    /// `POST /issue/{issue}/comment`.
    AddComment,
    /// `GET /issue/{issue}/comment`.
    ListComments,
    /// `POST /search/jql`.
    Search,
    /// `POST /search/approximate-count`.
    Count,
    /// `GET accessible-resources` (cloud id discovery).
    Discovery,
}

impl Op {
    /// Every operation, in declaration order.
    pub const ALL: [Self; 18] = [
        Self::Myself,
        Self::ServerInfo,
        Self::GetProject,
        Self::ListProjects,
        Self::ListIssueTypes,
        Self::CreateIssue,
        Self::GetIssue,
        Self::EditIssue,
        Self::DeleteIssue,
        Self::SetProperty,
        Self::GetProperty,
        Self::GetTransitions,
        Self::DoTransition,
        Self::AddComment,
        Self::ListComments,
        Self::Search,
        Self::Count,
        Self::Discovery,
    ];

    /// Returns the base retry class.
    ///
    /// The idempotent client calls pass `MarkedWrite` or `CheckedWrite`
    /// to the policy in place of this class.
    #[must_use]
    pub const fn class(self) -> OpClass {
        match self {
            Self::CreateIssue | Self::DoTransition | Self::AddComment => OpClass::UnsafeWrite,
            Self::EditIssue | Self::DeleteIssue | Self::SetProperty => OpClass::IdempotentWrite,
            Self::Myself
            | Self::ServerInfo
            | Self::GetProject
            | Self::ListProjects
            | Self::ListIssueTypes
            | Self::GetIssue
            | Self::GetProperty
            | Self::GetTransitions
            | Self::ListComments
            | Self::Search
            | Self::Count
            | Self::Discovery => OpClass::Read,
        }
    }

    /// Returns the name, for example `create_issue`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Myself => "myself",
            Self::ServerInfo => "server_info",
            Self::GetProject => "get_project",
            Self::ListProjects => "list_projects",
            Self::ListIssueTypes => "list_issue_types",
            Self::CreateIssue => "create_issue",
            Self::GetIssue => "get_issue",
            Self::EditIssue => "edit_issue",
            Self::DeleteIssue => "delete_issue",
            Self::SetProperty => "set_property",
            Self::GetProperty => "get_property",
            Self::GetTransitions => "get_transitions",
            Self::DoTransition => "do_transition",
            Self::AddComment => "add_comment",
            Self::ListComments => "list_comments",
            Self::Search => "search",
            Self::Count => "count",
            Self::Discovery => "discovery",
        }
    }

    /// Returns the HTTP statuses that mean success for this operation.
    #[must_use]
    pub const fn expected(self) -> &'static [u16] {
        match self {
            Self::CreateIssue | Self::AddComment => &[201],
            Self::EditIssue => &[204, 200],
            Self::DeleteIssue | Self::DoTransition => &[204],
            Self::SetProperty => &[200, 201],
            Self::Myself
            | Self::ServerInfo
            | Self::GetProject
            | Self::ListProjects
            | Self::ListIssueTypes
            | Self::GetIssue
            | Self::GetProperty
            | Self::GetTransitions
            | Self::ListComments
            | Self::Search
            | Self::Count
            | Self::Discovery => &[200],
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why the outcome of a write is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownCause {
    /// Jira replied with this 5xx status.
    ServerError(u16),
    /// The connection failed after the request can have left.
    Network,
    /// The request timed out.
    Timeout,
    /// Jira replied with a 2xx that did not decode.
    BadReply,
}

/// The marker of a marked write. A lookup can find the write by it later.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarkerRef {
    /// The marker label of an idempotent create.
    pub label: Option<String>,
    /// The marker text of an idempotent comment.
    pub comment_marker: Option<String>,
}

impl MarkerRef {
    /// A marker of an idempotent create.
    #[must_use]
    pub const fn label(label: String) -> Self {
        Self {
            label: Some(label),
            comment_marker: None,
        }
    }

    /// A marker of an idempotent comment.
    #[must_use]
    pub const fn comment(marker: String) -> Self {
        Self {
            label: None,
            comment_marker: Some(marker),
        }
    }
}

/// An error of a Jira client call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum JiraError {
    /// The caller gave a bad value. Nothing was sent.
    #[error("invalid {what}: {reason}")]
    InvalidInput {
        /// What is bad, for example `issue key`.
        what: &'static str,
        /// The rule. It never holds the bad value.
        reason: String,
    },
    /// HTTP 404. Jira gives 404 also when the caller has no permission.
    #[error("jira {op}: not found or no permission")]
    NotFoundOrForbidden {
        /// The operation.
        op: Op,
    },
    /// HTTP 401 or 403.
    #[error("jira {op}: unauthorized (HTTP {status}); {hint}")]
    Unauthorized {
        /// The operation.
        op: Op,
        /// 401 or 403.
        status: u16,
        /// The scopes or the token that the call needs.
        hint: &'static str,
    },
    /// Jira asks for a CAPTCHA (`X-Seraph-LoginReason: AUTHENTICATION_DENIED`).
    #[error("jira {op}: CAPTCHA required; log in once in a browser")]
    CaptchaRequired {
        /// The operation.
        op: Op,
    },
    /// HTTP 400 or 422 with Jira error messages.
    #[error("jira {op}: rejected (HTTP {status}): {} message(s), fields {fields:?}", .messages.len())]
    Validation {
        /// The operation.
        op: Op,
        /// 400 or 422.
        status: u16,
        /// Jira `errorMessages` and the messages of `errors`. Not in `Display`.
        messages: Vec<String>,
        /// The field names of Jira `errors`.
        fields: Vec<String>,
    },
    /// HTTP 409.
    #[error("jira {op}: conflict")]
    Conflict {
        /// The operation.
        op: Op,
    },
    /// HTTP 410: Jira removed the endpoint.
    #[error("jira {op}: gone")]
    Gone {
        /// The operation.
        op: Op,
    },
    /// HTTP 413.
    #[error("jira {op}: payload too large")]
    PayloadTooLarge {
        /// The operation.
        op: Op,
    },
    /// HTTP 429, and no attempts are left (or the write is not safe to send again).
    #[error("jira {op}: rate limited")]
    RateLimited {
        /// The operation.
        op: Op,
        /// The `Retry-After` value, in whole seconds.
        retry_after: Option<Duration>,
        /// The `RateLimit-Reason` value.
        reason: Option<String>,
    },
    /// HTTP 5xx, and no attempts are left.
    #[error("jira {op}: upstream HTTP {status}")]
    Upstream {
        /// The operation.
        op: Op,
        /// The status.
        status: u16,
    },
    /// The write can have reached Jira. The client did not send it again.
    #[error("jira {op}: outcome unknown ({cause:?}); not sent again")]
    OutcomeUnknown {
        /// The operation.
        op: Op,
        /// Why the outcome is unknown.
        cause: UnknownCause,
        /// The marker. A later lookup can find the write by it.
        marker: Option<MarkerRef>,
    },
    /// A network error, and no attempts are left.
    #[error("jira {op}: network error ({code})")]
    Network {
        /// The operation.
        op: Op,
        /// A short code, for example `connect`.
        code: &'static str,
    },
    /// HTTP 3xx. The client never follows a redirect.
    #[error("jira {op}: redirect (HTTP {status}) not followed")]
    Redirect {
        /// The operation.
        op: Op,
        /// The status.
        status: u16,
    },
    /// A status that the operation does not expect.
    #[error("jira {op}: unexpected HTTP {status}")]
    UnexpectedStatus {
        /// The operation.
        op: Op,
        /// The status.
        status: u16,
    },
    /// A reply that does not decode.
    #[error("jira {op}: bad reply: {reason}")]
    Decode {
        /// The operation.
        op: Op,
        /// A serde error kind and a JSON path. Never the body.
        reason: String,
    },
    /// Jira sent a page token again.
    #[error("jira {op}: page token repeated after {pages} page(s)")]
    PaginationStalled {
        /// The operation.
        op: Op,
        /// The pages before the repeat.
        pages: u32,
    },
    /// The page limit is reached.
    #[error("jira {op}: page limit {max} reached")]
    PageLimit {
        /// The operation.
        op: Op,
        /// The limit.
        max: u32,
    },
    /// No transition goes to the target status.
    #[error("no transition to {target}; available: {available:?}")]
    TransitionNotAvailable {
        /// The target, as text.
        target: String,
        /// The names of the available transitions.
        available: Vec<String>,
    },
    /// The Labels field is not on the create screen, so a marked create cannot work.
    #[error(
        "labels are not on the create screen of {project}; add the Labels field or use create_issue"
    )]
    LabelsNotOnScreen {
        /// The project.
        project: ProjectKey,
    },
    /// The cloud id cannot be found.
    #[error("cloud id: {0}")]
    CloudId(String),
    /// The client or the backend has a bad configuration.
    #[error("jira config ({code})")]
    Config {
        /// A short code.
        code: &'static str,
    },
    /// The credentials cannot be used.
    #[error("jira auth ({code})")]
    Auth {
        /// A short code.
        code: &'static str,
    },
}

/// How a camel host shows an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CamelKind {
    /// A validation error. A retry does not help.
    Validation,
    /// A configuration error.
    Config,
    /// The credentials are bad.
    Unauthenticated,
    /// An HTTP error with this status.
    Http(u16),
    /// An I/O error. A retry can help.
    Io,
}

/// The class of an error: one table for all hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorClass {
    /// A stable short code, for example `rate_limited`.
    pub code: &'static str,
    /// `true` when a later retry can help.
    pub retryable: bool,
    /// The HTTP status that an app gives to its own caller.
    pub http_status: u16,
    /// The camel error kind.
    pub camel: CamelKind,
}

impl JiraError {
    /// Makes an `InvalidInput` error.
    pub(crate) fn invalid(what: &'static str, reason: impl Into<String>) -> Self {
        Self::InvalidInput {
            what,
            reason: reason.into(),
        }
    }

    /// Returns the class. One table for all hosts: each host map is one
    /// line over this.
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        use CamelKind::{Config, Http, Io, Unauthenticated, Validation};
        let (code, retryable, http_status, camel) = match self {
            Self::InvalidInput { .. } => ("invalid_input", false, 400, Validation),
            Self::NotFoundOrForbidden { .. } => ("not_found_or_forbidden", false, 404, Http(404)),
            Self::Unauthorized { .. } => ("unauthorized", false, 502, Unauthenticated),
            Self::CaptchaRequired { .. } => ("captcha_required", false, 502, Unauthenticated),
            Self::Validation { .. } => ("validation", false, 422, Validation),
            Self::Conflict { .. } => ("conflict", false, 409, Http(409)),
            Self::Gone { .. } => ("gone", false, 502, Config),
            Self::PayloadTooLarge { .. } => ("payload_too_large", false, 413, Validation),
            Self::RateLimited { .. } => ("rate_limited", true, 503, Http(429)),
            Self::Upstream { status, .. } => ("upstream", true, 502, Http(*status)),
            Self::OutcomeUnknown {
                marker: Some(_), ..
            } => ("outcome_pending", true, 502, Io),
            Self::OutcomeUnknown { marker: None, .. } => {
                ("outcome_unknown", false, 502, Validation)
            }
            Self::Network { .. } => ("network", true, 502, Io),
            Self::Redirect { .. } => ("redirect", false, 502, Config),
            Self::UnexpectedStatus { .. } => ("unexpected_status", false, 502, Config),
            Self::Decode { .. } => ("decode", false, 502, Validation),
            Self::PaginationStalled { .. } => ("pagination_stalled", false, 502, Validation),
            Self::PageLimit { .. } => ("page_limit", false, 502, Validation),
            Self::TransitionNotAvailable { .. } => {
                ("transition_not_available", false, 409, Validation)
            }
            Self::LabelsNotOnScreen { .. } => ("labels_not_on_screen", false, 422, Validation),
            Self::CloudId(_) => ("cloud_id", false, 502, Config),
            Self::Config { .. } => ("config", false, 500, Config),
            Self::Auth { .. } => ("auth", false, 502, Unauthenticated),
        };
        ErrorClass {
            code,
            retryable,
            http_status,
            camel,
        }
    }

    /// Returns the stable code: `class().code`.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.class().code
    }

    /// Returns `true` when a later retry can help: `class().retryable`.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.class().retryable
    }

    /// Returns the HTTP status of the Jira reply, when there is one.
    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        match self {
            Self::NotFoundOrForbidden { .. } => Some(404),
            Self::Conflict { .. } => Some(409),
            Self::Gone { .. } => Some(410),
            Self::PayloadTooLarge { .. } => Some(413),
            Self::RateLimited { .. } => Some(429),
            Self::Unauthorized { status, .. }
            | Self::Validation { status, .. }
            | Self::Upstream { status, .. }
            | Self::Redirect { status, .. }
            | Self::UnexpectedStatus { status, .. }
            | Self::OutcomeUnknown {
                cause: UnknownCause::ServerError(status),
                ..
            } => Some(*status),
            _ => None,
        }
    }

    /// Returns the Jira error messages. They are not in `Display`.
    #[must_use]
    pub fn messages(&self) -> &[String] {
        match self {
            Self::Validation { messages, .. } => messages,
            _ => &[],
        }
    }

    /// Returns the `Retry-After` wait of a rate limit.
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Returns `false` only when the request cannot have run in Jira.
    ///
    /// A durable host reads it after a failed attempt (plan 7.2).
    #[must_use]
    pub fn may_have_run(&self) -> bool {
        true
    }
}

/// The codes of a `JiraError::Network` that did not leave this process.
pub const NOT_SENT_NETWORK_CODES: [&str; 3] =
    ["store_error", "connection_refresh_backoff", "provider"];

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "tests with tables"
)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn may_have_run_table() {
        let op = Op::CreateIssue;
        let not_run = [
            JiraError::invalid("issue key", "bad"),
            JiraError::Config { code: "no_client" },
            JiraError::Auth {
                code: "refresh_exhausted",
            },
            JiraError::CloudId("none".into()),
            JiraError::NotFoundOrForbidden { op },
            JiraError::Unauthorized {
                op,
                status: 401,
                hint: SCOPE_HINT,
            },
            JiraError::CaptchaRequired { op },
            JiraError::Validation {
                op,
                status: 400,
                messages: vec![],
                fields: vec![],
            },
            JiraError::Conflict { op },
            JiraError::Gone { op },
            JiraError::PayloadTooLarge { op },
            JiraError::RateLimited {
                op,
                retry_after: None,
                reason: None,
            },
            JiraError::TransitionNotAvailable {
                target: "done".into(),
                available: vec![],
            },
            JiraError::LabelsNotOnScreen {
                project: crate::ids::ProjectKey::parse("OPS").unwrap(),
            },
            JiraError::Network {
                op,
                code: "store_error",
            },
            JiraError::Network {
                op,
                code: "connection_refresh_backoff",
            },
            JiraError::Network {
                op,
                code: "provider",
            },
        ];
        for err in &not_run {
            assert!(!err.may_have_run(), "{err:?}");
        }
        let may_run = [
            JiraError::Upstream { op, status: 503 },
            JiraError::Network {
                op,
                code: "connect",
            },
            JiraError::Network {
                op,
                code: "timeout",
            },
            JiraError::Network {
                op,
                code: "upstream",
            },
            JiraError::OutcomeUnknown {
                op,
                cause: UnknownCause::Network,
                marker: None,
            },
            JiraError::Redirect { op, status: 302 },
            JiraError::UnexpectedStatus { op, status: 202 },
            JiraError::Decode {
                op,
                reason: "x".into(),
            },
            JiraError::PaginationStalled { op, pages: 2 },
            JiraError::PageLimit { op, max: 2 },
        ];
        for err in &may_run {
            assert!(err.may_have_run(), "{err:?}");
        }
        assert_eq!(
            NOT_SENT_NETWORK_CODES,
            ["store_error", "connection_refresh_backoff", "provider"]
        );
    }

    /// Returns the variant index. The match has no `_` arm, so a new
    /// variant fails to compile here until the table has a row.
    const fn variant_index(e: &JiraError) -> usize {
        match e {
            JiraError::InvalidInput { .. } => 0,
            JiraError::NotFoundOrForbidden { .. } => 1,
            JiraError::Unauthorized { .. } => 2,
            JiraError::CaptchaRequired { .. } => 3,
            JiraError::Validation { .. } => 4,
            JiraError::Conflict { .. } => 5,
            JiraError::Gone { .. } => 6,
            JiraError::PayloadTooLarge { .. } => 7,
            JiraError::RateLimited { .. } => 8,
            JiraError::Upstream { .. } => 9,
            JiraError::OutcomeUnknown { .. } => 10,
            JiraError::Network { .. } => 11,
            JiraError::Redirect { .. } => 12,
            JiraError::UnexpectedStatus { .. } => 13,
            JiraError::Decode { .. } => 14,
            JiraError::PaginationStalled { .. } => 15,
            JiraError::PageLimit { .. } => 16,
            JiraError::TransitionNotAvailable { .. } => 17,
            JiraError::LabelsNotOnScreen { .. } => 18,
            JiraError::CloudId(_) => 19,
            JiraError::Config { .. } => 20,
            JiraError::Auth { .. } => 21,
        }
    }

    const VARIANTS: usize = 22;

    /// One sample of each variant, and the row of the plan table (4.6).
    pub(crate) fn samples() -> Vec<(JiraError, &'static str, bool, u16, CamelKind)> {
        let op = Op::CreateIssue;
        vec![
            (
                JiraError::invalid("issue key", "must match a rule"),
                "invalid_input",
                false,
                400,
                CamelKind::Validation,
            ),
            (
                JiraError::NotFoundOrForbidden { op },
                "not_found_or_forbidden",
                false,
                404,
                CamelKind::Http(404),
            ),
            (
                JiraError::Unauthorized {
                    op,
                    status: 401,
                    hint: SCOPE_HINT,
                },
                "unauthorized",
                false,
                502,
                CamelKind::Unauthenticated,
            ),
            (
                JiraError::CaptchaRequired { op },
                "captcha_required",
                false,
                502,
                CamelKind::Unauthenticated,
            ),
            (
                JiraError::Validation {
                    op,
                    status: 400,
                    messages: vec!["Field 'priority' is required".to_owned()],
                    fields: vec!["priority".to_owned()],
                },
                "validation",
                false,
                422,
                CamelKind::Validation,
            ),
            (
                JiraError::Conflict { op },
                "conflict",
                false,
                409,
                CamelKind::Http(409),
            ),
            (
                JiraError::Gone { op },
                "gone",
                false,
                502,
                CamelKind::Config,
            ),
            (
                JiraError::PayloadTooLarge { op },
                "payload_too_large",
                false,
                413,
                CamelKind::Validation,
            ),
            (
                JiraError::RateLimited {
                    op,
                    retry_after: Some(Duration::from_secs(7)),
                    reason: Some("jira-burst-based".to_owned()),
                },
                "rate_limited",
                true,
                503,
                CamelKind::Http(429),
            ),
            (
                JiraError::Upstream { op, status: 503 },
                "upstream",
                true,
                502,
                CamelKind::Http(503),
            ),
            (
                JiraError::OutcomeUnknown {
                    op,
                    cause: UnknownCause::ServerError(503),
                    marker: Some(MarkerRef::label("autumn-idem-abc".to_owned())),
                },
                "outcome_pending",
                true,
                502,
                CamelKind::Io,
            ),
            (
                JiraError::OutcomeUnknown {
                    op,
                    cause: UnknownCause::Network,
                    marker: None,
                },
                "outcome_unknown",
                false,
                502,
                CamelKind::Validation,
            ),
            (
                JiraError::Network {
                    op,
                    code: "connect",
                },
                "network",
                true,
                502,
                CamelKind::Io,
            ),
            (
                JiraError::Redirect { op, status: 302 },
                "redirect",
                false,
                502,
                CamelKind::Config,
            ),
            (
                JiraError::UnexpectedStatus { op, status: 200 },
                "unexpected_status",
                false,
                502,
                CamelKind::Config,
            ),
            (
                JiraError::Decode {
                    op,
                    reason: "invalid type at .key".to_owned(),
                },
                "decode",
                false,
                502,
                CamelKind::Validation,
            ),
            (
                JiraError::PaginationStalled { op, pages: 3 },
                "pagination_stalled",
                false,
                502,
                CamelKind::Validation,
            ),
            (
                JiraError::PageLimit { op, max: 1_000 },
                "page_limit",
                false,
                502,
                CamelKind::Validation,
            ),
            (
                JiraError::TransitionNotAvailable {
                    target: "Done".to_owned(),
                    available: vec!["Start".to_owned()],
                },
                "transition_not_available",
                false,
                409,
                CamelKind::Validation,
            ),
            (
                JiraError::LabelsNotOnScreen {
                    project: ProjectKey::parse("OPS").unwrap(),
                },
                "labels_not_on_screen",
                false,
                422,
                CamelKind::Validation,
            ),
            (
                JiraError::CloudId("no Jira site".to_owned()),
                "cloud_id",
                false,
                502,
                CamelKind::Config,
            ),
            (
                JiraError::Config { code: "no_client" },
                "config",
                false,
                500,
                CamelKind::Config,
            ),
            (
                JiraError::Auth {
                    code: "refresh_exhausted",
                },
                "auth",
                false,
                502,
                CamelKind::Unauthenticated,
            ),
        ]
    }

    #[test]
    fn class_table_covers_every_variant() {
        let rows = samples();
        let covered: BTreeSet<usize> = rows.iter().map(|(e, ..)| variant_index(e)).collect();
        assert_eq!(covered.len(), VARIANTS);
        for (e, code, retryable, http_status, camel) in rows {
            let expected = ErrorClass {
                code,
                retryable,
                http_status,
                camel,
            };
            assert_eq!(e.class(), expected, "{e:?}");
            assert_eq!(e.code(), code);
            assert_eq!(e.is_retryable(), retryable);
        }
    }

    #[test]
    fn codes_are_unique() {
        let rows = samples();
        let codes: BTreeSet<&str> = rows.iter().map(|(e, ..)| e.code()).collect();
        assert_eq!(codes.len(), rows.len());
        assert!(
            codes
                .iter()
                .all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        );
    }

    #[test]
    fn op_expected_table() {
        let rows: [(Op, &str, OpClass, &[u16]); 18] = [
            (Op::Myself, "myself", OpClass::Read, &[200]),
            (Op::ServerInfo, "server_info", OpClass::Read, &[200]),
            (Op::GetProject, "get_project", OpClass::Read, &[200]),
            (Op::ListProjects, "list_projects", OpClass::Read, &[200]),
            (
                Op::ListIssueTypes,
                "list_issue_types",
                OpClass::Read,
                &[200],
            ),
            (
                Op::CreateIssue,
                "create_issue",
                OpClass::UnsafeWrite,
                &[201],
            ),
            (Op::GetIssue, "get_issue", OpClass::Read, &[200]),
            (
                Op::EditIssue,
                "edit_issue",
                OpClass::IdempotentWrite,
                &[204, 200],
            ),
            (
                Op::DeleteIssue,
                "delete_issue",
                OpClass::IdempotentWrite,
                &[204],
            ),
            (
                Op::SetProperty,
                "set_property",
                OpClass::IdempotentWrite,
                &[200, 201],
            ),
            (Op::GetProperty, "get_property", OpClass::Read, &[200]),
            (Op::GetTransitions, "get_transitions", OpClass::Read, &[200]),
            (
                Op::DoTransition,
                "do_transition",
                OpClass::UnsafeWrite,
                &[204],
            ),
            (Op::AddComment, "add_comment", OpClass::UnsafeWrite, &[201]),
            (Op::ListComments, "list_comments", OpClass::Read, &[200]),
            (Op::Search, "search", OpClass::Read, &[200]),
            (Op::Count, "count", OpClass::Read, &[200]),
            (Op::Discovery, "discovery", OpClass::Read, &[200]),
        ];
        assert_eq!(rows.map(|r| r.0), Op::ALL);
        for (op, name, class, expected) in rows {
            assert_eq!(op.name(), name);
            assert_eq!(op.to_string(), name);
            assert_eq!(op.class(), class, "{name}");
            assert_eq!(op.expected(), expected, "{name}");
        }
    }

    #[test]
    fn accessors() {
        for (e, ..) in samples() {
            let status = e.status();
            match &e {
                JiraError::NotFoundOrForbidden { .. } => assert_eq!(status, Some(404)),
                JiraError::Unauthorized { .. } => assert_eq!(status, Some(401)),
                JiraError::Validation { .. } => {
                    assert_eq!(status, Some(400));
                    assert_eq!(e.messages(), ["Field 'priority' is required"]);
                }
                JiraError::Conflict { .. } => assert_eq!(status, Some(409)),
                JiraError::Gone { .. } => assert_eq!(status, Some(410)),
                JiraError::PayloadTooLarge { .. } => assert_eq!(status, Some(413)),
                JiraError::RateLimited { .. } => {
                    assert_eq!(status, Some(429));
                    assert_eq!(e.retry_after(), Some(Duration::from_secs(7)));
                }
                JiraError::Upstream { .. }
                | JiraError::OutcomeUnknown {
                    cause: UnknownCause::ServerError(_),
                    ..
                } => assert_eq!(status, Some(503)),
                JiraError::Redirect { .. } => assert_eq!(status, Some(302)),
                JiraError::UnexpectedStatus { .. } => assert_eq!(status, Some(200)),
                _ => assert_eq!(status, None, "{e:?}"),
            }
            if !matches!(e, JiraError::Validation { .. }) {
                assert_eq!(e.messages(), &[] as &[String]);
            }
            if !matches!(e, JiraError::RateLimited { .. }) {
                assert_eq!(e.retry_after(), None);
            }
        }
    }

    #[test]
    fn display_holds_no_messages() {
        let e = JiraError::Validation {
            op: Op::CreateIssue,
            status: 400,
            messages: vec!["secret text from the body".to_owned()],
            fields: vec!["summary".to_owned()],
        };
        let text = e.to_string();
        assert_eq!(
            text,
            "jira create_issue: rejected (HTTP 400): 1 message(s), fields [\"summary\"]"
        );
        let unauthorized = JiraError::Unauthorized {
            op: Op::Myself,
            status: 403,
            hint: SCOPE_HINT,
        };
        assert!(unauthorized.to_string().contains("read:jira-work"));
        for (e, ..) in samples() {
            assert!(!e.to_string().contains("Field 'priority'"), "{e}");
        }
    }

    #[test]
    fn marker_ref_shapes() {
        let label = MarkerRef::label("autumn-idem-x".to_owned());
        assert_eq!(label.label.as_deref(), Some("autumn-idem-x"));
        assert_eq!(label.comment_marker, None);
        let comment = MarkerRef::comment("autumn-ref: x".to_owned());
        assert_eq!(comment.label, None);
        let json = serde_json::to_value(&comment).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"label": null, "comment_marker": "autumn-ref: x"})
        );
        assert_eq!(serde_json::from_value::<MarkerRef>(json).unwrap(), comment);
    }
}

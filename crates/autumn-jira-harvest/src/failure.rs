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

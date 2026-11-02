//! A rust-camel 0.57 `jira:` producer component.
//!
//! A route sends an exchange to `jira:<operation>?<options>`. The producer
//! calls the Jira Cloud client of `autumn-jira` and writes the result into
//! the exchange. The component is producer-only.
//!
//! | Operation | Jira call |
//! |---|---|
//! | `createIssue` | create an issue (idempotent with `HarvestIdempotencyKey`) |
//! | `getIssue` | read one issue |
//! | `editIssue` | edit the summary, the labels or other fields |
//! | `transitionToStatus` | move an issue to a status |
//! | `addComment` | add a comment (idempotent with `HarvestIdempotencyKey`) |
//! | `search` | run a JQL search |
//!
//! The component checks every option when the route is added, not at the
//! first message. [`uri_options`] gives the options for the component
//! metadata. [`to_camel_error`] maps a [`autumn_jira::JiraError`] to a
//! [`camel_api::CamelError`]. The map keeps the retry class that
//! harvest-camel reads, so harvest retries exactly the errors that the
//! client calls retryable.

// The camel traits take and return `CamelError` by value. It is large, but
// a box does not match the trait signatures.
#![allow(clippy::result_large_err, reason = "CamelError is 168 bytes")]

mod error;
mod options;

pub use error::to_camel_error;
pub use options::{EndpointOptions, Operation, uri_options};

/// The camel URI scheme of this component.
pub const SCHEME: &str = "jira";

/// The operations: the path of a `jira:` URI.
pub const OPERATIONS: &[&str] = &[
    "createIssue",
    "getIssue",
    "editIssue",
    "transitionToStatus",
    "addComment",
    "search",
];

/// Out header: the issue key. In header only with `allowHeaderOverrides=true`:
/// then it replaces the `issueKey` option.
pub const HEADER_ISSUE_KEY: &str = "JiraIssueKey";

/// Out header: the numeric issue id.
pub const HEADER_ISSUE_ID: &str = "JiraIssueId";

/// Out header: the outcome, `created`, `found`, `moved` or `already_there`.
pub const HEADER_OUTCOME: &str = "JiraOutcome";

/// In header only with `allowHeaderOverrides=true`: then it replaces the
/// `jql` option.
pub const HEADER_JQL: &str = "JiraJql";

/// In header only with `allowHeaderOverrides=true`: then it replaces the
/// `status` option.
pub const HEADER_STATUS: &str = "JiraStatus";

/// Out header of `search`: the token of the next page, when there is one.
pub const HEADER_NEXT_TOKEN: &str = "JiraNextPageToken";

/// The default largest request body: 2 MiB, the harvest input limit.
pub const DEFAULT_MAX_BODY_SIZE: usize = 2 * 1024 * 1024;

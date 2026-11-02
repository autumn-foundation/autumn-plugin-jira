//! A rust-camel 0.57 `jira:` producer component.
//!
//! A route sends an exchange to `jira:<operation>?<options>`. The producer
//! calls the Jira Cloud client of `autumn-jira` and writes the result into
//! the exchange. The component is producer-only: `from("jira:...")` fails.
//!
//! ```no_run
//! # async fn demo(client: autumn_jira::JiraClient) -> Result<(), camel_api::CamelError> {
//! let mut camel = camel_core::CamelContext::builder().build().await?;
//! camel.register_component(autumn_jira_camel::JiraComponent::new(client));
//! # Ok(())
//! # }
//! ```
//!
//! | Operation | Jira call | Body in | Reply body |
//! |---|---|---|---|
//! | `createIssue` | create an issue | `{summary, description?, labels?, fields?}` | `{id, key, outcome}` |
//! | `getIssue` | read one issue | ignored | the issue |
//! | `editIssue` | edit the issue | `{setSummary?, addLabels?, removeLabels?, fields?}` | `{issue}` |
//! | `transitionToStatus` | move the issue to a status | empty, or `{fields}` | `{issue, outcome, status}` |
//! | `addComment` | add a comment | `{text}` or a string | `{issue, id, outcome}` |
//! | `search` | run a JQL search | ignored | `{issues, nextPageToken}` |
//!
//! The reply is `Body::Json` in `input`, and `output` stays `None`. The out
//! headers are [`HEADER_ISSUE_KEY`], [`HEADER_ISSUE_ID`],
//! [`HEADER_OUTCOME`] and [`HEADER_NEXT_TOKEN`]. After its call, each step
//! removes the out headers of an earlier step and sets its own.
//!
//! The component checks every option when the route is added, not at the
//! first message. [`uri_options`] gives the options for the component
//! metadata.
//!
//! # Header overrides
//!
//! With `allowHeaderOverrides=true`, the in headers [`HEADER_ISSUE_KEY`],
//! [`HEADER_STATUS`] and [`HEADER_JQL`] replace the options. Without the
//! flag, such a header that differs from the option fails the exchange with
//! a `ValidationError` that names `allowHeaderOverrides`. A header from an
//! outside caller must not pick the issue, the status or the query.
//!
//! The [`HEADER_ISSUE_KEY`] out header of an earlier `jira:` step is not an
//! override header. Without the flag, the `issueKey` option of the next
//! step applies, so `getIssue?issueKey=OPS-1` and then
//! `addComment?issueKey=OPS-2` comments on OPS-2. With the flag, the out
//! header gives the issue, so a step can act on the issue that an earlier
//! step made. When an other step changes the header, the header is not the
//! out header any more, and the override rule applies.
//!
//! # Idempotency under harvest
//!
//! harvest-camel sends the headers `HarvestIdempotencyKey` and
//! `HarvestActivityAttempt`. With the key, `createIssue` and `addComment`
//! use the idempotent client calls: the write carries a marker, and a
//! lookup finds an earlier write. An attempt above 1 does a lookup only,
//! because the route cannot know if the earlier attempt sent the write.
//! The search index can lag, and harvest-camel gives the attempt no
//! `Retry-After`. So a `createIssue` lookup that finds nothing waits
//! `settleMs` (default 10 s) and looks up one more time. A comment lookup
//! reads the comment list, which is always current, so it does not wait. A
//! negative lookup then fails with `jira outcome_unknown`. The component
//! does not send the write again. Only the unsafe opt-in
//! `resendAfterNegativeLookup=true` sends it again (ADR 0003). Without the
//! key, the component sends a write at most once.
//! `requireIdempotencyKey=true` makes the key header required.
//!
//! # Camel redelivery
//!
//! A camel redelivery (`CamelRedelivered` or `CamelRedeliveryCounter` set)
//! of a marked write does a lookup only, also on harvest attempt 1. A
//! camel redelivery of a write with no key header fails with a
//! `ValidationError`, and the component sends nothing. Thus a camel
//! `retry(..)` never sends a `createIssue` or an `addComment` two times.
//!
//! A `transitionToStatus` on a camel redelivery, or on a harvest attempt
//! above 1, only reads the status. At the target, it gives
//! `already_there`. Else it fails with `jira outcome_unknown`, which is not
//! retryable. The earlier try can have run the transition. Then an
//! automation can have moved the issue on, so a second send can run the
//! transition two times.
//!
//! Do not use camel redelivery on a route that a harvest activity calls:
//! harvest is the one layer that retries. A redelivery of an earlier step
//! also sets these headers. So a later `jira:` write in that exchange
//! also does a lookup only, or reads the status only. The rule fails
//! closed: it never sends a write two times.
//!
//! # Errors
//!
//! [`to_camel_error`] maps a [`autumn_jira::JiraError`] to a
//! [`camel_api::CamelError`]. The map keeps the retry class that
//! harvest-camel reads, so harvest retries exactly the errors that the
//! client calls retryable. No error text holds a body, a header value or a
//! token. With `throwExceptionOnFailure=false`, a failed Jira call gives a
//! reply body `{"error": {code, retryable, message}}` in place of an error.

// The camel traits take and return `CamelError` by value. It is large, but
// a box does not match the trait signatures.
#![allow(clippy::result_large_err, reason = "CamelError is 168 bytes")]

mod component;
mod error;
mod headers;
mod input;
mod options;
mod producer;
mod settle;

pub use component::JiraComponent;
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
/// then it replaces the `issueKey` option. The out header of an earlier
/// `jira:` step does not replace the option without the flag.
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

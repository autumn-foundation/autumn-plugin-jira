//! `autumn_jira_harvest`: Jira for autumn-harvest workflows.
//!
//! - **Durable wait:** [`await_issue_status`] waits until an issue reaches a
//!   status. It races a webhook signal against a poll timer under one
//!   deadline. The deadline comes from `ctx.system_now()`, which the history
//!   records. Webhooks make the wait faster. The wait does not need them.
//! - **Failures:** [`failure_for`] maps each [`autumn_jira::JiraError`] to a
//!   typed `ActivityFailure` with the error type `jira.<code>`.
//!   [`previous_attempt`] reads the failure of the previous attempt for the
//!   safe-write rule of a durable host.
//! - **Activities:** eight `#[activity]` functions for create, get, search,
//!   edit, transition, comment, delete and watch. [`activities()`] returns
//!   them for `HarvestBuilder::activities`.
//! - **Signal bridge:** [`HarvestSignalBridge`] turns a verified Jira
//!   delivery into a harvest signal for the run that waits on the issue.
//! - **Signal routes:** a [`SignalRoute`] names the workflow run that waits
//!   on an issue. Its tag is an HMAC, so a person who edits the issue
//!   property cannot send a signal to another run.
//!
//! Webhooks are at least once. The wait checks the issue id and the
//! timestamp of each signal, so a repeated or an old signal does no harm.
//!
//! See the crate README and `docs/plan.md`.

pub mod activities;
pub mod bridge;
pub mod clients;
pub mod failure;
pub mod route;
pub mod wait;

pub use activities::{
    AddCommentInput, CommentOutput, CreateIssueInput, CreateIssueOutput, CreateKind,
    DeleteIssueInput, DeleteKind, DeleteOutput, EditIssueInput, EditIssueOutput, GET_ISSUE,
    GetIssueInput, SearchPageInput, SearchPageOutput, TransitionInput, TransitionKind,
    TransitionOutput, WatchIssueInput, WatchOutput, activities, jira_add_comment,
    jira_add_comment_info, jira_create_issue, jira_create_issue_info, jira_delete_issue,
    jira_delete_issue_info, jira_edit_issue, jira_edit_issue_info, jira_get_issue,
    jira_get_issue_info, jira_search_page, jira_search_page_info, jira_transition_to_status,
    jira_transition_to_status_info, jira_watch_issue, jira_watch_issue_info,
};
pub use bridge::{BridgeCounters, HarvestSignalBridge, RouteOutcome, routes_from_property, wants};
pub use clients::{ClientRef, JiraClients, client_for};
pub use failure::{
    CONFIG_ERROR, ERROR_PREFIX, INVALID_INPUT_ERROR, OUTCOME_PENDING_ERROR, OUTCOME_UNKNOWN_ERROR,
    SETTLE_SECS, WORKFLOW_ERROR, failure_for, previous_attempt,
};
pub use route::{ROUTES_PROPERTY, RouteKey, SignalRoute, TRACKED_LABEL};
pub use wait::{
    IssueSignal, MAX_POLL_ROUNDS, MAX_SIGNAL_ROUNDS, MAX_WAIT, SIGNAL, StatusWait, WaitOptions,
    WaitSource, await_issue_status,
};

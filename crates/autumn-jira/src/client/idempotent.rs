//! Idempotent writes: a create and a comment with a marker, and a
//! transition with a state check.

use super::{CommentOutcome, CreateOutcome, JiraClient, TransitionOutcome};
use crate::adf::Adf;
use crate::error::Result;
use crate::ids::{IdempotencyKey, IssueRef, ProjectKey};
use crate::model::{Comment, IssueRefs};
use crate::request::{NewIssue, StatusTarget, TransitionFields};

impl JiraClient {
    /// Creates an issue at most once for a key.
    ///
    /// The POST carries the marker label `<prefix><26 chars>` and the issue
    /// property `autumn-jira` with the full key. A lookup before the POST
    /// finds an issue of an earlier call. After a lost reply, a lookup
    /// finds the issue. The client never sends the POST a second time in
    /// one call.
    ///
    /// # Errors
    ///
    /// `OutcomeUnknown` with the marker when the lookups find nothing.
    /// `LabelsNotOnScreen` when the create screen has no Labels field. The
    /// typed errors of the calls.
    pub async fn create_issue_idempotent(
        &self,
        issue: &NewIssue,
        key: &IdempotencyKey,
    ) -> Result<CreateOutcome> {
        let _ = (issue, key);
        Err(super::stub())
    }

    /// Finds the issue of an idempotency key: the marker label and the
    /// `autumn-jira` property must both match. A label-only hit is
    /// skipped. With two or more hits, the lowest id wins.
    ///
    /// # Errors
    ///
    /// The typed errors of the search. A failed search never gives `None`.
    pub async fn find_by_idempotency_key(
        &self,
        project: &ProjectKey,
        key: &IdempotencyKey,
    ) -> Result<Option<IssueRefs>> {
        let _ = (project, key);
        Err(super::stub())
    }

    /// Adds a comment at most once for a key. The last paragraph of the
    /// body is the marker `autumn-ref: <26 chars>`.
    ///
    /// # Errors
    ///
    /// `OutcomeUnknown` with the marker when the lookup finds nothing. The
    /// typed errors of the calls.
    pub async fn add_comment_idempotent(
        &self,
        issue: impl Into<IssueRef> + Send,
        body: &Adf,
        key: &IdempotencyKey,
    ) -> Result<CommentOutcome> {
        let _ = (issue.into(), body, key);
        Err(super::stub())
    }

    /// Finds the comment of an idempotency key in the 100 newest comments.
    ///
    /// # Errors
    ///
    /// The typed errors of the call.
    pub async fn find_comment_by_idempotency_key(
        &self,
        issue: impl Into<IssueRef> + Send,
        key: &IdempotencyKey,
    ) -> Result<Option<Comment>> {
        let _ = (issue.into(), key);
        Err(super::stub())
    }

    /// Moves an issue to a status. Before each attempt, the client reads
    /// the status. At the target, it returns `AlreadyThere` and sends no
    /// transition. Else it sends the first transition to the target. A
    /// retry after a lost reply reads the status again, so it is safe.
    ///
    /// # Errors
    ///
    /// `TransitionNotAvailable` with the names of the transitions when no
    /// transition goes to the target. The typed errors of the calls.
    pub async fn transition_to_status_with(
        &self,
        issue: impl Into<IssueRef> + Send,
        target: &StatusTarget,
        fields: &TransitionFields,
    ) -> Result<TransitionOutcome> {
        let _ = (issue.into(), target, fields);
        Err(super::stub())
    }
}

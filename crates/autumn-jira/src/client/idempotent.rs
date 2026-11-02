//! Idempotent writes: a create and a comment with a marker, and a
//! transition with a state check.
//!
//! The marked writes are `MarkedWrite`. After a 5xx or a "maybe sent"
//! attempt the policy gives `Confirm`: the client looks up the marker and
//! never sends the write again in the same call (D5). A lookup that fails
//! never counts as "not found" (B3).

use std::time::Duration;

use serde_json::{Value, json};

use super::call::{self, Ended, Next};
use super::{
    CommentOutcome, CreateOutcome, JiraClient, TransitionOutcome, comment_request, issue_path,
    transition_request,
};
use crate::adf::Adf;
use crate::backend::{ApiRequest, Method};
use crate::error::{JiraError, MarkerRef, Op, Result, UnknownCause};
use crate::ids::{AccountId, FieldId, IdempotencyKey, IssueRef, ProjectKey};
use crate::jql::{Jql, JqlField};
use crate::model::{Comment, CommentPage, CreatedIssue, IssueRefs, decode};
use crate::policy::{self, OpClass};
use crate::request::{FieldSet, NewIssue, SearchRequest, StatusTarget, TransitionFields};

/// The issue property of an idempotent create.
pub(crate) const MARKER_PROPERTY: &str = "autumn-jira";
/// The key of the full idempotency key in [`MARKER_PROPERTY`].
const KEY_FIELD: &str = "idempotencyKey";
/// The most hits of one marker lookup.
const LOOKUP_RESULTS: u32 = 10;
/// The comments of one page of a comment lookup.
const COMMENT_LOOKUP: u32 = 100;

/// Returns the create body with the marker label and the marker property.
fn marked_body(issue: &NewIssue, key: &IdempotencyKey, label: &str) -> Value {
    let mut body = issue.to_body();
    if let Some(fields) = body.get_mut("fields").and_then(Value::as_object_mut) {
        let labels = fields
            .entry("labels")
            .or_insert_with(|| Value::Array(Vec::new()));
        match labels.as_array_mut() {
            Some(list) => list.push(json!(label)),
            // The app set `labels` to a value that is not a list. The
            // marker must go in, so it replaces the value.
            None => *labels = json!([label]),
        }
    }
    if let Some(object) = body.as_object_mut() {
        let properties = object
            .entry("properties")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(list) = properties.as_array_mut() {
            list.push(json!({"key": MARKER_PROPERTY, "value": {KEY_FIELD: key.as_str()}}));
        }
    }
    body
}

/// The part of the Jira message for a field that the screen does not have.
const NOT_ON_SCREEN: &str = "not on the appropriate screen";

/// Turns a 400 into `LabelsNotOnScreen` when `labels` is the only bad field
/// and its message says that the screen does not have it. Any other 400,
/// for example a label with a space, stays a `Validation` error with the
/// Jira messages.
fn labels_fix(error: JiraError, project: &ProjectKey) -> JiraError {
    match error {
        JiraError::Validation {
            status: 400,
            ref fields,
            ref messages,
            ..
        } if matches!(fields.as_slice(), [f] if f == "labels")
            && messages
                .iter()
                .any(|m| m.starts_with("labels:") && m.contains(NOT_ON_SCREEN)) =>
        {
            JiraError::LabelsNotOnScreen {
                project: project.clone(),
            }
        }
        other => other,
    }
}

/// Returns `true` for a 2xx that is not the expected status. The write can
/// have run, so the client looks up the marker, as for a 2xx that does not
/// decode (B11).
const fn is_odd_success(error: &JiraError) -> bool {
    matches!(
        error,
        JiraError::UnexpectedStatus {
            status: 200..=299,
            ..
        }
    )
}

/// Returns `true` when `own` wrote the comment and a line of it is `marker`.
fn is_marked_by(comment: &Comment, own: &AccountId, marker: &str) -> bool {
    comment
        .author
        .as_ref()
        .is_some_and(|a| &a.account_id == own)
        && comment
            .body
            .as_ref()
            .is_some_and(|b| b.to_plain_text().lines().any(|l| l.trim() == marker))
}

/// The error of a transition that the client does not send again.
const fn transition_unknown(cause: UnknownCause) -> JiraError {
    JiraError::OutcomeUnknown {
        op: Op::DoTransition,
        cause,
        marker: None,
    }
}

/// Orders ids as numbers: a shorter digit string is smaller.
fn id_order(refs: &IssueRefs) -> (usize, &str) {
    (refs.id.as_str().len(), refs.id.as_str())
}

impl JiraClient {
    /// Creates an issue at most once for a key.
    ///
    /// The POST carries the marker label `<prefix><26 chars>` and the issue
    /// property `autumn-jira` with the full key. A lookup before the POST
    /// finds an issue of an earlier call. After a lost reply, a lookup
    /// finds the issue. The client never sends the POST a second time in
    /// one call.
    ///
    /// A 2xx that does not decode, or that is not 201, also goes to the
    /// lookups.
    ///
    /// After `OutcomeUnknown` with the marker (`outcome_pending`), do not
    /// call this function again for the key. The pre-lookup reads a search
    /// index that can lag, so it can miss the first issue. Retry with
    /// [`Self::find_by_idempotency_key`] only (plan 7.2).
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
        let project = issue.project_key();
        if let Some(found) = self.find_by_idempotency_key(project, key).await? {
            return Ok(CreateOutcome::Found(found));
        }
        let label = key.marker_label(&self.settings.marker_label_prefix);
        let request = ApiRequest::new(
            Op::CreateIssue,
            Method::Post,
            format!("{}/issue", super::API),
        )
        .json(&marked_body(issue, key, &label));
        let marker = MarkerRef::label(label);
        let cause = match self.run(OpClass::MarkedWrite, &request).await {
            Ok(Ended::Reply(reply)) => match decode::<CreatedIssue>(Op::CreateIssue, &reply.body) {
                Ok(created) => return Ok(CreateOutcome::Created(created)),
                // A 2xx that does not decode: the issue can exist (B11).
                Err(_) => UnknownCause::BadReply,
            },
            Ok(Ended::Confirm(cause)) => cause,
            Err(failure) if is_odd_success(&failure.error) => UnknownCause::BadReply,
            Err(failure) => return Err(labels_fix(failure.error, project)),
        };
        let lookups = self.settings.lookup_attempts.min(policy::MAX_LOOKUPS);
        let delay = Duration::from_millis(self.settings.lookup_delay_ms);
        for _ in 0..lookups {
            tokio::time::sleep(delay).await;
            match self.find_by_idempotency_key(project, key).await {
                Ok(Some(found)) => return Ok(CreateOutcome::Found(found)),
                Ok(None) => {}
                Err(e) => tracing::warn!(code = e.code(), "jira: a marker lookup failed"),
            }
        }
        Err(JiraError::OutcomeUnknown {
            op: Op::CreateIssue,
            cause,
            marker: Some(marker),
        })
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
        let label = key.marker_label(&self.settings.marker_label_prefix);
        let jql = Jql::project(project).and(Jql::field(JqlField::Labels).eq(&label)?);
        let fields = FieldSet::new([FieldId::known("summary"), FieldId::known("status")]);
        let request = SearchRequest::new(jql)
            .fields(fields)
            .max_results(LOOKUP_RESULTS)
            .properties([MARKER_PROPERTY]);
        let page = self.search_page(&request, None).await?;
        let hits: Vec<IssueRefs> = page
            .issues
            .iter()
            .filter(|issue| {
                issue
                    .property(MARKER_PROPERTY)
                    .and_then(|v| v.get(KEY_FIELD))
                    .and_then(Value::as_str)
                    == Some(key.as_str())
            })
            .map(|issue| IssueRefs::new(issue.id.clone(), issue.key.clone()))
            .collect();
        if hits.len() > 1 {
            tracing::warn!(
                count = hits.len(),
                "jira: more than one issue has the idempotency marker; the lowest id wins"
            );
        }
        Ok(hits
            .into_iter()
            .min_by(|a, b| id_order(a).cmp(&id_order(b))))
    }

    /// Adds a comment at most once for a key. The last paragraph of the
    /// body is the marker `autumn-ref: <26 chars>`.
    ///
    /// After `OutcomeUnknown` with the marker, retry with
    /// [`Self::find_comment_by_idempotency_key`] only, as for
    /// [`Self::create_issue_idempotent`].
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
        let issue = issue.into();
        if let Some(found) = self
            .find_comment_by_idempotency_key(issue.clone(), key)
            .await?
        {
            return Ok(CommentOutcome::Found(found));
        }
        let marker = key.comment_marker();
        let request = comment_request(&issue, &body.clone().with_paragraph(&marker));
        let cause = match self.run(OpClass::MarkedWrite, &request).await {
            Ok(Ended::Reply(reply)) => match decode::<Comment>(Op::AddComment, &reply.body) {
                Ok(comment) => return Ok(CommentOutcome::Created(comment)),
                Err(_) => UnknownCause::BadReply,
            },
            Ok(Ended::Confirm(cause)) => cause,
            Err(failure) if is_odd_success(&failure.error) => UnknownCause::BadReply,
            Err(failure) => return Err(failure.error),
        };
        // The comment list is strongly consistent (A3): one lookup, no delay.
        match self.find_comment_by_idempotency_key(issue, key).await {
            Ok(Some(found)) => return Ok(CommentOutcome::Found(found)),
            Ok(None) => {}
            Err(e) => tracing::warn!(code = e.code(), "jira: a comment lookup failed"),
        }
        Err(JiraError::OutcomeUnknown {
            op: Op::AddComment,
            cause,
            marker: Some(MarkerRef::comment(marker)),
        })
    }

    /// Finds the comment of an idempotency key.
    ///
    /// A match is a comment by the account of the credentials
    /// ([`Self::account_id`]) with a line that is the full marker. A person
    /// can type or quote the marker, but a person is not the author of the
    /// client comments. The lookup reads the newest comments first, 100 for
    /// each page, at most `max_pages` pages. Other comments can come
    /// between two attempts, so the lookup does not stop at one page.
    ///
    /// # Errors
    ///
    /// `PageLimit` when `max_pages` pages do not hold the marker and the
    /// issue has more comments: "not found" is not known then. The typed
    /// errors of the calls. A failed call never gives `None`.
    pub async fn find_comment_by_idempotency_key(
        &self,
        issue: impl Into<IssueRef> + Send,
        key: &IdempotencyKey,
    ) -> Result<Option<Comment>> {
        let marker = key.comment_marker();
        let own = self.account_id().await?;
        let path = format!("{}/comment", issue_path(&issue.into()));
        let max = self.settings.max_pages;
        let mut start = 0_u64;
        let mut pages = 0_u32;
        loop {
            let request = ApiRequest::new(Op::ListComments, Method::Get, path.clone())
                .query("orderBy", "-created")
                .query("startAt", start.to_string())
                .query("maxResults", COMMENT_LOOKUP.to_string());
            let page: CommentPage = self.read(&request).await?;
            pages = pages.saturating_add(1);
            let returned = super::count(page.comments.len());
            if let Some(found) = page
                .comments
                .into_iter()
                .find(|c| is_marked_by(c, &own, &marker))
            {
                return Ok(Some(found));
            }
            let next = policy::offset_next(start, returned, page.total);
            match policy::token_page_step(pages, max, false, next.is_some(), false) {
                policy::PageStep::Fetch => {
                    if let Some(n) = next {
                        start = n;
                    }
                }
                policy::PageStep::Limit => {
                    return Err(JiraError::PageLimit {
                        op: Op::ListComments,
                        max,
                    });
                }
                // `repeated` is `false`, so `Stalled` does not come.
                policy::PageStep::Done | policy::PageStep::Stalled => return Ok(None),
            }
        }
    }

    /// Moves an issue to a status. Before each attempt, the client reads
    /// the status. At the target, it returns `AlreadyThere` and sends no
    /// transition. Else it sends the first transition to the target.
    ///
    /// After a 5xx or a "maybe sent" attempt, the status alone cannot show
    /// that the first attempt did not run. So the client sends again only
    /// when the issue is still at the status of the first attempt, and the
    /// transition is not global and not looped. Such a transition is not
    /// valid after a commit, so Jira rejects a second send. Else the call
    /// stops with `OutcomeUnknown`: an automation can have moved the issue
    /// on, or a global transition can run two times. A 400 or a 409 after
    /// a "maybe sent" attempt makes the client read the status once more:
    /// at the target, it returns `Moved`.
    ///
    /// # Errors
    ///
    /// `TransitionNotAvailable` with the names of the transitions when no
    /// transition goes to the target. `OutcomeUnknown` with no marker when
    /// a send again is not safe. The typed errors of the calls.
    pub async fn transition_to_status_with(
        &self,
        issue: impl Into<IssueRef> + Send,
        target: &StatusTarget,
        fields: &TransitionFields,
    ) -> Result<TransitionOutcome> {
        let issue = issue.into();
        let status_only = FieldSet::new([FieldId::known("status")]);
        let mut attempt = 1_u32;
        // The status before the first attempt, and the cause of the last
        // "maybe sent" attempt.
        let mut origin = None;
        let mut unknown: Option<UnknownCause> = None;
        loop {
            let current = self.get_issue_with(issue.clone(), &status_only).await?;
            let status = current.status();
            let status_id = status.as_ref().map(|s| s.id.clone());
            if let Some(status) = status.filter(|s| target.matches(s)) {
                return Ok(TransitionOutcome::AlreadyThere(status));
            }
            if let Some(cause) = unknown
                && status_id != origin
            {
                return Err(transition_unknown(cause));
            }
            origin = status_id;
            let list = self.transitions(issue.clone()).await?;
            let Some(transition) = list.iter().find(|t| target.matches(&t.to)) else {
                return Err(JiraError::TransitionNotAvailable {
                    target: target.to_string(),
                    available: list.iter().map(|t| t.name.clone()).collect(),
                });
            };
            if let Some(cause) = unknown
                && (transition.is_global || transition.looped)
            {
                return Err(transition_unknown(cause));
            }
            let request = transition_request(&issue, &transition.id, fields);
            let sent = self.backend.send(request).await;
            let maybe = matches!(
                call::classify(&sent),
                policy::Outcome::ServerError | policy::Outcome::MaybeSent
            )
            .then(|| call::unknown_cause(&sent));
            match self.judge(OpClass::CheckedWrite, Op::DoTransition, attempt, sent) {
                Next::Done(_) => {
                    return Ok(TransitionOutcome::Moved {
                        via: transition.id.clone(),
                        to: transition.to.clone(),
                    });
                }
                Next::Wait(wait) => {
                    unknown = maybe.or(unknown);
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                Next::Confirm(cause) => return Err(transition_unknown(cause)),
                Next::Fail(error @ (JiraError::Validation { .. } | JiraError::Conflict { .. }))
                    if unknown.is_some() =>
                {
                    // An earlier attempt can have moved the issue.
                    let now = self.get_issue_with(issue.clone(), &status_only).await?;
                    return now
                        .status()
                        .filter(|s| target.matches(s))
                        .map_or(Err(error), |to| {
                            Ok(TransitionOutcome::Moved {
                                via: transition.id.clone(),
                                to,
                            })
                        });
                }
                Next::Fail(error) => return Err(error),
            }
        }
    }
}

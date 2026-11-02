//! Idempotent writes: a create and a comment with a marker, and a
//! transition with a state check.
//!
//! The marked writes are `MarkedWrite`. After a 5xx or a "maybe sent"
//! attempt the policy gives `Confirm`: the client looks up the marker and
//! never sends the write again in the same call (D5). A lookup that fails
//! never counts as "not found" (B3).

use std::time::Duration;

use serde_json::{Value, json};

use super::call::{Ended, Next};
use super::{
    CommentOutcome, CreateOutcome, JiraClient, TransitionOutcome, comment_request, issue_path,
    transition_request,
};
use crate::adf::Adf;
use crate::backend::{ApiRequest, Method};
use crate::error::{JiraError, MarkerRef, Op, Result, UnknownCause};
use crate::ids::{FieldId, IdempotencyKey, IssueRef, ProjectKey};
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
/// The newest comments that a comment lookup reads.
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

/// Turns a 400 with `errors.labels` into `LabelsNotOnScreen`.
fn labels_fix(error: JiraError, project: &ProjectKey) -> JiraError {
    match error {
        JiraError::Validation {
            status: 400,
            ref fields,
            ..
        } if fields.iter().any(|f| f == "labels") => JiraError::LabelsNotOnScreen {
            project: project.clone(),
        },
        other => other,
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
        let marker = key.comment_marker();
        let path = format!("{}/comment", issue_path(&issue.into()));
        let request = ApiRequest::new(Op::ListComments, Method::Get, path)
            .query("orderBy", "-created")
            .query("maxResults", COMMENT_LOOKUP.to_string());
        let page: CommentPage = self.read(&request).await?;
        Ok(page
            .comments
            .into_iter()
            .find(|c| c.body.as_ref().is_some_and(|b| b.contains_text(&marker))))
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
        let issue = issue.into();
        let status_only = FieldSet::new([FieldId::known("status")]);
        let mut attempt = 1_u32;
        loop {
            let current = self.get_issue_with(issue.clone(), &status_only).await?;
            if let Some(status) = current.status().filter(|s| target.matches(s)) {
                return Ok(TransitionOutcome::AlreadyThere(status));
            }
            let list = self.transitions(issue.clone()).await?;
            let Some(transition) = list.iter().find(|t| target.matches(&t.to)) else {
                return Err(JiraError::TransitionNotAvailable {
                    target: target.to_string(),
                    available: list.iter().map(|t| t.name.clone()).collect(),
                });
            };
            let request = transition_request(&issue, &transition.id, fields);
            let sent = self.backend.send(request).await;
            match self.judge(OpClass::CheckedWrite, Op::DoTransition, attempt, sent) {
                Next::Done(_) => {
                    return Ok(TransitionOutcome::Moved {
                        via: transition.id.clone(),
                        to: transition.to.clone(),
                    });
                }
                Next::Wait(wait) => {
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                Next::Confirm(cause) => {
                    return Err(JiraError::OutcomeUnknown {
                        op: Op::DoTransition,
                        cause,
                        marker: None,
                    });
                }
                Next::Fail(error) => return Err(error),
            }
        }
    }
}

//! The `jira:` producer: one Jira call for each exchange.
//!
//! The reply goes into `input` as `Body::Json`, and `output` stays `None`
//! (the san-mateo-camel convention). The out headers carry the issue key,
//! the issue id, the outcome and the next page token. After the call, the
//! producer removes the out headers of an earlier step, so each out header
//! belongs to this step.
//!
//! The call reads the in headers before that removal, so the
//! `JiraIssueKey` out header of an earlier `jira:` step is still there. The
//! exchange property [`OWN_ISSUE_KEY`] holds the value that the producer
//! set. Without `allowHeaderOverrides`, a `JiraIssueKey` header equal to
//! that value is not an override header: the `issueKey` option applies.
//! With `allowHeaderOverrides`, the header gives the issue, so a step can
//! act on the issue of an earlier step.

use std::sync::Arc;
use std::time::Duration;

use autumn_jira::client::{CommentOutcome, CreateOutcome, IssuePage, TransitionOutcome};
use autumn_jira::error::{Op, UnknownCause};
use autumn_jira::ids::{IssueRef, PageToken};
use autumn_jira::policy::{self, PageStep, host_may_send};
use autumn_jira::request::{EditOptions, NewIssue, SearchRequest};
use autumn_jira::{Adf, JiraClient, JiraError};
use camel_api::{Body, CamelError, Exchange, Message, Value};
use serde_json::json;

use crate::error::to_camel_error;
use crate::headers::{self, Marked};
use crate::input;
use crate::options::{EndpointOptions, Operation};
use crate::settle::settle;
use crate::{HEADER_ISSUE_ID, HEADER_ISSUE_KEY, HEADER_NEXT_TOKEN, HEADER_OUTCOME};

/// The outcome of a write that made the issue or the comment.
const CREATED: &str = "created";
/// The outcome of a write that a marker lookup found.
const FOUND: &str = "found";
/// The outcome of a transition that this call sent.
const MOVED: &str = "moved";
/// The outcome of a transition that found the issue at the target.
const ALREADY_THERE: &str = "already_there";

/// The exchange property with the `JiraIssueKey` value that the last
/// `jira:` step set. Only this component writes it. An outside caller
/// sets headers, not exchange properties.
pub(crate) const OWN_ISSUE_KEY: &str = "autumn-jira-camel.ownIssueKey";

/// The out headers. The producer removes them after each call.
const OUT_HEADERS: [&str; 4] = [
    HEADER_ISSUE_KEY,
    HEADER_ISSUE_ID,
    HEADER_OUTCOME,
    HEADER_NEXT_TOKEN,
];

/// Sends the exchanges of one endpoint to Jira.
#[derive(Debug, Clone)]
pub(crate) struct Producer {
    client: JiraClient,
    options: Arc<EndpointOptions>,
}

/// The result of one call, before it goes into the exchange.
#[derive(Debug, Default)]
struct Reply {
    body: Value,
    key: Option<String>,
    id: Option<String>,
    outcome: Option<&'static str>,
    next_token: Option<String>,
}

impl Reply {
    /// A reply with the key or the id of an issue ref, and an outcome.
    fn for_issue(issue: &IssueRef, body: Value, outcome: Option<&'static str>) -> Self {
        let (key, id) = match issue {
            IssueRef::Key(key) => (Some(key.as_str().to_owned()), None),
            IssueRef::Id(id) => (None, Some(id.as_str().to_owned())),
        };
        Self {
            body,
            key,
            id,
            outcome,
            next_token: None,
        }
    }

    /// The reply of a create.
    fn issue(id: &str, key: &str, outcome: &'static str) -> Self {
        Self {
            body: json!({"id": id, "key": key, "outcome": outcome}),
            key: Some(key.to_owned()),
            id: Some(id.to_owned()),
            outcome: Some(outcome),
            next_token: None,
        }
    }

    /// The reply of a comment.
    fn comment(issue: &IssueRef, id: &str, outcome: &'static str) -> Self {
        let body = json!({"issue": issue.to_string(), "id": id, "outcome": outcome});
        Self::for_issue(issue, body, Some(outcome))
    }

    /// Puts the reply into the message.
    fn apply(self, message: &mut Message) {
        message.body = Body::Json(self.body);
        let headers = [
            (HEADER_ISSUE_KEY, self.key),
            (HEADER_ISSUE_ID, self.id),
            (HEADER_OUTCOME, self.outcome.map(str::to_owned)),
            (HEADER_NEXT_TOKEN, self.next_token),
        ];
        for (name, value) in headers {
            if let Some(value) = value {
                message.set_header(name, value);
            }
        }
    }
}

/// The in message of a call, and the `JiraIssueKey` value that an earlier
/// `jira:` step set.
#[derive(Debug, Clone, Copy)]
struct In<'a> {
    message: &'a Message,
    own: Option<&'a Value>,
}

/// A failure of one exchange.
#[derive(Debug)]
enum Failure {
    /// The Jira call failed. `throwExceptionOnFailure` applies.
    Jira(JiraError),
    /// The request is bad, or the host must not send the write. The
    /// exchange always fails.
    Camel(CamelError),
}

impl From<JiraError> for Failure {
    fn from(error: JiraError) -> Self {
        Self::Jira(error)
    }
}

impl From<CamelError> for Failure {
    fn from(error: CamelError) -> Self {
        Self::Camel(error)
    }
}

impl Producer {
    /// Makes the producer of one endpoint.
    pub(crate) const fn new(client: JiraClient, options: Arc<EndpointOptions>) -> Self {
        Self { client, options }
    }

    /// Sends one exchange and puts the reply into it.
    pub(crate) async fn send(&self, mut exchange: Exchange) -> Result<Exchange, CamelError> {
        let op = self.options.operation;
        let body = std::mem::take(&mut exchange.input.body);
        let own = exchange.properties.remove(OWN_ISSUE_KEY);
        let result = self.call(&exchange.input, own.as_ref(), body).await;
        let message = &mut exchange.input;
        for name in OUT_HEADERS {
            message.headers.remove(name);
        }
        match result {
            Ok(reply) => {
                tracing::debug!(operation = %op, outcome = reply.outcome, "jira: camel call done");
                if let Some(key) = &reply.key {
                    exchange
                        .properties
                        .insert(OWN_ISSUE_KEY.to_owned(), Value::String(key.clone()));
                }
                reply.apply(&mut exchange.input);
                Ok(exchange)
            }
            Err(Failure::Jira(error)) if !self.options.throw_exception_on_failure => {
                tracing::debug!(operation = %op, code = error.code(), "jira: camel call failed");
                // The Display of a JiraError holds no body and no secret.
                message.body = Body::Json(json!({"error": {
                    "code": error.code(),
                    "retryable": error.is_retryable(),
                    "message": error.to_string(),
                }}));
                Ok(exchange)
            }
            Err(Failure::Jira(error)) => Err(to_camel_error(&error, op.name())),
            Err(Failure::Camel(error)) => Err(error),
        }
    }

    /// Reads the request and makes the Jira call. Every check of the body
    /// and the headers comes before the first call.
    ///
    /// `own` is the `JiraIssueKey` value that an earlier `jira:` step set.
    async fn call(
        &self,
        message: &Message,
        own: Option<&Value>,
        body: Body,
    ) -> Result<Reply, Failure> {
        let op = self.options.operation;
        let max = self.options.max_body_size;
        let read = In { message, own };
        match op {
            Operation::CreateIssue => {
                self.create(message, input::read(body, op, max).await?)
                    .await
            }
            Operation::GetIssue => self.get(read).await,
            Operation::EditIssue => self.edit(read, input::read(body, op, max).await?).await,
            Operation::TransitionToStatus => {
                self.transition(read, input::read(body, op, max).await?)
                    .await
            }
            Operation::AddComment => self.comment(read, input::read(body, op, max).await?).await,
            Operation::Search => self.search(message).await,
        }
    }

    /// `createIssue`: a plain create without a key, else a marked create.
    async fn create(&self, message: &Message, raw: input::Raw) -> Result<Reply, Failure> {
        let options = &self.options;
        let op = options.operation;
        let body = input::create(raw)?;
        let marked = headers::marked(message, options)?;
        let (Some(project), Some(issue_type)) = (&options.project, &options.issue_type) else {
            // The parse makes both options required, so this is a guard.
            return Err(Failure::Camel(CamelError::ValidationError(format!(
                "{op} needs the project and issueType options"
            ))));
        };
        let mut issue =
            NewIssue::new(project.clone(), issue_type.clone(), body.summary).labels(body.labels);
        if let Some(description) = body.description {
            issue = issue.description(description);
        }
        for (id, value) in body.fields {
            issue = issue.field(id, value);
        }
        let Some(marked) = marked else {
            let created = self.client.create_issue(&issue).await?;
            return Ok(Reply::issue(
                created.id.as_str(),
                created.key.as_str(),
                CREATED,
            ));
        };
        if !self.may_send(&marked) {
            let mut found = self
                .client
                .find_by_idempotency_key(project, &marked.key)
                .await?;
            if found.is_none() && options.settle_ms > 0 {
                // The search index can lag behind the create of an earlier
                // attempt. harvest-camel gives the attempt no Retry-After,
                // so the producer waits here (ADR 0003).
                settle(Duration::from_millis(options.settle_ms)).await;
                found = self
                    .client
                    .find_by_idempotency_key(project, &marked.key)
                    .await?;
            }
            return match found {
                Some(found) => Ok(Reply::issue(found.id.as_str(), found.key.as_str(), FOUND)),
                None => Err(outcome_unknown(op)),
            };
        }
        Ok(
            match self
                .client
                .create_issue_idempotent(&issue, &marked.key)
                .await?
            {
                CreateOutcome::Created(created) => {
                    Reply::issue(created.id.as_str(), created.key.as_str(), CREATED)
                }
                CreateOutcome::Found(found) => {
                    Reply::issue(found.id.as_str(), found.key.as_str(), FOUND)
                }
            },
        )
    }

    /// `getIssue`. The body is ignored.
    async fn get(&self, read: In<'_>) -> Result<Reply, Failure> {
        let issue = headers::issue(read.message, read.own, &self.options)?;
        let fields = self.options.fields.clone().unwrap_or_default();
        let issue = self.client.get_issue_with(issue, &fields).await?;
        Ok(Reply {
            key: Some(issue.key.as_str().to_owned()),
            id: Some(issue.id.as_str().to_owned()),
            body: serde_json::to_value(&issue).map_err(unserializable)?,
            ..Reply::default()
        })
    }

    /// `editIssue`.
    async fn edit(&self, read: In<'_>, raw: input::Raw) -> Result<Reply, Failure> {
        let edit = input::edit(raw)?;
        let issue = headers::issue(read.message, read.own, &self.options)?;
        let edit_options = self
            .options
            .notify_users
            .map_or_else(EditOptions::default, EditOptions::notify_users);
        self.client
            .edit_issue(issue.clone(), &edit, edit_options)
            .await?;
        let body = json!({"issue": issue.to_string()});
        Ok(Reply::for_issue(&issue, body, None))
    }

    /// `transitionToStatus`.
    ///
    /// On the first try, the client reads the status and sends the
    /// transition. A harvest attempt above 1 or a camel redelivery comes
    /// after a try that can have sent it. A second send can run the
    /// transition two times, for example after an automation moved the
    /// issue on. So such a try only reads the status. At the target, it
    /// gives `already_there`. Else it fails with `jira outcome_unknown`,
    /// which is not retryable (R2A-write-safety-4).
    async fn transition(&self, read: In<'_>, raw: input::Raw) -> Result<Reply, Failure> {
        let fields = input::transition(raw)?;
        let issue = headers::issue(read.message, read.own, &self.options)?;
        let target = headers::status(read.message, &self.options)?;
        let outcome = if headers::retried(read.message)? {
            // The route cannot see why the earlier try failed.
            self.client
                .resume_transition_to_status(issue.clone(), &target, UnknownCause::Timeout)
                .await?
        } else {
            self.client
                .transition_to_status_with(issue.clone(), &target, &fields)
                .await?
        };
        let (kind, status) = match outcome {
            TransitionOutcome::AlreadyThere(status) => (ALREADY_THERE, status),
            TransitionOutcome::Moved { to, .. } => (MOVED, to),
        };
        let body = json!({
            "issue": issue.to_string(),
            "outcome": kind,
            "status": serde_json::to_value(&status).map_err(unserializable)?,
        });
        Ok(Reply::for_issue(&issue, body, Some(kind)))
    }

    /// `addComment`: a plain comment without a key, else a marked comment.
    ///
    /// On harvest attempt 1 with no camel redelivery, no earlier call used
    /// the key. So the client sends with no lookup first
    /// (`add_comment_idempotent_first`), and a busy issue does not stop
    /// the comment at the page cap of the lookup (R2A-write-safety-3).
    /// With no attempt header, or with `resendAfterNegativeLookup=true` on
    /// a retry, the client looks up the marker before it sends.
    async fn comment(&self, read: In<'_>, raw: input::Raw) -> Result<Reply, Failure> {
        let text: Adf = input::comment(raw)?;
        let issue = headers::issue(read.message, read.own, &self.options)?;
        let Some(marked) = headers::marked(read.message, &self.options)? else {
            let comment = self.client.add_comment(issue.clone(), &text).await?;
            return Ok(Reply::comment(&issue, comment.id.as_str(), CREATED));
        };
        if !self.may_send(&marked) {
            return match self
                .client
                .find_comment_by_idempotency_key(issue.clone(), &marked.key)
                .await?
            {
                Some(found) => Ok(Reply::comment(&issue, found.id.as_str(), FOUND)),
                None => Err(outcome_unknown(Operation::AddComment)),
            };
        }
        let outcome = if marked.harvest_first {
            self.client
                .add_comment_idempotent_first(issue.clone(), &text, &marked.key)
                .await?
        } else {
            self.client
                .add_comment_idempotent(issue.clone(), &text, &marked.key)
                .await?
        };
        Ok(match outcome {
            CommentOutcome::Created(comment) => {
                Reply::comment(&issue, comment.id.as_str(), CREATED)
            }
            CommentOutcome::Found(comment) => Reply::comment(&issue, comment.id.as_str(), FOUND),
        })
    }

    /// `search`: at most `maxPages` pages. When more pages exist, the reply
    /// has the token of the next page. The body is ignored.
    async fn search(&self, message: &Message) -> Result<Reply, Failure> {
        let options = &self.options;
        let jql = headers::search_jql(message, options)?;
        let mut request = SearchRequest::new(jql).max_results(options.max_results);
        if let Some(fields) = &options.fields {
            request = request.fields(fields.clone());
        }
        let mut issues = Vec::new();
        let mut token: Option<PageToken> = None;
        let mut seen: Vec<PageToken> = Vec::new();
        let mut pages = 0_u32;
        let next = loop {
            let page: IssuePage = self.client.search_page_at(&request, token.as_ref()).await?;
            pages += 1;
            issues.extend(page.issues);
            let next = page.next_token;
            let repeated = next.as_ref().is_some_and(|t| seen.contains(t));
            match policy::token_page_step(
                pages,
                options.max_pages,
                next.is_none(),
                next.is_some(),
                repeated,
            ) {
                PageStep::Fetch => {
                    seen.extend(next.clone());
                    token = next;
                }
                PageStep::Done => break None,
                // `maxPages` is the limit of one exchange, not an error.
                // The token lets the next exchange go on.
                PageStep::Limit => break next,
                PageStep::Stalled => {
                    return Err(Failure::Jira(JiraError::PaginationStalled {
                        op: Op::Search,
                        pages,
                    }));
                }
            }
        };
        let next = next.map(|t| t.as_str().to_owned());
        let issues = serde_json::to_value(&issues).map_err(unserializable)?;
        Ok(Reply {
            body: json!({"issues": issues, "nextPageToken": next}),
            next_token: next,
            ..Reply::default()
        })
    }

    /// `true` when policy lets the host send a marked write now.
    fn may_send(&self, marked: &Marked) -> bool {
        host_may_send(marked.previous, self.options.resend_after_negative_lookup)
    }
}

/// The failure of a negative lookup on a retry attempt. It is not
/// retryable: a resend can make a duplicate (ADR 0003).
fn outcome_unknown(op: Operation) -> Failure {
    Failure::Camel(CamelError::ValidationError(format!(
        "jira outcome_unknown: an earlier attempt of {op} can have run, and the marker \
         lookup found nothing; set resendAfterNegativeLookup=true to send it again (ADR 0003)"
    )))
}

/// A reply value that does not serialize. The models of the client always
/// serialize, so this is a guard only.
fn unserializable(_: serde_json::Error) -> Failure {
    Failure::Camel(CamelError::TypeConversionFailed(
        "jira: the reply does not serialize to JSON".to_owned(),
    ))
}

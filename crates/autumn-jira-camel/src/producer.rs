//! The `jira:` producer: one Jira call for each exchange.
//!
//! The reply goes into `input` as `Body::Json`, and `output` stays `None`
//! (the san-mateo-camel convention). The out headers carry the issue key,
//! the issue id, the outcome and the next page token.

use std::sync::Arc;

use autumn_jira::client::{CommentOutcome, CreateOutcome, IssuePage, TransitionOutcome};
use autumn_jira::ids::{IssueRef, PageToken};
use autumn_jira::policy::{self, PageStep, Previous, host_may_send};
use autumn_jira::request::{EditOptions, NewIssue, SearchRequest};
use autumn_jira::{Adf, IdempotencyKey, JiraClient, JiraError, Jql};
use camel_api::{Body, CamelError, Exchange, Message, Value};
use serde_json::json;

use crate::error::to_camel_error;
use crate::input;
use crate::options::{EndpointOptions, Operation};
use crate::{HEADER_ISSUE_ID, HEADER_ISSUE_KEY, HEADER_NEXT_TOKEN, HEADER_OUTCOME};

/// The harvest-camel header with the activity idempotency key. It is the
/// same for each attempt of one activity.
pub(crate) const HEADER_HARVEST_KEY: &str = "HarvestIdempotencyKey";
/// The harvest-camel header with the 1-based activity attempt.
pub(crate) const HEADER_HARVEST_ATTEMPT: &str = "HarvestActivityAttempt";

/// The outcome of a write that made the issue or the comment.
const CREATED: &str = "created";
/// The outcome of a write that a marker lookup found.
const FOUND: &str = "found";

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
    /// A reply with the key or the id of an issue ref.
    fn for_issue(issue: &IssueRef, body: Value) -> Self {
        let mut reply = Self {
            body,
            ..Self::default()
        };
        match issue {
            IssueRef::Key(key) => reply.key = Some(key.as_str().to_owned()),
            IssueRef::Id(id) => reply.id = Some(id.as_str().to_owned()),
        }
        reply
    }
}

impl Producer {
    /// Makes the producer of one endpoint.
    pub(crate) const fn new(client: JiraClient, options: Arc<EndpointOptions>) -> Self {
        Self { client, options }
    }

    /// Sends one exchange and puts the reply into it.
    pub(crate) async fn send(&self, mut exchange: Exchange) -> Result<Exchange, CamelError> {
        let options = &self.options;
        let op = options.operation;
        let body = std::mem::take(&mut exchange.input.body);
        let result = self.call(&exchange.input, body).await;
        let message = &mut exchange.input;
        for name in [
            HEADER_ISSUE_KEY,
            HEADER_ISSUE_ID,
            HEADER_OUTCOME,
            HEADER_NEXT_TOKEN,
        ] {
            message.headers.remove(name);
        }
        let reply = match result {
            Ok(reply) => reply,
            Err(Failure::Jira(error)) if !options.throw_exception_on_failure => {
                message.body = Body::Json(json!({"error": {
                    "code": error.code(),
                    "retryable": error.is_retryable(),
                    "message": error.to_string(),
                }}));
                return Ok(exchange);
            }
            Err(Failure::Jira(error)) => return Err(to_camel_error(&error, op.name())),
            Err(Failure::Camel(error)) => return Err(error),
        };
        apply(message, reply);
        Ok(exchange)
    }

    /// Reads the request and makes the Jira call.
    async fn call(&self, message: &Message, body: Body) -> Result<Reply, Failure> {
        let options = &self.options;
        let op = options.operation;
        let max = options.max_body_size;
        match op {
            Operation::CreateIssue => {
                let body = input::create(input::read(body, op, max).await?)?;
                let marked = marked(message, options)?;
                self.create(body, marked).await
            }
            Operation::GetIssue => self.get(&self.issue(message)?).await,
            Operation::EditIssue => {
                let edit = input::edit(input::read(body, op, max).await?)?;
                let issue = self.issue(message)?;
                let edit_options = options
                    .notify_users
                    .map_or_else(EditOptions::default, EditOptions::notify_users);
                self.client
                    .edit_issue(issue.clone(), &edit, edit_options)
                    .await?;
                Ok(Reply::for_issue(
                    &issue,
                    json!({"issue": issue.to_string()}),
                ))
            }
            Operation::TransitionToStatus => {
                let fields = input::transition(input::read(body, op, max).await?)?;
                let issue = self.issue(message)?;
                let target = options
                    .status
                    .clone()
                    .ok_or_else(|| missing(op, "the status option"))?;
                let outcome = self
                    .client
                    .transition_to_status_with(issue.clone(), &target, &fields)
                    .await?;
                let (kind, status) = match outcome {
                    TransitionOutcome::AlreadyThere(status) => ("already_there", status),
                    TransitionOutcome::Moved { to, .. } => ("moved", to),
                };
                let body = json!({
                    "issue": issue.to_string(),
                    "outcome": kind,
                    "status": serde_json::to_value(&status).map_err(unserializable)?,
                });
                let mut reply = Reply::for_issue(&issue, body);
                reply.outcome = Some(kind);
                Ok(reply)
            }
            Operation::AddComment => {
                let text = input::comment(input::read(body, op, max).await?)?;
                let issue = self.issue(message)?;
                match marked(message, options)? {
                    None => {
                        let comment = self.client.add_comment(issue.clone(), &text).await?;
                        Ok(comment_reply(&issue, comment.id.as_str(), CREATED))
                    }
                    Some(marked) => self.comment_marked(&issue, &text, &marked).await,
                }
            }
            Operation::Search => self.search().await,
        }
    }

    /// The issue of the call.
    fn issue(&self, _message: &Message) -> Result<IssueRef, Failure> {
        self.options
            .issue_key
            .clone()
            .ok_or_else(|| missing(self.options.operation, "the issueKey option"))
    }

    /// `createIssue`.
    async fn create(
        &self,
        body: input::CreateBody,
        marked: Option<Marked>,
    ) -> Result<Reply, Failure> {
        let options = &self.options;
        let op = options.operation;
        let project = options
            .project
            .clone()
            .ok_or_else(|| missing(op, "the project option"))?;
        let issue_type = options
            .issue_type
            .clone()
            .ok_or_else(|| missing(op, "the issueType option"))?;
        let mut issue = NewIssue::new(project, issue_type, body.summary).labels(body.labels);
        if let Some(description) = body.description {
            issue = issue.description(description);
        }
        for (id, value) in body.fields {
            issue = issue.field(id, value);
        }
        let Some(marked) = marked else {
            let created = self.client.create_issue(&issue).await?;
            return Ok(issue_reply(
                created.id.as_str(),
                created.key.as_str(),
                CREATED,
            ));
        };
        if !host_may_send(marked.previous, options.resend_after_negative_lookup) {
            let project = issue.project_key();
            return match self
                .client
                .find_by_idempotency_key(project, &marked.key)
                .await?
            {
                Some(found) => Ok(issue_reply(found.id.as_str(), found.key.as_str(), FOUND)),
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
                    issue_reply(created.id.as_str(), created.key.as_str(), CREATED)
                }
                CreateOutcome::Found(found) => {
                    issue_reply(found.id.as_str(), found.key.as_str(), FOUND)
                }
            },
        )
    }

    /// `addComment` with an idempotency key.
    async fn comment_marked(
        &self,
        issue: &IssueRef,
        text: &Adf,
        marked: &Marked,
    ) -> Result<Reply, Failure> {
        let client = &self.client;
        if !host_may_send(marked.previous, self.options.resend_after_negative_lookup) {
            return match client
                .find_comment_by_idempotency_key(issue.clone(), &marked.key)
                .await?
            {
                Some(found) => Ok(comment_reply(issue, found.id.as_str(), FOUND)),
                None => Err(outcome_unknown(Operation::AddComment)),
            };
        }
        Ok(
            match client
                .add_comment_idempotent(issue.clone(), text, &marked.key)
                .await?
            {
                CommentOutcome::Created(comment) => {
                    comment_reply(issue, comment.id.as_str(), CREATED)
                }
                CommentOutcome::Found(comment) => comment_reply(issue, comment.id.as_str(), FOUND),
            },
        )
    }

    /// `getIssue`.
    async fn get(&self, issue: &IssueRef) -> Result<Reply, Failure> {
        let fields = self.options.fields.clone().unwrap_or_default();
        let issue = self.client.get_issue_with(issue.clone(), &fields).await?;
        Ok(Reply {
            key: Some(issue.key.as_str().to_owned()),
            id: Some(issue.id.as_str().to_owned()),
            body: serde_json::to_value(&issue).map_err(unserializable)?,
            ..Reply::default()
        })
    }

    /// `search`: at most `maxPages` pages. When more pages exist, the reply
    /// has the token of the next page.
    async fn search(&self) -> Result<Reply, Failure> {
        let options = &self.options;
        let jql = match (&options.jql, &options.project) {
            (Some(jql), _) => Jql::raw(jql.as_str()),
            (None, Some(project)) => Jql::project(project),
            (None, None) => return Err(missing(options.operation, "the jql option")),
        };
        let mut request = SearchRequest::new(jql).max_results(options.max_results);
        if let Some(fields) = &options.fields {
            request = request.fields(fields.clone());
        }
        let mut issues = Vec::new();
        let mut token: Option<PageToken> = None;
        let mut seen: Vec<PageToken> = Vec::new();
        let mut pages = 0_u32;
        loop {
            let page: IssuePage = self.client.search_page_at(&request, token.as_ref()).await?;
            let next_token = page.next_token;
            pages += 1;
            issues.extend(page.issues);
            let repeated = next_token.as_ref().is_some_and(|t| seen.contains(t));
            match policy::token_page_step(
                pages,
                options.max_pages,
                next_token.is_none(),
                next_token.is_some(),
                repeated,
            ) {
                PageStep::Fetch => {
                    if let Some(next) = next_token {
                        seen.push(next.clone());
                        token = Some(next);
                    }
                }
                PageStep::Done => {
                    token = None;
                    break;
                }
                // `maxPages` is the limit of one exchange, not an error.
                // The token lets the next exchange go on.
                PageStep::Limit => {
                    token = next_token;
                    break;
                }
                PageStep::Stalled => {
                    return Err(Failure::Jira(JiraError::PaginationStalled {
                        op: autumn_jira::error::Op::Search,
                        pages,
                    }));
                }
            }
        }
        let next = token.map(|t| t.as_str().to_owned());
        Ok(Reply {
            body: json!({"issues": serde_json::to_value(&issues).map_err(unserializable)?, "nextPageToken": next}),
            next_token: next,
            ..Reply::default()
        })
    }
}

/// The key and the previous attempt of a marked write.
#[derive(Debug)]
struct Marked {
    key: IdempotencyKey,
    previous: Previous,
}

/// Returns a header value. The match is exact first and then with no
/// regard to case. A null value counts as no value.
fn header<'a>(message: &'a Message, name: &str) -> Option<&'a Value> {
    message
        .header(name)
        .or_else(|| message.header_ic(name))
        .filter(|v| !v.is_null())
}

/// A `ValidationError` for a bad header.
fn bad_header(name: &str, rule: &str) -> CamelError {
    CamelError::ValidationError(format!("the {name} header {rule}"))
}

/// Reads the harvest-camel headers of a marked write (plan D7).
///
/// `HarvestIdempotencyKey` is the only source of the key (W12). An attempt
/// above 1 gives `Previous::Unknown`: the camel route cannot see the
/// failure of the previous attempt, so it must assume that the write can
/// have run. No key header gives `None`: the write is sent at most once.
fn marked(message: &Message, options: &EndpointOptions) -> Result<Option<Marked>, CamelError> {
    let op = options.operation;
    let key = match header(message, HEADER_HARVEST_KEY) {
        None if options.require_idempotency_key => {
            return Err(CamelError::ValidationError(format!(
                "{op} needs the {HEADER_HARVEST_KEY} header: the route sets \
                 requireIdempotencyKey=true"
            )));
        }
        None => return Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => {
            IdempotencyKey::parse(text).unwrap_or_else(|_| IdempotencyKey::derive(&[text.as_str()]))
        }
        Some(_) => return Err(bad_header(HEADER_HARVEST_KEY, "must be a non-empty string")),
    };
    let attempt = match header(message, HEADER_HARVEST_ATTEMPT) {
        None => Some(1),
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(text)) => text.trim().parse::<u64>().ok(),
        Some(_) => None,
    }
    .filter(|n| *n >= 1)
    .ok_or_else(|| bad_header(HEADER_HARVEST_ATTEMPT, "must be a whole number from 1"))?;
    let previous = if attempt > 1 {
        Previous::Unknown
    } else {
        Previous::First
    };
    Ok(Some(Marked { key, previous }))
}

/// The failure of a negative lookup on a retry attempt. It is not
/// retryable: a resend can make a duplicate (ADR 0003).
fn outcome_unknown(op: Operation) -> Failure {
    Failure::Camel(CamelError::ValidationError(format!(
        "jira outcome_unknown: an earlier attempt of {op} can have run, and the marker \
         lookup found nothing; set resendAfterNegativeLookup=true to send it again (ADR 0003)"
    )))
}

/// A failure of one exchange: a Jira error, or a camel error before the
/// call.
#[derive(Debug)]
enum Failure {
    /// The Jira call failed. `throwExceptionOnFailure` applies.
    Jira(JiraError),
    /// The request is bad. The exchange always fails.
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

/// A required value is missing.
fn missing(op: Operation, what: &str) -> Failure {
    Failure::Camel(CamelError::ValidationError(format!("{op} needs {what}")))
}

/// A reply value that does not serialize. The models of the client always
/// serialize, so this is a guard only.
fn unserializable(_: serde_json::Error) -> Failure {
    Failure::Camel(CamelError::TypeConversionFailed(
        "jira: the reply does not serialize to JSON".to_owned(),
    ))
}

/// The reply of a create.
fn issue_reply(id: &str, key: &str, outcome: &'static str) -> Reply {
    Reply {
        body: json!({"id": id, "key": key, "outcome": outcome}),
        key: Some(key.to_owned()),
        id: Some(id.to_owned()),
        outcome: Some(outcome),
        next_token: None,
    }
}

/// The reply of a comment.
fn comment_reply(issue: &IssueRef, id: &str, outcome: &'static str) -> Reply {
    let body = json!({"issue": issue.to_string(), "id": id, "outcome": outcome});
    let mut reply = Reply::for_issue(issue, body);
    reply.outcome = Some(outcome);
    reply
}

/// Puts a reply into the message.
fn apply(message: &mut Message, reply: Reply) {
    message.body = Body::Json(reply.body);
    let headers = [
        (HEADER_ISSUE_KEY, reply.key),
        (HEADER_ISSUE_ID, reply.id),
        (HEADER_OUTCOME, reply.outcome.map(str::to_owned)),
        (HEADER_NEXT_TOKEN, reply.next_token),
    ];
    for (name, value) in headers {
        if let Some(value) = value {
            message.set_header(name, value);
        }
    }
}

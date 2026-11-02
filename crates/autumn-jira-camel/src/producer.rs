//! The `jira:` producer: one Jira call for each exchange.
//!
//! The reply goes into `input` as `Body::Json`, and `output` stays `None`
//! (the san-mateo-camel convention). The out headers carry the issue key,
//! the issue id, the outcome and the next page token.

use std::sync::Arc;

use autumn_jira::client::{IssuePage, TransitionOutcome};
use autumn_jira::ids::{IssueRef, PageToken};
use autumn_jira::policy::{self, PageStep};
use autumn_jira::request::{EditOptions, NewIssue, SearchRequest};
use autumn_jira::{JiraClient, JiraError, Jql};
use camel_api::{Body, CamelError, Exchange, Message, Value};
use serde_json::json;

use crate::error::to_camel_error;
use crate::input;
use crate::options::{EndpointOptions, Operation};
use crate::{HEADER_ISSUE_ID, HEADER_ISSUE_KEY, HEADER_NEXT_TOKEN, HEADER_OUTCOME};

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
                self.create(body).await
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
                let comment = self.client.add_comment(issue.clone(), &text).await?;
                Ok(comment_reply(&issue, comment.id.as_str(), "created"))
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
    async fn create(&self, body: input::CreateBody) -> Result<Reply, Failure> {
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
        let created = self.client.create_issue(&issue).await?;
        Ok(issue_reply(
            created.id.as_str(),
            created.key.as_str(),
            "created",
        ))
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

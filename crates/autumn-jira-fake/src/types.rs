//! Public data types of the fake: ops, faults, projects, requests,
//! deliveries and errors.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// One Jira operation. Faults, rate limits and counts use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum FakeOp {
    /// `GET /rest/api/3/myself`.
    Myself,
    /// `GET /rest/api/3/serverInfo`.
    ServerInfo,
    /// `GET /rest/api/3/project/{key}`.
    GetProject,
    /// `GET /rest/api/3/project/search`.
    ProjectSearch,
    /// `GET /rest/api/3/issue/createmeta/{key}/issuetypes`.
    CreateMeta,
    /// `POST /rest/api/3/issue`.
    CreateIssue,
    /// `GET /rest/api/3/issue/{key}`.
    GetIssue,
    /// `PUT /rest/api/3/issue/{key}`.
    EditIssue,
    /// `DELETE /rest/api/3/issue/{key}`.
    DeleteIssue,
    /// `GET /rest/api/3/issue/{key}/properties/{property}`.
    GetProperty,
    /// `PUT /rest/api/3/issue/{key}/properties/{property}`.
    SetProperty,
    /// `GET /rest/api/3/issue/{key}/transitions`.
    GetTransitions,
    /// `POST /rest/api/3/issue/{key}/transitions`.
    DoTransition,
    /// `GET /rest/api/3/issue/{key}/comment`.
    GetComments,
    /// `POST /rest/api/3/issue/{key}/comment`.
    AddComment,
    /// `GET` or `POST /rest/api/3/search/jql`.
    Search,
    /// `POST /rest/api/3/search/approximate-count`.
    Count,
    /// `GET https://api.atlassian.com/oauth/token/accessible-resources`.
    AccessibleResources,
    /// `POST https://auth.atlassian.com/oauth/token`.
    Token,
}

/// A fault for the next call of one op. Faults of one op are FIFO.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// The fake replies with this status. No write happens.
    Status(u16),
    /// The write happens. The fake replies with this status.
    StatusAfterCommit(u16),
    /// `TransportError::Connect`. No write happens.
    Network,
    /// The write happens. Then `TransportError::Connect`.
    NetworkAfterCommit,
    /// `TransportError::Timeout`. No write happens.
    Timeout,
    /// `TransportError::Timeout`. The write happens after the delay, in
    /// tokio time.
    TimeoutThenCommit(Duration),
    /// A 302 reply with `Location: https://evil.example/`. No write happens.
    Redirect,
    /// The search reply repeats the last `nextPageToken` and says that more
    /// pages come. Other ops ignore it.
    RepeatToken,
    /// The normal 2xx status (200 or 201) with a body that is not JSON. A
    /// write happens.
    BadJson,
}

/// One issue type of a [`FakeProject`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FakeIssueType {
    pub(crate) id: String,
    pub(crate) name: String,
}

/// A project of the fake site. All projects use the default workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeProject {
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) issue_types: Vec<FakeIssueType>,
    pub(crate) lead: String,
}

impl FakeProject {
    /// Makes a project with no issue types. The lead is ann.
    #[must_use]
    pub fn new(key: &str, id: &str, name: &str) -> Self {
        Self {
            key: key.to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            issue_types: Vec::new(),
            lead: crate::ANN.to_owned(),
        }
    }

    /// Adds an issue type.
    #[must_use]
    pub fn issue_type(mut self, id: &str, name: &str) -> Self {
        self.issue_types.push(FakeIssueType {
            id: id.to_owned(),
            name: name.to_owned(),
        });
        self
    }

    /// Sets the account id of the project lead.
    #[must_use]
    pub fn lead(mut self, account_id: &str) -> Self {
        account_id.clone_into(&mut self.lead);
        self
    }

    /// Returns the project key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// One request that the fake answered. It holds no headers, so no auth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeRequest {
    /// The upper-case method.
    pub method: String,
    /// The host, for example `acme.atlassian.net`.
    pub host: String,
    /// The path, for example `/rest/api/3/myself`.
    pub path: String,
    /// The raw query with no `?`. Empty when there is none.
    pub query: String,
}

/// A signed webhook delivery that the fake queued.
#[derive(Clone)]
pub struct FakeDelivery {
    /// The headers, for example `X-Hub-Signature`.
    pub headers: Vec<(String, String)>,
    /// The JSON body bytes.
    pub body: Vec<u8>,
    pub(crate) secret: Arc<[u8]>,
}

impl FakeDelivery {
    /// Returns the delivery id (`X-Atlassian-Webhook-Identifier`).
    #[must_use]
    pub fn id(&self) -> &str {
        self.header(crate::webhook::IDENTIFIER).unwrap_or_default()
    }

    /// Returns the first value of a header. The name match ignores case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Returns a copy with a changed body and the old signature. The body
    /// is still valid JSON: the `timestamp` goes up by one.
    #[must_use]
    pub fn tampered(&self) -> Self {
        let mut copy = self.clone();
        match serde_json::from_slice::<serde_json::Value>(&self.body) {
            Ok(mut body) if body["timestamp"].is_i64() => {
                let ts = body["timestamp"].as_i64().unwrap_or_default();
                body["timestamp"] = serde_json::Value::from(ts.saturating_add(1));
                copy.body = body.to_string().into_bytes();
            }
            _ => copy.body.push(b' '),
        }
        copy
    }

    /// Returns a copy from another actor, with a new id and a new
    /// signature. It sets `user` and, for a comment event, the comment
    /// author.
    #[must_use]
    pub fn with_actor(&self, account_id: &str) -> Self {
        let mut body: serde_json::Value =
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null);
        let user = crate::webhook::short_user(account_id);
        if body.get("user").is_some() {
            body["user"] = user.clone();
        }
        if body.get("comment").is_some() {
            body["comment"]["author"] = user.clone();
            body["comment"]["updateAuthor"] = user;
        }
        let id = format!("{}-as-{account_id}", self.id());
        crate::webhook::build(&self.secret, &id, body.to_string().into_bytes())
    }
}

impl fmt::Debug for FakeDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeDelivery")
            .field("id", &self.id())
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// An error of a person action on the fake.
#[derive(Debug, thiserror::Error)]
pub enum FakeError {
    /// The issue does not exist.
    #[error("no issue {0}")]
    NoIssue(String),
    /// The workflow has no transition from the current status to this one.
    #[error("no transition to {0}")]
    NoTransition(String),
}

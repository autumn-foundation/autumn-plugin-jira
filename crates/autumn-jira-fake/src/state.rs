//! The state of one fake site.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use san_mateo::transport::{HttpRequest, HttpTransport};
use serde_json::Value;
use tokio::time::Instant;

use crate::types::{FakeDelivery, FakeIssueType, FakeOp, FakeProject, FakeRequest, Fault};
use crate::workflow::{self, Status};
use crate::{ANN, BOT, DEFAULT_API_TOKEN, DEFAULT_CLOUD_ID, DEFAULT_EMAIL, DEFAULT_SITE};

/// The first issue id. Jira Cloud ids start near this value.
const FIRST_ISSUE_ID: u64 = 10_000;
/// The first comment id.
const FIRST_COMMENT_ID: u64 = 10_000;
/// The first changelog id.
const FIRST_CHANGE_ID: u64 = 10_100;

/// A wall clock that moves with tokio time, so a paused test clock also
/// stops the `created` and `updated` times.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Clock {
    start: Instant,
    wall: DateTime<Utc>,
}

impl Clock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            wall: Utc::now(),
        }
    }

    /// Returns the wall time now.
    pub(crate) fn now(&self) -> DateTime<Utc> {
        let elapsed = Instant::now().saturating_duration_since(self.start);
        self.wall + TimeDelta::from_std(elapsed).unwrap_or_else(|_| TimeDelta::zero())
    }
}

/// Formats a time as Jira does: `2026-11-02T10:00:00.000+0000`.
pub(crate) fn jira_time(time: &DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.3f%z").to_string()
}

/// One comment.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Comment {
    pub(crate) id: u64,
    pub(crate) author: String,
    pub(crate) body: Value,
    pub(crate) created: DateTime<Utc>,
    pub(crate) updated: DateTime<Utc>,
}

/// One issue.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Issue {
    pub(crate) id: u64,
    pub(crate) key: String,
    pub(crate) number: u64,
    pub(crate) project: String,
    /// The issue type id.
    pub(crate) type_id: String,
    pub(crate) summary: String,
    pub(crate) description: Option<Value>,
    pub(crate) status: &'static str,
    pub(crate) resolution: Option<&'static str>,
    pub(crate) resolution_date: Option<DateTime<Utc>>,
    pub(crate) labels: BTreeSet<String>,
    pub(crate) assignee: Option<String>,
    pub(crate) reporter: String,
    pub(crate) priority: &'static str,
    pub(crate) custom: BTreeMap<String, Value>,
    pub(crate) properties: BTreeMap<String, Value>,
    pub(crate) comments: Vec<Comment>,
    pub(crate) created: DateTime<Utc>,
    pub(crate) updated: DateTime<Utc>,
    pub(crate) status_changed: DateTime<Utc>,
}

impl Issue {
    /// Returns the workflow status.
    pub(crate) fn status(&self) -> Status {
        workflow::status(self.status)
    }
}

/// OAuth settings and the live tokens.
#[derive(Debug, Clone)]
pub(crate) struct OAuth {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    pub(crate) access: BTreeSet<String>,
    pub(crate) expired: BTreeSet<String>,
    pub(crate) refresh: String,
    pub(crate) issued: u64,
}

/// A planned reply for the next call of one op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Planned {
    Fault(Fault),
    RateLimit(u64),
}

/// One page token: the query text and the offset of the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageToken {
    pub(crate) jql: String,
    pub(crate) offset: usize,
}

/// A write that happens later (`Fault::TimeoutThenCommit`).
#[derive(Debug, Clone)]
pub(crate) struct PendingCommit {
    pub(crate) due: Instant,
    pub(crate) op: FakeOp,
    pub(crate) request: HttpRequest,
}

/// A change that search sees later (index lag).
#[derive(Debug, Clone)]
pub(crate) struct PendingIndex {
    pub(crate) due: Instant,
    pub(crate) id: u64,
    pub(crate) issue: Option<Issue>,
}

/// The state of one fake site.
pub(crate) struct State {
    pub(crate) site: String,
    pub(crate) cloud_id: String,
    pub(crate) basic: (String, String),
    pub(crate) oauth: Option<OAuth>,
    pub(crate) webhook_secret: Option<Arc<[u8]>>,
    pub(crate) fallback: Option<Arc<dyn HttpTransport>>,
    pub(crate) projects: Vec<FakeProject>,
    pub(crate) key_counters: BTreeMap<String, u64>,
    pub(crate) issues: BTreeMap<u64, Issue>,
    pub(crate) index: BTreeMap<u64, Issue>,
    pub(crate) index_pending: Vec<PendingIndex>,
    pub(crate) lag: Duration,
    pub(crate) next_issue_id: u64,
    pub(crate) next_comment_id: u64,
    pub(crate) next_change_id: u64,
    pub(crate) next_delivery: u64,
    pub(crate) faults: BTreeMap<FakeOp, VecDeque<Planned>>,
    pub(crate) labels_off: BTreeSet<String>,
    pub(crate) deliveries: Vec<FakeDelivery>,
    pub(crate) requests: Vec<FakeRequest>,
    pub(crate) counts: BTreeMap<FakeOp, usize>,
    pub(crate) refreshes: usize,
    pub(crate) page_tokens: BTreeMap<String, PageToken>,
    pub(crate) next_token: u64,
    pub(crate) last_token: Option<String>,
    pub(crate) pending_commits: Vec<PendingCommit>,
    pub(crate) clock: Clock,
}

impl State {
    /// The default site of plan section 5.
    pub(crate) fn new() -> Self {
        let ops = FakeProject::new("OPS", "10000", "Operations")
            .issue_type("10001", "Task")
            .issue_type("10002", "Incident")
            .issue_type("10003", "Bug")
            .lead(ANN);
        Self {
            site: DEFAULT_SITE.to_owned(),
            cloud_id: DEFAULT_CLOUD_ID.to_owned(),
            basic: (DEFAULT_EMAIL.to_owned(), DEFAULT_API_TOKEN.to_owned()),
            oauth: None,
            webhook_secret: None,
            fallback: None,
            projects: vec![ops],
            key_counters: BTreeMap::new(),
            issues: BTreeMap::new(),
            index: BTreeMap::new(),
            index_pending: Vec::new(),
            lag: Duration::ZERO,
            next_issue_id: FIRST_ISSUE_ID,
            next_comment_id: FIRST_COMMENT_ID,
            next_change_id: FIRST_CHANGE_ID,
            next_delivery: 1,
            faults: BTreeMap::new(),
            labels_off: BTreeSet::new(),
            deliveries: Vec::new(),
            requests: Vec::new(),
            counts: BTreeMap::new(),
            refreshes: 0,
            page_tokens: BTreeMap::new(),
            next_token: 1,
            last_token: None,
            pending_commits: Vec::new(),
            clock: Clock::new(),
        }
    }

    /// The site host, for example `acme.atlassian.net`.
    pub(crate) fn site_host(&self) -> String {
        format!("{}.atlassian.net", self.site)
    }

    /// The site base URL. Every `self` link uses it.
    pub(crate) fn base(&self) -> String {
        format!("https://{}", self.site_host())
    }

    /// The account id of every API caller.
    pub(crate) const fn caller() -> &'static str {
        BOT
    }

    /// Returns the project with this key or id.
    pub(crate) fn project(&self, key_or_id: &str) -> Option<&FakeProject> {
        self.projects
            .iter()
            .find(|p| p.key == key_or_id || p.id == key_or_id)
    }

    /// Returns the issue type of an issue.
    pub(crate) fn issue_type_of(&self, issue: &Issue) -> FakeIssueType {
        self.project(&issue.project)
            .and_then(|p| p.issue_types.iter().find(|t| t.id == issue.type_id))
            .cloned()
            .unwrap_or_else(|| FakeIssueType {
                id: issue.type_id.clone(),
                name: issue.type_id.clone(),
            })
    }

    /// Returns the id of the issue with this key (any case) or id.
    pub(crate) fn find_issue(&self, key_or_id: &str) -> Option<u64> {
        if let Ok(id) = key_or_id.parse::<u64>() {
            return self.issues.contains_key(&id).then_some(id);
        }
        self.issues
            .values()
            .find(|i| i.key.eq_ignore_ascii_case(key_or_id))
            .map(|i| i.id)
    }

    /// Counts one call of an op.
    pub(crate) fn count(&mut self, op: FakeOp) {
        *self.counts.entry(op).or_default() += 1;
    }

    /// Takes the next planned reply of an op.
    pub(crate) fn take_planned(&mut self, op: FakeOp) -> Option<Planned> {
        self.faults.get_mut(&op).and_then(VecDeque::pop_front)
    }

    /// Makes search see the current version of an issue after the lag.
    pub(crate) fn reindex(&mut self, id: u64) {
        let issue = self.issues.get(&id).cloned();
        if self.lag.is_zero() {
            self.index_pending.retain(|p| p.id != id);
            apply_index(&mut self.index, id, issue);
        } else {
            self.index_pending.push(PendingIndex {
                due: Instant::now() + self.lag,
                id,
                issue,
            });
        }
    }

    /// Applies the index changes that are due.
    pub(crate) fn tick_index(&mut self) {
        let now = Instant::now();
        self.index_pending.sort_by_key(|p| p.due);
        let due = self
            .index_pending
            .iter()
            .take_while(|p| p.due <= now)
            .count();
        for pending in self.index_pending.drain(..due) {
            apply_index(&mut self.index, pending.id, pending.issue);
        }
    }

    /// Takes the delayed writes that are due, in time order.
    pub(crate) fn take_due_commits(&mut self) -> Vec<PendingCommit> {
        let now = Instant::now();
        self.pending_commits.sort_by_key(|p| p.due);
        let due = self
            .pending_commits
            .iter()
            .take_while(|p| p.due <= now)
            .count();
        self.pending_commits.drain(..due).collect()
    }
}

/// Puts one issue version into the search index, or removes it.
fn apply_index(index: &mut BTreeMap<u64, Issue>, id: u64, issue: Option<Issue>) {
    match issue {
        Some(issue) => {
            index.insert(id, issue);
        }
        None => {
            index.remove(&id);
        }
    }
}

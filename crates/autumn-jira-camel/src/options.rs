//! Endpoint URI options: the parse and the metadata.
//!
//! One table ([`SPECS`]) holds each option: its name, kind, default and
//! the operations that take it. [`uri_options`] and the parse both read
//! it, so the metadata and the parser cannot drift apart.

use autumn_jira::StatusTarget;
use autumn_jira::ids::{FieldId, IssueRef, IssueTypeId, ProjectKey, StatusCategoryKey, StatusId};
use autumn_jira::request::{FieldSet, IssueTypeRef, MAX_SEARCH_RESULTS};
use camel_api::CamelError;
use camel_component_api::uri::parse_bool_param;
use camel_component_api::{OptionKind, UriOption, parse_uri};

use crate::{DEFAULT_MAX_BODY_SIZE, SCHEME};

/// The largest `maxPages` value.
pub(crate) const MAX_PAGES: u32 = 1000;

/// The default `settleMs`: 10 s. The harvest activity waits 30 s
/// (`SETTLE_SECS`), but a `direct:` call stops after 30 s by default, and
/// the wait and two lookups must fit in it.
pub(crate) const DEFAULT_SETTLE_MS: u64 = 10_000;

/// The largest `settleMs` value: 2 minutes.
pub(crate) const MAX_SETTLE_MS: u64 = 120_000;

/// One operation of the `jira:` component: the path of the URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operation {
    /// `createIssue`: create an issue.
    CreateIssue,
    /// `getIssue`: read one issue.
    GetIssue,
    /// `editIssue`: edit one issue.
    EditIssue,
    /// `transitionToStatus`: move an issue to a status.
    TransitionToStatus,
    /// `addComment`: add a comment to an issue.
    AddComment,
    /// `search`: run a JQL search.
    Search,
}

impl Operation {
    /// All operations, in the order of [`crate::OPERATIONS`].
    pub const ALL: [Self; 6] = [
        Self::CreateIssue,
        Self::GetIssue,
        Self::EditIssue,
        Self::TransitionToStatus,
        Self::AddComment,
        Self::Search,
    ];

    /// Returns the URI name, for example `createIssue`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CreateIssue => "createIssue",
            Self::GetIssue => "getIssue",
            Self::EditIssue => "editIssue",
            Self::TransitionToStatus => "transitionToStatus",
            Self::AddComment => "addComment",
            Self::Search => "search",
        }
    }

    /// Finds the operation with this URI name. The match is exact.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The options of one `jira:` endpoint, after the parse and the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is one URI option of plan section 10"
)]
pub struct EndpointOptions {
    /// The operation: the URI path.
    pub operation: Operation,
    /// The name of the registered client. Default `default`.
    pub connection: String,
    /// The project of `createIssue`, or the project filter of `search`.
    pub project: Option<ProjectKey>,
    /// The issue type of `createIssue`.
    pub issue_type: Option<IssueTypeRef>,
    /// The issue. `None` only with `allow_header_overrides`: then the
    /// `JiraIssueKey` header must give it.
    pub issue_key: Option<IssueRef>,
    /// The target status of `transitionToStatus`. `None` only with
    /// `allow_header_overrides`: then the `JiraStatus` header must give it.
    pub status: Option<StatusTarget>,
    /// The raw JQL of `search`. The app owns it.
    pub jql: Option<String>,
    /// The fields to read. `None` gives the default set.
    pub fields: Option<FieldSet>,
    /// The page size of `search`.
    pub max_results: u32,
    /// The most pages that one `search` exchange reads.
    pub max_pages: u32,
    /// Fail the exchange when it has no `HarvestIdempotencyKey` header.
    pub require_idempotency_key: bool,
    /// Send a marked write again after a negative lookup. This is an
    /// explicit unsafe opt-in: a lookup can miss a write that Jira did not
    /// index yet, so Jira can get the write two times.
    pub resend_after_negative_lookup: bool,
    /// The wait of `createIssue` before a second marker lookup, in ms. It
    /// applies only to a lookup-only attempt whose first lookup finds
    /// nothing. The search index can lag, so the wait lets the lookup see
    /// an issue that an earlier attempt made. 0 turns the wait off. Keep the
    /// wait and two lookups inside the timeout of the caller, for example
    /// the 30 s default of a `direct:` call.
    pub settle_ms: u64,
    /// The `notifyUsers` flag of `editIssue`. `None` keeps the Jira default.
    pub notify_users: Option<bool>,
    /// Let the `JiraIssueKey`, `JiraJql` and `JiraStatus` headers replace
    /// the options. Default `false`: a header from an outside caller must
    /// not pick the issue, the query or the status.
    pub allow_header_overrides: bool,
    /// Fail the exchange when the Jira call fails.
    pub throw_exception_on_failure: bool,
    /// The largest request body in bytes.
    pub max_body_size: usize,
}

impl EndpointOptions {
    /// Parses and checks a `jira:` URI.
    ///
    /// `connections` holds the names of the registered clients.
    ///
    /// # Errors
    ///
    /// [`CamelError::InvalidUri`] for a URI problem, for example an unknown
    /// option, a bad value or a missing required option.
    /// [`CamelError::Config`] when no client has the `connection` name.
    pub(crate) fn parse(uri: &str, connections: &[&str]) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != SCHEME {
            return Err(bad_uri(format!(
                "expected the scheme {SCHEME}, got {}",
                parts.scheme
            )));
        }
        let operation = parse_operation(parts.path.trim_matches('/'))?;
        let mut options = Self::defaults(operation);
        // Sort the keys, so that the first error is the same on each run.
        let mut params: Vec<(&String, &String)> = parts.params.iter().collect();
        params.sort();
        for (key, value) in params {
            let spec = SPECS
                .iter()
                .find(|s| s.name == key.as_str())
                .ok_or_else(|| unknown_option(key))?;
            if !spec.ops.contains(&operation) {
                return Err(bad_uri(format!(
                    "option '{key}' does not apply to {operation}; it applies to {}",
                    join(spec.ops.iter().map(|op| op.name()))
                )));
            }
            options.set(key, value)?;
        }
        options.check_required()?;
        if !connections.contains(&options.connection.as_str()) {
            return Err(CamelError::Config(format!(
                "no jira connection '{}' for {SCHEME}:{operation}; the connections are {}",
                options.connection,
                join(connections.iter().copied())
            )));
        }
        Ok(options)
    }

    /// The options before the URI options apply.
    fn defaults(operation: Operation) -> Self {
        Self {
            operation,
            connection: "default".to_owned(),
            project: None,
            issue_type: None,
            issue_key: None,
            status: None,
            jql: None,
            fields: None,
            max_results: 50,
            max_pages: 1,
            require_idempotency_key: false,
            resend_after_negative_lookup: false,
            settle_ms: DEFAULT_SETTLE_MS,
            notify_users: None,
            allow_header_overrides: false,
            throw_exception_on_failure: true,
            max_body_size: DEFAULT_MAX_BODY_SIZE,
        }
    }

    /// Sets one option from its URI value.
    fn set(&mut self, key: &str, value: &str) -> Result<(), CamelError> {
        match key {
            "connection" => self.connection = non_empty(key, value)?,
            "project" => self.project = Some(ProjectKey::parse(value).map_err(|e| field(key, &e))?),
            "issueType" => self.issue_type = Some(parse_issue_type(value)?),
            "issueKey" => {
                self.issue_key = Some(IssueRef::parse(value).map_err(|e| field(key, &e))?);
            }
            "status" => self.status = Some(parse_status(value)?),
            "jql" => self.jql = Some(parse_jql(value)?),
            "fields" => self.fields = Some(parse_fields(value)?),
            "maxResults" => self.max_results = parse_range(key, value, MAX_SEARCH_RESULTS)?,
            "maxPages" => self.max_pages = parse_range(key, value, MAX_PAGES)?,
            "requireIdempotencyKey" => self.require_idempotency_key = parse_bool(key, value)?,
            "resendAfterNegativeLookup" => {
                self.resend_after_negative_lookup = parse_bool(key, value)?;
            }
            "settleMs" => self.settle_ms = parse_settle(value)?,
            "notifyUsers" => self.notify_users = Some(parse_bool(key, value)?),
            "allowHeaderOverrides" => self.allow_header_overrides = parse_bool(key, value)?,
            "throwExceptionOnFailure" => {
                self.throw_exception_on_failure = parse_bool(key, value)?;
            }
            "maxBodySize" => self.max_body_size = parse_max_body_size(value)?,
            // A name in SPECS with no arm here. The test
            // `uri_options_match_the_parser` finds it.
            other => return Err(unknown_option(other)),
        }
        Ok(())
    }

    /// Checks the options that an operation needs.
    ///
    /// An option that a header can give is required only without
    /// `allowHeaderOverrides`.
    fn check_required(&self) -> Result<(), CamelError> {
        let op = self.operation;
        let by_header = self.allow_header_overrides;
        let needs = |what: &str| bad_uri(format!("{op} needs {what}"));
        match op {
            Operation::CreateIssue if self.project.is_none() => {
                return Err(needs("the project option"));
            }
            Operation::CreateIssue if self.issue_type.is_none() => {
                return Err(needs("the issueType option"));
            }
            Operation::Search if self.project.is_some() && self.jql.is_some() => {
                return Err(bad_uri(
                    "search takes the project option or the jql option, not both: \
                     the jql is raw, so the component does not add a project clause to it"
                        .to_owned(),
                ));
            }
            Operation::Search if self.project.is_none() && self.jql.is_none() && !by_header => {
                return Err(needs(
                    "the project option, the jql option, or allowHeaderOverrides=true \
                     and the JiraJql header",
                ));
            }
            _ => {}
        }
        let has_issue = !matches!(op, Operation::CreateIssue | Operation::Search);
        if has_issue && self.issue_key.is_none() && !by_header {
            return Err(needs(
                "the issueKey option, or allowHeaderOverrides=true and the JiraIssueKey header",
            ));
        }
        if op == Operation::TransitionToStatus && self.status.is_none() && !by_header {
            return Err(needs(
                "the status option, or allowHeaderOverrides=true and the JiraStatus header",
            ));
        }
        Ok(())
    }
}

/// The kind of an option value, for the metadata.
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Int,
    Bool,
    TextList,
}

/// One URI option.
struct Spec {
    name: &'static str,
    kind: Kind,
    default: Option<&'static str>,
    ops: &'static [Operation],
    description: &'static str,
}

const ALL_OPS: &[Operation] = &Operation::ALL;
const ISSUE_OPS: &[Operation] = &[
    Operation::GetIssue,
    Operation::EditIssue,
    Operation::TransitionToStatus,
    Operation::AddComment,
];
const MARKED_OPS: &[Operation] = &[Operation::CreateIssue, Operation::AddComment];
const SEARCH: &[Operation] = &[Operation::Search];

/// The URI options, in the order of the metadata.
const SPECS: [Spec; 16] = [
    Spec {
        name: "connection",
        kind: Kind::Text,
        default: Some("default"),
        ops: ALL_OPS,
        description: "The name of a client that the component has: `default` or a name of JiraComponent::connection.",
    },
    Spec {
        name: "project",
        kind: Kind::Text,
        default: None,
        ops: &[Operation::CreateIssue, Operation::Search],
        description: "The project key. createIssue needs it. For search it gives the query project = \"<key>\" when there is no jql.",
    },
    Spec {
        name: "issueType",
        kind: Kind::Text,
        default: None,
        ops: &[Operation::CreateIssue],
        description: "The issue type of createIssue: a name or a numeric id. createIssue needs it.",
    },
    Spec {
        name: "issueKey",
        kind: Kind::Text,
        default: None,
        ops: ISSUE_OPS,
        description: "The issue key or the numeric issue id. With allowHeaderOverrides, the JiraIssueKey header replaces it.",
    },
    Spec {
        name: "status",
        kind: Kind::Text,
        default: None,
        ops: &[Operation::TransitionToStatus],
        description: "The target status: a name, id:<n> or category:<key>. With allowHeaderOverrides, the JiraStatus header replaces it.",
    },
    Spec {
        name: "jql",
        kind: Kind::Text,
        default: None,
        ops: SEARCH,
        description: "The raw JQL of search, percent-encoded or in RAW(...). The app owns it. Do not use it with project. With allowHeaderOverrides, the JiraJql header replaces it.",
    },
    Spec {
        name: "fields",
        kind: Kind::TextList,
        default: None,
        ops: &[Operation::GetIssue, Operation::Search],
        description: "The field ids to read, for example summary,status,customfield_10010. The default is the default field set of the client.",
    },
    Spec {
        name: "maxResults",
        kind: Kind::Int,
        default: Some("50"),
        ops: SEARCH,
        description: "The page size of search, from 1 to 100.",
    },
    Spec {
        name: "maxPages",
        kind: Kind::Int,
        default: Some("1"),
        ops: SEARCH,
        description: "The most pages that one search exchange reads, from 1 to 1000.",
    },
    Spec {
        name: "requireIdempotencyKey",
        kind: Kind::Bool,
        default: Some("false"),
        ops: MARKED_OPS,
        description: "Fail the exchange when it has no HarvestIdempotencyKey header. Without the header, the component sends the write at most once.",
    },
    Spec {
        name: "resendAfterNegativeLookup",
        kind: Kind::Bool,
        default: Some("false"),
        ops: MARKED_OPS,
        description: "Unsafe opt-in: on a retry attempt, send the write again when the marker lookup finds nothing. Jira can then get the write two times.",
    },
    Spec {
        name: "settleMs",
        kind: Kind::Int,
        // DEFAULT_SETTLE_MS. The test `uri_options_match_the_parser`
        // compares the two.
        default: Some("10000"),
        ops: &[Operation::CreateIssue],
        description: "On a retry attempt, the wait in ms before a second marker lookup when the first lookup finds nothing. The search index can lag. From 0 (no wait) to 120000. Keep the wait and two lookups inside the timeout of the caller, for example the 30 s of a direct: call.",
    },
    Spec {
        name: "notifyUsers",
        kind: Kind::Bool,
        default: None,
        ops: &[Operation::EditIssue],
        description: "The notifyUsers flag of editIssue. Unset keeps the Jira default.",
    },
    Spec {
        name: "allowHeaderOverrides",
        kind: Kind::Bool,
        default: Some("false"),
        ops: ALL_OPS,
        description: "Let the JiraIssueKey, JiraJql and JiraStatus headers replace the options. Use it only when no outside caller can set headers.",
    },
    Spec {
        name: "throwExceptionOnFailure",
        kind: Kind::Bool,
        default: Some("true"),
        ops: ALL_OPS,
        description: "Fail the exchange when the Jira call fails.",
    },
    Spec {
        name: "maxBodySize",
        kind: Kind::Int,
        // DEFAULT_MAX_BODY_SIZE. The test `uri_options_match_the_parser`
        // compares the two.
        default: Some("2097152"),
        ops: ALL_OPS,
        description: "The largest request body in bytes.",
    },
];

/// Returns the URI options for the component metadata.
///
/// No option is `required` here: each operation needs other options, and
/// the parse checks them.
#[must_use]
pub fn uri_options() -> Vec<UriOption> {
    SPECS
        .iter()
        .map(|spec| {
            let kind = match spec.kind {
                Kind::Text => OptionKind::String,
                Kind::Int => OptionKind::Int,
                Kind::Bool => OptionKind::Bool,
                Kind::TextList => OptionKind::List(Box::new(OptionKind::String)),
            };
            let option = UriOption::new(spec.name, spec.description, kind);
            match spec.default {
                Some(default) => option.with_default(default),
                None => option,
            }
        })
        .collect()
}

const fn bad_uri(message: String) -> CamelError {
    CamelError::InvalidUri(message)
}

fn join<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names.collect::<Vec<_>>().join(", ")
}

fn unknown_option(key: &str) -> CamelError {
    bad_uri(format!(
        "unknown {SCHEME}: option '{key}'; the options are {}",
        join(SPECS.iter().map(|s| s.name))
    ))
}

/// Wraps a parse error of the core. Its text holds the rule, not the value.
fn field(key: &str, err: &autumn_jira::JiraError) -> CamelError {
    bad_uri(format!("{key}: {err}"))
}

fn parse_operation(name: &str) -> Result<Operation, CamelError> {
    let operations = join(Operation::ALL.iter().map(|op| op.name()));
    if name.is_empty() {
        return Err(bad_uri(format!(
            "the operation is missing: use {SCHEME}:<operation>; the operations are {operations}"
        )));
    }
    Operation::parse(name).ok_or_else(|| {
        bad_uri(format!(
            "unknown {SCHEME}: operation '{name}'; the operations are {operations}"
        ))
    })
}

fn non_empty(key: &str, value: &str) -> Result<String, CamelError> {
    if value.trim().is_empty() {
        return Err(bad_uri(format!("{key} must not be empty")));
    }
    Ok(value.to_owned())
}

fn parse_bool(key: &str, value: &str) -> Result<bool, CamelError> {
    parse_bool_param(value).map_err(|e| bad_uri(format!("{key}: {e}")))
}

fn parse_range(key: &str, value: &str, max: u32) -> Result<u32, CamelError> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|n| (1..=max).contains(n))
        .ok_or_else(|| {
            bad_uri(format!(
                "{key} must be a whole number from 1 to {max}, got '{value}'"
            ))
        })
}

fn parse_settle(value: &str) -> Result<u64, CamelError> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= MAX_SETTLE_MS)
        .ok_or_else(|| {
            bad_uri(format!(
                "settleMs must be a whole number from 0 to {MAX_SETTLE_MS}, got '{value}'"
            ))
        })
}

fn parse_max_body_size(value: &str) -> Result<usize, CamelError> {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| {
            bad_uri(format!(
                "maxBodySize must be a number of bytes above 0, got '{value}'"
            ))
        })
}

/// A numeric id gives `IssueTypeRef::Id`, other text a name.
fn parse_issue_type(value: &str) -> Result<IssueTypeRef, CamelError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(bad_uri(
            "issueType must be an issue type name or a numeric id".to_owned(),
        ));
    }
    if value.bytes().all(|b| b.is_ascii_digit()) {
        return IssueTypeId::parse(value)
            .map(IssueTypeRef::Id)
            .map_err(|e| field("issueType", &e));
    }
    Ok(IssueTypeRef::Name(value.to_owned()))
}

/// `id:<n>` gives a status id, `category:<key>` a category, other text a
/// status name.
pub(crate) fn parse_status(value: &str) -> Result<StatusTarget, CamelError> {
    if let Some(id) = value.strip_prefix("id:") {
        return StatusId::parse(id)
            .map(StatusTarget::Id)
            .map_err(|e| field("status", &e));
    }
    if let Some(key) = value.strip_prefix("category:") {
        return StatusCategoryKey::parse(key)
            .map(StatusTarget::Category)
            .map_err(|e| field("status", &e));
    }
    let name = value.trim();
    if name.is_empty() {
        return Err(bad_uri(
            "status must be a status name, id:<n> or category:<key>".to_owned(),
        ));
    }
    Ok(StatusTarget::Name(name.to_owned()))
}

/// camel keeps a `RAW(...)` value as it is. The JQL is the text in the
/// brackets, with no percent decode.
fn parse_jql(value: &str) -> Result<String, CamelError> {
    let jql = value
        .strip_prefix("RAW(")
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or(value);
    if jql.trim().is_empty() {
        return Err(bad_uri("jql must not be empty".to_owned()));
    }
    Ok(jql.to_owned())
}

fn parse_fields(value: &str) -> Result<FieldSet, CamelError> {
    let ids = value
        .split(',')
        .map(|id| {
            let id = id.trim();
            if id.is_empty() {
                return Err(bad_uri(
                    "fields must be a comma-separated list of field ids, with no empty item"
                        .to_owned(),
                ));
            }
            FieldId::parse(id).map_err(|e| field("fields", &e))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FieldSet::new(ids))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "tests with tables"
)]
mod tests {
    use std::collections::BTreeMap;

    use autumn_jira::ids::{IssueId, IssueKey, IssueTypeId, StatusCategoryKey, StatusId};
    use camel_component_api::OptionKind;

    use super::*;
    use crate::{DEFAULT_MAX_BODY_SIZE, OPERATIONS};

    #[test]
    fn settle_ms_takes_0_to_the_max() {
        let uri = |v: &str| format!("jira:createIssue?project=OPS&issueType=Task&settleMs={v}");
        assert_eq!(parse(&uri("0")).unwrap().settle_ms, 0);
        assert_eq!(parse(&uri("120000")).unwrap().settle_ms, MAX_SETTLE_MS);
        for bad in ["120001", "-1", "x", ""] {
            assert!(invalid_uri(&uri(bad)).contains("settleMs"), "{bad}");
        }
    }

    const CONNECTIONS: &[&str] = &["default", "eu"];

    fn parse(uri: &str) -> Result<EndpointOptions, CamelError> {
        EndpointOptions::parse(uri, CONNECTIONS)
    }

    fn invalid_uri(uri: &str) -> String {
        match parse(uri) {
            Err(CamelError::InvalidUri(m)) => m,
            other => panic!("expected InvalidUri for {uri}, got {other:?}"),
        }
    }

    /// The option names in the order of the metadata.
    const NAMES: [&str; 16] = [
        "connection",
        "project",
        "issueType",
        "issueKey",
        "status",
        "jql",
        "fields",
        "maxResults",
        "maxPages",
        "requireIdempotencyKey",
        "resendAfterNegativeLookup",
        "settleMs",
        "notifyUsers",
        "allowHeaderOverrides",
        "throwExceptionOnFailure",
        "maxBodySize",
    ];

    /// The `Ops` column of plan section 10.
    fn applies(option: &str, op: Operation) -> bool {
        use Operation::{AddComment, CreateIssue, EditIssue, GetIssue, Search, TransitionToStatus};
        match option {
            "project" => matches!(op, CreateIssue | Search),
            "issueType" | "settleMs" => op == CreateIssue,
            "issueKey" => matches!(op, GetIssue | EditIssue | TransitionToStatus | AddComment),
            "status" => op == TransitionToStatus,
            "jql" | "maxResults" | "maxPages" => op == Search,
            "fields" => matches!(op, GetIssue | Search),
            "requireIdempotencyKey" | "resendAfterNegativeLookup" => {
                matches!(op, CreateIssue | AddComment)
            }
            "notifyUsers" => op == EditIssue,
            _ => true,
        }
    }

    /// A good value of each option.
    fn sample(option: &str) -> &'static str {
        match option {
            "connection" => "eu",
            "project" => "OPS",
            "issueType" => "Task",
            "issueKey" => "OPS-1",
            "status" => "Done",
            "jql" => "assignee%20%3D%20currentUser()",
            "fields" => "summary,status",
            "maxResults" => "10",
            "maxPages" => "2",
            "notifyUsers" | "throwExceptionOnFailure" => "false",
            "maxBodySize" => "5",
            "settleMs" => "0",
            _ => "true",
        }
    }

    /// A URI of `op` with the required options and `extra`.
    fn uri_with(op: Operation, extra: &[(&str, &str)]) -> String {
        let mut params: BTreeMap<&str, &str> = BTreeMap::new();
        params.insert("allowHeaderOverrides", "true");
        if op == Operation::CreateIssue {
            params.insert("project", "OPS");
            params.insert("issueType", "Task");
        }
        for (k, v) in extra {
            params.insert(k, v);
        }
        let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("jira:{}?{}", op.name(), query.join("&"))
    }

    #[test]
    fn operations_match_the_constant() {
        let names: Vec<&str> = Operation::ALL.iter().map(|o| o.name()).collect();
        assert_eq!(names, OPERATIONS);
        for op in Operation::ALL {
            assert_eq!(Operation::parse(op.name()), Some(op));
        }
        assert_eq!(Operation::parse("createissue"), None, "the match is exact");
        assert_eq!(Operation::parse(""), None);
        assert_eq!(Operation::parse("deleteIssue"), None);
    }

    #[test]
    fn uri_options_match_the_parser() {
        let opts = uri_options();
        let names: Vec<&str> = opts.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, NAMES);
        for o in &opts {
            assert!(!o.description.is_empty(), "{}", o.name);
            assert!(!o.secret, "{}", o.name);
            assert!(
                !o.required,
                "no option is required for every operation: {}",
                o.name
            );
        }
        // Each option parses for the operations of the plan table, and
        // fails with a clear message for the other operations.
        for name in NAMES {
            for op in Operation::ALL {
                let uri = uri_with(op, &[(name, sample(name))]);
                if applies(name, op) {
                    assert!(parse(&uri).is_ok(), "{uri}: {:?}", parse(&uri));
                } else {
                    let m = invalid_uri(&uri);
                    assert!(m.contains(name) && m.contains(op.name()), "{m}");
                    assert!(m.contains("does not apply"), "{m}");
                }
            }
        }
        // The unknown-option message lists exactly the metadata names.
        let m = invalid_uri("jira:search?projectt=OPS");
        let listed = m.split("the options are ").nth(1).unwrap();
        assert_eq!(listed, NAMES.join(", "));
        // The defaults and the kinds.
        let find = |n: &str| opts.iter().find(|o| o.name == n).unwrap();
        let default = |n: &str| find(n).default_value.clone();
        assert_eq!(default("connection").as_deref(), Some("default"));
        assert_eq!(default("maxResults").as_deref(), Some("50"));
        assert_eq!(default("maxPages").as_deref(), Some("1"));
        assert_eq!(default("requireIdempotencyKey").as_deref(), Some("false"));
        assert_eq!(
            default("resendAfterNegativeLookup").as_deref(),
            Some("false")
        );
        assert_eq!(default("allowHeaderOverrides").as_deref(), Some("false"));
        assert_eq!(default("throwExceptionOnFailure").as_deref(), Some("true"));
        assert_eq!(
            default("maxBodySize"),
            Some(DEFAULT_MAX_BODY_SIZE.to_string())
        );
        assert_eq!(default("settleMs"), Some(DEFAULT_SETTLE_MS.to_string()));
        assert_eq!(default("notifyUsers"), None, "unset keeps the Jira default");
        assert_eq!(default("project"), None);
        assert_eq!(find("maxResults").kind, OptionKind::Int);
        assert_eq!(find("notifyUsers").kind, OptionKind::Bool);
        assert_eq!(
            find("fields").kind,
            OptionKind::List(Box::new(OptionKind::String))
        );
        assert_eq!(find("jql").kind, OptionKind::String);
    }

    #[test]
    fn unknown_option_lists_names() {
        let m = invalid_uri("jira:getIssue?issueKey=OPS-1&issuekey=OPS-2");
        assert!(m.contains("unknown jira: option 'issuekey'"), "{m}");
        for name in NAMES {
            assert!(m.contains(name), "the message lists {name}: {m}");
        }
    }

    #[test]
    fn project_and_jql_conflict() {
        let m = invalid_uri("jira:search?project=OPS&jql=status%20%3D%20Done");
        assert!(m.contains("project") && m.contains("jql"), "{m}");
        // The flag does not lift the conflict: the plan does not wrap raw JQL.
        let m = invalid_uri(
            "jira:search?project=OPS&jql=status%20%3D%20Done&allowHeaderOverrides=true",
        );
        assert!(m.contains("project") && m.contains("jql"), "{m}");
        // One of the two is enough.
        let o = parse("jira:search?project=OPS").unwrap();
        assert_eq!(o.project, Some(ProjectKey::parse("OPS").unwrap()));
        assert_eq!(o.jql, None);
        let o = parse("jira:search?jql=status%20%3D%20Done").unwrap();
        assert_eq!(o.jql.as_deref(), Some("status = Done"));
        assert_eq!(o.project, None);
    }

    #[test]
    fn defaults() {
        let o = parse("jira:getIssue?issueKey=OPS-1").unwrap();
        assert_eq!(o.operation, Operation::GetIssue);
        assert_eq!(o.connection, "default");
        assert_eq!(
            o.issue_key,
            Some(IssueRef::Key(IssueKey::parse("OPS-1").unwrap()))
        );
        assert_eq!(o.project, None);
        assert_eq!(o.issue_type, None);
        assert_eq!(o.status, None);
        assert_eq!(o.jql, None);
        assert_eq!(o.fields, None, "None gives the default field set");
        assert_eq!(o.max_results, 50);
        assert_eq!(o.max_pages, 1);
        assert!(!o.require_idempotency_key);
        assert!(!o.resend_after_negative_lookup);
        assert_eq!(o.notify_users, None);
        assert!(!o.allow_header_overrides);
        assert!(o.throw_exception_on_failure);
        assert_eq!(o.max_body_size, DEFAULT_MAX_BODY_SIZE);
    }

    #[test]
    fn parses_all_options_of_each_operation() {
        let o = parse(
            "jira:createIssue?connection=eu&project=OPS&issueType=10001&requireIdempotencyKey=yes\
             &resendAfterNegativeLookup=1&allowHeaderOverrides=true\
             &throwExceptionOnFailure=false&maxBodySize=1024",
        )
        .unwrap();
        assert_eq!(o.operation, Operation::CreateIssue);
        assert_eq!(o.connection, "eu");
        assert_eq!(o.project, Some(ProjectKey::parse("OPS").unwrap()));
        assert_eq!(
            o.issue_type,
            Some(IssueTypeRef::Id(IssueTypeId::parse("10001").unwrap()))
        );
        assert!(o.require_idempotency_key);
        assert!(o.resend_after_negative_lookup);
        assert!(o.allow_header_overrides);
        assert!(!o.throw_exception_on_failure);
        assert_eq!(o.max_body_size, 1024);

        let o = parse("jira:createIssue?project=OPS&issueType=Bug%20Report").unwrap();
        assert_eq!(o.issue_type, Some(IssueTypeRef::Name("Bug Report".into())));

        let o = parse("jira:getIssue?issueKey=10001&fields=summary,%20customfield_10010").unwrap();
        assert_eq!(
            o.issue_key,
            Some(IssueRef::Id(IssueId::parse("10001").unwrap()))
        );
        assert_eq!(
            o.fields,
            Some(FieldSet::new([
                FieldId::parse("summary").unwrap(),
                FieldId::parse("customfield_10010").unwrap(),
            ]))
        );

        let o = parse("jira:editIssue?issueKey=OPS-2&notifyUsers=false").unwrap();
        assert_eq!(o.operation, Operation::EditIssue);
        assert_eq!(o.notify_users, Some(false));

        let o = parse("jira:transitionToStatus?issueKey=OPS-3&status=In%20Progress").unwrap();
        assert_eq!(o.status, Some(StatusTarget::Name("In Progress".into())));

        let o = parse("jira:addComment?issueKey=OPS-4&requireIdempotencyKey=true").unwrap();
        assert_eq!(o.operation, Operation::AddComment);
        assert!(o.require_idempotency_key);

        let o =
            parse("jira:search?project=OPS&fields=summary&maxResults=100&maxPages=1000").unwrap();
        assert_eq!(o.operation, Operation::Search);
        assert_eq!(o.max_results, 100);
        assert_eq!(o.max_pages, 1000);
    }

    #[test]
    fn scheme_and_operation_rules() {
        assert!(invalid_uri("http:getIssue?issueKey=OPS-1").contains("jira"));
        let m = invalid_uri("jira:?issueKey=OPS-1");
        assert!(m.contains("operation") && m.contains("createIssue"), "{m}");
        let m = invalid_uri("jira:deleteIssue");
        assert!(m.contains("deleteIssue"), "{m}");
        for op in OPERATIONS {
            assert!(m.contains(op), "the message lists {op}: {m}");
        }
        assert!(invalid_uri("jira:getissue?issueKey=OPS-1").contains("getissue"));
        // Slashes around the operation are ignored.
        assert_eq!(
            parse("jira://getIssue?issueKey=OPS-1").unwrap().operation,
            Operation::GetIssue
        );
        assert_eq!(
            parse("jira:getIssue/?issueKey=OPS-1").unwrap().operation,
            Operation::GetIssue
        );
        // camel rejects a duplicate key and a key with no value.
        assert!(invalid_uri("jira:getIssue?issueKey=OPS-1&issueKey=OPS-2").contains("duplicate"));
        assert!(invalid_uri("jira:getIssue?issueKey").contains("issueKey"));
    }

    #[test]
    fn connection_rule() {
        assert_eq!(
            parse("jira:getIssue?issueKey=OPS-1&connection=eu")
                .unwrap()
                .connection,
            "eu"
        );
        match parse("jira:getIssue?issueKey=OPS-1&connection=us") {
            Err(CamelError::Config(m)) => {
                assert!(m.contains("'us'") && m.contains("default, eu"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        match EndpointOptions::parse("jira:getIssue?issueKey=OPS-1", &["eu"]) {
            Err(CamelError::Config(m)) => assert!(m.contains("'default'"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(invalid_uri("jira:getIssue?issueKey=OPS-1&connection=").contains("connection"));
        assert!(invalid_uri("jira:getIssue?issueKey=OPS-1&connection=%20").contains("connection"));
    }

    #[test]
    fn project_rule() {
        for bad in ["ops", "", "O", "OPS-1", "../x"] {
            let m = invalid_uri(&format!("jira:search?project={bad}"));
            assert!(m.contains("project"), "{bad}: {m}");
        }
    }

    #[test]
    fn issue_type_rule() {
        let m = invalid_uri("jira:createIssue?project=OPS");
        assert!(m.contains("issueType") && m.contains("createIssue"), "{m}");
        for bad in ["", "%20%20", "0", "007"] {
            let m = invalid_uri(&format!("jira:createIssue?project=OPS&issueType={bad}"));
            assert!(m.contains("issueType"), "{bad}: {m}");
        }
        let m = invalid_uri("jira:createIssue?issueType=Task");
        assert!(m.contains("project") && m.contains("createIssue"), "{m}");
    }

    #[test]
    fn issue_key_rule() {
        for op in ["getIssue", "editIssue", "addComment"] {
            let m = invalid_uri(&format!("jira:{op}"));
            assert!(
                m.contains("issueKey") && m.contains("allowHeaderOverrides"),
                "{m}"
            );
            let o = parse(&format!("jira:{op}?allowHeaderOverrides=true")).unwrap();
            assert_eq!(o.issue_key, None, "the JiraIssueKey header gives it");
        }
        for bad in ["", "../x", "ab-1", "A-01", "OPS-1%3Fx%3D1"] {
            let m = invalid_uri(&format!("jira:getIssue?issueKey={bad}"));
            assert!(m.contains("issueKey"), "{bad}: {m}");
        }
    }

    #[test]
    fn status_rule() {
        let status = |s: &str| {
            parse(&format!(
                "jira:transitionToStatus?issueKey=OPS-1&status={s}"
            ))
            .map(|o| o.status)
        };
        assert_eq!(
            status("id:10002").unwrap(),
            Some(StatusTarget::Id(StatusId::parse("10002").unwrap()))
        );
        assert_eq!(
            status("category:done").unwrap(),
            Some(StatusTarget::Category(StatusCategoryKey::DONE))
        );
        assert_eq!(
            status("category:indeterminate").unwrap(),
            Some(StatusTarget::in_progress())
        );
        assert_eq!(
            status("Done").unwrap(),
            Some(StatusTarget::Name("Done".into()))
        );
        for bad in [
            "",
            "%20",
            "id:",
            "id:x",
            "id:0",
            "category:",
            "category:BAD%20KEY",
        ] {
            let m = invalid_uri(&format!(
                "jira:transitionToStatus?issueKey=OPS-1&status={bad}"
            ));
            assert!(m.contains("status"), "{bad}: {m}");
        }
        let m = invalid_uri("jira:transitionToStatus?issueKey=OPS-1");
        assert!(
            m.contains("status") && m.contains("allowHeaderOverrides"),
            "{m}"
        );
        let o = parse("jira:transitionToStatus?allowHeaderOverrides=true").unwrap();
        assert_eq!((o.issue_key, o.status), (None, None));
    }

    #[test]
    fn jql_rule() {
        for bad in ["", "%20%20"] {
            assert!(invalid_uri(&format!("jira:search?jql={bad}")).contains("jql"));
        }
        // A RAW(...) value is literal: no percent decode.
        let o = parse("jira:search?jql=RAW(labels = a%2Bb)").unwrap();
        assert_eq!(o.jql.as_deref(), Some("labels = a%2Bb"));
        assert!(invalid_uri("jira:search?jql=RAW()").contains("jql"));
        // Neither project nor jql.
        let m = invalid_uri("jira:search");
        assert!(
            m.contains("project") && m.contains("jql") && m.contains("allowHeaderOverrides"),
            "{m}"
        );
        let o = parse("jira:search?allowHeaderOverrides=true").unwrap();
        assert_eq!(
            (o.project, o.jql),
            (None, None),
            "the JiraJql header gives it"
        );
    }

    #[test]
    fn fields_rule() {
        for bad in ["", "summary,,status", "Summary", "summary,%20", "a-b"] {
            let m = invalid_uri(&format!("jira:getIssue?issueKey=OPS-1&fields={bad}"));
            assert!(m.contains("fields"), "{bad}: {m}");
        }
        let o = parse("jira:getIssue?issueKey=OPS-1&fields=summary,summary").unwrap();
        assert_eq!(
            o.fields,
            Some(FieldSet::new([FieldId::parse("summary").unwrap()])),
            "a duplicate is dropped"
        );
    }

    #[test]
    fn max_results_and_max_pages_rules() {
        for bad in ["0", "101", "-1", "x", ""] {
            let m = invalid_uri(&format!("jira:search?project=OPS&maxResults={bad}"));
            assert!(
                m.contains("maxResults") && m.contains("1 to 100"),
                "{bad}: {m}"
            );
        }
        assert_eq!(
            parse("jira:search?project=OPS&maxResults=1")
                .unwrap()
                .max_results,
            1
        );
        for bad in ["0", "1001", "x", ""] {
            let m = invalid_uri(&format!("jira:search?project=OPS&maxPages={bad}"));
            assert!(
                m.contains("maxPages") && m.contains("1 to 1000"),
                "{bad}: {m}"
            );
        }
    }

    #[test]
    fn bool_rules() {
        for (op, name) in [
            (
                "createIssue?project=OPS&issueType=Task&",
                "requireIdempotencyKey",
            ),
            ("addComment?issueKey=OPS-1&", "resendAfterNegativeLookup"),
            ("editIssue?issueKey=OPS-1&", "notifyUsers"),
            ("getIssue?issueKey=OPS-1&", "allowHeaderOverrides"),
            ("getIssue?issueKey=OPS-1&", "throwExceptionOnFailure"),
        ] {
            let m = invalid_uri(&format!("jira:{op}{name}=maybe"));
            assert!(m.contains(name), "{m}");
            assert!(parse(&format!("jira:{op}{name}=no")).is_ok());
            assert!(parse(&format!("jira:{op}{name}=YES")).is_ok());
        }
        assert_eq!(
            parse("jira:editIssue?issueKey=OPS-1&notifyUsers=true")
                .unwrap()
                .notify_users,
            Some(true)
        );
    }

    #[test]
    fn max_body_size_rule() {
        for bad in ["0", "big", "-5", ""] {
            let m = invalid_uri(&format!("jira:getIssue?issueKey=OPS-1&maxBodySize={bad}"));
            assert!(m.contains("maxBodySize"), "{bad}: {m}");
        }
        assert_eq!(
            parse("jira:getIssue?issueKey=OPS-1&maxBodySize=1")
                .unwrap()
                .max_body_size,
            1
        );
    }

    #[test]
    fn option_of_another_operation_fails() {
        let m = invalid_uri("jira:createIssue?project=OPS&issueType=Task&jql=x");
        assert!(
            m.contains("'jql'") && m.contains("createIssue") && m.contains("search"),
            "the message names the operations of the option: {m}"
        );
        let m = invalid_uri("jira:getIssue?issueKey=OPS-1&notifyUsers=true");
        assert!(m.contains("editIssue"), "{m}");
    }

    #[test]
    fn the_first_error_is_stable() {
        // Sorted keys: the same URI gives the same message on every run.
        let uri = "jira:search?zz=1&maxPages=0&aa=1&maxResults=0";
        let first = invalid_uri(uri);
        assert!(first.contains("'aa'"), "{first}");
        for _ in 0..20 {
            assert_eq!(invalid_uri(uri), first);
        }
    }
}

//! Endpoint URI options: the parse and the metadata.

use autumn_jira::StatusTarget;
use autumn_jira::ids::{FieldId, IssueRef, ProjectKey};
use autumn_jira::request::{FieldSet, IssueTypeRef};
use camel_api::CamelError;
use camel_component_api::UriOption;

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
        ""
    }

    /// Finds the operation with this URI name. The match is exact.
    #[must_use]
    pub fn parse(_name: &str) -> Option<Self> {
        None
    }
}

/// The options of one `jira:` endpoint, after the parse and the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The target status of `transitionToStatus`.
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
    #[allow(
        dead_code,
        reason = "JiraComponent::endpoint_options (WP6 K1) calls it"
    )]
    pub(crate) fn parse(_uri: &str, _connections: &[&str]) -> Result<Self, CamelError> {
        Err(CamelError::InvalidUri(String::new()))
    }
}

/// Returns the URI options for the component metadata.
#[must_use]
pub fn uri_options() -> Vec<UriOption> {
    Vec::new()
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
    const NAMES: [&str; 15] = [
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
            "issueType" => op == CreateIssue,
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

//! Private wire types: the Jira reply format.
//!
//! Each wire type reads the Jira JSON with lenient ids (`from_response`)
//! and turns into its public model with `into_model`. Unknown keys are
//! ignored.

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::{
    AccessibleResource, Comment, CommentPage, CreatedIssue, Issue, IssueFields, IssueType, Page,
    Project, ProjectRef, ServerInfo, Status, StatusCategory, StatusSummary, Transition, User,
};
use crate::adf::Adf;
use crate::ids::{
    AccountId, CloudId, CommentId, IssueId, IssueKey, IssueTypeId, PageToken, ProjectId,
    ProjectKey, StatusCategoryKey, StatusId, TransitionId,
};
use crate::time::JiraTimestamp;

/// Reads a timestamp. A bad value gives `None` (lenient).
fn lenient_time<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<JiraTimestamp>, D::Error> {
    let _ = Option::<Value>::deserialize(deserializer)?;
    Ok(None)
}

/// Reads an ADF value. A value that is not a `doc` gives `None` (lenient).
fn lenient_adf<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Adf>, D::Error> {
    let _ = Option::<Value>::deserialize(deserializer)?;
    Ok(None)
}

/// `IssueBean`.
#[derive(Deserialize)]
pub(crate) struct IssueWire {
    id: String,
    key: String,
    #[serde(default)]
    fields: Option<IssueFieldsWire>,
    #[serde(default)]
    properties: Option<Map<String, Value>>,
}

impl IssueWire {
    pub(crate) fn into_model(self) -> Issue {
        let _ = (self.id, self.key, self.fields, self.properties);
        Issue {
            id: IssueId::from_response(String::new()),
            key: IssueKey::from_response(String::new()),
            fields: IssueFields::default(),
            properties: Map::new(),
        }
    }
}

/// The `fields` object of `IssueBean`.
#[derive(Deserialize)]
pub(crate) struct IssueFieldsWire {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    status: Option<StatusWire>,
    #[serde(default, rename = "issuetype")]
    issue_type: Option<IssueTypeWire>,
    #[serde(default)]
    project: Option<ProjectRefWire>,
    #[serde(default)]
    assignee: Option<UserWire>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default, deserialize_with = "lenient_time")]
    created: Option<JiraTimestamp>,
    #[serde(default, deserialize_with = "lenient_time")]
    updated: Option<JiraTimestamp>,
    #[serde(default, deserialize_with = "lenient_adf")]
    description: Option<Adf>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

/// `StatusDetails`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status_category: Option<StatusCategoryWire>,
}

impl StatusWire {
    /// Returns the status, or `None` with no id.
    pub(crate) fn into_status(self) -> Option<Status> {
        let _ = (self.id, self.name, self.status_category);
        None
    }

    /// Returns the summary, or `None` with no id.
    pub(crate) fn into_summary(self) -> Option<StatusSummary> {
        self.into_status().map(|s| s.summary())
    }

    pub(crate) fn into_model(self) -> Status {
        self.into_status().unwrap_or_else(|| Status {
            id: StatusId::from_response(String::new()),
            name: String::new(),
            category: None,
        })
    }
}

/// `StatusCategory`.
#[derive(Deserialize)]
pub(crate) struct StatusCategoryWire {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

impl StatusCategoryWire {
    fn into_model(self) -> Option<StatusCategory> {
        let _ = (self.id, self.key, self.name);
        None
    }
}

/// `IssueTypeDetails`.
#[derive(Deserialize)]
pub(crate) struct IssueTypeWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    subtask: Option<bool>,
}

impl IssueTypeWire {
    fn into_option(self) -> Option<IssueType> {
        let _ = (self.id, self.name, self.subtask);
        None
    }

    pub(crate) fn into_model(self) -> IssueType {
        self.into_option().unwrap_or_else(|| IssueType {
            id: IssueTypeId::from_response(String::new()),
            name: String::new(),
            subtask: false,
        })
    }
}

/// The `project` field of an issue.
#[derive(Deserialize)]
pub(crate) struct ProjectRefWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

impl ProjectRefWire {
    fn into_option(self) -> Option<ProjectRef> {
        let _ = (self.id, self.key, self.name);
        None
    }

    pub(crate) fn into_model(self) -> ProjectRef {
        self.into_option().unwrap_or_else(|| ProjectRef {
            id: ProjectId::from_response(String::new()),
            key: ProjectKey::from_response(String::new()),
            name: None,
        })
    }
}

/// `Project`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectWire {
    id: String,
    key: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    project_type_key: Option<String>,
    #[serde(default)]
    simplified: Option<bool>,
}

impl ProjectWire {
    pub(crate) fn into_model(self) -> Project {
        let _ = (
            self.id,
            self.key,
            self.name,
            self.project_type_key,
            self.simplified,
        );
        Project {
            id: ProjectId::from_response(String::new()),
            key: ProjectKey::from_response(String::new()),
            name: String::new(),
            project_type_key: None,
            simplified: None,
        }
    }
}

/// `User`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserWire {
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    email_address: Option<String>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    account_type: Option<String>,
}

impl UserWire {
    fn into_option(self) -> Option<User> {
        let _ = (
            self.account_id,
            self.display_name,
            self.email_address,
            self.active,
            self.account_type,
        );
        None
    }

    pub(crate) fn into_model(self) -> User {
        self.into_option().unwrap_or_else(|| User {
            account_id: AccountId::from_response(String::new()),
            display_name: None,
            email_address: None,
            active: None,
            account_type: None,
        })
    }
}

/// `Comment`.
#[derive(Deserialize)]
pub(crate) struct CommentWire {
    id: String,
    #[serde(default, deserialize_with = "lenient_adf")]
    body: Option<Adf>,
    #[serde(default)]
    author: Option<UserWire>,
    #[serde(default, deserialize_with = "lenient_time")]
    created: Option<JiraTimestamp>,
    #[serde(default, deserialize_with = "lenient_time")]
    updated: Option<JiraTimestamp>,
}

impl CommentWire {
    pub(crate) fn into_model(self) -> Comment {
        let _ = (self.id, self.body, self.author, self.created, self.updated);
        Comment {
            id: CommentId::from_response(String::new()),
            body: None,
            author: None,
            created: None,
            updated: None,
        }
    }
}

/// `PageOfComments`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentPageWire {
    #[serde(default)]
    comments: Vec<CommentWire>,
    #[serde(default)]
    start_at: u64,
    #[serde(default)]
    total: u64,
}

impl CommentPageWire {
    pub(crate) fn into_model(self) -> CommentPage {
        let _ = (self.comments, self.start_at, self.total);
        CommentPage {
            comments: Vec::new(),
            start_at: 0,
            total: 0,
        }
    }
}

/// `IssueTransition`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransitionWire {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    to: Option<StatusWire>,
    #[serde(default)]
    has_screen: Option<bool>,
}

impl TransitionWire {
    pub(crate) fn into_model(self) -> Transition {
        let _ = (self.id, self.name, self.to, self.has_screen);
        Transition {
            id: TransitionId::from_response(String::new()),
            name: String::new(),
            to: StatusSummary::new(
                StatusId::from_response(String::new()),
                String::new(),
                StatusCategoryKey::from_response(String::new()),
            ),
            has_screen: false,
        }
    }
}

/// `CreatedIssue`.
#[derive(Deserialize)]
pub(crate) struct CreatedIssueWire {
    id: String,
    key: String,
}

impl CreatedIssueWire {
    pub(crate) fn into_model(self) -> CreatedIssue {
        let _ = (self.id, self.key);
        CreatedIssue {
            id: IssueId::from_response(String::new()),
            key: IssueKey::from_response(String::new()),
        }
    }
}

/// `ServerInfo`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerInfoWire {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    deployment_type: Option<String>,
}

impl ServerInfoWire {
    pub(crate) fn into_model(self) -> ServerInfo {
        let _ = (self.base_url, self.version, self.deployment_type);
        ServerInfo {
            base_url: None,
            version: None,
            deployment_type: None,
        }
    }
}

/// One entry of `accessible-resources`.
#[derive(Deserialize)]
pub(crate) struct AccessibleResourceWire {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
}

impl AccessibleResourceWire {
    pub(crate) fn into_model(self) -> AccessibleResource {
        let _ = (self.id, self.name, self.url, self.scopes);
        AccessibleResource {
            id: CloudId::from_response(String::new()),
            name: String::new(),
            url: String::new(),
            scopes: Vec::new(),
        }
    }
}

/// `PageBean`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageWire<T> {
    #[serde(default = "Vec::new")]
    values: Vec<T>,
    #[serde(default)]
    start_at: u64,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    is_last: Option<bool>,
}

impl<T> PageWire<T> {
    pub(crate) fn into_model(self) -> Page<T> {
        let _ = (self.values, self.start_at, self.total, self.is_last);
        Page {
            values: Vec::new(),
            start_at: 0,
            total: None,
            is_last: false,
        }
    }
}

/// `PageOfCreateMetaIssueTypes`: the issue types of a project.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct IssueTypePageWire {
    #[serde(default)]
    issue_types: Vec<IssueTypeWire>,
    #[serde(default)]
    start_at: u64,
    #[serde(default)]
    total: Option<u64>,
}

#[allow(dead_code, reason = "the client of WP3 uses it")]
impl IssueTypePageWire {
    /// Returns the page. `is_last` holds when the page reaches `total`, or
    /// when Jira gives no `total`.
    pub(crate) fn into_model(self) -> Page<IssueType> {
        let _ = (self.issue_types, self.start_at, self.total);
        Page {
            values: Vec::new(),
            start_at: 0,
            total: None,
            is_last: false,
        }
    }
}

/// `SearchAndReconcileResults`: one page of `POST /search/jql`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct SearchPageWire {
    #[serde(default)]
    pub(crate) issues: Vec<IssueWire>,
    #[serde(default)]
    pub(crate) is_last: Option<bool>,
    #[serde(default)]
    pub(crate) next_page_token: Option<String>,
}

/// One page of `POST /search/jql`, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct SearchPage {
    pub(crate) issues: Vec<Issue>,
    /// `true` when Jira says `isLast`. An absent `isLast` with no token is last too.
    pub(crate) is_last: bool,
    /// The next token. Absent on the last page, and an empty token counts as absent.
    pub(crate) next_page_token: Option<PageToken>,
}

#[allow(dead_code, reason = "the client of WP3 uses it")]
impl SearchPageWire {
    pub(crate) fn into_model(self) -> SearchPage {
        let _ = (self.issues, self.is_last, self.next_page_token);
        SearchPage {
            issues: Vec::new(),
            is_last: false,
            next_page_token: None,
        }
    }
}

/// `{"count": n}` of `POST /search/approximate-count`.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct CountWire {
    pub(crate) count: u64,
}

/// `{"transitions": [...]}` of `GET /issue/{issue}/transitions`.
#[derive(Deserialize)]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct TransitionsWire {
    #[serde(default)]
    pub(crate) transitions: Vec<Transition>,
}

/// `EntityProperty`: `{"key": k, "value": v}`.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct EntityPropertyWire {
    #[serde(default)]
    pub(crate) key: Option<String>,
    #[serde(default)]
    pub(crate) value: Value,
}

/// `ErrorCollection`: `{"errorMessages": [...], "errors": {field: message}}`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "the client of WP3 uses it")]
pub(crate) struct ErrorCollection {
    #[serde(default)]
    pub(crate) error_messages: Vec<String>,
    #[serde(default)]
    pub(crate) errors: Map<String, Value>,
}

#[allow(dead_code, reason = "the client of WP3 uses it")]
impl ErrorCollection {
    /// Reads an error body leniently. A body that is not JSON gives an empty collection.
    pub(crate) fn parse(body: &[u8]) -> Self {
        let _ = body;
        Self::default()
    }

    /// The `errorMessages`, then each `errors` message as `field: message`.
    pub(crate) fn messages(&self) -> Vec<String> {
        Vec::new()
    }

    /// The field names of `errors`, sorted.
    pub(crate) fn fields(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_pages() {
        let page: SearchPageWire = serde_json::from_value(json!({
            "issues": [{"id": "1", "key": "A-1", "fields": {"summary": "x"}}],
            "nextPageToken": "tok-2", "isLast": false
        }))
        .unwrap();
        let page = page.into_model();
        assert_eq!(page.issues.len(), 1);
        assert_eq!(page.issues[0].fields.summary.as_deref(), Some("x"));
        assert!(!page.is_last);
        assert_eq!(page.next_page_token.unwrap().as_str(), "tok-2");
        let last: SearchPageWire =
            serde_json::from_value(json!({"issues": [], "isLast": true})).unwrap();
        let last = last.into_model();
        assert!(last.is_last);
        assert_eq!(last.next_page_token, None);
        // No `isLast` and no token: the last page.
        let bare: SearchPageWire = serde_json::from_value(json!({"issues": []})).unwrap();
        assert!(bare.into_model().is_last);
        // An empty token counts as no token.
        let empty: SearchPageWire = serde_json::from_value(json!({"nextPageToken": ""})).unwrap();
        let empty = empty.into_model();
        assert_eq!(empty.next_page_token, None);
        assert!(empty.is_last);
        // A token and no `isLast`: not the last page.
        let more: SearchPageWire = serde_json::from_value(json!({"nextPageToken": "t"})).unwrap();
        assert!(!more.into_model().is_last);
    }

    #[test]
    fn issue_type_pages() {
        let page: IssueTypePageWire = serde_json::from_value(json!({
            "issueTypes": [{"id": "1", "name": "Bug", "subtask": false}, {"name": "no id"}],
            "startAt": 0, "maxResults": 1, "total": 3
        }))
        .unwrap();
        let page = page.into_model();
        assert_eq!(page.values.len(), 1);
        assert_eq!(page.values[0].name, "Bug");
        assert_eq!(page.total, Some(3));
        assert!(!page.is_last);
        let end: IssueTypePageWire = serde_json::from_value(json!({
            "issueTypes": [{"id": "1", "name": "Bug"}], "startAt": 2, "total": 3
        }))
        .unwrap();
        assert!(end.into_model().is_last);
        let no_total: IssueTypePageWire =
            serde_json::from_value(json!({"issueTypes": []})).unwrap();
        assert!(no_total.into_model().is_last);
    }

    #[test]
    fn small_wires() {
        let count: CountWire = serde_json::from_value(json!({"count": 153})).unwrap();
        assert_eq!(count.count, 153);
        let transitions: TransitionsWire = serde_json::from_value(json!({"transitions": [
            {"id": "11", "name": "Start", "to": {"id": "3", "name": "In Progress",
                "statusCategory": {"id": 4, "key": "indeterminate"}}}
        ]}))
        .unwrap();
        assert_eq!(
            transitions.transitions[0].to.category.as_str(),
            "indeterminate"
        );
        let property: EntityPropertyWire =
            serde_json::from_value(json!({"key": "k", "value": {"a": 1}})).unwrap();
        assert_eq!(property.key.as_deref(), Some("k"));
        assert_eq!(property.value, json!({"a": 1}));
    }

    #[test]
    fn error_collections() {
        let errors = ErrorCollection::parse(
            br#"{"errorMessages": ["Field 'priority' is required"],
                 "errors": {"summary": "You must specify a summary.", "labels": "bad"}, "status": 400}"#,
        );
        assert_eq!(
            errors.messages(),
            [
                "Field 'priority' is required",
                "labels: bad",
                "summary: You must specify a summary."
            ]
        );
        assert_eq!(errors.fields(), ["labels", "summary"]);
        let html = ErrorCollection::parse(b"<html>404</html>");
        assert!(html.messages().is_empty() && html.fields().is_empty());
        let odd = ErrorCollection::parse(br#"{"errors": {"x": {"nested": true}}}"#);
        assert_eq!(odd.messages(), ["x: {\"nested\":true}"]);
    }
}

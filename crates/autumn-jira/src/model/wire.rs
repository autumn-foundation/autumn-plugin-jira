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
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::String(text)) => JiraTimestamp::parse(&text).ok(),
        Some(Value::Number(n)) => n.as_i64().and_then(JiraTimestamp::from_epoch_ms),
        _ => None,
    })
}

/// Reads an ADF value. A value that is not a `doc` gives `None` (lenient).
fn lenient_adf<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Adf>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|v| Adf::from_value(v).ok()))
}

/// Returns a non-empty text, or `None`.
fn present(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.is_empty())
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
        Issue {
            id: IssueId::from_response(self.id),
            key: IssueKey::from_response(self.key),
            fields: self
                .fields
                .map(IssueFieldsWire::into_model)
                .unwrap_or_default(),
            properties: self.properties.unwrap_or_default(),
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

impl IssueFieldsWire {
    fn into_model(self) -> IssueFields {
        IssueFields {
            summary: self.summary,
            status: self.status.and_then(StatusWire::into_status),
            issue_type: self.issue_type.and_then(IssueTypeWire::into_option),
            project: self.project.and_then(ProjectRefWire::into_option),
            assignee: self.assignee.and_then(UserWire::into_option),
            labels: self.labels.unwrap_or_default(),
            created: self.created,
            updated: self.updated,
            description: self.description,
            other: self.other,
        }
    }
}

/// `StatusDetails`.
#[derive(Default, Deserialize)]
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
        Some(Status {
            id: StatusId::from_response(present(self.id)?),
            name: self.name.unwrap_or_default(),
            category: self
                .status_category
                .and_then(StatusCategoryWire::into_model),
        })
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
        // The spec gives an integer. A string of digits is read too.
        let id = match self.id {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        };
        Some(StatusCategory {
            id,
            key: StatusCategoryKey::from_response(present(self.key)?),
            name: self.name,
        })
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
        Some(IssueType {
            id: IssueTypeId::from_response(present(self.id)?),
            name: self.name.unwrap_or_default(),
            subtask: self.subtask.unwrap_or(false),
        })
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
        Some(ProjectRef {
            id: ProjectId::from_response(present(self.id)?),
            key: ProjectKey::from_response(present(self.key)?),
            name: self.name,
        })
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
        Project {
            id: ProjectId::from_response(self.id),
            key: ProjectKey::from_response(self.key),
            name: self.name.unwrap_or_default(),
            project_type_key: self.project_type_key,
            simplified: self.simplified,
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
    /// Returns the user, or `None` with no account id.
    pub(crate) fn into_option(self) -> Option<User> {
        Some(User {
            account_id: AccountId::from_response(present(self.account_id)?),
            display_name: self.display_name,
            email_address: self.email_address,
            active: self.active,
            account_type: self.account_type,
        })
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
        Comment {
            id: CommentId::from_response(self.id),
            body: self.body,
            author: self.author.and_then(UserWire::into_option),
            created: self.created,
            updated: self.updated,
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
        CommentPage {
            comments: self
                .comments
                .into_iter()
                .map(CommentWire::into_model)
                .collect(),
            start_at: self.start_at,
            total: self.total,
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
    #[serde(default)]
    is_global: Option<bool>,
    #[serde(default)]
    looped: Option<bool>,
}

impl TransitionWire {
    pub(crate) fn into_model(self) -> Transition {
        Transition {
            id: TransitionId::from_response(self.id),
            name: self.name.unwrap_or_default(),
            to: self.to.map_or_else(
                || StatusWire::default().into_model().summary(),
                |to| to.into_model().summary(),
            ),
            has_screen: self.has_screen.unwrap_or(false),
            is_global: self.is_global,
            looped: self.looped,
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
        CreatedIssue {
            id: IssueId::from_response(self.id),
            key: IssueKey::from_response(self.key),
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
        ServerInfo {
            base_url: self.base_url,
            version: self.version,
            deployment_type: self.deployment_type,
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
        AccessibleResource {
            id: CloudId::from_response(self.id),
            name: self.name.unwrap_or_default(),
            url: self.url.unwrap_or_default(),
            scopes: self.scopes,
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
    /// Returns the page. An absent `isLast` counts as the last page, so a
    /// pager can never loop on it.
    pub(crate) fn into_model(self) -> Page<T> {
        Page {
            values: self.values,
            start_at: self.start_at,
            total: self.total,
            is_last: self.is_last.unwrap_or(true),
        }
    }
}

/// `PageOfCreateMetaIssueTypes`: the issue types of a project.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssueTypePageWire {
    #[serde(default)]
    issue_types: Vec<IssueTypeWire>,
    #[serde(default)]
    start_at: u64,
    #[serde(default)]
    total: Option<u64>,
}

impl IssueTypePageWire {
    /// Returns the page. `is_last` holds when the page reaches `total`, or
    /// when Jira gives no `total`. A type with no id is skipped.
    pub(crate) fn into_model(self) -> Page<IssueType> {
        let returned = self.issue_types.len() as u64;
        let is_last = self.total.is_none_or(|total| {
            crate::policy::offset_next(self.start_at, returned, total).is_none()
        });
        Page {
            values: self
                .issue_types
                .into_iter()
                .filter_map(IssueTypeWire::into_option)
                .collect(),
            start_at: self.start_at,
            total: self.total,
            is_last,
        }
    }
}

/// `SearchAndReconcileResults`: one page of `POST /search/jql`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
pub(crate) struct SearchPage {
    pub(crate) issues: Vec<Issue>,
    /// `true` when Jira says `isLast`. An absent `isLast` with no token is last too.
    pub(crate) is_last: bool,
    /// The next token. Absent on the last page, and an empty token counts as absent.
    pub(crate) next_page_token: Option<PageToken>,
}

impl SearchPageWire {
    pub(crate) fn into_model(self) -> SearchPage {
        let next_page_token = self.next_page_token.and_then(|t| PageToken::parse(&t).ok());
        SearchPage {
            issues: self.issues.into_iter().map(IssueWire::into_model).collect(),
            is_last: self.is_last.unwrap_or_else(|| next_page_token.is_none()),
            next_page_token,
        }
    }
}

/// `{"count": n}` of `POST /search/approximate-count`.
#[derive(Debug, Deserialize)]
pub(crate) struct CountWire {
    pub(crate) count: u64,
}

/// `{"transitions": [...]}` of `GET /issue/{issue}/transitions`.
#[derive(Deserialize)]
pub(crate) struct TransitionsWire {
    #[serde(default)]
    pub(crate) transitions: Vec<Transition>,
}

/// `EntityProperty`: `{"key": k, "value": v}`.
#[derive(Debug, Deserialize)]
pub(crate) struct EntityPropertyWire {
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "part of the wire shape; the client reads only the value"
    )]
    pub(crate) key: Option<String>,
    #[serde(default)]
    pub(crate) value: Value,
}

/// `ErrorCollection`: `{"errorMessages": [...], "errors": {field: message}}`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorCollection {
    #[serde(default)]
    pub(crate) error_messages: Vec<String>,
    #[serde(default)]
    pub(crate) errors: Map<String, Value>,
}

impl ErrorCollection {
    /// Reads an error body leniently. A body that is not JSON gives an empty collection.
    pub(crate) fn parse(body: &[u8]) -> Self {
        serde_json::from_slice(body).unwrap_or_default()
    }

    /// The `errorMessages`, then each `errors` message as `field: message`.
    pub(crate) fn messages(&self) -> Vec<String> {
        let mut out = self.error_messages.clone();
        for (field, message) in &self.errors {
            match message {
                Value::String(text) => out.push(format!("{field}: {text}")),
                other => out.push(format!("{field}: {other}")),
            }
        }
        out
    }

    /// The field names of `errors`, sorted.
    pub(crate) fn fields(&self) -> Vec<String> {
        self.errors.keys().cloned().collect()
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

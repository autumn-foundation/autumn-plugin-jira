//! Request types: what the client sends to Jira.
//!
//! The builders return `Self`. The JSON bodies are `pub(crate)`: the client
//! builds them, and it adds the markers of an idempotent write.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::adf::Adf;
use crate::error::{JiraError, Result};
use crate::ids::{
    AccountId, FieldId, IssueTypeId, PageToken, ProjectKey, StatusCategoryKey, StatusId,
    TransitionId,
};
use crate::jql::Jql;
use crate::model::StatusSummary;

/// The largest `maxResults` of one search page.
pub const MAX_SEARCH_RESULTS: u32 = 100;
/// The largest number of issue properties in one search.
pub const MAX_SEARCH_PROPERTIES: usize = 5;

/// An issue type, by id or by name. The body is `{"id": ..}` or `{"name": ..}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IssueTypeRef {
    /// By numeric id.
    Id(IssueTypeId),
    /// By name. Jira looks up the name in the project and the locale.
    Name(String),
}

impl IssueTypeRef {
    /// Returns the JSON body.
    #[allow(dead_code, reason = "the client of WP3 uses it")]
    pub(crate) fn to_body(&self) -> Value {
        match self {
            Self::Id(id) => json!({"id": id}),
            Self::Name(name) => json!({"name": name}),
        }
    }
}

/// A new issue.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
#[non_exhaustive]
pub struct NewIssue {
    project: ProjectKey,
    issue_type: IssueTypeRef,
    summary: String,
    description: Option<Adf>,
    labels: Vec<String>,
    assignee: Option<AccountId>,
    priority_name: Option<String>,
    fields: Vec<(FieldId, Value)>,
    properties: Vec<(String, Value)>,
}

impl NewIssue {
    /// Starts a new issue.
    pub fn new(project: ProjectKey, issue_type: IssueTypeRef, summary: impl Into<String>) -> Self {
        Self {
            project,
            issue_type,
            summary: summary.into(),
            description: None,
            labels: Vec::new(),
            assignee: None,
            priority_name: None,
            fields: Vec::new(),
            properties: Vec::new(),
        }
    }

    /// Sets the description.
    pub fn description(mut self, description: Adf) -> Self {
        self.description = Some(description);
        self
    }

    /// Adds labels.
    pub fn labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels.extend(labels.into_iter().map(Into::into));
        self
    }

    /// Sets the assignee.
    pub fn assignee(mut self, account: AccountId) -> Self {
        self.assignee = Some(account);
        self
    }

    /// Sets the priority by name.
    pub fn priority_name(mut self, name: impl Into<String>) -> Self {
        self.priority_name = Some(name.into());
        self
    }

    /// Sets a field as given. The app owns custom fields: the value goes
    /// to Jira with no change, and it replaces a typed field of the same id.
    pub fn field(mut self, id: FieldId, value: Value) -> Self {
        self.fields.push((id, value));
        self
    }

    /// Adds an issue property. Jira sets it in the same create call.
    pub fn property(mut self, key: impl Into<String>, value: Value) -> Self {
        self.properties.push((key.into(), value));
        self
    }

    /// Returns the project.
    #[must_use]
    pub const fn project_key(&self) -> &ProjectKey {
        &self.project
    }

    /// Returns the summary.
    #[must_use]
    pub fn summary_text(&self) -> &str {
        &self.summary
    }

    /// Returns the labels.
    #[must_use]
    pub fn label_list(&self) -> &[String] {
        &self.labels
    }

    /// Returns the JSON body of `POST /issue`.
    #[allow(dead_code, reason = "the client of WP3 uses it")]
    pub(crate) fn to_body(&self) -> Value {
        let mut fields = Map::new();
        fields.insert("project".to_owned(), json!({"key": self.project}));
        fields.insert("issuetype".to_owned(), self.issue_type.to_body());
        fields.insert("summary".to_owned(), json!(self.summary));
        if let Some(description) = &self.description {
            fields.insert("description".to_owned(), description.as_value().clone());
        }
        if !self.labels.is_empty() {
            fields.insert("labels".to_owned(), json!(self.labels));
        }
        if let Some(assignee) = &self.assignee {
            fields.insert("assignee".to_owned(), json!({"accountId": assignee}));
        }
        if let Some(priority) = &self.priority_name {
            fields.insert("priority".to_owned(), json!({"name": priority}));
        }
        for (id, value) in &self.fields {
            fields.insert(id.as_str().to_owned(), value.clone());
        }
        let mut body = Map::new();
        body.insert("fields".to_owned(), Value::Object(fields));
        if !self.properties.is_empty() {
            let properties: Vec<Value> = self
                .properties
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect();
            body.insert("properties".to_owned(), Value::Array(properties));
        }
        Value::Object(body)
    }
}

/// An edit of an issue: `PUT /issue/{issue}`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[must_use]
#[non_exhaustive]
pub struct IssueEdit {
    fields: Map<String, Value>,
    label_ops: Vec<Value>,
}

impl IssueEdit {
    /// Starts an empty edit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the summary.
    pub fn set_summary(mut self, summary: impl Into<String>) -> Self {
        self.fields
            .insert("summary".to_owned(), Value::String(summary.into()));
        self
    }

    /// Sets the description.
    pub fn set_description(mut self, description: Adf) -> Self {
        self.fields
            .insert("description".to_owned(), description.into_value());
        self
    }

    /// Adds a label.
    pub fn add_label(mut self, label: impl Into<String>) -> Self {
        self.label_ops.push(json!({"add": label.into()}));
        self
    }

    /// Removes a label.
    pub fn remove_label(mut self, label: impl Into<String>) -> Self {
        self.label_ops.push(json!({"remove": label.into()}));
        self
    }

    /// Sets the assignee. `None` removes the assignee.
    pub fn set_assignee(mut self, account: Option<AccountId>) -> Self {
        let value = account.map_or(Value::Null, |a| json!({"accountId": a}));
        self.fields.insert("assignee".to_owned(), value);
        self
    }

    /// Sets a field as given. The app owns custom fields.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "builder style: the caller hands over the id"
    )]
    pub fn set_field(mut self, id: FieldId, value: Value) -> Self {
        self.fields.insert(id.as_str().to_owned(), value);
        self
    }

    /// Returns `true` when the edit changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.label_ops.is_empty()
    }

    /// Returns the JSON body.
    #[allow(dead_code, reason = "the client of WP3 uses it")]
    pub(crate) fn to_body(&self) -> Value {
        let mut body = Map::new();
        if !self.fields.is_empty() {
            body.insert("fields".to_owned(), Value::Object(self.fields.clone()));
        }
        if !self.label_ops.is_empty() {
            body.insert("update".to_owned(), json!({"labels": self.label_ops}));
        }
        Value::Object(body)
    }
}

/// The query options of an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct EditOptions {
    /// `notifyUsers`. The client sends it only when it is `Some` (F44).
    pub notify_users: Option<bool>,
}

impl EditOptions {
    /// Sets `notifyUsers`.
    #[must_use]
    pub const fn notify_users(notify: bool) -> Self {
        Self {
            notify_users: Some(notify),
        }
    }

    /// Returns the query pairs.
    #[allow(dead_code, reason = "the client of WP3 uses it")]
    pub(crate) fn to_query(self) -> Vec<(String, String)> {
        self.notify_users
            .map(|n| ("notifyUsers".to_owned(), n.to_string()))
            .into_iter()
            .collect()
    }
}

/// The fields that a read asks for. Jira returns only these (F17).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct FieldSet(Vec<FieldId>);

/// The field ids of `FieldSet::default`.
const DEFAULT_FIELDS: [&str; 9] = [
    "summary",
    "status",
    "issuetype",
    "project",
    "assignee",
    "labels",
    "created",
    "updated",
    "description",
];

impl Default for FieldSet {
    /// `summary,status,issuetype,project,assignee,labels,created,updated,description`.
    fn default() -> Self {
        Self(DEFAULT_FIELDS.into_iter().map(FieldId::known).collect())
    }
}

impl FieldSet {
    /// A set of the given fields, in order, with no duplicates.
    pub fn new(fields: impl IntoIterator<Item = FieldId>) -> Self {
        fields.into_iter().fold(Self(Vec::new()), Self::with)
    }

    /// Adds a field. A field that is in the set already is skipped.
    pub fn with(mut self, field: FieldId) -> Self {
        if !self.0.contains(&field) {
            self.0.push(field);
        }
        self
    }

    /// Returns the fields.
    #[must_use]
    pub fn ids(&self) -> &[FieldId] {
        &self.0
    }

    /// Returns the fields joined by `,` for a query.
    #[must_use]
    pub fn to_csv(&self) -> String {
        self.0
            .iter()
            .map(FieldId::as_str)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A search: `POST /search/jql`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
#[non_exhaustive]
pub struct SearchRequest {
    jql: Jql,
    fields: FieldSet,
    max_results: u32,
    properties: Vec<String>,
}

impl SearchRequest {
    /// Starts a search with the default fields and 50 results a page.
    pub fn new(jql: Jql) -> Self {
        Self {
            jql,
            fields: FieldSet::default(),
            max_results: 50,
            properties: Vec::new(),
        }
    }

    /// Sets the fields. Each search sends its fields (F17).
    pub fn fields(mut self, fields: FieldSet) -> Self {
        self.fields = fields;
        self
    }

    /// Sets the page size, clamped to 1 to 100.
    pub fn max_results(mut self, max: u32) -> Self {
        self.max_results = max.clamp(1, MAX_SEARCH_RESULTS);
        self
    }

    /// Asks for issue properties. Jira takes up to 5: more give
    /// `InvalidInput` when the client sends the search.
    pub fn properties(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties.extend(keys.into_iter().map(Into::into));
        self
    }

    /// Returns the query.
    #[must_use]
    pub const fn jql(&self) -> &Jql {
        &self.jql
    }

    /// Returns the page size.
    #[must_use]
    pub const fn page_size(&self) -> u32 {
        self.max_results
    }

    /// Returns the JSON body of one page.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for more than 5 properties.
    #[allow(dead_code, reason = "the client of WP3 uses it")]
    pub(crate) fn to_body(&self, next_page_token: Option<&PageToken>) -> Result<Value> {
        if self.properties.len() > MAX_SEARCH_PROPERTIES {
            return Err(JiraError::invalid(
                "search",
                "a search takes up to 5 issue properties",
            ));
        }
        let mut body = Map::new();
        body.insert("jql".to_owned(), json!(self.jql.as_str()));
        body.insert("fields".to_owned(), json!(self.fields.ids()));
        body.insert("maxResults".to_owned(), json!(self.max_results));
        if !self.properties.is_empty() {
            body.insert("properties".to_owned(), json!(self.properties));
        }
        if let Some(token) = next_page_token {
            body.insert("nextPageToken".to_owned(), json!(token.as_str()));
        }
        Ok(Value::Object(body))
    }
}

/// The target of `transition_to_status`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StatusTarget {
    /// A status name. The match ignores ASCII case.
    Name(String),
    /// A status id.
    Id(StatusId),
    /// A status category.
    Category(StatusCategoryKey),
}

impl StatusTarget {
    /// The `done` category.
    #[must_use]
    pub const fn done() -> Self {
        Self::Category(StatusCategoryKey::DONE)
    }

    /// The `indeterminate` (in progress) category.
    #[must_use]
    pub const fn in_progress() -> Self {
        Self::Category(StatusCategoryKey::INDETERMINATE)
    }

    /// The `new` (to do) category.
    #[must_use]
    pub const fn todo() -> Self {
        Self::Category(StatusCategoryKey::NEW)
    }

    /// Returns `true` when `status` is at the target.
    #[must_use]
    pub fn matches(&self, status: &StatusSummary) -> bool {
        match self {
            Self::Name(name) => name.eq_ignore_ascii_case(&status.name),
            Self::Id(id) => *id == status.id,
            Self::Category(category) => *category == status.category,
        }
    }
}

impl fmt::Display for StatusTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(f, "status {name}"),
            Self::Id(id) => write!(f, "status id {id}"),
            Self::Category(category) => write!(f, "category {category}"),
        }
    }
}

/// The fields and updates of a transition, for example a `resolution`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct TransitionFields {
    /// `fields`, for example `{"resolution": {"name": "Done"}}`.
    pub fields: Map<String, Value>,
    /// `update`, for example a comment.
    pub update: Map<String, Value>,
}

impl TransitionFields {
    /// Sets a field.
    #[must_use]
    pub fn field(mut self, name: impl Into<String>, value: Value) -> Self {
        self.fields.insert(name.into(), value);
        self
    }

    /// Sets the resolution by name: `{"resolution": {"name": ..}}`.
    #[must_use]
    pub fn resolution(self, name: &str) -> Self {
        self.field("resolution", json!({"name": name}))
    }

    /// Returns the JSON body of `POST /issue/{issue}/transitions`.
    #[allow(dead_code, reason = "the client of WP3 uses it")]
    pub(crate) fn to_body(&self, id: &TransitionId) -> Value {
        let mut body = Map::new();
        body.insert("transition".to_owned(), json!({"id": id}));
        if !self.fields.is_empty() {
            body.insert("fields".to_owned(), Value::Object(self.fields.clone()));
        }
        if !self.update.is_empty() {
            body.insert("update".to_owned(), Value::Object(self.update.clone()));
        }
        Value::Object(body)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    fn ops() -> ProjectKey {
        ProjectKey::parse("OPS").unwrap()
    }

    #[test]
    fn new_issue_body() {
        let issue = NewIssue::new(ops(), IssueTypeRef::Name("Task".to_owned()), "Disk full")
            .description(Adf::text("Body"))
            .labels(["alert"])
            .labels(vec!["a2".to_owned()])
            .assignee(AccountId::parse("5b10a2844c20165700ede21g").unwrap())
            .priority_name("High")
            .field(
                FieldId::parse("customfield_10010").unwrap(),
                json!({"value": "x"}),
            )
            .property("autumn-jira", json!({"idempotencyKey": "k"}));
        assert_eq!(issue.project_key(), &ops());
        assert_eq!(issue.summary_text(), "Disk full");
        assert_eq!(issue.label_list(), ["alert", "a2"]);
        assert_eq!(
            issue.to_body(),
            json!({
                "fields": {
                    "project": {"key": "OPS"},
                    "issuetype": {"name": "Task"},
                    "summary": "Disk full",
                    "description": Adf::text("Body").as_value(),
                    "labels": ["alert", "a2"],
                    "assignee": {"accountId": "5b10a2844c20165700ede21g"},
                    "priority": {"name": "High"},
                    "customfield_10010": {"value": "x"}
                },
                "properties": [{"key": "autumn-jira", "value": {"idempotencyKey": "k"}}]
            })
        );
        let plain = NewIssue::new(
            ops(),
            IssueTypeRef::Id(IssueTypeId::parse("10002").unwrap()),
            "s",
        );
        assert_eq!(
            plain.to_body(),
            json!({"fields": {"project": {"key": "OPS"}, "issuetype": {"id": "10002"}, "summary": "s"}})
        );
        // The app owns custom fields: a field replaces a typed field.
        let own = NewIssue::new(ops(), IssueTypeRef::Name("Bug".to_owned()), "s")
            .field(FieldId::parse("summary").unwrap(), json!("app value"));
        assert_eq!(own.to_body()["fields"]["summary"], "app value");
    }

    #[test]
    fn issue_type_ref_shapes() {
        let by_name = IssueTypeRef::Name("Task".to_owned());
        assert_eq!(by_name.to_body(), json!({"name": "Task"}));
        assert_eq!(
            serde_json::to_value(&by_name).unwrap(),
            json!({"name": "Task"})
        );
        let by_id: IssueTypeRef = serde_json::from_value(json!({"id": "1"})).unwrap();
        assert_eq!(by_id.to_body(), json!({"id": "1"}));
        assert!(serde_json::from_value::<IssueTypeRef>(json!({"id": "0"})).is_err());
    }

    #[test]
    fn edit_body_and_options() {
        let edit = IssueEdit::new()
            .set_summary("new")
            .set_description(Adf::text("d"))
            .add_label("a")
            .remove_label("b")
            .set_assignee(None)
            .set_field(FieldId::parse("customfield_1").unwrap(), json!(5));
        assert!(!edit.is_empty());
        assert_eq!(
            edit.to_body(),
            json!({
                "fields": {
                    "summary": "new",
                    "description": Adf::text("d").as_value(),
                    "assignee": null,
                    "customfield_1": 5
                },
                "update": {"labels": [{"add": "a"}, {"remove": "b"}]}
            })
        );
        let assign = IssueEdit::new().set_assignee(Some(AccountId::parse("abc").unwrap()));
        assert_eq!(
            assign.to_body(),
            json!({"fields": {"assignee": {"accountId": "abc"}}})
        );
        let labels = IssueEdit::new().add_label("x");
        assert_eq!(
            labels.to_body(),
            json!({"update": {"labels": [{"add": "x"}]}})
        );
        assert!(IssueEdit::new().is_empty());
        assert_eq!(IssueEdit::new().to_body(), json!({}));
        assert_eq!(EditOptions::default().to_query(), []);
        assert_eq!(
            EditOptions::notify_users(false).to_query(),
            [("notifyUsers".to_owned(), "false".to_owned())]
        );
        assert_eq!(
            EditOptions::notify_users(true).to_query(),
            [("notifyUsers".to_owned(), "true".to_owned())]
        );
    }

    #[test]
    fn field_sets() {
        assert_eq!(
            FieldSet::default().to_csv(),
            "summary,status,issuetype,project,assignee,labels,created,updated,description"
        );
        assert_eq!(FieldSet::default().ids().len(), DEFAULT_FIELDS.len());
        let summary = FieldId::parse("summary").unwrap();
        let status = FieldId::parse("status").unwrap();
        let set = FieldSet::new([summary.clone(), status.clone(), summary])
            .with(FieldId::parse("customfield_10010").unwrap())
            .with(status);
        assert_eq!(set.to_csv(), "summary,status,customfield_10010");
        assert_eq!(FieldSet::new([]).to_csv(), "");
    }

    #[test]
    fn search_body() {
        let jql = Jql::project(&ops());
        let request = SearchRequest::new(jql.clone())
            .fields(FieldSet::new([FieldId::parse("summary").unwrap()]))
            .max_results(500)
            .properties(["autumn-jira"]);
        assert_eq!(request.jql(), &jql);
        assert_eq!(request.page_size(), 100);
        assert_eq!(
            request.to_body(None).unwrap(),
            json!({"jql": "project = \"OPS\"", "fields": ["summary"], "maxResults": 100, "properties": ["autumn-jira"]})
        );
        let token = PageToken::parse("tok").unwrap();
        assert_eq!(
            request.to_body(Some(&token)).unwrap()["nextPageToken"],
            "tok"
        );
        let default = SearchRequest::new(jql.clone());
        let body = default.to_body(None).unwrap();
        assert_eq!(body["maxResults"], 50);
        assert_eq!(body["fields"].as_array().unwrap().len(), 9);
        assert!(body.get("properties").is_none());
        assert_eq!(
            SearchRequest::new(jql.clone()).max_results(0).page_size(),
            1
        );
        let too_many = SearchRequest::new(jql).properties(["a", "b", "c", "d", "e", "f"]);
        assert_eq!(too_many.to_body(None).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn status_targets() {
        let done = StatusSummary::new(
            StatusId::parse("3").unwrap(),
            "Done".to_owned(),
            StatusCategoryKey::DONE,
        );
        assert!(StatusTarget::done().matches(&done));
        assert!(!StatusTarget::in_progress().matches(&done));
        assert!(!StatusTarget::todo().matches(&done));
        assert!(StatusTarget::Name("dONE".to_owned()).matches(&done));
        assert!(!StatusTarget::Name("Closed".to_owned()).matches(&done));
        assert!(StatusTarget::Id(StatusId::parse("3").unwrap()).matches(&done));
        assert!(!StatusTarget::Id(StatusId::parse("4").unwrap()).matches(&done));
        assert_eq!(StatusTarget::done().to_string(), "category done");
        assert_eq!(
            StatusTarget::Name("Done".to_owned()).to_string(),
            "status Done"
        );
        assert_eq!(
            StatusTarget::Id(StatusId::parse("3").unwrap()).to_string(),
            "status id 3"
        );
        let json = serde_json::to_value(StatusTarget::done()).unwrap();
        assert_eq!(json, json!({"category": "done"}));
        assert_eq!(
            serde_json::from_value::<StatusTarget>(json).unwrap(),
            StatusTarget::done()
        );
        assert_eq!(
            serde_json::to_value(StatusTarget::Name("Done".to_owned())).unwrap(),
            json!({"name": "Done"})
        );
    }

    #[test]
    fn transition_body() {
        let id = TransitionId::parse("31").unwrap();
        assert_eq!(
            TransitionFields::default().to_body(&id),
            json!({"transition": {"id": "31"}})
        );
        let mut with_update = TransitionFields::default()
            .resolution("Done")
            .field("customfield_1", json!(2));
        with_update
            .update
            .insert("comment".to_owned(), json!([{"add": {"body": "x"}}]));
        assert_eq!(
            with_update.to_body(&id),
            json!({
                "transition": {"id": "31"},
                "fields": {"resolution": {"name": "Done"}, "customfield_1": 2},
                "update": {"comment": [{"add": {"body": "x"}}]}
            })
        );
    }
}

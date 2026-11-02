//! Response models.
//!
//! The models read Jira replies leniently: unknown keys are ignored, ids are
//! strings, and an id keeps the shape that Jira gives (F42). `Deserialize`
//! reads the Jira wire format through the private `wire` types. `Serialize`
//! writes the Rust field names, for host replies and logs.
//!
//! `StatusSummary`, `IssueSummary` and `IssueRefs` are host values: their
//! `Serialize` and `Deserialize` use the same Rust field names, so a harvest
//! history can hold them.

pub(crate) mod wire;

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::adf::Adf;
use crate::error::{JiraError, Op, Result};
use crate::ids::{
    AccountId, CloudId, CommentId, FieldId, IssueId, IssueKey, IssueTypeId, ProjectId, ProjectKey,
    StatusCategoryKey, StatusId, TransitionId,
};
use crate::time::JiraTimestamp;

/// Implements `Deserialize` for a model through its wire type.
macro_rules! from_wire {
    ($model:ty, $wire:ty) => {
        impl<'de> Deserialize<'de> for $model {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                <$wire>::deserialize(deserializer).map(<$wire>::into_model)
            }
        }
    };
}

/// An issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Issue {
    /// The numeric id.
    pub id: IssueId,
    /// The key. A moved issue keeps the key that Jira returns.
    pub key: IssueKey,
    /// The fields that the call asked for.
    pub fields: IssueFields,
    /// The issue properties that the call asked for: key to value.
    pub properties: Map<String, Value>,
}

impl Issue {
    /// Returns an issue property that the call asked for.
    #[must_use]
    pub fn property(&self, key: &str) -> Option<&Value> {
        self.properties.get(key)
    }

    /// Returns a field that has no typed slot, for example a custom field.
    #[must_use]
    pub fn field(&self, id: &FieldId) -> Option<&Value> {
        self.fields.other.get(id.as_str())
    }

    /// Returns the status summary, when the call asked for `status`.
    #[must_use]
    pub fn status(&self) -> Option<StatusSummary> {
        self.fields.status.as_ref().map(Status::summary)
    }
}

/// The fields of an issue. A field that the call did not ask for is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[non_exhaustive]
pub struct IssueFields {
    /// `summary`.
    pub summary: Option<String>,
    /// `status`.
    pub status: Option<Status>,
    /// `issuetype`.
    pub issue_type: Option<IssueType>,
    /// `project`.
    pub project: Option<ProjectRef>,
    /// `assignee`. `None` also for an unassigned issue.
    pub assignee: Option<User>,
    /// `labels`.
    pub labels: Vec<String>,
    /// `created`.
    pub created: Option<JiraTimestamp>,
    /// `updated`.
    pub updated: Option<JiraTimestamp>,
    /// `description`.
    pub description: Option<Adf>,
    /// The other fields, as Jira gives them.
    pub other: Map<String, Value>,
}

/// A status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Status {
    /// The numeric id.
    pub id: StatusId,
    /// The name, for example `In Progress`.
    pub name: String,
    /// The category.
    pub category: Option<StatusCategory>,
}

impl Status {
    /// Returns the summary. A status with no category gets `undefined`, the
    /// key that Jira uses for "No Category".
    #[must_use]
    pub fn summary(&self) -> StatusSummary {
        StatusSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            category: self.category.as_ref().map_or_else(
                || StatusCategoryKey::from_response("undefined".to_owned()),
                |c| c.key.clone(),
            ),
        }
    }
}

/// A status category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct StatusCategory {
    /// The numeric id. Jira sends an integer here.
    pub id: Option<i64>,
    /// The key, for example `done`.
    pub key: StatusCategoryKey,
    /// The name, for example `Done`.
    pub name: Option<String>,
}

/// The id, name and category of a status.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StatusSummary {
    /// The numeric id.
    pub id: StatusId,
    /// The name.
    pub name: String,
    /// The category key.
    pub category: StatusCategoryKey,
}

impl StatusSummary {
    /// Makes a summary.
    #[must_use]
    pub const fn new(id: StatusId, name: String, category: StatusCategoryKey) -> Self {
        Self { id, name, category }
    }
}

/// A short view of an issue for hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IssueSummary {
    /// The numeric id.
    pub id: IssueId,
    /// The key.
    pub key: IssueKey,
    /// The summary.
    pub summary: Option<String>,
    /// The status.
    pub status: Option<StatusSummary>,
    /// The labels.
    pub labels: Vec<String>,
    /// The last update.
    pub updated: Option<JiraTimestamp>,
}

impl From<&Issue> for IssueSummary {
    fn from(issue: &Issue) -> Self {
        Self {
            id: issue.id.clone(),
            key: issue.key.clone(),
            summary: issue.fields.summary.clone(),
            status: issue.status(),
            labels: issue.fields.labels.clone(),
            updated: issue.fields.updated,
        }
    }
}

/// An issue type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct IssueType {
    /// The numeric id.
    pub id: IssueTypeId,
    /// The name, for example `Task`.
    pub name: String,
    /// `true` for a sub-task type.
    pub subtask: bool,
}

/// The project of an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ProjectRef {
    /// The numeric id.
    pub id: ProjectId,
    /// The key.
    pub key: ProjectKey,
    /// The name.
    pub name: Option<String>,
}

/// A project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Project {
    /// The numeric id.
    pub id: ProjectId,
    /// The key.
    pub key: ProjectKey,
    /// The name.
    pub name: String,
    /// The type, for example `software`.
    pub project_type_key: Option<String>,
    /// `true` for a team-managed project.
    pub simplified: Option<bool>,
}

/// A user. `Debug` hides the email address.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct User {
    /// The account id.
    pub account_id: AccountId,
    /// The display name.
    pub display_name: Option<String>,
    /// The email address. Privacy settings can hide it.
    pub email_address: Option<String>,
    /// `true` for an active account.
    pub active: Option<bool>,
    /// The account type, for example `atlassian` or `app`.
    pub account_type: Option<String>,
}

impl fmt::Debug for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("User")
            .field("account_id", &self.account_id)
            .field("display_name", &self.display_name)
            .field(
                "email_address",
                &self.email_address.as_ref().map(|_| "<redacted>"),
            )
            .field("active", &self.active)
            .field("account_type", &self.account_type)
            .finish()
    }
}

/// A comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Comment {
    /// The numeric id.
    pub id: CommentId,
    /// The body.
    pub body: Option<Adf>,
    /// The author.
    pub author: Option<User>,
    /// When the comment was made.
    pub created: Option<JiraTimestamp>,
    /// The last update.
    pub updated: Option<JiraTimestamp>,
}

/// One page of comments (offset pager).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct CommentPage {
    /// The comments.
    pub comments: Vec<Comment>,
    /// The offset of the first comment.
    pub start_at: u64,
    /// The number of comments on the issue.
    pub total: u64,
}

/// A transition of an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Transition {
    /// The numeric id.
    pub id: TransitionId,
    /// The name. It is not the name of the target status.
    pub name: String,
    /// The target status.
    pub to: StatusSummary,
    /// `true` when the transition has a screen.
    pub has_screen: bool,
}

/// The id and key of a new issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct CreatedIssue {
    /// The numeric id.
    pub id: IssueId,
    /// The key.
    pub key: IssueKey,
}

/// The id and key of an issue.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IssueRefs {
    /// The numeric id.
    pub id: IssueId,
    /// The key.
    pub key: IssueKey,
}

impl IssueRefs {
    /// Makes the refs.
    #[must_use]
    pub const fn new(id: IssueId, key: IssueKey) -> Self {
        Self { id, key }
    }
}

impl From<CreatedIssue> for IssueRefs {
    fn from(created: CreatedIssue) -> Self {
        Self {
            id: created.id,
            key: created.key,
        }
    }
}

/// Site information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ServerInfo {
    /// The base URL that Jira reports.
    pub base_url: Option<String>,
    /// The version.
    pub version: Option<String>,
    /// The deployment type, for example `Cloud`.
    pub deployment_type: Option<String>,
}

/// One site that an OAuth token can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AccessibleResource {
    /// The cloud id.
    pub id: CloudId,
    /// The site name.
    pub name: String,
    /// The site URL, for example `https://acme.atlassian.net`.
    pub url: String,
    /// The scopes of the token on this site.
    pub scopes: Vec<String>,
}

/// One page of an offset pager (`PageBean`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Page<T> {
    /// The values.
    pub values: Vec<T>,
    /// The offset of the first value.
    pub start_at: u64,
    /// The number of values, when Jira gives it.
    pub total: Option<u64>,
    /// `true` on the last page.
    pub is_last: bool,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Page<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        wire::PageWire::deserialize(deserializer).map(wire::PageWire::into_model)
    }
}

from_wire!(Issue, wire::IssueWire);
from_wire!(Status, wire::StatusWire);
from_wire!(IssueType, wire::IssueTypeWire);
from_wire!(ProjectRef, wire::ProjectRefWire);
from_wire!(Project, wire::ProjectWire);
from_wire!(User, wire::UserWire);
from_wire!(Comment, wire::CommentWire);
from_wire!(CommentPage, wire::CommentPageWire);
from_wire!(Transition, wire::TransitionWire);
from_wire!(CreatedIssue, wire::CreatedIssueWire);
from_wire!(ServerInfo, wire::ServerInfoWire);
from_wire!(AccessibleResource, wire::AccessibleResourceWire);

/// Decodes a reply body. The error holds the serde error class and the
/// line and column only: never a value from the body (F11).
///
/// # Errors
///
/// `JiraError::Decode` when the body does not decode.
pub(crate) fn decode<T: DeserializeOwned>(op: Op, body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|e| {
        let class = match e.classify() {
            serde_json::error::Category::Io => "io",
            serde_json::error::Category::Syntax => "syntax",
            serde_json::error::Category::Data => "data",
            serde_json::error::Category::Eof => "eof",
        };
        JiraError::Decode {
            op,
            reason: format!("{class} error at line {} column {}", e.line(), e.column()),
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue_json() -> Value {
        json!({
            "id": "10001",
            "key": "lower-7",
            "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
            "expand": "renderedFields",
            "fields": {
                "summary": "Disk full",
                "status": {"id": "3", "name": "In Progress", "self": "x",
                    "statusCategory": {"id": 4, "key": "indeterminate", "name": "In Progress", "colorName": "yellow"}},
                "issuetype": {"id": "10002", "name": "Task", "subtask": false, "iconUrl": "x"},
                "project": {"id": "10000", "key": "OPS", "name": "Operations"},
                "assignee": {"accountId": "5b10a2844c20165700ede21g", "displayName": "Mia",
                    "emailAddress": "mia@example.com", "active": true, "accountType": "atlassian"},
                "labels": ["alert", "autumn-idem-abc"],
                "created": "2021-01-17T12:34:00.000+0000",
                "updated": "2021-01-17T12:35:00.000+0000",
                "description": {"type": "doc", "version": 1, "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Body"}]}]},
                "customfield_10010": {"value": "High"},
                "priority": {"id": "2", "name": "High"}
            },
            "properties": {"autumn-jira": {"idempotencyKey": "k1"}}
        })
    }

    #[test]
    fn response_ids_are_lenient() {
        let issue: Issue = serde_json::from_value(issue_json()).unwrap();
        // A response key that the strict rule rejects is kept as given.
        assert_eq!(issue.key.as_str(), "lower-7");
        assert!(IssueKey::parse("lower-7").is_err());
        assert_eq!(issue.id.as_str(), "10001");
        let odd: Status = serde_json::from_value(json!({
            "id": "007", "name": "Odd", "statusCategory": {"id": 9, "key": "In Flight"}
        }))
        .unwrap();
        assert_eq!(odd.id.as_str(), "007");
        assert_eq!(odd.summary().category.as_str(), "In Flight");
        let project: Project =
            serde_json::from_value(json!({"id": "1", "key": "lower", "name": "x"})).unwrap();
        assert_eq!(project.key.as_str(), "lower");
        let created: CreatedIssue =
            serde_json::from_value(json!({"id": "1", "key": "x-1", "self": "y"})).unwrap();
        assert_eq!(created.key.as_str(), "x-1");
        let user: User = serde_json::from_value(json!({"accountId": "a b"})).unwrap();
        assert_eq!(user.account_id.as_str(), "a b");
    }

    #[test]
    fn issue_fields_decode() {
        let issue: Issue = serde_json::from_value(issue_json()).unwrap();
        let f = &issue.fields;
        assert_eq!(f.summary.as_deref(), Some("Disk full"));
        let status = issue.status().unwrap();
        assert_eq!(status.name, "In Progress");
        assert_eq!(status.id.as_str(), "3");
        assert_eq!(status.category, StatusCategoryKey::INDETERMINATE);
        assert_eq!(
            f.status.as_ref().unwrap().category.as_ref().unwrap().id,
            Some(4)
        );
        assert_eq!(f.issue_type.as_ref().unwrap().name, "Task");
        assert!(!f.issue_type.as_ref().unwrap().subtask);
        assert_eq!(f.project.as_ref().unwrap().key.as_str(), "OPS");
        assert_eq!(
            f.project.as_ref().unwrap().name.as_deref(),
            Some("Operations")
        );
        let assignee = f.assignee.as_ref().unwrap();
        assert_eq!(assignee.display_name.as_deref(), Some("Mia"));
        assert!(!format!("{assignee:?}").contains("mia@example.com"));
        assert_eq!(f.labels, ["alert", "autumn-idem-abc"]);
        assert_eq!(f.created.unwrap().epoch_ms(), 1_610_886_840_000);
        assert_eq!(f.updated.unwrap().epoch_ms(), 1_610_886_900_000);
        assert_eq!(f.description.as_ref().unwrap().to_plain_text(), "Body");
        let custom = FieldId::parse("customfield_10010").unwrap();
        assert_eq!(issue.field(&custom), Some(&json!({"value": "High"})));
        assert!(f.other.contains_key("priority"));
        assert!(!f.other.contains_key("summary"));
        assert_eq!(
            issue.property("autumn-jira"),
            Some(&json!({"idempotencyKey": "k1"}))
        );
        let summary = IssueSummary::from(&issue);
        assert_eq!(summary.key.as_str(), "lower-7");
        assert_eq!(summary.summary.as_deref(), Some("Disk full"));
        assert_eq!(summary.status, Some(status));
        assert_eq!(summary.labels, ["alert", "autumn-idem-abc"]);
        assert_eq!(summary.updated, f.updated);
    }

    #[test]
    fn sparse_and_odd_fields_are_lenient() {
        let issue: Issue = serde_json::from_value(json!({
            "id": "1", "key": "A-1",
            "fields": {
                "summary": null, "assignee": null, "labels": null, "description": null,
                "created": "not a time", "status": {"name": "no id"},
                "project": {"key": "P"}, "issuetype": {"name": "x"}
            }
        }))
        .unwrap();
        assert_eq!(issue.fields, IssueFields::default());
        assert!(issue.properties.is_empty());
        let bare: Issue = serde_json::from_value(json!({"id": "1", "key": "A-1"})).unwrap();
        assert_eq!(bare.fields, IssueFields::default());
        let bad_adf: Issue = serde_json::from_value(json!({
            "id": "1", "key": "A-1", "fields": {"description": "plain text"}
        }))
        .unwrap();
        assert_eq!(bad_adf.fields.description, None);
        let no_category: Status =
            serde_json::from_value(json!({"id": "1", "name": "New"})).unwrap();
        assert_eq!(no_category.summary().category.as_str(), "undefined");
        assert!(serde_json::from_value::<Issue>(json!({"key": "A-1"})).is_err());
    }

    #[test]
    fn other_models_decode() {
        let page: Page<Project> = serde_json::from_value(json!({
            "self": "x", "maxResults": 2, "startAt": 0, "total": 3, "isLast": false,
            "values": [
                {"id": "10000", "key": "OPS", "name": "Operations", "projectTypeKey": "software", "simplified": false},
                {"id": "10001", "key": "WEB", "name": "Web"}
            ]
        }))
        .unwrap();
        assert_eq!(page.values.len(), 2);
        assert_eq!(page.values[0].project_type_key.as_deref(), Some("software"));
        assert_eq!(page.values[1].simplified, None);
        assert_eq!(
            (page.start_at, page.total, page.is_last),
            (0, Some(3), false)
        );
        let last: Page<Project> = serde_json::from_value(json!({"values": []})).unwrap();
        assert_eq!((last.start_at, last.total, last.is_last), (0, None, true));

        let comments: CommentPage = serde_json::from_value(json!({
            "startAt": 0, "maxResults": 100, "total": 1,
            "comments": [{
                "id": "10100", "self": "x",
                "author": {"accountId": "712020:abc", "displayName": "Bot"},
                "body": {"type": "doc", "version": 1, "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "hi"}]}]},
                "created": "2021-01-17T12:34:00.000+0000"
            }]
        }))
        .unwrap();
        assert_eq!(comments.total, 1);
        let comment = &comments.comments[0];
        assert_eq!(comment.id.as_str(), "10100");
        assert_eq!(comment.body.as_ref().unwrap().to_plain_text(), "hi");
        assert_eq!(
            comment.author.as_ref().unwrap().account_id.as_str(),
            "712020:abc"
        );
        assert_eq!(comment.updated, None);

        let transition: Transition = serde_json::from_value(json!({
            "id": "2", "name": "Close Issue", "hasScreen": false, "isAvailable": true,
            "to": {"id": "10000", "name": "In Progress",
                "statusCategory": {"id": 1, "key": "in-flight", "name": "In Progress"}}
        }))
        .unwrap();
        assert_eq!(transition.id.as_str(), "2");
        assert_eq!(transition.to.name, "In Progress");
        assert_eq!(transition.to.category.as_str(), "in-flight");
        assert!(!transition.has_screen);

        let info: ServerInfo = serde_json::from_value(json!({
            "baseUrl": "https://acme.atlassian.net", "version": "1001.0.0", "deploymentType": "Cloud"
        }))
        .unwrap();
        assert_eq!(info.deployment_type.as_deref(), Some("Cloud"));
        assert_eq!(info.base_url.as_deref(), Some("https://acme.atlassian.net"));

        let resource: AccessibleResource = serde_json::from_value(json!({
            "id": "1324a887-45db-1bf4-1e99-ef0ff456d421", "name": "Site", "url": "https://acme.atlassian.net",
            "scopes": ["write:jira-work"], "avatarUrl": "x"
        }))
        .unwrap();
        assert_eq!(resource.scopes, ["write:jira-work"]);
        assert_eq!(resource.url, "https://acme.atlassian.net");

        let issue_type: IssueType =
            serde_json::from_value(json!({"id": "1", "name": "Bug", "subtask": true})).unwrap();
        assert!(issue_type.subtask);
        let project_ref: ProjectRef =
            serde_json::from_value(json!({"id": "1", "key": "OPS"})).unwrap();
        assert_eq!(project_ref.name, None);
        let created: CreatedIssue =
            serde_json::from_value(json!({"id": "2", "key": "OPS-2"})).unwrap();
        let refs = IssueRefs::from(created);
        assert_eq!(
            refs,
            IssueRefs::new(
                IssueId::parse("2").unwrap(),
                IssueKey::parse("OPS-2").unwrap()
            )
        );
    }

    #[test]
    fn host_values_round_trip() {
        let summary = IssueSummary {
            id: IssueId::parse("1").unwrap(),
            key: IssueKey::parse("OPS-1").unwrap(),
            summary: Some("s".to_owned()),
            status: Some(StatusSummary::new(
                StatusId::parse("3").unwrap(),
                "Done".to_owned(),
                StatusCategoryKey::DONE,
            )),
            labels: vec!["a".to_owned()],
            updated: JiraTimestamp::from_epoch_ms(1_000),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["status"]["category"], "done");
        assert_eq!(
            serde_json::from_value::<IssueSummary>(json).unwrap(),
            summary
        );
        let refs = IssueRefs::new(
            IssueId::parse("1").unwrap(),
            IssueKey::parse("OPS-1").unwrap(),
        );
        let json = serde_json::to_value(&refs).unwrap();
        assert_eq!(json, json!({"id": "1", "key": "OPS-1"}));
        assert_eq!(serde_json::from_value::<IssueRefs>(json).unwrap(), refs);
    }

    #[test]
    fn decode_errors_hold_no_body_value() {
        let body = br#"{"id": 12345, "key": "SECRET-TEXT"}"#;
        let err = decode::<Issue>(Op::GetIssue, body).unwrap_err();
        assert_eq!(err.code(), "decode");
        let text = format!("{err} {err:?}");
        assert!(!text.contains("12345"), "{text}");
        assert!(!text.contains("SECRET"), "{text}");
        assert!(text.contains("data"), "{text}");
        let syntax = decode::<Issue>(Op::GetIssue, b"<html>secret</html>").unwrap_err();
        assert!(!syntax.to_string().contains("secret"), "{syntax}");
        assert!(syntax.to_string().contains("syntax"), "{syntax}");
        let eof = decode::<Issue>(Op::GetIssue, b"").unwrap_err();
        assert!(eof.to_string().contains("eof"), "{eof}");
        let ok: Issue = decode(Op::GetIssue, br#"{"id": "1", "key": "A-1"}"#).unwrap();
        assert_eq!(ok.key.as_str(), "A-1");
    }
}

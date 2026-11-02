//! JSON replies in the Jira Cloud REST v3 shapes. The shapes follow the
//! spec schemas (`IssueBean`, `Comment`, `Project`, ...). The test
//! `fake_replies_match_the_spec_shapes` checks them.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::state::{Comment, Issue, State, jira_time};
use crate::types::{FakeIssueType, FakeProject};
use crate::workflow::{self, Status, Transition};

/// The `expand` text of an issue bean.
const ISSUE_EXPAND: &str =
    "renderedFields,names,schema,operations,editmeta,changelog,versionedRepresentations";

/// Returns an `ErrorCollection`.
pub(crate) fn errors(messages: &[&str], fields: &[(&str, &str)]) -> Value {
    let fields: Map<String, Value> = fields
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Value::from(*v)))
        .collect();
    json!({"errorMessages": messages, "errors": fields})
}

/// Returns an `ErrorCollection` from a field error map.
pub(crate) fn field_errors(fields: &Map<String, Value>) -> Value {
    json!({"errorMessages": [], "errors": fields})
}

/// Returns the avatar URLs of an entity.
fn avatars(kind: &str, id: &str) -> Value {
    let base = format!("https://avatar-management.example/{kind}/{id}");
    json!({
        "16x16": format!("{base}/16"),
        "24x24": format!("{base}/24"),
        "32x32": format!("{base}/32"),
        "48x48": format!("{base}/48"),
    })
}

/// Returns a user (`UserDetails`). An unknown account id gets its id as name.
pub(crate) fn user(state: &State, account_id: &str) -> Value {
    let known = workflow::user(account_id);
    let mut out = json!({
        "self": format!("{}/rest/api/3/user?accountId={account_id}", state.base()),
        "accountId": account_id,
        "avatarUrls": avatars("user", account_id),
        "displayName": known.map_or(account_id, |u| u.display_name),
        "active": true,
        "timeZone": "Etc/UTC",
        "accountType": "atlassian",
    });
    if let Some(u) = known {
        out["emailAddress"] = Value::from(u.email);
    }
    out
}

/// Returns the reply of `GET /myself` (`User`).
pub(crate) fn myself(state: &State) -> Value {
    let mut out = user(state, State::caller());
    out["locale"] = Value::from("en_US");
    out["expand"] = Value::from("groups,applicationRoles");
    out
}

/// Returns the reply of `GET /serverInfo` (`ServerInformation`).
pub(crate) fn server_info(state: &State) -> Value {
    let now = state.clock.now();
    json!({
        "baseUrl": state.base(),
        "displayUrl": state.base(),
        "version": "1001.0.0-SNAPSHOT",
        "versionNumbers": [1001, 0, 0],
        "deploymentType": "Cloud",
        "buildNumber": 100_300,
        "buildDate": jira_time(&now),
        "serverTime": jira_time(&now),
        "scmInfo": "fake",
        "serverTitle": "Jira",
        "serverTimeZone": "Etc/UTC",
    })
}

/// Returns a status (`StatusDetails`).
pub(crate) fn status(state: &State, status: Status) -> Value {
    let base = state.base();
    let category = status.category;
    json!({
        "self": format!("{base}/rest/api/3/status/{}", status.id),
        "description": "",
        "iconUrl": format!("{base}/images/icons/statuses/generic.png"),
        "name": status.name,
        "id": status.id,
        "statusCategory": {
            "self": format!("{base}/rest/api/3/statuscategory/{}", category.id),
            "id": category.id,
            "key": category.key,
            "colorName": category.color,
            "name": category.name,
        },
    })
}

/// Returns an issue type (`IssueTypeDetails`).
pub(crate) fn issue_type(state: &State, issue_type: &FakeIssueType) -> Value {
    let base = state.base();
    json!({
        "self": format!("{base}/rest/api/3/issuetype/{}", issue_type.id),
        "id": issue_type.id,
        "description": format!("A {} issue.", issue_type.name),
        "iconUrl": format!("{base}/images/icons/issuetypes/{}.png", issue_type.id),
        "name": issue_type.name,
        "subtask": false,
        "avatarId": 10_318,
        "hierarchyLevel": 0,
    })
}

/// Returns an issue type for create (`IssueTypeIssueCreateMetadata`).
pub(crate) fn create_meta_type(state: &State, issue_type_value: &FakeIssueType) -> Value {
    let mut out = issue_type(state, issue_type_value);
    out["expand"] = Value::from("fields");
    out
}

/// Returns a project (`Project`). The lead comes only with `expand=lead`.
pub(crate) fn project(state: &State, project: &FakeProject, lead: bool) -> Value {
    let mut out = json!({
        "expand": "description,lead,issueTypes,url,projectKeys,permissions,insight",
        "self": format!("{}/rest/api/3/project/{}", state.base(), project.id),
        "id": project.id,
        "key": project.key,
        "name": project.name,
        "description": "",
        "avatarUrls": avatars("project", &project.id),
        "projectTypeKey": "software",
        "simplified": false,
        "style": "classic",
        "isPrivate": false,
        "properties": {},
        "issueTypes": project.issue_types.iter().map(|t| issue_type(state, t)).collect::<Vec<_>>(),
    });
    if lead {
        out["lead"] = user(state, &project.lead);
    }
    out
}

/// Returns the short project of an issue field.
fn project_ref(state: &State, key: &str) -> Value {
    state.project(key).map_or(Value::Null, |p| {
        json!({
            "self": format!("{}/rest/api/3/project/{}", state.base(), p.id),
            "id": p.id,
            "key": p.key,
            "name": p.name,
            "projectTypeKey": "software",
            "simplified": false,
            "avatarUrls": avatars("project", &p.id),
        })
    })
}

/// Returns a priority field value.
fn priority(state: &State, id: &str) -> Value {
    let (id, name) = workflow::priority(id);
    let base = state.base();
    json!({
        "self": format!("{base}/rest/api/3/priority/{id}"),
        "iconUrl": format!("{base}/images/icons/priorities/{}.svg", name.to_ascii_lowercase()),
        "name": name,
        "id": id,
    })
}

/// Returns a resolution field value, or null.
fn resolution(state: &State, id: Option<&str>) -> Value {
    id.and_then(workflow::resolution)
        .map_or(Value::Null, |(id, name, description)| {
            json!({
                "self": format!("{}/rest/api/3/resolution/{id}", state.base()),
                "id": id,
                "description": description,
                "name": name,
            })
        })
}

/// Returns a comment (`Comment`).
pub(crate) fn comment(state: &State, issue: &Issue, comment: &Comment) -> Value {
    json!({
        "self": format!("{}/rest/api/3/issue/{}/comment/{}", state.base(), issue.id, comment.id),
        "id": comment.id.to_string(),
        "author": user(state, &comment.author),
        "body": comment.body,
        "updateAuthor": user(state, &comment.author),
        "created": jira_time(&comment.created),
        "updated": jira_time(&comment.updated),
        "jsdPublic": true,
    })
}

/// Returns a page of comments (`PageOfComments`).
pub(crate) fn comment_page(
    state: &State,
    issue: &Issue,
    comments: &[&Comment],
    start_at: usize,
    max_results: usize,
    total: usize,
) -> Value {
    json!({
        "comments": comments.iter().map(|c| comment(state, issue, c)).collect::<Vec<_>>(),
        "startAt": start_at,
        "maxResults": max_results,
        "total": total,
    })
}

/// Which fields an issue bean holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FieldFilter {
    all: bool,
    include: BTreeSet<String>,
    exclude: BTreeSet<String>,
}

impl FieldFilter {
    /// Every field.
    pub(crate) fn all() -> Self {
        Self {
            all: true,
            ..Self::default()
        }
    }

    /// Parses a `fields` list. An empty list gives `default`.
    pub(crate) fn parse(items: &[String], default: Self) -> Self {
        if items.is_empty() {
            return default;
        }
        let mut filter = Self::default();
        for item in items {
            match item.as_str() {
                "*all" | "*navigable" => filter.all = true,
                other => match other.strip_prefix('-') {
                    Some(name) => {
                        filter.exclude.insert(name.to_owned());
                    }
                    None => {
                        filter.include.insert(other.to_owned());
                    }
                },
            }
        }
        if filter.include.is_empty() && !filter.exclude.is_empty() {
            filter.all = true;
        }
        filter
    }

    /// Returns `true` when the bean holds this field.
    fn keeps(&self, name: &str) -> bool {
        !self.exclude.contains(name) && (self.all || self.include.contains(name))
    }

    /// Returns `true` when the filter asks for `id` and `key` only.
    pub(crate) fn ids_only(&self) -> bool {
        !self.all && self.include.iter().all(|f| f == "id" || f == "key")
    }
}

/// Returns every field of an issue.
pub(crate) fn issue_fields(state: &State, issue: &Issue) -> Map<String, Value> {
    let mut fields = Map::new();
    let comments: Vec<&Comment> = issue.comments.iter().collect();
    let total = comments.len();
    let mut put = |name: &str, value: Value| {
        fields.insert(name.to_owned(), value);
    };
    put("summary", Value::from(issue.summary.clone()));
    put(
        "description",
        issue.description.clone().unwrap_or(Value::Null),
    );
    put("status", status(state, issue.status()));
    put("issuetype", issue_type(state, &state.issue_type_of(issue)));
    put("project", project_ref(state, &issue.project));
    put(
        "assignee",
        issue
            .assignee
            .as_deref()
            .map_or(Value::Null, |a| user(state, a)),
    );
    put("reporter", user(state, &issue.reporter));
    put("creator", user(state, &issue.reporter));
    put("priority", priority(state, issue.priority));
    put(
        "labels",
        Value::from(issue.labels.iter().cloned().collect::<Vec<_>>()),
    );
    put("resolution", resolution(state, issue.resolution));
    put(
        "resolutiondate",
        issue
            .resolution_date
            .as_ref()
            .map_or(Value::Null, |t| Value::from(jira_time(t))),
    );
    put("created", Value::from(jira_time(&issue.created)));
    put("updated", Value::from(jira_time(&issue.updated)));
    put(
        "statuscategorychangedate",
        Value::from(jira_time(&issue.status_changed)),
    );
    put(
        "comment",
        comment_page(state, issue, &comments, 0, total.max(1), total),
    );
    for (name, value) in &issue.custom {
        put(name, value.clone());
    }
    fields
}

/// What an issue bean holds besides the fields.
#[derive(Debug, Clone, Default)]
pub(crate) struct BeanParts {
    pub(crate) fields: FieldFilter,
    /// `None`: no `properties` key. `Some`: these keys, when set.
    pub(crate) properties: Option<Vec<String>>,
    pub(crate) transitions: bool,
}

/// Returns an issue bean (`IssueBean`).
pub(crate) fn issue_bean(state: &State, issue: &Issue, parts: &BeanParts) -> Value {
    let fields: Map<String, Value> = issue_fields(state, issue)
        .into_iter()
        .filter(|(name, _)| parts.fields.keeps(name))
        .collect();
    let mut out = json!({
        "expand": ISSUE_EXPAND,
        "id": issue.id.to_string(),
        "self": format!("{}/rest/api/3/issue/{}", state.base(), issue.id),
        "key": issue.key,
        "fields": fields,
    });
    if let Some(keys) = &parts.properties {
        let props: Map<String, Value> = keys
            .iter()
            .filter_map(|k| issue.properties.get(k).map(|v| (k.clone(), v.clone())))
            .collect();
        out["properties"] = Value::Object(props);
    }
    if parts.transitions {
        out["transitions"] = Value::from(transitions(state, issue, false, None));
    }
    out
}

/// Returns the full issue bean with every property.
pub(crate) fn full_issue(state: &State, issue: &Issue) -> Value {
    let parts = BeanParts {
        fields: FieldFilter::all(),
        properties: Some(issue.properties.keys().cloned().collect()),
        transitions: false,
    };
    issue_bean(state, issue, &parts)
}

/// Returns one transition (`IssueTransition`).
fn transition(state: &State, t: Transition, with_fields: bool) -> Value {
    let mut out = json!({
        "id": t.id,
        "name": t.name,
        "to": status(state, workflow::status(t.to)),
        "hasScreen": t.needs_resolution,
        "isGlobal": false,
        "isInitial": false,
        "isAvailable": true,
        "isConditional": false,
        "looped": false,
    });
    if with_fields {
        let mut fields = Map::new();
        if t.needs_resolution {
            let values: Vec<Value> = workflow::RESOLUTIONS
                .iter()
                .map(|(id, name, _)| json!({"id": id, "name": name}))
                .collect();
            fields.insert(
                "resolution".to_owned(),
                json!({
                    "required": true,
                    "schema": {"type": "resolution", "system": "resolution"},
                    "name": "Resolution",
                    "key": "resolution",
                    "hasDefaultValue": false,
                    "operations": ["set"],
                    "allowedValues": values,
                }),
            );
        }
        out["fields"] = Value::Object(fields);
    }
    out
}

/// Returns the transitions out of the status of an issue.
pub(crate) fn transitions(
    state: &State,
    issue: &Issue,
    with_fields: bool,
    only: Option<&str>,
) -> Vec<Value> {
    workflow::transitions_from(issue.status)
        .filter(|t| only.is_none_or(|id| t.id == id))
        .map(|t| transition(state, t, with_fields))
        .collect()
}

/// Returns the plain text of an ADF node. Blocks join with a newline.
pub(crate) fn adf_text(node: &Value) -> String {
    text_with(node, "\n")
}

/// Returns the REST v2 text of an ADF node, as in a webhook payload.
/// Blocks join with a blank line, as in wiki markup. The fake does not
/// write wiki markup for marks.
pub(crate) fn wiki_text(node: &Value) -> String {
    text_with(node, "\n\n")
}

/// Returns the text of an ADF node. Blocks join with `block`.
fn text_with(node: &Value, block: &str) -> String {
    match node.get("type").and_then(Value::as_str) {
        Some("text") => node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Some("hardBreak") => "\n".to_owned(),
        _ => {
            let children = node
                .get("content")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let inline = children.iter().all(|c| {
                matches!(
                    c.get("type").and_then(Value::as_str),
                    Some("text" | "hardBreak" | "mention" | "emoji" | "inlineCard")
                )
            });
            let parts = children.iter().map(|c| text_with(c, block));
            if inline {
                parts.collect()
            } else {
                parts.collect::<Vec<_>>().join(block)
            }
        }
    }
}

/// Returns an ADF document with one paragraph for each line.
pub(crate) fn adf_from_text(text: &str) -> Value {
    let content: Vec<Value> = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                json!({"type": "paragraph", "content": []})
            } else {
                json!({"type": "paragraph", "content": [{"type": "text", "text": line}]})
            }
        })
        .collect();
    json!({"type": "doc", "version": 1, "content": content})
}

/// Returns `true` for an ADF document root.
pub(crate) fn is_adf(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("doc")
        && value.get("content").is_some_and(Value::is_array)
}

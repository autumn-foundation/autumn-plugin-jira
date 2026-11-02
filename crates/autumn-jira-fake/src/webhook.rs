//! Signed webhook deliveries in the shape of the Jira webhook page.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::render;
use crate::sign::sign;
use crate::state::{Comment, Issue, State};
use crate::types::FakeDelivery;
use crate::workflow;

/// The signature header.
pub(crate) const SIGNATURE: &str = "X-Hub-Signature";
/// The delivery id header.
pub(crate) const IDENTIFIER: &str = "X-Atlassian-Webhook-Identifier";
/// The flow header.
const FLOW: &str = "X-Atlassian-Webhook-Flow";

/// One changelog item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) field: String,
    pub(crate) from: Option<String>,
    pub(crate) from_string: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) to_string: Option<String>,
}

impl Change {
    fn json(&self) -> Value {
        json!({
            "field": self.field,
            "fieldtype": if self.field.starts_with("customfield_") { "custom" } else { "jira" },
            "fieldId": self.field,
            "from": self.from,
            "fromString": self.from_string,
            "to": self.to,
            "toString": self.to_string,
        })
    }
}

/// One change event.
#[derive(Debug, Clone)]
pub(crate) enum Event<'a> {
    Created,
    Updated {
        type_name: &'a str,
        changes: Vec<Change>,
    },
    Deleted,
    CommentCreated(&'a Comment),
}

/// Queues a signed delivery when the site has a webhook secret.
pub(crate) fn emit(state: &mut State, event: &Event<'_>, issue: &Issue, actor: &str) {
    let Some(secret) = state.webhook_secret.clone() else {
        return;
    };
    let timestamp = state.clock.now().timestamp_millis();
    let body = match event {
        Event::CommentCreated(comment) => json!({
            "timestamp": timestamp,
            "webhookEvent": "comment_created",
            "comment": render::comment(state, issue, comment),
            "issue": reduced_issue(state, issue),
        }),
        Event::Created | Event::Updated { .. } | Event::Deleted => {
            let (name, type_name) = match event {
                Event::Created => ("jira:issue_created", "issue_created"),
                Event::Deleted => ("jira:issue_deleted", "issue_deleted"),
                Event::Updated { type_name, .. } => ("jira:issue_updated", *type_name),
                Event::CommentCreated(_) => ("", ""),
            };
            let parts = render::BeanParts {
                fields: render::FieldFilter::all(),
                ..render::BeanParts::default()
            };
            let mut body = json!({
                "timestamp": timestamp,
                "webhookEvent": name,
                "issue_event_type_name": type_name,
                "user": render::user(state, actor),
                "issue": render::issue_bean(state, issue, &parts),
            });
            if let Event::Updated { changes, .. } = event {
                let id = state.next_change_id;
                state.next_change_id += 1;
                body["changelog"] = json!({
                    "id": id.to_string(),
                    "items": changes.iter().map(Change::json).collect::<Vec<_>>(),
                });
            }
            body
        }
    };
    let id = format!("{}-{}", state.site, state.next_delivery);
    state.next_delivery += 1;
    state
        .deliveries
        .push(build(&secret, &id, body.to_string().into_bytes()));
}

/// Returns the reduced issue of a comment event.
fn reduced_issue(state: &State, issue: &Issue) -> Value {
    let fields = render::issue_fields(state, issue);
    let keep = [
        "summary",
        "status",
        "issuetype",
        "project",
        "assignee",
        "priority",
    ];
    let reduced: serde_json::Map<String, Value> = fields
        .into_iter()
        .filter(|(name, _)| keep.contains(&name.as_str()))
        .collect();
    json!({
        "id": issue.id.to_string(),
        "self": format!("{}/rest/api/3/issue/{}", state.base(), issue.id),
        "key": issue.key,
        "fields": reduced,
    })
}

/// Builds a signed delivery.
pub(crate) fn build(secret: &Arc<[u8]>, id: &str, body: Vec<u8>) -> FakeDelivery {
    FakeDelivery {
        headers: vec![
            (
                "Content-Type".to_owned(),
                "application/json; charset=utf-8".to_owned(),
            ),
            (SIGNATURE.to_owned(), sign(secret, &body)),
            (IDENTIFIER.to_owned(), id.to_owned()),
            (FLOW.to_owned(), "Primary".to_owned()),
        ],
        body,
        secret: Arc::clone(secret),
    }
}

/// Returns a short user object for a changed actor.
pub(crate) fn short_user(account_id: &str) -> Value {
    json!({
        "accountId": account_id,
        "accountType": "atlassian",
        "displayName": workflow::user(account_id).map_or(account_id, |u| u.display_name),
        "active": true,
    })
}

/// Returns the changelog items from `old` to `new`.
pub(crate) fn diff(old: &Issue, new: &Issue) -> Vec<Change> {
    let mut out = Vec::new();
    let mut push = |field: &str,
                    from: Option<String>,
                    from_string: Option<String>,
                    to: Option<String>,
                    to_string: Option<String>| {
        out.push(Change {
            field: field.to_owned(),
            from,
            from_string,
            to,
            to_string,
        });
    };
    if old.status != new.status {
        push(
            "status",
            Some(old.status.to_owned()),
            Some(old.status().name.to_owned()),
            Some(new.status.to_owned()),
            Some(new.status().name.to_owned()),
        );
    }
    if old.resolution != new.resolution {
        let name = |id: Option<&str>| id.and_then(workflow::resolution).map(|r| r.1.to_owned());
        push(
            "resolution",
            old.resolution.map(str::to_owned),
            name(old.resolution),
            new.resolution.map(str::to_owned),
            name(new.resolution),
        );
    }
    if old.summary != new.summary {
        push(
            "summary",
            None,
            Some(old.summary.clone()),
            None,
            Some(new.summary.clone()),
        );
    }
    if old.description != new.description {
        let text = |d: Option<&Value>| d.map(render::adf_text);
        push(
            "description",
            None,
            text(old.description.as_ref()),
            None,
            text(new.description.as_ref()),
        );
    }
    if old.labels != new.labels {
        let joined = |l: &std::collections::BTreeSet<String>| {
            Some(l.iter().cloned().collect::<Vec<_>>().join(" "))
        };
        push(
            "labels",
            None,
            joined(&old.labels),
            None,
            joined(&new.labels),
        );
    }
    if old.assignee != new.assignee {
        let name = |a: Option<&String>| {
            a.map(|a| workflow::user(a).map_or_else(|| a.clone(), |u| u.display_name.to_owned()))
        };
        push(
            "assignee",
            old.assignee.clone(),
            name(old.assignee.as_ref()),
            new.assignee.clone(),
            name(new.assignee.as_ref()),
        );
    }
    if old.priority != new.priority {
        push(
            "priority",
            Some(old.priority.to_owned()),
            Some(workflow::priority(old.priority).1.to_owned()),
            Some(new.priority.to_owned()),
            Some(workflow::priority(new.priority).1.to_owned()),
        );
    }
    let names: std::collections::BTreeSet<&String> =
        old.custom.keys().chain(new.custom.keys()).collect();
    for name in names {
        let (a, b) = (old.custom.get(name), new.custom.get(name));
        if a != b {
            push(
                name,
                None,
                a.map(Value::to_string),
                None,
                b.map(Value::to_string),
            );
        }
    }
    out
}

//! Webhook types, the payload parser and the signature check.
//!
//! Jira signs a delivery with HMAC-SHA256 of the raw body:
//! `X-Hub-Signature: sha256=<lower hex>`. Deliveries are at least once: the
//! same delivery id can come again, so a handler must be idempotent.

use std::fmt;

use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;

use crate::adf::Adf;
use crate::error::{JiraError, Result};
use crate::ids::{
    AccountId, CommentId, IssueId, IssueKey, ProjectKey, StatusId, WebhookDeliveryId,
};
use crate::model::StatusSummary;
use crate::model::wire::StatusWire;
use crate::policy;
use crate::time::JiraTimestamp;

/// The signature header: `sha256=<64 lower hex>`.
pub const SIGNATURE_HEADER: &str = "X-Hub-Signature";
/// The delivery id header. It is the same on each retry of a delivery.
pub const DELIVERY_HEADER: &str = "X-Atlassian-Webhook-Identifier";
/// The retry count header.
pub const RETRY_HEADER: &str = "X-Atlassian-Webhook-Retry";
/// The flow header: `Primary` or `Secondary`.
pub const FLOW_HEADER: &str = "X-Atlassian-Webhook-Flow";

/// The text that marks a comment of this client (the loop guard).
const COMMENT_MARKER: &str = "autumn-ref: ";
/// The method part of the signature header.
const SIGNATURE_PREFIX: &str = "sha256=";

const ISSUE_CREATED: &str = "jira:issue_created";
const ISSUE_UPDATED: &str = "jira:issue_updated";
const ISSUE_DELETED: &str = "jira:issue_deleted";
const COMMENT_CREATED: &str = "comment_created";
const COMMENT_UPDATED: &str = "comment_updated";
const COMMENT_DELETED: &str = "comment_deleted";

type HmacSha256 = Hmac<Sha256>;

/// One verified delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The headers of the delivery.
    pub meta: DeliveryMeta,
    /// The event.
    pub event: JiraEvent,
    /// The `timestamp` of the payload.
    pub timestamp: Option<JiraTimestamp>,
    /// The account id of `user`: the actor.
    pub actor: Option<AccountId>,
}

impl Delivery {
    /// Makes a delivery from its headers and its parsed body.
    #[must_use]
    pub fn new(meta: DeliveryMeta, parsed: ParsedEvent) -> Self {
        Self {
            meta,
            event: parsed.event,
            timestamp: parsed.timestamp,
            actor: parsed.actor,
        }
    }
}

/// The headers of one delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryMeta {
    /// `X-Atlassian-Webhook-Identifier`.
    pub id: WebhookDeliveryId,
    /// `X-Atlassian-Webhook-Retry`. 0 on the first delivery.
    pub retry: u32,
    /// `X-Atlassian-Webhook-Flow`.
    pub flow: Flow,
}

impl DeliveryMeta {
    /// Makes the headers.
    #[must_use]
    pub const fn new(id: WebhookDeliveryId, retry: u32, flow: Flow) -> Self {
        Self { id, retry, flow }
    }
}

/// The delivery flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Jira sends it within about 30 seconds.
    Primary,
    /// A bulk or cascade event. Jira sends it within about 15 minutes.
    Secondary,
    /// No header, or an unknown value.
    Unknown,
}

impl Flow {
    /// Parses the flow header. The match ignores ASCII case.
    #[must_use]
    pub const fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("primary") {
            Self::Primary
        } else if value.eq_ignore_ascii_case("secondary") {
            Self::Secondary
        } else {
            Self::Unknown
        }
    }
}

/// A Jira webhook event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum JiraEvent {
    /// `jira:issue_created`.
    IssueCreated(IssueEvent),
    /// `jira:issue_updated`.
    IssueUpdated(IssueEvent),
    /// `jira:issue_deleted`.
    IssueDeleted(IssueEvent),
    /// `comment_created`.
    CommentCreated(CommentEvent),
    /// `comment_updated`.
    CommentUpdated(CommentEvent),
    /// `comment_deleted`.
    CommentDeleted(CommentEvent),
    /// Another `webhookEvent`. The plugin does not dispatch it (F39).
    Other {
        /// The `webhookEvent` value.
        name: String,
    },
}

/// An issue event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEvent {
    /// The issue.
    pub issue: WebhookIssue,
    /// The items of `changelog`. Empty when the payload has none.
    pub changelog: Vec<ChangeItem>,
    /// `issue_event_type_name`, as free text.
    pub type_name: Option<String>,
}

/// A comment event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentEvent {
    /// The issue. Comment payloads carry a reduced issue.
    pub issue: WebhookIssue,
    /// The comment.
    pub comment: WebhookComment,
}

/// The issue of a payload. Lenient: comment payloads carry a reduced issue
/// (B12). Only `id` is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookIssue {
    /// The numeric id.
    pub id: IssueId,
    /// The key.
    pub key: Option<IssueKey>,
    /// The fields in the payload.
    pub fields: WebhookFields,
}

/// The fields of a payload issue. Each is `None` when the payload has none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebhookFields {
    /// `summary`.
    pub summary: Option<String>,
    /// `status`.
    pub status: Option<StatusSummary>,
    /// `labels`.
    pub labels: Option<Vec<String>>,
    /// `project.key`.
    pub project: Option<ProjectKey>,
    /// `issuetype.name`.
    pub issue_type: Option<String>,
}

/// The comment of a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookComment {
    /// The numeric id.
    pub id: CommentId,
    /// The body.
    pub body: Option<Adf>,
    /// The account id of the author.
    pub author: Option<AccountId>,
}

/// One changelog item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeItem {
    /// The field name, for example `status`.
    pub field: String,
    /// The old raw value, for example a status id.
    pub from: Option<String>,
    /// The new raw value.
    pub to: Option<String>,
    /// The old display value, for example a status name.
    pub from_string: Option<String>,
    /// The new display value.
    pub to_string: Option<String>,
}

/// One side of a status change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRef {
    /// The status id.
    pub id: Option<StatusId>,
    /// The status name.
    pub name: Option<String>,
}

/// A status change from the changelog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChange {
    /// The old status.
    pub from: StatusRef,
    /// The new status.
    pub to: StatusRef,
}

impl JiraEvent {
    /// Returns the `webhookEvent` name, for example `jira:issue_updated`.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::IssueCreated(_) => ISSUE_CREATED,
            Self::IssueUpdated(_) => ISSUE_UPDATED,
            Self::IssueDeleted(_) => ISSUE_DELETED,
            Self::CommentCreated(_) => COMMENT_CREATED,
            Self::CommentUpdated(_) => COMMENT_UPDATED,
            Self::CommentDeleted(_) => COMMENT_DELETED,
            Self::Other { name } => name,
        }
    }

    /// Returns the issue of the event.
    #[must_use]
    pub const fn issue(&self) -> Option<&WebhookIssue> {
        match self {
            Self::IssueCreated(e) | Self::IssueUpdated(e) | Self::IssueDeleted(e) => Some(&e.issue),
            Self::CommentCreated(e) | Self::CommentUpdated(e) | Self::CommentDeleted(e) => {
                Some(&e.issue)
            }
            Self::Other { .. } => None,
        }
    }

    /// Returns the status change of an issue event, from the changelog item
    /// with field `status`.
    #[must_use]
    pub fn status_change(&self) -> Option<StatusChange> {
        let (Self::IssueCreated(e) | Self::IssueUpdated(e) | Self::IssueDeleted(e)) = self else {
            return None;
        };
        let item = e
            .changelog
            .iter()
            .find(|i| i.field.eq_ignore_ascii_case("status"))?;
        Some(StatusChange {
            from: status_ref(item.from.as_deref(), item.from_string.as_ref()),
            to: status_ref(item.to.as_deref(), item.to_string.as_ref()),
        })
    }

    /// Returns `true` when the comment holds `autumn-ref: `: the client
    /// wrote it (loop guard, B9).
    #[must_use]
    pub fn has_comment_marker(&self) -> bool {
        match self {
            Self::CommentCreated(e) | Self::CommentUpdated(e) | Self::CommentDeleted(e) => e
                .comment
                .body
                .as_ref()
                .is_some_and(|b| b.contains_text(COMMENT_MARKER)),
            _ => false,
        }
    }
}

impl fmt::Display for JiraEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A parsed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    /// The event.
    pub event: JiraEvent,
    /// `timestamp`: epoch ms.
    pub timestamp: Option<JiraTimestamp>,
    /// `user.accountId`.
    pub actor: Option<AccountId>,
}

/// Parses a payload. Call it after the signature check. It accepts
/// `changelog.id` as a string or a number (F40), and it ignores unknown keys.
///
/// # Errors
///
/// `JiraError::InvalidInput` for a body that is not JSON, has no
/// `webhookEvent`, or has no issue id on an issue or comment event. The
/// error never holds a value from the body.
pub fn parse_event(body: &[u8]) -> Result<ParsedEvent> {
    let payload: PayloadWire = serde_json::from_slice(body)
        .map_err(|_| bad_body("must be a JSON object with a webhookEvent string"))?;
    let name = payload
        .webhook_event
        .ok_or_else(|| bad_body("must have a webhookEvent string"))?;
    let timestamp = payload.timestamp.as_ref().and_then(|t| match t {
        Value::Number(n) => n.as_i64().and_then(JiraTimestamp::from_epoch_ms),
        Value::String(text) => JiraTimestamp::parse(text).ok(),
        _ => None,
    });
    let actor = account(payload.user.as_ref());
    let event = match name.as_str() {
        ISSUE_CREATED | ISSUE_UPDATED | ISSUE_DELETED => {
            let event = IssueEvent {
                issue: webhook_issue(payload.issue.as_ref())?,
                changelog: changelog(payload.changelog.as_ref()),
                type_name: payload.issue_event_type_name,
            };
            match name.as_str() {
                ISSUE_CREATED => JiraEvent::IssueCreated(event),
                ISSUE_UPDATED => JiraEvent::IssueUpdated(event),
                _ => JiraEvent::IssueDeleted(event),
            }
        }
        COMMENT_CREATED | COMMENT_UPDATED | COMMENT_DELETED => {
            let event = CommentEvent {
                issue: webhook_issue(payload.issue.as_ref())?,
                comment: webhook_comment(payload.comment.as_ref())?,
            };
            match name.as_str() {
                COMMENT_CREATED => JiraEvent::CommentCreated(event),
                COMMENT_UPDATED => JiraEvent::CommentUpdated(event),
                _ => JiraEvent::CommentDeleted(event),
            }
        }
        _ => JiraEvent::Other { name },
    };
    Ok(ParsedEvent {
        event,
        timestamp,
        actor,
    })
}

/// A parse error. It never holds a value from the body.
fn bad_body(reason: &'static str) -> JiraError {
    JiraError::invalid("webhook body", reason)
}

/// One side of a status change.
fn status_ref(id: Option<&str>, name: Option<&String>) -> StatusRef {
    StatusRef {
        id: id.and_then(|t| StatusId::parse(t).ok()),
        name: name.cloned(),
    }
}

/// Returns a non-empty string, or a number as text. Other values give `None`.
fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Returns `accountId` of a user object.
fn account(user: Option<&Value>) -> Option<AccountId> {
    text(user?.get("accountId")).map(AccountId::from_response)
}

/// Reads the payload issue. Only a numeric `id` is required.
fn webhook_issue(issue: Option<&Value>) -> Result<WebhookIssue> {
    let issue = issue.ok_or_else(|| bad_body("an issue event needs an issue"))?;
    let id = text(issue.get("id"))
        .and_then(|id| IssueId::parse(&id).ok())
        .ok_or_else(|| bad_body("the issue needs a numeric id"))?;
    let fields = issue.get("fields");
    let field = |name: &str| fields.and_then(|f| f.get(name));
    let status = field("status")
        .and_then(|s| StatusWire::deserialize(s).ok())
        .and_then(StatusWire::into_summary);
    let labels = field("labels").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    });
    Ok(WebhookIssue {
        id,
        key: text(issue.get("key")).map(IssueKey::from_response),
        fields: WebhookFields {
            summary: field("summary").and_then(Value::as_str).map(str::to_owned),
            status,
            labels,
            project: text(field("project").and_then(|p| p.get("key")))
                .map(ProjectKey::from_response),
            issue_type: text(field("issuetype").and_then(|t| t.get("name"))),
        },
    })
}

/// Reads the payload comment. A numeric `id` is required.
fn webhook_comment(comment: Option<&Value>) -> Result<WebhookComment> {
    let comment = comment.ok_or_else(|| bad_body("a comment event needs a comment"))?;
    let id = text(comment.get("id"))
        .and_then(|id| CommentId::parse(&id).ok())
        .ok_or_else(|| bad_body("the comment needs a numeric id"))?;
    Ok(WebhookComment {
        id,
        body: comment
            .get("body")
            .and_then(|b| Adf::from_value(b.clone()).ok()),
        author: account(comment.get("author")),
    })
}

/// Reads `changelog.items`. The `changelog.id` can be a string or a number.
/// The parser does not read it (F40).
fn changelog(changelog: Option<&Value>) -> Vec<ChangeItem> {
    let Some(items) = changelog
        .and_then(|c| c.get("items"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| ChangeItem {
            field: text(item.get("field")).unwrap_or_default(),
            from: text(item.get("from")),
            to: text(item.get("to")),
            from_string: text(item.get("fromString")),
            to_string: text(item.get("toString")),
        })
        .collect()
}

/// Checks `X-Hub-Signature` in constant time (`Mac::verify_slice`).
///
/// It checks the header shape first with `policy::signature_shape_ok`. For
/// apps that do not use autumn.
#[must_use]
pub fn verify_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    if !policy::signature_shape_ok(header.as_bytes()) {
        return false;
    }
    let Some(Ok(expected)) = header
        .as_bytes()
        .get(SIGNATURE_PREFIX.len()..)
        .map(|hex| data_encoding::HEXLOWER.decode(hex))
    else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    // `verify_slice` compares in constant time. Never compare MAC bytes with `==`.
    mac.verify_slice(&expected).is_ok()
}

/// Returns the signature header value: `sha256=<lower hex>`.
#[must_use]
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    // HMAC takes a key of any length, so `new_from_slice` does not fail.
    HmacSha256::new_from_slice(secret).map_or_else(
        |_| String::new(),
        |mut mac| {
            mac.update(body);
            format!(
                "{SIGNATURE_PREFIX}{}",
                data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes())
            )
        },
    )
}

/// The payload format.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PayloadWire {
    #[serde(default)]
    webhook_event: Option<String>,
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    user: Option<Value>,
    #[serde(default)]
    issue: Option<Value>,
    #[serde(default)]
    comment: Option<Value>,
    #[serde(default)]
    changelog: Option<Value>,
    #[serde(default, rename = "issue_event_type_name")]
    issue_event_type_name: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use serde_json::json;

    /// The Atlassian test vector (verified with openssl).
    const VECTOR_SECRET: &[u8] = b"It's a Secret to Everybody";
    const VECTOR_BODY: &[u8] = b"Hello World!";
    const VECTOR_HEADER: &str =
        "sha256=a4771c39fbe90f317c7824e83ddef3caae9cb3d976c214ace1f2937e133263c9";

    fn issue_payload(event: &str) -> Value {
        json!({
            "timestamp": 1_606_480_436_302_i64,
            "webhookEvent": event,
            "issue_event_type_name": "issue_generic",
            "user": {"accountId": "5b10a2844c20165700ede21g", "accountType": "atlassian", "displayName": "Mia"},
            "issue": {"id": "99291", "key": "OPS-7", "self": "x", "fields": {
                "summary": "Disk full",
                "status": {"id": "3", "name": "In Progress", "statusCategory": {"id": 4, "key": "indeterminate"}},
                "labels": ["autumn-tracked"],
                "project": {"id": "10000", "key": "OPS"},
                "issuetype": {"id": "1", "name": "Task"}
            }},
            "changelog": {"id": "10124", "items": [
                {"field": "summary", "fieldtype": "jira", "fromString": "a", "toString": "b"},
                {"field": "status", "fieldtype": "jira", "from": "1", "fromString": "To Do", "to": "3", "toString": "In Progress"}
            ]},
            "matchedWebhookIds": [1],
            "unknownKey": {"deep": true}
        })
    }

    fn comment_payload(event: &str, text: &str) -> Value {
        json!({
            "timestamp": 1_606_480_436_302_i64,
            "webhookEvent": event,
            "comment": {
                "id": "10100", "self": "x",
                "author": {"accountId": "712020:bot"},
                "body": {"type": "doc", "version": 1, "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": text}]}]}
            },
            "issue": {"id": "99291", "key": "OPS-7", "fields": {"summary": "Disk full"}}
        })
    }

    fn parse(value: &Value) -> ParsedEvent {
        parse_event(&serde_json::to_vec(value).unwrap()).unwrap()
    }

    #[test]
    fn atlassian_test_vector() {
        assert_eq!(sign(VECTOR_SECRET, VECTOR_BODY), VECTOR_HEADER);
        assert!(verify_signature(VECTOR_SECRET, VECTOR_BODY, VECTOR_HEADER));
        assert!(!verify_signature(
            b"another secret",
            VECTOR_BODY,
            VECTOR_HEADER
        ));
        assert!(!verify_signature(
            VECTOR_SECRET,
            b"Hello World?",
            VECTOR_HEADER
        ));
        assert!(!verify_signature(b"", VECTOR_BODY, VECTOR_HEADER));
    }

    #[test]
    fn rejects_bad_shapes() {
        let hex = &VECTOR_HEADER[7..];
        for bad in [
            format!("sha1={hex}"),
            format!("SHA256={hex}"),
            format!("sha512={hex}"),
            format!("sha256={}", hex.to_uppercase()),
            format!("sha256={}", &hex[..63]),
            format!("sha256={hex}0"),
            format!("sha256= {}", &hex[1..]),
            format!(" sha256={hex}"),
            hex.to_owned(),
            String::new(),
            "sha256=".to_owned(),
            format!("sha256={}g", &hex[..63]),
        ] {
            assert!(
                !verify_signature(VECTOR_SECRET, VECTOR_BODY, &bad),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn parses_all_events() {
        for (name, created) in [
            ("jira:issue_created", true),
            ("jira:issue_updated", false),
            ("jira:issue_deleted", false),
        ] {
            let parsed = parse(&issue_payload(name));
            assert_eq!(parsed.event.name(), name);
            assert_eq!(parsed.event.to_string(), name);
            assert_eq!(parsed.timestamp.unwrap().epoch_ms(), 1_606_480_436_302);
            assert_eq!(
                parsed.actor.as_ref().unwrap().as_str(),
                "5b10a2844c20165700ede21g"
            );
            let issue = parsed.event.issue().unwrap();
            assert_eq!(issue.id.as_str(), "99291");
            assert_eq!(issue.key.as_ref().unwrap().as_str(), "OPS-7");
            assert_eq!(issue.fields.summary.as_deref(), Some("Disk full"));
            let status = issue.fields.status.as_ref().unwrap();
            assert_eq!(status.name, "In Progress");
            assert_eq!(status.category.as_str(), "indeterminate");
            assert_eq!(
                issue.fields.labels.as_deref(),
                Some(&["autumn-tracked".to_owned()][..])
            );
            assert_eq!(issue.fields.project.as_ref().unwrap().as_str(), "OPS");
            assert_eq!(issue.fields.issue_type.as_deref(), Some("Task"));
            match &parsed.event {
                JiraEvent::IssueCreated(e)
                | JiraEvent::IssueUpdated(e)
                | JiraEvent::IssueDeleted(e) => {
                    assert_eq!(e.changelog.len(), 2);
                    assert_eq!(e.type_name.as_deref(), Some("issue_generic"));
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(matches!(parsed.event, JiraEvent::IssueCreated(_)), created);
        }
        for name in ["comment_created", "comment_updated", "comment_deleted"] {
            let parsed = parse(&comment_payload(name, "hello"));
            assert_eq!(parsed.event.name(), name);
            assert_eq!(parsed.actor, None);
            let (JiraEvent::CommentCreated(e)
            | JiraEvent::CommentUpdated(e)
            | JiraEvent::CommentDeleted(e)) = &parsed.event
            else {
                panic!("{:?}", parsed.event);
            };
            assert_eq!(e.comment.id.as_str(), "10100");
            assert_eq!(e.comment.author.as_ref().unwrap().as_str(), "712020:bot");
            assert_eq!(e.comment.body.as_ref().unwrap().to_plain_text(), "hello");
            assert_eq!(e.issue.id.as_str(), "99291");
            assert!(!parsed.event.has_comment_marker());
            assert_eq!(parsed.event.status_change(), None);
        }
        let marked = parse(&comment_payload("comment_created", "done\nautumn-ref: abc"));
        assert!(marked.event.has_comment_marker());
    }

    #[test]
    fn unknown_event_is_other() {
        let parsed = parse(&json!({"webhookEvent": "sprint_started", "sprint": {"id": 1}}));
        assert_eq!(
            parsed.event,
            JiraEvent::Other {
                name: "sprint_started".to_owned()
            }
        );
        assert_eq!(parsed.event.name(), "sprint_started");
        assert_eq!(parsed.event.issue(), None);
        assert_eq!(parsed.event.status_change(), None);
        assert!(!parsed.event.has_comment_marker());
        assert_eq!(parsed.timestamp, None);
    }

    #[test]
    fn reduced_issue_parses() {
        let parsed = parse(&json!({
            "webhookEvent": "comment_created",
            "comment": {"id": "1"},
            "issue": {"id": "5"}
        }));
        let issue = parsed.event.issue().unwrap();
        assert_eq!(issue.id.as_str(), "5");
        assert_eq!(issue.key, None);
        assert_eq!(issue.fields, WebhookFields::default());
        let odd = parse(&json!({
            "webhookEvent": "jira:issue_created",
            "issue": {"id": "5", "key": "lower-7", "fields": {"status": {"name": "no id"}, "project": {"name": "x"}}}
        }));
        let issue = odd.event.issue().unwrap();
        // A key from Jira is kept as given (lenient, F42).
        assert_eq!(issue.key.as_ref().map(IssueKey::as_str), Some("lower-7"));
        assert_eq!(issue.fields.status, None);
        assert_eq!(issue.fields.project, None);
        for bad in [
            json!({"webhookEvent": "jira:issue_created"}),
            json!({"webhookEvent": "jira:issue_created", "issue": {"key": "A-1"}}),
            json!({"webhookEvent": "jira:issue_created", "issue": {"id": "0"}}),
            json!({"webhookEvent": "comment_created", "issue": {"id": "5"}}),
            json!({"webhookEvent": "comment_created", "issue": {"id": "5"}, "comment": {}}),
            json!({"issue": {"id": "5"}}),
            json!({"webhookEvent": 7}),
            json!([1, 2]),
        ] {
            let body = serde_json::to_vec(&bad).unwrap();
            assert!(parse_event(&body).is_err(), "{bad}");
        }
        let err = parse_event(b"{\"secret-token\": ").unwrap_err();
        assert!(!err.to_string().contains("secret"), "{err}");
        assert!(parse_event(b"not json").is_err());
    }

    #[test]
    fn changelog_id_string_or_number() {
        let mut payload = issue_payload("jira:issue_updated");
        payload["changelog"]["id"] = json!(10_124);
        assert!(parse(&payload).event.status_change().is_some());
        payload["changelog"]["id"] = json!("10124");
        assert!(parse(&payload).event.status_change().is_some());
        payload["changelog"] = json!(null);
        let parsed = parse(&payload);
        assert_eq!(parsed.event.status_change(), None);
        let JiraEvent::IssueUpdated(e) = parsed.event else {
            panic!("not updated");
        };
        assert_eq!(e.changelog, []);
        // Item values that are numbers are read as text.
        let mut numbers = issue_payload("jira:issue_updated");
        numbers["changelog"]["items"] = json!([{"field": "status", "from": 1, "to": 3}]);
        let change = parse(&numbers).event.status_change().unwrap();
        assert_eq!(change.to.id.unwrap().as_str(), "3");
    }

    #[test]
    fn status_change_from_changelog() {
        let parsed = parse(&issue_payload("jira:issue_updated"));
        let change = parsed.event.status_change().unwrap();
        assert_eq!(
            change,
            StatusChange {
                from: StatusRef {
                    id: Some(StatusId::parse("1").unwrap()),
                    name: Some("To Do".to_owned()),
                },
                to: StatusRef {
                    id: Some(StatusId::parse("3").unwrap()),
                    name: Some("In Progress".to_owned()),
                },
            }
        );
        let mut no_status = issue_payload("jira:issue_updated");
        no_status["changelog"]["items"] =
            json!([{"field": "summary", "fromString": "a", "toString": "b"}]);
        assert_eq!(parse(&no_status).event.status_change(), None);
        let mut bare = issue_payload("jira:issue_updated");
        bare["changelog"]["items"] = json!([{"field": "status"}]);
        let change = parse(&bare).event.status_change().unwrap();
        assert_eq!(
            change.to,
            StatusRef {
                id: None,
                name: None
            }
        );
    }

    #[test]
    fn flow_and_delivery() {
        assert_eq!(Flow::parse("Primary"), Flow::Primary);
        assert_eq!(Flow::parse("secondary"), Flow::Secondary);
        assert_eq!(Flow::parse(""), Flow::Unknown);
        assert_eq!(Flow::parse("Tertiary"), Flow::Unknown);
        let meta = DeliveryMeta::new(WebhookDeliveryId::parse("d-1").unwrap(), 2, Flow::Primary);
        let delivery = Delivery::new(meta.clone(), parse(&issue_payload("jira:issue_created")));
        assert_eq!(delivery.meta, meta);
        assert_eq!(delivery.event.name(), "jira:issue_created");
        assert!(delivery.timestamp.is_some());
        assert!(delivery.actor.is_some());
    }
}

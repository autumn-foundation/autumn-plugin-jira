//! Reads the request body of an exchange (plan section 10).
//!
//! The error texts name the operation and the rule. They never hold a value
//! of the body: a body can hold personal data or a secret.

use autumn_jira::Adf;
use autumn_jira::ids::FieldId;
use autumn_jira::request::{IssueEdit, TransitionFields};
use camel_api::{Body, CamelError, Value};
use serde_json::Map;

use crate::options::Operation;

/// A body after the read: JSON, plain text or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Raw {
    /// No body, or a JSON `null`.
    Empty,
    /// A JSON value.
    Json(Value),
    /// Text that is not JSON.
    Text(String),
}

/// Reads a body with the limit `max` in bytes.
///
/// # Errors
///
/// `StreamLimitExceeded` over the limit. `ValidationError` for bytes that
/// are not UTF-8.
pub(crate) async fn read(body: Body, op: Operation, max: usize) -> Result<Raw, CamelError> {
    let bytes = match body {
        Body::Json(value) => {
            // The limit applies to the JSON text, as for other bodies.
            let size = serde_json::to_vec(&value).map_or(usize::MAX, |v| v.len());
            if size > max {
                return Err(CamelError::StreamLimitExceeded(max));
            }
            return Ok(if value.is_null() {
                Raw::Empty
            } else {
                Raw::Json(value)
            });
        }
        other => other.into_bytes(max).await?,
    };
    if bytes.is_empty() {
        return Ok(Raw::Empty);
    }
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|_| invalid(op, "the body must be JSON or UTF-8 text"))?;
    Ok(match serde_json::from_str::<Value>(&text) {
        Ok(Value::Null) => Raw::Empty,
        Ok(value) => Raw::Json(value),
        Err(_) => Raw::Text(text),
    })
}

/// A `ValidationError` for a body rule.
pub(crate) fn invalid(op: Operation, rule: &str) -> CamelError {
    CamelError::ValidationError(format!("{op}: {rule}"))
}

/// The body of `createIssue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateBody {
    pub(crate) summary: String,
    pub(crate) description: Option<Adf>,
    pub(crate) labels: Vec<String>,
    pub(crate) fields: Vec<(FieldId, Value)>,
}

/// The keys of a `createIssue` body.
const CREATE_KEYS: &[&str] = &["summary", "description", "labels", "fields"];
/// The keys of an `editIssue` body.
const EDIT_KEYS: &[&str] = &["setSummary", "addLabels", "removeLabels", "fields"];

/// Reads a `createIssue` body: `{summary, description?, labels?, fields?}`.
pub(crate) fn create(raw: Raw) -> Result<CreateBody, CamelError> {
    let op = Operation::CreateIssue;
    let object = object(op, raw, "the body must be a JSON object with a summary")?;
    known_keys(op, &object, CREATE_KEYS)?;
    let summary = match object.get("summary") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(_) => return Err(invalid(op, "summary must be a non-empty string")),
        None => return Err(invalid(op, "the body needs a summary")),
    };
    let description = match object.get("description") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(Adf::text(s)),
        Some(value) => Some(
            Adf::from_value(value.clone())
                .map_err(|_| invalid(op, "description must be a string or an ADF document"))?,
        ),
    };
    Ok(CreateBody {
        summary,
        description,
        labels: strings(op, &object, "labels")?,
        fields: fields(op, &object)?,
    })
}

/// Reads an `editIssue` body: `{setSummary?, addLabels?, removeLabels?,
/// fields?}`. It must hold at least one change.
pub(crate) fn edit(raw: Raw) -> Result<IssueEdit, CamelError> {
    let op = Operation::EditIssue;
    let object = object(op, raw, "the body must be a JSON object")?;
    known_keys(op, &object, EDIT_KEYS)?;
    let mut edit = IssueEdit::new();
    match object.get("setSummary") {
        None => {}
        Some(Value::String(s)) if !s.trim().is_empty() => edit = edit.set_summary(s.as_str()),
        Some(_) => return Err(invalid(op, "setSummary must be a non-empty string")),
    }
    for label in strings(op, &object, "addLabels")? {
        edit = edit.add_label(label);
    }
    for label in strings(op, &object, "removeLabels")? {
        edit = edit.remove_label(label);
    }
    for (id, value) in fields(op, &object)? {
        edit = edit.set_field(id, value);
    }
    if edit.is_empty() {
        return Err(invalid(
            op,
            "the body has no change; set setSummary, addLabels, removeLabels or fields",
        ));
    }
    Ok(edit)
}

/// Reads a `transitionToStatus` body: nothing, or `{fields}`.
pub(crate) fn transition(raw: Raw) -> Result<TransitionFields, CamelError> {
    let op = Operation::TransitionToStatus;
    if raw == Raw::Empty {
        return Ok(TransitionFields::default());
    }
    let object = object(
        op,
        raw,
        "the body must be empty or a JSON object with fields",
    )?;
    known_keys(op, &object, &["fields"])?;
    let mut out = TransitionFields::default();
    match object.get("fields") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                out = out.field(name.as_str(), value.clone());
            }
        }
        Some(_) => return Err(invalid(op, "fields must be a JSON object")),
    }
    Ok(out)
}

/// Reads an `addComment` body: `{text}` or a string.
pub(crate) fn comment(raw: Raw) -> Result<Adf, CamelError> {
    let op = Operation::AddComment;
    let text = match raw {
        Raw::Text(text) | Raw::Json(Value::String(text)) => text,
        Raw::Json(Value::Object(object)) => {
            known_keys(op, &object, &["text"])?;
            match object.get("text") {
                Some(Value::String(text)) => text.clone(),
                Some(_) => return Err(invalid(op, "text must be a string")),
                None => return Err(invalid(op, "the body needs a text")),
            }
        }
        Raw::Empty | Raw::Json(_) => {
            return Err(invalid(op, "the body must be {\"text\": ...} or a string"));
        }
    };
    if text.trim().is_empty() {
        return Err(invalid(op, "the text must not be empty"));
    }
    Ok(Adf::text(&text))
}

/// Returns the JSON object of a body.
fn object(op: Operation, raw: Raw, rule: &str) -> Result<Map<String, Value>, CamelError> {
    match raw {
        Raw::Json(Value::Object(object)) => Ok(object),
        _ => Err(invalid(op, rule)),
    }
}

/// Fails on a key that the operation does not know. The text lists the
/// known keys, not the unknown key: the key comes from the body.
fn known_keys(op: Operation, object: &Map<String, Value>, keys: &[&str]) -> Result<(), CamelError> {
    if object.keys().all(|k| keys.contains(&k.as_str())) {
        return Ok(());
    }
    Err(invalid(
        op,
        &format!(
            "the body has an unknown key; the keys are {}",
            keys.join(", ")
        ),
    ))
}

/// Reads a list of strings. A missing key gives an empty list.
fn strings(
    op: Operation,
    object: &Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, CamelError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => Ok(s.clone()),
                _ => Err(invalid(op, &format!("{key} must be a list of strings"))),
            })
            .collect(),
        Some(_) => Err(invalid(op, &format!("{key} must be a list of strings"))),
    }
}

/// Reads `fields`: a JSON object from field id to value.
fn fields(op: Operation, object: &Map<String, Value>) -> Result<Vec<(FieldId, Value)>, CamelError> {
    match object.get("fields") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(id, value)| {
                FieldId::parse(id)
                    .map(|id| (id, value.clone()))
                    .map_err(|_| {
                        invalid(
                            op,
                            "each key of fields must be a field id, for example customfield_10010",
                        )
                    })
            })
            .collect(),
        Some(_) => Err(invalid(op, "fields must be a JSON object")),
    }
}

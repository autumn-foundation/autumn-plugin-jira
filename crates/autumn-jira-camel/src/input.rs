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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, reason = "tests")]
mod tests {
    use camel_api::Value;
    use serde_json::json;

    use super::*;

    const OP: Operation = Operation::CreateIssue;

    async fn raw(body: Body) -> Result<Raw, CamelError> {
        read(body, OP, 64).await
    }

    fn rule(err: &CamelError) -> String {
        match err {
            CamelError::ValidationError(m) => m.clone(),
            other => panic!("expected ValidationError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_takes_json_text_and_bytes() {
        assert_eq!(raw(Body::Empty).await.unwrap(), Raw::Empty);
        assert_eq!(raw(Body::Json(Value::Null)).await.unwrap(), Raw::Empty);
        assert_eq!(raw(Body::Text("null".into())).await.unwrap(), Raw::Empty);
        assert_eq!(
            raw(Body::Bytes(Vec::new().into())).await.unwrap(),
            Raw::Empty
        );
        assert_eq!(
            raw(Body::Text(r#"{"a":1}"#.into())).await.unwrap(),
            Raw::Json(json!({"a": 1}))
        );
        assert_eq!(
            raw(Body::Bytes(b"\"x\"".to_vec().into())).await.unwrap(),
            Raw::Json(json!("x"))
        );
        assert_eq!(
            raw(Body::Text("plain words".into())).await.unwrap(),
            Raw::Text("plain words".into())
        );
        let err = raw(Body::Bytes(vec![0xff, 0xfe].into())).await.unwrap_err();
        assert!(rule(&err).contains("UTF-8"), "{err}");
        // The limit applies to each body kind.
        let big = "x".repeat(65);
        for body in [
            Body::Json(json!(big)),
            Body::Text(big.clone()),
            Body::Bytes(big.clone().into_bytes().into()),
        ] {
            let err = raw(body).await.unwrap_err();
            assert!(
                matches!(err, CamelError::StreamLimitExceeded(64)),
                "{err:?}"
            );
        }
    }

    #[test]
    fn create_reads_each_key() {
        let body = create(Raw::Json(json!({
            "summary": "S",
            "description": {"type": "doc", "version": 1, "content": []},
            "labels": ["a", "b"],
            "fields": {"customfield_10010": 3},
        })))
        .unwrap();
        assert_eq!(body.summary, "S");
        assert!(body.description.is_some());
        assert_eq!(body.labels, ["a", "b"]);
        assert_eq!(body.fields.len(), 1);
        let body = create(Raw::Json(json!({"summary": "S", "description": null}))).unwrap();
        assert_eq!(body.description, None);
        for (bad, needle) in [
            (json!({}), "needs a summary"),
            (json!({"summary": " "}), "summary"),
            (json!({"summary": "S", "description": {"type": "x"}}), "ADF"),
            (json!({"summary": "S", "labels": [1]}), "labels"),
            (json!({"summary": "S", "fields": []}), "fields"),
            (json!({"summary": "S", "fields": {"Bad Id": 1}}), "field id"),
        ] {
            let err = create(Raw::Json(bad.clone())).unwrap_err();
            assert!(rule(&err).contains(needle), "{bad}: {err}");
        }
        assert!(create(Raw::Empty).is_err());
    }

    #[test]
    fn edit_needs_one_change() {
        let edit = edit(Raw::Json(json!({"removeLabels": ["a"]}))).unwrap();
        assert!(!edit.is_empty());
        let err = super::edit(Raw::Json(json!({"setSummary": " "}))).unwrap_err();
        assert!(rule(&err).contains("setSummary"), "{err}");
        let err = super::edit(Raw::Text("x".into())).unwrap_err();
        assert!(rule(&err).contains("editIssue"), "{err}");
    }

    #[test]
    fn transition_takes_nothing_or_fields() {
        assert_eq!(transition(Raw::Empty).unwrap(), TransitionFields::default());
        assert_eq!(
            transition(Raw::Json(json!({"fields": null}))).unwrap(),
            TransitionFields::default()
        );
        let fields = transition(Raw::Json(
            json!({"fields": {"resolution": {"name": "Done"}}}),
        ))
        .unwrap();
        assert_eq!(
            fields,
            TransitionFields::default().field("resolution", json!({"name": "Done"}))
        );
        for bad in [json!({"fields": 1}), json!({"update": {}}), json!([1])] {
            let err = transition(Raw::Json(bad.clone())).unwrap_err();
            assert!(rule(&err).contains("transitionToStatus"), "{bad}: {err}");
        }
    }

    #[test]
    fn comment_takes_text_or_a_string() {
        assert_eq!(comment(Raw::Text("a".into())).unwrap(), Adf::text("a"));
        assert_eq!(comment(Raw::Json(json!("b"))).unwrap(), Adf::text("b"));
        assert_eq!(
            comment(Raw::Json(json!({"text": "c"}))).unwrap(),
            Adf::text("c")
        );
        for (bad, needle) in [
            (Raw::Json(json!({})), "needs a text"),
            (Raw::Json(json!({"text": 1})), "must be a string"),
            (Raw::Json(json!({"body": "x"})), "unknown key"),
            (Raw::Json(json!(1)), "or a string"),
            (Raw::Empty, "or a string"),
            (Raw::Text(" ".into()), "must not be empty"),
        ] {
            let err = comment(bad).unwrap_err();
            assert!(rule(&err).contains(needle), "{needle}: {err}");
        }
    }
}

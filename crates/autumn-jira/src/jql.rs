//! A small JQL builder.
//!
//! Each value goes through `policy::jql_quote`, so a value is always one
//! string literal. A value with a control char gives `InvalidInput`.
//! `Jql::raw` is the only way to give JQL text: the app owns that text.

use std::fmt;

use crate::error::{JiraError, Result};
use crate::ids::ProjectKey;
use crate::policy::jql_quote;

/// A JQL query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jql {
    text: String,
    /// The byte offset of ` ORDER BY`, when the query has one.
    order_at: Option<usize>,
    /// `true` for text from `Jql::raw` that `and` did not wrap yet.
    raw: bool,
}

/// A sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

/// A JQL field.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum JqlField {
    /// `project`.
    Project,
    /// `key`.
    Key,
    /// `labels`.
    Labels,
    /// `status`.
    Status,
    /// `statusCategory`.
    StatusCategory,
    /// `issuetype`.
    IssueType,
    /// `assignee`.
    Assignee,
    /// `created`.
    Created,
    /// `updated`.
    Updated,
    /// `cf[N]`: a custom field by number.
    Custom(u64),
    /// `issue.property[key].path`.
    Property {
        /// The property key: 1 to 255 chars of `[A-Za-z0-9._-]`.
        key: String,
        /// The path in the value: names of `[A-Za-z0-9_]` joined by `.`.
        path: String,
    },
}

impl JqlField {
    /// Returns the JQL name, or `None` for a bad custom field or property.
    fn name(&self) -> Option<String> {
        let name = match self {
            Self::Project => "project",
            Self::Key => "key",
            Self::Labels => "labels",
            Self::Status => "status",
            Self::StatusCategory => "statusCategory",
            Self::IssueType => "issuetype",
            Self::Assignee => "assignee",
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Custom(0) => return None,
            Self::Custom(n) => return Some(format!("cf[{n}]")),
            Self::Property { key, path } => {
                return (is_property_key(key) && is_property_path(path))
                    .then(|| format!("issue.property[{key}].{path}"));
            }
        };
        Some(name.to_owned())
    }

    fn checked_name(&self) -> Result<String> {
        self.name().ok_or_else(|| {
            JiraError::invalid(
                "jql field",
                "a custom field must be above 0; a property needs a key of \
                 [A-Za-z0-9._-]{1,255} and a path of [A-Za-z0-9_] names joined by '.'",
            )
        })
    }
}

/// A clause that waits for its operator: `Jql::field(f).eq("x")`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct JqlClause {
    field: JqlField,
}

/// A complete condition, for example `labels = "x"`. `Jql::and` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JqlCondition(String);

impl JqlCondition {
    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JqlClause {
    /// `field = "value"`.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for a bad field or a control char in the value.
    pub fn eq(self, value: impl AsRef<str>) -> Result<JqlCondition> {
        self.binary("=", value.as_ref())
    }

    /// `field != "value"`.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for a bad field or a control char in the value.
    pub fn ne(self, value: impl AsRef<str>) -> Result<JqlCondition> {
        self.binary("!=", value.as_ref())
    }

    /// `field in ("a", "b")`.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for a bad field, an empty list or a control
    /// char in a value.
    pub fn in_<S: AsRef<str>>(self, values: &[S]) -> Result<JqlCondition> {
        let name = self.field.checked_name()?;
        if values.is_empty() {
            return Err(JiraError::invalid("jql value", "an in list needs a value"));
        }
        let quoted = values
            .iter()
            .map(|v| quote(v.as_ref()))
            .collect::<Result<Vec<_>>>()?;
        Ok(JqlCondition(format!("{name} in ({})", quoted.join(", "))))
    }

    /// `field is EMPTY`.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for a bad field.
    pub fn is_empty(self) -> Result<JqlCondition> {
        let name = self.field.checked_name()?;
        Ok(JqlCondition(format!("{name} is EMPTY")))
    }

    fn binary(self, op: &str, value: &str) -> Result<JqlCondition> {
        let name = self.field.checked_name()?;
        Ok(JqlCondition(format!("{name} {op} {}", quote(value)?)))
    }
}

impl Jql {
    /// `project = "KEY"`: a bounded start. Jira rejects an unbounded search.
    #[must_use]
    pub fn project(key: &ProjectKey) -> Self {
        Self {
            text: format!("project = {}", jql_quote(key.as_str())),
            order_at: None,
            raw: false,
        }
    }

    /// Starts a clause on `name`.
    pub const fn field(name: JqlField) -> JqlClause {
        JqlClause { field: name }
    }

    /// Adds `AND condition`. Raw text goes in parentheses first, so an `OR`
    /// in it cannot change the meaning.
    #[must_use]
    pub fn and(mut self, condition: JqlCondition) -> Self {
        if self.raw {
            self.text = format!("({})", self.text);
            self.raw = false;
        }
        let JqlCondition(text) = condition;
        let clause = format!(" AND {text}");
        match self.order_at {
            Some(at) => {
                self.text.insert_str(at, &clause);
                self.order_at = Some(at + clause.len());
            }
            None => self.text.push_str(&clause),
        }
        self
    }

    /// Adds a sort key. A bad custom field or property is skipped.
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "builder style: the caller hands over the field"
    )]
    pub fn order_by(mut self, field: JqlField, order: Order) -> Self {
        let Some(name) = field.name() else {
            return self;
        };
        let order = match order {
            Order::Asc => "ASC",
            Order::Desc => "DESC",
        };
        if self.order_at.is_some() {
            self.text.push_str(", ");
        } else {
            self.order_at = Some(self.text.len());
            self.text.push_str(" ORDER BY ");
        }
        self.text.push_str(&name);
        self.text.push(' ');
        self.text.push_str(order);
        self
    }

    /// Uses JQL text as it is. The name is explicit: the app owns the text.
    #[must_use]
    pub fn raw(jql: impl Into<String>) -> Self {
        Self {
            text: jql.into(),
            order_at: None,
            raw: true,
        }
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for Jql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Quotes a value. A control char gives `InvalidInput`.
fn quote(value: &str) -> Result<String> {
    if value.chars().any(char::is_control) {
        return Err(JiraError::invalid(
            "jql value",
            "a value must not hold a control char",
        ));
    }
    Ok(jql_quote(value))
}

/// 1 to 255 chars of `[A-Za-z0-9._-]`.
fn is_property_key(key: &str) -> bool {
    (1..=255).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Names of `[A-Za-z0-9_]` joined by `.`.
fn is_property_path(path: &str) -> bool {
    path.len() <= 255
        && path
            .split('.')
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    fn ops() -> ProjectKey {
        ProjectKey::parse("OPS").unwrap()
    }

    #[test]
    fn builds_clauses() {
        let jql = Jql::project(&ops())
            .and(Jql::field(JqlField::Labels).eq("autumn-idem-abc").unwrap())
            .and(Jql::field(JqlField::Status).ne("Done").unwrap())
            .and(
                Jql::field(JqlField::IssueType)
                    .in_(&["Bug", "Task"])
                    .unwrap(),
            )
            .and(Jql::field(JqlField::Assignee).is_empty().unwrap())
            .order_by(JqlField::Created, Order::Desc)
            .order_by(JqlField::Key, Order::Asc);
        assert_eq!(
            jql.as_str(),
            "project = \"OPS\" AND labels = \"autumn-idem-abc\" AND status != \"Done\" \
             AND issuetype in (\"Bug\", \"Task\") AND assignee is EMPTY \
             ORDER BY created DESC, key ASC"
        );
        // A clause after ORDER BY goes before it.
        let late = Jql::project(&ops())
            .order_by(JqlField::Updated, Order::Asc)
            .and(Jql::field(JqlField::StatusCategory).eq("done").unwrap());
        assert_eq!(
            late.to_string(),
            "project = \"OPS\" AND statusCategory = \"done\" ORDER BY updated ASC"
        );
        let custom = Jql::project(&ops())
            .and(Jql::field(JqlField::Custom(10010)).eq("x").unwrap())
            .and(
                Jql::field(JqlField::Property {
                    key: "autumn-jira".to_owned(),
                    path: "idempotencyKey".to_owned(),
                })
                .eq("k1")
                .unwrap(),
            );
        assert_eq!(
            custom.as_str(),
            "project = \"OPS\" AND cf[10010] = \"x\" AND \
             issue.property[autumn-jira].idempotencyKey = \"k1\""
        );
        let created = Jql::field(JqlField::Created).eq("2026-01-01").unwrap();
        assert_eq!(created.as_str(), "created = \"2026-01-01\"");
        let key = Jql::field(JqlField::Key).eq("OPS-1").unwrap();
        assert_eq!(
            Jql::field(JqlField::Project).eq("A").unwrap().as_str(),
            "project = \"A\""
        );
        assert_eq!(key.as_str(), "key = \"OPS-1\"");
    }

    #[test]
    fn raw_text_is_wrapped_before_and() {
        let jql = Jql::raw("project = A OR project = B")
            .and(Jql::field(JqlField::Labels).eq("x").unwrap())
            .and(Jql::field(JqlField::Labels).eq("y").unwrap());
        assert_eq!(
            jql.as_str(),
            "(project = A OR project = B) AND labels = \"x\" AND labels = \"y\""
        );
        assert_eq!(
            Jql::raw("assignee = currentUser()").as_str(),
            "assignee = currentUser()"
        );
    }

    #[test]
    fn injection_stays_one_literal() {
        let evil = "x\" OR project = SECRET";
        let jql = Jql::project(&ops()).and(Jql::field(JqlField::Labels).eq(evil).unwrap());
        assert_eq!(
            jql.as_str(),
            "project = \"OPS\" AND labels = \"x\\\" OR project = SECRET\""
        );
        let backslash = Jql::field(JqlField::Labels).eq("a\\\" OR 1=1").unwrap();
        assert_eq!(backslash.as_str(), "labels = \"a\\\\\\\" OR 1=1\"");
        let list = Jql::field(JqlField::Labels).in_(&["a\", \"b"]).unwrap();
        assert_eq!(list.as_str(), "labels in (\"a\\\", \\\"b\")");
    }

    #[test]
    fn control_chars_rejected() {
        for bad in ["a\nb", "a\rb", "\u{0}", "tab\there", "\u{7f}", "\u{85}"] {
            let err = Jql::field(JqlField::Labels).eq(bad).unwrap_err();
            assert_eq!(err.code(), "invalid_input", "{bad:?}");
            assert!(Jql::field(JqlField::Labels).ne(bad).is_err());
            assert!(Jql::field(JqlField::Labels).in_(&["ok", bad]).is_err());
        }
        assert!(Jql::field(JqlField::Labels).in_::<&str>(&[]).is_err());
    }

    #[test]
    fn field_names_validated() {
        let bad_fields = [
            JqlField::Custom(0),
            JqlField::Property {
                key: String::new(),
                path: "a".to_owned(),
            },
            JqlField::Property {
                key: "k] OR x".to_owned(),
                path: "a".to_owned(),
            },
            JqlField::Property {
                key: "k".to_owned(),
                path: String::new(),
            },
            JqlField::Property {
                key: "k".to_owned(),
                path: "a..b".to_owned(),
            },
            JqlField::Property {
                key: "k".to_owned(),
                path: "a = 1 OR b".to_owned(),
            },
            JqlField::Property {
                key: "k".repeat(256),
                path: "a".to_owned(),
            },
        ];
        for field in bad_fields {
            assert!(Jql::field(field.clone()).eq("x").is_err(), "{field:?}");
            assert!(Jql::field(field.clone()).is_empty().is_err(), "{field:?}");
            let skipped = Jql::project(&ops()).order_by(field, Order::Asc);
            assert_eq!(skipped.as_str(), "project = \"OPS\"");
        }
        let good = JqlField::Property {
            key: "a.b-c_d".to_owned(),
            path: "x.y_1".to_owned(),
        };
        assert_eq!(
            Jql::field(good).is_empty().unwrap().as_str(),
            "issue.property[a.b-c_d].x.y_1 is EMPTY"
        );
    }
}

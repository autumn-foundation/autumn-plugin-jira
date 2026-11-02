//! Newtypes for Jira ids, keys and names.
//!
//! `parse` is strict, and `Deserialize` calls `parse`. So an autumn handler
//! that takes `Path<IssueKey>` rejects bad input with a 400. Response models
//! read Jira ids through `model::wire` with `from_response`, which keeps the
//! value as Jira gives it (a moved or renamed key can have another shape).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::error::{JiraError, Result};

/// Number of base32 chars in a marker: 26 chars hold 130 bits.
const MARKER_CHARS: usize = 26;

/// Defines a string newtype with a strict `parse` and a strict `Deserialize`.
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident, $what:literal, $rule:literal, $check:path) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Parses a ", $what, " strictly. Rule: ", $rule, ".")]
            ///
            /// # Errors
            ///
            /// `JiraError::InvalidInput` when the text does not match the rule.
            pub fn parse(text: &str) -> Result<Self> {
                if $check(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(JiraError::invalid($what, concat!("must match ", $rule)))
                }
            }

            /// Returns the text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Keeps a value from a Jira reply as it is (lenient).
            #[allow(dead_code, reason = "not every id comes in a reply")]
            pub(crate) const fn from_response(text: String) -> Self {
                Self(text)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = JiraError;
            fn from_str(text: &str) -> Result<Self> {
                Self::parse(text)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::parse(&text).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// Defines a numeric id newtype. Jira sends these ids as strings.
macro_rules! numeric_id {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        string_id!(
            $(#[$meta])*
            #[derive(Debug)]
            $name,
            $what,
            "[1-9][0-9]{0,18}",
            is_numeric_id
        );
    };
}

string_id!(
    /// A project key, for example `OPS`.
    #[derive(Debug)]
    ProjectKey,
    "project key",
    "[A-Z][A-Z0-9_]{1,9}",
    is_project_key
);

string_id!(
    /// An issue key, for example `OPS-12`.
    #[derive(Debug)]
    IssueKey,
    "issue key",
    "[A-Z][A-Z0-9_]{1,9}-[1-9][0-9]{0,9}",
    is_issue_key
);

numeric_id!(
    /// The numeric id of an issue, for example `10001`.
    IssueId,
    "issue id"
);
numeric_id!(
    /// The numeric id of a project.
    ProjectId,
    "project id"
);
numeric_id!(
    /// The numeric id of a status.
    StatusId,
    "status id"
);
numeric_id!(
    /// The numeric id of a comment.
    CommentId,
    "comment id"
);
numeric_id!(
    /// The numeric id of a transition.
    TransitionId,
    "transition id"
);
numeric_id!(
    /// The numeric id of an issue type.
    IssueTypeId,
    "issue type id"
);

string_id!(
    /// An Atlassian account id.
    #[derive(Debug)]
    AccountId,
    "account id",
    "1 to 128 chars of [A-Za-z0-9:_-]",
    is_account_id
);

string_id!(
    /// A cloud id: a lower-case UUID.
    #[derive(Debug)]
    CloudId,
    "cloud id",
    "a lower-case UUID of 36 chars",
    is_cloud_id
);

string_id!(
    /// The subdomain of a site: `acme` for `acme.atlassian.net`.
    #[derive(Debug)]
    SiteName,
    "site name",
    "[a-z0-9][a-z0-9-]{0,62}",
    is_site_name
);

string_id!(
    /// A field id, for example `summary` or `customfield_10010`.
    #[derive(Debug)]
    FieldId,
    "field id",
    "[a-z][A-Za-z0-9_]{0,63} or customfield_[0-9]{1,10}",
    is_field_id
);

string_id!(
    /// The id of one webhook delivery (`X-Atlassian-Webhook-Identifier`).
    #[derive(Debug)]
    WebhookDeliveryId,
    "webhook delivery id",
    "1 to 200 printable ASCII chars with no space",
    is_printable_token
);

string_id!(
    /// An opaque page token of the search pager.
    PageToken,
    "page token",
    "a non-empty string",
    is_non_empty
);

impl fmt::Debug for PageToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageToken({} chars)", self.0.len())
    }
}

impl FieldId {
    /// Makes a field id from a known good name.
    pub(crate) fn known(name: &'static str) -> Self {
        Self(name.to_owned())
    }
}

impl IssueKey {
    /// Returns the project part: `OPS` for `OPS-12`.
    #[must_use]
    pub fn project(&self) -> ProjectKey {
        let project = self.0.rsplit_once('-').map_or("", |(p, _)| p);
        ProjectKey(project.to_owned())
    }

    /// Returns the number part: `12` for `OPS-12`. A lenient response key
    /// with no number gives 0.
    #[must_use]
    pub fn number(&self) -> u64 {
        self.0
            .rsplit_once('-')
            .and_then(|(_, n)| n.parse().ok())
            .unwrap_or(0)
    }
}

/// The key of a status category.
///
/// Strict input: 1 to 64 chars of `[a-z0-9-]`. Replies are lenient: the
/// spec shows `in-flight`, and other keys can come.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StatusCategoryKey(Cow<'static, str>);

impl StatusCategoryKey {
    /// The "To Do" category.
    pub const NEW: Self = Self(Cow::Borrowed("new"));
    /// The "In Progress" category.
    pub const INDETERMINATE: Self = Self(Cow::Borrowed("indeterminate"));
    /// The "Done" category.
    pub const DONE: Self = Self(Cow::Borrowed("done"));

    /// Parses a status category key strictly. Rule: 1 to 64 chars of `[a-z0-9-]`.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when the text does not match the rule.
    pub fn parse(text: &str) -> Result<Self> {
        if is_category_key(text) {
            Ok(Self(Cow::Owned(text.to_owned())))
        } else {
            Err(JiraError::invalid(
                "status category key",
                "must match 1 to 64 chars of [a-z0-9-]",
            ))
        }
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Keeps a value from a Jira reply as it is (lenient).
    pub(crate) const fn from_response(text: String) -> Self {
        Self(Cow::Owned(text))
    }
}

impl fmt::Display for StatusCategoryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for StatusCategoryKey {
    type Err = JiraError;
    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
}

impl Serialize for StatusCategoryKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for StatusCategoryKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// An issue, by numeric id or by key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueRef {
    /// A numeric id, for example `10001`.
    Id(IssueId),
    /// A key, for example `OPS-12`.
    Key(IssueKey),
}

impl IssueRef {
    /// Parses an issue id or an issue key. Only digits give an id.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when the text is neither.
    pub fn parse(text: &str) -> Result<Self> {
        if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            IssueId::parse(text).map(Self::Id)
        } else {
            IssueKey::parse(text).map(Self::Key)
        }
    }

    /// Returns the text for the URL path segment.
    #[must_use]
    pub fn path_segment(&self) -> &str {
        match self {
            Self::Id(id) => id.as_str(),
            Self::Key(key) => key.as_str(),
        }
    }
}

impl fmt::Display for IssueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.path_segment())
    }
}

impl FromStr for IssueRef {
    type Err = JiraError;
    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
}

impl Serialize for IssueRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.path_segment())
    }
}

impl<'de> Deserialize<'de> for IssueRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl From<IssueKey> for IssueRef {
    fn from(key: IssueKey) -> Self {
        Self::Key(key)
    }
}

impl From<&IssueKey> for IssueRef {
    fn from(key: &IssueKey) -> Self {
        Self::Key(key.clone())
    }
}

impl From<IssueId> for IssueRef {
    fn from(id: IssueId) -> Self {
        Self::Id(id)
    }
}

impl From<&IssueId> for IssueRef {
    fn from(id: &IssueId) -> Self {
        Self::Id(id.clone())
    }
}

/// A key that makes a write idempotent: 1 to 200 printable ASCII chars with
/// no space.
///
/// It is not a secret, so `Debug` shows it. Jira ignores an
/// `Idempotency-Key` header, so the client never sends one. It writes a
/// marker that a lookup can find (see `marker_label` and `comment_marker`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Parses a key strictly. Rule: 1 to 200 printable ASCII chars with no space.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when the text does not match the rule.
    pub fn parse(text: &str) -> Result<Self> {
        if is_printable_token(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(JiraError::invalid(
                "idempotency key",
                "must match 1 to 200 printable ASCII chars with no space",
            ))
        }
    }

    /// Makes a stable key from app data.
    ///
    /// The key is `d1-` and the lower-case hex SHA-256 of the parts. Each
    /// part goes in with its byte length and `:` first, so `["ab", "c"]`
    /// and `["a", "bc"]` give different keys.
    #[must_use]
    pub fn derive(parts: &[&str]) -> Self {
        let mut hash = Sha256::new();
        for part in parts {
            hash.update(part.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(part.as_bytes());
        }
        Self(format!(
            "d1-{}",
            data_encoding::HEXLOWER.encode(&hash.finalize())
        ))
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the marker label: `prefix` and 26 chars of lower-case base32
    /// of SHA-256(key).
    #[must_use]
    pub fn marker_label(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.marker())
    }

    /// Returns the comment marker: `autumn-ref: ` and the same 26 chars.
    #[must_use]
    pub fn comment_marker(&self) -> String {
        format!("autumn-ref: {}", self.marker())
    }

    fn marker(&self) -> String {
        let digest = Sha256::digest(self.0.as_bytes());
        let mut text = data_encoding::BASE32_NOPAD.encode(&digest);
        text.truncate(MARKER_CHARS);
        text.make_ascii_lowercase();
        text
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for IdempotencyKey {
    type Err = JiraError;
    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

// ------------------------------------------------------------ rules
// The rules are written by hand. The crate has no regex dependency.

const fn is_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

/// `[A-Z][A-Z0-9_]{1,9}`.
fn is_project_key(text: &str) -> bool {
    let bytes = text.as_bytes();
    (2..=10).contains(&bytes.len())
        && bytes.first().copied().is_some_and(is_upper)
        && bytes[1..]
            .iter()
            .all(|b| is_upper(*b) || b.is_ascii_digit() || *b == b'_')
}

/// `[A-Z][A-Z0-9_]{1,9}-[1-9][0-9]{0,9}`.
fn is_issue_key(text: &str) -> bool {
    text.split_once('-')
        .is_some_and(|(project, number)| is_project_key(project) && is_number(number, 9))
}

/// `[1-9][0-9]{0,max_rest}`.
fn is_number(text: &str, max_rest: usize) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max_rest + 1
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

fn is_numeric_id(text: &str) -> bool {
    is_number(text, 18)
}

/// 1 to 128 chars of `[A-Za-z0-9:_-]`.
fn is_account_id(text: &str) -> bool {
    (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-'))
}

/// A lower-case UUID: `8-4-4-4-12` lower hex digits.
fn is_cloud_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                crate::policy::is_lower_hex(*b)
            }
        })
}

/// `[a-z0-9][a-z0-9-]{0,62}`.
fn is_site_name(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=63).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// 1 to 64 chars of `[a-z0-9-]`.
fn is_category_key(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `[a-z][A-Za-z0-9_]{0,63}` or `customfield_[0-9]{1,10}`.
///
/// The first form also holds each custom field id, so the second form adds
/// no ids. The plan names both forms, so both are here.
fn is_field_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    let plain = (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_');
    let custom = text
        .strip_prefix("customfield_")
        .is_some_and(|n| (1..=10).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()));
    plain || custom
}

/// 1 to 200 printable ASCII chars with no space.
fn is_printable_token(text: &str) -> bool {
    (1..=200).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_graphic())
}

const fn is_non_empty(text: &str) -> bool {
    !text.is_empty()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "tests with tables"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// One row: the type name, the input and the expected result.
    fn check(what: &str, input: &str) -> bool {
        match what {
            "project" => ProjectKey::parse(input).is_ok(),
            "issue_key" => IssueKey::parse(input).is_ok(),
            "issue_id" => IssueId::parse(input).is_ok(),
            "project_id" => ProjectId::parse(input).is_ok(),
            "status_id" => StatusId::parse(input).is_ok(),
            "comment_id" => CommentId::parse(input).is_ok(),
            "transition_id" => TransitionId::parse(input).is_ok(),
            "issue_type_id" => IssueTypeId::parse(input).is_ok(),
            "issue_ref" => IssueRef::parse(input).is_ok(),
            "account" => AccountId::parse(input).is_ok(),
            "cloud" => CloudId::parse(input).is_ok(),
            "site" => SiteName::parse(input).is_ok(),
            "category" => StatusCategoryKey::parse(input).is_ok(),
            "field" => FieldId::parse(input).is_ok(),
            "delivery" => WebhookDeliveryId::parse(input).is_ok(),
            "token" => PageToken::parse(input).is_ok(),
            "idem" => IdempotencyKey::parse(input).is_ok(),
            other => panic!("unknown type {other}"),
        }
    }

    #[test]
    fn strict_rules_table() {
        let long_account = "a".repeat(128);
        let too_long_account = "a".repeat(129);
        let site_63 = format!("a{}", "b".repeat(62));
        let site_64 = format!("a{}", "b".repeat(63));
        let token_200 = "x".repeat(200);
        let token_201 = "x".repeat(201);
        let field_64 = format!("a{}", "b".repeat(63));
        let field_65 = format!("a{}", "b".repeat(64));
        let rows: Vec<(&str, &str, bool)> = vec![
            ("project", "OPS", true),
            ("project", "AB", true),
            ("project", "A1_B", true),
            ("project", "ABCDEFGHIJ", true),
            ("project", "ABCDEFGHIJK", false),
            ("project", "A", false),
            ("project", "ops", false),
            ("project", "1AB", false),
            ("project", "_AB", false),
            ("project", "A-B", false),
            ("project", "", false),
            ("issue_key", "OPS-1", true),
            ("issue_key", "OPS-1234567890", true),
            ("issue_key", "OPS-12345678901", false),
            ("issue_key", "A_1-9", true),
            ("issue_key", "../x", false),
            ("issue_key", "ab-1", false),
            ("issue_key", "A-01", false),
            ("issue_key", "AB-01", false),
            ("issue_key", "AB-0", false),
            ("issue_key", "AB-", false),
            ("issue_key", "AB", false),
            ("issue_key", "AB-1-2", false),
            ("issue_key", "AB-1 ", false),
            ("issue_key", "AB-1/../x", false),
            ("issue_id", "1", true),
            ("issue_id", "10001", true),
            ("issue_id", "1234567890123456789", true),
            ("issue_id", "12345678901234567890", false),
            ("issue_id", "0", false),
            ("issue_id", "01", false),
            ("issue_id", "-1", false),
            ("issue_id", "1a", false),
            ("issue_id", "", false),
            ("project_id", "10000", true),
            ("status_id", "3", true),
            ("comment_id", "10100", true),
            ("transition_id", "31", true),
            ("issue_type_id", "10002", true),
            ("transition_id", "x", false),
            ("issue_ref", "10001", true),
            ("issue_ref", "OPS-1", true),
            ("issue_ref", "0", false),
            ("issue_ref", "ops-1", false),
            ("issue_ref", "", false),
            ("account", "5b10a2844c20165700ede21g", true),
            ("account", "712020:abc-DEF_1", true),
            ("account", &long_account, true),
            ("account", &too_long_account, false),
            ("account", "", false),
            ("account", "a b", false),
            ("account", "a/b", false),
            ("cloud", "1324a887-45db-1bf4-1e99-ef0ff456d421", true),
            ("cloud", "1324A887-45DB-1BF4-1E99-EF0FF456D421", false),
            ("cloud", "1324a887-45db-1bf4-1e99-ef0ff456d42", false),
            ("cloud", "1324a887045db-1bf4-1e99-ef0ff456d421", false),
            ("cloud", "1324a887-45db-1bf4-1e99-ef0ff456d42g", false),
            ("site", "acme", true),
            ("site", "0acme-1", true),
            ("site", &site_63, true),
            ("site", &site_64, false),
            ("site", "-acme", false),
            ("site", "Acme", false),
            ("site", "acme.evil.com", false),
            ("site", "acme/x", false),
            ("site", "", false),
            ("category", "new", true),
            ("category", "in-flight", true),
            ("category", "Done", false),
            ("category", "", false),
            ("field", "summary", true),
            ("field", "issuetype", true),
            ("field", "customfield_10010", true),
            ("field", "customfield_12345678901", true),
            ("field", "customField_1", true),
            ("field", &field_64, true),
            ("field", &field_65, false),
            ("field", "Summary", false),
            ("field", "1abc", false),
            ("field", "a-b", false),
            ("field", "", false),
            ("delivery", "c9d0d0d6-8c5a-4b37-a9a4-e4f4b5e3b1f1", true),
            ("delivery", &token_200, true),
            ("delivery", &token_201, false),
            ("delivery", "a b", false),
            ("delivery", "a\u{7f}", false),
            ("delivery", "\u{e9}", false),
            ("delivery", "", false),
            ("token", "abc", true),
            ("token", "", false),
            ("idem", "order-42", true),
            ("idem", &token_200, true),
            ("idem", &token_201, false),
            ("idem", "a\tb", false),
            ("idem", "", false),
        ];
        for (what, input, expected) in rows {
            assert_eq!(check(what, input), expected, "{what} {input:?}");
        }
    }

    #[test]
    fn issue_key_parts() {
        let key = IssueKey::parse("OPS_2-42").unwrap();
        assert_eq!(key.project().as_str(), "OPS_2");
        assert_eq!(key.number(), 42);
        let lenient = IssueKey::from_response("lower-7".to_owned());
        assert_eq!(lenient.as_str(), "lower-7");
        assert_eq!(lenient.project().as_str(), "lower");
        assert_eq!(lenient.number(), 7);
        assert_eq!(IssueKey::from_response("odd".to_owned()).number(), 0);
    }

    #[test]
    fn issue_ref_picks_by_shape() {
        let id = IssueId::parse("10001").unwrap();
        let key = IssueKey::parse("OPS-1").unwrap();
        assert_eq!(IssueRef::parse("10001").unwrap(), IssueRef::Id(id.clone()));
        assert_eq!(
            IssueRef::parse("OPS-1").unwrap(),
            IssueRef::Key(key.clone())
        );
        assert_eq!(IssueRef::from(&id).path_segment(), "10001");
        assert_eq!(IssueRef::from(key.clone()).to_string(), "OPS-1");
        assert_eq!(IssueRef::from(&key), IssueRef::from(key));
        assert_eq!(IssueRef::from(id.clone()), IssueRef::from(&id));
        assert_eq!("OPS-1".parse::<IssueRef>().unwrap().path_segment(), "OPS-1");
    }

    #[test]
    fn deserialize_is_strict() {
        assert!(serde_json::from_str::<IssueKey>("\"OPS-1\"").is_ok());
        assert!(serde_json::from_str::<IssueKey>("\"../x\"").is_err());
        assert!(serde_json::from_str::<IssueKey>("\"lower-7\"").is_err());
        assert!(serde_json::from_str::<ProjectKey>("\"ops\"").is_err());
        assert!(serde_json::from_str::<IssueId>("\"0\"").is_err());
        assert!(serde_json::from_str::<IssueId>("10001").is_err());
        assert!(serde_json::from_str::<IssueRef>("\"OPS-1\"").is_ok());
        assert!(serde_json::from_str::<IssueRef>("\"a/b\"").is_err());
        assert!(serde_json::from_str::<AccountId>("\"a b\"").is_err());
        assert!(serde_json::from_str::<CloudId>("\"x\"").is_err());
        assert!(serde_json::from_str::<SiteName>("\"a.b\"").is_err());
        assert!(serde_json::from_str::<StatusCategoryKey>("\"Done\"").is_err());
        assert!(serde_json::from_str::<FieldId>("\"-\"").is_err());
        assert!(serde_json::from_str::<WebhookDeliveryId>("\"\"").is_err());
        assert!(serde_json::from_str::<PageToken>("\"\"").is_err());
        assert!(serde_json::from_str::<IdempotencyKey>("\"a b\"").is_err());
        let key: IssueKey = serde_json::from_str("\"OPS-9\"").unwrap();
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"OPS-9\"");
        let category: StatusCategoryKey = serde_json::from_str("\"done\"").unwrap();
        assert_eq!(category, StatusCategoryKey::DONE);
        assert_eq!(serde_json::to_string(&category).unwrap(), "\"done\"");
        let r: IssueRef = serde_json::from_str("\"10001\"").unwrap();
        assert_eq!(serde_json::to_string(&r).unwrap(), "\"10001\"");
        let idem: IdempotencyKey = serde_json::from_str("\"k-1\"").unwrap();
        assert_eq!(serde_json::to_string(&idem).unwrap(), "\"k-1\"");
    }

    #[test]
    fn errors_do_not_echo_the_input() {
        let err = IssueKey::parse("SECRET value").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
        assert!(!err.to_string().contains("SECRET"), "{err}");
    }

    #[test]
    fn category_constants_and_lenient_replies() {
        assert_eq!(StatusCategoryKey::NEW.as_str(), "new");
        assert_eq!(
            StatusCategoryKey::INDETERMINATE.to_string(),
            "indeterminate"
        );
        assert_eq!(
            "done".parse::<StatusCategoryKey>().unwrap(),
            StatusCategoryKey::DONE
        );
        let odd = StatusCategoryKey::from_response("Odd Key".to_owned());
        assert_eq!(odd.as_str(), "Odd Key");
    }

    #[test]
    fn page_token_debug_shows_the_length_only() {
        let token = PageToken::parse("secret-ish-token").unwrap();
        assert_eq!(format!("{token:?}"), "PageToken(16 chars)");
        assert_eq!(token.to_string(), "secret-ish-token");
    }

    #[test]
    fn idempotency_markers() {
        let key = IdempotencyKey::parse("order-42").unwrap();
        let label = key.marker_label("autumn-idem-");
        // Pinned: lower base32 of SHA-256("order-42"), first 26 chars.
        assert_eq!(label, "autumn-idem-hp4lcv6eeoho7znojjtozka4no");
        assert_eq!(label.len(), "autumn-idem-".len() + 26);
        let marker = key.comment_marker();
        assert_eq!(&marker[..12], "autumn-ref: ");
        assert_eq!(&marker[12..], &label[12..]);
        assert!(
            marker[12..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        );
        assert_eq!(format!("{key:?}"), "IdempotencyKey(\"order-42\")");
        assert_eq!(key.to_string(), "order-42");
        assert_eq!("order-42".parse::<IdempotencyKey>().unwrap(), key);
    }

    fn any_input() -> impl Strategy<Value = String> {
        prop_oneof![
            any::<String>(),
            "[A-Z][A-Z0-9_]{0,10}(-[0-9]{0,11})?",
            "[0-9]{0,21}",
            "[a-z0-9:_-]{0,130}",
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            "(customfield_)?[a-zA-Z0-9_]{0,66}",
        ]
    }

    proptest! {
        #[test]
        fn display_round_trip_proptest(input in any_input()) {
            macro_rules! round_trip {
                ($($t:ty),*) => {$(
                    if let Ok(v) = <$t>::parse(&input) {
                        prop_assert_eq!(<$t>::parse(&v.to_string()).unwrap(), v.clone());
                        prop_assert_eq!(v.as_str(), input.as_str());
                        let json = serde_json::to_string(&v).unwrap();
                        prop_assert_eq!(serde_json::from_str::<$t>(&json).unwrap(), v);
                    }
                )*};
            }
            round_trip!(ProjectKey, IssueKey, IssueId, ProjectId, StatusId, CommentId,
                TransitionId, IssueTypeId, AccountId, CloudId, SiteName, StatusCategoryKey,
                FieldId, WebhookDeliveryId, PageToken, IdempotencyKey);
            if let Ok(r) = IssueRef::parse(&input) {
                prop_assert_eq!(IssueRef::parse(&r.to_string()).unwrap(), r);
            }
        }

        #[test]
        fn no_panic_proptest(input in any_input()) {
            for what in ["project", "issue_key", "issue_id", "issue_ref", "account", "cloud",
                "site", "category", "field", "delivery", "token", "idem"] {
                let _ = check(what, &input);
            }
            if let Ok(key) = IssueKey::parse(&input) {
                prop_assert!(key.number() > 0);
                prop_assert!(ProjectKey::parse(key.project().as_str()).is_ok());
            }
            let _ = IssueKey::from_response(input.clone()).number();
            let _ = IssueKey::from_response(input).project();
        }

        #[test]
        fn a_valid_key_never_holds_a_path_char(input in any_input()) {
            if let Ok(r) = IssueRef::parse(&input) {
                prop_assert!(!r.path_segment().contains(['/', '.', '?', '#', '%', ' ']));
            }
        }
    }
}

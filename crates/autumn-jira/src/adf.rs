//! Atlassian Document Format (ADF): the rich text of descriptions and
//! comments in REST v3.
//!
//! An `Adf` value always has a valid root: `{"type": "doc", "version": 1,
//! "content": [...]}`. The builders never make an empty text node, because
//! Jira can reject one (F38).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::error::{JiraError, Result};

/// An ADF document with a valid `doc` root, version 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Adf(Value);

impl Adf {
    /// Makes a document with one paragraph per line. An empty line gives an
    /// empty paragraph, never an empty text node.
    #[must_use]
    pub fn text(text: &str) -> Self {
        let _ = text;
        Self(json!({}))
    }

    /// Starts a builder.
    #[must_use]
    pub fn builder() -> AdfBuilder {
        AdfBuilder::default()
    }

    /// Returns the plain text. Blocks join with `\n`. An unknown node gives
    /// its text.
    #[must_use]
    pub fn to_plain_text(&self) -> String {
        String::new()
    }

    /// Returns `true` when the plain text holds `needle`.
    #[must_use]
    pub fn contains_text(&self, needle: &str) -> bool {
        self.to_plain_text().contains(needle)
    }

    /// Adds one paragraph per line of `text` at the end.
    #[must_use]
    pub fn with_paragraph(self, text: &str) -> Self {
        let _ = text;
        self
    }

    /// Returns the JSON value.
    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.0
    }

    /// Checks the root and wraps a JSON value.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when the root is not `{"type": "doc",
    /// "version": 1, "content": [...]}`.
    pub fn from_value(value: Value) -> Result<Self> {
        let _ = value;
        Err(JiraError::invalid("adf", "not ready"))
    }
}

impl Serialize for Adf {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Adf {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(serde::de::Error::custom)
    }
}

/// The marks of one span.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mark {
    Plain,
    Strong,
    Em,
    Code,
    Link(String),
}

/// One run of inline text with one mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    text: String,
    mark: Mark,
}

impl Span {
    /// Plain text.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            mark: Mark::Plain,
        }
    }

    /// Bold text.
    #[must_use]
    pub fn strong(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            mark: Mark::Strong,
        }
    }

    /// Italic text.
    #[must_use]
    pub fn em(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            mark: Mark::Em,
        }
    }

    /// Inline code.
    #[must_use]
    pub fn code(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            mark: Mark::Code,
        }
    }

    /// A link. `build` fails when `url` does not start with `https://`.
    #[must_use]
    pub fn link(text: &str, url: &str) -> Self {
        Self {
            text: text.to_owned(),
            mark: Mark::Link(url.to_owned()),
        }
    }
}

/// Builds an ADF document block by block.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct AdfBuilder {
    content: Vec<Value>,
    error: Option<&'static str>,
}

impl AdfBuilder {
    /// Adds a paragraph. A `\n` gives a hard break.
    pub fn paragraph(self, text: &str) -> Self {
        let _ = text;
        self
    }

    /// Adds a paragraph of spans.
    pub fn paragraph_spans(self, spans: &[Span]) -> Self {
        let _ = spans;
        self
    }

    /// Adds a heading. `level` is 1 to 6, else `build` fails.
    pub fn heading(self, level: u8, text: &str) -> Self {
        let _ = (level, text);
        self
    }

    /// Adds a bullet list with one item for each entry.
    pub fn bullet_list(self, items: &[&str]) -> Self {
        let _ = items;
        self
    }

    /// Adds an ordered list with one item for each entry.
    pub fn ordered_list(self, items: &[&str]) -> Self {
        let _ = items;
        self
    }

    /// Adds a code block.
    pub fn code_block(self, language: Option<&str>, text: &str) -> Self {
        let _ = (language, text);
        self
    }

    /// Adds a horizontal rule.
    pub fn rule(self) -> Self {
        self
    }

    /// Returns the document.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for a heading level out of 1 to 6, or a link
    /// that is not `https://`.
    pub fn build(self) -> Result<Adf> {
        let _ = (self.content, self.error);
        Err(JiraError::invalid("adf", "not ready"))
    }
}

/// Keeps `Map` in use for the GREEN code.
#[allow(dead_code, reason = "the GREEN code uses it")]
fn empty_map() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Returns `true` when the tree holds an empty text node.
    fn has_empty_text(value: &Value) -> bool {
        match value {
            Value::Object(map) => {
                (map.get("type") == Some(&json!("text"))
                    && map
                        .get("text")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty))
                    || map.values().any(has_empty_text)
            }
            Value::Array(items) => items.iter().any(has_empty_text),
            _ => false,
        }
    }

    #[test]
    fn text_splits_lines() {
        let adf = Adf::text("first\nsecond");
        assert_eq!(
            adf.as_value(),
            &json!({"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "first"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "second"}]},
            ]})
        );
        assert_eq!(adf.to_plain_text(), "first\nsecond");
        assert!(adf.contains_text("cond"));
        assert!(!adf.contains_text("third"));
    }

    #[test]
    fn no_empty_text_nodes() {
        for text in ["", "\n", "a\n\nb", "\n\n\n", "x\n"] {
            let adf = Adf::text(text);
            assert!(!has_empty_text(adf.as_value()), "{text:?}");
            assert_eq!(adf.to_plain_text(), text);
        }
        assert_eq!(
            Adf::text("").as_value(),
            &json!({"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": []}]})
        );
        let built = Adf::builder()
            .paragraph("")
            .paragraph_spans(&[Span::strong(""), Span::text("x")])
            .heading(2, "")
            .bullet_list(&["", "y"])
            .code_block(None, "")
            .build()
            .unwrap();
        assert!(!has_empty_text(built.as_value()), "{:?}", built.as_value());
        let marked = Adf::text("hello").with_paragraph("autumn-ref: abc");
        assert!(!has_empty_text(marked.as_value()));
        assert_eq!(marked.to_plain_text(), "hello\nautumn-ref: abc");
    }

    #[test]
    fn builder_nodes() {
        let adf = Adf::builder()
            .heading(1, "Alert")
            .paragraph("line one\nline two")
            .paragraph_spans(&[
                Span::text("see "),
                Span::strong("bold"),
                Span::em("it"),
                Span::code("x()"),
                Span::link("docs", "https://example.com/a"),
            ])
            .bullet_list(&["a", "b"])
            .ordered_list(&["one"])
            .code_block(Some("rust"), "fn main() {}")
            .rule()
            .build()
            .unwrap();
        let content = adf.as_value()["content"].as_array().unwrap();
        assert_eq!(content.len(), 7);
        assert_eq!(
            content[0],
            json!({"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Alert"}]})
        );
        assert_eq!(
            content[1],
            json!({"type": "paragraph", "content": [
                {"type": "text", "text": "line one"},
                {"type": "hardBreak"},
                {"type": "text", "text": "line two"},
            ]})
        );
        assert_eq!(
            content[2]["content"],
            json!([
                {"type": "text", "text": "see "},
                {"type": "text", "text": "bold", "marks": [{"type": "strong"}]},
                {"type": "text", "text": "it", "marks": [{"type": "em"}]},
                {"type": "text", "text": "x()", "marks": [{"type": "code"}]},
                {"type": "text", "text": "docs", "marks": [{"type": "link", "attrs": {"href": "https://example.com/a"}}]},
            ])
        );
        assert_eq!(
            content[3],
            json!({"type": "bulletList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "a"}]}]},
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "b"}]}]},
            ]})
        );
        assert_eq!(content[4]["type"], "orderedList");
        assert_eq!(
            content[5],
            json!({"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn main() {}"}]})
        );
        assert_eq!(content[6], json!({"type": "rule"}));
        assert_eq!(
            adf.to_plain_text(),
            "Alert\nline one\nline two\nsee bolditx()docs\na\nb\none\nfn main() {}\n"
        );
        assert!(Adf::builder().heading(0, "x").build().is_err());
        assert!(Adf::builder().heading(7, "x").build().is_err());
        assert!(Adf::builder().heading(6, "x").build().is_ok());
        let plain = Adf::builder().code_block(None, "x").build().unwrap();
        assert_eq!(
            plain.as_value()["content"][0],
            json!({"type": "codeBlock", "content": [{"type": "text", "text": "x"}]})
        );
    }

    #[test]
    fn links_need_https() {
        for bad in [
            "http://example.com",
            "javascript:alert(1)",
            "ftp://x",
            "https://",
            "https://a b",
            "",
        ] {
            let result = Adf::builder()
                .paragraph_spans(&[Span::link("x", bad)])
                .build();
            assert!(result.is_err(), "{bad}");
        }
        assert!(
            Adf::builder()
                .paragraph_spans(&[Span::link("x", "https://a.example/b?c=d")])
                .build()
                .is_ok()
        );
    }

    #[test]
    fn from_value_checks_the_root() {
        let good = json!({"type": "doc", "version": 1, "content": []});
        assert_eq!(Adf::from_value(good.clone()).unwrap().as_value(), &good);
        for bad in [
            json!(null),
            json!("text"),
            json!({"type": "paragraph", "version": 1, "content": []}),
            json!({"type": "doc", "version": 2, "content": []}),
            json!({"type": "doc", "version": 1}),
            json!({"type": "doc", "version": 1, "content": {}}),
        ] {
            assert!(Adf::from_value(bad.clone()).is_err(), "{bad}");
        }
        let parsed: Adf = serde_json::from_value(good.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), good);
        assert!(serde_json::from_value::<Adf>(json!({"type": "x"})).is_err());
    }

    #[test]
    fn plain_text_of_unknown_and_inline_nodes() {
        let adf = Adf::from_value(json!({"type": "doc", "version": 1, "content": [
            {"type": "panel", "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "hi "},
                    {"type": "mention", "attrs": {"id": "1", "text": "@mia"}},
                    {"type": "emoji", "attrs": {"shortName": ":smile:"}},
                    {"type": "inlineCard", "attrs": {"url": "https://x"}}
                ]}
            ]},
            {"type": "blockquote", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "quote"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "two"}]}
            ]},
            {"type": "mystery", "text": "odd"},
            "not a node"
        ]}))
        .unwrap();
        assert_eq!(adf.to_plain_text(), "hi @mia\nquote\ntwo\nodd\n");
    }

    proptest! {
        #[test]
        fn plain_text_round_trip_proptest(text in any::<String>()) {
            let adf = Adf::text(&text);
            prop_assert_eq!(adf.to_plain_text(), text.clone());
            prop_assert!(!has_empty_text(adf.as_value()));
            prop_assert!(adf.contains_text(&text));
            let back = Adf::from_value(adf.as_value().clone()).unwrap();
            prop_assert_eq!(back, adf);
        }
    }
}

//! What the client writes to Jira.
//!
//! The README section "What the plugin writes to your Jira" has one row
//! for each entry of `WRITES`. A guard test compares them (G20, R5).

/// The kind of a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// A label on an issue.
    Label,
    /// An issue property.
    IssueProperty,
    /// Text in a comment.
    CommentText,
}

/// One thing that the client writes to Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JiraWrite {
    /// The label, the property key or the text.
    pub what: &'static str,
    /// The kind.
    pub kind: WriteKind,
    /// The call that writes it.
    pub when: &'static str,
}

/// Each thing that the client and its adapters write to Jira.
pub const WRITES: &[JiraWrite] = &[
    JiraWrite {
        what: "autumn-idem-<26 chars>",
        kind: WriteKind::Label,
        when: "create_issue_idempotent",
    },
    JiraWrite {
        what: "autumn-jira",
        kind: WriteKind::IssueProperty,
        when: "create_issue_idempotent",
    },
    JiraWrite {
        what: "autumn-ref: <26 chars>",
        kind: WriteKind::CommentText,
        when: "add_comment_idempotent",
    },
    JiraWrite {
        what: "autumn-tracked",
        kind: WriteKind::Label,
        when: "harvest route or watch",
    },
    JiraWrite {
        what: "autumn-jira-routes",
        kind: WriteKind::IssueProperty,
        when: "harvest route or watch",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IdempotencyKey;

    #[test]
    fn writes_table() {
        let rows: Vec<(&str, WriteKind, &str)> =
            WRITES.iter().map(|w| (w.what, w.kind, w.when)).collect();
        assert_eq!(
            rows,
            [
                (
                    "autumn-idem-<26 chars>",
                    WriteKind::Label,
                    "create_issue_idempotent"
                ),
                (
                    "autumn-jira",
                    WriteKind::IssueProperty,
                    "create_issue_idempotent"
                ),
                (
                    "autumn-ref: <26 chars>",
                    WriteKind::CommentText,
                    "add_comment_idempotent"
                ),
                ("autumn-tracked", WriteKind::Label, "harvest route or watch"),
                (
                    "autumn-jira-routes",
                    WriteKind::IssueProperty,
                    "harvest route or watch"
                ),
            ]
        );
    }

    #[test]
    fn writes_match_the_markers() {
        let key = IdempotencyKey::parse("k").unwrap();
        let label = key.marker_label("autumn-idem-");
        let marker = key.comment_marker();
        let label_row = WRITES[0].what.replace("<26 chars>", "");
        let marker_row = WRITES[2].what.replace("<26 chars>", "");
        assert_eq!(&label[..label_row.len()], label_row);
        assert_eq!(label.len() - label_row.len(), 26);
        assert_eq!(&marker[..marker_row.len()], marker_row);
        assert_eq!(marker.len() - marker_row.len(), 26);
    }
}

//! Activity inputs and outputs.
//!
//! Each input holds ids and options only. It never holds a secret or a body.
//! The `#[activity]` functions come with the client (WP5 part H1). This
//! module already holds the input of `jira_get_issue`, because the durable
//! wait calls that activity.

use std::time::Duration;

use autumn_harvest::policy::RetryPolicy;
use autumn_jira::IssueRef;
use autumn_jira::ids::FieldId;
use serde::{Deserialize, Serialize};

use crate::clients::ClientRef;

/// The name of the activity that reads one issue.
pub const GET_ISSUE: &str = "jira_get_issue";

/// The `start_to_close` of a read activity: 3 minutes (plan 7.2, B2).
pub const READ_START_TO_CLOSE: Duration = Duration::from_secs(180);

/// The retry policy of `jira_get_issue`: 5 attempts, from 1 s.
#[must_use]
pub fn get_issue_retry() -> RetryPolicy {
    RetryPolicy::exponential(5, Duration::from_secs(1))
}

/// The input of `jira_get_issue`. The output is an
/// [`autumn_jira::model::IssueSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetIssueInput {
    /// The client to use.
    #[serde(default)]
    pub client: ClientRef,
    /// The issue, by key or by id.
    pub issue: IssueRef,
    /// The fields to read. `None` reads the default field set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldId>>,
}

impl GetIssueInput {
    /// Reads `issue` with the default client and the default fields.
    #[must_use]
    pub fn new(issue: impl Into<IssueRef>) -> Self {
        Self {
            client: ClientRef::Default,
            issue: issue.into(),
            fields: None,
        }
    }

    /// Uses `client`.
    #[must_use]
    pub fn client(mut self, client: ClientRef) -> Self {
        self.client = client;
        self
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;
    use autumn_jira::IssueId;
    use serde_json::json;

    #[test]
    fn get_issue_input_shape() {
        let input = GetIssueInput::new(IssueId::parse("10001").unwrap())
            .client(ClientRef::Named { name: "ops".into() });
        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(
            value,
            json!({"client": {"type": "named", "name": "ops"}, "issue": "10001"})
        );
        let back: GetIssueInput = serde_json::from_value(json!({"issue": "OPS-1"})).unwrap();
        assert_eq!(back.client, ClientRef::Default);
        assert_eq!(back.fields, None);
        assert_eq!(get_issue_retry().max_attempts, 5);
    }
}

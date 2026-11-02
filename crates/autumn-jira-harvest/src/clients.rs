//! Client references: which Jira client an activity uses.
//!
//! A [`ClientRef`] is plain data, so it goes into an activity input and into
//! the workflow history. It never holds a secret.

use serde::{Deserialize, Serialize};

/// Which client an activity uses. Serializable, so it sits in activity input (Y4).
///
/// `Default` and `Named` read the `JiraClients` state of the worker.
/// `Connection` reads the `SanMateo` state and uses one san-mateo
/// connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRef {
    /// The default client.
    #[default]
    Default,
    /// A client that the app registered with a name.
    Named {
        /// The name.
        name: String,
    },
    /// A client for one san-mateo connection.
    Connection {
        /// The san-mateo environment.
        environment: String,
        /// The san-mateo integration (provider config key).
        integration: String,
        /// The san-mateo connection id.
        connection_id: String,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serde_shape_is_tagged() {
        assert_eq!(
            serde_json::to_value(ClientRef::default()).unwrap(),
            json!({"type": "default"})
        );
        let named = ClientRef::Named { name: "ops".into() };
        assert_eq!(
            serde_json::to_value(&named).unwrap(),
            json!({"type": "named", "name": "ops"})
        );
        let connection = ClientRef::Connection {
            environment: "prod".into(),
            integration: "jira".into(),
            connection_id: "c-1".into(),
        };
        let value = serde_json::to_value(&connection).unwrap();
        assert_eq!(
            value,
            json!({"type": "connection", "environment": "prod", "integration": "jira", "connection_id": "c-1"})
        );
        assert_eq!(
            serde_json::from_value::<ClientRef>(value).unwrap(),
            connection
        );
    }
}

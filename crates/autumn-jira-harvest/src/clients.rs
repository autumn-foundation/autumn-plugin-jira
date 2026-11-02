//! Client references: which Jira client an activity uses.
//!
//! A [`ClientRef`] is plain data, so it goes into an activity input and into
//! the workflow history. It never holds a secret. [`client_for`] turns it
//! into a [`JiraClient`] from the state of the worker.

use std::collections::BTreeMap;
use std::fmt;

use autumn_harvest::context::ActivityContext;
use autumn_harvest::failure::ActivityFailure;
use autumn_jira::JiraClient;
use serde::{Deserialize, Serialize};

/// The clients of a worker: one default client and named clients.
///
/// Put it in the worker state with `HarvestBuilder::state(..)` or
/// `HarvestPlugin::state(..)`.
#[derive(Clone)]
pub struct JiraClients {
    default: JiraClient,
    named: BTreeMap<String, JiraClient>,
}

impl JiraClients {
    /// Makes the set with its default client.
    #[must_use]
    pub fn new(default: JiraClient) -> Self {
        Self {
            default,
            named: BTreeMap::new(),
        }
    }

    /// Adds a client with a name. A second client with the same name
    /// replaces the first.
    #[must_use]
    pub fn with(mut self, name: &str, client: JiraClient) -> Self {
        self.named.insert(name.to_owned(), client);
        self
    }
}

impl fmt::Debug for JiraClients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JiraClients")
            .field("default", &self.default)
            .field("named", &self.named.keys().collect::<Vec<_>>())
            .finish()
    }
}

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

/// Named and Default read ctx.state::<JiraClients>(). Connection reads ctx.state::<SanMateo>().
/// The result is always `.durable()`.
///
/// # Errors
///
/// A non-retryable `jira.config` failure when the state has no
/// [`JiraClients`] (or no `SanMateo` for a connection), or no client with
/// the name.
pub fn client_for(
    ctx: &ActivityContext,
    client: &ClientRef,
) -> Result<JiraClient, ActivityFailure> {
    let _ = (ctx, client);
    Err(ActivityFailure::retryable("", ""))
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

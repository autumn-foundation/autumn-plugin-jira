//! The `jira:` camel component and its endpoint.

use std::collections::BTreeMap;

use async_trait::async_trait;
use autumn_jira::JiraClient;
use camel_api::CamelError;
use camel_component_api::{Component, ComponentContext, Endpoint};

use crate::SCHEME;
use crate::options::EndpointOptions;

/// The `jira:` camel component. It is producer-only.
#[derive(Debug, Clone)]
pub struct JiraComponent {
    clients: BTreeMap<String, JiraClient>,
}

impl JiraComponent {
    /// Makes a component with the `default` client.
    #[must_use]
    pub fn new(default: JiraClient) -> Self {
        let mut clients = BTreeMap::new();
        clients.insert("default".to_owned(), default);
        Self { clients }
    }

    /// Adds a named client.
    #[must_use]
    pub fn connection(mut self, name: &str, client: JiraClient) -> Self {
        self.clients.insert(name.to_owned(), client);
        self
    }

    /// Parses and checks the options of a `jira:` URI.
    ///
    /// # Errors
    ///
    /// K1 RED stub.
    pub fn endpoint_options(&self, _uri: &str) -> Result<EndpointOptions, CamelError> {
        Err(CamelError::EndpointCreationFailed("K1 RED stub".to_owned()))
    }
}

#[async_trait]
impl Component for JiraComponent {
    fn scheme(&self) -> &str {
        SCHEME
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Err(CamelError::EndpointCreationFailed("K1 RED stub".to_owned()))
    }
}

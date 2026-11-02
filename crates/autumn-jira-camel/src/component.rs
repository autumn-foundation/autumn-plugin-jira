//! The `jira:` camel component and its endpoint.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use autumn_jira::JiraClient;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError};
use camel_component_api::{
    Component, ComponentCapabilities, ComponentContext, ComponentMetadata, Consumer, Endpoint,
    ProducerContext, RuntimeObservability,
};

use crate::SCHEME;
use crate::options::{EndpointOptions, uri_options};
use crate::producer::Producer;

/// The name of the client that [`JiraComponent::new`] takes.
const DEFAULT_CONNECTION: &str = "default";

/// The `jira:` camel component. It is producer-only.
///
/// The component holds one client for each connection name: `default`
/// and the names of [`JiraComponent::connection`]. The `connection` URI
/// option picks one. For a route that a harvest activity calls, register
/// `client.durable()`: then harvest is the one layer that retries. Do not
/// give such a route a camel redelivery. A camel redelivery of a marked
/// write does a lookup only, and a redelivery of an unmarked write fails.
///
/// ```no_run
/// # fn demo(client: autumn_jira::JiraClient, eu: autumn_jira::JiraClient) {
/// let component = autumn_jira_camel::JiraComponent::new(client).connection("eu", eu);
/// # let _ = component;
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct JiraComponent {
    clients: BTreeMap<String, JiraClient>,
}

impl JiraComponent {
    /// Makes a component with the `default` client.
    #[must_use]
    pub fn new(default: JiraClient) -> Self {
        let mut clients = BTreeMap::new();
        clients.insert(DEFAULT_CONNECTION.to_owned(), default);
        Self { clients }
    }

    /// Adds a client with a connection name. A second call with the same
    /// name replaces the client.
    #[must_use]
    pub fn connection(mut self, name: &str, client: JiraClient) -> Self {
        self.clients.insert(name.to_owned(), client);
        self
    }

    /// Parses and checks the options of a `jira:` URI.
    ///
    /// # Errors
    ///
    /// [`CamelError::InvalidUri`] for a URI problem, for example an unknown
    /// option, a bad value or a missing required option.
    /// [`CamelError::Config`] when no client has the `connection` name.
    pub fn endpoint_options(&self, uri: &str) -> Result<EndpointOptions, CamelError> {
        let names: Vec<&str> = self.clients.keys().map(String::as_str).collect();
        EndpointOptions::parse(uri, &names)
    }
}

#[async_trait]
impl Component for JiraComponent {
    fn scheme(&self) -> &str {
        SCHEME
    }

    fn metadata(&self) -> ComponentMetadata {
        let mut metadata = ComponentMetadata::minimal(SCHEME)
            .with_description(
                "Creates, reads, edits, moves, comments on and searches Jira Cloud issues.",
            )
            .with_capabilities(ComponentCapabilities {
                supports_producer: true,
                ..ComponentCapabilities::default()
            })
            .with_uri_options(uri_options());
        env!("CARGO_PKG_VERSION").clone_into(&mut metadata.version);
        "jira:operation".clone_into(&mut metadata.uri_syntax);
        metadata
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        // Every option is checked here, at the route add, not at the first
        // message.
        let options = self.endpoint_options(uri)?;
        let client = self
            .clients
            .get(&options.connection)
            .cloned()
            .ok_or_else(|| {
                CamelError::Config(format!("no jira connection '{}'", options.connection))
            })?;
        Ok(Box::new(JiraEndpoint {
            uri: uri.to_owned(),
            producer: Producer::new(client, Arc::new(options)),
        }))
    }
}

/// One `jira:` endpoint.
struct JiraEndpoint {
    uri: String,
    producer: Producer,
}

impl Endpoint for JiraEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(format!(
            "{SCHEME}: is producer-only; use it in to(...), not in from(...)"
        )))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let producer = self.producer.clone();
        Ok(BoxProcessor::from_fn(move |exchange| {
            let producer = producer.clone();
            async move { producer.send(exchange).await }
        }))
    }
}

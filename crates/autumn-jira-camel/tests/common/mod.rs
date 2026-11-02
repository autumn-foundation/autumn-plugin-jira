//! Test harness: a `FakeJira`, a client on it, and a camel context with
//! `direct:` routes to `jira:` endpoints.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::result_large_err,
    reason = "test helpers; each test file uses a part of them; CamelError is large"
)]

use std::sync::Arc;

use autumn_jira::client::ClientSettings;
use autumn_jira::ids::SiteName;
use autumn_jira::san_mateo::Secret;
use autumn_jira::san_mateo::transport::HttpTransport;
use autumn_jira::{BasicAuth, JiraClient, JiraSite};
use autumn_jira_camel::JiraComponent;
use autumn_jira_fake::FakeJira;
use camel_api::{Body, BoxProcessor, CamelError, Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use tower::ServiceExt;

/// The harvest-camel header with the activity idempotency key.
pub const KEY: &str = "HarvestIdempotencyKey";
/// The harvest-camel header with the 1-based activity attempt.
pub const ATTEMPT: &str = "HarvestActivityAttempt";

/// A durable client on the fake, with short waits.
pub fn client(fake: &FakeJira) -> JiraClient {
    client_on(Arc::new(fake.clone()))
}

/// A durable client of the fake site on `transport`, with short waits.
pub fn client_on(transport: Arc<dyn HttpTransport>) -> JiraClient {
    let site = JiraSite::Subdomain(SiteName::parse(autumn_jira_fake::DEFAULT_SITE).unwrap());
    let auth = BasicAuth::new(
        autumn_jira_fake::DEFAULT_EMAIL,
        Secret::new(autumn_jira_fake::DEFAULT_API_TOKEN),
    )
    .unwrap();
    let mut settings = ClientSettings::for_durable_host();
    settings.base_backoff_ms = 10;
    settings.max_backoff_ms = 100;
    settings.lookup_delay_ms = 10;
    JiraClient::api_token(site, auth, transport).with_settings(settings)
}

/// A component with the default client on the fake.
pub fn component(fake: &FakeJira) -> JiraComponent {
    JiraComponent::new(client(fake))
}

/// Starts a camel context with one route for each `(from, to)` pair.
///
/// # Errors
///
/// The error of the route add, for example a bad `jira:` URI.
pub async fn start(
    component: JiraComponent,
    routes: &[(&str, &str)],
) -> Result<CamelContext, CamelError> {
    let mut camel = CamelContext::builder().build().await?;
    camel.register_component(DirectComponent::new());
    camel.register_component(component);
    for (n, (from, to)) in routes.iter().enumerate() {
        let route = RouteBuilder::from(from)
            .route_id(format!("route-{n}"))
            .to(*to)
            .build()?;
        camel.add_route_definition(route).await?;
    }
    camel.start().await?;
    Ok(camel)
}

/// Starts a camel context with one route `direct:in` to `to`. The route
/// has a camel redelivery: `retries` more tries with a 1 ms wait, for each
/// error that `matches` accepts.
///
/// # Errors
///
/// The error of the route add, for example a bad `jira:` URI.
pub async fn start_redelivering(
    component: JiraComponent,
    to: &str,
    retries: u32,
    matches: impl Fn(&CamelError) -> bool + Send + Sync + 'static,
) -> Result<CamelContext, CamelError> {
    let mut camel = CamelContext::builder().build().await?;
    camel.register_component(DirectComponent::new());
    camel.register_component(component);
    let wait = std::time::Duration::from_millis(1);
    let handler = camel_api::ErrorHandlerConfig::log_only()
        .on_exception(matches)
        .retry(retries)
        .with_backoff(wait, 1.0, wait)
        .build();
    let route = RouteBuilder::from("direct:in")
        .route_id("route-redelivery")
        .error_handler(handler)
        .to(to)
        .build()?;
    camel.add_route_definition(route).await?;
    camel.start().await?;
    Ok(camel)
}

/// Starts a camel context with `direct:in` to `to`.
pub async fn start_one(component: JiraComponent, to: &str) -> Result<CamelContext, CamelError> {
    start(component, &[("direct:in", to)]).await
}

/// Sends one message to `from` and waits for the result. An exchange with
/// an error gives that error.
pub async fn send(
    camel: &CamelContext,
    from: &str,
    message: Message,
) -> Result<Exchange, CamelError> {
    let direct = camel.registry().get_or_err("direct")?;
    let endpoint = direct.create_endpoint(from, camel)?;
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
    let producer: BoxProcessor = endpoint.create_producer(rt, &camel.producer_context())?;
    let mut exchange = producer.oneshot(Exchange::new(message)).await?;
    let error = exchange.error.take();
    error.map_or(Ok(exchange), Err)
}

/// Stops a camel context.
pub async fn stop(mut camel: CamelContext) {
    camel.stop().await.unwrap();
}

/// A message with a JSON body.
pub fn json(value: Value) -> Message {
    Message::new(Body::Json(value))
}

/// A message with a JSON body and headers.
pub fn json_with(value: Value, headers: &[(&str, Value)]) -> Message {
    let mut message = json(value);
    for (name, value) in headers {
        message.set_header(*name, value.clone());
    }
    message
}

/// The JSON reply body of an exchange. The reply is in `input`, and
/// `output` is `None`.
pub fn reply(exchange: &Exchange) -> Value {
    assert!(exchange.output.is_none(), "the reply goes into input");
    match &exchange.input.body {
        Body::Json(value) => value.clone(),
        other => panic!("expected a JSON reply, got {other:?}"),
    }
}

/// A header of the reply.
pub fn header(exchange: &Exchange, name: &str) -> Option<Value> {
    exchange.input.header(name).cloned()
}

/// The message of a `ValidationError`.
pub fn validation(err: &CamelError) -> &str {
    match err {
        CamelError::ValidationError(m) => m,
        other => panic!("expected ValidationError, got {other:?}"),
    }
}

/// The error text of a route add that must fail. camel-core can wrap the
/// endpoint error, but the text keeps the inner message.
pub async fn route_add_error(component: JiraComponent, to: &str) -> String {
    match start_one(component, to).await {
        Ok(camel) => {
            stop(camel).await;
            panic!("the route add of {to} must fail");
        }
        Err(err) => err.to_string(),
    }
}

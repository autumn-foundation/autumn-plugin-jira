//! Example `camel_route`: a camel route that creates a Jira issue and moves
//! it to In Progress.
//!
//! The route is `direct:in` to `jira:createIssue` to
//! `jira:transitionToStatus?status=category:indeterminate`. The exchange
//! has the `HarvestIdempotencyKey` header, as a harvest activity sends it
//! through harvest-camel. The run sends the exchange two times: attempt 1
//! creates and moves the issue, and attempt 2 (a harvest retry) finds the
//! issue by its marker and sends no second write. `check` compares the out
//! headers and the Jira state with exact values. No network: `FakeJira` is
//! the transport.
//!
//! ```text
//! cargo run --locked -p autumn-jira-camel --example camel_route
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the example prints its summary"
)]
#![allow(clippy::result_large_err, reason = "CamelError is 168 bytes")]

use std::collections::BTreeMap;
use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use autumn_jira::client::ClientSettings;
use autumn_jira::ids::SiteName;
use autumn_jira::san_mateo::Secret;
use autumn_jira::{BasicAuth, JiraClient, JiraSite};
use autumn_jira_camel::JiraComponent;
use autumn_jira_fake::{FakeJira, FakeOp};
use camel_api::{Body, BoxProcessor, CamelError, Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use serde_json::json;
use tower::ServiceExt;

/// The longest time that one run can take.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// The create step. The body gives the summary and the labels.
const CREATE: &str = "jira:createIssue?project=OPS&issueType=Task";

/// The move step. The `JiraIssueKey` out header of the create gives the
/// issue, so the step needs `allowHeaderOverrides=true`. Use the flag only
/// on a route that no outside caller can send headers to.
const MOVE: &str =
    "jira:transitionToStatus?status=category:indeterminate&allowHeaderOverrides=true";

/// The idempotency key that harvest-camel sends for one activity.
const ACTIVITY_KEY: &str = "incident-A-1001/open-ticket";

/// What one exchange gave.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    /// The `Jira*` out headers of the exchange.
    headers: BTreeMap<String, Value>,
    /// The reply body of the last step.
    reply: Value,
}

/// What the run saw.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    /// Attempt 1: create and move.
    first: Step,
    /// Attempt 2 with the same key: found and already there.
    second: Step,
    /// The create POSTs that Jira got.
    create_posts: usize,
    /// The transition POSTs that Jira got.
    transition_posts: usize,
    /// The status of the issue at the end.
    status: String,
}

/// A durable client on the fake: the route runs under a harvest activity,
/// so harvest owns the retries.
fn client_for(fake: &FakeJira) -> Result<JiraClient, Box<dyn Error>> {
    let site = JiraSite::Subdomain(SiteName::parse(autumn_jira_fake::DEFAULT_SITE)?);
    let auth = BasicAuth::new(
        autumn_jira_fake::DEFAULT_EMAIL,
        Secret::new(autumn_jira_fake::DEFAULT_API_TOKEN),
    )?;
    let mut settings = ClientSettings::for_durable_host();
    settings.base_backoff_ms = 50;
    settings.lookup_delay_ms = 100;
    Ok(JiraClient::api_token(site, auth, Arc::new(fake.clone())).with_settings(settings))
}

/// Starts camel with the route `direct:in` to `create` to [`MOVE`].
///
/// # Errors
///
/// The error of the route add, for example an unknown `jira:` option.
async fn start(fake: &FakeJira, create: &str) -> Result<CamelContext, Box<dyn Error>> {
    let mut camel = CamelContext::builder().build().await?;
    camel.register_component(DirectComponent::new());
    camel.register_component(JiraComponent::new(client_for(fake)?));
    let route = RouteBuilder::from("direct:in")
        .route_id("open-ticket")
        .to(create)
        // The create reply is not a transition body.
        .set_body(Body::Empty)
        .to(MOVE)
        .build()?;
    camel.add_route_definition(route).await?;
    camel.start().await?;
    Ok(camel)
}

/// Sends one attempt to `direct:in`.
async fn send(camel: &CamelContext, attempt: u32) -> Result<Step, CamelError> {
    let mut message = Message::new(Body::Json(json!({
        "summary": "Disk full on db-1",
        "labels": ["alert"],
    })));
    message.set_header("HarvestIdempotencyKey", ACTIVITY_KEY);
    message.set_header("HarvestActivityAttempt", attempt);
    let direct = camel.registry().get_or_err("direct")?;
    let endpoint = direct.create_endpoint("direct:in", camel)?;
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
    let producer: BoxProcessor = endpoint.create_producer(rt, &camel.producer_context())?;
    let mut exchange: Exchange = producer.oneshot(Exchange::new(message)).await?;
    if let Some(error) = exchange.error.take() {
        return Err(error);
    }
    let headers = exchange
        .input
        .headers
        .iter()
        .filter(|(name, _)| name.starts_with("Jira"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let reply = match exchange.input.body {
        Body::Json(value) => value,
        _ => Value::Null,
    };
    Ok(Step { headers, reply })
}

/// Runs the flow on a fake.
async fn run(fake: &FakeJira) -> Result<Summary, Box<dyn Error>> {
    let mut camel = start(fake, CREATE).await?;
    let result = async {
        let first = send(&camel, 1).await?;
        let second = send(&camel, 2).await?;
        Ok::<_, CamelError>((first, second))
    }
    .await;
    // Stop camel in all cases. The error of the run comes first.
    let stopped = camel.stop().await;
    let (first, second) = result?;
    stopped?;
    let status = fake
        .issue("OPS-1")
        .and_then(|issue| {
            issue["fields"]["status"]["name"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default();
    Ok(Summary {
        first,
        second,
        create_posts: fake.count(FakeOp::CreateIssue),
        transition_posts: fake.count(FakeOp::DoTransition),
        status,
    })
}

/// The `Jira*` headers of a step.
fn headers(outcome: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("JiraIssueKey".to_owned(), json!("OPS-1")),
        ("JiraOutcome".to_owned(), json!(outcome)),
    ])
}

/// The reply of the move step.
fn moved(outcome: &str) -> Value {
    json!({
        "issue": "OPS-1",
        "outcome": outcome,
        "status": {"id": "10001", "name": "In Progress", "category": "indeterminate"},
    })
}

/// The expected summary.
fn expected() -> Summary {
    Summary {
        first: Step {
            headers: headers("moved"),
            reply: moved("moved"),
        },
        second: Step {
            headers: headers("already_there"),
            reply: moved("already_there"),
        },
        create_posts: 1,
        transition_posts: 1,
        status: "In Progress".to_owned(),
    }
}

/// Compares the summary with the expected values.
///
/// # Errors
///
/// A text that starts with `check failed:` and shows both values.
fn check(summary: &Summary) -> Result<(), String> {
    let want = expected();
    if *summary == want {
        Ok(())
    } else {
        Err(format!("check failed: got {summary:?}, want {want:?}"))
    }
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("no runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let fake = FakeJira::new();
    let result = runtime.block_on(async { tokio::time::timeout(RUN_TIMEOUT, run(&fake)).await });
    let summary = match result {
        Ok(Ok(summary)) => summary,
        Ok(Err(e)) => {
            eprintln!("run failed: {e}");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            eprintln!("run failed: no result in {RUN_TIMEOUT:?}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = check(&summary) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    let show = |step: &Step| {
        step.headers
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!("route:          direct:in -> {CREATE} -> {MOVE}");
    println!("attempt 1:      {}", show(&summary.first));
    println!("attempt 2:      {}", show(&summary.second));
    println!("create POSTs:   {}", summary.create_posts);
    println!("transitions:    {}", summary.transition_posts);
    println!("status:         {}", summary.status);
    println!("check: ok");
    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_route_creates_and_moves() {
        let fake = FakeJira::new();
        let summary = tokio::time::timeout(RUN_TIMEOUT, run(&fake))
            .await
            .unwrap()
            .unwrap();
        check(&summary).unwrap();
        // The create carried the marker label of the activity key.
        let labels = fake.issue("OPS-1").unwrap()["fields"]["labels"].to_string();
        assert!(labels.contains("autumn-idem-"), "{labels}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_option_fails_at_route_add() {
        let fake = FakeJira::new();
        let err = start(
            &fake,
            "jira:createIssue?project=OPS&issueType=Task&summary=x",
        )
        .await
        .map(|_| ())
        .expect_err("an unknown option fails at the route add");
        let text = err.to_string();
        assert!(text.contains("unknown jira: option 'summary'"), "{text}");
        assert!(
            text.contains("allowHeaderOverrides"),
            "it lists the names: {text}"
        );
        assert_eq!(fake.requests().len(), 0, "no call");
    }

    #[test]
    fn a_wrong_value_fails_the_check() {
        let mut summary = expected();
        summary.create_posts = 2;
        let err = check(&summary).expect_err("a wrong value fails");
        assert!(err.starts_with("check failed:"), "{err}");
        assert!(check(&expected()).is_ok());
    }
}

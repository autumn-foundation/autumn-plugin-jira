//! The ecosystem example: alert to ticket to resolution.
//!
//! ```text
//! direct:alerts ─► harvest:alerts ─► workflow incident_flow (harvest worker, Postgres)
//!   1. san_mateo_proxy        GET /project/OPS?expand=lead       (san-mateo-harvest, the escape hatch)
//!   2. jira_create_issue      503 after the commit, then found    (autumn-jira-harvest)
//!   3. jira_transition        To Do -> In Progress
//!   4. await_issue_status     worker A stops, worker B resumes; a person resolves the issue,
//!                             FakeJira signs a webhook, the autumn app (JiraPlugin) verifies it,
//!                             and HarvestSignalBridge signals the run
//!   5. jira_add_comment       one comment, with its marker
//!   6. notify_chat            direct:notify ─► jira:getIssue ─► san-mateo:team-chat
//! GET /incidents/OPS-1         the autumn app route (JiraService through san-mateo)
//! ```
//!
//! Each tool has one job. camel: the edges. harvest: the durable flow.
//! autumn-jira-harvest: every Jira write. san-mateo-harvest: one escape
//! hatch. san-mateo: the credentials of Jira (OAuth, refresh, cloud id) and
//! of the chat API (API key). autumn: the webhook door and one app route.
//!
//! `FakeJira` is Jira Cloud and the Atlassian OAuth server. A
//! `MemoryTransport` is the chat API. No test and no run calls a real
//! service. The run prints a timeline of nine lines and checks each line
//! exactly. Each run uses its own queue and ids, so you can run it many
//! times on one database.
//!
//! ```sh
//! DATABASE_URL=postgres://jira:jira@localhost/jira_test \
//!   cargo run --locked --manifest-path examples/ecosystem/Cargo.toml
//! # or, with Docker for Postgres:
//! bash scripts/run-ecosystem.sh
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the example prints its summary"
)]
#![allow(clippy::result_large_err, reason = "CamelError is 168 bytes")]

mod app;
mod routes;
mod summary;
mod transport;
mod workflow;

#[cfg(test)]
mod tests;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use autumn_harvest::diesel;
use autumn_harvest::diesel_async::{AsyncConnection, AsyncPgConnection};
use autumn_harvest::execution::resolve_execution_id_by_workflow_id;
use autumn_harvest::handle::WorkflowHandleClient;
use autumn_harvest::migrate;
use autumn_harvest::pool::{DbRole, EngineDbTimeouts, engine_pool};
use autumn_harvest::prelude::{HarvestBuilder, WorkerConfig};
use autumn_harvest::telemetry::NoOpMetrics;
use autumn_harvest::worker::DbPool;
use autumn_harvest_plugin::api::HarvestApiState;
use autumn_harvest_plugin::connector::{ConnectorRuntime, MappedMessage, SourceBinding};
use autumn_harvest_plugin::{
    HarvestDatabaseConfig, HarvestMode, HarvestRunner, HarvestRunnerResources,
    HarvestRuntimeConfig, OrphanStartupAction,
};
use autumn_jira::JiraClient;
use autumn_jira::san_mateo::clock::{Entropy, OsEntropy};
use autumn_jira::san_mateo::{SanMateo, Secret};
use autumn_jira_camel::JiraComponent;
use autumn_jira_fake::{ANN, BOT, FakeOp, Fault};
use autumn_jira_harvest::{
    BridgeCounters, HarvestSignalBridge, JiraClients, RouteKey, StatusWait, WaitSource,
};
use autumn_plugin_jira::testing::DeliverExt;
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use harvest_camel::{CamelHandle, HarvestBridge, call_route, check_route};
use san_mateo_camel::SanMateoComponent;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::routes::{ALERT_ID, ALERTS, NOTIFY, STREAM, routes};
use crate::summary::{Summary, check};
use crate::transport::{CHAT_MESSAGES_URL, ENV, PROJECT, Sites, TICK};
use crate::workflow::{
    AlertInput, AttemptLog, INCIDENT_FLOW, IncidentOutput, WorkerName, incident_flow_info,
    notify_chat_info,
};

/// The error type of the example.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The longest time for one run. CI must not hang. The env var
/// `JIRA_EXAMPLE_TIME_FACTOR` scales it, for example under coverage.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// The extra time after the run timeout for the stops. Only a setup or a
/// stop that hangs uses it.
const STOP_GRACE: Duration = Duration::from_secs(30);

/// The alert of the scenario.
const ALERT: &str = "A-1001";
/// The webhook secret of the Jira site. autumn needs 32 bytes or more in
/// prod.
const WEBHOOK_SECRET: &str = "ecosystem-webhook-secret-0123456789abcdef";
/// The OAuth client secret of the `atlassian` integration.
const CLIENT_SECRET: &str = "ecosystem-oauth-client-secret";
/// The advisory lock key of the migrations of this example.
const MIGRATION_LOCK: i64 = 0x6a69_7261_6563;

/// The factor of `JIRA_EXAMPLE_TIME_FACTOR`: 1 when unset.
fn time_factor() -> u32 {
    std::env::var("JIRA_EXAMPLE_TIME_FACTOR")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|f| *f > 0)
        .unwrap_or(1)
}

/// The run timeout, scaled.
fn run_timeout() -> Duration {
    RUN_TIMEOUT.saturating_mul(time_factor())
}

/// The knobs of one run. [`Scenario::happy`] is the run of `main`.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// The webhook secret of the autumn app. The happy path uses the secret
    /// of the Jira site.
    pub app_secret: String,
    /// A person moves the issue to Done.
    pub person_resolves: bool,
    /// The driver posts the deliveries of the person to the app.
    pub deliver: bool,
    /// After each delivery, the driver also posts the same delivery again,
    /// a tampered copy and a copy from the bot.
    pub webhook_negatives: bool,
    /// The search index lag of the fake.
    pub index_lag: Duration,
    /// The wait before the attempt after a lost create reply.
    pub settle_secs: u64,
    /// The longest wait for Done.
    pub wait_secs: u64,
    /// The poll period of the wait. `None`: only a webhook ends the wait.
    pub poll_every_secs: Option<u64>,
}

impl Scenario {
    /// The scenario of plan 11.3.
    #[must_use]
    pub fn happy() -> Self {
        Self {
            app_secret: WEBHOOK_SECRET.to_owned(),
            person_resolves: true,
            deliver: true,
            webhook_negatives: false,
            index_lag: Duration::from_secs(2),
            settle_secs: 3,
            wait_secs: 60 * u64::from(time_factor()),
            poll_every_secs: None,
        }
    }
}

/// The fakes of the scenario. The fault script is the same for each
/// scenario: the first create reply is lost after the commit.
#[must_use]
pub fn scenario_sites(scenario: &Scenario) -> Sites {
    let sites = Sites::new(WEBHOOK_SECRET, CLIENT_SECRET);
    sites
        .jira
        .fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    sites.jira.index_lag(scenario.index_lag);
    sites
}

/// What one run gave.
#[derive(Debug)]
pub struct Report {
    /// The timeline.
    pub summary: Summary,
    /// The output of the workflow.
    pub output: IncidentOutput,
    /// The status of each webhook POST.
    pub webhook_statuses: Vec<u16>,
    /// The counters of the bridge.
    pub bridge: BridgeCounters,
    /// The webhook counters of the plugin, from `/actuator/health`.
    pub webhook_counters: Value,
}

/// Applies the harvest migrations under an advisory lock, so two runs do
/// not race.
async fn migrate_once(url: &str) -> Result<(), BoxError> {
    use autumn_harvest::diesel_async::RunQueryDsl;
    let mut conn = AsyncPgConnection::establish(url).await?;
    diesel::sql_query(format!("SELECT pg_advisory_lock({MIGRATION_LOCK})"))
        .execute(&mut conn)
        .await?;
    let applied = migrate::apply_to_connection(&mut conn, &migrate::embedded()).await;
    diesel::sql_query(format!("SELECT pg_advisory_unlock({MIGRATION_LOCK})"))
        .execute(&mut conn)
        .await?;
    applied.map_err(|e| format!("the harvest migrations failed: {e}"))?;
    Ok(())
}

/// The state that each harvest worker gets.
#[derive(Clone)]
struct WorkerState {
    url: String,
    pool: DbPool,
    queue: &'static str,
    san: SanMateo,
    jira: JiraClient,
    key: RouteKey,
    camel: CamelHandle,
    attempts: AttemptLog,
}

/// Starts one harvest worker on the queue of the run.
async fn start_worker(state: &WorkerState, name: &str) -> Result<HarvestRunner, BoxError> {
    let mut worker = WorkerConfig::default()
        .with_queues([state.queue])
        .with_notification_database_url(&state.url);
    // A short drain, so that a stop is fast.
    worker.shutdown_timeout = Duration::from_secs(5);
    let mut activities = autumn_jira_harvest::activities();
    activities.push(san_mateo_harvest::san_mateo_proxy_info());
    activities.push(notify_chat_info());
    let built = HarvestBuilder::new()
        .workflows(vec![incident_flow_info()])
        .activities(activities)
        .state(JiraClients::new(state.jira.clone()))
        .state(state.key.clone())
        .state(state.san.clone())
        .state(state.camel.clone())
        .state(WorkerName(name.to_owned()))
        .activity_interceptor(state.attempts.clone())
        .worker(worker)
        .build();
    let mut config = HarvestRuntimeConfig {
        mode: HarvestMode::External,
        database: HarvestDatabaseConfig {
            url: Some(state.url.clone()),
        },
        ..HarvestRuntimeConfig::default()
    };
    // Other examples share the database. Their workflow types are not
    // "orphans" of this app.
    config.startup.orphaned_workflows = OrphanStartupAction::Off;
    HarvestRunner::start(
        built,
        &config,
        HarvestRunnerResources::new(state.pool.clone()),
    )
    .await
    .map_err(|e| format!("worker {name} did not start: {e}").into())
}

/// Starts camel: the harvest bridge, `jira:`, `san-mateo:` and the routes.
async fn start_camel(
    san: &SanMateo,
    jira: JiraClient,
) -> Result<(CamelContext, Arc<harvest_camel::CamelSource>), BoxError> {
    let (source, harvest) = HarvestBridge::new(STREAM);
    let mut camel = CamelContext::builder().build().await?;
    camel.register_component(DirectComponent::new());
    camel.register_component(harvest);
    camel.register_component(JiraComponent::new(jira));
    camel.register_component(SanMateoComponent::new(san.clone()).environment(ENV));
    for route in routes()? {
        // Refuse error handlers that commit a message that harvest does
        // not hold.
        check_route(&route)?;
        camel.add_route_definition(route).await?;
    }
    camel.start().await?;
    Ok((camel, source))
}

/// The connector: it reads the `alerts` stream and starts one run for each
/// new alert id. A redelivery attaches to the run of the first one.
fn connector(
    source: Arc<harvest_camel::CamelSource>,
    api: HarvestApiState,
    queue: &'static str,
    input: AlertInput,
) -> ConnectorRuntime {
    let binding = SourceBinding::starts("camel-alerts", STREAM, INCIDENT_FLOW)
        .queue(queue)
        .map_json(move |_ctx, alert: Value| {
            let id = alert["alert_id"]
                .as_str()
                .ok_or("the alert has no alert_id")?;
            // The workflow id holds the run id, so each run of the example
            // has its own workflow.
            let workflow_id = format!("{id}.{queue}");
            let mut start = input.clone();
            id.clone_into(&mut start.alert_id);
            Ok::<_, &str>(MappedMessage::new(workflow_id, json!(start)))
        });
    ConnectorRuntime::for_binding(
        Arc::new(binding),
        source,
        api,
        Arc::new(NoOpMetrics),
        Some(&incident_flow_info()),
    )
}

/// The number of runs on the queue of this run.
async fn runs_on(pool: &DbPool, queue: &str) -> Result<i64, BoxError> {
    use autumn_harvest::diesel_async::RunQueryDsl;
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let mut conn = pool.get().await?;
    let row: Count = diesel::sql_query(
        "SELECT count(*) AS n FROM harvest_workflow_executions WHERE queue_name = $1",
    )
    .bind::<diesel::sql_types::Text, _>(queue)
    .get_result(&mut conn)
    .await?;
    Ok(row.n)
}

/// `n` with its noun: `one run`, `2 runs`.
fn count_of(n: impl Into<u64>, one: &str, many: &str) -> String {
    match n.into() {
        1 => format!("one {one}"),
        n => format!("{n} {many}"),
    }
}

/// The plural `s` for a count.
const fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Runs the scenario on the Postgres database at `url` and returns the
/// timeline. It does not check it: [`main`] and the tests do.
///
/// # Errors
///
/// A setup error, a workflow failure (`workflow <id>: ...`), or the run
/// timeout.
pub async fn run(url: &str, scenario: &Scenario, sites: Sites) -> Result<Report, BoxError> {
    run_within(url, scenario, sites, run_timeout(), &StopLog::default()).await
}

/// The stops of one run: the connector, the worker and camel. The tests
/// read it after a run.
#[derive(Debug, Default)]
struct StopLog {
    connector: AtomicBool,
    runner: AtomicBool,
    camel: AtomicBool,
}

impl StopLog {
    /// Records one stop.
    fn stopped(flag: &AtomicBool) {
        flag.store(true, Ordering::SeqCst);
    }

    /// `true` when the run stopped the connector, the worker and camel.
    #[cfg(test)]
    fn all(&self) -> bool {
        [&self.connector, &self.runner, &self.camel]
            .iter()
            .all(|f| f.load(Ordering::SeqCst))
    }
}

/// Stops the worker in `runner`, when there is one, and records the stop
/// in `flag`.
async fn stop_worker(runner: Option<HarvestRunner>, flag: &AtomicBool) {
    if let Some(runner) = runner {
        runner.stop().await;
    }
    StopLog::stopped(flag);
}

/// [`run`] with the time limit `limit`. `stops` records each stop.
async fn run_within(
    url: &str,
    scenario: &Scenario,
    sites: Sites,
    limit: Duration,
    stops: &StopLog,
) -> Result<Report, BoxError> {
    let late = format!("the run did not end in {limit:?}");
    // The deadline stops the drive, and then the run stops the connector,
    // the worker and camel. A timeout that drops the run would skip these
    // stops. The backstop is only for a setup or a stop that hangs.
    let deadline = tokio::time::Instant::now() + limit;
    let backstop = limit.saturating_add(STOP_GRACE.saturating_mul(time_factor()));
    // The future of the run is large, so it lives on the heap.
    let run = run_unbounded(
        url,
        scenario,
        sites,
        Limit {
            deadline,
            late: &late,
        },
        stops,
    );
    tokio::time::timeout(backstop, Box::pin(run))
        .await
        .map_err(|_| late.clone())?
}

/// The deadline of the drive, and the error text when it passes.
#[derive(Debug, Clone, Copy)]
struct Limit<'a> {
    deadline: tokio::time::Instant,
    late: &'a str,
}

async fn run_unbounded(
    url: &str,
    scenario: &Scenario,
    sites: Sites,
    limit: Limit<'_>,
    stops: &StopLog,
) -> Result<Report, BoxError> {
    migrate_once(url).await?;
    let pool = engine_pool(url, 8, DbRole::Hot, &EngineDbTimeouts::default())?;
    let san = transport::san_mateo(&sites, CLIENT_SECRET).await?;

    // [2] san-mateo: the stored token is expired. The discovery refreshes
    // it, reads accessible-resources and stores the cloud id.
    let refreshes = sites.jira.token_refreshes();
    let cloud_id = transport::discover_cloud_id(&san).await?;
    let refreshes = sites.jira.token_refreshes() - refreshes;
    let stored = transport::stored_cloud_id(&san).await?.as_deref() == Some(cloud_id.as_str());
    let line2 = format!(
        "token for acme expired -> refreshed ({refreshes} token call{}); cloudId {}",
        s(refreshes),
        if stored {
            "stored in metadata"
        } else {
            "not stored"
        }
    );

    let jira = transport::jira_client(&san);
    let key = RouteKey::new(Secret::new(format!("route-key-{}", OsEntropy.hex(16))))?;
    // The autumn app first: it starts nothing that needs a stop.
    let bridge = HarvestSignalBridge::new(pool.clone(), jira.clone(), key.clone())
        .allow_workflow(INCIDENT_FLOW);
    let app = app::test_app(&san, bridge.clone(), &scenario.app_secret)?;
    let (mut camel, source) = start_camel(&san, transport::durable_client(&san)).await?;
    let handle = CamelHandle::new();
    handle.resolve(&camel, NOTIFY)?;
    // Each run has its own queue, so two runs on one database do not take
    // the tasks of each other. The connector needs a `'static` name: one
    // short string for each run.
    let queue: &'static str = Box::leak(format!("ecosystem-{}", OsEntropy.hex(8)).into_boxed_str());
    let state = WorkerState {
        url: url.to_owned(),
        pool: pool.clone(),
        queue,
        san: san.clone(),
        jira: jira.clone(),
        key: key.clone(),
        camel: handle,
        attempts: AttemptLog::default(),
    };
    let runner = start_worker(&state, "A").await?;
    let api = HarvestApiState::new();
    api.install_storage_pool(runner.storage_pool());
    api.install(runner.api_runtime());
    let input = AlertInput {
        alert_id: String::new(),
        summary: "Disk full on db-1".to_owned(),
        cloud_id,
        settle_secs: scenario.settle_secs,
        wait_secs: scenario.wait_secs,
        poll_every_secs: scenario.poll_every_secs,
    };
    let cancel = CancellationToken::new();
    let connector = tokio::spawn({
        let (cancel, runtime) = (cancel.clone(), connector(source, api.clone(), queue, input));
        async move { runtime.run(cancel).await }
    });

    let mut runner = Some(runner);
    let mut work = Drive {
        url,
        state: &state,
        api: &api,
        camel: &camel,
        app: &app,
        sites: &sites,
        scenario,
        runner: &mut runner,
        deadline: limit.deadline,
    };
    let driven = tokio::time::timeout_at(limit.deadline, drive(&mut work))
        .await
        .unwrap_or_else(|_| Err(limit.late.into()));

    // Stop in all cases: the connector, the worker, then camel. The error
    // of the run comes first.
    cancel.cancel();
    let stopped_connector = connector.await;
    StopLog::stopped(&stops.connector);
    stop_worker(runner.take(), &stops.runner).await;
    let stopped_camel = camel.stop().await;
    StopLog::stopped(&stops.camel);
    let driven = driven?;
    stopped_connector?;
    stopped_camel?;

    let counters = bridge.counters();
    let summary = timeline(&driven, line2, &sites, &state.attempts, counters)?;
    Ok(Report {
        summary,
        output: driven.output,
        webhook_statuses: driven.webhook_statuses,
        bridge: counters,
        webhook_counters: driven.webhook_counters,
    })
}

/// Builds the nine lines from the facts of the run.
fn timeline(
    driven: &Driven,
    line2: String,
    sites: &Sites,
    attempts: &AttemptLog,
    counters: BridgeCounters,
) -> Result<Summary, BoxError> {
    let mut summary = Summary::default();
    // [1] camel: two sends of one alert id, one run.
    summary.line(
        "camel",
        format!(
            "alert {ALERT} sent twice -> {} {INCIDENT_FLOW}/{ALERT}",
            count_of(
                u64::try_from(driven.runs).unwrap_or_default(),
                "run",
                "runs"
            )
        ),
    );
    summary.line("sanmateo", line2);
    let output = &driven.output;
    let issue = output.issue_key.to_string();

    // [3] the escape hatch: the call that the fake got, and the lead.
    let escape = sites.jira.requests().iter().any(|r| {
        r.method == "GET"
            && r.host == "api.atlassian.com"
            && r.path.ends_with(&format!("/rest/api/3/project/{PROJECT}"))
            && r.query == "expand=lead"
    });
    summary.line(
        "escape",
        if escape {
            format!(
                "san_mateo_proxy GET project {PROJECT} expand=lead -> lead {}",
                output.lead
            )
        } else {
            "san_mateo_proxy sent no project call".to_owned()
        },
    );

    // [4] the create: the attempt log of the workers, and the POSTs that
    // the fake got.
    let posts = sites.jira.count(FakeOp::CreateIssue);
    let creates = attempts.of("jira_create_issue");
    let failures: Vec<String> = creates
        .iter()
        .filter_map(|a| a.error_type.as_deref())
        .map(|e| e.trim_start_matches("jira.").to_owned())
        .collect();
    let last = creates.last().map_or(0, |a| a.number);
    summary.line(
        "jira",
        format!(
            "create: 503 after commit -> {} -> attempt {last} {} {issue} ({posts} POST{})",
            if failures.is_empty() {
                "no failed attempt".to_owned()
            } else {
                failures.join(" -> ")
            },
            serde_json::to_value(output.created)?
                .as_str()
                .unwrap_or_default(),
            s(posts)
        ),
    );
    summary.line(
        "jira",
        format!(
            "{issue} -> {} ({})",
            output.moved_to,
            serde_json::to_value(output.moved)?
                .as_str()
                .unwrap_or_default()
        ),
    );

    // [6] the restart: the run ended after worker A stopped.
    let resumed_by = output
        .chat
        .as_ref()
        .map_or("B", |chat| chat.worker.as_str());
    summary.line(
        "harvest",
        if driven.stopped_while_waiting {
            format!("worker A stopped while waiting; worker {resumed_by} resumed the run")
        } else {
            "worker A ran the whole run".to_owned()
        },
    );

    end_lines(&mut summary, driven, counters)?;

    let chats = sites.chat.requests_to(CHAT_MESSAGES_URL);
    summary.one_chat_message_with_a_key = match chats.as_slice() {
        [one] => one
            .header_value("idempotency-key")
            .is_some_and(|k| !k.trim().is_empty()),
        _ => false,
    };
    Ok(summary)
}

/// Lines [7] to [9]: the webhook, the end of the run and the app route.
fn end_lines(
    summary: &mut Summary,
    driven: &Driven,
    counters: BridgeCounters,
) -> Result<(), BoxError> {
    let output = &driven.output;
    let issue = output.issue_key.to_string();
    // [7] autumn and the bridge.
    summary.line(
        "autumn",
        match driven.webhook_statuses.as_slice() {
            [] => format!(
                "no webhook POST; bridge signalled {} run{}",
                counters.signalled,
                s(usize::try_from(counters.signalled).unwrap_or_default())
            ),
            statuses => format!(
                "POST {} -> {}; bridge signalled {} run{}",
                app::WEBHOOK_PATH,
                statuses
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                counters.signalled,
                s(usize::try_from(counters.signalled).unwrap_or_default())
            ),
        },
    );

    // [8] the end of the run.
    let comment = serde_json::to_value(output.comment)?;
    let comment = comment.as_str().unwrap_or_default();
    summary.line(
        "harvest",
        match (&output.wait, &output.chat) {
            (StatusWait::Reached { status, source }, Some(chat)) => format!(
                "{issue} {} (source {}) -> comment ({comment}) -> chat message {}",
                status.name,
                source_name(*source),
                chat.status
            ),
            (StatusWait::Reached { status, source }, None) => format!(
                "{issue} {} (source {}) -> comment ({comment}) -> no chat message",
                status.name,
                source_name(*source)
            ),
            (StatusWait::TimedOut { .. }, _) => {
                format!("{issue} timed out -> escalation comment ({comment})")
            }
        },
    );
    summary.line("autumn", &driven.incident);

    Ok(())
}

/// The name of a wait source in the timeline.
const fn source_name(source: WaitSource) -> &'static str {
    match source {
        WaitSource::Initial => "initial",
        WaitSource::Signal => "signal",
        WaitSource::Poll => "poll",
    }
}

/// What the driver works with.
struct Drive<'a> {
    url: &'a str,
    state: &'a WorkerState,
    api: &'a HarvestApiState,
    camel: &'a CamelContext,
    app: &'a autumn_web::test::TestClient,
    sites: &'a Sites,
    scenario: &'a Scenario,
    /// The worker that runs now: A, then B.
    runner: &'a mut Option<HarvestRunner>,
    /// The end of the drive.
    deadline: tokio::time::Instant,
}

/// What the driver saw.
struct Driven {
    runs: i64,
    output: IncidentOutput,
    stopped_while_waiting: bool,
    webhook_statuses: Vec<u16>,
    webhook_counters: Value,
    incident: String,
}

/// Is the run in the wait? The transition is done, and the wait has read
/// the issue once (the transition read it first).
fn waiting(sites: &Sites) -> bool {
    sites.jira.count(FakeOp::DoTransition) >= 1 && sites.jira.count(FakeOp::GetIssue) >= 2
}

/// A person resolves the issue. The fake signs one delivery, and the
/// driver posts it to the app, as Jira does. Returns the status of each
/// POST.
async fn person_resolves(d: &Drive<'_>) -> Result<Vec<u16>, BoxError> {
    // The deliveries of the bot (the API writes) are not part of this step.
    drop(d.sites.jira.take_deliveries());
    let key = d
        .sites
        .jira
        .issues()
        .first()
        .and_then(|i| i["key"].as_str().map(str::to_owned))
        .ok_or("the run made no issue")?;
    d.sites.jira.human_transition(&key, "Done", ANN)?;
    let mut statuses = Vec::new();
    if !d.scenario.deliver {
        return Ok(statuses);
    }
    for delivery in d.sites.jira.take_deliveries() {
        statuses.push(d.app.deliver(&delivery).await.status.as_u16());
        if d.scenario.webhook_negatives {
            // Webhooks are at least once: the same delivery again. Then a
            // tampered copy, and a copy from the bot.
            for copy in [
                delivery.clone(),
                delivery.tampered(),
                delivery.with_actor(BOT),
            ] {
                statuses.push(d.app.deliver(&copy).await.status.as_u16());
            }
        }
    }
    Ok(statuses)
}

/// The person, the webhook, the worker restart and the app route.
async fn drive(d: &mut Drive<'_>) -> Result<Driven, BoxError> {
    // [1] The broker sends the alert two times, as after a crash.
    // `call_route` returns when harvest holds the message.
    let broker = CamelHandle::new();
    broker.resolve(d.camel, ALERTS)?;
    let alert = json!({"alert_id": ALERT, "severity": "high"});
    for _ in 0..2 {
        let id = [(
            ALERT_ID.to_owned(),
            json!(format!("{ALERT}.{}", d.state.queue)),
        )];
        call_route(&broker, ALERTS, alert.clone(), id).await?;
    }
    let runs = runs_on(&d.state.pool, d.state.queue).await?;
    let workflow_id = format!("{ALERT}.{}", d.state.queue);
    let run = {
        let mut conn = d.state.pool.get().await?;
        resolve_execution_id_by_workflow_id(&mut conn, INCIDENT_FLOW, &workflow_id)
            .await?
            .ok_or("the alert started no run")?
    };
    let handle = WorkflowHandleClient::single(d.state.pool.clone(), d.url).handle(run.exec_id);
    // The result task ends at the deadline of the drive too.
    let timeout = d
        .deadline
        .saturating_duration_since(tokio::time::Instant::now());
    let mut result = tokio::spawn(async move { handle.result_raw_with_timeout(timeout).await });

    // [6] When the run waits, stop worker A and start worker B.
    let mut stopped_while_waiting = false;
    let mut finished = None;
    loop {
        if result.is_finished() {
            finished = Some((&mut result).await?);
            break;
        }
        if waiting(d.sites) {
            // Let the wait record its first read and park the run.
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Some(a) = d.runner.take() {
                a.stop().await;
            }
            let b = start_worker(d.state, "B").await?;
            d.api.install_storage_pool(b.storage_pool());
            d.api.install(b.api_runtime());
            *d.runner = Some(b);
            stopped_while_waiting = true;
            break;
        }
        tokio::time::sleep(TICK).await;
    }

    // [7] A person resolves the issue, and the driver posts the delivery.
    let webhook_statuses = if finished.is_none() && d.scenario.person_resolves {
        person_resolves(d).await?
    } else {
        Vec::new()
    };
    let output = match finished {
        Some(output) => output,
        None => result.await?,
    }
    .map_err(|e| {
        // The error type comes first, for example `jira.outcome_unknown`.
        let kind = e.workflow_error_type().unwrap_or("harvest.error");
        format!("workflow {workflow_id}: {kind}: {e}")
    })?;
    let output: IncidentOutput = serde_json::from_value(output)?;

    // [9] The app route reads the issue through san-mateo.
    let path = format!("/incidents/{}", output.issue_key);
    let reply = d.app.get(&path).send().await;
    let body: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    let incident = if reply.status.as_u16() == 200 {
        format!(
            "GET {path} -> {} {}",
            body["key"].as_str().unwrap_or_default(),
            body["status"].as_str().unwrap_or_default()
        )
    } else {
        format!("GET {path} -> {}", reply.status.as_u16())
    };
    let health = d.app.get("/actuator/health").send().await;
    let health: Value = serde_json::from_slice(&health.body).unwrap_or(Value::Null);
    let webhook_counters = health["components"]["jira"]["details"]["webhook"].clone();
    Ok(Driven {
        runs,
        output,
        stopped_while_waiting,
        webhook_statuses,
        webhook_counters,
        incident,
    })
}

/// Installs one rustls crypto provider for the process. The graph turns on
/// two (`ring` and `aws-lc-rs`, spike S3), and then rustls cannot pick one.
fn install_crypto_provider() {
    // A second call finds a provider and changes nothing.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn main() -> ExitCode {
    install_crypto_provider();
    let Some(url) = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()) else {
        eprintln!("set DATABASE_URL to a Postgres URL, or run scripts/run-ecosystem.sh");
        return ExitCode::FAILURE;
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("no runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let scenario = Scenario::happy();
    let report = match runtime.block_on(run(&url, &scenario, scenario_sites(&scenario))) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("run failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", report.summary);
    if let Err(e) = check(&report.summary) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    println!("check: ok");
    ExitCode::SUCCESS
}

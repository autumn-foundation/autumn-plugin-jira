//! Example `harvest_workflow`: Jira in a durable harvest workflow on Postgres.
//!
//! ```text
//! run 1: workflow jira_ticket_lifecycle (the recipe)
//!   1. jira_create_issue with a route   503 after the commit: attempt 1 gives
//!                                       jira.outcome_pending, attempt 2 finds
//!                                       the issue by its marker (no 2nd POST)
//!   2. jira_transition_to_status        To Do -> In Progress
//!   3. await_issue_status (Done)        a person resolves the issue; the
//!                                       FakeJira delivery goes into
//!                                       HarvestSignalBridge::deliver
//!   4. jira_add_comment                 one comment, with its marker
//! run 2: workflow jira_watch_lifecycle
//!   1. jira_watch_issue                 on an issue that a person made
//!   2. await_issue_status (Done)        no webhook: a poll ends the wait
//! ```
//!
//! All texts are input fields (R16). The inputs and the history hold ids and
//! texts only, never a token. `FakeJira` is the Jira site. For production,
//! use `JiraClient::api_token` with a `ReqwestTransport`, or a san-mateo
//! connection, and read the route key from a secret store.
//!
//! Each run uses its own queue and its own workflow ids, so two copies can
//! run at the same time on one database.
//!
//! ```sh
//! DATABASE_URL=postgres://jira:jira@localhost/jira_test \
//!   cargo run --locked -p autumn-jira-harvest --example harvest_workflow
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the example prints its summary"
)]

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use autumn_harvest::StartWorkflowParams;
use autumn_harvest::diesel;
use autumn_harvest::diesel_async::{AsyncConnection, AsyncPgConnection};
use autumn_harvest::migrate;
use autumn_harvest::pool::{DbRole, EngineDbTimeouts, engine_pool};
use autumn_harvest::prelude::*;
use autumn_harvest::worker::{DbPool, Worker};
use autumn_jira::client::ClientSettings;
use autumn_jira::ids::{SiteName, WebhookDeliveryId};
use autumn_jira::request::IssueTypeRef;
use autumn_jira::san_mateo::Secret;
use autumn_jira::san_mateo::clock::{Entropy, OsEntropy};
use autumn_jira::webhook::{DeliveryMeta, Flow, parse_event};
use autumn_jira::{
    BasicAuth, Delivery, EventSink, IssueKey, IssueRef, JiraClient, JiraSite, NewIssue, ProjectKey,
    StatusTarget,
};
use autumn_jira_fake::{ANN, FakeJira, FakeOp, Fault};
use autumn_jira_harvest::{
    AddCommentInput, CommentOutput, CreateIssueInput, CreateIssueOutput, HarvestSignalBridge,
    JiraClients, RouteKey, SignalRoute, StatusWait, TransitionInput, TransitionOutput, WaitOptions,
    WatchIssueInput, WatchOutput, await_issue_status, drain_jira_signals, jira_add_comment_info,
    jira_create_issue_info, jira_transition_to_status_info, jira_watch_issue_info,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The longest time for one run. CI must not hang. The env var
/// `JIRA_EXAMPLE_TIME_FACTOR` scales it, for example under coverage.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

const LIFECYCLE: &str = "jira_ticket_lifecycle";
const WATCH: &str = "jira_watch_lifecycle";
const WEBHOOK_SECRET: &str = "example-webhook-secret";
/// The advisory lock key of the migrations of this example.
const MIGRATION_LOCK: i64 = 0x6a69_7261_6578;

/// The input of the recipe workflow. Each text is an input field.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LifecycleInput {
    /// The create. The workflow adds its own route.
    create: CreateIssueInput,
    /// The status after the triage.
    working: StatusTarget,
    /// The status that ends the wait.
    done: StatusTarget,
    /// The comment after the wait.
    comment: String,
    /// The wait.
    wait: WaitOptions,
    /// The harvest retry of each activity: attempts and a fixed interval.
    retry_attempts: u32,
    /// The fixed retry interval, in milliseconds.
    retry_interval_ms: u64,
}

/// The output of the recipe workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LifecycleOutput {
    created: CreateIssueOutput,
    moved: TransitionOutput,
    wait: StatusWait,
    comment: CommentOutput,
}

/// Keeps a harvest error type in the workflow failure.
fn failed(error: &HarvestError) -> WorkflowFailure {
    WorkflowFailure::new(
        error.activity_error_type().unwrap_or("jira.workflow"),
        error.to_string(),
    )
}

/// The recipe: create with a route, move, wait for Done, comment.
#[workflow]
async fn jira_ticket_lifecycle(
    ctx: &WorkflowContext,
    input: LifecycleInput,
) -> Result<LifecycleOutput, WorkflowFailure> {
    // Keep the activities on the queue of this run.
    let queue = ctx.queue_name().to_owned();
    let retry = || {
        Some(RetryPolicy::fixed(
            input.retry_attempts,
            Duration::from_millis(input.retry_interval_ms),
        ))
    };
    let route = SignalRoute::new(LIFECYCLE, ctx.workflow_id())
        .map_err(|e| WorkflowFailure::new("jira.invalid_input", e.to_string()).non_retryable())?;
    let mut create = input.create.clone();
    create.route = Some(route);
    let created: CreateIssueOutput = ctx
        .execute_activity_with_opts(
            &jira_create_issue_info(),
            &create,
            Some(&queue),
            retry(),
            None,
        )
        .await
        .map_err(|e| failed(&e))?;
    let issue = IssueRef::Id(created.id.clone());
    let moved: TransitionOutput = ctx
        .execute_activity_with_opts(
            &jira_transition_to_status_info(),
            TransitionInput::new(issue.clone(), input.working.clone()),
            Some(&queue),
            retry(),
            None,
        )
        .await
        .map_err(|e| failed(&e))?;
    let wait = await_issue_status(ctx, &created.id, &input.done, &input.wait).await?;
    let comment: CommentOutput = ctx
        .execute_activity_with_opts(
            &jira_add_comment_info(),
            AddCommentInput::new(issue, input.comment.as_str()),
            Some(&queue),
            retry(),
            None,
        )
        .await
        .map_err(|e| failed(&e))?;
    // A webhook can come while the comment runs. Take it, so that no
    // signal stays unread in the history.
    drain_jira_signals(ctx)?;
    Ok(LifecycleOutput {
        created,
        moved,
        wait,
        comment,
    })
}

/// The input of the watch workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WatchInput {
    issue: IssueKey,
    done: StatusTarget,
    wait: WaitOptions,
}

/// Watches an issue that a person made, then waits until it is done.
#[workflow]
async fn jira_watch_lifecycle(
    ctx: &WorkflowContext,
    input: WatchInput,
) -> Result<StatusWait, WorkflowFailure> {
    let route = SignalRoute::new(WATCH, ctx.workflow_id())
        .map_err(|e| WorkflowFailure::new("jira.invalid_input", e.to_string()).non_retryable())?;
    let watched: WatchOutput = ctx
        .execute_activity_with_opts(
            &jira_watch_issue_info(),
            WatchIssueInput::new(IssueRef::Key(input.issue.clone()), route),
            Some(ctx.queue_name()),
            None,
            None,
        )
        .await
        .map_err(|e| failed(&e))?;
    await_issue_status(ctx, &watched.issue.id, &input.done, &input.wait).await
}

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

/// Applies the harvest migrations under an advisory lock, so two copies
/// do not race.
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

/// The Jira client of the fake, with short waits.
fn jira_client(fake: &FakeJira) -> Result<JiraClient, BoxError> {
    let mut settings = ClientSettings::default();
    settings.base_backoff_ms = 50;
    let site = JiraSite::Subdomain(SiteName::parse(autumn_jira_fake::DEFAULT_SITE)?);
    let auth = BasicAuth::new(
        autumn_jira_fake::DEFAULT_EMAIL,
        Secret::new(autumn_jira_fake::DEFAULT_API_TOKEN),
    )?;
    Ok(JiraClient::api_token(site, auth, Arc::new(fake.clone())).with_settings(settings))
}

/// One worker on its own queue, with the Jira state.
struct Harness {
    url: String,
    pool: DbPool,
    queue: String,
    run_id: String,
    fake: FakeJira,
    bridge: HarvestSignalBridge,
    worker: Arc<Worker>,
    running: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn start(url: &str, fake: FakeJira) -> Result<Self, BoxError> {
        migrate_once(url).await?;
        let pool = engine_pool(url, 8, DbRole::Hot, &EngineDbTimeouts::default())?;
        let run_id = OsEntropy.hex(8);
        let queue = format!("jira-example-{run_id}");
        let client = jira_client(&fake)?;
        // From an env var or a secret store in production.
        let key = RouteKey::new(Secret::new(format!("route-key-{}", OsEntropy.hex(16))))?;
        let bridge = HarvestSignalBridge::new(pool.clone(), client.clone(), key.clone())
            .allow_workflow(LIFECYCLE)
            .allow_workflow(WATCH);
        let mut config = WorkerConfig::default()
            .with_queues([queue.as_str()])
            .with_notification_database_url(url);
        // A short drain, so that a failed run stops fast.
        config.shutdown_timeout = Duration::from_secs(5);
        let (registry, _dags, _schedules, config) = HarvestBuilder::new()
            .workflows(vec![
                jira_ticket_lifecycle_info(),
                jira_watch_lifecycle_info(),
            ])
            .activities(autumn_jira_harvest::activities())
            .state(JiraClients::new(client))
            .state(key)
            .worker(config)
            .try_build()?
            .into_worker_parts();
        let worker = Arc::new(Worker::new(config.into(), Arc::new(registry))?);
        let running = tokio::spawn({
            let (worker, pool) = (Arc::clone(&worker), pool.clone());
            async move { worker.run(&pool).await }
        });
        Ok(Self {
            url: url.to_owned(),
            pool,
            queue,
            run_id,
            fake,
            bridge,
            worker,
            running,
        })
    }

    /// Starts a workflow and returns its handle.
    async fn launch(
        &self,
        name: &str,
        workflow_id: &str,
        input: Value,
    ) -> Result<StartedWorkflowHandle, BoxError> {
        let client = WorkflowHandleClient::single(self.pool.clone(), self.url.clone());
        let shard = client.pick_shard_for_new_workflow(name, workflow_id);
        let params = StartWorkflowParams::new(
            name,
            workflow_id,
            ExecutionId::new_for_shard(shard),
            input,
            &self.queue,
        );
        let mut conn = self.pool.get().await?;
        Ok(client.start_or_load(&mut conn, params).await?)
    }

    async fn stop(self) -> Result<(), BoxError> {
        self.worker.shutdown();
        self.running.await?;
        Ok(())
    }
}

/// Feeds each queued `FakeJira` delivery into the bridge, as the webhook
/// route of the plugin does after the signature check.
async fn feed(fake: &FakeJira, bridge: &HarvestSignalBridge) -> Result<usize, BoxError> {
    let deliveries = fake.take_deliveries();
    for d in &deliveries {
        let parsed = parse_event(&d.body)?;
        let flow = Flow::parse(d.header("X-Atlassian-Webhook-Flow").unwrap_or("Primary"));
        let meta = DeliveryMeta::new(WebhookDeliveryId::parse(d.id())?, 0, flow);
        bridge
            .deliver(Arc::new(Delivery::new(meta, parsed)))
            .await
            .map_err(|e| format!("the bridge failed: {e}"))?;
    }
    Ok(deliveries.len())
}

/// The input of run 1.
fn lifecycle_input() -> Result<LifecycleInput, BoxError> {
    let mut create = CreateIssueInput::new(
        ProjectKey::parse("OPS")?,
        IssueTypeRef::Name("Incident".into()),
        "Disk full on db-1",
    );
    create.description_text = Some("The disk of db-1 is 98% full.".into());
    create.labels = vec!["alert".into()];
    create.settle_secs = Some(3);
    let mut wait = WaitOptions::new(Duration::from_secs(60) * time_factor());
    // No poll: only the webhook ends this wait.
    wait.poll_every = None;
    Ok(LifecycleInput {
        create,
        working: StatusTarget::in_progress(),
        done: StatusTarget::done(),
        comment: "Resolved: the old logs are gone.".into(),
        wait,
        retry_attempts: 3,
        retry_interval_ms: 100,
    })
}

/// Run 1: the recipe, resolved by a webhook.
async fn lifecycle(h: &Harness) -> Result<Value, BoxError> {
    let workflow_id = format!("ticket-{}", h.run_id);
    let started = h
        .launch(
            LIFECYCLE,
            &workflow_id,
            serde_json::to_value(lifecycle_input()?)?,
        )
        .await?;
    let stop = Arc::new(AtomicBool::new(false));
    // The person and the webhook: when the wait has read the issue, a
    // person resolves it. Each delivery goes into the bridge.
    let person = tokio::spawn({
        let (fake, bridge, stop) = (h.fake.clone(), h.bridge.clone(), Arc::clone(&stop));
        async move {
            let mut resolved = false;
            while !stop.load(Ordering::SeqCst) {
                feed(&fake, &bridge).await?;
                // One read by the transition and one by the wait. The
                // bridge reads the issue too, once for each delivery with
                // a route that passes the checks.
                let c = bridge.counters();
                let bridge_reads = c.signalled + c.duplicate + c.no_run + c.finished;
                let reads = u64::try_from(fake.count(FakeOp::GetIssue)).unwrap_or(u64::MAX);
                let waiting = fake.count(FakeOp::DoTransition) >= 1 && reads >= 2 + bridge_reads;
                if waiting
                    && !resolved
                    && let Some(key) = lone_key(&fake)
                {
                    fake.human_transition(&key, "Done", ANN)?;
                    resolved = true;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            feed(&fake, &bridge).await
        }
    });
    let output = started
        .handle
        .result_raw_with_timeout(run_timeout())
        .await
        .map_err(|e| format!("workflow {workflow_id}: {e}"));
    stop.store(true, Ordering::SeqCst);
    person.await??;
    Ok(output?)
}

/// The key of the one issue of run 1.
fn lone_key(fake: &FakeJira) -> Option<String> {
    let issues = fake.issues();
    match issues.as_slice() {
        [issue] => issue["key"].as_str().map(str::to_owned),
        _ => None,
    }
}

/// Run 2: watch an issue that a person made; a poll ends the wait.
async fn watch(h: &Harness) -> Result<Value, BoxError> {
    let made = jira_client(&h.fake)?
        .create_issue(&NewIssue::new(
            ProjectKey::parse("OPS")?,
            IssueTypeRef::Name("Task".into()),
            "The printer is on fire",
        ))
        .await?;
    let reads_before = h.fake.count(FakeOp::GetIssue);
    let mut wait = WaitOptions::new(Duration::from_secs(60) * time_factor());
    wait.poll_every = Some(Duration::from_secs(1));
    let input = WatchInput {
        issue: made.key.clone(),
        done: StatusTarget::done(),
        wait,
    };
    let workflow_id = format!("watch-{}", h.run_id);
    let started = h
        .launch(WATCH, &workflow_id, serde_json::to_value(input)?)
        .await?;
    // When the watch and the first read of the wait are done, a person
    // moves the issue. No delivery goes to the bridge.
    let deadline = tokio::time::Instant::now() + run_timeout();
    while h.fake.count(FakeOp::GetIssue) < reads_before + 2 {
        if tokio::time::Instant::now() > deadline {
            return Err(format!("workflow {workflow_id}: the watch did not start").into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.fake
        .human_transition(made.key.as_str(), "In Progress", ANN)?;
    h.fake.human_transition(made.key.as_str(), "Done", ANN)?;
    let dropped = h.fake.take_deliveries().len();
    let output = started
        .handle
        .result_raw_with_timeout(run_timeout())
        .await
        .map_err(|e| format!("workflow {workflow_id}: {e}"))?;
    Ok(json!({"wait": output, "deliveries_not_sent": dropped > 0}))
}

/// Runs both workflows on one worker and returns the summary.
async fn run(url: &str, fake: FakeJira) -> Result<Value, BoxError> {
    let h = Harness::start(url, fake).await?;
    let result = async {
        let first = lifecycle(&h).await?;
        // The counts of run 1, before run 2 makes its own issue.
        let counts = json!({
            "create_posts": h.fake.count(FakeOp::CreateIssue),
            "comment_posts": h.fake.count(FakeOp::AddComment),
        });
        let second = watch(&h).await?;
        Ok::<_, BoxError>((first, counts, second))
    }
    .await;
    let summary = result.map(|(first, counts, second)| summarize(&first, &counts, &second, &h));
    h.stop().await?;
    summary
}

/// The values that `check` compares.
fn summarize(first: &Value, counts: &Value, second: &Value, h: &Harness) -> Value {
    let wait = |w: &Value| {
        w.get("Reached").map_or_else(
            || json!({"timed_out": w}),
            |r| json!({"status": r["status"]["name"], "source": r["source"]}),
        )
    };
    let counters = h.bridge.counters();
    json!({
        "lifecycle": {
            "create": first["created"]["outcome"],
            "transition": first["moved"]["outcome"],
            "after_transition": first["moved"]["status"]["name"],
            "wait": wait(&first["wait"]),
            "comment": first["comment"]["outcome"],
        },
        "create_posts": counts["create_posts"],
        "issues_with_the_summary": h.fake.issues().iter()
            .filter(|i| i["fields"]["summary"] == "Disk full on db-1").count(),
        "comment_posts": counts["comment_posts"],
        "bridge_signalled": counters.signalled > 0,
        "bridge_bad_tags": counters.bad_tag,
        "watch": wait(&second["wait"]),
        "watch_deliveries_not_sent": second["deliveries_not_sent"],
    })
}

/// Compares the summary with the expected values. The error starts with
/// `check failed:`.
fn check(summary: &Value) -> Result<(), String> {
    let expected = json!({
        "lifecycle": {
            "create": "found",
            "transition": "moved",
            "after_transition": "In Progress",
            "wait": {"status": "Done", "source": "Signal"},
            "comment": "created",
        },
        "create_posts": 1,
        "issues_with_the_summary": 1,
        "comment_posts": 1,
        "bridge_signalled": true,
        "bridge_bad_tags": 0,
        "watch": {"status": "Done", "source": "Poll"},
        "watch_deliveries_not_sent": true,
    });
    if *summary == expected {
        Ok(())
    } else {
        Err(format!(
            "check failed: the summary {summary} is not {expected}"
        ))
    }
}

/// The fake of the scenario: webhooks on, a search index lag, and a 503
/// after the commit of the first create.
fn scenario_fake() -> FakeJira {
    let fake = FakeJira::new().with_webhook_secret(WEBHOOK_SECRET);
    fake.index_lag(Duration::from_secs(1));
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    fake
}

fn main() -> ExitCode {
    let Some(url) = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()) else {
        eprintln!("set DATABASE_URL to a Postgres URL");
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
    let summary = match runtime.block_on(run(&url, scenario_fake())) {
        Ok(summary) => summary,
        Err(e) => {
            eprintln!("run failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = check(&summary) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    let pretty = serde_json::to_string_pretty(&summary).unwrap_or_default();
    println!("{pretty}");
    println!("check: ok");
    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    const DB_ENV: &str = "JIRA_TEST_DATABASE_URL";

    /// The test database URL. With no URL, the test skips, but not in CI.
    fn database_url() -> Option<String> {
        match std::env::var(DB_ENV) {
            Ok(url) if !url.is_empty() => Some(url),
            _ => {
                assert!(
                    std::env::var_os("CI").is_none(),
                    "CI must set {DB_ENV} to a Postgres URL"
                );
                eprintln!("skipping: set {DB_ENV} to a Postgres URL to run the example");
                None
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn lifecycle_resolves_by_webhook() {
        let Some(url) = database_url() else { return };
        let summary = run(&url, scenario_fake()).await.expect("the example runs");
        check(&summary).expect("the summary is right");
        assert_eq!(summary["lifecycle"]["wait"]["source"], "Signal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn watch_resolves_by_poll() {
        let Some(url) = database_url() else { return };
        let fake = FakeJira::new().with_webhook_secret(WEBHOOK_SECRET);
        let h = Harness::start(&url, fake).await.unwrap();
        let second = watch(&h).await;
        let bridge = h.bridge.counters();
        h.stop().await.unwrap();
        let second = second.expect("the watch run ends");
        assert_eq!(second["wait"]["Reached"]["source"], "Poll", "{second}");
        assert_eq!(bridge.deliveries, 0, "no webhook");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_400_fails_with_one_post() {
        let Some(url) = database_url() else { return };
        let fake = FakeJira::new().with_webhook_secret(WEBHOOK_SECRET);
        fake.fault(FakeOp::CreateIssue, Fault::Status(400));
        let err = run(&url, fake.clone())
            .await
            .expect_err("the workflow fails");
        let err = err.to_string();
        assert!(err.starts_with("workflow ticket-"), "{err}");
        // A 400 is not retryable: one POST, and no wait for the timeout.
        assert_eq!(fake.count(FakeOp::CreateIssue), 1, "{err}");
        assert_eq!(fake.issues().len(), 0, "no issue");
    }

    #[test]
    fn a_wrong_value_fails_the_check() {
        let mut summary = json!({"lifecycle": {"create": "created"}});
        let err = check(&summary).expect_err("a wrong value fails");
        assert!(err.starts_with("check failed:"), "{err}");
        summary["lifecycle"]["create"] = json!("found");
        assert!(check(&summary).is_err());
    }
}

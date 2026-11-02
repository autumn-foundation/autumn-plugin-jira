//! The signal bridge on Postgres (AC34).
//!
//! Each test makes its own pool, its own queue and its own workflow ids
//! from a nonce. The migrations run once for each test binary, under a
//! Postgres advisory lock. With no database URL a test skips, but never
//! when `CI` is set.
//!
//! The tests with no database call use a pool to a closed port: a database
//! call there gives an error, so an `Ok` proves that no call was made.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::sync::Arc;

use autumn_harvest::StartWorkflowParams;
use autumn_harvest::diesel;
use autumn_harvest::diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use autumn_harvest::migrate;
use autumn_harvest::pool::{DbRole, EngineDbTimeouts, engine_pool};
use autumn_harvest::prelude::{ExecutionId, WorkflowHandleClient};
use autumn_harvest::worker::DbPool;
use autumn_jira::ids::{SiteName, WebhookDeliveryId};
use autumn_jira::request::IssueTypeRef;
use autumn_jira::san_mateo::Secret;
use autumn_jira::san_mateo::clock::{Entropy, OsEntropy};
use autumn_jira::webhook::{DeliveryMeta, Flow, parse_event};
use autumn_jira::{
    BasicAuth, Delivery, EventSink, HandlerError, IssueKey, JiraClient, JiraSite, NewIssue,
    ProjectKey,
};
use autumn_jira_fake::{ANN, DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp, Fault};
use autumn_jira_harvest::{
    HarvestSignalBridge, IssueSignal, ROUTES_PROPERTY, RouteKey, RouteOutcome, SIGNAL, SignalRoute,
    TRACKED_LABEL,
};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

const DB_ENV: &str = "JIRA_TEST_DATABASE_URL";
const WEBHOOK_SECRET: &str = "webhook-secret";
/// The advisory lock key of the migrations of this crate's tests.
const MIGRATION_LOCK: i64 = 0x6a69_7261_6876;
/// A Postgres URL with no server: each call fails at once.
const CLOSED_URL: &str = "postgres://jira:jira@127.0.0.1:9/none";
const WORKFLOW: &str = "incident_flow";

static MIGRATED: OnceCell<()> = OnceCell::const_new();

/// The test database URL. With no URL, the test skips, but not in CI.
fn database_url() -> Option<String> {
    match std::env::var(DB_ENV) {
        Ok(url) if !url.is_empty() => Some(url),
        _ => {
            assert!(
                std::env::var_os("CI").is_none(),
                "CI must set {DB_ENV} to a Postgres URL"
            );
            eprintln!("skipping: set {DB_ENV} to a Postgres URL");
            None
        }
    }
}

/// Applies the harvest migrations once, under an advisory lock.
async fn migrated(url: &str) {
    MIGRATED
        .get_or_init(|| async {
            let mut conn = AsyncPgConnection::establish(url).await.unwrap();
            diesel::sql_query(format!("SELECT pg_advisory_lock({MIGRATION_LOCK})"))
                .execute(&mut conn)
                .await
                .unwrap();
            let applied = migrate::apply_to_connection(&mut conn, &migrate::embedded()).await;
            diesel::sql_query(format!("SELECT pg_advisory_unlock({MIGRATION_LOCK})"))
                .execute(&mut conn)
                .await
                .unwrap();
            applied.expect("the migrations apply");
        })
        .await;
}

/// A pool for one test.
fn pool(url: &str) -> DbPool {
    engine_pool(url, 2, DbRole::Hot, &EngineDbTimeouts::default()).unwrap()
}

fn nonce() -> String {
    OsEntropy.hex(8)
}

fn route_key() -> RouteKey {
    RouteKey::new(Secret::new("bridge-route-key-0123456789-abcdef")).unwrap()
}

fn client(fake: &FakeJira) -> JiraClient {
    JiraClient::api_token(
        JiraSite::Subdomain(SiteName::parse("acme").unwrap()),
        BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN)).unwrap(),
        Arc::new(fake.clone()),
    )
}

/// A fake with webhooks on.
fn fake() -> FakeJira {
    FakeJira::new().with_webhook_secret(WEBHOOK_SECRET)
}

/// Makes a tracked issue with these route entries, and drops its deliveries.
async fn tracked_issue(fake: &FakeJira, entries: Vec<Value>) -> IssueKey {
    let issue = NewIssue::new(
        ProjectKey::parse("OPS").unwrap(),
        IssueTypeRef::Name("Task".into()),
        "Disk full",
    )
    .labels([TRACKED_LABEL])
    .property(ROUTES_PROPERTY, json!({"routes": entries}));
    let created = client(fake).create_issue(&issue).await.unwrap();
    let _ = fake.take_deliveries();
    created.key
}

/// A person moves the issue to In Progress. Returns the delivery.
fn status_change(fake: &FakeJira, key: &IssueKey) -> Delivery {
    fake.human_transition(key.as_str(), "In Progress", ANN)
        .unwrap();
    let mut deliveries = fake.take_deliveries();
    let last = deliveries.pop().expect("a delivery");
    let parsed = parse_event(&last.body).unwrap();
    let meta = DeliveryMeta::new(
        WebhookDeliveryId::parse(last.id()).unwrap(),
        0,
        Flow::parse(last.header("X-Atlassian-Webhook-Flow").unwrap_or("Primary")),
    );
    Delivery::new(meta, parsed)
}

/// Starts a run with no worker. It stays `RUNNING` and buffers signals.
async fn start_run(
    pool: &DbPool,
    url: &str,
    workflow_id: &str,
) -> autumn_harvest::prelude::StartedWorkflowHandle {
    let client = WorkflowHandleClient::single(pool.clone(), url.to_owned());
    let shard = client.pick_shard_for_new_workflow(WORKFLOW, workflow_id);
    let queue = format!("jira-bridge-test-{}", nonce());
    let params = StartWorkflowParams::new(
        WORKFLOW,
        workflow_id,
        ExecutionId::new_for_shard(shard),
        json!({}),
        &queue,
    );
    let mut conn = pool.get().await.unwrap();
    client.start_or_load(&mut conn, params).await.unwrap()
}

fn bridge(pool: DbPool, fake: &FakeJira) -> HarvestSignalBridge {
    HarvestSignalBridge::new(pool, client(fake), route_key()).allow_workflow(WORKFLOW)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_delivery_gives_one_signal() {
    let Some(url) = database_url() else { return };
    migrated(&url).await;
    let pool = pool(&url);
    let workflow_id = format!("incident:{}", nonce());
    let started = start_run(&pool, &url, &workflow_id).await;
    let fake = fake();
    let route = SignalRoute::new(WORKFLOW, &workflow_id).unwrap();
    let key = tracked_issue(&fake, vec![route.property_entry(&route_key())]).await;
    let delivery = status_change(&fake, &key);
    let bridge = bridge(pool.clone(), &fake);

    assert_eq!(
        bridge.route(&delivery).await.unwrap(),
        vec![RouteOutcome::Signalled]
    );
    // Jira sends the same delivery again: one signal.
    assert_eq!(
        bridge.route(&delivery).await.unwrap(),
        vec![RouteOutcome::Duplicate]
    );
    bridge.deliver(Arc::new(delivery.clone())).await.unwrap();
    let exec_id = started.handle.exec_id();
    let mut conn = pool.get().await.unwrap();
    let signals = autumn_harvest::signal::load_pending_signals(&mut conn, exec_id)
        .await
        .unwrap();
    assert_eq!(signals.len(), 1);
    let signal = &signals[0];
    assert_eq!(signal.signal_name, SIGNAL);
    let payload: IssueSignal = serde_json::from_value(signal.payload.clone()).unwrap();
    assert_eq!(payload.delivery_id, delivery.meta.id);
    assert_eq!(payload.event, "jira:issue_updated");
    assert_eq!(payload.issue_key.as_ref(), Some(&key));
    assert_eq!(payload.status.unwrap().name, "In Progress");
    assert!(payload.timestamp_ms.is_some());
    let text = serde_json::to_string(&signal.payload).unwrap();
    assert!(!text.contains(DEFAULT_API_TOKEN), "{text}");
    let counters = bridge.counters();
    assert_eq!(
        (counters.deliveries, counters.signalled, counters.duplicate),
        (3, 1, 2)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_run_is_skipped() {
    let Some(url) = database_url() else { return };
    migrated(&url).await;
    let fake = fake();
    let route = SignalRoute::new(WORKFLOW, &format!("incident:{}", nonce())).unwrap();
    let key = tracked_issue(&fake, vec![route.property_entry(&route_key())]).await;
    let delivery = status_change(&fake, &key);
    let bridge = bridge(pool(&url), &fake);
    assert_eq!(
        bridge.route(&delivery).await.unwrap(),
        vec![RouteOutcome::NoRun]
    );
    // The sink answers Ok: the plugin gives 200.
    bridge.deliver(Arc::new(delivery)).await.unwrap();
    assert_eq!(bridge.counters().no_run, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finished_run_is_skipped() {
    let Some(url) = database_url() else { return };
    migrated(&url).await;
    let pool = pool(&url);
    let workflow_id = format!("incident:{}", nonce());
    let started = start_run(&pool, &url, &workflow_id).await;
    started
        .handle
        .cancel("the test ends the run")
        .await
        .unwrap();
    let fake = fake();
    let route = SignalRoute::new(WORKFLOW, &workflow_id).unwrap();
    let key = tracked_issue(&fake, vec![route.property_entry(&route_key())]).await;
    let delivery = status_change(&fake, &key);
    let bridge = bridge(pool.clone(), &fake);
    assert_eq!(
        bridge.route(&delivery).await.unwrap(),
        vec![RouteOutcome::Finished]
    );
    bridge.deliver(Arc::new(delivery)).await.unwrap();
    assert_eq!(bridge.counters().finished, 2);
    let mut conn = pool.get().await.unwrap();
    let signals = autumn_harvest::signal::load_pending_signals(&mut conn, started.handle.exec_id())
        .await
        .unwrap();
    assert!(signals.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_error_is_transient() {
    let fake = fake();
    let route = SignalRoute::new(WORKFLOW, "incident:closed-db").unwrap();
    let key = tracked_issue(&fake, vec![route.property_entry(&route_key())]).await;
    let delivery = status_change(&fake, &key);
    let bridge = bridge(pool(CLOSED_URL), &fake);
    let err = bridge.route(&delivery).await.unwrap_err();
    assert!(matches!(err, HandlerError::Transient(_)), "{err:?}");
    assert!(!err.to_string().contains("jira:jira"), "{err}");
    let err = bridge
        .deliver(Arc::new(delivery.clone()))
        .await
        .unwrap_err();
    assert!(matches!(err, HandlerError::Transient(_)), "{err:?}");
    assert_eq!(bridge.counters().transient, 2);

    // A Jira 503 on the property read is transient too. A 404 means no
    // property: nothing to route.
    fake.fault(FakeOp::GetProperty, Fault::Status(503));
    fake.fault(FakeOp::GetProperty, Fault::Status(503));
    fake.fault(FakeOp::GetProperty, Fault::Status(503));
    fake.fault(FakeOp::GetProperty, Fault::Status(503));
    fake.fault(FakeOp::GetProperty, Fault::Status(503));
    let err = bridge.route(&delivery).await.unwrap_err();
    assert!(matches!(err, HandlerError::Transient(_)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_tag_makes_no_db_call() {
    let fake = fake();
    let good = SignalRoute::new(WORKFLOW, "incident:A-1").unwrap();
    let other = SignalRoute::new("other_flow", "B-2").unwrap();
    let forged = json!({"wf": WORKFLOW, "id": "incident:A-9", "tag": good.tag(&route_key())});
    let key = tracked_issue(&fake, vec![forged, other.property_entry(&route_key())]).await;
    let delivery = status_change(&fake, &key);
    // Each DB call on this pool fails, so Ok means no DB call.
    let bridge = bridge(pool(CLOSED_URL), &fake);
    assert_eq!(
        bridge.route(&delivery).await.unwrap(),
        vec![RouteOutcome::BadTag, RouteOutcome::NotAllowed]
    );
    let counters = bridge.counters();
    assert_eq!((counters.bad_tag, counters.not_allowed), (1, 1));

    // An untracked issue, and an event with no status change, read nothing.
    let plain = client(&fake)
        .create_issue(&NewIssue::new(
            ProjectKey::parse("OPS").unwrap(),
            IssueTypeRef::Name("Task".into()),
            "Untracked",
        ))
        .await
        .unwrap();
    let _ = fake.take_deliveries();
    let untracked = status_change(&fake, &plain.key);
    let reads = fake.count(FakeOp::GetProperty);
    assert_eq!(bridge.route(&untracked).await.unwrap(), vec![]);
    fake.human_comment(key.as_str(), "a person writes", ANN)
        .unwrap();
    let comment = fake.take_deliveries().pop().unwrap();
    let parsed = parse_event(&comment.body).unwrap();
    let meta = DeliveryMeta::new(
        WebhookDeliveryId::parse(comment.id()).unwrap(),
        0,
        Flow::Primary,
    );
    assert_eq!(
        bridge.route(&Delivery::new(meta, parsed)).await.unwrap(),
        vec![]
    );
    assert_eq!(fake.count(FakeOp::GetProperty), reads, "no property read");
    assert_eq!(bridge.counters().skipped, 2);
    // The Debug output of the bridge hides the key.
    let debug = format!("{bridge:?}");
    assert!(!debug.contains("bridge-route-key"), "{debug}");
}

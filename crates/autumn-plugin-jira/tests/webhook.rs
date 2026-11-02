//! AC29 and AC30: the webhook route and the dispatch.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use autumn_jira_fake::FakeJira;
use autumn_plugin_jira::{Delivery, EventSink, HandlerError, JiraConfig, JiraEvent, JiraPlugin};
use autumn_web::AppState;
use autumn_web::test::{TestApp, TestClient};
use common::*;
use serde_json::{Value, json};

/// Records each delivery with its name, then gives the next scripted result.
#[derive(Clone)]
struct Scripted {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
    results: Arc<Mutex<Vec<Result<(), HandlerError>>>>,
}

impl Scripted {
    fn new(name: &'static str, log: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name,
            log: Arc::clone(log),
            results: Arc::default(),
        }
    }

    fn then(self, result: Result<(), HandlerError>) -> Self {
        self.results.lock().unwrap().push(result);
        self
    }
}

#[async_trait]
impl EventSink for Scripted {
    async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("{}:{}", self.name, delivery.meta.id));
        let mut results = self.results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }
}

fn log() -> Arc<Mutex<Vec<String>>> {
    Arc::default()
}

fn entries(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// A plugin with `webhook` settings changed by `edit`.
fn plugin_with(fake: &FakeJira, edit: impl FnOnce(&mut JiraConfig)) -> JiraPlugin {
    let mut config = JiraConfig::default();
    edit(&mut config);
    JiraPlugin::with_config(config).client(client_on(Arc::new(fake.clone())))
}

/// The `webhook` counters in `/actuator/health`.
async fn counters(client: &TestClient) -> Value {
    let r = client.get("/actuator/health").send().await;
    let body = json_of(&r);
    let jira = &body["components"]["jira"]["details"]["webhook"];
    assert!(jira.is_object(), "{body}");
    jira.clone()
}

fn fresh() -> Vec<u8> {
    issue_updated(Some(now_ms()), "acct-ann")
}

#[tokio::test(flavor = "multi_thread")]
async fn signed_is_200() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin(&fake).sink(Scripted::new("a", &log)));
    deliver(&client, "d-1", &fresh()).await.assert_status(200);
    assert_eq!(entries(&log), ["a:d-1"]);
    assert_eq!(counters(&client).await["accepted"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_signature_is_400() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin(&fake).sink(Scripted::new("a", &log)));
    client
        .post(PATH)
        .header("X-Atlassian-Webhook-Identifier", "d-2")
        .body(fresh())
        .send()
        .await
        .assert_status(400);
    assert_eq!(entries(&log), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_signature_is_401() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin(&fake).sink(Scripted::new("a", &log)));
    let other = format!(
        "sha256={}",
        autumn_web::webhook::hmac_sha256_hex(b"another-secret", &fresh())
    );
    client
        .post(PATH)
        .header("X-Hub-Signature", &other)
        .header("X-Atlassian-Webhook-Identifier", "d-3")
        .body(fresh())
        .send()
        .await
        .assert_status(401);
    assert_eq!(entries(&log), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_is_409_without_dispatch() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin(&fake).sink(Scripted::new("a", &log)));
    let body = fresh();
    deliver(&client, "d-4", &body).await.assert_status(200);
    deliver(&client, "d-4", &body).await.assert_status(409);
    assert_eq!(entries(&log), ["a:d-4"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn ignored_cases_are_200_and_counted() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin_with(&fake, |c| {
        c.webhook.ignore_actor_account_ids = vec!["acct-bot".to_owned()];
    })
    .sink(Scripted::new("a", &log)));
    let hour = 3_600_000;
    let cases: [(&str, Vec<u8>); 6] = [
        (
            "stale",
            issue_updated(Some(now_ms() - 3 * hour), "acct-ann"),
        ),
        ("future", issue_updated(Some(now_ms() + hour), "acct-ann")),
        ("no-ts", issue_updated(None, "acct-ann")),
        ("self", issue_updated(Some(now_ms()), "acct-bot")),
        (
            "unknown",
            json!({"webhookEvent": "sprint_started", "timestamp": now_ms()})
                .to_string()
                .into_bytes(),
        ),
        ("bad-json", b"{not json".to_vec()),
    ];
    for (id, body) in &cases {
        let r = deliver(&client, id, body).await;
        assert_eq!(r.status.as_u16(), 200, "{id}: {}", r.text());
    }
    assert_eq!(entries(&log), Vec::<String>::new());
    let c = counters(&client).await;
    for (name, want) in [
        ("ignored_stale", 1),
        ("ignored_future", 1),
        ("ignored_no_timestamp", 1),
        ("ignored_self", 1),
        ("ignored_unknown", 1),
        ("undecodable", 1),
        ("accepted", 0),
    ] {
        assert_eq!(c[name], want, "{name}: {c}");
    }
    // Inside the skew and the age: accepted.
    let near = issue_updated(Some(now_ms() + 60_000), "acct-ann");
    deliver(&client, "near", &near).await.assert_status(200);
    assert_eq!(entries(&log), ["a:near"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_is_503_then_redelivery_succeeds() {
    let fake = FakeJira::new();
    let log = log();
    let sink = Scripted::new("a", &log).then(Err(HandlerError::transient("db down")));
    let client = app(plugin(&fake).sink(sink));
    let body = fresh();
    deliver(&client, "d-5", &body).await.assert_status(503);
    // autumn frees the replay keys on a 5xx, so Jira can send it again.
    deliver(&client, "d-5", &body).await.assert_status(200);
    assert_eq!(entries(&log), ["a:d-5", "a:d-5"]);
    let c = counters(&client).await;
    assert_eq!(
        (c["handler_failed"].as_u64(), c["accepted"].as_u64()),
        (Some(1), Some(1)),
        "{c}"
    );
}

/// Sleeps, then sets `done`.
struct Slow {
    sleep: Duration,
    done: Arc<AtomicBool>,
}

#[async_trait]
impl EventSink for Slow {
    async fn deliver(&self, _delivery: Arc<Delivery>) -> Result<(), HandlerError> {
        tokio::time::sleep(self.sleep).await;
        self.done.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_is_503() {
    let fake = FakeJira::new();
    let log = log();
    let done = Arc::new(AtomicBool::new(false));
    let client = app(plugin_with(&fake, |c| c.webhook.handler_timeout_ms = 100)
        .sink(Slow {
            sleep: Duration::from_millis(400),
            done: Arc::clone(&done),
        })
        .sink(Scripted::new("after", &log)));
    deliver(&client, "d-6", &fresh()).await.assert_status(503);
    assert!(
        entries(&log).is_empty(),
        "a sink after a timeout does not run"
    );
    assert_eq!(counters(&client).await["timed_out"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn timed_out_sink_still_finishes() {
    let fake = FakeJira::new();
    let done = Arc::new(AtomicBool::new(false));
    let client = app(
        plugin_with(&fake, |c| c.webhook.handler_timeout_ms = 100).sink(Slow {
            sleep: Duration::from_millis(300),
            done: Arc::clone(&done),
        }),
    );
    deliver(&client, "d-7", &fresh()).await.assert_status(503);
    assert!(!done.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(done.load(Ordering::SeqCst), "the task runs to its end");
}

#[tokio::test(flavor = "multi_thread")]
async fn permanent_is_200() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin(&fake)
        .sink(Scripted::new("a", &log).then(Err(HandlerError::permanent("bad data"))))
        .sink(Scripted::new("b", &log)));
    deliver(&client, "d-8", &fresh()).await.assert_status(200);
    // A permanent error does not stop the later sinks.
    assert_eq!(entries(&log), ["a:d-8", "b:d-8"]);
    let c = counters(&client).await;
    assert_eq!(
        (c["permanent"].as_u64(), c["accepted"].as_u64()),
        (Some(1), Some(1)),
        "{c}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sinks_run_in_order() {
    let fake = FakeJira::new();
    let log = log();
    let closure_log = Arc::clone(&log);
    let seen_state = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&seen_state);
    let client = app(plugin(&fake)
        .sink(Scripted::new("first", &log))
        .on_event(move |delivery: Arc<Delivery>, state: AppState| {
            let log = Arc::clone(&closure_log);
            let seen = Arc::clone(&seen);
            async move {
                seen.store(
                    autumn_plugin_jira::JiraService::from_state(&state).is_some(),
                    Ordering::SeqCst,
                );
                assert!(matches!(delivery.event, JiraEvent::IssueUpdated(_)));
                log.lock()
                    .unwrap()
                    .push(format!("closure:{}", delivery.meta.id));
                Ok(())
            }
        })
        .sink(Scripted::new("third", &log)));
    for n in 0..3 {
        deliver(&client, &format!("o-{n}"), &fresh())
            .await
            .assert_status(200);
    }
    assert_eq!(
        entries(&log),
        [
            "first:o-0",
            "closure:o-0",
            "third:o-0",
            "first:o-1",
            "closure:o-1",
            "third:o-1",
            "first:o-2",
            "closure:o-2",
            "third:o-2"
        ]
    );
    assert!(
        seen_state.load(Ordering::SeqCst),
        "the closure gets the app state"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_stops_later_sinks() {
    let fake = FakeJira::new();
    let log = log();
    let client = app(plugin(&fake)
        .sink(Scripted::new("a", &log).then(Err(HandlerError::transient("busy"))))
        .sink(Scripted::new("b", &log)));
    deliver(&client, "d-9", &fresh()).await.assert_status(503);
    assert_eq!(entries(&log), ["a:d-9"]);
    // Jira sends it again. Each sink is at least once: `a` sees it twice.
    deliver(&client, "d-9", &fresh()).await.assert_status(200);
    assert_eq!(entries(&log), ["a:d-9", "a:d-9", "b:d-9"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_mib_body_passes() {
    let fake = FakeJira::new();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let client = app(plugin(&fake).on_event(move |_d, _s| {
        let seen = Arc::clone(&seen);
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }));
    let mut body: Value = serde_json::from_slice(&fresh()).unwrap();
    body["issue"]["fields"]["description_text"] = json!("x".repeat(2 * 1024 * 1024));
    let body = body.to_string().into_bytes();
    assert!(body.len() > 2 * 1024 * 1024);
    deliver(&client, "big-1", &body).await.assert_status(200);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn over_cap_is_400() {
    let fake = FakeJira::new();
    let log = log();
    let mut cfg = autumn_config();
    cfg.security.webhooks.endpoints[0].max_body_bytes = 1024 * 1024;
    let client = TestApp::new()
        .config(cfg)
        .plugin(plugin(&fake).sink(Scripted::new("a", &log)))
        .build();
    let mut body: Value = serde_json::from_slice(&fresh()).unwrap();
    body["pad"] = json!("x".repeat(2 * 1024 * 1024));
    deliver(&client, "big-2", &body.to_string().into_bytes())
        .await
        .assert_status(400);
    assert_eq!(entries(&log), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn csrf_on_still_passes() {
    let fake = FakeJira::new();
    let log = log();
    let mut cfg = autumn_config();
    cfg.security.csrf.enabled = true;
    let client = TestApp::new()
        .config(cfg)
        .plugin(plugin(&fake).sink(Scripted::new("a", &log)))
        .build();
    deliver(&client, "csrf-1", &fresh())
        .await
        .assert_status(200);
    assert_eq!(entries(&log), ["a:csrf-1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_route_reads_the_retry_and_flow_headers() {
    let fake = FakeJira::new();
    let seen: Arc<Mutex<Vec<(u32, String)>>> = Arc::default();
    let record = Arc::clone(&seen);
    let client = app(plugin(&fake).on_event(move |d: Arc<Delivery>, _s| {
        let record = Arc::clone(&record);
        async move {
            record
                .lock()
                .unwrap()
                .push((d.meta.retry, format!("{:?}", d.meta.flow)));
            Ok(())
        }
    }));
    let body = fresh();
    client
        .post(PATH)
        .header("X-Hub-Signature", &signature(&body))
        .header("X-Atlassian-Webhook-Identifier", "h-1")
        .header("X-Atlassian-Webhook-Retry", "2")
        .header("X-Atlassian-Webhook-Flow", "Secondary")
        .body(body.clone())
        .send()
        .await
        .assert_status(200);
    // autumn also keys its replay store on the signature, so the second
    // delivery has another body.
    let other = issue_updated(Some(now_ms() + 1), "acct-ann");
    deliver(&client, "h-2", &other).await.assert_status(200);
    assert_eq!(
        *seen.lock().unwrap(),
        [(2, "Secondary".to_owned()), (0, "Unknown".to_owned())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fake_delivery_reaches_the_sink() {
    let fake = FakeJira::new().with_webhook_secret(SECRET);
    let jira = client_on(Arc::new(fake.clone()));
    jira.create_issue(&autumn_plugin_jira::jira::NewIssue::new(
        autumn_plugin_jira::jira::ProjectKey::parse("OPS").unwrap(),
        autumn_plugin_jira::jira::request::IssueTypeRef::Name("Task".to_owned()),
        "Disk full",
    ))
    .await
    .unwrap();
    let log = log();
    let client = app(plugin(&fake).sink(Scripted::new("a", &log)));
    for d in fake.take_deliveries() {
        let mut request = client.post(PATH);
        for (k, v) in &d.headers {
            request = request.header(k, v);
        }
        request.body(d.body.clone()).send().await.assert_status(200);
    }
    assert_eq!(entries(&log).len(), 1, "{:?}", entries(&log));
}

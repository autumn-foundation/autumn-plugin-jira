//! The tests of the example (EX5, EX6, EX7). The house pattern: a
//! `#[cfg(test)]` module in `src`.
//!
//! A test that needs Postgres reads `JIRA_TEST_DATABASE_URL`. With no URL
//! it skips, but not in CI.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "tests: a failure is a test failure"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use autumn_jira::san_mateo::credentials::OAuth2Credentials;
use autumn_jira::san_mateo::{ConnectionKey, Credentials, ImportConnection, Tags};
use autumn_jira::{NewIssue, request::IssueTypeRef};
use autumn_jira_fake::{FakeJira, FakeProject};
use autumn_plugin_jira::JiraService;
use autumn_web::webhook::hmac_sha256_hex;
use serde_json::Map;
use tokio::sync::Mutex;

use super::*;
use crate::summary::EXPECTED;
use crate::transport::{JIRA_INTEGRATION, LEAD};
use crate::workflow::{ChatOutput, Outcome};

/// The pinned copy of `harvest_camel::is_retryable` and its samples, from
/// the camel crate. The test below compares it with the real function.
#[path = "../../../crates/autumn-jira-camel/tests/retry_rule.rs"]
mod retry_rule;

const DB_ENV: &str = "JIRA_TEST_DATABASE_URL";

/// One harvest scenario at a time. A harvest runner installs process-wide
/// state (the shard router and the dispatch slot), so two runners of one
/// process must not overlap.
static ONE_RUN: Mutex<()> = Mutex::const_new(());

/// The test database URL. With no URL, the test skips, but not in CI.
fn database_url() -> Option<String> {
    install_crypto_provider();
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

/// Runs a scenario on its own fakes.
async fn run_scenario(url: &str, scenario: &Scenario) -> (Result<Report, BoxError>, Sites) {
    let _one = ONE_RUN.lock().await;
    let sites = scenario_sites(scenario);
    (Box::pin(run(url, scenario, sites.clone())).await, sites)
}

// --- EX5: the happy path --------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_path_timeline_is_exact() {
    let Some(url) = database_url() else { return };
    let (report, sites) = Box::pin(run_scenario(&url, &Scenario::happy())).await;
    let report = report.expect("the happy path runs");
    check(&report.summary).unwrap_or_else(|e| panic!("{e}\n{}", report.summary));
    // The facts behind the lines.
    assert_eq!(sites.jira.count(FakeOp::CreateIssue), 1, "one POST");
    assert_eq!(sites.jira.issues().len(), 1, "one issue");
    assert_eq!(sites.jira.token_refreshes(), 1, "one token refresh");
    assert_eq!(sites.jira.count(FakeOp::AddComment), 1, "one comment");
    let chat = report.output.chat.as_ref().expect("a chat message");
    assert_eq!(chat.worker, "B", "worker B finished the run");
    let messages = sites.chat.requests_to(transport::CHAT_MESSAGES_URL);
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0]
            .header_value("x-api-key")
            .is_some_and(|k| k.starts_with("chat-key-")),
        "san-mateo adds the API key"
    );
    assert_eq!(report.output.lead.as_str(), LEAD);
    assert_eq!(report.webhook_statuses, vec![200]);
    assert_eq!(report.bridge.signalled, 1);
}

// --- EX6: the failures ----------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_create_fails_closed() {
    let Some(url) = database_url() else { return };
    // The search index lags more than the settle delay: the lookup of
    // attempt 2 cannot see the issue.
    let scenario = Scenario {
        index_lag: Duration::from_secs(30),
        ..Scenario::happy()
    };
    let (report, sites) = Box::pin(run_scenario(&url, &scenario)).await;
    let err = report.expect_err("the run fails closed").to_string();
    assert!(err.starts_with("workflow A-1001."), "{err}");
    assert!(err.contains("jira.outcome_unknown"), "{err}");
    // One POST: the activity did not send the create again.
    assert_eq!(sites.jira.count(FakeOp::CreateIssue), 1, "{err}");
    assert_eq!(sites.jira.issues().len(), 1, "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_webhook_secret_resolves_by_poll() {
    let Some(url) = database_url() else { return };
    let scenario = Scenario {
        app_secret: "another-webhook-secret-0123456789abcdef".to_owned(),
        poll_every_secs: Some(1),
        ..Scenario::happy()
    };
    let (report, _sites) = Box::pin(run_scenario(&url, &scenario)).await;
    let report = report.expect("a poll ends the wait");
    let lines = &report.summary.lines;
    assert_eq!(
        lines[6],
        "[7] autumn   POST /jira/webhook -> 401; bridge signalled 0 runs"
    );
    assert_eq!(
        lines[7],
        "[8] harvest  OPS-1 Done (source poll) -> comment (created) -> chat message 201"
    );
    assert_eq!(report.bridge.deliveries, 0, "no delivery passed autumn");
    let err = check(&report.summary).expect_err("this is not the happy path");
    assert!(err.starts_with("check failed: line 7"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_webhook_no_poll_times_out() {
    let Some(url) = database_url() else { return };
    let scenario = Scenario {
        person_resolves: false,
        wait_secs: 5,
        poll_every_secs: None,
        ..Scenario::happy()
    };
    let (report, sites) = Box::pin(run_scenario(&url, &scenario)).await;
    let report = report.expect("the run takes the timeout branch");
    assert_eq!(report.output.outcome, Outcome::TimedOut);
    assert!(report.output.chat.is_none(), "no chat message");
    assert_eq!(
        report.summary.lines[7],
        "[8] harvest  OPS-1 timed out -> escalation comment (created)"
    );
    assert_eq!(
        report.summary.lines[8],
        "[9] autumn   GET /incidents/OPS-1 -> OPS-1 In Progress"
    );
    let comment = sites.jira.snapshot().to_string();
    assert!(
        comment.contains("Escalation: no one resolved alert A-1001 in 5 s."),
        "{comment}"
    );
    assert_eq!(
        sites.chat.requests_to(transport::CHAT_MESSAGES_URL).len(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_timeout_still_stops_everything() {
    let Some(url) = database_url() else { return };
    // The run waits for Done, and nothing resolves the issue.
    let scenario = Scenario {
        person_resolves: false,
        wait_secs: 3600,
        poll_every_secs: None,
        ..Scenario::happy()
    };
    let limit = Duration::from_secs(20).saturating_mul(time_factor());
    let stops = StopLog::default();
    let err = {
        let _one = ONE_RUN.lock().await;
        let sites = scenario_sites(&scenario);
        Box::pin(run_within(&url, &scenario, sites, limit, &stops))
            .await
            .expect_err("the run times out")
            .to_string()
    };
    assert!(err.starts_with("the run did not end in"), "{err}");
    assert!(
        stops.all(),
        "the connector, the worker and camel stop: {stops:?}"
    );
    // The next run in this process passes.
    let (report, _sites) = Box::pin(run_scenario(&url, &Scenario::happy())).await;
    let report = report.expect("the happy path runs after a timeout");
    check(&report.summary).unwrap_or_else(|e| panic!("{e}\n{}", report.summary));
}

#[test]
fn a_wrong_value_fails_the_check() {
    let mut summary = Summary {
        lines: EXPECTED.iter().map(|l| (*l).to_owned()).collect(),
        one_chat_message_with_a_key: true,
    };
    check(&summary).expect("the expected timeline passes");
    summary.lines[3] = summary.lines[3].replace("(1 POST)", "(2 POSTs)");
    let err = check(&summary).expect_err("a second POST fails the check");
    assert!(err.starts_with("check failed: line 4"), "{err}");
    summary.lines[3] = EXPECTED[3].to_owned();
    summary.one_chat_message_with_a_key = false;
    let err = check(&summary).expect_err("a chat message with no key fails");
    assert!(err.starts_with("check failed:"), "{err}");
    summary.one_chat_message_with_a_key = true;
    summary.lines.pop();
    let err = check(&summary).expect_err("a missing line fails");
    assert!(err.starts_with("check failed: line 9"), "{err}");
}

#[test]
fn the_line_format_has_an_eight_wide_tag() {
    let mut summary = Summary::default();
    summary.line("camel", "a");
    summary.line("sanmateo", "b");
    assert_eq!(summary.lines, vec!["[1] camel    a", "[2] sanmateo b"]);
    assert_eq!(count_of(1_u64, "run", "runs"), "one run");
    assert_eq!(count_of(0_u64, "run", "runs"), "0 runs");
}

// --- EX7: the edges -------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webhook_negatives() {
    let Some(url) = database_url() else { return };
    let scenario = Scenario {
        webhook_negatives: true,
        ..Scenario::happy()
    };
    let (report, _sites) = Box::pin(run_scenario(&url, &scenario)).await;
    let report = report.expect("the run ends");
    // The delivery, the same delivery again, a tampered copy, a copy from
    // the bot.
    assert_eq!(report.webhook_statuses, vec![200, 409, 401, 200]);
    // The bridge is a sink for all actors (R2-webhook-security-5): a status
    // that the bot sets must signal a waiting run too. So the copy from the
    // bot gets to the bridge. It has its own delivery id. The run takes it
    // as a late signal (`drain_jira_signals`), or the run is already
    // finished.
    assert_eq!(report.bridge.deliveries, 2, "{:?}", report.bridge);
    assert!(report.bridge.signalled >= 1, "{:?}", report.bridge);
    assert_eq!(
        report.bridge.signalled + report.bridge.finished,
        2,
        "{:?}",
        report.bridge
    );
    assert_eq!(
        report.webhook_counters["accepted"], 2,
        "{}",
        report.webhook_counters
    );
    assert_eq!(
        report.webhook_counters["ignored_self"], 0,
        "{}",
        report.webhook_counters
    );
    assert_eq!(
        report.summary.lines[7],
        "[8] harvest  OPS-1 Done (source signal) -> comment (created) -> chat message 201"
    );
}

/// The second tenant: its own site, cloud id and tokens.
const GLOBEX_CLOUD_ID: &str = "99999999-8888-4777-8666-555555555555";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_tenants_stay_apart() {
    install_crypto_provider();
    // Two sites. The fake of acme sends the calls of the other site to the
    // fake of globex.
    let globex = FakeJira::new()
        .with_site("globex", GLOBEX_CLOUD_ID)
        .with_project(FakeProject::new("OPS", "20000", "Globex ops").issue_type("20001", "Task"))
        .with_oauth("ops-client", CLIENT_SECRET, "globex-at", "globex-rt");
    let sites = Sites::new(WEBHOOK_SECRET, CLIENT_SECRET);
    let sites = Sites {
        jira: sites.jira.with_fallback(Arc::new(globex.clone())),
        chat: sites.chat,
    };
    let san = transport::san_mateo(&sites, CLIENT_SECRET).await.unwrap();
    san.import_connection(
        ENV,
        ImportConnection {
            connection_id: Some("globex".to_owned()),
            provider_config_key: JIRA_INTEGRATION.to_owned(),
            credentials: Credentials::OAuth2(OAuth2Credentials {
                access_token: Secret::new("globex-at"),
                refresh_token: Some(Secret::new("globex-rt")),
                expires_at: None,
                raw: json!({}),
            }),
            connection_config: Map::new(),
            metadata: Map::new(),
            tags: Tags::new(),
            end_user: None,
            webhook_url_override: None,
        },
    )
    .await
    .unwrap();
    // The app serves both tenants with one plugin: `for_connection(id)`.
    let pool = engine_pool(
        "postgres://unused@127.0.0.1:1/unused",
        1,
        DbRole::Hot,
        &EngineDbTimeouts::default(),
    )
    .unwrap();
    let key = RouteKey::new(Secret::new("route-key-0123456789abcdef0123456789")).unwrap();
    let bridge = HarvestSignalBridge::new(pool, transport::jira_client(&san), key);
    let app = app::test_app(&san, bridge, WEBHOOK_SECRET).unwrap();
    let jira = JiraService::require(app.state()).unwrap();
    let mut keys = Vec::new();
    for (tenant, summary) in [("acme", "Acme disk full"), ("globex", "Globex disk full")] {
        let client = jira.for_connection(tenant).unwrap();
        let made = client
            .create_issue(&NewIssue::new(
                autumn_jira::ProjectKey::parse(PROJECT).unwrap(),
                IssueTypeRef::Name("Task".to_owned()),
                summary,
            ))
            .await
            .unwrap();
        keys.push(made.key.to_string());
    }
    // Each site has its own issue OPS-1, and only its own.
    assert_eq!(keys, vec!["OPS-1", "OPS-1"]);
    let summaries = |fake: &FakeJira| -> Vec<String> {
        fake.issues()
            .iter()
            .map(|i| i["fields"]["summary"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(summaries(&sites.jira), vec!["Acme disk full"]);
    assert_eq!(summaries(&globex), vec!["Globex disk full"]);
    // Each connection stores its own cloud id.
    let cloud_id = |id: &str| {
        let san = san.clone();
        let key = ConnectionKey::new(JIRA_INTEGRATION, id);
        async move {
            let options = autumn_jira::san_mateo::GetOptions {
                refresh_if_needed: false,
                ..autumn_jira::san_mateo::GetOptions::default()
            };
            let conn = san.get_connection(ENV, &key, options).await.unwrap();
            conn.metadata["cloudId"].as_str().unwrap().to_owned()
        }
    };
    assert_eq!(cloud_id("acme").await, autumn_jira_fake::DEFAULT_CLOUD_ID);
    assert_eq!(cloud_id("globex").await, GLOBEX_CLOUD_ID);
    // The app route reads the issue of the default tenant, acme.
    let reply = app.get("/incidents/OPS-1").send().await;
    assert_eq!(reply.status.as_u16(), 200);
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(body, json!({"key": "OPS-1", "status": "To Do"}));
}

#[test]
fn is_retryable_copy_matches_harvest_camel() {
    let errors = retry_rule::every_error();
    assert!(errors.len() > 500, "{}", errors.len());
    for err in &errors {
        for operation in autumn_jira_camel::OPERATIONS {
            let mapped = autumn_jira_camel::to_camel_error(err, operation);
            assert_eq!(
                retry_rule::is_retryable(&mapped),
                harvest_camel::is_retryable(&mapped),
                "{err:?} via {operation} gave {mapped:?}"
            );
            // The real rule also gives the retry class of the client.
            assert_eq!(
                harvest_camel::is_retryable(&mapped),
                err.is_retryable(),
                "{err:?} via {operation}"
            );
        }
    }
}

// --- EX7: the three plugins boot with run() -------------------------------

const CHILD_MARK: &str = "ECOSYSTEM_BOOT_CHILD";

/// The server of the boot test. It runs only in the child process, with
/// `run()`: `SanMateoPlugin`, `HarvestPlugin` (external mode) and
/// `JiraPlugin`. Its Jira is `FakeJira`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs only as the child of boot_three_plugins_real_http"]
async fn boot_child_server() {
    if std::env::var(CHILD_MARK).is_err() {
        return;
    }
    install_crypto_provider();
    let url = std::env::var(DB_ENV).unwrap();
    let sites = Sites::new(WEBHOOK_SECRET, CLIENT_SECRET);
    let san = transport::san_mateo(&sites, CLIENT_SECRET).await.unwrap();
    let pool = engine_pool(&url, 4, DbRole::Hot, &EngineDbTimeouts::default()).unwrap();
    let key = RouteKey::new(Secret::new(format!("route-key-{}", OsEntropy.hex(16)))).unwrap();
    let jira = transport::jira_client(&san);
    let bridge =
        HarvestSignalBridge::new(pool, jira.clone(), key.clone()).allow_workflow(INCIDENT_FLOW);
    let queue = format!("ecosystem-boot-{}", OsEntropy.hex(8));
    let mut activities = autumn_jira_harvest::activities();
    activities.push(san_mateo_harvest::san_mateo_proxy_info());
    let harvest = autumn_harvest_plugin::HarvestPlugin::new()
        .workflows(vec![incident_flow_info()])
        .activities(activities)
        .state(JiraClients::new(jira))
        .state(key)
        .state(san.clone())
        .worker(WorkerConfig::default().with_queues([queue.as_str()]));
    Box::pin(
        autumn_web::app()
            .routes(autumn_web::routes![app::incident])
            .plugin(app::san_mateo_plugin(&san).unwrap())
            .plugin(harvest)
            .plugin(app::jira_plugin(bridge).unwrap())
            .run(),
    )
    .await;
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// A folder with the `autumn.toml` of the app, plus the webhook endpoint
/// and the harvest mode.
fn app_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ecosystem-boot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let toml = format!(
        "{}\n{}\n[harvest]\nmode = \"external\"\n",
        app::AUTUMN_TOML,
        autumn_plugin_jira::webhook_endpoint_toml(app::WEBHOOK_PATH, app::WEBHOOK_SECRET_ENV)
    );
    std::fs::write(dir.join("autumn.toml"), toml).unwrap();
    dir
}

/// Starts the child with a clean env, so the `AUTUMN_*` vars, `.env` or
/// `autumn.toml` of a developer cannot change the result.
fn spawn(dir: &Path, port: u16, url: &str) -> KillOnDrop {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "--exact",
        "tests::boot_child_server",
        "--include-ignored",
        "--nocapture",
    ])
    .env_clear()
    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
    .env("AUTUMN_MANIFEST_DIR", dir)
    .current_dir(dir)
    .env(CHILD_MARK, "1")
    .env(DB_ENV, url)
    .env("AUTUMN_HARVEST_DATABASE__URL", url)
    .env(app::WEBHOOK_SECRET_ENV, WEBHOOK_SECRET)
    .env("AUTUMN_SERVER__PORT", port.to_string())
    .env("AUTUMN_SERVER__HOST", "127.0.0.1");
    // A file, not a pipe: a full pipe blocks the logs of the child, and
    // then its server stops.
    let log = std::fs::File::create(dir.join("child.log")).unwrap();
    cmd.stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log));
    KillOnDrop(cmd.spawn().unwrap())
}

/// The log of the child.
fn output_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("child.log")).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_three_plugins_real_http() {
    let Some(url) = database_url() else { return };
    // The harvest tables exist before the child boots.
    migrate_once(&url).await.unwrap();
    let port = free_port();
    let dir = app_dir();
    let mut child = spawn(&dir, port, &url);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let start = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("the app stopped at boot ({status}):\n{}", output_of(&dir));
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "the app did not answer"
        );
        // `/ready` answers 200 when the start is done. Before that, autumn
        // answers 503 "still starting up".
        if let Ok(res) = client.get(format!("{base}/ready")).send().await
            && res.status().as_u16() == 200
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // One signed delivery over real HTTP: a status change of an issue that
    // no run tracks. The bridge reads its routes, finds none, and the route
    // answers 200.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let body = json!({
        "timestamp": u64::try_from(ts).unwrap(),
        "webhookEvent": "jira:issue_updated",
        "user": {"accountId": ANN},
        "issue": {"id": "10001", "key": "OPS-1", "fields": {"summary": "Disk full"}},
        "changelog": {"id": "1", "items": [{
            "field": "status", "fromString": "To Do", "toString": "Done",
            "from": "10000", "to": "10002"
        }]},
    })
    .to_string();
    let signature = format!(
        "sha256={}",
        hmac_sha256_hex(WEBHOOK_SECRET.as_bytes(), body.as_bytes())
    );
    let res = client
        .post(format!("{base}{}", app::WEBHOOK_PATH))
        .header("X-Hub-Signature", signature)
        .header("X-Atlassian-Webhook-Identifier", "boot-1")
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let reply = res.text().await.unwrap_or_default();
    let health = client
        .get(format!("{base}/actuator/health"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let health: Value = serde_json::from_str(&health).unwrap_or(Value::Null);
    drop(child);
    let log = output_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(status, 200, "{reply}\n{health}\n{log}");
    let components = &health["components"];
    for name in ["jira", "san_mateo"] {
        assert!(components.get(name).is_some(), "no {name} in {health}");
    }
}

// --- R4-harvest-determinism-2: a Jira signal after the wait ---------------

/// An in-memory env with a mock for each activity of `incident_flow`. The
/// issue is Done at the first read of the wait, so the wait ends at once.
fn flow_env() -> autumn_harvest::testing::WorkflowTestEnv {
    use autumn_jira_harvest::{
        GET_ISSUE, jira_add_comment_info, jira_create_issue_info, jira_transition_to_status_info,
    };
    let in_progress = json!({"id": "3", "name": "In Progress", "category": "indeterminate"});
    let done = json!({"id": "4", "name": "Done", "category": "done"});
    let issue = json!({
        "id": "10001",
        "key": "OPS-1",
        "summary": "Disk full",
        "status": done,
        "labels": ["autumn-tracked"],
        "updated": null
    });
    let created = json!({"id": "10001", "key": "OPS-1", "outcome": "created"});
    let moved = json!({"outcome": "moved", "status": in_progress, "via": "21"});
    let comment = json!({"id": "20001", "outcome": "created"});
    let chat = serde_json::to_value(ChatOutput {
        status: 201,
        worker: "A".to_owned(),
    })
    .unwrap();
    let project = json!({"status": 200, "headers": [], "body": {"lead": {"accountId": LEAD}}});
    autumn_harvest::testing::WorkflowTestEnv::new()
        .with_workflow_id(format!("{ALERT}.sim"))
        .mock_activity(san_mateo_harvest::san_mateo_proxy_info().name, move |_| {
            Ok(project.clone())
        })
        .mock_activity(jira_create_issue_info().name, move |_| Ok(created.clone()))
        .mock_activity(jira_transition_to_status_info().name, move |_| {
            Ok(moved.clone())
        })
        .mock_activity(GET_ISSUE, move |_| Ok(issue.clone()))
        .mock_activity(jira_add_comment_info().name, move |_| Ok(comment.clone()))
        .mock_activity(notify_chat_info().name, move |_| Ok(chat.clone()))
}

/// The input of the in-memory run: no poll, a wait of 600 s.
fn flow_input() -> Value {
    serde_json::to_value(AlertInput {
        alert_id: ALERT.to_owned(),
        summary: "Disk full".to_owned(),
        cloud_id: autumn_jira_fake::DEFAULT_CLOUD_ID.to_owned(),
        settle_secs: 3,
        wait_secs: 600,
        poll_every_secs: None,
    })
    .unwrap()
}

/// A webhook can come after the wait, while the run adds its comment or
/// tells the chat. Each seed sends the Done signal at another task
/// boundary. Every history must replay: the run takes a late signal with
/// `drain_jira_signals` before it returns. A late signal is a signal after
/// the schedule of the comment, which comes after the wait.
#[tokio::test]
async fn a_late_jira_signal_replays() {
    use autumn_harvest::WorkflowEvent;
    use autumn_harvest::dst::SeedPlan;
    use autumn_jira_harvest::{IssueSignal, SIGNAL, jira_add_comment_info};
    // Jira sends a delivery again until it gets a 2xx, and each copy from
    // another actor has its own id. So more than one signal can come.
    let done = |n: u32| {
        serde_json::to_value(IssueSignal {
            delivery_id: autumn_jira::ids::WebhookDeliveryId::parse(&format!("acme-{n}")).unwrap(),
            event: "jira:issue_updated".to_owned(),
            issue_id: autumn_jira::IssueId::parse("10001").unwrap(),
            issue_key: None,
            status: Some(
                serde_json::from_value(json!({"id": "4", "name": "Done", "category": "done"}))
                    .unwrap(),
            ),
            timestamp_ms: Some(9_000_000_000_000),
        })
        .unwrap()
    };
    let env = flow_env();
    let mut late = 0;
    let mut sim = env.simulation().seeds(SeedPlan {
        first: 0,
        count: 64,
    });
    for n in 1..=4 {
        sim = sim.signal(SIGNAL, done(n));
    }
    let report = sim
        .sweep(incident_flow_info().handler, flow_input(), |run| {
            let output: IncidentOutput =
                serde_json::from_value(run.result().clone().expect("the run ends")).unwrap();
            assert_eq!(output.outcome, Outcome::Resolved, "seed {}", run.seed());
            let events = run.events();
            let at = |hit: &dyn Fn(&WorkflowEvent) -> bool| events.iter().position(hit);
            let comment = at(&|e| {
                matches!(e, WorkflowEvent::ActivityScheduled { name, .. }
                    if name == jira_add_comment_info().name)
            });
            let signal = events
                .iter()
                .rposition(|e| matches!(e, WorkflowEvent::SignalReceived { .. }));
            if signal > comment {
                late += 1;
            }
        })
        .await;
    if let Err(failure) = report {
        panic!("{failure}");
    }
    // Some seeds send the signal after the end of the wait.
    assert!(late > 0, "no seed sent the signal late");
}

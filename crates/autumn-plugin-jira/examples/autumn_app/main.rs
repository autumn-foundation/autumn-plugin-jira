//! An autumn-web app with the Jira plugin.
//!
//! ```text
//! Jira  ─► POST /jira/webhook      autumn checks the signature ─► LogSink
//! you   ─► GET  /issues/{key}      your route ─► JiraService ─► Jira
//! ```
//!
//! One `.plugin(..)` call adds Jira. `autumn.toml` (next to this file) holds
//! the names of the env vars with the secrets, never the secrets. The app
//! uses `FakeJira`, an in-memory Jira, unless `JIRA_SITE`, `JIRA_EMAIL` and
//! `JIRA_API_TOKEN` are all set. Then it calls your Jira Cloud site with a
//! `ReqwestTransport`.
//!
//! Run it from the repository root. autumn reads `autumn.toml` from
//! `AUTUMN_MANIFEST_DIR`:
//!
//! ```sh
//! export AUTUMN_MANIFEST_DIR=crates/autumn-plugin-jira/examples/autumn_app
//! export JIRA_WEBHOOK_SECRET="$(openssl rand -hex 32)"
//! cargo run --locked -p autumn-plugin-jira --features testing --example autumn_app
//! curl http://127.0.0.1:3000/issues/OPS-1
//! ```
//!
//! A release build starts in the prod profile, a debug build in the dev
//! profile. `AUTUMN_ENV` selects a different profile. The prod profile of
//! autumn-web also needs `AUTUMN_SECURITY__SIGNING_SECRET` and
//! `AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS`, and a webhook secret of 32 bytes
//! or more. `autumn-prod.toml` keeps the replay store in memory, which is
//! correct for one replica only.
//!
//! In Jira, add a webhook to `https://<your host>/jira/webhook` with the
//! same secret, and keep `excludeBody: false`. Webhooks are at least once:
//! a sink can get the same delivery again, so make it idempotent.
//!
//! `scripts/smoke-autumn-example.sh` boots this app in the dev and prod
//! profiles and checks it. The tests below run the same routes and the
//! `[jira]` section of the same `autumn.toml` in `TestApp`.

use std::sync::Arc;

use async_trait::async_trait;
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira};
use autumn_plugin_jira::jira::ids::SiteName;
use autumn_plugin_jira::jira::request::IssueTypeRef;
use autumn_plugin_jira::jira::san_mateo::Secret;
use autumn_plugin_jira::jira::{BasicAuth, JiraSite, NewIssue, ProjectKey};
use autumn_plugin_jira::{
    Delivery, EventSink, HandlerError, IssueKey, JiraClient, JiraConfig, JiraPlugin, JiraResultExt,
    JiraService,
};
use autumn_web::{AutumnResult, Json, Path};
use serde_json::{Value, json};

/// The env vars that select a real Jira site.
const REAL_ENV: [&str; 3] = ["JIRA_SITE", "JIRA_EMAIL", "JIRA_API_TOKEN"];

#[autumn_web::get("/")]
async fn index() -> &'static str {
    "autumn_app: GET /issues/{key}, POST /jira/webhook"
}

/// Your route: the service as an extractor, and `.http()?` for the error.
/// A bad key gives 400 (strict `IssueKey`), a missing issue 404.
#[autumn_web::get("/issues/{key}")]
async fn issue(jira: JiraService, Path(key): Path<IssueKey>) -> AutumnResult<Json<Value>> {
    let _ = (jira, key);
    Err(autumn_web::AutumnError::internal_server_error_msg(
        "not yet",
    ))
}

/// A sink that logs the delivery id and the event name, never the body.
struct LogSink;

#[async_trait]
impl EventSink for LogSink {
    async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError> {
        tracing::info!(
            delivery_id = %delivery.meta.id,
            event = %delivery.event,
            "jira event"
        );
        Ok(())
    }
}

/// The fake Jira of the demo, with the issue `OPS-1`.
async fn seeded_fake() -> Result<FakeJira, String> {
    let fake = FakeJira::new();
    let site = SiteName::parse("acme").map_err(|e| e.to_string())?;
    let auth =
        BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN)).map_err(|e| e.to_string())?;
    let client = JiraClient::api_token(JiraSite::Subdomain(site), auth, Arc::new(fake.clone()));
    let project = ProjectKey::parse("OPS").map_err(|e| e.to_string())?;
    client
        .create_issue(&NewIssue::new(
            project,
            IssueTypeRef::Name("Task".to_owned()),
            "Disk full on db-1",
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(fake)
}

/// The plugin: on the real site when `JIRA_SITE`, `JIRA_EMAIL` and
/// `JIRA_API_TOKEN` are set, else on the seeded fake. It also returns the
/// mode name for the summary.
async fn plugin() -> Result<(JiraPlugin, &'static str), String> {
    let real = REAL_ENV
        .iter()
        .all(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()));
    if real {
        let mut config =
            JiraConfig::from_toml_str(include_str!("autumn.toml")).map_err(|e| e.to_string())?;
        if let (Some(api), Ok(site)) = (config.api_token.as_mut(), std::env::var("JIRA_SITE")) {
            api.site = Some(site);
        }
        return Ok((JiraPlugin::with_config(config).sink(LogSink), "real"));
    }
    let fake = seeded_fake().await?;
    let plugin = JiraPlugin::new()
        .transport(Arc::new(fake))
        .secret_env("JIRA_EMAIL", DEFAULT_EMAIL)
        .secret_env("JIRA_API_TOKEN", DEFAULT_API_TOKEN)
        .sink(LogSink);
    Ok((plugin, "fake"))
}

// `autumn_web::main` gives autumn the build mode, so a release build starts
// in the prod profile.
#[autumn_web::main]
#[allow(
    clippy::print_stderr,
    reason = "main prints its summary and its errors"
)]
async fn main() {
    let (plugin, jira) = match plugin().await {
        Ok(found) => found,
        Err(e) => {
            eprintln!("autumn_app: the start failed: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("autumn_app: jira = {jira}; routes: GET /, GET /issues/{{key}}, POST /jira/webhook");
    // `run` reads `[jira]` from autumn.toml. A missing secret env var stops
    // the boot with a non-zero exit, and the error names the variable.
    // `run` stops on SIGTERM or Ctrl-C.
    Box::pin(
        autumn_web::app()
            .routes(autumn_web::routes![index, issue])
            .plugin(plugin)
            .run(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use autumn_jira_fake::{FakeOp, Fault};
    use autumn_plugin_jira::testing::{DeliverExt, app_config};
    use autumn_web::test::{TestApp, TestClient, TestResponse};

    use super::*;

    type BoxError = Box<dyn std::error::Error + Send + Sync>;

    /// The longest time for the test scenario.
    const RUN_TIMEOUT: Duration = Duration::from_secs(60);

    /// Runs the app in `TestApp` with the same routes and `[jira]` config as
    /// `main`, drives it over HTTP and checks the result.
    async fn run(fake: FakeJira) -> Result<Value, BoxError> {
        let config = JiraConfig::from_toml_str(include_str!("autumn.toml"))?;
        let fake = fake.with_webhook_secret(autumn_plugin_jira::testing::TEST_WEBHOOK_SECRET);
        let mut settings = config.http.clone();
        settings.base_backoff_ms = 10;
        let mut config = config;
        config.http = settings;
        let plugin = JiraPlugin::with_config(config)
            .transport(Arc::new(fake.clone()))
            .secret_env("JIRA_EMAIL", DEFAULT_EMAIL)
            .secret_env("JIRA_API_TOKEN", DEFAULT_API_TOKEN)
            .sink(LogSink);
        let client = TestApp::new()
            .config(app_config())
            .routes(autumn_web::routes![index, issue])
            .plugin(plugin)
            .build();
        let summary = tokio::time::timeout(RUN_TIMEOUT, drive(&client, &fake))
            .await
            .map_err(|_| format!("the scenario did not end in {RUN_TIMEOUT:?}"))??;
        let text = summary.to_string();
        if text.contains(DEFAULT_API_TOKEN) || text.contains(DEFAULT_EMAIL) {
            return Err("the summary shows a secret".into());
        }
        check(&summary)?;
        Ok(summary)
    }

    /// A person and Jira: they call the app over HTTP.
    async fn drive(client: &TestClient, fake: &FakeJira) -> Result<Value, BoxError> {
        let jira = JiraService::require(client.state()).map_err(|e| e.message())?;
        let created = jira
            .create_issue(&NewIssue::new(
                ProjectKey::parse("OPS")?,
                IssueTypeRef::Name("Task".to_owned()),
                "Disk full on db-1",
            ))
            .await?;
        let got = client.get(&format!("/issues/{}", created.key)).send().await;
        let missing = client.get("/issues/OPS-999").send().await;
        fake.human_transition(created.key.as_str(), "In Progress", "ann")?;
        let mut webhooks = Vec::new();
        for delivery in fake.take_deliveries() {
            webhooks.push(client.deliver(&delivery).await.status.as_u16());
        }
        let health = client.get("/actuator/health").send().await;
        let health_body = body(&health)?;
        Ok(json!({
            "issue": {"status": got.status.as_u16(), "body": body(&got).unwrap_or(Value::Null)},
            "missing_status": missing.status.as_u16(),
            "webhooks": webhooks,
            "health": {
                "jira": health_body["components"]["jira"]["status"],
                "accepted": health_body["components"]["jira"]["details"]["webhook"]["accepted"],
            },
        }))
    }

    fn body(r: &TestResponse) -> Result<Value, BoxError> {
        Ok(serde_json::from_slice(&r.body)?)
    }

    /// Compares the summary with the expected values. The error starts with
    /// `check failed:`.
    fn check(summary: &Value) -> Result<(), String> {
        let expected = json!({
            "issue": {"status": 200, "body": {"key": "OPS-1", "summary": "Disk full on db-1", "status": "To Do"}},
            "missing_status": 404,
            "webhooks": [200, 200],
            "health": {"jira": "UP", "accepted": 2},
        });
        if *summary == expected {
            Ok(())
        } else {
            Err(format!(
                "check failed: the summary {summary} is not {expected}"
            ))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_app_serves_issues_and_webhooks() {
        let summary = run(FakeJira::new()).await.expect("the example passes");
        check(&summary).expect("run checks the summary");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wrong_value_fails_the_check() {
        let fake = FakeJira::new();
        for _ in 0..5 {
            fake.fault(FakeOp::GetIssue, Fault::Status(503));
        }
        let err = run(fake).await.expect_err("the check finds the error");
        let err = err.to_string();
        assert!(err.starts_with("check failed:"), "{err}");
        assert!(err.contains(r#""status":502"#), "{err}");
    }

    #[tokio::test]
    async fn the_demo_fake_has_the_issue() {
        let fake = seeded_fake().await.expect("the seed works");
        assert!(fake.issue("OPS-1").is_some());
        let (_, mode) = plugin().await.expect("the plugin builds");
        assert_eq!(mode, "fake");
    }
}
